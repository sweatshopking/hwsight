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
    /// A setting stopping this driver loading automatically: a config file path, or
    /// "kernel command line". `None` if nothing blocks it.
    pub blocked_by: Option<String>,
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
    pub fn candidates(&self, modalias: &str, blocklist: &Blocklist) -> Vec<DriverCandidate> {
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
                let loaded = entry.builtin || self.is_loaded(&entry.module);
                found.push(DriverCandidate {
                    module: entry.module.clone(),
                    builtin: entry.builtin,
                    loaded,
                    // Built-in drivers can't be blocked this way, and a loaded one
                    // clearly wasn't stopped.
                    blocked_by: if entry.builtin || loaded {
                        None
                    } else {
                        blocklist.blocked_by(&entry.module).map(str::to_owned)
                    },
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

/// Where modprobe reads its settings, highest priority first (see `man modprobe.d`).
/// A file name in an earlier folder hides the same name in later ones.
pub const MODPROBE_DIRS: &[&str] = &[
    "/etc/modprobe.d",
    "/run/modprobe.d",
    "/usr/local/lib/modprobe.d",
    "/usr/lib/modprobe.d",
    "/lib/modprobe.d",
];

/// Drivers that settings stop from loading automatically.
#[derive(Debug, Default, Clone)]
pub struct Blocklist {
    /// Module name to what blocks it.
    blocked: Vec<(String, String)>,
}

impl Blocklist {
    /// Read modprobe settings from `dirs` and the kernel command line `cmdline`.
    pub fn load(dirs: &[PathBuf], cmdline: Option<&str>) -> Self {
        let mut list = Self::default();
        for path in modprobe_files(dirs) {
            if let Ok(text) = fs::read_to_string(&path) {
                for module in blocked_in_config(&text) {
                    list.add(module, path.display().to_string());
                }
            }
        }
        if let Some(cmdline) = cmdline {
            for module in blocked_on_cmdline(cmdline) {
                list.add(module, "kernel command line".to_owned());
            }
        }
        list
    }

    /// The real system's settings.
    pub fn system() -> Self {
        let dirs: Vec<PathBuf> = MODPROBE_DIRS.iter().map(PathBuf::from).collect();
        let cmdline = fs::read_to_string("/proc/cmdline").ok();
        Self::load(&dirs, cmdline.as_deref())
    }

    pub fn blocked_by(&self, module: &str) -> Option<&str> {
        let module = module.replace('-', "_");
        self.blocked
            .iter()
            .find(|(m, _)| *m == module)
            .map(|(_, by)| by.as_str())
    }

    fn add(&mut self, module: String, by: String) {
        let module = module.replace('-', "_");
        if self.blocked.iter().all(|(m, _)| *m != module) {
            self.blocked.push((module, by));
        }
    }
}

/// `*.conf` files in priority order, skipping names hidden by a higher-priority folder.
/// Files are read in name order within the combined set, as modprobe does.
fn modprobe_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut by_name: Vec<(String, PathBuf)> = Vec::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".conf") || by_name.iter().any(|(n, _)| *n == name) {
                continue;
            }
            by_name.push((name, entry.path()));
        }
    }
    by_name.sort_by(|a, b| a.0.cmp(&b.0));
    by_name.into_iter().map(|(_, p)| p).collect()
}

/// Modules a modprobe.d file stops from loading: `blacklist name`, and
/// `install name /bin/false` (or `true`), which replaces loading with doing nothing.
fn blocked_in_config(text: &str) -> Vec<String> {
    let joined = text.replace("\\\n", " "); // backslash line continuations
    let mut found = Vec::new();
    for line in joined.lines() {
        let line = line.split('#').next().unwrap_or("");
        let mut words = line.split_whitespace();
        match (words.next(), words.next()) {
            (Some("blacklist"), Some(module)) => found.push(module.to_owned()),
            (Some("install"), Some(module)) => {
                let command: Vec<&str> = words.collect();
                let does_nothing = matches!(
                    command.as_slice(),
                    [cmd] if ["/bin/false", "/bin/true", "/usr/bin/false", "/usr/bin/true", "false", "true"]
                        .contains(cmd)
                );
                if does_nothing {
                    found.push(module.to_owned());
                }
            }
            _ => {}
        }
    }
    found
}

/// `module_blacklist=a,b` and `modprobe.blacklist=a,b` on the kernel command line.
fn blocked_on_cmdline(cmdline: &str) -> Vec<String> {
    cmdline
        .split_whitespace()
        .filter_map(|arg| {
            arg.strip_prefix("module_blacklist=")
                .or_else(|| arg.strip_prefix("modprobe.blacklist="))
        })
        .flat_map(|list| list.split(','))
        .filter(|m| !m.is_empty())
        .map(str::to_owned)
        .collect()
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
        let found = db.candidates(GPU, &Blocklist::default());
        assert_eq!(
            found,
            vec![
                DriverCandidate {
                    module: "nouveau".into(),
                    builtin: false,
                    loaded: false,
                    blocked_by: None
                },
                DriverCandidate {
                    module: "nvidia_drm".into(),
                    builtin: false,
                    loaded: true,
                    blocked_by: None
                },
            ]
        );
    }

    #[test]
    fn character_classes_and_single_char_wildcards() {
        let alias = "alias pci:v000010DEd00002C0?sv*sd*bc0[23]sc*i* testmod\n";
        let (_tmp, db) = db_with(alias, b"", &[]);
        assert_eq!(db.candidates(GPU, &Blocklist::default()).len(), 1);
        assert!(
            db.candidates(&GPU.replace("bc03", "bc04"), &Blocklist::default())
                .is_empty()
        );
    }

    #[test]
    fn builtin_drivers_count_as_loaded() {
        let builtin = b"xhci_pci.alias=pci:v*d*sv*sd*bc0Csc03i30\0xhci_pci.license=GPL\0";
        let (_tmp, db) = db_with("", builtin, &[]);
        let found = db.candidates(
            "pci:v00001022d000015B6sv00001043sd00008877bc0Csc03i30",
            &Blocklist::default(),
        );
        assert_eq!(
            found,
            vec![DriverCandidate {
                module: "xhci_pci".into(),
                builtin: true,
                loaded: true,
                blocked_by: None
            }]
        );
    }

    #[test]
    fn each_module_listed_once() {
        let alias = "alias pci:v000010DE* dupe\nalias pci:v000010DEd00002C05* dupe\n";
        let (_tmp, db) = db_with(alias, b"", &[]);
        assert_eq!(db.candidates(GPU, &Blocklist::default()).len(), 1);
    }

    #[test]
    fn blocklist_from_config_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let etc = tmp.path().join("etc");
        let usr = tmp.path().join("usr");
        fs::create_dir_all(&etc).expect("mkdir");
        fs::create_dir_all(&usr).expect("mkdir");
        fs::write(
            usr.join("nvidia.conf"),
            "# comment\nblacklist nouveau\nblacklist nova-core # trailing\n",
        )
        .expect("write");
        fs::write(
            usr.join("other.conf"),
            "install pcspkr /bin/false\ninstall mlx4_core /usr/bin/modprobe --ignore-install mlx4_core\n",
        )
        .expect("write");
        // Same name in a higher-priority folder hides the lower one completely.
        fs::write(usr.join("hidden.conf"), "blacklist r8169\n").expect("write");
        fs::write(etc.join("hidden.conf"), "# emptied by the user\n").expect("write");
        fs::write(usr.join("README"), "blacklist ignored_not_conf\n").expect("write");

        let list = Blocklist::load(
            &[etc, usr.clone()],
            Some("quiet module_blacklist=foo,bar-baz"),
        );
        let nvidia_conf = usr.join("nvidia.conf").display().to_string();
        assert_eq!(list.blocked_by("nouveau"), Some(nvidia_conf.as_str()));
        assert!(list.blocked_by("nova_core").is_some());
        assert!(list.blocked_by("pcspkr").is_some());
        assert!(
            list.blocked_by("mlx4_core").is_none(),
            "install that still loads isn't a block"
        );
        assert!(
            list.blocked_by("r8169").is_none(),
            "hidden by /etc file of the same name"
        );
        assert!(list.blocked_by("ignored_not_conf").is_none());
        assert_eq!(list.blocked_by("bar_baz"), Some("kernel command line"));
        assert_eq!(list.blocked_by("foo"), Some("kernel command line"));
    }

    #[test]
    fn blocked_candidates_are_marked_unless_loaded() {
        let alias = "alias pci:v000010DE* nouveau\nalias pci:v000010DE* nvidia\n";
        let (_tmp, db) = db_with(alias, b"", &["nvidia"]);
        let list = Blocklist::load(&[], Some("modprobe.blacklist=nouveau,nvidia"));
        let found = db.candidates(GPU, &list);
        assert_eq!(found[0].blocked_by.as_deref(), Some("kernel command line"));
        assert_eq!(found[1].blocked_by, None, "loaded anyway, so not blocked");
    }

    #[test]
    fn missing_files_mean_no_database() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(AliasDb::load(tmp.path(), tmp.path(), "pci:").is_none());
    }
}
