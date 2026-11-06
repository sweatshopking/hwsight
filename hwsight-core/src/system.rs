//! The parts of the computer that aren't plugged in: memory and the motherboard.

use std::fs;
use std::path::PathBuf;

use crate::descriptions::Descriptions;
use crate::device::Device;

/// Where the firmware's information lives. Tests point these at fake folders.
#[derive(Debug, Clone)]
pub struct SystemPaths {
    /// `/sys/class/dmi/id`: motherboard and BIOS.
    pub dmi_dir: PathBuf,
    /// udev's saved copy of the firmware's memory table, readable by anyone.
    pub udev_dmi_data: PathBuf,
    /// Where the kernel exposes each memory stick's SPD chip.
    pub spd_dir: PathBuf,
    /// Exists when the computer started in UEFI mode.
    pub efi_dir: PathBuf,
}

impl SystemPaths {
    pub fn system() -> Self {
        Self {
            dmi_dir: PathBuf::from("/sys/class/dmi/id"),
            udev_dmi_data: PathBuf::from("/run/udev/data/+dmi:id"),
            spd_dir: PathBuf::from("/sys/bus/i2c/drivers/spd5118"),
            efi_dir: PathBuf::from("/sys/firmware/efi"),
        }
    }

    /// Paths that don't exist: no built-in devices are found.
    pub fn none() -> Self {
        let nowhere = PathBuf::from("/nonexistent/hwsight");
        Self {
            dmi_dir: nowhere.join("dmi"),
            udev_dmi_data: nowhere.join("udev-dmi"),
            spd_dir: nowhere.join("spd"),
            efi_dir: nowhere.join("efi"),
        }
    }
}

/// Text the firmware fills in when it doesn't know.
const PLACEHOLDERS: [&str; 9] = [
    "unknown",
    "not specified",
    "not provided",
    "none",
    "to be filled by o.e.m.",
    "default string",
    "system product name",
    "system version",
    "n/a",
];

/// The text, or `None` when it's empty or a placeholder.
pub(crate) fn clean(value: Option<&String>) -> Option<String> {
    let text = value?.trim();
    (!text.is_empty() && !PLACEHOLDERS.contains(&text.to_lowercase().as_str()))
        .then(|| text.to_owned())
}

/// The memory and the motherboard, when the firmware tells us about them.
pub(crate) fn scan(paths: &SystemPaths, descriptions: &Descriptions) -> Vec<Device> {
    let mut devices = Vec::new();
    if let Ok(data) = fs::read_to_string(&paths.udev_dmi_data) {
        let spds = crate::spd::read_all(&paths.spd_dir);
        devices.extend(crate::memory::device(
            &data,
            &paths.udev_dmi_data,
            &paths.spd_dir,
            &spds,
            descriptions,
        ));
    }
    devices.extend(crate::board::device(
        &paths.dmi_dir,
        &paths.efi_dir,
        descriptions,
    ));
    devices
}
