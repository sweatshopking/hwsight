//! The device model shared by every bus (PCI now; USB, platform, I2C, and others later).

use std::path::PathBuf;

use serde::Serialize;

use crate::modules::DriverCandidate;

/// How a device is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Bus {
    Pci,
}

/// Plain-English device groups, in the order they're shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Graphics,
    Sound,
    Network,
    Bluetooth,
    Storage,
    UsbControllers,
    Thunderbolt,
    Input,
    Camera,
    Communication,
    Security,
    Accelerators,
    Processors,
    SystemDevices,
    Other,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Self::Graphics => "Graphics",
            Self::Sound => "Sound",
            Self::Network => "Network",
            Self::Bluetooth => "Bluetooth",
            Self::Storage => "Storage",
            Self::UsbControllers => "USB controllers",
            Self::Thunderbolt => "Thunderbolt",
            Self::Input => "Input devices",
            Self::Camera => "Cameras",
            Self::Communication => "Modems and serial ports",
            Self::Security => "Security and encryption",
            Self::Accelerators => "AI and compute accelerators",
            Self::Processors => "Processors",
            Self::SystemDevices => "System devices",
            Self::Other => "Other devices",
        }
    }
}

/// Whether a device has what it needs to work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// A driver is attached.
    Working,
    /// Nothing is attached, and that's normal for this kind of device.
    NoDriverNeeded,
    /// A driver is installed but not loaded.
    DriverAvailableNotLoaded,
    /// A suitable driver is loaded, but it didn't take the device (often a failed start,
    /// missing firmware, or the device is turned off).
    DriverNotAttached,
    /// Every installed driver for this device is turned off by a setting.
    DriverBlocked,
    /// No installed driver supports this device.
    NoDriver,
    /// Deliberately set aside for a virtual machine, so the host doesn't use it.
    ReservedForVirtualMachine,
}

impl Status {
    /// Whether the user should be told something is wrong.
    pub fn is_problem(self) -> bool {
        !matches!(
            self,
            Self::Working | Self::NoDriverNeeded | Self::ReservedForVirtualMachine
        )
    }

    pub fn summary(self) -> &'static str {
        match self {
            Self::Working => "Working",
            Self::NoDriverNeeded => "Working (no driver needed)",
            Self::DriverAvailableNotLoaded => "Driver installed but not loaded",
            Self::DriverNotAttached => "Driver loaded but not using this device",
            Self::DriverBlocked => "Driver turned off by a setting",
            Self::NoDriver => "No driver",
            Self::ReservedForVirtualMachine => "Reserved for a virtual machine",
        }
    }
}

/// One piece of hardware.
#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub bus: Bus,
    /// Bus address, e.g. `0000:01:00.0` for PCI.
    pub address: String,
    pub sysfs_path: PathBuf,
    /// The best short name we have. Never empty.
    pub display_name: String,
    pub vendor_name: Option<String>,
    pub product_name: Option<String>,
    /// The exact model from the card or laptop maker, when known.
    pub model_name: Option<String>,
    /// What kind of device the hardware says it is, e.g. "VGA compatible controller".
    pub class_name: Option<String>,
    pub category: Category,
    /// Vendor and product ID, e.g. `10de:2c05`. Searchable online even when unnamed.
    pub hardware_id: String,
    /// True when the ID database has no name for this exact product.
    pub unrecognized: bool,
    pub modalias: Option<String>,
    /// The driver currently attached.
    pub driver: Option<String>,
    /// Installed drivers that support this device.
    pub driver_candidates: Vec<DriverCandidate>,
    pub status: Status,
}

/// Placeholder drivers that hold a device for passing through to a virtual machine.
const VM_PASSTHROUGH_DRIVERS: &[&str] = &["vfio-pci", "pci-stub", "pciback", "xen-pciback"];

/// Decide a device's status from what's attached and what's installed.
pub(crate) fn diagnose(
    driver: Option<&str>,
    candidates: &[DriverCandidate],
    needs_driver: bool,
) -> Status {
    if let Some(driver) = driver {
        if VM_PASSTHROUGH_DRIVERS.contains(&driver) {
            Status::ReservedForVirtualMachine
        } else {
            Status::Working
        }
    } else if !needs_driver {
        Status::NoDriverNeeded
    } else if candidates.iter().any(|c| c.loaded) {
        Status::DriverNotAttached
    } else if candidates.iter().any(|c| c.blocked_by.is_none()) {
        Status::DriverAvailableNotLoaded
    } else if !candidates.is_empty() {
        Status::DriverBlocked
    } else {
        Status::NoDriver
    }
}

/// Shorten a vendor name for use inside a sentence:
/// `"Advanced Micro Devices, Inc. [AMD/ATI]"` becomes `"AMD/ATI"`,
/// `"Realtek Semiconductor Co., Ltd."` becomes `"Realtek Semiconductor"`.
pub fn short_vendor(name: &str) -> String {
    if let Some(inner) = bracketed(name) {
        return inner.to_owned();
    }
    const SUFFIXES: &[&str] = &[
        " Inc.",
        " Inc",
        " Co., Ltd.",
        " Co., Ltd",
        " Co.",
        " Ltd.",
        " Ltd",
        " Corporation",
        " Corp.",
        " Corp",
        " Technologies",
        " Technology",
        " GmbH",
        " AG",
        " Limited",
        " LLC",
    ];
    let mut short = name.trim();
    // Strip repeatedly: "Qualcomm Technologies, Inc" needs two passes.
    while let Some(rest) = SUFFIXES.iter().find_map(|s| short.strip_suffix(s)) {
        let rest = rest.trim_end_matches(',').trim();
        if rest.is_empty() {
            break; // Never strip a name down to nothing.
        }
        short = rest;
    }
    short.to_owned()
}

/// The marketing name in `"GB203 [GeForce RTX 5070 Ti]"`, if there is one.
pub(crate) fn bracketed(name: &str) -> Option<&str> {
    let start = name.rfind('[')?;
    let end = name[start..].find(']')? + start;
    let inner = name[start + 1..end].trim();
    // Brackets also hold labels like "Bus [C:A]". A product name has a real word in it.
    let has_word = inner
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w.chars().filter(|c| c.is_alphabetic()).count() >= 3);
    has_word.then_some(inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(loaded: bool) -> DriverCandidate {
        DriverCandidate {
            module: "m".into(),
            builtin: false,
            loaded,
            blocked_by: None,
        }
    }

    fn blocked() -> DriverCandidate {
        DriverCandidate {
            blocked_by: Some("/etc/modprobe.d/x.conf".into()),
            ..candidate(false)
        }
    }

    #[test]
    fn diagnosis_order() {
        assert_eq!(diagnose(Some("nvidia"), &[], true), Status::Working);
        assert_eq!(
            diagnose(None, &[candidate(false)], false),
            Status::NoDriverNeeded
        );
        assert_eq!(
            diagnose(None, &[candidate(false), candidate(true)], true),
            Status::DriverNotAttached
        );
        assert_eq!(
            diagnose(None, &[candidate(false)], true),
            Status::DriverAvailableNotLoaded
        );
        assert_eq!(diagnose(None, &[], true), Status::NoDriver);
        assert_eq!(diagnose(None, &[blocked()], true), Status::DriverBlocked);
        assert_eq!(
            diagnose(None, &[blocked(), candidate(false)], true),
            Status::DriverAvailableNotLoaded
        );
        assert_eq!(
            diagnose(Some("vfio-pci"), &[], true),
            Status::ReservedForVirtualMachine
        );
        assert!(!Status::ReservedForVirtualMachine.is_problem());
        assert!(Status::DriverBlocked.is_problem());
    }

    #[test]
    fn vendor_shortening() {
        assert_eq!(
            short_vendor("Advanced Micro Devices, Inc. [AMD/ATI]"),
            "AMD/ATI"
        );
        assert_eq!(
            short_vendor("Realtek Semiconductor Co., Ltd."),
            "Realtek Semiconductor"
        );
        assert_eq!(short_vendor("NVIDIA Corporation"), "NVIDIA");
        assert_eq!(short_vendor("Intel Corporation"), "Intel");
        assert_eq!(short_vendor("Qualcomm Technologies, Inc"), "Qualcomm");
        assert_eq!(short_vendor("Inc."), "Inc.");
    }

    #[test]
    fn marketing_names() {
        assert_eq!(
            bracketed("GB203 [GeForce RTX 5070 Ti]"),
            Some("GeForce RTX 5070 Ti")
        );
        assert_eq!(bracketed("Raphael"), None);
        assert_eq!(bracketed("Foo []"), None);
        assert_eq!(bracketed("Internal GPP Bridge to Bus [C:A]"), None);
        assert_eq!(
            bracketed("Granite Ridge [Radeon 610M]"),
            Some("Radeon 610M")
        );
        assert_eq!(
            bracketed("Advanced Micro Devices, Inc. [AMD/ATI]"),
            Some("AMD/ATI")
        );
    }
}
