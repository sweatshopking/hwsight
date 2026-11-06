//! "Useful software": programs worth having for a device (Solaar for a Logitech
//! receiver, liquidctl for a liquid cooler, ...), from the updatable `software.toml`.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use crate::data::{self, DataDirs, DataFile, DataProblem, DataSource};
use crate::data_text::DataText;
use crate::device::Bus;
use crate::distro::Distro;
use crate::rules::{Matcher, compile_matcher};

#[derive(Debug, Clone)]
pub struct Software {
    pub version: String,
    pub source: DataSource,
    table: Table,
    /// Translations for the user's language (English by default).
    text: DataText,
}

/// One recommended program, as written in `software.toml` (texts in English).
#[derive(Debug, Clone)]
pub struct Program {
    pub id: String,
    pub name: String,
    pub what: String,
    pub homepage: String,
    pub commands: Vec<String>,
    pub flathub: Option<String>,
    pub search: String,
    pub get: Option<String>,
    packages: HashMap<String, String>,
    matchers: Vec<Matcher>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Table {
    programs: Vec<Program>,
}

#[derive(Debug, Deserialize)]
struct RawFile {
    version: String,
    #[serde(default)]
    software: Vec<RawProgram>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProgram {
    id: String,
    name: String,
    what: String,
    homepage: String,
    #[serde(default)]
    commands: Vec<String>,
    flathub: Option<String>,
    #[serde(default)]
    packages: HashMap<String, String>,
    search: Option<String>,
    get: Option<String>,
    #[serde(rename = "match", default)]
    matches: Vec<RawMatch>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMatch {
    bus: Option<String>,
    class: Option<String>,
    subclasses: Option<Vec<String>>,
    prog_ifs: Option<Vec<String>>,
    vendor: Option<String>,
    device: Option<String>,
}

/// Program and package names: letters, digits, and - _ . + only (they end up in
/// commands, so nothing else may slip through).
fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
}

fn check(p: &RawProgram) -> Result<(), String> {
    let id = &p.id;
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "software id `{id}` must be lowercase letters, digits, and -"
        ));
    }
    if p.name.trim().is_empty() || p.what.trim().is_empty() {
        return Err(format!("software `{id}` needs a name and a `what` text"));
    }
    if !p.homepage.starts_with("https://") {
        return Err(format!("software `{id}`: homepage must be an https link"));
    }
    if let Some(app) = &p.flathub
        && !(is_name(app) && app.split('.').count() >= 3)
    {
        return Err(format!("software `{id}`: `{app}` isn't a Flathub app ID"));
    }
    if let Some(bad) = p
        .commands
        .iter()
        .chain(p.packages.values())
        .find(|n| !is_name(n))
    {
        return Err(format!("software `{id}`: `{bad}` isn't a valid name"));
    }
    if p.matches.is_empty() {
        return Err(format!("software `{id}` matches no devices"));
    }
    Ok(())
}

impl DataFile for Table {
    const FILE_NAME: &'static str = "software.toml";
    const SCHEMA: u32 = 1;
    const EMBEDDED: &'static str = include_str!("../data/software.toml");

    fn parse_body(text: &str) -> Result<(String, Self), String> {
        let raw: RawFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut seen = HashSet::new();
        let mut programs = Vec::new();
        for p in raw.software {
            check(&p)?;
            if !seen.insert(p.id.clone()) {
                return Err(format!("software `{}` is listed twice", p.id));
            }
            let matchers = p
                .matches
                .into_iter()
                .map(|m| {
                    compile_matcher(m.bus, m.class, m.subclasses, m.prog_ifs, m.vendor, m.device)
                })
                .collect::<Result<_, _>>()?;
            programs.push(Program {
                search: p.search.unwrap_or_else(|| p.name.clone()),
                id: p.id,
                name: p.name,
                what: p.what,
                homepage: p.homepage,
                commands: p.commands,
                flathub: p.flathub,
                get: p.get,
                packages: p.packages,
                matchers,
            });
        }
        Ok((raw.version, Self { programs }))
    }

    fn empty() -> Self {
        Self::default()
    }
}

impl Software {
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

    /// Show texts in the language of `text`; English where it has no translation.
    pub fn with_text(mut self, text: DataText) -> Self {
        self.text = text;
        self
    }

    /// Every program for a device that offers these class codes (one for a PCI device;
    /// one per interface for a USB device), in file order, each once.
    pub fn for_device(
        &self,
        bus: Bus,
        class_codes: &[u32],
        vendor: u16,
        device: u16,
    ) -> Vec<&Program> {
        // A device with no class codes can still match on vendor and device alone.
        let codes: &[u32] = if class_codes.is_empty() {
            &[0]
        } else {
            class_codes
        };
        self.table
            .programs
            .iter()
            .filter(|p| {
                p.matchers
                    .iter()
                    .any(|m| codes.iter().any(|&c| m.matches(bus, c, vendor, device)))
            })
            .collect()
    }

    /// The distro package for `program`, if this distro packages it.
    pub fn package_for<'a>(&self, program: &'a Program, distro: &Distro) -> Option<&'a str> {
        distro
            .data_keys()
            .iter()
            .find_map(|k| program.packages.get(k))
            .map(String::as_str)
            .filter(|p| *p != "none")
    }

    /// What the program does, in the user's language.
    pub fn what(&self, program: &Program) -> String {
        self.text.get(&program.what).to_owned()
    }

    /// How to get it without a distro package, in the user's language, if the data
    /// says. (Without it, the app writes its own "search for ..." advice.)
    pub fn get(&self, program: &Program) -> Option<String> {
        program.get.as_deref().map(|t| self.text.get(t).to_owned())
    }

    /// Every English text in the built-in file, for the translation checks.
    #[cfg(test)]
    pub(crate) fn english_texts() -> Vec<String> {
        Self::embedded()
            .table
            .programs
            .into_iter()
            .flat_map(|p| std::iter::once(p.what).chain(p.get))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distro::Family;

    fn distro(id: &str) -> Distro {
        Distro::from_os_release(&format!("ID={id}\n"), false)
    }

    #[test]
    fn built_in_file_is_valid() {
        assert!(data::parse_data::<Table>(Table::EMBEDDED).is_ok());
        assert!(!Software::embedded().table.programs.is_empty());
    }

    #[test]
    fn bolt_receiver_gets_solaar_and_unrelated_devices_get_nothing() {
        let s = Software::embedded();
        let found = s.for_device(Bus::Usb, &[0x030101], 0x046d, 0xc548);
        assert_eq!(
            found.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["solaar"]
        );
        assert!(
            s.for_device(Bus::Pci, &[0x030000], 0x046d, 0xc548)
                .is_empty()
        );
        assert!(
            s.for_device(Bus::Usb, &[0x030101], 0x046d, 0x0001)
                .is_empty()
        );
    }

    #[test]
    fn vendor_wide_match_and_no_class_codes() {
        let s = Software::embedded();
        let found = s.for_device(Bus::Usb, &[], 0x1e71, 0x300e);
        assert_eq!(
            found.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["liquidctl"]
        );
    }

    #[test]
    fn package_per_distro_and_none() {
        let s = Software::embedded();
        let openrgb = s
            .table
            .programs
            .iter()
            .find(|p| p.id == "openrgb")
            .expect("openrgb");
        assert_eq!(s.package_for(openrgb, &distro("fedora")), Some("openrgb"));
        assert_eq!(
            s.package_for(openrgb, &distro("opensuse-tumbleweed")),
            Some("OpenRGB")
        );
        assert_eq!(s.package_for(openrgb, &distro("debian")), None);
        assert_eq!(distro("debian").family, Family::Debian);
    }

    #[test]
    fn bad_entries_are_rejected() {
        let ok = "schema = 1\nversion = \"2026.11.06\"\n[[software]]\nid = \"x\"\nname = \"X\"\nwhat = \"Does x.\"\nhomepage = \"https://x.org\"\n[[software.match]]\nvendor = \"1234\"\n";
        assert!(data::parse_data::<Table>(ok).is_ok());
        for bad in [
            ok.replace("https://x.org", "http://x.org"),
            ok.replace("[[software.match]]\nvendor = \"1234\"\n", ""),
            ok.replace("id = \"x\"", "id = \"X Y\""),
            ok.replace("name = \"X\"", "name = \"X\"\ncommands = [\"x; rm -rf ~\"]"),
            ok.replace("name = \"X\"", "name = \"X\"\nflathub = \"notanid\""),
            format!(
                "{ok}[[software]]\nid = \"x\"\nname = \"X\"\nwhat = \"Again.\"\nhomepage = \"https://x.org\"\n[[software.match]]\nvendor = \"1\"\n"
            ),
        ] {
            assert!(data::parse_data::<Table>(&bad).is_err(), "accepted:\n{bad}");
        }
    }

    #[test]
    fn texts_are_translated() {
        let s = Software::embedded();
        let solaar = s
            .table
            .programs
            .iter()
            .find(|p| p.id == "solaar")
            .expect("solaar")
            .clone();
        let text = DataText::from_toml(&format!(
            "schema = 1\nversion = \"2026.11.06\"\nlanguage = \"de\"\n[text]\n{:?} = \"Koppelt Logitech-Geräte.\"\n",
            solaar.what
        ));
        let s = s.with_text(text);
        assert_eq!(s.what(&solaar), "Koppelt Logitech-Geräte.");
        assert_eq!(s.get(&solaar), None);
    }
}
