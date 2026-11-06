//! "What it does": a plain-English description of each device, from the updatable
//! `descriptions.toml`.

use std::collections::HashMap;

use serde::Deserialize;

use crate::data::{self, DataDirs, DataFile, DataProblem, DataSource};
use crate::data_text::DataText;
use crate::device::{Bus, Category};
use crate::rules::{Matcher, compile_matcher};

#[derive(Debug, Clone)]
pub struct Descriptions {
    pub version: String,
    pub source: DataSource,
    table: Table,
    /// Translations for the user's language (English by default).
    text: DataText,
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
            text: DataText::english(),
        }
    }

    /// Every English text in the built-in file, for the translation checks.
    #[cfg(test)]
    pub(crate) fn english_texts() -> Vec<String> {
        let table = Self::embedded().table;
        let mut texts: Vec<String> = table.entries.into_iter().map(|(_, t)| t).collect();
        texts.extend(table.by_category.into_values());
        texts
    }

    /// Describe devices in the language of `text`; English where it has no translation.
    pub fn with_text(mut self, text: DataText) -> Self {
        self.text = text;
        self
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
        best.map(|(_, text)| text)
            .or_else(|| {
                self.table
                    .by_category
                    .get(category_key(category))
                    .map(String::as_str)
            })
            .map(|english| self.text.get(english).to_owned())
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
        Category::Memory => "memory",
        Category::Motherboard => "motherboard",
        Category::SystemDevices => "system_devices",
        Category::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_are_translated_including_category_fallbacks() {
        let english = Descriptions::embedded();
        let sound = english
            .describe(Bus::Pci, &[], 0, 0, Category::Sound)
            .expect("sound has a fallback");
        let text = DataText::from_toml(&format!(
            "schema = 1\nversion = \"2026.11.06\"\nlanguage = \"de\"\n[text]\n{sound:?} = \"Spielt Ton ab.\"\n"
        ));
        let german = english.clone().with_text(text);
        assert_eq!(
            german
                .describe(Bus::Pci, &[], 0, 0, Category::Sound)
                .as_deref(),
            Some("Spielt Ton ab.")
        );
        // Text without a translation stays English.
        assert_eq!(
            german.describe(Bus::Pci, &[], 0, 0, Category::Network),
            english.describe(Bus::Pci, &[], 0, 0, Category::Network)
        );
    }

    const ALL: [Category; 18] = [
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
        Category::Memory,
        Category::Motherboard,
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

    #[test]
    fn every_new_pci_entry_matches() {
        let d = Descriptions::embedded();
        for (class, subclass) in [
            (0x01, 0x00),
            (0x01, 0x01),
            (0x01, 0x04),
            (0x01, 0x05),
            (0x01, 0x07),
            (0x01, 0x80),
            (0x03, 0x02),
            (0x04, 0x00),
            (0x04, 0x01),
            (0x07, 0x00),
            (0x07, 0x01),
            (0x07, 0x03),
            (0x08, 0x05),
            (0x0c, 0x00),
            (0x0c, 0x06),
            (0x0c, 0x80),
            (0x0d, 0x80),
            (0x11, 0x80),
            (0x13, 0x00),
        ] {
            let code = (class << 16) | (subclass << 8);
            assert!(
                d.describe(Bus::Pci, &[code], 0, 0, Category::Other)
                    .is_some_and(|text| !text.contains("doesn't fit the usual groups")),
                "no specific PCI description for {class:02x}/{subclass:02x}"
            );
        }
    }

    #[test]
    fn every_new_usb_entry_matches() {
        let d = Descriptions::embedded();
        for (class, subclass) in [
            (0x0f, 0x00),
            (0x10, 0x00),
            (0x11, 0x00),
            (0xdc, 0x00),
            (0xef, 0x00),
            (0xfe, 0x01),
        ] {
            let code = (class << 16) | (subclass << 8);
            assert!(
                d.describe(Bus::Usb, &[code], 0, 0, Category::Other)
                    .is_some_and(|text| !text.contains("doesn't fit the usual groups")),
                "no specific USB description for {class:02x}/{subclass:02x}"
            );
        }
    }
}
