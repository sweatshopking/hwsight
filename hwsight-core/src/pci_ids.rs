//! Human-readable names for PCI vendors, devices, and classes.
//!
//! Names come from every `pci.ids` file we can find (downloaded update, distro copy),
//! newest first, then the copy built into the `pci-ids` crate. An ID missing from the
//! newest list is looked up in the older ones too, so nothing that any list knows is lost.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::data::DataDirs;
use crate::rules::is_date_version;

/// Name lookups for PCI IDs.
#[derive(Debug)]
pub struct PciNames {
    /// Newest first. The built-in database is always the implicit last layer.
    layers: Vec<IdsLayer>,
}

#[derive(Debug)]
struct IdsLayer {
    path: PathBuf,
    version: Option<String>,
    db: PciIdsDb,
}

/// A name list in use, for showing the user where names come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdsSource {
    pub path: PathBuf,
    /// From the file's `Version:` header, e.g. `2026.10.01`.
    pub version: Option<String>,
}

impl PciNames {
    /// Every `pci.ids` in the data folders, plus the built-in database.
    pub fn load(dirs: &DataDirs) -> Self {
        Self::from_files(&dirs.id_files("pci.ids"))
    }

    /// The given files (unreadable ones are skipped), plus the built-in database.
    pub fn from_files(paths: &[PathBuf]) -> Self {
        let mut layers: Vec<IdsLayer> = paths.iter().filter_map(|p| load_layer(p)).collect();
        // Newest first; files without a readable version go last.
        layers.sort_by(|a, b| b.version.cmp(&a.version));
        Self { layers }
    }

    /// Only the database built into the program.
    pub fn embedded() -> Self {
        Self { layers: Vec::new() }
    }

    /// The name lists in use, newest first (the built-in database isn't listed).
    pub fn sources(&self) -> Vec<IdsSource> {
        self.layers
            .iter()
            .map(|l| IdsSource {
                path: l.path.clone(),
                version: l.version.clone(),
            })
            .collect()
    }

    fn find<T>(&self, get: impl Fn(&PciIdsDb) -> Option<T>) -> Option<T> {
        self.layers.iter().find_map(|l| get(&l.db))
    }

    pub fn vendor(&self, vendor: u16) -> Option<String> {
        self.find(|db| db.vendors.get(&vendor).cloned())
            .or_else(|| embedded::vendor(vendor))
    }

    pub fn device(&self, vendor: u16, device: u16) -> Option<String> {
        self.find(|db| db.devices.get(&(vendor, device)).cloned())
            .or_else(|| embedded::device(vendor, device))
    }

    pub fn subsystem(
        &self,
        vendor: u16,
        device: u16,
        sub_vendor: u16,
        sub_device: u16,
    ) -> Option<String> {
        self.find(|db| {
            db.subsystems
                .get(&(vendor, device, sub_vendor, sub_device))
                .cloned()
        })
        .or_else(|| embedded::subsystem(vendor, device, sub_vendor, sub_device))
    }

    /// The most specific class name available: subclass if known, else the base class.
    pub fn class(&self, class: u8, subclass: u8) -> Option<String> {
        self.find(|db| db.subclasses.get(&(class, subclass)).cloned())
            .or_else(|| embedded::subclass(class, subclass))
            .or_else(|| self.find(|db| db.classes.get(&class).cloned()))
            .or_else(|| embedded::class(class))
    }
}

fn load_layer(path: &Path) -> Option<IdsLayer> {
    let bytes = fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let db = PciIdsDb::parse(&text);
    if db.vendors.is_empty() {
        return None; // Empty or not a pci.ids file at all.
    }
    Some(IdsLayer {
        path: path.to_path_buf(),
        version: header_version(&text),
        db,
    })
}

/// The `#\tVersion: 2026.10.01` line near the top of an ids file.
pub(crate) fn header_version(text: &str) -> Option<String> {
    text.lines()
        .take(50)
        .filter(|l| l.starts_with('#'))
        .find_map(|l| l.trim_start_matches('#').trim().strip_prefix("Version:"))
        .map(str::trim)
        .filter(|v| is_date_version(v))
        .map(str::to_owned)
}

/// Lookups in the database built into the `pci-ids` crate.
mod embedded {
    pub(super) fn vendor(vendor: u16) -> Option<String> {
        pci_ids::Vendors::iter()
            .find(|v| v.id() == vendor)
            .map(|v| v.name().to_owned())
    }

    pub(super) fn device(vendor: u16, device: u16) -> Option<String> {
        pci_ids::Device::from_vid_pid(vendor, device).map(|d| d.name().to_owned())
    }

    pub(super) fn subsystem(
        vendor: u16,
        device: u16,
        sub_vendor: u16,
        sub_device: u16,
    ) -> Option<String> {
        pci_ids::Device::from_vid_pid(vendor, device)?
            .subsystems()
            .find(|s| s.subvendor() == sub_vendor && s.subdevice() == sub_device)
            .map(|s| s.name().to_owned())
    }

    pub(super) fn subclass(class: u8, subclass: u8) -> Option<String> {
        pci_ids::Subclass::from_cid_sid(class, subclass).map(|s| s.name().to_owned())
    }

    pub(super) fn class(class: u8) -> Option<String> {
        pci_ids::Classes::iter()
            .find(|c| c.id() == class)
            .map(|c| c.name().to_owned())
    }
}

/// A parsed `pci.ids` file. Prog-if entries are skipped because we don't show them.
#[derive(Debug, Default)]
struct PciIdsDb {
    vendors: HashMap<u16, String>,
    devices: HashMap<(u16, u16), String>,
    subsystems: HashMap<(u16, u16, u16, u16), String>,
    classes: HashMap<u8, String>,
    subclasses: HashMap<(u8, u8), String>,
}

impl PciIdsDb {
    /// Parse the `pci.ids` format. Malformed lines are skipped rather than failing the
    /// whole file, so one bad entry can't cost us every name.
    fn parse(text: &str) -> Self {
        let mut db = Self::default();
        let mut vendor: Option<u16> = None;
        let mut device: Option<u16> = None;
        let mut class: Option<u8> = None;
        let mut in_classes = false;

        for line in text.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let depth = line.bytes().take_while(|&b| b == b'\t').count();
            let body = &line[depth..];

            match (in_classes, depth) {
                (_, 0) if body.starts_with("C ") => {
                    in_classes = true;
                    vendor = None;
                    device = None;
                    class = split_id(&body[2..]).and_then(|(id, name)| {
                        let id = u8::from_str_radix(id, 16).ok()?;
                        db.classes.insert(id, name.to_owned());
                        Some(id)
                    });
                }
                (_, 0) => {
                    // A non-class top-level entry. Anything after the class section that
                    // isn't a 4-digit vendor (e.g. "X" lists) ends class parsing too.
                    in_classes = false;
                    class = None;
                    device = None;
                    vendor = split_id(body).and_then(|(id, name)| {
                        let id = parse_u16(id)?;
                        db.vendors.insert(id, name.to_owned());
                        Some(id)
                    });
                }
                (false, 1) => {
                    device = vendor.zip(split_id(body)).and_then(|(v, (id, name))| {
                        let id = parse_u16(id)?;
                        db.devices.insert((v, id), name.to_owned());
                        Some(id)
                    });
                }
                (false, 2) => {
                    if let (Some(v), Some(d)) = (vendor, device) {
                        // "subvendor subdevice  name"
                        let mut parts = body.splitn(3, ' ');
                        let sv = parts.next().and_then(parse_u16);
                        let sd = parts.next().and_then(parse_u16);
                        let name = parts.next().map(str::trim);
                        if let (Some(sv), Some(sd), Some(name)) = (sv, sd, name) {
                            db.subsystems.insert((v, d, sv, sd), name.to_owned());
                        }
                    }
                }
                (true, 1) => {
                    if let (Some(c), Some((id, name))) = (class, split_id(body))
                        && let Ok(id) = u8::from_str_radix(id, 16)
                    {
                        db.subclasses.insert((c, id), name.to_owned());
                    }
                }
                _ => {}
            }
        }
        db
    }
}

/// Split `"10de  NVIDIA Corporation"` into `("10de", "NVIDIA Corporation")`.
fn split_id(body: &str) -> Option<(&str, &str)> {
    let (id, name) = body.split_once(' ')?;
    let name = name.trim();
    (!id.is_empty() && !name.is_empty()).then_some((id, name))
}

fn parse_u16(hex: &str) -> Option<u16> {
    if hex.len() != 4 {
        return None;
    }
    u16::from_str_radix(hex, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# comment
10de  NVIDIA Corporation
\t2c05  GB203 [GeForce RTX 5070 Ti]
\t\t1043 8a2f  TUF Gaming RTX 5070 Ti
1022  Advanced Micro Devices, Inc. [AMD]
\t14d8  Raphael/Granite Ridge Root Complex
C 03  Display controller
\t00  VGA compatible controller
\t\t00  VGA controller
C 06  Bridge
\t00  Host bridge
";

    fn db() -> PciIdsDb {
        PciIdsDb::parse(SAMPLE)
    }

    #[test]
    fn parses_vendors_devices_and_subsystems() {
        let db = db();
        assert_eq!(
            db.vendors.get(&0x10de).map(String::as_str),
            Some("NVIDIA Corporation")
        );
        assert_eq!(
            db.devices.get(&(0x10de, 0x2c05)).map(String::as_str),
            Some("GB203 [GeForce RTX 5070 Ti]")
        );
        assert_eq!(
            db.subsystems
                .get(&(0x10de, 0x2c05, 0x1043, 0x8a2f))
                .map(String::as_str),
            Some("TUF Gaming RTX 5070 Ti")
        );
        assert_eq!(
            db.devices.get(&(0x1022, 0x14d8)).map(String::as_str),
            Some("Raphael/Granite Ridge Root Complex")
        );
    }

    #[test]
    fn parses_classes_without_mixing_them_into_vendors() {
        let db = db();
        assert_eq!(
            db.classes.get(&0x03).map(String::as_str),
            Some("Display controller")
        );
        assert_eq!(
            db.subclasses.get(&(0x03, 0x00)).map(String::as_str),
            Some("VGA compatible controller")
        );
        assert_eq!(
            db.subclasses.get(&(0x06, 0x00)).map(String::as_str),
            Some("Host bridge")
        );
        // Class-section "00" lines must not become devices of the last vendor.
        assert_eq!(db.devices.len(), 2);
    }

    #[test]
    fn skips_malformed_lines() {
        let db = PciIdsDb::parse("zzzz  Bad vendor\n\t1234  Orphan device\n10de\n");
        assert!(db.vendors.is_empty());
        assert!(db.devices.is_empty());
    }

    #[test]
    fn falls_back_to_embedded_for_ids_the_file_lacks() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("pci.ids");
        fs::write(&path, SAMPLE).expect("write");
        let names = PciNames::from_files(&[path]);
        // Intel isn't in SAMPLE, but the built-in database knows it.
        assert_eq!(names.vendor(0x8086).as_deref(), Some("Intel Corporation"));
        assert_eq!(names.vendor(0x10de).as_deref(), Some("NVIDIA Corporation"));
    }

    #[test]
    fn newest_file_wins_and_older_files_fill_gaps() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let old = tmp.path().join("old.ids");
        let new = tmp.path().join("new.ids");
        fs::write(
            &old,
            "#\tVersion: 2020.01.01\n10de  Old NVIDIA\nabcd  Only In Old\n",
        )
        .expect("write");
        fs::write(&new, "#\tVersion: 2026.10.01\n10de  New NVIDIA\n").expect("write");
        // Order given doesn't matter; versions decide.
        let names = PciNames::from_files(&[old, new.clone()]);
        assert_eq!(names.vendor(0x10de).as_deref(), Some("New NVIDIA"));
        assert_eq!(names.vendor(0xabcd).as_deref(), Some("Only In Old"));
        assert_eq!(names.sources()[0].path, new);
        assert_eq!(names.sources()[0].version.as_deref(), Some("2026.10.01"));
    }

    #[test]
    fn embedded_database_knows_common_vendors() {
        let names = PciNames::embedded();
        assert_eq!(names.vendor(0x8086).as_deref(), Some("Intel Corporation"));
        assert!(names.class(0x03, 0x00).is_some());
    }
}
