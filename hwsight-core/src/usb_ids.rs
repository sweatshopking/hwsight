//! Human-readable names for USB vendors, products, and classes.
//!
//! Loaded the same way as PCI names: every `usb.ids` found, newest first, with older
//! files filling gaps. See `pci_ids.rs`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::data::DataDirs;
use crate::pci_ids::{IdsSource, header_version};

/// Name lookups for USB IDs.
#[derive(Debug)]
pub struct UsbNames {
    layers: Vec<IdsLayer>,
}

#[derive(Debug)]
struct IdsLayer {
    path: PathBuf,
    version: Option<String>,
    db: UsbIdsDb,
}

impl UsbNames {
    /// Every `usb.ids` in the data folders.
    pub fn load(dirs: &DataDirs) -> Self {
        Self::from_files(&dirs.id_files("usb.ids"))
    }

    /// The given files, newest first, with older files filling missing names.
    pub fn from_files(paths: &[PathBuf]) -> Self {
        let mut layers: Vec<IdsLayer> = paths.iter().filter_map(|path| load_layer(path)).collect();
        layers.sort_by(|a, b| b.version.cmp(&a.version));
        Self { layers }
    }

    /// Only the database built into the program.
    pub fn embedded() -> Self {
        // TODO: embed a usb.ids snapshot as a last-resort fallback.
        Self { layers: Vec::new() }
    }

    /// The name lists in use, newest first.
    pub fn sources(&self) -> Vec<IdsSource> {
        self.layers
            .iter()
            .map(|layer| IdsSource {
                path: layer.path.clone(),
                version: layer.version.clone(),
            })
            .collect()
    }

    /// The vendor's name, e.g. `046d` -> "Logitech, Inc.".
    pub fn vendor(&self, vid: u16) -> Option<String> {
        self.find(|db| db.vendors.get(&vid).cloned())
    }

    /// The product's name, e.g. `046d:c52b` -> "Unifying Receiver".
    pub fn product(&self, vid: u16, pid: u16) -> Option<String> {
        self.find(|db| db.products.get(&(vid, pid)).cloned())
    }

    /// The most specific name known for an interface class: protocol, else subclass,
    /// else class.
    pub fn class(&self, class: u8, subclass: u8, protocol: u8) -> Option<String> {
        self.find(|db| db.protocols.get(&(class, subclass, protocol)).cloned())
            .or_else(|| self.find(|db| db.subclasses.get(&(class, subclass)).cloned()))
            .or_else(|| self.find(|db| db.classes.get(&class).cloned()))
    }

    fn find<T>(&self, get: impl Fn(&UsbIdsDb) -> Option<T>) -> Option<T> {
        self.layers.iter().find_map(|layer| get(&layer.db))
    }
}

fn load_layer(path: &Path) -> Option<IdsLayer> {
    let bytes = fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let db = UsbIdsDb::parse(&text);
    if db.vendors.is_empty() {
        return None;
    }
    Some(IdsLayer {
        path: path.to_path_buf(),
        version: header_version(&text),
        db,
    })
}

/// Whether text parses as a `usb.ids` file with at least one vendor.
pub(crate) fn looks_valid(text: &str) -> bool {
    !UsbIdsDb::parse(text).vendors.is_empty()
}

#[derive(Debug, Default)]
struct UsbIdsDb {
    vendors: HashMap<u16, String>,
    products: HashMap<(u16, u16), String>,
    classes: HashMap<u8, String>,
    subclasses: HashMap<(u8, u8), String>,
    protocols: HashMap<(u8, u8, u8), String>,
}

impl UsbIdsDb {
    fn parse(text: &str) -> Self {
        let mut db = Self::default();
        let mut vendor = None;
        let mut class = None;
        let mut subclass = None;
        let mut section = Section::Vendors;

        for line in text.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let depth = line.bytes().take_while(|&byte| byte == b'\t').count();
            let body = &line[depth..];

            if depth == 0 {
                if let Some((id, name)) = body.strip_prefix("C ").and_then(split_id) {
                    if let Some(id) = parse_byte(id) {
                        db.classes.insert(id, name.to_owned());
                        class = Some(id);
                        subclass = None;
                        vendor = None;
                        section = Section::Classes;
                    } else {
                        class = None;
                        subclass = None;
                        section = Section::Ignored;
                    }
                } else if is_ignored_section(body) {
                    vendor = None;
                    class = None;
                    subclass = None;
                    section = Section::Ignored;
                } else if let Some((id, name)) =
                    split_id(body).and_then(|(id, name)| parse_hex(id).map(|id| (id, name)))
                {
                    db.vendors.insert(id, name.to_owned());
                    vendor = Some(id);
                    class = None;
                    subclass = None;
                    section = Section::Vendors;
                } else {
                    vendor = None;
                    class = None;
                    subclass = None;
                    section = Section::Ignored;
                }
                continue;
            }

            match section {
                Section::Vendors if depth == 1 => {
                    if let (Some(vid), Some((pid, name))) = (
                        vendor,
                        split_id(body).and_then(|(id, name)| parse_hex(id).map(|id| (id, name))),
                    ) {
                        db.products.insert((vid, pid), name.to_owned());
                    }
                }
                Section::Classes if depth == 1 => {
                    // A broken subclass line must not leave its protocols attached to the
                    // previous subclass.
                    subclass = None;
                    if let (Some(cid), Some((sid, name))) = (
                        class,
                        split_id(body).and_then(|(id, name)| parse_byte(id).map(|id| (id, name))),
                    ) {
                        db.subclasses.insert((cid, sid), name.to_owned());
                        subclass = Some(sid);
                    }
                }
                Section::Classes if depth == 2 => {
                    if let (Some(cid), Some(sid), Some((pid, name))) = (
                        class,
                        subclass,
                        split_id(body).and_then(|(id, name)| parse_byte(id).map(|id| (id, name))),
                    ) {
                        db.protocols.insert((cid, sid, pid), name.to_owned());
                    }
                }
                _ => {}
            }
        }
        db
    }
}

#[derive(Clone, Copy)]
enum Section {
    Vendors,
    Classes,
    Ignored,
}

fn split_id(body: &str) -> Option<(&str, &str)> {
    let (id, name) = body.trim_start().split_once(char::is_whitespace)?;
    let name = name.trim();
    (!id.is_empty() && !name.is_empty()).then_some((id, name))
}

fn parse_hex(value: &str) -> Option<u16> {
    (value.len() == 4)
        .then(|| u16::from_str_radix(value, 16).ok())
        .flatten()
}

fn parse_byte(value: &str) -> Option<u8> {
    (value.len() == 2)
        .then(|| u8::from_str_radix(value, 16).ok())
        .flatten()
}

fn is_ignored_section(body: &str) -> bool {
    ["AT", "HID", "R", "BIAS", "PHY", "HUT", "L", "HCC", "VT"]
        .iter()
        .any(|prefix| body == *prefix || body.starts_with(&format!("{prefix} ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Version: 2026.10.01
1d6b  Linux Foundation
\t0001  1.1 root hub
\t0002  2.0 root hub
046d  Logitech, Inc.
\tc534  Unifying Receiver
\tc52b  Nano Receiver
C 01  Audio
\t01  Audio Control
\t\t01  Protocol One
C 03  Human Interface Device
\t01  Boot Interface
\t\t01  Keyboard
";

    #[test]
    fn parses_vendors_and_products() {
        let db = UsbIdsDb::parse(SAMPLE);
        assert_eq!(db.vendors.len(), 2);
        assert_eq!(db.products.len(), 4);
        assert_eq!(
            db.vendors.get(&0x046d).map(String::as_str),
            Some("Logitech, Inc.")
        );
        assert_eq!(
            db.products.get(&(0x046d, 0xc534)).map(String::as_str),
            Some("Unifying Receiver")
        );
        assert_eq!(
            db.vendors.get(&0x1d6b).map(String::as_str),
            Some("Linux Foundation")
        );
        assert_eq!(
            db.products.get(&(0x1d6b, 1)).map(String::as_str),
            Some("1.1 root hub")
        );
        assert_eq!(
            db.products.get(&(0x1d6b, 2)).map(String::as_str),
            Some("2.0 root hub")
        );
        assert_eq!(
            db.products.get(&(0x046d, 0xc52b)).map(String::as_str),
            Some("Nano Receiver")
        );
    }

    #[test]
    fn class_lookup_is_most_specific() {
        let db = UsbIdsDb::parse(SAMPLE);
        assert_eq!(db.classes.get(&1).map(String::as_str), Some("Audio"));
        assert_eq!(
            db.subclasses.get(&(1, 1)).map(String::as_str),
            Some("Audio Control")
        );
        assert_eq!(
            db.protocols.get(&(1, 1, 1)).map(String::as_str),
            Some("Protocol One")
        );
        let names = UsbNames {
            layers: vec![IdsLayer {
                path: PathBuf::new(),
                version: None,
                db,
            }],
        };
        assert_eq!(names.class(1, 0, 0).as_deref(), Some("Audio"));
        assert_eq!(names.class(1, 1, 0).as_deref(), Some("Audio Control"));
        assert_eq!(names.class(1, 1, 1).as_deref(), Some("Protocol One"));
        assert_eq!(names.class(1, 1, 2).as_deref(), Some("Audio Control"));
    }

    #[test]
    fn other_sections_are_ignored() {
        // Same shape as the real file: top-level "AT 0101  ..." lines, and indented
        // usages under "HUT" and dialects under "L".
        let db = UsbIdsDb::parse(
            "1d6b  Linux Foundation\n\t0001  Root hub\n\
C 03  Human Interface Device\n\t01  Boot Interface\n\t\t01  Keyboard\n\
AT 0101  USB Streaming\nHID 01  HID\nR 00  Not Specified\n\
HUT 01  Generic Desktop Controls\n\t001  Pointer\n\t002  Mouse\n\
L 0001  Arabic\n\t01  Saudi Arabia\n\t02  Iraq\n",
        );
        assert_eq!(db.vendors.len(), 1);
        assert_eq!(db.products.len(), 1);
        assert_eq!(db.classes.len(), 1);
        assert_eq!(db.subclasses.len(), 1);
        assert_eq!(db.protocols.len(), 1);
        assert_eq!(db.vendors.get(&1), None);
        assert_eq!(db.products.get(&(0, 1)), None);
        assert_eq!(db.classes.get(&0), None);
    }

    #[test]
    fn newest_file_wins_and_older_files_fill_gaps() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let old = tmp.path().join("old.ids");
        let new = tmp.path().join("new.ids");
        fs::write(
            &old,
            "# Version: 2020.01.01\n1d6b  Old Name\nabcd  Only Old\n",
        )
        .expect("write");
        fs::write(&new, "# Version: 2026.10.01\n1d6b  New Name\n").expect("write");
        let names = UsbNames::from_files(&[old, new.clone()]);
        assert_eq!(names.vendor(0x1d6b).as_deref(), Some("New Name"));
        assert_eq!(names.vendor(0xabcd).as_deref(), Some("Only Old"));
        assert_eq!(names.sources()[0].path, new);
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let db = UsbIdsDb::parse("zzzz  Bad\n\t1234  Orphan\n1d6b\n1d6b  Good\n\tbad  Bad\n");
        assert_eq!(db.vendors.len(), 1);
        assert_eq!(db.products.len(), 0);
    }

    #[test]
    fn real_system_file_if_present() {
        let path = Path::new("/usr/share/hwdata/usb.ids");
        if !path.is_file() {
            return;
        }
        let names = UsbNames::from_files(&[path.to_path_buf()]);
        assert_eq!(names.vendor(0x1d6b).as_deref(), Some("Linux Foundation"));
        assert_eq!(
            names.product(0x1d6b, 0x0002).as_deref(),
            Some("2.0 root hub")
        );
        assert_eq!(names.class(0x03, 0x00, 0x01).as_deref(), Some("Keyboard"));
        assert_eq!(
            names.class(0x09, 0x00, 0x00).as_deref(),
            Some("Full speed (or root) hub")
        );
        assert!(names.sources()[0].version.is_some());
    }

    #[test]
    fn broken_subclass_line_drops_its_protocols() {
        let db = UsbIdsDb::parse(
            "1d6b  Linux Foundation\nC 03  HID\n\t01  Boot\n\t\t01  Keyboard\n\tzz  Broken\n\t\t02  Mouse\n",
        );
        assert_eq!(db.protocols.len(), 1);
        assert_eq!(db.protocols.get(&(3, 1, 2)), None);
    }
}
