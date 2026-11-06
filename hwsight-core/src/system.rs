//! The parts of the computer that aren't plugged in: the processor, memory, disk
//! drives, monitors, and the motherboard.

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
    /// `/sys/devices/system/cpu`: the processor's cores, caches, and speed settings.
    pub cpu_dir: PathBuf,
    /// `/proc/cpuinfo`: the processor's name and features.
    pub cpuinfo: PathBuf,
    /// `/sys/class/hwmon`: the kernel's temperature sensors.
    pub hwmon_dir: PathBuf,
    /// `/sys/block`: the disk drives.
    pub block_dir: PathBuf,
    /// Ask the UDisks2 service for drive health (off in tests).
    pub udisks: bool,
    /// `/sys/class/drm`: the connected screens.
    pub drm_dir: PathBuf,
    /// The system's list of monitor maker codes (hwdata).
    pub pnp_ids: PathBuf,
    /// Ask the graphics driver for each screen's modes and current mode (off in tests).
    pub drm_live: bool,
    /// Ask the fwupd service about firmware versions and updates (off in tests).
    pub fwupd: bool,
    /// `/proc/net/wireless`: Wi-Fi signal levels.
    pub proc_net_wireless: PathBuf,
    /// Ask NetworkManager about Wi-Fi bands and speeds (off in tests).
    pub network_manager: bool,
    /// `/sys/class/tpm`: the security chip.
    pub tpm_dir: PathBuf,
    /// `/proc/driver/nvidia`: the NVIDIA driver's own information.
    pub nvidia_proc: PathBuf,
    /// `/sys/class/sound`: sound cards, linked to the devices they belong to.
    pub sound_dir: PathBuf,
    /// `/proc/asound`: each sound card's chips (codecs).
    pub asound_dir: PathBuf,
    /// `/etc/apt`: Debian's software sources.
    pub apt_dir: PathBuf,
}

impl SystemPaths {
    pub fn system() -> Self {
        Self {
            dmi_dir: PathBuf::from("/sys/class/dmi/id"),
            udev_dmi_data: PathBuf::from("/run/udev/data/+dmi:id"),
            spd_dir: PathBuf::from("/sys/bus/i2c/drivers/spd5118"),
            efi_dir: PathBuf::from("/sys/firmware/efi"),
            cpu_dir: PathBuf::from("/sys/devices/system/cpu"),
            cpuinfo: PathBuf::from("/proc/cpuinfo"),
            hwmon_dir: PathBuf::from("/sys/class/hwmon"),
            block_dir: PathBuf::from("/sys/block"),
            udisks: true,
            drm_dir: PathBuf::from("/sys/class/drm"),
            pnp_ids: PathBuf::from("/usr/share/hwdata/pnp.ids"),
            drm_live: true,
            fwupd: true,
            proc_net_wireless: PathBuf::from("/proc/net/wireless"),
            network_manager: true,
            tpm_dir: PathBuf::from("/sys/class/tpm"),
            nvidia_proc: PathBuf::from("/proc/driver/nvidia"),
            sound_dir: PathBuf::from("/sys/class/sound"),
            asound_dir: PathBuf::from("/proc/asound"),
            apt_dir: PathBuf::from("/etc/apt"),
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
            cpu_dir: nowhere.join("cpu"),
            cpuinfo: nowhere.join("cpuinfo"),
            hwmon_dir: nowhere.join("hwmon"),
            block_dir: nowhere.join("block"),
            udisks: false,
            drm_dir: nowhere.join("drm"),
            pnp_ids: nowhere.join("pnp.ids"),
            drm_live: false,
            fwupd: false,
            proc_net_wireless: nowhere.join("wireless"),
            network_manager: false,
            tpm_dir: nowhere.join("tpm"),
            nvidia_proc: nowhere.join("nvidia"),
            sound_dir: nowhere.join("sound"),
            asound_dir: nowhere.join("asound"),
            apt_dir: nowhere.join("apt"),
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
    devices.extend(crate::cpu::device(
        &paths.cpu_dir,
        &paths.cpuinfo,
        &paths.hwmon_dir,
    ));
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
    let health = if paths.udisks {
        crate::udisks::read_drives()
    } else {
        Vec::new()
    };
    devices.extend(crate::drives::devices(
        &paths.block_dir,
        &health,
        descriptions,
    ));
    let live = if paths.drm_live {
        crate::displays::read_live()
    } else {
        Default::default()
    };
    devices.extend(crate::displays::devices(
        &paths.drm_dir,
        &paths.pnp_ids,
        &live,
        descriptions,
    ));
    devices.extend(crate::board::device(
        &paths.dmi_dir,
        &paths.efi_dir,
        descriptions,
    ));
    devices
}
