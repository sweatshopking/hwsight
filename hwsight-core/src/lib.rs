//! hwsight core: finds every device in the computer and works out whether it's working.
//!
//! No UI code lives here, so the command line and graphical apps share one engine.

pub mod data;
pub mod device;
pub mod modules;
mod pci;
pub mod pci_ids;
pub mod rules;

use std::path::PathBuf;

use serde::Serialize;

pub use data::DataDirs;
pub use device::{Bus, Category, Device, Status, short_vendor};
pub use modules::{Blocklist, DriverCandidate};
pub use pci_ids::{IdsSource, PciNames};
pub use rules::{DataSource, Rules, RulesProblem};

/// Everything a scan found, plus anything that limited it.
#[derive(Debug, Serialize)]
pub struct ScanReport {
    pub devices: Vec<Device>,
    pub notes: Vec<ScanNote>,
    pub data: DataInfo,
}

/// Something the user (or a bug report) should know about the scan itself.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScanNote {
    /// The kernel's driver list couldn't be read, so missing drivers can't be detected.
    NoDriverList,
    /// The PCI device folder couldn't be read.
    PciUnreadable { error: String },
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

/// Which data versions the scan used, so users and bug reports can see how fresh it is.
#[derive(Debug, Serialize)]
pub struct DataInfo {
    pub rules_version: String,
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
    /// The running kernel's module folder, e.g. `/lib/modules/6.18.0`.
    pub kernel_modules: Option<PathBuf>,
    /// Where loaded modules appear, normally `/sys/module`.
    pub loaded_modules: PathBuf,
    /// modprobe settings folders, highest priority first.
    pub modprobe_dirs: Vec<PathBuf>,
    /// The kernel command line file, normally `/proc/cmdline`.
    pub kernel_cmdline: PathBuf,
}

impl Scanner {
    pub fn system() -> Self {
        Self {
            data_dirs: DataDirs::standard(),
            pci_devices: PathBuf::from("/sys/bus/pci/devices"),
            kernel_modules: modules::running_kernel_modules_dir(),
            loaded_modules: PathBuf::from("/sys/module"),
            modprobe_dirs: modules::MODPROBE_DIRS.iter().map(PathBuf::from).collect(),
            kernel_cmdline: PathBuf::from("/proc/cmdline"),
        }
    }

    /// Find every device. Never fails: problems are reported in `notes` instead, so
    /// the user always sees as much as we could find.
    pub fn scan(&self) -> ScanReport {
        let mut notes = Vec::new();

        let (rules, problems) = Rules::load(&self.data_dirs);
        notes.extend(problems.into_iter().map(|p| match p {
            RulesProblem::NewerSchema { path, schema } => {
                ScanNote::DataNeedsNewerApp { path, schema }
            }
            RulesProblem::Invalid { path, error } => ScanNote::DataInvalid { path, error },
        }));

        let names = PciNames::load(&self.data_dirs);
        let aliases = self
            .kernel_modules
            .as_deref()
            .and_then(|dir| modules::AliasDb::load(dir, &self.loaded_modules, "pci:"));
        if aliases.is_none() {
            notes.push(ScanNote::NoDriverList);
        }

        let cmdline = std::fs::read_to_string(&self.kernel_cmdline).ok();
        let blocklist = modules::Blocklist::load(&self.modprobe_dirs, cmdline.as_deref());

        let devices = match pci::scan(
            &self.pci_devices,
            &names,
            aliases.as_ref(),
            &blocklist,
            &rules,
        ) {
            Ok(devices) => devices,
            Err(e) => {
                notes.push(ScanNote::PciUnreadable {
                    error: e.to_string(),
                });
                Vec::new()
            }
        };

        let data = DataInfo {
            rules_version: rules.version.clone(),
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
            notes,
            data,
        }
    }
}

/// Scan the real machine.
pub fn scan() -> ScanReport {
    Scanner::system().scan()
}
