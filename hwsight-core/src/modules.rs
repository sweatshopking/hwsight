//! Which kernel driver modules could handle a device.
//!
//! Every device publishes a "modalias" string describing itself, such as
//! `pci:v000010DEd00002C05sv...`. Each driver lists glob patterns for the devices it
//! supports. The kernel's module tools keep those patterns in two files:
//!
//! - `modules.alias`: drivers built as loadable modules.
//! - `modules.builtin.modinfo`: drivers compiled into the kernel itself (always present).
//!
//! Matching a device's modalias against these patterns is exactly how `modprobe`
//! decides which driver to load, so it tells us whether a driver exists for a device.

use std::fs;
use std::path::{Path, PathBuf};

use glob::{MatchOptions, Pattern};
use serde::Serialize;

/// A driver that claims to support a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DriverCandidate {
    /// Module name, as `modprobe` and `/sys/module` spell it (underscores, not dashes).
    pub module: String,
    /// Compiled into the kernel, so it can't be "not loaded".
    pub builtin: bool,
    /// Currently loaded (always true for built-in drivers).
    pub loaded: bool,
}

#[derive(Debug)]
struct AliasEntry {
    pattern: Pattern,
    module: String,
    builtin: bool,
}

/// Driver alias patterns for one kernel, limited to a single bus (e.g. `pci:`).
#[derive(Debug)]
pub struct AliasDb {
    entries: Vec<AliasEntry>,
    loaded_modules_dir: PathBuf,
}

impl AliasDb {
    /// Load aliases for the bus prefix `bus` (e.g. `"pci:"`) from a kernel's module
    /// directory, such as `/lib/modules/6.18.0`. Returns `None` if neither alias file
    /// can be read, since then we can't say anything about available drivers.
    pub fn load(modules_dir: &Path, loaded_modules_dir: &Path, bus: &str) -> Option<Self> {
        let modular = fs::read_to_string(modules_dir.join("modules.alias")).ok();
        let builtin = fs::read(modules_dir.join("modules.builtin.modinfo")).ok();
        if modular.is_none() && builtin.is_none() {
            return None;
        }

        let mut entries = Vec::new();
        if let Some(text) = modular {
            entries.extend(parse_modules_alias(&text, bus));
        }
        if let Some(bytes) = builtin {
            entries.extend(parse_builtin_modinfo(&bytes, bus));
        }
        Some(Self {
            entries,
            loaded_modules_dir: loaded_modules_dir.to_path_buf(),
        })
    }

    /// Every driver whose patterns match `modalias`, each listed once, in file order.
    pub fn candidates(&self, modalias: &str) -> Vec<DriverCandidate> {
        let options = MatchOptions {
            case_sensitive: true,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        };
        let mut found: Vec<DriverCandidate> = Vec::new();
        for entry in &self.entries {
            if found.iter().any(|c| c.module == entry.module) {
                continue;
            }
            if entry.pattern.matches_with(modalias, options) {
                found.push(DriverCandidate {
                    module: entry.module.clone(),
                    builtin: entry.builtin,
                    loaded: entry.builtin || self.is_loaded(&entry.module),
                });
            }
        }
        found
    }

    fn is_loaded(&self, module: &str) -> bool {
        // A loaded module appears as /sys/module/<name>. Built-in code can create
        // entries there too, but we only ask about loadable modules here.
        self.loaded_modules_dir.join(module).is_dir()
    }
}

/// Lines look like `alias pci:v000010DEd*sv*sd*bc03sc*i* nouveau`.
fn parse_modules_alias(text: &str, bus: &str) -> Vec<AliasEntry> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            if parts.next()? != "alias" {
                return None;
            }
            let pattern = parts.next()?;
            let module = parts.next()?;
            entry(pattern, module, bus, false)
        })
        .collect()
}

/// NUL-separated `module.key=value` records; we want `module.alias=pci:...`.
fn parse_builtin_modinfo(bytes: &[u8], bus: &str) -> Vec<AliasEntry> {
    bytes
        .split(|&b| b == 0)
        .filter_map(|record| {
            let record = std::str::from_utf8(record).ok()?;
            let (module, pattern) = record.split_once(".alias=")?;
            entry(pattern, module, bus, true)
        })
        .collect()
}

fn entry(pattern: &str, module: &str, bus: &str, builtin: bool) -> Option<AliasEntry> {
    if !pattern.starts_with(bus) || module.is_empty() {
        return None;
    }
    Some(AliasEntry {
        pattern: Pattern::new(pattern).ok()?,
        module: module.replace('-', "_"),
        builtin,
    })
}

/// The module directory for the running kernel. Checks `/lib/modules` then
/// `/usr/lib/modules`, since distros differ on which one is real.
pub fn running_kernel_modules_dir() -> Option<PathBuf> {
    let release = fs::read_to_string("/proc/sys/kernel/osrelease").ok()?;
    let release = release.trim();
    ["/lib/modules", "/usr/lib/modules"]
        .iter()
        .map(|base| Path::new(base).join(release))
        .find(|dir| dir.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with(alias: &str, builtin: &[u8], loaded: &[&str]) -> (tempfile::TempDir, AliasDb) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let modules = tmp.path().join("modules");
        let sys_module = tmp.path().join("sys_module");
        fs::create_dir_all(&modules).expect("mkdir");
        fs::create_dir_all(&sys_module).expect("mkdir");
        fs::write(modules.join("modules.alias"), alias).expect("write");
        fs::write(modules.join("modules.builtin.modinfo"), builtin).expect("write");
        for m in loaded {
            fs::create_dir_all(sys_module.join(m)).expect("mkdir");
        }
        let db = AliasDb::load(&modules, &sys_module, "pci:").expect("db");
        (tmp, db)
    }

    const GPU: &str = "pci:v000010DEd00002C05sv00001043sd00008A2Fbc03sc00i00";

    #[test]
    fn matches_wildcards_and_reports_load_state() {
        let alias = "\
alias pci:v000010DEd*sv*sd*bc03sc*i* nouveau
alias pci:v000010DEd00002C05sv*sd*bc*sc*i* nvidia-drm
alias pci:v00008086d*sv*sd*bc03sc*i* i915
alias usb:v*p*d*dc*dsc*dp*ic*isc*ip*in* usbcore
";
        let (_tmp, db) = db_with(alias, b"", &["nvidia_drm"]);
        let found = db.candidates(GPU);
        assert_eq!(
            found,
            vec![
                DriverCandidate { module: "nouveau".into(), builtin: false, loaded: false },
                DriverCandidate { module: "nvidia_drm".into(), builtin: false, loaded: true },
            ]
        );
    }

    #[test]
    fn character_classes_and_single_char_wildcards() {
        let alias = "alias pci:v000010DEd00002C0?sv*sd*bc0[23]sc*i* testmod\n";
        let (_tmp, db) = db_with(alias, b"", &[]);
        assert_eq!(db.candidates(GPU).len(), 1);
        assert!(db.candidates(&GPU.replace("bc03", "bc04")).is_empty());
    }

    #[test]
    fn builtin_drivers_count_as_loaded() {
        let builtin = b"xhci_pci.alias=pci:v*d*sv*sd*bc0Csc03i30\0xhci_pci.license=GPL\0";
        let (_tmp, db) = db_with("", builtin, &[]);
        let found = db.candidates("pci:v00001022d000015B6sv00001043sd00008877bc0Csc03i30");
        assert_eq!(
            found,
            vec![DriverCandidate { module: "xhci_pci".into(), builtin: true, loaded: true }]
        );
    }

    #[test]
    fn each_module_listed_once() {
        let alias = "alias pci:v000010DE* dupe\nalias pci:v000010DEd00002C05* dupe\n";
        let (_tmp, db) = db_with(alias, b"", &[]);
        assert_eq!(db.candidates(GPU).len(), 1);
    }

    #[test]
    fn missing_files_mean_no_database() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(AliasDb::load(tmp.path(), tmp.path(), "pci:").is_none());
    }
}
