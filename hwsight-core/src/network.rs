//! Network connections: cable speed and duplex, Wi-Fi signal, band, and speed.
//!
//! The link state comes from the interface's sysfs folder and `/proc/net/wireless`;
//! Wi-Fi band and speed come from NetworkManager over D-Bus (read-only). Neither needs a
//! password. MAC addresses and the Wi-Fi network's name are never read: they identify
//! the owner. Wired speed is judged without asking the adapter what it could do (that
//! needs a low-level call): 100 Mb/s or less on a modern adapter almost always means a
//! damaged cable or a slow router port.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use zbus::blocking::Proxy;
use zbus::blocking::connection::Builder;
use zbus::zvariant::OwnedObjectPath;

use crate::details::{Detail, DetailGroup, Verdict};
use crate::device::{Bus, Category, Device};

const NM: &str = "org.freedesktop.NetworkManager";
const TIMEOUT: Duration = Duration::from_secs(3);

/// What NetworkManager knows about a connected Wi-Fi adapter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WifiLive {
    pub frequency_mhz: u32,
    /// The speed the link is running at now, in kilobits per second.
    pub bitrate_kbps: u32,
    pub strength_percent: u8,
    pub supports_5ghz: bool,
    /// The Wi-Fi generation the link uses now (4, 5, 6, or 7), from the `iw` program.
    pub standard_in_use: Option<u8>,
    /// The newest generation the adapter supports.
    pub standard_max: Option<u8>,
}

/// The Wi-Fi generation of a link, from the bit-rate line of `iw dev <name> link`:
/// `EHT-MCS` is Wi-Fi 7, `HE-MCS` Wi-Fi 6, `VHT-MCS` Wi-Fi 5, a bare `MCS` Wi-Fi 4.
/// Nothing else in the output is looked at (it also names the network).
fn standard_in_use(link: &str) -> Option<u8> {
    let line = link
        .lines()
        .find(|l| l.trim_start().starts_with("tx bitrate:"))?;
    line.split_whitespace().find_map(|word| match word {
        "EHT-MCS" => Some(7),
        "HE-MCS" => Some(6),
        "VHT-MCS" => Some(5),
        "MCS" => Some(4),
        _ => None,
    })
}

/// The newest Wi-Fi generation an adapter supports, from `iw phy <phy> info`.
fn standard_max(info: &str) -> Option<u8> {
    let has = |needle: &str| info.lines().any(|l| l.contains(needle));
    if has("EHT Iftypes") {
        Some(7)
    } else if has("HE Iftypes") {
        Some(6)
    } else if has("VHT Capabilities") {
        Some(5)
    } else if has("HT TX/RX MCS rate indexes supported") {
        Some(4)
    } else {
        None
    }
}

/// Output of the `iw` program (usually installed with NetworkManager); nothing if it's missing.
fn iw(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("iw")
        .args(args)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Wi-Fi adapters by interface name.
pub(crate) type WifiByInterface = HashMap<String, WifiLive>;

/// NetworkManager's capability bit for the 5 GHz band.
const CAP_5GHZ: u32 = 0x400;

pub(crate) fn read_wifi_live() -> WifiByInterface {
    let mut found = WifiByInterface::new();
    let Some(connection) = Builder::system()
        .ok()
        .and_then(|b| b.method_timeout(TIMEOUT).build().ok())
    else {
        return found;
    };
    let Ok(manager) = Proxy::new(&connection, NM, "/org/freedesktop/NetworkManager", NM) else {
        return found;
    };
    let Ok(devices) = manager.call::<_, _, Vec<OwnedObjectPath>>("GetDevices", &()) else {
        return found;
    };
    for path in devices {
        let device =
            |interface: &str| Proxy::new(&connection, NM, path.as_str(), interface.to_owned());
        let Ok(base) = device("org.freedesktop.NetworkManager.Device") else {
            continue;
        };
        // 2 is Wi-Fi.
        if base.get_property::<u32>("DeviceType").ok() != Some(2) {
            continue;
        }
        let (Ok(name), Ok(wifi)) = (
            base.get_property::<String>("Interface"),
            device("org.freedesktop.NetworkManager.Device.Wireless"),
        ) else {
            continue;
        };
        let access_point = wifi
            .get_property::<OwnedObjectPath>("ActiveAccessPoint")
            .ok()
            .filter(|p| p.as_str() != "/")
            .and_then(|p| {
                Proxy::new(
                    &connection,
                    NM,
                    p.to_string(),
                    "org.freedesktop.NetworkManager.AccessPoint",
                )
                .ok()
            });
        let Some(access_point) = access_point else {
            continue;
        };
        let live = WifiLive {
            frequency_mhz: access_point.get_property("Frequency").unwrap_or(0),
            bitrate_kbps: wifi.get_property("Bitrate").unwrap_or(0),
            strength_percent: access_point.get_property("Strength").unwrap_or(0),
            supports_5ghz: wifi
                .get_property::<u32>("WirelessCapabilities")
                .is_ok_and(|c| c & CAP_5GHZ != 0),
            standard_in_use: iw(&["dev", &name, "link"]).and_then(|t| standard_in_use(&t)),
            standard_max: read(Path::new(&format!("/sys/class/net/{name}/phy80211/name")))
                .and_then(|phy| iw(&["phy", &phy, "info"]))
                .and_then(|t| standard_max(&t)),
        };
        found.insert(name, live);
    }
    found
}

fn read(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The interfaces that belong to a device, with their sysfs folders: a PCI card's `net/`
/// folder, or, for a USB adapter, the `net/` folder inside one of its interfaces.
fn interfaces(device_path: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let mut look = |dir: &Path| {
        if let Ok(entries) = fs::read_dir(dir.join("net")) {
            for entry in entries.filter_map(Result::ok) {
                found.push((
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.path(),
                ));
            }
        }
    };
    look(device_path);
    if let Ok(entries) = fs::read_dir(device_path) {
        for entry in entries.filter_map(Result::ok) {
            if entry.path().is_dir() {
                look(&entry.path());
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// "2.5 Gb/s", "1 Gb/s", "100 Mb/s".
fn speed_text(mbps: u64) -> String {
    if mbps >= 1000 {
        let gigabits = mbps as f64 / 1000.0;
        if gigabits.fract() == 0.0 {
            format!("{gigabits:.0} Gb/s")
        } else {
            format!("{gigabits:.1} Gb/s")
        }
    } else {
        format!("{mbps} Mb/s")
    }
}

/// The signal level in dBm from `/proc/net/wireless`, when the driver reports dBm.
fn signal_dbm(proc_wireless: &Path, interface: &str) -> Option<i32> {
    let text = fs::read_to_string(proc_wireless).ok()?;
    text.lines().find_map(|line| {
        let (name, rest) = line.split_once(':')?;
        if name.trim() != interface {
            return None;
        }
        // status, link quality, signal level, noise ...
        let level: f64 = rest
            .split_whitespace()
            .nth(2)?
            .trim_end_matches('.')
            .parse()
            .ok()?;
        (level < 0.0).then_some(level as i32)
    })
}

fn wired_rows(dir: &Path) -> Vec<Detail> {
    let connected = read(&dir.join("carrier")).is_some_and(|c| c == "1");
    let mut rows = vec![Detail::new(
        crate::tr!("Cable"),
        if connected {
            crate::tr!("Connected")
        } else {
            crate::tr!("No cable connected")
        },
        dir.join("carrier").display().to_string(),
    )];
    if !connected {
        return rows;
    }
    if let Some(mbps) = read(&dir.join("speed"))
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|s| *s > 0)
    {
        let mbps = mbps as u64;
        let (verdict, why) = if mbps <= 100 {
            (
                Verdict::Reduced,
                Some(crate::tr!(
                    "The cable is only running at {}, which is much slower than modern network connections. A damaged or very old cable, or a slow port on the router, is the usual cause. Try another cable.",
                    speed_text(mbps)
                )),
            )
        } else {
            (Verdict::Good, None)
        };
        rows.push(
            Detail::new(
                crate::tr!("Link speed"),
                speed_text(mbps),
                dir.join("speed").display().to_string(),
            )
            .raw(format!("{mbps} Mb/s"))
            .verdict(verdict, why),
        );
    }
    if let Some(duplex) = read(&dir.join("duplex")).filter(|d| d == "full" || d == "half") {
        let (value, verdict, why) = if duplex == "full" {
            (crate::tr!("Full"), Verdict::Good, None)
        } else {
            (
                crate::tr!("Half"),
                Verdict::Reduced,
                Some(crate::tr!(
                    "Half duplex means the two ends disagree about how to talk, which makes the connection slow and unreliable. A different cable or router port usually fixes it."
                )),
            )
        };
        rows.push(
            Detail::new(
                crate::tr!("Duplex"),
                value,
                dir.join("duplex").display().to_string(),
            )
            .raw(duplex)
            .verdict(verdict, why),
        );
    }
    rows
}

fn band(frequency_mhz: u32) -> Option<&'static str> {
    match frequency_mhz {
        2400..=2500 => Some("2.4"),
        4900..=5900 => Some("5"),
        5925..=7125 => Some("6"),
        _ => None,
    }
}

fn wifi_rows(dir: &Path, name: &str, proc_wireless: &Path, live: Option<&WifiLive>) -> Vec<Detail> {
    let connected = read(&dir.join("operstate")).is_some_and(|s| s == "up");
    let mut rows = vec![Detail::new(
        crate::tr!("Wi-Fi"),
        if connected {
            crate::tr!("Connected")
        } else {
            crate::tr!("Not connected")
        },
        dir.join("operstate").display().to_string(),
    )];
    if !connected {
        return rows;
    }
    let weak = crate::tr!(
        "The Wi-Fi signal is weak, which means slower and less reliable connections. Moving closer to the router, or removing obstacles in between, helps."
    );
    if let Some(dbm) = signal_dbm(proc_wireless, name) {
        let (verdict, why) = if dbm <= -70 {
            (Verdict::Reduced, Some(weak))
        } else {
            (Verdict::Good, None)
        };
        rows.push(
            Detail::new(
                crate::tr!("Signal strength"),
                crate::tr!("{} dBm", dbm),
                proc_wireless.display().to_string(),
            )
            .verdict(verdict, why),
        );
    } else if let Some(percent) = live.map(|l| l.strength_percent).filter(|p| *p > 0) {
        let (verdict, why) = if percent < 35 {
            (Verdict::Reduced, Some(weak))
        } else {
            (Verdict::Good, None)
        };
        rows.push(
            Detail::new(
                crate::tr!("Signal strength"),
                format!("{percent}%"),
                crate::tr!("NetworkManager"),
            )
            .verdict(verdict, why),
        );
    }
    if let Some(live) = live {
        if let Some(ghz) = band(live.frequency_mhz) {
            let why = (ghz == "2.4" && live.supports_5ghz).then(|| {
                crate::tr!(
                    "This adapter also supports the faster, less crowded 5 GHz band, if your router offers it."
                )
            });
            rows.push(
                Detail::new(
                    crate::tr!("Wi-Fi band"),
                    crate::tr!("{} GHz", ghz),
                    crate::tr!("NetworkManager"),
                )
                .raw(format!("{} MHz", live.frequency_mhz))
                .verdict(Verdict::Info, why),
            );
        }
        if let Some(now) = live.standard_in_use {
            let behind = live.standard_max.filter(|max| *max > now);
            rows.push(
                Detail::new(
                    crate::tr!("Wi-Fi standard"),
                    crate::tr!("Wi-Fi {}", now),
                    crate::tr!("The iw program"),
                )
                .verdict(
                    Verdict::Info,
                    behind.map(|max| {
                        crate::tr!(
                            "This adapter supports Wi-Fi {}, but this connection uses Wi-Fi {}. The router (or its settings) is the limit, not the adapter.",
                            max,
                            now
                        )
                    }),
                ),
            );
        }
        if live.bitrate_kbps > 0 {
            rows.push(
                Detail::new(
                    crate::tr!("Connection speed"),
                    speed_text(u64::from(live.bitrate_kbps) / 1000),
                    crate::tr!("NetworkManager"),
                )
                .verdict(
                    Verdict::Info,
                    Some(crate::tr!(
                        "The speed the Wi-Fi link is running at now. Real downloads are slower."
                    )),
                ),
            );
        }
    }
    rows
}

/// Add a "Network connection" group to every network device that has an interface.
pub(crate) fn attach(devices: &mut [Device], proc_wireless: &Path, live: &WifiByInterface) {
    for device in devices
        .iter_mut()
        .filter(|d| d.category == Category::Network && matches!(d.bus, Bus::Pci | Bus::Usb))
    {
        for (name, dir) in interfaces(&device.sysfs_path) {
            let wireless = dir.join("wireless").is_dir();
            let mut items = if wireless {
                wifi_rows(&dir, &name, proc_wireless, live.get(&name))
            } else {
                wired_rows(&dir)
            };
            items.push(
                Detail::new(
                    crate::tr!("Interface name"),
                    name.clone(),
                    dir.display().to_string(),
                )
                .technical(),
            );
            if let Some(mtu) = read(&dir.join("mtu")) {
                items.push(
                    Detail::new(
                        crate::tr!("Largest packet size (MTU)"),
                        mtu,
                        dir.join("mtu").display().to_string(),
                    )
                    .technical(),
                );
            }
            device.details.push(DetailGroup {
                title: crate::tr!("Network connection"),
                items,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::BuiltIn;
    use crate::i18n::in_english;

    fn write(root: &Path, name: &str, text: &str) {
        let file = root.join(name);
        fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        fs::write(file, format!("{text}\n")).expect("write");
    }

    fn nic(root: &Path, bus: Bus) -> Device {
        let mut d = Device::built_in(BuiltIn {
            address: "x",
            sysfs_path: root.to_path_buf(),
            display_name: "Network card".into(),
            vendor_name: None,
            category: Category::Network,
            hardware_id: "10ec:8125".into(),
            description: String::new(),
            details: Vec::new(),
        });
        d.bus = bus;
        d
    }

    fn run(root: &Path, bus: Bus, wireless: &Path, live: &WifiByInterface) -> Device {
        let mut devices = vec![nic(root, bus)];
        in_english(|| attach(&mut devices, wireless, live));
        devices.remove(0)
    }

    fn item<'a>(device: &'a Device, label: &str) -> &'a Detail {
        device
            .details
            .iter()
            .flat_map(|g| &g.items)
            .find(|d| d.label == label)
            .unwrap_or_else(|| panic!("no detail {label:?} in {:#?}", device.details))
    }

    const NOWHERE: &str = "/nonexistent/wireless";

    #[test]
    fn a_unplugged_port_says_so_and_is_not_a_problem() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(temp.path(), "net/eno1/carrier", "0");
        write(temp.path(), "net/eno1/speed", "-1");
        let d = run(
            temp.path(),
            Bus::Pci,
            Path::new(NOWHERE),
            &WifiByInterface::new(),
        );
        assert_eq!(item(&d, "Cable").value, "No cable connected");
        assert_eq!(item(&d, "Cable").verdict, Verdict::Info);
        assert!(
            !d.details
                .iter()
                .flat_map(|g| &g.items)
                .any(|i| i.label == "Link speed")
        );
        assert_eq!(item(&d, "Interface name").value, "eno1");
        assert!(item(&d, "Interface name").technical);
    }

    #[test]
    fn wired_speed_is_shown_nicely_and_slow_links_are_flagged() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(temp.path(), "net/eno1/carrier", "1");
        write(temp.path(), "net/eno1/speed", "2500");
        write(temp.path(), "net/eno1/duplex", "full");
        let d = run(
            temp.path(),
            Bus::Pci,
            Path::new(NOWHERE),
            &WifiByInterface::new(),
        );
        assert_eq!(item(&d, "Link speed").value, "2.5 Gb/s");
        assert_eq!(item(&d, "Link speed").verdict, Verdict::Good);
        assert_eq!(item(&d, "Duplex").value, "Full");

        write(temp.path(), "net/eno1/speed", "100");
        write(temp.path(), "net/eno1/duplex", "half");
        let d = run(
            temp.path(),
            Bus::Pci,
            Path::new(NOWHERE),
            &WifiByInterface::new(),
        );
        let speed = item(&d, "Link speed");
        assert_eq!(speed.value, "100 Mb/s");
        assert_eq!(speed.verdict, Verdict::Reduced);
        assert!(
            speed
                .explanation
                .as_deref()
                .is_some_and(|t| t.contains("only running at 100 Mb/s"))
        );
        assert_eq!(item(&d, "Duplex").verdict, Verdict::Reduced);

        write(temp.path(), "net/eno1/speed", "1000");
        write(temp.path(), "net/eno1/duplex", "full");
        let d = run(
            temp.path(),
            Bus::Pci,
            Path::new(NOWHERE),
            &WifiByInterface::new(),
        );
        assert_eq!(item(&d, "Link speed").value, "1 Gb/s");
    }

    fn wifi_card(root: &Path, up: bool) {
        write(root, "net/wlan0/wireless/.keep", "");
        write(root, "net/wlan0/operstate", if up { "up" } else { "down" });
        write(root, "net/wlan0/mtu", "1500");
    }

    fn proc_wireless(root: &Path, level: &str) -> std::path::PathBuf {
        write(
            root,
            "wireless",
            &format!(
                "Inter-| sta-|   Quality        |   Discarded packets\n face | tus | link level noise |  nwid\n wlan0: 0000   51.  {level}.  -256        0\n"
            ),
        );
        root.join("wireless")
    }

    fn live(frequency: u32, supports_5: bool) -> WifiByInterface {
        WifiByInterface::from([(
            "wlan0".to_owned(),
            WifiLive {
                frequency_mhz: frequency,
                bitrate_kbps: 720_600,
                strength_percent: 60,
                supports_5ghz: supports_5,
                standard_in_use: Some(6),
                standard_max: Some(7),
            },
        )])
    }

    #[test]
    fn the_wifi_generation_is_read_from_iw_output() {
        let link = |rate: &str| {
            format!("Connected to 34:bc (on wlan0)\n\tSSID: home\n\ttx bitrate: {rate}\n")
        };
        assert_eq!(
            standard_in_use(&link("720.6 MBit/s 80MHz HE-MCS 7 HE-NSS 2")),
            Some(6)
        );
        assert_eq!(
            standard_in_use(&link("2882 MBit/s 160MHz EHT-MCS 11 EHT-NSS 2")),
            Some(7)
        );
        assert_eq!(
            standard_in_use(&link("866.7 MBit/s 80MHz VHT-MCS 9 VHT-NSS 2")),
            Some(5)
        );
        assert_eq!(
            standard_in_use(&link("144.4 MBit/s MCS 15 short GI")),
            Some(4)
        );
        assert_eq!(standard_in_use(&link("54.0 MBit/s")), None);
        assert_eq!(standard_in_use("Not connected."), None);
        assert_eq!(
            standard_max("\t\tHE Iftypes: managed\n\t\tVHT Capabilities (0x1)\n"),
            Some(6)
        );
        assert_eq!(
            standard_max("\t\tEHT Iftypes: managed\n\t\tHE Iftypes: managed\n"),
            Some(7)
        );
        assert_eq!(
            standard_max("\t\tHT TX/RX MCS rate indexes supported: 0-15\n"),
            Some(4)
        );
        assert_eq!(standard_max(""), None);
    }

    #[test]
    fn a_router_limiting_the_adapter_is_explained() {
        let root = tempfile::tempdir().expect("tempdir");
        wifi_card(root.path(), true);
        let rows = in_english(|| {
            wifi_rows(
                &root.path().join("net/wlan0"),
                "wlan0",
                &root.path().join("wireless"),
                live(5805, true).get("wlan0"),
            )
        });
        let standard = rows
            .iter()
            .find(|r| r.label == "Wi-Fi standard")
            .expect("row");
        assert_eq!(standard.value, "Wi-Fi 6");
        assert!(
            standard
                .explanation
                .as_deref()
                .unwrap_or("")
                .contains("supports Wi-Fi 7")
        );
    }

    #[test]
    fn wifi_shows_signal_band_and_speed() {
        let temp = tempfile::tempdir().expect("tempdir");
        wifi_card(temp.path(), true);
        let wireless = proc_wireless(temp.path(), "-59");
        let d = run(temp.path(), Bus::Pci, &wireless, &live(5805, true));
        assert_eq!(item(&d, "Wi-Fi").value, "Connected");
        assert_eq!(item(&d, "Signal strength").value, "-59 dBm");
        assert_eq!(item(&d, "Signal strength").verdict, Verdict::Good);
        assert_eq!(item(&d, "Wi-Fi band").value, "5 GHz");
        assert_eq!(item(&d, "Wi-Fi band").raw.as_deref(), Some("5805 MHz"));
        assert_eq!(item(&d, "Connection speed").value, "720 Mb/s");
        assert_eq!(item(&d, "Largest packet size (MTU)").value, "1500");
    }

    #[test]
    fn a_weak_signal_is_flagged_and_the_slow_band_gets_a_hint() {
        let temp = tempfile::tempdir().expect("tempdir");
        wifi_card(temp.path(), true);
        let wireless = proc_wireless(temp.path(), "-78");
        let d = run(temp.path(), Bus::Pci, &wireless, &live(2437, true));
        let signal = item(&d, "Signal strength");
        assert_eq!(signal.verdict, Verdict::Reduced);
        assert!(
            signal
                .explanation
                .as_deref()
                .is_some_and(|t| t.contains("Moving closer"))
        );
        let band = item(&d, "Wi-Fi band");
        assert_eq!(band.value, "2.4 GHz");
        assert_eq!(band.verdict, Verdict::Info, "a hint, not an alarm");
        assert!(
            band.explanation
                .as_deref()
                .is_some_and(|t| t.contains("5 GHz"))
        );
        let only_2g = run(temp.path(), Bus::Pci, &wireless, &live(2437, false));
        assert_eq!(item(&only_2g, "Wi-Fi band").explanation, None);
    }

    #[test]
    fn without_dbm_the_percentage_is_used_and_a_disconnected_adapter_says_so() {
        let temp = tempfile::tempdir().expect("tempdir");
        wifi_card(temp.path(), true);
        let d = run(temp.path(), Bus::Pci, Path::new(NOWHERE), &live(5805, true));
        assert_eq!(item(&d, "Signal strength").value, "60%");
        let mut weak = live(5805, true);
        weak.get_mut("wlan0").expect("wlan0").strength_percent = 20;
        let d = run(temp.path(), Bus::Pci, Path::new(NOWHERE), &weak);
        assert_eq!(item(&d, "Signal strength").verdict, Verdict::Reduced);

        wifi_card(temp.path(), false);
        let d = run(
            temp.path(),
            Bus::Pci,
            Path::new(NOWHERE),
            &WifiByInterface::new(),
        );
        assert_eq!(item(&d, "Wi-Fi").value, "Not connected");
        assert!(
            !d.details
                .iter()
                .flat_map(|g| &g.items)
                .any(|i| i.label == "Signal strength")
        );
    }

    #[test]
    fn usb_adapters_and_other_devices_are_handled() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(temp.path(), "1-2:1.0/net/enx1/carrier", "1");
        write(temp.path(), "1-2:1.0/net/enx1/speed", "1000");
        let d = run(
            temp.path(),
            Bus::Usb,
            Path::new(NOWHERE),
            &WifiByInterface::new(),
        );
        assert_eq!(item(&d, "Link speed").value, "1 Gb/s");
        // A device with no interface gets nothing, and so does a non-network device.
        let empty = tempfile::tempdir().expect("tempdir");
        assert!(
            run(
                empty.path(),
                Bus::Pci,
                Path::new(NOWHERE),
                &WifiByInterface::new()
            )
            .details
            .is_empty()
        );
        let mut other = vec![nic(temp.path(), Bus::Usb)];
        other[0].category = Category::Storage;
        attach(&mut other, Path::new(NOWHERE), &WifiByInterface::new());
        assert!(other[0].details.is_empty());
        assert_eq!(band(5100), Some("5"));
        assert_eq!(band(6000), Some("6"));
        assert_eq!(band(900), None);
    }

    #[test]
    fn nothing_that_identifies_the_owner_is_read() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(temp.path(), "net/eno1/carrier", "1");
        write(temp.path(), "net/eno1/address", "aa:bb:cc:dd:ee:ff");
        write(temp.path(), "net/eno1/speed", "1000");
        let d = run(
            temp.path(),
            Bus::Pci,
            Path::new(NOWHERE),
            &WifiByInterface::new(),
        );
        assert!(!format!("{:?}", d.details).to_lowercase().contains("aa:bb"));
    }
}
