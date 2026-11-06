//! Drivers that Linux doesn't ship: which package (or project) provides one for a device
//! that has no driver at all, from the updatable `drivers.toml`.
//!
//! A device only gets a suggestion here when its status is "no driver": every driver
//! the running kernel has was already tried, so there's no separate "unless the kernel
//! already supports it" check.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use crate::data::{self, DataDirs, DataFile, DataProblem, DataSource};
use crate::data_text::DataText;
use crate::device::Bus;
use crate::distro::Distro;
use crate::rules::{Matcher, compile_matcher};

#[derive(Debug, Clone)]
pub struct Drivers {
    pub version: String,
    pub source: DataSource,
    table: Table,
    /// Translations for the user's language (English by default).
    text: DataText,
}

/// Where a driver comes from on one distro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source<'a> {
    /// A package in the distro's own software sources.
    Repo(&'a str),
    /// A community package on the Arch User Repository. Built on the user's computer, so
    /// hwsight only explains it and never installs it.
    Aur(&'a str),
    /// No package for this distro: follow the project's page.
    ByHand,
}

#[derive(Debug, Clone)]
pub struct DriverEntry {
    pub id: String,
    pub name: String,
    pub what: String,
    pub homepage: String,
    packages: HashMap<String, String>,
    sections: HashMap<String, String>,
    matchers: Vec<Matcher>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Table {
    drivers: Vec<DriverEntry>,
}

#[derive(Debug, Deserialize)]
struct RawFile {
    version: String,
    #[serde(default)]
    driver: Vec<RawDriver>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDriver {
    id: String,
    name: String,
    what: String,
    homepage: String,
    #[serde(default)]
    packages: HashMap<String, String>,
    /// The archive section a repo package lives in, per distro, when it isn't on by
    /// default (Debian's `non-free`).
    #[serde(default)]
    sections: HashMap<String, String>,
    #[serde(rename = "match", default)]
    matches: Vec<RawMatch>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMatch {
    bus: Option<String>,
    vendor: String,
    device: String,
}

/// Package names end up in commands, so only the characters real names use.
fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
        && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
}

/// A package value is a name, `aur:<name>`, or `none`.
fn check_package(value: &str) -> bool {
    value == "none" || is_name(value.strip_prefix("aur:").unwrap_or(value))
}

fn check(d: &RawDriver) -> Result<(), String> {
    let id = &d.id;
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "driver id `{id}` must be lowercase letters, digits, and -"
        ));
    }
    if d.name.trim().is_empty() || d.what.trim().is_empty() {
        return Err(format!("driver `{id}` needs a name and a `what` text"));
    }
    if !d.homepage.starts_with("https://") {
        return Err(format!("driver `{id}`: homepage must be an https link"));
    }
    if let Some(bad) = d.packages.values().find(|p| !check_package(p)) {
        return Err(format!("driver `{id}`: `{bad}` isn't a valid package"));
    }
    if let Some(bad) = d
        .sections
        .values()
        .find(|s| s.is_empty() || !s.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
    {
        return Err(format!("driver `{id}`: `{bad}` isn't a valid section"));
    }
    if d.matches.is_empty() {
        return Err(format!("driver `{id}` matches no devices"));
    }
    Ok(())
}

impl DataFile for Table {
    const FILE_NAME: &'static str = "drivers.toml";
    const SCHEMA: u32 = 1;
    const EMBEDDED: &'static str = include_str!("../data/drivers.toml");

    fn parse_body(text: &str) -> Result<(String, Self), String> {
        let raw: RawFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut seen = HashSet::new();
        let mut drivers = Vec::new();
        for d in raw.driver {
            check(&d)?;
            if !seen.insert(d.id.clone()) {
                return Err(format!("driver `{}` is listed twice", d.id));
            }
            // Drivers are matched on the exact hardware ID only: a class or vendor-wide
            // match would offer third-party code for devices it was never meant for.
            let matchers = d
                .matches
                .into_iter()
                .map(|m| compile_matcher(m.bus, None, None, None, Some(m.vendor), Some(m.device)))
                .collect::<Result<_, _>>()?;
            drivers.push(DriverEntry {
                id: d.id,
                name: d.name,
                what: d.what,
                homepage: d.homepage,
                packages: d.packages,
                sections: d.sections,
                matchers,
            });
        }
        Ok((raw.version, Self { drivers }))
    }

    fn empty() -> Self {
        Self::default()
    }
}

impl Drivers {
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

    /// Every English `what` text in the built-in file, for the translation checks.
    #[cfg(test)]
    pub(crate) fn english_texts() -> Vec<String> {
        Self::embedded()
            .table
            .drivers
            .into_iter()
            .map(|d| d.what)
            .collect()
    }

    /// Drivers for a device with this hardware ID, in file order.
    pub fn for_device(&self, bus: Bus, vendor: u16, product: u16) -> Vec<&DriverEntry> {
        self.table
            .drivers
            .iter()
            .filter(|d| {
                d.matchers
                    .iter()
                    .any(|m| m.matches(bus, 0, vendor, product))
            })
            .collect()
    }

    /// Where `driver` comes from on this distro. The most specific release key wins
    /// (`debian-bookworm` before `debian`).
    pub fn source<'a>(&self, driver: &'a DriverEntry, distro: &Distro) -> Source<'a> {
        let value = distro
            .data_keys()
            .iter()
            .find_map(|k| driver.packages.get(k))
            .map(String::as_str);
        match value {
            None | Some("none") => Source::ByHand,
            Some(v) => match v.strip_prefix("aur:") {
                Some(name) => Source::Aur(name),
                None => Source::Repo(v),
            },
        }
    }

    /// The command that installs driver `id` on this distro. Only a repo package from
    /// this file qualifies: AUR packages are built from community recipes and are never
    /// installed by hwsight, and a distro without a package has nothing to install.
    pub(crate) fn install_argv(&self, id: &str, distro: &Distro) -> Result<Vec<String>, String> {
        let driver = self
            .table
            .drivers
            .iter()
            .find(|d| d.id == id)
            .ok_or_else(|| crate::tr!("{} isn't a driver hwsight knows how to install.", id))?;
        let Source::Repo(package) = self.source(driver, distro) else {
            return Err(crate::tr!(
                "hwsight can only install drivers that your distro packages itself."
            ));
        };
        distro
            .family
            .install_argv(&[package], distro.immutable)
            .ok_or_else(|| {
                crate::tr!("hwsight can't install software on this system. Use your system's package manager instead.")
            })
    }

    /// Install driver `id` through pkexec, which asks for the password in the desktop's
    /// own window. The package comes from this file, never from the caller.
    pub fn install(&self, id: &str, distro: &Distro) -> Result<String, String> {
        crate::optional_tools::run_installer(&self.install_argv(id, distro)?)
    }

    /// The archive section `driver`'s package lives in on this distro, when it needs
    /// switching on (e.g. Debian's `non-free`).
    pub fn section<'a>(&self, driver: &'a DriverEntry, distro: &Distro) -> Option<&'a str> {
        distro
            .data_keys()
            .iter()
            .find_map(|k| driver.sections.get(k))
            .map(String::as_str)
    }

    /// What the driver is for, in the user's language.
    pub fn what(&self, driver: &DriverEntry) -> String {
        self.text.get(&driver.what).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn distro(os_release: &str) -> Distro {
        Distro::from_os_release(os_release, false)
    }

    fn table(body: &str) -> Result<(String, Table), String> {
        Table::parse_body(&format!("schema = 1\nversion = \"2026.11.07\"\n{body}"))
    }

    const ENTRY: &str = r#"
[[driver]]
id = "broadcom-wl"
name = "Broadcom wl"
what = "Wi-Fi driver."
homepage = "https://example.org/wl"
packages = { arch = "broadcom-wl", debian = "broadcom-sta-dkms", ubuntu-jammy = "bcmwl-kernel-source", fedora = "none", opensuse = "aur:nope" }
[[driver.match]]
vendor = "14e4"
device = "43a0"
"#;

    #[test]
    fn embedded_file_is_valid() {
        assert!(data::parse_data::<Table>(Table::EMBEDDED).is_ok());
        assert!(!Drivers::embedded().table.drivers.is_empty());
    }

    #[test]
    fn matches_the_exact_hardware_id_only() {
        let (_, t) = table(ENTRY).unwrap_or_default();
        let d = Drivers {
            version: String::new(),
            source: DataSource::Embedded,
            table: t,
            text: DataText::english(),
        };
        assert_eq!(d.for_device(Bus::Pci, 0x14e4, 0x43a0).len(), 1);
        assert!(d.for_device(Bus::Pci, 0x14e4, 0x43a1).is_empty());
        assert!(d.for_device(Bus::Usb, 0x14e4, 0x43a0).is_empty());
    }

    #[test]
    fn source_depends_on_the_distro() {
        let (_, t) = table(ENTRY).unwrap_or_default();
        let d = Drivers {
            version: String::new(),
            source: DataSource::Embedded,
            table: t,
            text: DataText::english(),
        };
        let entry = &d.table.drivers[0];
        assert_eq!(
            d.source(entry, &distro("ID=arch\n")),
            Source::Repo("broadcom-wl")
        );
        assert_eq!(
            d.source(entry, &distro("ID=debian\n")),
            Source::Repo("broadcom-sta-dkms")
        );
        // A release key beats the family key.
        assert_eq!(
            d.source(entry, &distro("ID=ubuntu\nVERSION_CODENAME=jammy\n")),
            Source::Repo("bcmwl-kernel-source")
        );
        assert_eq!(d.source(entry, &distro("ID=fedora\n")), Source::ByHand);
        assert_eq!(
            d.source(
                entry,
                &distro("ID=opensuse-tumbleweed\nID_LIKE=\"opensuse suse\"\n")
            ),
            Source::Aur("nope")
        );
        assert_eq!(d.source(entry, &distro("ID=plan9\n")), Source::ByHand);
    }

    #[test]
    fn only_repo_packages_can_be_installed() {
        let (_, t) = table(ENTRY).unwrap_or_default();
        let d = Drivers {
            version: String::new(),
            source: DataSource::Embedded,
            table: t,
            text: DataText::english(),
        };
        assert_eq!(
            d.install_argv("broadcom-wl", &distro("ID=debian\n")),
            Ok(vec![
                "apt-get".to_owned(),
                "install".to_owned(),
                "-y".to_owned(),
                "broadcom-sta-dkms".to_owned()
            ])
        );
        // No package, an AUR package, an unknown id, and a read-only system all refuse.
        assert!(
            d.install_argv("broadcom-wl", &distro("ID=fedora\n"))
                .is_err()
        );
        assert!(
            d.install_argv(
                "broadcom-wl",
                &distro("ID=opensuse-tumbleweed\nID_LIKE=\"opensuse suse\"\n")
            )
            .is_err()
        );
        assert!(d.install_argv("nope", &distro("ID=debian\n")).is_err());
        assert!(
            d.install_argv("broadcom-wl", &Distro::from_os_release("ID=debian\n", true))
                .is_err()
        );
    }

    #[test]
    fn bad_data_is_rejected() {
        for bad in [
            ENTRY.replace("broadcom-sta-dkms", "x; rm -rf ~"),
            ENTRY.replace("https://example.org/wl", "http://example.org/wl"),
            ENTRY.replace("14e4", "zzzz"),
            format!("{ENTRY}{ENTRY}"),
            ENTRY.replace(
                "[[driver.match]]\nvendor = \"14e4\"\ndevice = \"43a0\"\n",
                "",
            ),
        ] {
            assert!(table(&bad).is_err(), "accepted: {bad}");
        }
    }
}
