//! Which package provides a missing firmware file, from the updatable `firmware.toml`.

use std::collections::HashMap;

use serde::Deserialize;

use crate::data::{self, DataDirs, DataFile, DataProblem, DataSource};
use crate::distro::Family;

#[derive(Debug, Clone)]
pub struct FirmwarePackages {
    pub version: String,
    pub source: DataSource,
    table: Table,
}

#[derive(Debug, Clone, Default)]
struct Table {
    entries: Vec<Entry>,
    notes: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct Entry {
    path: String,
    packages: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct RawFile {
    version: String,
    #[serde(default)]
    notes: HashMap<String, String>,
    #[serde(default)]
    firmware: Vec<RawEntry>,
}

#[derive(Debug, Deserialize)]
struct RawEntry {
    path: String,
    /// Family key to package name. Unknown families are kept but never asked for.
    #[serde(flatten)]
    packages: HashMap<String, String>,
}

impl DataFile for Table {
    const FILE_NAME: &'static str = "firmware.toml";
    const SCHEMA: u32 = 1;
    const EMBEDDED: &'static str = include_str!("../data/firmware.toml");

    fn parse_body(text: &str) -> Result<(String, Self), String> {
        let raw: RawFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut entries = Vec::with_capacity(raw.firmware.len());
        for e in raw.firmware {
            if e.path.contains("..") || e.path.starts_with('/') {
                return Err(format!("bad firmware path `{}`", e.path));
            }
            if let Some((family, pkg)) = e.packages.iter().find(|(_, p)| !is_package_name(p)) {
                return Err(format!("bad package name `{pkg}` for {family}"));
            }
            entries.push(Entry {
                path: e.path,
                packages: e.packages,
            });
        }
        // Longest path first, so the most specific entry is found first.
        entries.sort_by_key(|e| std::cmp::Reverse(e.path.len()));
        Ok((
            raw.version,
            Self {
                entries,
                notes: raw.notes,
            },
        ))
    }

    fn empty() -> Self {
        Self::default()
    }
}

/// Package names end up in commands shown to the user, so only allow the characters
/// real package names use. This blocks a tampered file from suggesting `x; rm -rf ~`.
fn is_package_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
}

impl FirmwarePackages {
    pub fn embedded() -> Self {
        Self::from_loaded(data::embedded::<Table>())
    }

    pub fn load(dirs: &DataDirs) -> (Self, Vec<DataProblem>) {
        let (loaded, problems) = data::load_newest::<Table>(dirs);
        (Self::from_loaded(loaded), problems)
    }

    fn from_loaded(loaded: data::Loaded<Table>) -> Self {
        Self {
            version: loaded.version,
            source: loaded.source,
            table: loaded.data,
        }
    }

    /// The package that provides firmware `file` on this distro family.
    pub fn package_for(&self, file: &str, family: Family) -> Option<&str> {
        let key = family.key()?;
        self.table
            .entries
            .iter()
            .filter(|e| file.starts_with(&e.path))
            .find_map(|e| e.packages.get(key))
            .map(String::as_str)
    }

    /// Extra advice for this family, e.g. Debian's non-free-firmware section.
    pub fn note(&self, family: Family) -> Option<&str> {
        self.table.notes.get(family.key()?).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn embedded_file_is_valid() {
        assert!(data::parse_data::<Table>(Table::EMBEDDED).is_ok());
        assert!(!FirmwarePackages::embedded().version.is_empty());
    }

    #[test]
    fn most_specific_entry_wins_per_distro() {
        let fw = FirmwarePackages::embedded();
        assert_eq!(
            fw.package_for("rtl_nic/rtl8125b-2.fw", Family::Arch),
            Some("linux-firmware-realtek")
        );
        assert_eq!(
            fw.package_for("rtl_nic/rtl8125b-2.fw", Family::Debian),
            Some("firmware-realtek")
        );
        // Ubuntu has one package for everything, found through the fallback entry.
        assert_eq!(
            fw.package_for("rtl_nic/rtl8125b-2.fw", Family::Ubuntu),
            Some("linux-firmware")
        );
        assert_eq!(
            fw.package_for("iwlwifi-ty-a0-gf-a0-89.ucode", Family::Fedora),
            Some("iwlwifi-mvm-firmware")
        );
        // "intel/sof..." is more specific than "intel/".
        assert_eq!(
            fw.package_for("intel/sof-ipc4/mtl/sof-mtl.ri", Family::Arch),
            Some("sof-firmware")
        );
        assert_eq!(
            fw.package_for("intel/ibt-0040-0041.sfi", Family::Arch),
            Some("linux-firmware-intel")
        );
        // A distro missing from a specific entry falls back to the general package.
        assert_eq!(
            fw.package_for("amd/amd_sev_fam19h_model1xh.sbin", Family::Fedora),
            Some("linux-firmware")
        );
        assert_eq!(
            fw.package_for("anything/new.bin", Family::OpenSuse),
            Some("kernel-firmware-all")
        );
        assert_eq!(fw.package_for("rtl_nic/x.fw", Family::Unknown), None);
        assert!(fw.note(Family::Debian).is_some());
        assert!(fw.note(Family::Arch).is_none());
    }

    #[test]
    fn unsafe_package_names_reject_the_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::write(
            tmp.path().join("firmware.toml"),
            "schema = 1\nversion = \"2099.01.01\"\n[[firmware]]\npath = \"\"\narch = \"x; rm -rf ~\"\n",
        )
        .expect("write");
        let (fw, problems) = FirmwarePackages::load(&DataDirs::only(tmp.path()));
        assert_eq!(fw.source, DataSource::Embedded);
        assert!(matches!(problems.as_slice(), [DataProblem::Invalid { .. }]));
    }

    #[test]
    fn unknown_families_are_allowed() {
        let text = "schema = 1\nversion = \"2099.01.01\"\n[[firmware]]\npath = \"\"\nvoid = \"linux-firmware\"\n";
        assert!(data::parse_data::<Table>(text).is_ok());
    }

    #[test]
    fn package_name_rules() {
        assert!(is_package_name("linux-firmware-realtek"));
        assert!(is_package_name("libstdc++6"));
        assert!(!is_package_name("-rf"));
        assert!(!is_package_name("a b"));
        assert!(!is_package_name(""));
    }
}
