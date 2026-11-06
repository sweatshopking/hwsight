//! The motherboard and its BIOS, from the firmware's own labels in `/sys/class/dmi/id`.
//! (Serial numbers are in that folder too; they're deliberately never read.)

use std::fs;
use std::path::Path;

use crate::descriptions::Descriptions;
use crate::details::{Detail, DetailGroup};
use crate::device::{BuiltIn, Bus, Category, Device};
use crate::system::clean;

fn read(dir: &Path, name: &str) -> Option<String> {
    clean(fs::read_to_string(dir.join(name)).ok().as_ref())
}

fn detail(dir: &Path, name: &str, label: String) -> Option<Detail> {
    let value = read(dir, name)?;
    Some(Detail::new(
        label,
        value,
        dir.join(name).display().to_string(),
    ))
}

/// The motherboard as one device, or `None` when the firmware doesn't name it.
pub(crate) fn device(dir: &Path, efi_dir: &Path, descriptions: &Descriptions) -> Option<Device> {
    let vendor = read(dir, "board_vendor");
    let model = read(dir, "board_name");
    if vendor.is_none() && model.is_none() {
        return None;
    }
    let board: Vec<Detail> = [
        detail(dir, "board_vendor", crate::tr!("Maker")),
        detail(dir, "board_name", crate::tr!("Model")),
        detail(dir, "board_version", crate::tr!("Revision")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut bios: Vec<Detail> = [
        detail(dir, "bios_vendor", crate::tr!("Maker")),
        detail(dir, "bios_version", crate::tr!("Version")),
        detail(dir, "bios_date", crate::tr!("Date")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mode = if efi_dir.is_dir() {
        crate::tr!("UEFI")
    } else {
        crate::tr!("Legacy BIOS")
    };
    bios.push(Detail::new(
        crate::tr!("Started in"),
        mode,
        efi_dir.display().to_string(),
    ));
    let groups = vec![
        DetailGroup {
            title: crate::tr!("Motherboard"),
            items: board,
        },
        DetailGroup {
            title: crate::tr!("BIOS"),
            items: bios,
        },
    ];
    let description = descriptions
        .describe(Bus::System, &[], 0, 0, Category::Motherboard)
        .unwrap_or_default();
    Some(Device::built_in(BuiltIn {
        address: "motherboard",
        sysfs_path: dir.to_path_buf(),
        display_name: model.clone().or_else(|| vendor.clone()).unwrap_or_default(),
        vendor_name: vendor,
        category: Category::Motherboard,
        hardware_id: model.unwrap_or_default(),
        description,
        details: groups,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::in_english;

    fn dmi(files: &[(&str, &str)]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        for (name, text) in files {
            fs::write(temp.path().join(name), format!("{text}\n")).expect("write");
        }
        temp
    }

    fn build(dir: &Path, efi: &Path) -> Option<Device> {
        in_english(|| device(dir, efi, &Descriptions::embedded()))
    }

    #[test]
    fn names_the_board_and_bios_and_skips_placeholders_and_serials() {
        let temp = dmi(&[
            ("board_vendor", "ASUSTeK COMPUTER INC."),
            ("board_name", "TUF GAMING B650-PLUS WIFI"),
            ("board_version", "Rev 1.xx"),
            ("board_serial", "SECRET123"),
            ("bios_vendor", "American Megatrends Inc."),
            ("bios_version", "3263"),
            ("bios_date", "06/09/2025"),
        ]);
        let efi = tempfile::tempdir().expect("tempdir");
        let d = build(temp.path(), efi.path()).expect("board");
        assert_eq!(d.display_name, "TUF GAMING B650-PLUS WIFI");
        assert_eq!(d.vendor_name.as_deref(), Some("ASUSTeK COMPUTER INC."));
        assert_eq!(d.category, Category::Motherboard);
        assert_eq!(d.hardware_id, "TUF GAMING B650-PLUS WIFI");
        let values: Vec<(&str, &str)> = d
            .details
            .iter()
            .flat_map(|g| &g.items)
            .map(|i| (i.label.as_str(), i.value.as_str()))
            .collect();
        assert!(values.contains(&("Revision", "Rev 1.xx")));
        assert!(values.contains(&("Version", "3263")));
        assert!(values.contains(&("Date", "06/09/2025")));
        assert!(values.contains(&("Started in", "UEFI")));
        assert!(
            !format!("{:?}", d.details).contains("SECRET123"),
            "serial numbers are never read"
        );
    }

    #[test]
    fn a_computer_without_uefi_says_legacy_and_placeholders_are_dropped() {
        let temp = dmi(&[
            ("board_vendor", "To Be Filled By O.E.M."),
            ("board_name", "Default string"),
            ("bios_version", "1.0"),
        ]);
        assert!(build(temp.path(), Path::new("/nonexistent")).is_none());
        let named = dmi(&[("board_name", "B450M"), ("board_vendor", "Unknown")]);
        let d = build(named.path(), Path::new("/nonexistent")).expect("board");
        assert_eq!(d.vendor_name, None);
        let started = d
            .details
            .iter()
            .flat_map(|g| &g.items)
            .find(|i| i.label == "Started in")
            .expect("boot mode");
        assert_eq!(started.value, "Legacy BIOS");
    }
}
