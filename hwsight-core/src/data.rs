//! Where hwsight finds its updatable data.
//!
//! Data comes in layers. For each file we look in:
//!
//! 1. The download folder, `$XDG_DATA_HOME/hwsight/data` (normally
//!    `~/.local/share/hwsight/data`), where hwsight's updater saves fresh copies.
//! 2. System data folders, `$XDG_DATA_DIRS/hwsight/data` (normally under `/usr/share`),
//!    where a distro package may install data.
//! 3. For device name lists only, the distro's own copies (from the `hwdata` package).
//!
//! The data built into the program is the final fallback, so hwsight always works offline.
//! Callers compare the versions of what they find and use the newest.

use std::env;
use std::path::{Path, PathBuf};

/// Where distros install the device name lists, most common first.
const DISTRO_ID_DIRS: &[&str] = &[
    "/usr/share/hwdata", // Arch, Fedora, openSUSE, newer Debian/Ubuntu
    "/usr/share/misc",   // Debian, Ubuntu
    "/usr/share",
    "/usr/local/share/hwdata",
];

#[derive(Debug, Clone)]
pub struct DataDirs {
    /// Where downloaded updates go. `None` if we can't work out a home folder.
    pub download: Option<PathBuf>,
    /// Package-installed hwsight data folders.
    pub system: Vec<PathBuf>,
    /// Distro folders holding `pci.ids` and `usb.ids`.
    pub distro_ids: Vec<PathBuf>,
}

impl DataDirs {
    /// The standard locations, following the XDG base directory spec.
    pub fn standard() -> Self {
        let download = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|base| base.join("hwsight/data"));

        let data_dirs = env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
        let system = data_dirs
            .split(':')
            .map(Path::new)
            .filter(|p| p.is_absolute())
            .map(|p| p.join("hwsight/data"))
            .collect();

        Self {
            download,
            system,
            distro_ids: DISTRO_ID_DIRS.iter().map(PathBuf::from).collect(),
        }
    }

    /// Only the given folder (used by tests and for pointing at a custom data set).
    pub fn only(dir: &Path) -> Self {
        Self {
            download: Some(dir.to_path_buf()),
            system: Vec::new(),
            distro_ids: Vec::new(),
        }
    }

    /// Every existing copy of hwsight data file `name`, download folder first.
    pub fn hwsight_files(&self, name: &str) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = self
            .download
            .iter()
            .chain(&self.system)
            .map(|dir| dir.join(name))
            .filter(|p| p.is_file())
            .collect();
        files.dedup();
        files
    }

    /// Every existing copy of a device name list such as `pci.ids`, including distro copies.
    pub fn id_files(&self, name: &str) -> Vec<PathBuf> {
        let mut files = self.hwsight_files(name);
        for path in self.distro_ids.iter().map(|dir| dir.join(name)) {
            // Skip symlinks to a file we already have (e.g. /usr/share/misc -> hwdata).
            let same = files.iter().any(|f| same_file(f, &path));
            if path.is_file() && !same {
                files.push(path);
            }
        }
        files
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
