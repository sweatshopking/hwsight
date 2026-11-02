//! PCI devices: graphics cards, network and sound chips, storage controllers, USB
//! controllers, and the motherboard's own internals.
//!
//! Everything comes from `/sys/bus/pci/devices`, which any user can read.

use std::fs;
use std::io;
use std::path::Path;

use crate::descriptions::Descriptions;
use crate::device::{Bus, Category, Device, Status, bracketed, diagnose, short_vendor};
use crate::modules::{AliasDb, Blocklist};
use crate::pci_ids::PciNames;
use crate::rules::Rules;

/// Read every PCI device under `devices_dir` (normally `/sys/bus/pci/devices`).
/// A missing folder means the machine has no PCI bus, which isn't an error.
pub(crate) fn scan(
    devices_dir: &Path,
    names: &PciNames,
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
        // Entries are symlinks into /sys/devices; follow them for the real path.
        let path = fs::canonicalize(entry.path()).unwrap_or_else(|_| entry.path());
        let address = entry.file_name().to_string_lossy().into_owned();
        if let Some(device) = read_device(
            &path,
            address,
            names,
            aliases,
            blocklist,
            rules,
            descriptions,
        ) {
            devices.push(device);
        }
    }
    devices.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then_with(|| a.address.cmp(&b.address))
    });
    Ok(devices)
}

fn read_device(
    path: &Path,
    address: String,
    names: &PciNames,
    aliases: Option<&AliasDb>,
    blocklist: &Blocklist,
    rules: &Rules,
    descriptions: &Descriptions,
) -> Option<Device> {
    // Vendor, device, and class are always present on a real PCI device. Without them
    // there's nothing meaningful to show, so skip the entry.
    let vendor_id = read_hex(path, "vendor").and_then(|v| u16::try_from(v).ok())?;
    let device_id = read_hex(path, "device").and_then(|v| u16::try_from(v).ok())?;
    let class_code = read_hex(path, "class")? & 0x00ff_ffff;
    let sub_vendor = read_hex(path, "subsystem_vendor").and_then(|v| u16::try_from(v).ok());
    let sub_device = read_hex(path, "subsystem_device").and_then(|v| u16::try_from(v).ok());

    let class = (class_code >> 16) as u8;
    let subclass = (class_code >> 8) as u8;
    let prog_if = class_code as u8;

    let vendor_name = names.vendor(vendor_id);
    let product_name = names.device(vendor_id, device_id);
    let model_name = sub_vendor
        .zip(sub_device)
        .and_then(|(sv, sd)| names.subsystem(vendor_id, device_id, sv, sd));
    let class_name = names.class(class, subclass);

    let modalias = read_trimmed(path, "modalias");
    let driver = fs::read_link(path.join("driver"))
        .ok()
        .and_then(|target| target.file_name().map(|n| n.to_string_lossy().into_owned()));
    let driver_candidates = match (aliases, &modalias) {
        (Some(db), Some(alias)) => db.candidates(alias, blocklist),
        _ => Vec::new(),
    };
    let no_driver_reason = rules.no_driver_needed(Bus::Pci, class_code, vendor_id, device_id);
    let needs_driver = no_driver_reason.is_none();
    let mut status = diagnose(driver.as_deref(), &driver_candidates, needs_driver);

    // A driver being attached isn't proof the device works: check it created what it
    // should have (a network connection, a display device, ...).
    let mut missing_interface = None;
    if status == Status::Working
        && let Some(expected) = rules.expected_interface(class_code, vendor_id, device_id)
        && !has_child_starting_with(path, expected.interface)
    {
        status = Status::DriverNotReady;
        missing_interface = Some(expected.what.to_owned());
    }

    let device_category = category(class, subclass, prog_if);
    let description = descriptions.describe(
        Bus::Pci,
        &[class_code],
        vendor_id,
        device_id,
        device_category,
    );

    let display_name = display_name(
        product_name.as_deref(),
        vendor_name.as_deref(),
        class_name.as_deref(),
    );

    Some(Device {
        bus: Bus::Pci,
        address,
        sysfs_path: path.to_path_buf(),
        display_name,
        unrecognized: product_name.is_none(),
        vendor_name,
        product_name,
        model_name,
        class_name,
        category: device_category,
        hardware_id: format!("{vendor_id:04x}:{device_id:04x}"),
        modalias,
        driver,
        driver_candidates,
        status,
        missing_interface,
        note: (status == Status::NoDriverNeeded)
            .then(|| no_driver_reason.map(str::to_owned))
            .flatten(),
        log_issues: Vec::new(),
        description,
    })
}

fn has_child_starting_with(dir: &Path, prefix: &str) -> bool {
    fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with(prefix))
    })
}

/// The best short name we can make. Unknown hardware still gets a useful description.
fn display_name(product: Option<&str>, vendor: Option<&str>, class: Option<&str>) -> String {
    if let Some(product) = product {
        return bracketed(product).unwrap_or(product).to_owned();
    }
    let kind = class.map_or_else(|| "device".to_owned(), str::to_lowercase);
    match vendor {
        Some(vendor) => format!("Unknown {} {kind}", short_vendor(vendor)),
        None => format!("Unknown {kind}"),
    }
}

/// Group by the class code the hardware reports, so brand new devices land in the right
/// place without any database knowing them.
fn category(class: u8, subclass: u8, prog_if: u8) -> Category {
    match (class, subclass) {
        (0x00, 0x01) | (0x03, _) => Category::Graphics,
        (0x01, _) => Category::Storage,
        (0x02, _) | (0x0c, 0x06) => Category::Network,
        (0x04, 0x01 | 0x03) => Category::Sound,
        (0x04, 0x00) => Category::Camera, // multimedia video, e.g. capture cards
        (0x04, _) => Category::Sound,
        (0x07, _) | (0x0c, 0x09) | (0x0f, _) => Category::Communication,
        (0x09, _) => Category::Input,
        (0x0b, _) => Category::Processors,
        // USB4 host interfaces are USB class 0c03 with programming interface 40.
        (0x0c, 0x03) if prog_if == 0x40 => Category::Thunderbolt,
        (0x0c, 0x03) => Category::UsbControllers,
        (0x0c, 0x0a) => Category::Thunderbolt,
        (0x0d, 0x11) => Category::Bluetooth,
        (0x0d, _) => Category::Network,
        (0x10, _) => Category::Security,
        (0x12, _) => Category::Accelerators,
        (0x05 | 0x06 | 0x08 | 0x0a | 0x0c | 0x0e | 0x11 | 0x13, _) => Category::SystemDevices,
        _ => Category::Other,
    }
}

fn read_trimmed(dir: &Path, file: &str) -> Option<String> {
    let text = fs::read_to_string(dir.join(file)).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// sysfs IDs look like `0x10de`.
fn read_hex(dir: &Path, file: &str) -> Option<u32> {
    let text = read_trimmed(dir, file)?;
    u32::from_str_radix(text.strip_prefix("0x").unwrap_or(&text), 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::DataDirs;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    struct FakeSys {
        _tmp: tempfile::TempDir,
        devices: PathBuf,
        modules: PathBuf,
        sys_module: PathBuf,
        cmdline: String,
    }

    impl FakeSys {
        fn new() -> Self {
            let tmp = tempfile::tempdir().expect("tempdir");
            let root = tmp.path().to_path_buf();
            let fake = Self {
                devices: root.join("devices"),
                modules: root.join("modules"),
                sys_module: root.join("sys_module"),
                cmdline: "quiet modprobe.blacklist=btusb_fake".to_owned(),
                _tmp: tmp,
            };
            for d in [
                &fake.devices,
                &fake.modules,
                &fake.sys_module,
                &root.join("drivers"),
            ] {
                fs::create_dir_all(d).expect("mkdir");
            }
            fake
        }

        fn add(&self, addr: &str, vendor: &str, device: &str, class: &str, driver: Option<&str>) {
            let dir = self.devices.join(addr);
            fs::create_dir_all(&dir).expect("mkdir");
            fs::write(dir.join("vendor"), format!("0x{vendor}\n")).expect("write");
            fs::write(dir.join("device"), format!("0x{device}\n")).expect("write");
            fs::write(dir.join("class"), format!("0x{class}\n")).expect("write");
            fs::write(dir.join("subsystem_vendor"), "0x1043\n").expect("write");
            fs::write(dir.join("subsystem_device"), "0x8877\n").expect("write");
            let (c, s, i) = (&class[0..2], &class[2..4], &class[4..6]);
            let alias = format!(
                "pci:v0000{}d0000{}sv00001043sd00008877bc{}sc{}i{}\n",
                vendor.to_uppercase(),
                device.to_uppercase(),
                c.to_uppercase(),
                s.to_uppercase(),
                i.to_uppercase()
            );
            fs::write(dir.join("modalias"), alias).expect("write");
            if let Some(drv) = driver {
                let target = self
                    .devices
                    .parent()
                    .expect("parent")
                    .join("drivers")
                    .join(drv);
                fs::create_dir_all(&target).expect("mkdir");
                symlink(&target, dir.join("driver")).expect("symlink");
            }
        }

        fn scan(&self, alias: &str, loaded: &[&str]) -> Vec<Device> {
            fs::write(self.modules.join("modules.alias"), alias).expect("write");
            for m in loaded {
                fs::create_dir_all(self.sys_module.join(m)).expect("mkdir");
            }
            let aliases = AliasDb::load(&self.modules, &self.sys_module, "pci:");
            let names = PciNames::embedded();
            let (rules, _) = Rules::load(&DataDirs::only(Path::new("/nonexistent")));
            let blocklist = Blocklist::load(&[], Some(&self.cmdline));
            scan(
                &self.devices,
                &names,
                aliases.as_ref(),
                &blocklist,
                &rules,
                &Descriptions::embedded(),
            )
            .expect("scan")
        }
    }

    fn by_addr<'a>(devices: &'a [Device], addr: &str) -> &'a Device {
        devices
            .iter()
            .find(|d| d.address == addr)
            .expect("device present")
    }

    #[test]
    fn statuses_for_each_situation() {
        let sys = FakeSys::new();
        sys.add("0000:00:00.0", "1022", "14d8", "060000", None); // host bridge
        sys.add("0000:01:00.0", "10de", "2c05", "030000", Some("nvidia")); // working GPU
        sys.add("0000:02:00.0", "8086", "2725", "028000", None); // wifi, driver not loaded
        sys.add("0000:03:00.0", "10ec", "8125", "020000", None); // ethernet, driver failed
        sys.add("0000:04:00.0", "ffee", "0001", "ff0000", None); // nothing knows it
        sys.add("0000:05:00.0", "8086", "a0f0", "028000", None); // driver blocked
        sys.add("0000:06:00.0", "10de", "2c05", "030000", Some("vfio-pci")); // VM passthrough
        sys.add("0000:08:00.0", "14c3", "7961", "028000", Some("mt7921e")); // wifi, no network
        // The working GPU's driver created its display device.
        fs::create_dir_all(sys.devices.join("0000:01:00.0/drm/card1")).expect("mkdir");

        let alias = "\
alias pci:v00008086d00002725sv*sd*bc*sc*i* iwlwifi
alias pci:v000010ECd00008125sv*sd*bc*sc*i* r8169
alias pci:v00008086d0000A0F0sv*sd*bc*sc*i* btusb_fake
";
        let devices = sys.scan(alias, &["r8169"]);
        assert_eq!(devices.len(), 8);
        let wifi = by_addr(&devices, "0000:08:00.0");
        assert_eq!(wifi.status, Status::DriverNotReady);
        assert_eq!(
            wifi.missing_interface.as_deref(),
            Some("network connection")
        );
        assert_eq!(wifi.driver.as_deref(), Some("mt7921e"));
        assert_eq!(
            by_addr(&devices, "0000:05:00.0").status,
            Status::DriverBlocked
        );
        assert_eq!(
            by_addr(&devices, "0000:06:00.0").status,
            Status::ReservedForVirtualMachine
        );
        assert_eq!(
            by_addr(&devices, "0000:00:00.0").status,
            Status::NoDriverNeeded
        );
        let gpu = by_addr(&devices, "0000:01:00.0");
        assert_eq!(gpu.status, Status::Working);
        assert_eq!(gpu.driver.as_deref(), Some("nvidia"));
        assert_eq!(gpu.category, Category::Graphics);
        assert_eq!(
            by_addr(&devices, "0000:02:00.0").status,
            Status::DriverAvailableNotLoaded
        );
        assert_eq!(
            by_addr(&devices, "0000:03:00.0").status,
            Status::DriverNotAttached
        );
        let unknown = by_addr(&devices, "0000:04:00.0");
        assert_eq!(unknown.status, Status::NoDriver);
        assert!(unknown.unrecognized);
        assert_eq!(unknown.hardware_id, "ffee:0001");
        assert!(unknown.display_name.starts_with("Unknown"));
    }

    #[test]
    fn devices_sorted_by_category() {
        let sys = FakeSys::new();
        sys.add("0000:00:00.0", "1022", "14d8", "060000", None);
        sys.add("0000:01:00.0", "10de", "2c05", "030000", Some("nvidia"));
        let devices = sys.scan("", &[]);
        assert_eq!(devices[0].category, Category::Graphics);
        assert_eq!(devices[1].category, Category::SystemDevices);
    }

    #[test]
    fn missing_pci_bus_is_empty_not_an_error() {
        let names = PciNames::embedded();
        let rules = Rules::embedded();
        let devices = scan(
            Path::new("/nonexistent/pci"),
            &names,
            None,
            &Blocklist::default(),
            &rules,
            &Descriptions::embedded(),
        )
        .expect("ok");
        assert!(devices.is_empty());
    }

    #[test]
    fn unknown_devices_still_get_useful_names() {
        assert_eq!(
            display_name(
                None,
                Some("Realtek Semiconductor Co., Ltd."),
                Some("Network controller")
            ),
            "Unknown Realtek Semiconductor network controller"
        );
        assert_eq!(
            display_name(None, None, Some("Audio device")),
            "Unknown audio device"
        );
        assert_eq!(display_name(None, None, None), "Unknown device");
        assert_eq!(
            display_name(
                Some("GB203 [GeForce RTX 5070 Ti]"),
                Some("NVIDIA Corporation"),
                None
            ),
            "GeForce RTX 5070 Ti"
        );
    }

    #[test]
    fn categories_from_class_codes() {
        assert_eq!(category(0x03, 0x00, 0x00), Category::Graphics);
        assert_eq!(category(0x04, 0x03, 0x00), Category::Sound);
        assert_eq!(category(0x0c, 0x03, 0x30), Category::UsbControllers);
        assert_eq!(category(0x0c, 0x03, 0x40), Category::Thunderbolt);
        assert_eq!(category(0x0d, 0x11, 0x00), Category::Bluetooth);
        assert_eq!(category(0x02, 0x80, 0x00), Category::Network);
        assert_eq!(category(0x01, 0x08, 0x02), Category::Storage);
        assert_eq!(category(0xff, 0x00, 0x00), Category::Other);
    }
}
