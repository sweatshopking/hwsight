//! "Useful software": programs worth having for a device (Solaar for a Logitech
//! receiver, liquidctl for a liquid cooler, ...), from the updatable `software.toml`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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

/// Filesystem-only checks for programs and Flatpak applications.
#[derive(Debug, Clone)]
pub struct InstallCheck {
    pub path_dirs: Vec<PathBuf>,
    pub flatpak_dirs: Vec<PathBuf>,
}

impl InstallCheck {
    /// Folders on `$PATH`, and the system and user Flatpak app folders.
    pub fn system() -> Self {
        let path_dirs = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .into_iter()
            .flatten()
            .filter(|path| !path.as_os_str().is_empty())
            .collect();
        let flatpak_dirs = std::iter::once(PathBuf::from("/var/lib/flatpak/app"))
            .chain(
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local/share/flatpak/app")),
            )
            .collect();
        Self {
            path_dirs,
            flatpak_dirs,
        }
    }

    /// One of the program's commands is an executable file on PATH, or its Flathub app
    /// is installed. Only looks at files; never runs anything.
    pub fn is_installed(&self, program: &Program) -> bool {
        program.commands.iter().any(|command| {
            self.path_dirs.iter().any(|dir| {
                let path = dir.join(command);
                is_executable(&path)
            })
        }) || program
            .flathub
            .as_ref()
            .is_some_and(|app| self.flatpak_dirs.iter().any(|dir| dir.join(app).is_dir()))
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && path
            .metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// One "useful software" recommendation for a device, ready to show.
#[derive(Debug, Clone, Serialize)]
pub struct Tip {
    pub id: String,
    pub name: String,
    pub what: String,
    pub homepage: String,
    pub installed: bool,
    pub install_command: Option<String>,
    pub how_to_get: Option<String>,
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

    /// The advice for one program: installed or not, and if not, the install command
    /// (Flathub first on an immutable distro, then a distro package, then Flathub) and
    /// how to get it when no distro package gave the command.
    pub fn tip(&self, program: &Program, distro: &Distro, check: &InstallCheck) -> Tip {
        let installed = check.is_installed(program);
        let package_command = self
            .package_for(program, distro)
            .and_then(|package| distro.family.install_command(package, distro.immutable));
        let flatpak_command = program
            .flathub
            .as_ref()
            .map(|id| format!("flatpak install flathub {id}"));
        let install_command = if installed {
            None
        } else if distro.immutable {
            flatpak_command.clone().or(package_command.clone())
        } else {
            package_command.clone().or(flatpak_command.clone())
        };
        let from_package = install_command.is_some() && install_command == package_command;
        let how_to_get = if installed || from_package {
            None
        } else if let Some(get) = self.get(program) {
            Some(get)
        } else if flatpak_command.is_some() && flatpak_command == install_command {
            Some(crate::tr!(
                "{} isn't in your distro's software sources, but you can install it from Flathub with this command.",
                program.name
            ))
        } else {
            Some(crate::tr!(
                "{} isn't in your distro's software sources. Look for \"{}\" in your app store (such as Discover or GNOME Software) or on Flathub, or follow the install steps on its website: {}",
                program.name,
                program.search,
                program.homepage
            ))
        };
        Tip {
            id: program.id.clone(),
            name: program.name.clone(),
            what: self.what(program),
            homepage: program.homepage.clone(),
            installed,
            install_command,
            how_to_get,
        }
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
    use std::fs;

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

    fn program<'a>(software: &'a Software, id: &str) -> &'a Program {
        software
            .table
            .programs
            .iter()
            .find(|p| p.id == id)
            .expect("program")
    }

    #[test]
    fn install_check_finds_a_command_on_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let executable = temp.path().join("solaar");
        fs::write(&executable, "").expect("write");
        let mut mode = fs::metadata(&executable).expect("metadata").permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            mode.set_mode(0o755);
        }
        fs::set_permissions(&executable, mode).expect("permissions");
        let check = InstallCheck {
            path_dirs: vec![temp.path().to_owned()],
            flatpak_dirs: vec![],
        };
        assert!(check.is_installed(program(&Software::embedded(), "solaar")));
        fs::write(temp.path().join("liquidctl"), "").expect("write");
        assert!(!check.is_installed(program(&Software::embedded(), "liquidctl")));
    }

    #[test]
    fn install_check_finds_a_flatpak() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(temp.path().join("org.openrgb.OpenRGB")).expect("mkdir");
        let check = InstallCheck {
            path_dirs: vec![],
            flatpak_dirs: vec![temp.path().to_owned()],
        };
        assert!(check.is_installed(program(&Software::embedded(), "openrgb")));
    }

    #[test]
    fn tip_uses_the_distro_package() {
        let s = Software::embedded();
        let tip = s.tip(
            program(&s, "liquidctl"),
            &distro("fedora"),
            &InstallCheck {
                path_dirs: vec![],
                flatpak_dirs: vec![],
            },
        );
        assert_eq!(
            tip.install_command.as_deref(),
            Some("sudo dnf install liquidctl")
        );
        assert_eq!(tip.how_to_get, None);
    }

    #[test]
    fn tip_prefers_flathub_on_an_immutable_distro() {
        let s = Software::embedded();
        let d = Distro::from_os_release("ID=fedora\n", true);
        let tip = s.tip(
            program(&s, "openrgb"),
            &d,
            &InstallCheck {
                path_dirs: vec![],
                flatpak_dirs: vec![],
            },
        );
        assert_eq!(
            tip.install_command.as_deref(),
            Some("flatpak install flathub org.openrgb.OpenRGB")
        );
        // Fedora packages it, but the Flathub command is the one shown, so say why.
        assert!(
            tip.how_to_get
                .as_deref()
                .is_some_and(|t| t.contains("Flathub"))
        );
    }

    #[test]
    fn tip_falls_back_to_flathub() {
        let s = Software::embedded();
        let tip = s.tip(
            program(&s, "openrgb"),
            &distro("debian"),
            &InstallCheck {
                path_dirs: vec![],
                flatpak_dirs: vec![],
            },
        );
        assert_eq!(
            tip.install_command.as_deref(),
            Some("flatpak install flathub org.openrgb.OpenRGB")
        );
        assert!(
            tip.how_to_get
                .as_deref()
                .is_some_and(|t| t.contains("Flathub"))
        );
    }

    #[test]
    fn tip_uses_the_get_text() {
        let s = Software::embedded();
        let tip = crate::i18n::in_english(|| {
            s.tip(
                program(&s, "liquidctl"),
                &distro("opensuse-tumbleweed"),
                &InstallCheck {
                    path_dirs: vec![],
                    flatpak_dirs: vec![],
                },
            )
        });
        assert_eq!(tip.install_command, None);
        assert_eq!(tip.how_to_get, s.get(program(&s, "liquidctl")));
    }

    #[test]
    fn tip_explains_how_to_search() {
        let s = Software {
            version: "test".into(),
            source: DataSource::Embedded,
            table: Table {
                programs: vec![Program {
                    id: "test".into(),
                    name: "Test app".into(),
                    what: "Does tests.".into(),
                    homepage: "https://example.com".into(),
                    commands: vec![],
                    flathub: None,
                    search: "test app package".into(),
                    get: None,
                    packages: HashMap::new(),
                    matchers: vec![],
                }],
            },
            text: DataText::english(),
        };
        let tip = crate::i18n::in_english(|| {
            s.tip(
                &s.table.programs[0],
                &distro("debian"),
                &InstallCheck {
                    path_dirs: vec![],
                    flatpak_dirs: vec![],
                },
            )
        });
        let advice = tip.how_to_get.expect("advice");
        assert!(advice.contains("Test app"));
        assert!(advice.contains("test app package"));
        assert!(advice.contains("https://example.com"));
    }

    #[test]
    fn installed_tip_has_no_command() {
        let s = Software::embedded();
        let temp = tempfile::tempdir().expect("tempdir");
        let command = temp.path().join("liquidctl");
        fs::write(&command, "").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&command).expect("metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&command, permissions).expect("permissions");
        }
        let tip = s.tip(
            program(&s, "liquidctl"),
            &distro("fedora"),
            &InstallCheck {
                path_dirs: vec![temp.path().to_owned()],
                flatpak_dirs: vec![],
            },
        );
        assert!(tip.installed);
        assert_eq!(tip.install_command, None);
        assert_eq!(tip.how_to_get, None);
    }
}
