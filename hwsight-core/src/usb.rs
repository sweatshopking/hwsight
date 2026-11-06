//! USB devices: dongles, webcams, game controllers, card readers, and everything else
//! plugged into a USB port.
//!
//! A USB device has one or more "interfaces" (a webcam may have a video one and a sound
//! one), and drivers attach to interfaces, not to the device. So a device works when any
//! of its interfaces has a driver. Interfaces without one are often fine: lighting and
//! cooler controllers, for example, are driven directly by apps.
//!
//! Everything comes from `/sys/bus/usb/devices`, which any user can read.

use std::fs;
use std::io;
use std::path::Path;

use crate::descriptions::Descriptions;
use crate::device::{Bus, Category, Device, Status, diagnose, short_vendor};
use crate::modules::{AliasDb, Blocklist, DriverCandidate};
use crate::rules::Rules;
use crate::usb_ids::UsbNames;

/// The Linux Foundation's vendor ID, used by the virtual "root hubs" inside every USB
/// controller. Those are already listed as the controller itself, so we skip them.
const ROOT_HUB_VENDOR: u16 = 0x1d6b;

#[derive(Debug)]
struct Interface {
    name: String,
    class: u8,
    subclass: u8,
    protocol: u8,
    driver: Option<String>,
    modalias: Option<String>,
    /// Folders the driver created, e.g. `input`, `video4linux`, `bluetooth`.
    children: Vec<String>,
}

/// Read every USB device under `devices_dir` (normally `/sys/bus/usb/devices`).
pub(crate) fn scan(
    devices_dir: &Path,
    names: &UsbNames,
    aliases: Option<&AliasDb>,
    blocklist: &Blocklist,
    rules: &Rules,
    descriptions: &Descriptions,
) -> io::Result<Vec<Device>> {
    let entries = match fs::read_dir(devices_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    let mut devices = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Devices are named like "1-5" or "1-5.2"; interfaces have a ":" ("1-5:1.0");
        // root hubs are "usb1".
        if name.contains(':') || name.starts_with("usb") {
            continue;
        }
        let path = fs::canonicalize(entry.path()).unwrap_or_else(|_| entry.path());
        if let Some(device) =
            read_device(&path, name, names, aliases, blocklist, rules, descriptions)
        {
            devices.push(device);
        }
    }
    Ok(devices)
}

fn read_device(
    path: &Path,
    address: String,
    names: &UsbNames,
    aliases: Option<&AliasDb>,
    blocklist: &Blocklist,
    rules: &Rules,
    descriptions: &Descriptions,
) -> Option<Device> {
    let vendor_id = read_hex(path, "idVendor")?;
    let product_id = read_hex(path, "idProduct")?;
    if vendor_id == ROOT_HUB_VENDOR && read_hex(path, "bDeviceClass") == Some(0x09) {
        return None;
    }
    let device_class = read_hex(path, "bDeviceClass").unwrap_or(0) as u8;

    // The device's own name strings. Trimmed, because some pad them with spaces.
    let manufacturer = read_trimmed(path, "manufacturer");
    let product = read_trimmed(path, "product");

    let interfaces = read_interfaces(path, &address);
    let authorized = read_trimmed(path, "authorized").is_none_or(|a| a != "0");

    // Every installed driver for any interface, each listed once.
    let mut candidates: Vec<DriverCandidate> = Vec::new();
    if let Some(db) = aliases {
        for alias in interfaces.iter().filter_map(|i| i.modalias.as_deref()) {
            for c in db.candidates(alias, blocklist) {
                if !candidates.iter().any(|known| known.module == c.module) {
                    candidates.push(c);
                }
            }
        }
    }

    let bound: Vec<&str> = interfaces
        .iter()
        .filter_map(|i| i.driver.as_deref())
        .collect();
    let driver = (!bound.is_empty()).then(|| {
        let mut unique: Vec<&str> = Vec::new();
        for d in &bound {
            if !unique.contains(d) {
                unique.push(d);
            }
        }
        unique.join(", ")
    });

    let mut note = None;
    let status = if !authorized {
        // Blocked by the system's USB security settings (e.g. USBGuard).
        note = Some(crate::tr!(
            "This device was blocked by the computer's USB security settings, so nothing is allowed to use it."
        ));
        Status::DriverBlocked
    } else if driver.is_some() {
        Status::Working
    } else {
        let class_code = |i: &Interface| {
            (u32::from(i.class) << 16) | (u32::from(i.subclass) << 8) | u32::from(i.protocol)
        };
        let reasons: Vec<&str> = interfaces
            .iter()
            .filter_map(|i| rules.no_driver_needed(Bus::Usb, class_code(i), vendor_id, product_id))
            .collect();
        let needs_driver = interfaces.is_empty() || reasons.len() < interfaces.len();
        if !needs_driver {
            note = reasons.first().map(|r| (*r).to_owned());
        }
        diagnose(None, &candidates, needs_driver)
    };

    let category = category(device_class, &interfaces);
    let interface_classes: Vec<u32> = interfaces
        .iter()
        .map(|i| (u32::from(i.class) << 16) | (u32::from(i.subclass) << 8) | u32::from(i.protocol))
        .collect();
    let description = descriptions.describe(
        Bus::Usb,
        &interface_classes,
        vendor_id,
        product_id,
        category,
    );

    // Prefer what the device says about itself: some devices borrow another maker's IDs
    // (many game pads report as an Xbox 360 controller), so the database can be wrong
    // about them. Use the database when the device's own name is missing or too vague.
    let own_product = product.clone().filter(|p| !rules.is_generic_usb_name(p));
    let db_product = names.product(vendor_id, product_id);
    let vendor_name = manufacturer
        .clone()
        .filter(|m| !rules.is_generic_usb_name(m))
        .or_else(|| names.vendor(vendor_id));
    let class_name = interfaces
        .first()
        .and_then(|i| names.class(i.class, i.subclass, i.protocol));
    let display_name = display_name(
        own_product.as_deref().or(db_product.as_deref()),
        product.is_some(),
        vendor_name.as_deref(),
        category,
    );
    let unrecognized = own_product.is_none() && db_product.is_none();

    Some(Device {
        bus: Bus::Usb,
        address,
        sysfs_path: path.to_path_buf(),
        display_name,
        vendor_name,
        product_name: own_product.or(db_product),
        model_name: None,
        class_name,
        category,
        hardware_id: format!("{vendor_id:04x}:{product_id:04x}"),
        unrecognized,
        modalias: interfaces.first().and_then(|i| i.modalias.clone()),
        driver,
        driver_candidates: candidates,
        status,
        missing_interface: None,
        log_issues: Vec::new(),
        note,
        description,
        software: Vec::new(),
        class_codes: interface_classes,
        fixes: Vec::new(),
    })
}

/// The interfaces of the device's active configuration, e.g. `1-5:1.0`, `1-5:1.1`.
fn read_interfaces(device_path: &Path, address: &str) -> Vec<Interface> {
    let prefix = format!("{address}:");
    let Ok(entries) = fs::read_dir(device_path) else {
        return Vec::new();
    };
    let mut interfaces: Vec<Interface> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(&prefix) {
                return None;
            }
            let path = entry.path();
            Some(Interface {
                class: read_hex(&path, "bInterfaceClass")? as u8,
                subclass: read_hex(&path, "bInterfaceSubClass").unwrap_or(0) as u8,
                protocol: read_hex(&path, "bInterfaceProtocol").unwrap_or(0) as u8,
                driver: driver_name(&path),
                modalias: read_trimmed(&path, "modalias"),
                children: child_names(&path),
                name,
            })
        })
        .collect();
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    interfaces
}

/// Group by what the interfaces say they are, then by what the drivers created, so new
/// devices land in the right place without any database knowing them.
fn category(device_class: u8, interfaces: &[Interface]) -> Category {
    let has = |class: u8, subclass: Option<u8>| {
        interfaces
            .iter()
            .any(|i| i.class == class && subclass.is_none_or(|s| s == i.subclass))
    };
    let by_class = if has(0xe0, Some(0x01)) {
        Category::Bluetooth
    } else if has(0x0e, None) || has(0x06, None) {
        Category::Camera
    } else if has(0x02, None) || has(0x0a, None) {
        Category::Network
    } else if has(0x08, None) {
        Category::Storage
    } else if has(0x07, None) {
        Category::Printers
    } else if has(0x0b, None) {
        Category::Security
    } else if has(0x01, None) {
        Category::Sound
    } else if has(0x03, None) {
        Category::Input
    } else if has(0x09, None) || device_class == 0x09 {
        Category::UsbControllers
    } else {
        Category::Other
    };
    if by_class != Category::Other {
        return by_class;
    }

    // Vendor-specific interfaces: look at what the driver made (e.g. a game controller
    // driver creates an input device, a Wi-Fi dongle driver a network interface).
    let made = |child: &str| {
        interfaces
            .iter()
            .any(|i| i.children.iter().any(|c| c == child))
    };
    if made("net") || made("ieee80211") {
        Category::Network
    } else if made("bluetooth") {
        Category::Bluetooth
    } else if made("video4linux") {
        Category::Camera
    } else if made("sound") {
        Category::Sound
    } else if made("input") {
        Category::Input
    } else {
        Category::Other
    }
}

/// The device's own product name, or a description built from what we know.
/// The best name: a specific product name if we have one; otherwise a description
/// from the maker and the kind of device. "Unknown" only when the device gave no name
/// at all (a vague one like "Controller" still tells us it identified itself).
fn display_name(
    product: Option<&str>,
    gave_a_name: bool,
    vendor: Option<&str>,
    category: Category,
) -> String {
    if let Some(product) = product {
        return product.to_owned();
    }
    let kind = match category {
        Category::Other => crate::tr!("USB device"),
        other => other.singular(),
    };
    let described = match vendor {
        Some(v) => crate::tr!("{} {}", short_vendor(v), kind),
        None => crate::tr!("USB {}", kind).replace("USB USB", "USB"),
    };
    if gave_a_name {
        described
    } else {
        crate::tr!("Unknown {}", described)
    }
}

fn driver_name(path: &Path) -> Option<String> {
    fs::read_link(path.join("driver"))
        .ok()
        .and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned()))
}

fn child_names(path: &Path) -> Vec<String> {
    fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn read_trimmed(dir: &Path, file: &str) -> Option<String> {
    let text = fs::read_to_string(dir.join(file)).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// USB sysfs IDs are bare hex, e.g. `046d`.
fn read_hex(dir: &Path, file: &str) -> Option<u16> {
    u16::from_str_radix(&read_trimmed(dir, file)?, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::DataDirs;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    struct Fake {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        drivers: PathBuf,
    }

    impl Fake {
        fn new() -> Self {
            let tmp = tempfile::tempdir().expect("tempdir");
            let root = tmp.path().join("devices");
            let drivers = tmp.path().join("drivers");
            fs::create_dir_all(&root).expect("mkdir");
            fs::create_dir_all(&drivers).expect("mkdir");
            Self {
                _tmp: tmp,
                root,
                drivers,
            }
        }

        fn device(&self, name: &str, vid: &str, pid: &str, class: &str, product: Option<&str>) {
            let dir = self.root.join(name);
            fs::create_dir_all(&dir).expect("mkdir");
            for (f, v) in [
                ("idVendor", vid),
                ("idProduct", pid),
                ("bDeviceClass", class),
            ] {
                fs::write(dir.join(f), format!("{v}\n")).expect("write");
            }
            fs::write(dir.join("authorized"), "1\n").expect("write");
            if let Some(p) = product {
                fs::write(dir.join("product"), format!("{p}  \n")).expect("write");
                fs::write(dir.join("manufacturer"), "Maker Inc.\n").expect("write");
            }
        }

        /// `class` like "e0/01/01"; `children` are folders the driver created.
        #[allow(clippy::too_many_arguments)]
        fn interface(
            &self,
            dev: &str,
            n: u8,
            vid: &str,
            pid: &str,
            class: &str,
            driver: Option<&str>,
            children: &[&str],
        ) {
            let name = format!("{dev}:1.{n}");
            let dir = self.root.join(dev).join(&name);
            fs::create_dir_all(&dir).expect("mkdir");
            let parts: Vec<&str> = class.split('/').collect();
            fs::write(dir.join("bInterfaceClass"), parts[0]).expect("write");
            fs::write(dir.join("bInterfaceSubClass"), parts[1]).expect("write");
            fs::write(dir.join("bInterfaceProtocol"), parts[2]).expect("write");
            let alias = format!(
                "usb:v{}p{}d0100dc00dsc00dp00ic{}isc{}ip{}in{n:02X}",
                vid.to_uppercase(),
                pid.to_uppercase(),
                parts[0].to_uppercase(),
                parts[1].to_uppercase(),
                parts[2].to_uppercase()
            );
            fs::write(dir.join("modalias"), alias).expect("write");
            for c in children {
                fs::create_dir_all(dir.join(c)).expect("mkdir");
            }
            if let Some(d) = driver {
                let target = self.drivers.join(d);
                fs::create_dir_all(&target).expect("mkdir");
                symlink(&target, dir.join("driver")).expect("symlink");
            }
        }

        fn scan(&self, alias: &str, cmdline: &str) -> Vec<Device> {
            let modules = self.root.parent().expect("parent").join("modules");
            fs::create_dir_all(&modules).expect("mkdir");
            fs::write(modules.join("modules.alias"), alias).expect("write");
            let db = AliasDb::load(&modules, &modules, "usb:");
            let rules = Rules::load(&DataDirs::only(Path::new("/nonexistent"))).0;
            let blocklist = Blocklist::load(&[], Some(cmdline));
            let ids = self.root.parent().expect("parent").join("usb.ids");
            fs::write(
                &ids,
                "# Version: 2026.01.01\n045e  Microsoft Corp.\n\t028e  Xbox360 Controller\n\
                 0bda  Realtek Semiconductor Corp.\nC 03  Human Interface Device\n\t00  No Subclass\n",
            )
            .expect("write");
            let names = UsbNames::from_files(&[ids]);
            scan(
                &self.root,
                &names,
                db.as_ref(),
                &blocklist,
                &rules,
                &Descriptions::embedded(),
            )
            .expect("scan")
        }
    }

    fn find<'a>(devices: &'a [Device], addr: &str) -> &'a Device {
        devices.iter().find(|d| d.address == addr).expect("device")
    }

    #[test]
    fn real_world_devices() {
        let f = Fake::new();
        // Root hub: skipped (the controller is listed via PCI).
        f.device("usb1", "1d6b", "0002", "09", Some("xHCI Host Controller"));
        // RGB controller: vendor-specific interface unbound, HID one bound -> working.
        f.device("1-11", "0b05", "19af", "00", Some("AURA LED Controller"));
        f.interface("1-11", 0, "0b05", "19af", "ff/ff/ff", None, &[]);
        f.interface("1-11", 2, "0b05", "19af", "03/00/00", Some("usbhid"), &[]);
        // Bluetooth dongle, working.
        f.device("1-5", "13d3", "3563", "ef", Some("Wireless_Device"));
        f.interface(
            "1-5",
            0,
            "13d3",
            "3563",
            "e0/01/01",
            Some("btusb"),
            &["bluetooth"],
        );
        // Game controller: vendor-specific, categorised by the input device it made.
        f.device("5-2", "045e", "028e", "ff", Some("Controller"));
        f.interface(
            "5-2",
            0,
            "045e",
            "028e",
            "ff/5d/01",
            Some("xpad"),
            &["input"],
        );
        f.interface("5-2", 1, "045e", "028e", "ff/5d/03", None, &[]);
        // Wi-Fi dongle with no driver anywhere: the case hwsight exists for.
        f.device("1-3", "0bda", "b812", "00", None);
        f.interface("1-3", 0, "0bda", "b812", "ff/ff/ff", None, &[]);
        // Bluetooth dongle whose driver is turned off.
        f.device("1-4", "8087", "0029", "e0", Some("AX200 Bluetooth"));
        f.interface("1-4", 0, "8087", "0029", "e0/01/01", None, &[]);

        let alias = "alias usb:v*p*d*dc*dsc*dp*icE0isc01ip01in* btusb\n";
        let devices = f.scan(alias, "modprobe.blacklist=btusb");

        assert!(
            devices.iter().all(|d| d.address != "usb1"),
            "root hub skipped"
        );
        let aura = find(&devices, "1-11");
        assert_eq!(aura.status, Status::Working);
        assert_eq!(aura.driver.as_deref(), Some("usbhid"));
        assert_eq!(aura.category, Category::Input);
        assert_eq!(aura.display_name, "AURA LED Controller");
        assert_eq!(aura.class_name, None); // first interface is vendor-specific (ff)

        let bt = find(&devices, "1-5");
        assert_eq!(bt.status, Status::Working);
        assert_eq!(bt.category, Category::Bluetooth);
        // "Wireless_Device" is vague and the database has no name: describe it.
        assert_eq!(bt.display_name, "Maker Bluetooth device");
        assert!(bt.unrecognized);

        let pad = find(&devices, "5-2");
        assert_eq!(pad.status, Status::Working);
        assert_eq!(pad.category, Category::Input);
        // "Controller" is too vague, so the database name is used; the maker the
        // device reported is kept (it borrows Microsoft's IDs).
        assert_eq!(pad.display_name, "Xbox360 Controller");
        assert_eq!(pad.vendor_name.as_deref(), Some("Maker Inc."));
        assert!(!pad.unrecognized);

        let wifi = find(&devices, "1-3");
        assert_eq!(wifi.status, Status::NoDriver);
        assert!(wifi.unrecognized);
        assert_eq!(
            wifi.display_name,
            "Unknown Realtek Semiconductor USB device"
        );
        assert_eq!(
            wifi.vendor_name.as_deref(),
            Some("Realtek Semiconductor Corp.")
        );
        assert_eq!(wifi.hardware_id, "0bda:b812");

        let blocked = find(&devices, "1-4");
        assert_eq!(blocked.status, Status::DriverBlocked);
        assert_eq!(blocked.driver_candidates[0].module, "btusb");
    }

    #[test]
    fn usbguard_blocked_devices() {
        let f = Fake::new();
        f.device("2-1", "0781", "5581", "00", Some("Ultra"));
        fs::write(f.root.join("2-1/authorized"), "0\n").expect("write");
        let devices = f.scan("", "");
        let d = find(&devices, "2-1");
        assert_eq!(d.status, Status::DriverBlocked);
        assert!(
            d.note
                .as_deref()
                .is_some_and(|n| n.contains("USB security"))
        );
    }

    #[test]
    fn names_when_nothing_specific_is_known() {
        assert_eq!(
            display_name(None, false, None, Category::Other),
            "Unknown USB device"
        );
        assert_eq!(
            display_name(None, true, None, Category::Other),
            "USB device"
        );
        assert_eq!(
            display_name(None, true, None, Category::Camera),
            "USB camera"
        );
        assert_eq!(
            display_name(None, true, Some("MediaTek Inc."), Category::Bluetooth),
            "MediaTek Bluetooth device"
        );
        assert_eq!(
            display_name(None, false, Some("Logitech, Inc."), Category::Input),
            "Unknown Logitech input device"
        );
        assert_eq!(
            display_name(Some("Logi Bolt Receiver"), true, None, Category::Input),
            "Logi Bolt Receiver"
        );
    }

    #[test]
    fn missing_usb_folder_is_empty() {
        let rules = Rules::embedded();
        let devices = scan(
            Path::new("/nonexistent/usb"),
            &UsbNames::embedded(),
            None,
            &Blocklist::default(),
            &rules,
            &Descriptions::embedded(),
        );
        assert!(devices.expect("ok").is_empty());
    }
}
