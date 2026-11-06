//! The device model shared by every bus (PCI now; USB, platform, I2C, and others later).

use std::path::PathBuf;

use serde::Serialize;

use crate::kernel_log::LogIssue;
use crate::modules::DriverCandidate;

/// How a device is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Bus {
    Pci,
    Usb,
    /// Part of the computer itself, not plugged in: memory, the motherboard, and so on.
    System,
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
    Printers,
    Communication,
    Security,
    Accelerators,
    Processors,
    Memory,
    Drives,
    Motherboard,
    SystemDevices,
    Other,
}

impl Category {
    pub fn label(self) -> String {
        match self {
            Self::Graphics => crate::tr!("Graphics"),
            Self::Sound => crate::tr!("Sound"),
            Self::Network => crate::tr!("Network"),
            Self::Bluetooth => crate::tr!("Bluetooth"),
            Self::Storage => crate::tr!("Storage"),
            Self::UsbControllers => crate::tr!("USB controllers and hubs"),
            Self::Thunderbolt => crate::tr!("Thunderbolt"),
            Self::Input => crate::tr!("Input devices"),
            Self::Camera => crate::tr!("Cameras"),
            Self::Printers => crate::tr!("Printers and scanners"),
            Self::Communication => crate::tr!("Modems and serial ports"),
            Self::Security => crate::tr!("Security and encryption"),
            Self::Accelerators => crate::tr!("AI and compute accelerators"),
            Self::Processors => crate::tr!("Processors"),
            Self::Memory => crate::tr!("Memory"),
            Self::Drives => crate::tr!("Disk drives"),
            Self::Motherboard => crate::tr!("Motherboard and BIOS"),
            Self::SystemDevices => crate::tr!("System devices"),
            Self::Other => crate::tr!("Other devices"),
        }
    }

    /// One device of this kind, for sentences like "Unknown Logitech USB input device".
    pub fn singular(self) -> String {
        match self {
            Self::Graphics => crate::tr!("graphics device"),
            Self::Sound => crate::tr!("sound device"),
            Self::Network => crate::tr!("network device"),
            Self::Bluetooth => crate::tr!("Bluetooth device"),
            Self::Storage => crate::tr!("storage device"),
            Self::UsbControllers => crate::tr!("hub"),
            Self::Thunderbolt => crate::tr!("Thunderbolt device"),
            Self::Input => crate::tr!("input device"),
            Self::Camera => crate::tr!("camera"),
            Self::Printers => crate::tr!("printer or scanner"),
            Self::Communication => crate::tr!("modem or serial device"),
            Self::Security => crate::tr!("security device"),
            Self::Accelerators => crate::tr!("accelerator"),
            Self::Processors => crate::tr!("processor"),
            Self::Memory => crate::tr!("memory"),
            Self::Drives => crate::tr!("drive"),
            Self::Motherboard => crate::tr!("motherboard"),
            Self::SystemDevices => crate::tr!("system device"),
            Self::Other => crate::tr!("device"),
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
    /// A driver is attached, but it didn't finish setting the device up (for example a
    /// network card with no network connection), often because firmware failed to load.
    DriverNotReady,
    /// Every installed driver for this device is turned off by a setting.
    DriverBlocked,
    /// No installed driver supports this device.
    NoDriver,
    /// Deliberately set aside for a virtual machine, so the host doesn't use it.
    ReservedForVirtualMachine,
    /// A drive reports that it is failing (or that its spare storage has run out).
    DriveFailing,
}

impl Status {
    /// Whether the user should be told something is wrong.
    pub fn is_problem(self) -> bool {
        !matches!(
            self,
            Self::Working | Self::NoDriverNeeded | Self::ReservedForVirtualMachine
        )
    }

    pub fn summary(self) -> String {
        match self {
            Self::Working => crate::tr!("Working"),
            Self::NoDriverNeeded => crate::tr!("Working (no driver needed)"),
            Self::DriverAvailableNotLoaded => crate::tr!("Driver installed but not loaded"),
            Self::DriverNotAttached => crate::tr!("Driver loaded but not using this device"),
            Self::DriverNotReady => crate::tr!("Driver loaded, but the device isn't ready"),
            Self::DriverBlocked => crate::tr!("Driver turned off by a setting"),
            Self::NoDriver => crate::tr!("No driver"),
            Self::ReservedForVirtualMachine => crate::tr!("Reserved for a virtual machine"),
            Self::DriveFailing => crate::tr!("Drive reports a health problem"),
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
    /// What the kernel log says went wrong with this device this boot. Can include
    /// harmless noise for working devices (drivers often try several firmware versions),
    /// so only treat these as the explanation when `status` is a problem.
    pub log_issues: Vec<LogIssue>,
    /// For `DriverNotReady`: what the driver should have created, in plain English.
    pub missing_interface: Option<String>,
    /// A plain-English explanation of the status when there's more to say, e.g. why no
    /// driver is needed, or that USB security settings blocked the device.
    pub note: Option<String>,
    /// What this kind of device does, in plain English (from descriptions.toml).
    pub description: Option<String>,
    /// Facts for the "More details" and "Technical" levels, in groups. Empty when
    /// nothing could be read.
    pub details: Vec<crate::details::DetailGroup>,
    /// Software recommendations for this device.
    pub software: Vec<crate::software::Tip>,
    /// Class codes used to match descriptions and software recommendations.
    #[serde(skip)]
    pub(crate) class_codes: Vec<u32>,
    /// Steps to fix a problem, most specific first (from fixes.toml). Empty when the
    /// device works.
    pub fixes: Vec<crate::fixes::Fix>,
}

/// What a built-in device (memory, the motherboard) is made from.
pub(crate) struct BuiltIn {
    pub address: &'static str,
    pub sysfs_path: PathBuf,
    pub display_name: String,
    pub vendor_name: Option<String>,
    pub category: Category,
    pub hardware_id: String,
    pub description: String,
    pub details: Vec<crate::details::DetailGroup>,
}

impl Device {
    /// A part of the computer that isn't on a bus (memory, the motherboard), working
    /// unless a reader says otherwise. Facts about it go in `details`.
    pub(crate) fn built_in(spec: BuiltIn) -> Self {
        Self {
            bus: Bus::System,
            address: spec.address.to_owned(),
            sysfs_path: spec.sysfs_path,
            display_name: spec.display_name,
            vendor_name: spec.vendor_name,
            product_name: None,
            model_name: None,
            class_name: None,
            category: spec.category,
            hardware_id: spec.hardware_id,
            unrecognized: false,
            modalias: None,
            driver: None,
            driver_candidates: Vec::new(),
            status: Status::Working,
            log_issues: Vec::new(),
            missing_interface: None,
            note: None,
            description: Some(spec.description),
            details: spec.details,
            software: Vec::new(),
            class_codes: Vec::new(),
            fixes: Vec::new(),
        }
    }
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

    #[test]
    fn labels_and_summaries_are_english_by_default() {
        crate::i18n::in_english(|| {
            assert_eq!(Status::Working.summary(), "Working");
            assert_eq!(Category::Graphics.label(), "Graphics");
        });
    }
}
