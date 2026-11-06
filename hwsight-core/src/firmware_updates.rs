//! Firmware versions and updates, shown on the device each belongs to.
//!
//! Drives, wireless receivers and the keyboards and mice paired to them, the processor's
//! secure processor, and the BIOS all have firmware. fwupd knows which versions they run
//! and, from its catalog, whether newer ones exist. This attaches that to the right
//! device as a "Firmware" group, with an amber finding when an update is waiting, or
//! when the catalog was never downloaded (so "no update" couldn't be trusted).

use std::collections::BTreeMap;

use crate::details::{Detail, DetailGroup, Verdict};
use crate::device::{Bus, Category, Device};
use crate::fwupd::{Catalog, Fwupd, FwupdDevice};

/// fwupd devices that are noise to a normal user: key databases, a monitor's connector,
/// pins, and the processor (its microcode is already shown with the processor).
const SKIPPED: [&str; 7] = [
    "gpio",
    "linux_display",
    "amd_gpu",
    "cpu",
    "uefi_pk",
    "uefi_kek",
    "uefi_db",
];

/// Firmware that belongs to the computer as a whole.
const SYSTEM_PLUGINS: [&str; 4] = ["uefi_capsule", "uefi_dbx", "tpm", "bios"];

const STORAGE_PLUGINS: [&str; 3] = ["nvme", "ata", "scsi"];

/// "NVME\VEN_C0A9&DEV_5427" -> "c0a9:5427"; the same for PCI, USB, and Unifying IDs.
fn hardware_id(instance_id: &str) -> Option<String> {
    let (kind, rest) = instance_id.split_once('\\')?;
    if !matches!(kind, "PCI" | "NVME" | "USB" | "UFY" | "HIDRAW") {
        return None;
    }
    let mut vendor = None;
    let mut product = None;
    for field in rest.split('&') {
        if let Some(v) = field
            .strip_prefix("VEN_")
            .or_else(|| field.strip_prefix("VID_"))
        {
            vendor = Some(v);
        } else if let Some(p) = field
            .strip_prefix("DEV_")
            .or_else(|| field.strip_prefix("PID_"))
        {
            product = Some(p);
        }
    }
    let (vendor, product) = (vendor?, product?);
    let hex4 = |s: &str| s.len() == 4 && s.chars().all(|c| c.is_ascii_hexdigit());
    (hex4(vendor) && hex4(product)).then(|| format!("{vendor}:{product}").to_lowercase())
}

/// Drive models are cut short by the kernel ("ST8000DM004-2CX1"), so a name matches when
/// one is the start of the other.
fn same_model(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_lowercase(), b.trim().to_lowercase());
    a.len() >= 6 && b.len() >= 6 && (a.starts_with(&b) || b.starts_with(&a))
}

/// Which device this firmware belongs to.
fn target(fw: &FwupdDevice, all: &[FwupdDevice], devices: &[Device], depth: u8) -> Option<usize> {
    if SKIPPED.contains(&fw.plugin.as_str()) || (fw.version.is_none() && !fw.updatable) {
        return None;
    }
    if STORAGE_PLUGINS.contains(&fw.plugin.as_str()) {
        return devices
            .iter()
            .position(|d| d.category == Category::Drives && same_model(&d.hardware_id, &fw.name));
    }
    if SYSTEM_PLUGINS.contains(&fw.plugin.as_str()) {
        return devices.iter().position(|d| d.address == "motherboard");
    }
    // A paired keyboard or mouse belongs with the receiver it's reached through.
    if let Some(parent) = fw
        .parent
        .as_deref()
        .and_then(|id| all.iter().find(|d| d.id == id))
        && depth < 3
    {
        return target(parent, all, devices, depth + 1);
    }
    let ids: Vec<String> = fw.ids.iter().filter_map(|i| hardware_id(i)).collect();
    devices
        .iter()
        .position(|d| matches!(d.bus, Bus::Pci | Bus::Usb) && ids.contains(&d.hardware_id))
}

fn catalog_finding(catalog: Catalog) -> Option<Detail> {
    let label = crate::tr!("Firmware update check");
    let source = crate::tr!("The fwupd firmware service");
    match catalog {
        Catalog::Fresh => None,
        Catalog::NeverDownloaded => Some(
            Detail::new(label, crate::tr!("Not set up"), source)
                .verdict(
                    Verdict::Reduced,
                    Some(crate::tr!(
                        "hwsight can't tell whether your devices have firmware updates, because fwupd has never downloaded its list of available updates. This command downloads it from the Linux Vendor Firmware Service. To keep it up to date automatically, turn on the timer: sudo systemctl enable --now fwupd-refresh.timer"
                    )),
                )
                .command("fwupdmgr refresh"),
        ),
        Catalog::Stale { days } => Some(
            Detail::new(label, crate::tr!("{} days out of date", days), source)
                .verdict(
                    Verdict::Reduced,
                    Some(crate::tr!(
                        "fwupd's list of available firmware updates is old, so newer updates may be missing. This command refreshes it."
                    )),
                )
                .command("fwupdmgr refresh"),
        ),
        Catalog::NoSource => Some(
            Detail::new(label, crate::tr!("Off"), source)
                .verdict(
                    Verdict::Info,
                    Some(crate::tr!(
                        "No source of firmware updates is switched on in fwupd, so updates can't be found. This command switches on the Linux Vendor Firmware Service."
                    )),
                )
                .command("sudo fwupdmgr enable-remote lvfs"),
        ),
    }
}

fn group_for(owned: &[&FwupdDevice], category: Category, catalog: Catalog) -> Option<DetailGroup> {
    let source = crate::tr!("The fwupd firmware service");
    let mut items = Vec::new();
    let mut checked = false;
    let mut any_update = false;
    for fw in owned {
        // The device's own firmware reads "Firmware version"; others are named.
        // A device's own firmware is the one not reached through another device. On the
        // motherboard every piece (BIOS, TPM, ...) is named instead.
        let own = category != Category::Motherboard && fw.parent.is_none();
        let label = if own {
            crate::tr!("Firmware version")
        } else {
            fw.name.clone()
        };
        items.push(Detail::new(
            label,
            fw.version.clone().unwrap_or_else(|| crate::tr!("Unknown")),
            source.clone(),
        ));
        if let Some((level, threshold)) = fw.battery {
            let low = level <= threshold;
            items.push(
                Detail::new(
                    crate::tr!("{} battery", fw.name),
                    format!("{level}%"),
                    source.clone(),
                )
                .verdict(
                    if low { Verdict::Reduced } else { Verdict::Good },
                    low.then(|| crate::tr!("The battery is low. Recharge or replace it soon.")),
                ),
            );
        }
        if fw.updatable && matches!(catalog, Catalog::Fresh | Catalog::Stale { .. }) {
            checked = true;
        }
        if let Some(update) = &fw.update {
            any_update = true;
            let mut why = update.summary.clone().unwrap_or_default();
            if !why.is_empty() {
                why.push(' ');
            }
            why.push_str(&crate::tr!(
                "Newer firmware can fix bugs and security problems."
            ));
            if matches!(category, Category::Drives | Category::Motherboard) {
                why.push(' ');
                why.push_str(&crate::tr!(
                    "Back up important files first, and keep the computer plugged in while it updates."
                ));
            }
            let label = if own {
                crate::tr!("Firmware update")
            } else {
                crate::tr!("{} update", fw.name)
            };
            items.push(
                Detail::new(
                    label,
                    crate::tr!("Version {} is available", update.version),
                    source.clone(),
                )
                .verdict(Verdict::Reduced, Some(why))
                .command("fwupdmgr update"),
            );
        }
    }
    if checked && !any_update {
        items.push(
            Detail::new(
                crate::tr!("Firmware update check"),
                crate::tr!("No updates available"),
                source,
            )
            .verdict(Verdict::Good, None),
        );
    }
    (!items.is_empty()).then(|| DetailGroup {
        title: crate::tr!("Firmware"),
        items,
    })
}

/// Add a "Firmware" group to every device fwupd has something to say about.
pub(crate) fn attach(devices: &mut [Device], fwupd: &Fwupd) {
    let mut by_device: BTreeMap<usize, Vec<&FwupdDevice>> = BTreeMap::new();
    for fw in &fwupd.devices {
        if let Some(index) = target(fw, &fwupd.devices, devices, 0) {
            by_device.entry(index).or_default().push(fw);
        }
    }
    for (index, owned) in by_device {
        let category = devices[index].category;
        if let Some(mut group) = group_for(&owned, category, fwupd.catalog) {
            // The catalog problem belongs with the computer, not with every drive.
            if devices[index].address == "motherboard"
                && let Some(finding) = catalog_finding(fwupd.catalog)
            {
                group.items.insert(0, finding);
            }
            devices[index].details.push(group);
        }
    }
    // A computer with nothing else to show still needs to hear that updates can't be found.
    if let Some(board) = devices.iter_mut().find(|d| d.address == "motherboard")
        && !board
            .details
            .iter()
            .any(|g| g.title == crate::tr!("Firmware"))
        && let Some(finding) = catalog_finding(fwupd.catalog)
    {
        board.details.push(DetailGroup {
            title: crate::tr!("Firmware"),
            items: vec![finding],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::BuiltIn;
    use crate::fwupd::Update;
    use crate::i18n::in_english;
    use std::path::PathBuf;

    fn device(address: &str, bus: Bus, category: Category, id: &str) -> Device {
        let mut d = Device::built_in(BuiltIn {
            address: "x",
            sysfs_path: PathBuf::new(),
            display_name: address.to_owned(),
            vendor_name: None,
            category,
            hardware_id: id.to_owned(),
            description: String::new(),
            details: Vec::new(),
        });
        d.address = address.to_owned();
        d.bus = bus;
        d
    }

    fn fw(id: &str, name: &str, plugin: &str, version: Option<&str>) -> FwupdDevice {
        FwupdDevice {
            id: id.to_owned(),
            parent: None,
            name: name.to_owned(),
            version: version.map(str::to_owned),
            plugin: plugin.to_owned(),
            updatable: true,
            ids: Vec::new(),
            battery: None,
            update: None,
        }
    }

    fn computer() -> Vec<Device> {
        vec![
            device("sdb", Bus::System, Category::Drives, "ST8000DM004-2CX1"),
            device("nvme0n1", Bus::System, Category::Drives, "CT1000P310SSD8"),
            device("1-2", Bus::Usb, Category::Input, "046d:c548"),
            device(
                "motherboard",
                Bus::System,
                Category::Motherboard,
                "TUF GAMING",
            ),
            device("processor", Bus::System, Category::Processors, "family 25"),
        ]
    }

    fn group<'a>(devices: &'a [Device], address: &str) -> Option<&'a DetailGroup> {
        devices
            .iter()
            .find(|d| d.address == address)?
            .details
            .iter()
            .find(|g| g.title == "Firmware")
    }

    fn item<'a>(group: &'a DetailGroup, label: &str) -> &'a Detail {
        group
            .items
            .iter()
            .find(|d| d.label == label)
            .unwrap_or_else(|| panic!("no {label:?} in {group:#?}"))
    }

    fn run(devices: &mut [Device], catalog: Catalog, list: Vec<FwupdDevice>) {
        in_english(|| {
            attach(
                devices,
                &Fwupd {
                    catalog,
                    devices: list,
                },
            );
        });
    }

    #[test]
    fn instance_ids_become_hardware_ids() {
        assert_eq!(
            hardware_id("NVME\\VEN_C0A9&DEV_5427").as_deref(),
            Some("c0a9:5427")
        );
        assert_eq!(
            hardware_id("UFY\\VID_046D&PID_C548").as_deref(),
            Some("046d:c548")
        );
        assert_eq!(
            hardware_id("PCI\\VEN_1022&DEV_1649&REV_00").as_deref(),
            Some("1022:1649")
        );
        assert_eq!(
            hardware_id("HIDRAW\\VEN_046D&MOD_B38E00000000"),
            None,
            "no product id"
        );
        assert_eq!(hardware_id("IDE\\ST8000"), None);
        assert!(same_model("ST8000DM004-2CX188", "ST8000DM004-2CX1"));
        assert!(!same_model("ST8000DM004-2CX188", "ST8000VN002-2ZM1"));
        assert!(!same_model("abc", "abc"), "too short to trust");
    }

    #[test]
    fn a_drive_gets_its_firmware_version_and_an_all_clear_when_the_catalog_is_fresh() {
        let mut devices = computer();
        run(
            &mut devices,
            Catalog::Fresh,
            vec![fw("1", "ST8000DM004-2CX188", "ata", Some("0001"))],
        );
        let g = group(&devices, "sdb").expect("group");
        assert_eq!(item(g, "Firmware version").value, "0001");
        assert_eq!(
            item(g, "Firmware update check").value,
            "No updates available"
        );
        assert_eq!(item(g, "Firmware update check").verdict, Verdict::Good);
        assert!(
            group(&devices, "nvme0n1").is_none(),
            "the other drive has nothing"
        );
    }

    #[test]
    fn an_available_update_is_an_amber_finding_with_a_command_and_a_backup_warning() {
        let mut devices = computer();
        let mut drive = fw("1", "CT1000P310SSD8", "nvme", Some("VACR001"));
        drive.update = Some(Update {
            version: "VACR002".into(),
            summary: Some("Fixes a power bug.".into()),
        });
        run(&mut devices, Catalog::Fresh, vec![drive]);
        let g = group(&devices, "nvme0n1").expect("group");
        let update = item(g, "Firmware update");
        assert_eq!(update.value, "Version VACR002 is available");
        assert_eq!(update.verdict, Verdict::Reduced);
        assert_eq!(update.command.as_deref(), Some("fwupdmgr update"));
        let why = update.explanation.as_deref().expect("why");
        assert!(why.starts_with("Fixes a power bug."));
        assert!(why.contains("Back up important files first"));
        assert!(
            g.items.iter().all(|i| i.label != "Firmware update check"),
            "no all-clear when there is an update"
        );
    }

    #[test]
    fn paired_wireless_devices_attach_to_their_receiver_with_battery_levels() {
        let mut devices = computer();
        let receiver = {
            let mut r = fw(
                "r",
                "Bolt Receiver",
                "logitech_hidpp",
                Some("MPR05.03_B0020"),
            );
            r.ids = vec!["UFY\\VID_046D&PID_C548".into()];
            r
        };
        let mut keyboard = fw(
            "k",
            "Alto Keys K98",
            "logitech_hidpp",
            Some("RBK93.02_B0009"),
        );
        keyboard.parent = Some("r".into());
        keyboard.battery = Some((100, 20));
        let mut mouse = fw("m", "Logi M650", "logitech_hidpp", Some("RBM16.10_B0014"));
        mouse.parent = Some("r".into());
        mouse.battery = Some((15, 20));
        run(
            &mut devices,
            Catalog::Fresh,
            vec![receiver, keyboard, mouse],
        );
        let g = group(&devices, "1-2").expect("receiver group");
        assert_eq!(item(g, "Alto Keys K98").value, "RBK93.02_B0009");
        assert_eq!(item(g, "Alto Keys K98 battery").value, "100%");
        assert_eq!(item(g, "Alto Keys K98 battery").verdict, Verdict::Good);
        let low = item(g, "Logi M650 battery");
        assert_eq!(low.verdict, Verdict::Reduced);
        assert!(
            low.explanation
                .as_deref()
                .is_some_and(|t| t.contains("low"))
        );
        assert_eq!(item(g, "Firmware version").value, "MPR05.03_B0020");
    }

    #[test]
    fn a_catalog_that_was_never_downloaded_is_called_out_on_the_motherboard() {
        let mut devices = computer();
        run(
            &mut devices,
            Catalog::NeverDownloaded,
            vec![
                fw("s", "System Firmware", "uefi_capsule", Some("12899")),
                fw("d", "UEFI dbx", "uefi_dbx", Some("20230301")),
                fw("t", "TPM", "tpm", Some("6.32.0.6")),
                fw("c", "KEK CA", "uefi_kek", Some("2023")),
                fw("g", "GPIO controller", "gpio", None),
            ],
        );
        let g = group(&devices, "motherboard").expect("group");
        let check = item(g, "Firmware update check");
        assert_eq!(check.value, "Not set up");
        assert_eq!(check.verdict, Verdict::Reduced);
        assert_eq!(check.command.as_deref(), Some("fwupdmgr refresh"));
        assert_eq!(
            g.items[0].label, "Firmware update check",
            "the problem comes first"
        );
        assert_eq!(item(g, "System Firmware").value, "12899");
        assert_eq!(item(g, "UEFI dbx").value, "20230301");
        assert_eq!(item(g, "TPM").value, "6.32.0.6");
        assert!(
            g.items.iter().all(|i| i.label != "KEK CA"),
            "key databases are noise"
        );
        assert!(
            g.items
                .iter()
                .all(|i| i.label != "Firmware update check" || i.value == "Not set up"),
            "no all-clear when the catalog is missing"
        );
    }

    #[test]
    fn stale_and_switched_off_catalogs_say_so() {
        let mut stale = computer();
        run(
            &mut stale,
            Catalog::Stale { days: 45 },
            vec![fw("t", "TPM", "tpm", Some("1"))],
        );
        let check = item(
            group(&stale, "motherboard").expect("group"),
            "Firmware update check",
        );
        assert_eq!(check.value, "45 days out of date");
        assert_eq!(check.command.as_deref(), Some("fwupdmgr refresh"));

        let mut off = computer();
        run(&mut off, Catalog::NoSource, Vec::new());
        let check = item(
            group(&off, "motherboard").expect("group"),
            "Firmware update check",
        );
        assert_eq!(check.value, "Off");
        assert_eq!(
            check.verdict,
            Verdict::Info,
            "not the user's fault, so not amber"
        );
        assert_eq!(
            check.command.as_deref(),
            Some("sudo fwupdmgr enable-remote lvfs")
        );

        let mut fine = computer();
        run(&mut fine, Catalog::Fresh, Vec::new());
        assert!(group(&fine, "motherboard").is_none(), "nothing to say");
    }
}
