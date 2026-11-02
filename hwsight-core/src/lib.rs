//! hwsight core: finds every device in the computer and works out whether it's working.
//!
//! No UI code lives here, so the command line and graphical apps share one engine.

pub mod data;
pub mod device;
pub mod distro;
pub mod firmware;
pub mod kernel_log;
pub mod modules;
mod pci;
pub mod pci_ids;
pub mod rules;
pub mod update;
pub mod usb_ids;
pub use usb_ids::UsbNames;
mod usb;

use std::path::PathBuf;

use serde::Serialize;

pub use data::{DataDirs, DataProblem, DataSource};
pub use device::{Bus, Category, Device, Status, short_vendor};
pub use distro::{Distro, Family};
pub use firmware::FirmwarePackages;
pub use kernel_log::{LogIssue, LogSource};
pub use modules::{Blocklist, DriverCandidate};
pub use pci_ids::{IdsSource, PciNames};
pub use rules::Rules;

/// Everything a scan found, plus anything that limited it.
#[derive(Debug, Serialize)]
pub struct ScanReport {
    pub devices: Vec<Device>,
    /// Problems on USB ports where nothing working is plugged in, e.g. a device that
    /// couldn't connect. These devices never show up in `devices`.
    pub port_issues: Vec<PortIssue>,
    pub notes: Vec<ScanNote>,
    pub data: DataInfo,
    /// Where problem explanations were read from; `None` if the log wasn't readable.
    pub kernel_log: Option<LogSource>,
    pub distro: Distro,
    /// Distro-specific advice for installing firmware, e.g. enabling a repository.
    pub firmware_note: Option<String>,
}

/// A problem on a USB port rather than a device.
#[derive(Debug, Clone, Serialize)]
pub struct PortIssue {
    /// e.g. "USB bus 1, port 3"
    pub port: String,
    pub issue: LogIssue,
}

/// Something the user (or a bug report) should know about the scan itself.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScanNote {
    /// The kernel's driver list couldn't be read, so missing drivers can't be detected.
    NoDriverList,
    /// The USB device folder couldn't be read.
    UsbUnreadable { error: String },
    /// The PCI device folder couldn't be read.
    PciUnreadable { error: String },
    /// The kernel log couldn't be read, so problems can't be explained in detail.
    KernelLogUnreadable,
    /// A downloaded data file needs a newer hwsight.
    DataNeedsNewerApp { path: PathBuf, schema: u32 },
    /// A data file was broken and skipped.
    DataInvalid { path: PathBuf, error: String },
}

impl ScanNote {
    pub fn message(&self) -> String {
        match self {
            Self::NoDriverList => "Couldn't read the kernel's list of drivers, so hwsight can't \
                tell whether a driver is available for devices without one."
                .to_owned(),
            Self::KernelLogUnreadable => "Couldn't read the kernel log, so hwsight can't say why \
                a device isn't working. On some systems only administrators can read it."
                .to_owned(),
            Self::UsbUnreadable { error } => format!("Couldn't read the USB device list: {error}"),
            Self::PciUnreadable { error } => format!("Couldn't read the PCI device list: {error}"),
            Self::DataNeedsNewerApp { path, .. } => format!(
                "{} is from a newer version of hwsight. Update hwsight to use it.",
                path.display()
            ),
            Self::DataInvalid { path, error } => {
                format!("Skipped damaged data file {}: {error}", path.display())
            }
        }
    }
}

impl From<DataProblem> for ScanNote {
    fn from(problem: DataProblem) -> Self {
        match problem {
            DataProblem::NewerSchema { path, schema } => Self::DataNeedsNewerApp { path, schema },
            DataProblem::Invalid { path, error } => Self::DataInvalid { path, error },
        }
    }
}

/// Which data versions the scan used, so users and bug reports can see how fresh it is.
#[derive(Debug, Serialize)]
pub struct DataInfo {
    pub rules_version: String,
    pub firmware_version: String,
    /// `None` means the built-in copy.
    pub rules_path: Option<PathBuf>,
    /// `pci.ids` files used, newest first. The built-in database is always the fallback.
    pub pci_ids: Vec<IdsSourceInfo>,
}

#[derive(Debug, Serialize)]
pub struct IdsSourceInfo {
    pub path: PathBuf,
    pub version: Option<String>,
}

/// Where a scan reads from. `Scanner::system()` is the real machine; tests point the
/// paths at fake folders.
#[derive(Debug, Clone)]
pub struct Scanner {
    pub data_dirs: DataDirs,
    pub pci_devices: PathBuf,
    pub usb_devices: PathBuf,
    /// The running kernel's module folder, e.g. `/lib/modules/6.18.0`.
    pub kernel_modules: Option<PathBuf>,
    /// Where loaded modules appear, normally `/sys/module`.
    pub loaded_modules: PathBuf,
    /// modprobe settings folders, highest priority first.
    pub modprobe_dirs: Vec<PathBuf>,
    /// The kernel command line file, normally `/proc/cmdline`.
    pub kernel_cmdline: PathBuf,
    pub kernel_log: KernelLogInput,
    /// Folders the kernel loads firmware from.
    pub firmware_dirs: Vec<PathBuf>,
    pub distro: Distro,
}

/// Where to get kernel log lines.
#[derive(Debug, Clone)]
pub enum KernelLogInput {
    /// Read this boot's log from the system.
    System,
    /// Use these lines (tests, or a log the user attached to a bug report).
    Lines(Vec<String>),
    /// Don't read the log.
    Skip,
}

impl Scanner {
    pub fn system() -> Self {
        Self {
            data_dirs: DataDirs::standard(),
            pci_devices: PathBuf::from("/sys/bus/pci/devices"),
            usb_devices: PathBuf::from("/sys/bus/usb/devices"),
            kernel_modules: modules::running_kernel_modules_dir(),
            loaded_modules: PathBuf::from("/sys/module"),
            modprobe_dirs: modules::MODPROBE_DIRS.iter().map(PathBuf::from).collect(),
            kernel_cmdline: PathBuf::from("/proc/cmdline"),
            kernel_log: KernelLogInput::System,
            firmware_dirs: kernel_log::firmware_dirs(),
            distro: Distro::detect(),
        }
    }

    /// Find every device. Never fails: problems are reported in `notes` instead, so
    /// the user always sees as much as we could find.
    pub fn scan(&self) -> ScanReport {
        let mut notes = Vec::new();

        let (rules, problems) = Rules::load(&self.data_dirs);
        notes.extend(problems.into_iter().map(ScanNote::from));
        let (firmware, problems) = FirmwarePackages::load(&self.data_dirs);
        notes.extend(problems.into_iter().map(ScanNote::from));

        let names = PciNames::load(&self.data_dirs);
        let load_aliases = |bus: &str| {
            self.kernel_modules
                .as_deref()
                .and_then(|dir| modules::AliasDb::load(dir, &self.loaded_modules, bus))
        };
        let pci_aliases = load_aliases("pci:");
        let usb_aliases = load_aliases("usb:");
        if pci_aliases.is_none() {
            notes.push(ScanNote::NoDriverList);
        }

        let cmdline = std::fs::read_to_string(&self.kernel_cmdline).ok();
        let blocklist = modules::Blocklist::load(&self.modprobe_dirs, cmdline.as_deref());

        let mut devices = Vec::new();
        match pci::scan(
            &self.pci_devices,
            &names,
            pci_aliases.as_ref(),
            &blocklist,
            &rules,
        ) {
            Ok(found) => devices.extend(found),
            Err(e) => notes.push(ScanNote::PciUnreadable {
                error: e.to_string(),
            }),
        }
        match usb::scan(&self.usb_devices, usb_aliases.as_ref(), &blocklist, &rules) {
            Ok(found) => devices.extend(found),
            Err(e) => notes.push(ScanNote::UsbUnreadable {
                error: e.to_string(),
            }),
        }
        devices.sort_by(|a, b| {
            a.category
                .cmp(&b.category)
                .then_with(|| a.bus.cmp(&b.bus))
                .then_with(|| natural_address(&a.address).cmp(&natural_address(&b.address)))
        });

        let (log_source, log_lines) = match &self.kernel_log {
            KernelLogInput::System => match kernel_log::read_system_log() {
                Some((source, lines)) => (Some(source), lines),
                None => {
                    notes.push(ScanNote::KernelLogUnreadable);
                    (None, Vec::new())
                }
            },
            KernelLogInput::Lines(lines) => (None, lines.clone()),
            KernelLogInput::Skip => (None, Vec::new()),
        };
        let mut issues = kernel_log::issues_by_device(&log_lines, &self.firmware_dirs);
        for found in issues.values_mut() {
            for issue in found {
                if let LogIssue::MissingFirmware {
                    file,
                    package,
                    install_command,
                    ..
                } = issue
                {
                    *package = firmware.package_for(file, &self.distro).map(str::to_owned);
                    *install_command = package
                        .as_deref()
                        .and_then(|p| self.distro.family.install_command(p, self.distro.immutable));
                }
            }
        }
        for device in &mut devices {
            // Messages name the device ("1-5") or one of its USB interfaces ("1-5:1.0").
            let prefix = format!("{}:", device.address);
            let keys: Vec<String> = issues
                .keys()
                .filter(|k| **k == device.address || k.starts_with(&prefix))
                .cloned()
                .collect();
            for key in keys {
                for issue in issues.remove(&key).unwrap_or_default() {
                    if !device.log_issues.contains(&issue) {
                        device.log_issues.push(issue);
                    }
                }
            }
        }
        // What's left is about USB ports with nothing working plugged in: devices that
        // couldn't connect never appear in the device list at all.
        let mut port_issues: Vec<PortIssue> = issues
            .into_iter()
            .filter_map(|(key, found)| {
                let port = usb_port_label(&key)?;
                Some(found.into_iter().map(move |issue| PortIssue {
                    port: port.clone(),
                    issue,
                }))
            })
            .flatten()
            .collect();
        port_issues.sort_by(|a, b| a.port.cmp(&b.port));

        let firmware_note = devices
            .iter()
            .flat_map(|d| &d.log_issues)
            .any(|i| {
                matches!(
                    i,
                    LogIssue::MissingFirmware {
                        package: Some(_),
                        ..
                    }
                )
            })
            .then(|| firmware.note(self.distro.family).map(str::to_owned))
            .flatten();

        let data = DataInfo {
            rules_version: rules.version.clone(),
            firmware_version: firmware.version.clone(),
            rules_path: match &rules.source {
                DataSource::Embedded => None,
                DataSource::File(p) => Some(p.clone()),
            },
            pci_ids: names
                .sources()
                .into_iter()
                .map(|s| IdsSourceInfo {
                    path: s.path,
                    version: s.version,
                })
                .collect(),
        };

        ScanReport {
            devices,
            port_issues,
            notes,
            data,
            kernel_log: log_source,
            distro: self.distro.clone(),
            firmware_note,
        }
    }
}

/// Sort "1-10" after "1-9" and "0000:0a:00.0" after "0000:09:00.0" by comparing the
/// numbers inside addresses as numbers.
fn natural_address(address: &str) -> Vec<(u64, String)> {
    address
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(|part| match u64::from_str_radix(part, 16) {
            Ok(n) if !part.is_empty() => (n, String::new()),
            _ => (u64::MAX, part.to_owned()),
        })
        .collect()
}

/// A plain description of a USB port from a kernel name: `1-3` or `usb1-port3` becomes
/// "USB bus 1, port 3"; `1-3.2` becomes "USB bus 1, port 3, then hub port 2". `None` for
/// names that aren't USB ports (e.g. PCI addresses).
pub fn usb_port_label(name: &str) -> Option<String> {
    let (bus, ports) = if let Some(rest) = name.strip_prefix("usb") {
        let (bus, port) = rest.split_once("-port")?;
        (bus, port.to_owned())
    } else {
        let (bus, ports) = name.split_once('-')?;
        (bus, ports.to_owned())
    };
    let bus: u32 = bus.parse().ok()?;
    let ports: Vec<u32> = ports
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let (first, hubs) = ports.split_first()?;
    let mut label = format!("USB bus {bus}, port {first}");
    for hub_port in hubs {
        label.push_str(&format!(", then hub port {hub_port}"));
    }
    Some(label)
}

/// Scan the real machine.
pub fn scan() -> ScanReport {
    Scanner::system().scan()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn usb_port_labels() {
        assert_eq!(usb_port_label("1-3").as_deref(), Some("USB bus 1, port 3"));
        assert_eq!(
            usb_port_label("usb2-port4").as_deref(),
            Some("USB bus 2, port 4")
        );
        assert_eq!(
            usb_port_label("3-1.4").as_deref(),
            Some("USB bus 3, port 1, then hub port 4")
        );
        assert_eq!(usb_port_label("0000:07:00.0"), None);
        assert_eq!(usb_port_label("usb1"), None);
    }

    #[test]
    fn addresses_sort_naturally() {
        let mut list = vec!["1-10", "1-9", "1-2.1", "1-2"];
        list.sort_by_key(|a| natural_address(a));
        assert_eq!(list, vec!["1-2", "1-2.1", "1-9", "1-10"]);
    }

    /// A whole scan of a fake machine: one network card whose driver failed because of
    /// missing firmware, explained by the kernel log.
    #[test]
    fn full_scan_explains_a_broken_device() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let dev = root.join("pci/0000:07:00.0");
        fs::create_dir_all(&dev).expect("mkdir");
        for (file, value) in [
            ("vendor", "0x10ec"),
            ("device", "0x8125"),
            ("class", "0x020000"),
            (
                "modalias",
                "pci:v000010ECd00008125sv00001043sd000087D7bc02sc00i00",
            ),
        ] {
            fs::write(dev.join(file), value).expect("write");
        }
        let modules = root.join("modules");
        fs::create_dir_all(modules.join("r8169")).expect("mkdir");
        fs::write(
            modules.join("modules.alias"),
            "alias pci:v000010ECd00008125sv*sd*bc*sc*i* r8169\n",
        )
        .expect("write");
        fs::write(root.join("cmdline"), "quiet").expect("write");

        let scanner = Scanner {
            data_dirs: DataDirs::only(&root.join("data")),
            pci_devices: root.join("pci"),
            usb_devices: root.join("usb"),
            kernel_modules: Some(modules.clone()),
            loaded_modules: modules, // contains an "r8169" folder, so it counts as loaded
            modprobe_dirs: Vec::new(),
            kernel_cmdline: root.join("cmdline"),
            kernel_log: KernelLogInput::Lines(vec![
                "r8169 0000:07:00.0: Direct firmware load for rtl_nic/rtl8125b-2.fw failed with error -2".into(),
                "r8169 0000:07:00.0: probe with driver r8169 failed with error -2".into(),
            ]),
            firmware_dirs: vec![root.join("firmware")],
            distro: Distro::from_os_release("ID=debian\n", false),
        };
        let report = scanner.scan();
        assert!(report.notes.is_empty(), "{:?}", report.notes);
        let [nic] = report.devices.as_slice() else {
            panic!("expected one device, got {:?}", report.devices);
        };
        assert_eq!(nic.status, Status::DriverNotAttached);
        assert_eq!(nic.category, Category::Network);
        assert_eq!(nic.log_issues.len(), 2);
        assert_eq!(
            nic.log_issues[0].message(),
            "The driver needs a firmware file that isn't installed: rtl_nic/rtl8125b-2.fw. \
             It comes in the firmware-realtek package."
        );
        assert!(matches!(
            &nic.log_issues[0],
            LogIssue::MissingFirmware { package: Some(p), install_command: Some(c), .. }
                if p == "firmware-realtek" && c == "sudo apt install firmware-realtek"
        ));
        assert!(
            report
                .firmware_note
                .as_deref()
                .is_some_and(|n| n.contains("non-free-firmware"))
        );
        assert_eq!(report.distro.family, Family::Debian);
    }
}
