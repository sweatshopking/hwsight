//! "What it does": a plain-English description of each device, from the updatable
//! `descriptions.toml`.

use std::collections::HashMap;

use serde::Deserialize;

use crate::data::{self, DataDirs, DataFile, DataProblem, DataSource};
use crate::device::{Bus, Category};
use crate::rules::{Matcher, compile_matcher};

#[derive(Debug, Clone)]
pub struct Descriptions {
    pub version: String,
    pub source: DataSource,
    table: Table,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Table {
    entries: Vec<(Matcher, String)>,
    /// Keyed by the category's JSON name, e.g. "usb_controllers".
    by_category: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct RawFile {
    version: String,
    #[serde(default)]
    category: HashMap<String, String>,
    #[serde(default)]
    describe: Vec<RawEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    bus: Option<String>,
    class: Option<String>,
    subclasses: Option<Vec<String>>,
    prog_ifs: Option<Vec<String>>,
    vendor: Option<String>,
    device: Option<String>,
    text: String,
}

impl DataFile for Table {
    const FILE_NAME: &'static str = "descriptions.toml";
    const SCHEMA: u32 = 1;
    const EMBEDDED: &'static str = include_str!("../data/descriptions.toml");

    fn parse_body(text: &str) -> Result<(String, Self), String> {
        let raw: RawFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let entries = raw
            .describe
            .into_iter()
            .map(|e| {
                if e.text.trim().is_empty() {
                    return Err("a description can't be empty".to_owned());
                }
                let m =
                    compile_matcher(e.bus, e.class, e.subclasses, e.prog_ifs, e.vendor, e.device)?;
                Ok((m, e.text))
            })
            .collect::<Result<_, String>>()?;
        Ok((
            raw.version,
            Self {
                entries,
                by_category: raw.category,
            },
        ))
    }

    fn empty() -> Self {
        Self::default()
    }
}

impl Descriptions {
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

    /// The best description for a device that offers these class codes (one for a PCI
    /// device; one per interface for a USB device, in order). The most specific match
    /// wins, and on a tie the earlier class code does; with no match, the category's text.
    pub fn describe(
        &self,
        bus: Bus,
        class_codes: &[u32],
        vendor: u16,
        device: u16,
        category: Category,
    ) -> Option<String> {
        let mut best: Option<(u8, &str)> = None;
        for &code in class_codes {
            for (m, text) in &self.table.entries {
                if m.matches(bus, code, vendor, device)
                    && best.is_none_or(|(score, _)| m.specificity() > score)
                {
                    best = Some((m.specificity(), text));
                }
            }
        }
        best.map(|(_, text)| text.to_owned())
            .or_else(|| self.table.by_category.get(category_key(category)).cloned())
    }
}

/// The category's name as written in data files (its JSON name).
fn category_key(category: Category) -> &'static str {
    match category {
        Category::Graphics => "graphics",
        Category::Sound => "sound",
        Category::Network => "network",
        Category::Bluetooth => "bluetooth",
        Category::Storage => "storage",
        Category::UsbControllers => "usb_controllers",
        Category::Thunderbolt => "thunderbolt",
        Category::Input => "input",
        Category::Camera => "camera",
        Category::Printers => "printers",
        Category::Communication => "communication",
        Category::Security => "security",
        Category::Accelerators => "accelerators",
        Category::Processors => "processors",
        Category::SystemDevices => "system_devices",
        Category::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Category; 16] = [
        Category::Graphics,
        Category::Sound,
        Category::Network,
        Category::Bluetooth,
        Category::Storage,
        Category::UsbControllers,
        Category::Thunderbolt,
        Category::Input,
        Category::Camera,
        Category::Printers,
        Category::Communication,
        Category::Security,
        Category::Accelerators,
        Category::Processors,
        Category::SystemDevices,
        Category::Other,
    ];

    #[test]
    fn embedded_file_is_valid_and_every_category_has_text() {
        assert!(data::parse_data::<Table>(Table::EMBEDDED).is_ok());
        let d = Descriptions::embedded();
        for c in ALL {
            assert!(
                d.describe(Bus::Pci, &[], 0, 0, c)
                    .is_some_and(|t| !t.is_empty()),
                "no text for {c:?}"
            );
        }
    }

    #[test]
    fn most_specific_match_wins() {
        let d = Descriptions::embedded();
        let usb = |codes: &[u32]| d.describe(Bus::Usb, codes, 0, 0, Category::Other);
        let pci = |code: u32| d.describe(Bus::Pci, &[code], 0, 0, Category::Other);
        assert!(pci(0x0c0330).is_some_and(|t| t.contains("group of USB ports")));
        assert!(
            pci(0x0c0340).is_some_and(|t| t.contains("USB4")),
            "prog-if beats subclass"
        );
        assert!(pci(0x028000).is_some_and(|t| t.contains("Wi-Fi")));
        // A webcam offers video then audio: the earlier interface wins the tie.
        assert!(usb(&[0x0e0100, 0x010100]).is_some_and(|t| t.contains("webcam")));
        // Bluetooth's exact class/subclass/protocol beats a generic HID interface.
        assert!(usb(&[0x030000, 0xe00101]).is_some_and(|t| t.contains("Bluetooth")));
        // USB class 09 is a hub; PCI class 09 must not get the hub text.
        assert!(usb(&[0x090000]).is_some_and(|t| t.contains("more USB ports")));
        assert!(pci(0x090000).is_some_and(|t| t.contains("doesn't fit")));
    }

    #[test]
    fn exact_devices_beat_their_connection_type() {
        let d = Descriptions::embedded();
        // NZXT Kraken: a vendor-specific and a HID interface, but the exact entry wins.
        let kraken = d.describe(
            Bus::Usb,
            &[0xff0000, 0x030000],
            0x1e71,
            0x300e,
            Category::Input,
        );
        assert!(kraken.is_some_and(|t| t.contains("liquid cooler")));
        // Any Xbox 360 style pad, whatever its maker.
        let pad = d.describe(Bus::Usb, &[0xff5d01], 0x1234, 0x5678, Category::Input);
        assert!(pad.is_some_and(|t| t.contains("game controller")));
    }

    #[test]
    fn falls_back_to_the_category() {
        let d = Descriptions::embedded();
        let vendor_specific = d.describe(Bus::Usb, &[0xffffff], 0, 0, Category::Input);
        assert!(vendor_specific.is_some_and(|t| t.contains("game controllers")));
    }

    #[test]
    fn empty_text_rejects_the_file() {
        let text =
            "schema = 1\nversion = \"2099.01.01\"\n[[describe]]\nclass = \"03\"\ntext = \" \"\n";
        assert!(data::parse_data::<Table>(text).is_err());
    }
}
