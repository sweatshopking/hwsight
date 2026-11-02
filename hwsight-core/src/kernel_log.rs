//! Reasons for device problems, read from the kernel log.
//!
//! When a driver can't start or a firmware file is missing, the kernel says why in its
//! log. Messages about a device start with the driver and device name, for example
//! `r8169 0000:07:00.0: Direct firmware load for rtl_nic/rtl8125b-2.fw failed with error -2`.
//!
//! We read the log with `journalctl` (systemd distros) or `dmesg` (everything else).
//! Either may be off-limits to normal users on some distros; then we say so and carry on.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

/// Something the kernel log says went wrong with a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogIssue {
    /// The driver asked for a firmware file that wasn't there.
    MissingFirmware {
        file: String,
        error: i32,
        /// The file exists now (installed since boot), so a restart may fix it.
        installed_now: bool,
        /// The package that provides it on this distro, when known.
        package: Option<String>,
        /// How to install that package here, for showing to the user.
        install_command: Option<String>,
    },
    /// The driver tried to start the device and gave up.
    DriverFailed { driver: String, error: i32 },
    /// Something plugged into a USB port couldn't connect. `error` is 0 when the kernel
    /// didn't give one.
    UsbConnectFailed { error: i32 },
    /// A USB port reported drawing too much power.
    UsbOverCurrent,
}

impl LogIssue {
    pub fn message(&self) -> String {
        match self {
            Self::MissingFirmware {
                file,
                installed_now: true,
                ..
            } => format!(
                "Firmware file {file} was missing when the computer started, but it's installed \
                 now. Restarting should fix this."
            ),
            Self::MissingFirmware {
                file,
                package: Some(package),
                ..
            } => format!(
                "The driver needs a firmware file that isn't installed: {file}. \
                 It comes in the {package} package."
            ),
            Self::MissingFirmware { file, .. } => {
                format!("The driver needs a firmware file that isn't installed: {file}")
            }
            Self::UsbConnectFailed { error } => {
                let why = if *error == 0 {
                    String::new()
                } else {
                    format!(" ({})", errno_meaning(*error))
                };
                format!(
                    "A device plugged in here couldn't connect{why}. This is usually a bad \
                     cable, a faulty device, or not enough power. Try another cable or port."
                )
            }
            Self::UsbOverCurrent => "This USB port reported drawing too much power. The device \
                plugged into it may be faulty or need its own power supply."
                .to_owned(),
            Self::DriverFailed { driver, error } => format!(
                "The {driver} driver couldn't start this device ({}).",
                errno_meaning(*error)
            ),
        }
    }
}

/// Where the log came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogSource {
    Journal,
    Dmesg,
}

/// Read this boot's kernel warnings and errors. `None` if no method works.
pub fn read_system_log() -> Option<(LogSource, Vec<String>)> {
    // -p warning: firmware and probe failures are logged as warnings or errors, and this
    // keeps the amount we read small.
    let journal = ["-k", "-b", "-q", "--no-pager", "-o", "cat", "-p", "warning"];
    if let Some(lines) = run_for_lines("journalctl", &journal) {
        if !lines.is_empty() {
            return Some((LogSource::Journal, lines));
        }
        // No warnings could mean a healthy system or no permission to read the log.
        // Every booted kernel has logged something, so one line of any level proves access.
        let probe = ["-k", "-b", "-q", "--no-pager", "-o", "cat", "-n", "1"];
        if run_for_lines("journalctl", &probe).is_some_and(|l| !l.is_empty()) {
            return Some((LogSource::Journal, lines));
        }
    }
    // dmesg has no reliable way to filter by level on every version, so read it all.
    // Readable dmesg output is never empty, so empty means no permission.
    run_for_lines("dmesg", &[])
        .filter(|lines| !lines.is_empty())
        .map(|lines| (LogSource::Dmesg, lines))
}

/// Run a command and return its output lines, or `None` if it fails or takes longer
/// than a few seconds (a huge journal must never hang the app).
fn run_for_lines(program: &str, args: &[&str]) -> Option<Vec<String>> {
    const TIMEOUT: Duration = Duration::from_secs(5);
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < TIMEOUT => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let output = reader.join().ok()?;
    if !status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output)
            .lines()
            .map(strip_dmesg_timestamp)
            .map(str::to_owned)
            .collect(),
    )
}

/// dmesg lines start with `[    1.234567] `.
fn strip_dmesg_timestamp(line: &str) -> &str {
    if line.starts_with('[')
        && let Some(end) = line.find("] ")
    {
        return &line[end + 2..];
    }
    line
}

/// Problems per device, keyed by device name as the kernel writes it (for PCI, the
/// address such as `0000:07:00.0`). Repeats of the same problem are listed once.
pub fn issues_by_device(
    lines: &[String],
    firmware_dirs: &[PathBuf],
) -> HashMap<String, Vec<LogIssue>> {
    let mut found: HashMap<String, Vec<LogIssue>> = HashMap::new();
    // Saved dmesg output keeps its timestamps; journal lines have none.
    for line in lines.iter().map(|l| strip_dmesg_timestamp(l)) {
        let Some((device, issue)) = parse_line(line, firmware_dirs) else {
            continue;
        };
        let list = found.entry(device).or_default();
        if !list.contains(&issue) {
            list.push(issue);
        }
    }
    found
}

fn parse_line(line: &str, firmware_dirs: &[PathBuf]) -> Option<(String, LogIssue)> {
    // Old driver core format, without a device prefix:
    // "r8169: probe of 0000:07:00.0 failed with error -5"
    if let Some((driver, rest)) = line.split_once(": probe of ")
        && !driver.contains(' ')
        && let Some((device, error)) = rest.split_once(" failed with error ")
    {
        let issue = LogIssue::DriverFailed {
            driver: driver.to_owned(),
            error: parse_errno(error)?,
        };
        return Some((device.to_owned(), issue));
    }

    let (device, message) = split_device_prefix(line)?;

    // USB devices that never made it onto the bus:
    // "usb 1-3: device descriptor read/64, error -71"
    // "usb 1-3: device not accepting address 5, error -71"
    // "usb usb1-port3: unable to enumerate USB device"
    // "usb usb1-port3: over-current condition"
    if line.starts_with("usb ") {
        if message.starts_with("device descriptor read")
            || message.starts_with("device not accepting address")
        {
            let error = message
                .rsplit_once(", error ")
                .and_then(|(_, e)| parse_errno(e))
                .unwrap_or(0);
            return Some((device.to_owned(), LogIssue::UsbConnectFailed { error }));
        }
        if message.starts_with("unable to enumerate USB device") {
            return Some((device.to_owned(), LogIssue::UsbConnectFailed { error: 0 }));
        }
        if message.starts_with("over-current condition") {
            return Some((device.to_owned(), LogIssue::UsbOverCurrent));
        }
    }

    // "probe with driver r8169 failed with error -5" (newer kernels)
    if let Some(rest) = message.strip_prefix("probe with driver ")
        && let Some((drv, error)) = rest.split_once(" failed with error ")
    {
        let issue = LogIssue::DriverFailed {
            driver: drv.to_owned(),
            error: parse_errno(error)?,
        };
        return Some((device.to_owned(), issue));
    }

    // "Direct firmware load for X failed with error -2"
    if let Some(rest) = message.strip_prefix("Direct firmware load for ")
        && let Some((file, error)) = rest.split_once(" failed with error ")
    {
        return Some((
            device.to_owned(),
            missing_firmware(file, parse_errno(error)?, firmware_dirs),
        ));
    }

    // "firmware: failed to load X (-2)"
    if let Some(rest) = message.strip_prefix("firmware: failed to load ")
        && let Some((file, error)) = rest.rsplit_once(" (")
    {
        let error = parse_errno(error.trim_end_matches(')'))?;
        return Some((
            device.to_owned(),
            missing_firmware(file, error, firmware_dirs),
        ));
    }

    None
}

fn missing_firmware(file: &str, error: i32, firmware_dirs: &[PathBuf]) -> LogIssue {
    LogIssue::MissingFirmware {
        file: file.to_owned(),
        error,
        installed_now: firmware_installed(file, firmware_dirs),
        package: None,
        install_command: None,
    }
}

/// Split `"r8169 0000:07:00.0: message"` into device and message.
fn split_device_prefix(line: &str) -> Option<(&str, &str)> {
    let (driver, rest) = line.split_once(' ')?;
    let (device, message) = rest.split_once(": ")?;
    let looks_like_name =
        |s: &str| !s.is_empty() && !s.contains(' ') && s.chars().all(|c| c.is_ascii_graphic());
    (looks_like_name(driver) && looks_like_name(device)).then_some((device, message))
}

fn parse_errno(s: &str) -> Option<i32> {
    s.trim().parse().ok()
}

/// Folders the kernel searches for firmware, in its own order.
pub fn firmware_dirs() -> Vec<PathBuf> {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let release = release.trim();
    let mut dirs = Vec::new();
    // A custom path set with firmware_class.path= comes first.
    if let Ok(custom) = std::fs::read_to_string("/sys/module/firmware_class/parameters/path") {
        let custom = custom.trim();
        if !custom.is_empty() {
            dirs.push(PathBuf::from(custom));
        }
    }
    for base in ["/lib/firmware", "/usr/lib/firmware"] {
        if !release.is_empty() {
            dirs.push(Path::new(base).join("updates").join(release));
        }
        dirs.push(Path::new(base).join("updates"));
        if !release.is_empty() {
            dirs.push(Path::new(base).join(release));
        }
        dirs.push(PathBuf::from(base));
    }
    dirs
}

/// Whether a firmware file exists, including compressed copies the kernel can unpack.
fn firmware_installed(file: &str, dirs: &[PathBuf]) -> bool {
    if file.contains("..") {
        return false;
    }
    dirs.iter().any(|dir| {
        ["", ".zst", ".xz"]
            .iter()
            .any(|ext| dir.join(format!("{file}{ext}")).is_file())
    })
}

/// A plain-English meaning for the error numbers drivers report.
pub fn errno_meaning(error: i32) -> &'static str {
    match error.abs() {
        1 | 13 => "it wasn't allowed to",
        2 => "a file it needs is missing",
        5 => "the device didn't respond correctly",
        11 => "the device was temporarily unavailable",
        12 => "the computer ran out of memory for it",
        16 => "the device was already in use",
        19 => "the device wasn't found when the driver looked for it",
        22 => "the device sent something the driver didn't expect",
        32 => "the device stopped responding",
        71 => "the connection kept failing",
        95 => "this device model isn't supported by the driver",
        110 => "the device didn't respond in time",
        517 => "it was waiting on another part that never became ready",
        _ => "an unexpected error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    #[test]
    fn firmware_and_driver_failures_by_device() {
        let log = lines(
            "\
r8169 0000:07:00.0: Direct firmware load for rtl_nic/rtl8125b-2.fw failed with error -2
r8169 0000:07:00.0: Direct firmware load for rtl_nic/rtl8125b-2.fw failed with error -2
iwlwifi 0000:02:00.0: firmware: failed to load iwlwifi-ty-a0-gf-a0-89.ucode (-2)
amdgpu 0000:0c:00.0: probe with driver amdgpu failed with error -110
r8169: probe of 0000:07:00.0 failed with error -5
[UFW BLOCK] IN=wlan0 OUT= MAC= SRC=192.168.4.1
usb 1-2: new high-speed USB device number 3 using xhci_hcd
",
        );
        let found = issues_by_device(&log, &[]);
        assert_eq!(found.len(), 3);
        assert_eq!(
            found["0000:07:00.0"],
            vec![
                LogIssue::MissingFirmware {
                    file: "rtl_nic/rtl8125b-2.fw".into(),
                    error: -2,
                    installed_now: false,
                    package: None,
                    install_command: None,
                },
                LogIssue::DriverFailed {
                    driver: "r8169".into(),
                    error: -5
                },
            ]
        );
        assert!(matches!(
            &found["0000:02:00.0"][0],
            LogIssue::MissingFirmware { file, .. } if file == "iwlwifi-ty-a0-gf-a0-89.ucode"
        ));
        assert_eq!(
            found["0000:0c:00.0"],
            vec![LogIssue::DriverFailed {
                driver: "amdgpu".into(),
                error: -110
            }]
        );
    }

    #[test]
    fn firmware_installed_since_boot_is_noticed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(tmp.path().join("rtl_nic")).expect("mkdir");
        fs::write(tmp.path().join("rtl_nic/rtl8125b-2.fw.zst"), b"x").expect("write");
        let log = lines(
            "r8169 0000:07:00.0: Direct firmware load for rtl_nic/rtl8125b-2.fw failed with error -2",
        );
        let found = issues_by_device(&log, &[tmp.path().to_path_buf()]);
        assert!(matches!(
            found["0000:07:00.0"][0],
            LogIssue::MissingFirmware {
                installed_now: true,
                ..
            }
        ));
    }

    #[test]
    fn path_tricks_never_count_as_installed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(!firmware_installed(
            "../../etc/passwd",
            &[tmp.path().to_path_buf()]
        ));
    }

    #[test]
    fn usb_connection_problems() {
        let log = lines(
            "\
usb 1-3: device descriptor read/64, error -71
usb 1-3: device not accepting address 5, error -71
usb usb1-port3: unable to enumerate USB device
usb usb2-port1: over-current condition
usb 1-4: new high-speed USB device number 3 using xhci_hcd
",
        );
        let found = issues_by_device(&log, &[]);
        assert_eq!(
            found["1-3"],
            vec![LogIssue::UsbConnectFailed { error: -71 }]
        );
        assert_eq!(
            found["usb1-port3"],
            vec![LogIssue::UsbConnectFailed { error: 0 }]
        );
        assert_eq!(found["usb2-port1"], vec![LogIssue::UsbOverCurrent]);
        assert!(!found.contains_key("1-4"));
        assert!(
            found["1-3"][0]
                .message()
                .contains("the connection kept failing")
        );
    }

    #[test]
    fn saved_dmesg_lines_are_understood() {
        let log = lines(
            "[    2.104512] r8169 0000:07:00.0: probe with driver r8169 failed with error -110",
        );
        let found = issues_by_device(&log, &[]);
        assert_eq!(
            found["0000:07:00.0"],
            vec![LogIssue::DriverFailed {
                driver: "r8169".into(),
                error: -110
            }]
        );
    }

    #[test]
    fn dmesg_timestamps_are_removed() {
        assert_eq!(
            strip_dmesg_timestamp("[    1.234567] r8169 0000:07:00.0: x"),
            "r8169 0000:07:00.0: x"
        );
        assert_eq!(strip_dmesg_timestamp("no timestamp"), "no timestamp");
    }

    #[test]
    fn messages_are_plain_english() {
        let issue = LogIssue::DriverFailed {
            driver: "amdgpu".into(),
            error: -110,
        };
        assert_eq!(
            issue.message(),
            "The amdgpu driver couldn't start this device (the device didn't respond in time)."
        );
        assert_eq!(errno_meaning(-9999), "an unexpected error");
    }
}
