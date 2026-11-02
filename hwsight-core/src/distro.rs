//! Which Linux distribution this is, so advice names the right packages and commands.

use std::fs;
use std::path::Path;

use serde::Serialize;

/// Distro families that share package names and tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Arch,
    Debian,
    Ubuntu,
    Fedora,
    OpenSuse,
    Unknown,
}

impl Family {
    /// The key used for this family in hwsight data files.
    pub fn key(self) -> Option<&'static str> {
        match self {
            Self::Arch => Some("arch"),
            Self::Debian => Some("debian"),
            Self::Ubuntu => Some("ubuntu"),
            Self::Fedora => Some("fedora"),
            Self::OpenSuse => Some("opensuse"),
            Self::Unknown => None,
        }
    }

    /// The command a user would run to install `package`, or `None` if we don't know it.
    /// For display only: hwsight version 1 never installs anything itself.
    pub fn install_command(self, package: &str, immutable: bool) -> Option<String> {
        if immutable {
            return match self {
                Self::Fedora => Some(format!("rpm-ostree install {package}")),
                _ => None,
            };
        }
        Some(match self {
            Self::Arch => format!("sudo pacman -S {package}"),
            Self::Debian | Self::Ubuntu => format!("sudo apt install {package}"),
            Self::Fedora => format!("sudo dnf install {package}"),
            Self::OpenSuse => format!("sudo zypper install {package}"),
            Self::Unknown => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Distro {
    /// `ID` from os-release, e.g. `cachyos`.
    pub id: String,
    /// `PRETTY_NAME` (or `NAME`) for showing to people.
    pub name: String,
    pub family: Family,
    /// Read-only system (Fedora Atomic, Bazzite, SteamOS...): normal installs don't work.
    pub immutable: bool,
}

impl Distro {
    pub fn detect() -> Self {
        let text = ["/etc/os-release", "/usr/lib/os-release"]
            .iter()
            .find_map(|p| fs::read_to_string(p).ok())
            .unwrap_or_default();
        let immutable = Path::new("/run/ostree-booted").exists();
        Self::from_os_release(&text, immutable)
    }

    pub fn from_os_release(text: &str, immutable: bool) -> Self {
        let field = |key: &str| {
            text.lines().find_map(|line| {
                let value = line.strip_prefix(key)?.strip_prefix('=')?;
                Some(value.trim().trim_matches(['"', '\'']).to_owned())
            })
        };
        let id = field("ID").unwrap_or_default().to_lowercase();
        let like = field("ID_LIKE").unwrap_or_default().to_lowercase();
        let name = field("PRETTY_NAME")
            .or_else(|| field("NAME"))
            .unwrap_or_else(|| "Linux".to_owned());
        let ids: Vec<&str> = std::iter::once(id.as_str())
            .chain(like.split_whitespace())
            .collect();
        let has = |want: &[&str]| ids.iter().any(|i| want.contains(i));

        // Order matters: Ubuntu-based distros also list "debian", so check Ubuntu first.
        let family = if has(&["arch", "archlinux"]) {
            Family::Arch
        } else if has(&["ubuntu"]) {
            Family::Ubuntu
        } else if has(&["debian"]) {
            Family::Debian
        } else if has(&["fedora", "rhel", "centos"]) {
            Family::Fedora
        } else if has(&["opensuse", "suse", "opensuse-tumbleweed", "opensuse-leap"]) {
            Family::OpenSuse
        } else {
            Family::Unknown
        };
        Self {
            id,
            name,
            family,
            immutable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn family(os_release: &str) -> Family {
        Distro::from_os_release(os_release, false).family
    }

    #[test]
    fn families_from_real_os_release_files() {
        assert_eq!(
            family("NAME=\"CachyOS Linux\"\nID=cachyos\nID_LIKE=arch\n"),
            Family::Arch
        );
        assert_eq!(family("ID=arch\n"), Family::Arch);
        assert_eq!(
            family("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"),
            Family::Ubuntu
        );
        assert_eq!(
            family("ID=pop\nID_LIKE=\"ubuntu debian\"\n"),
            Family::Ubuntu
        );
        assert_eq!(family("ID=debian\n"), Family::Debian);
        assert_eq!(
            family("ID=nobara\nID_LIKE=\"rhel centos fedora\"\n"),
            Family::Fedora
        );
        assert_eq!(family("ID=bazzite\nID_LIKE=\"fedora\"\n"), Family::Fedora);
        assert_eq!(
            family("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n"),
            Family::OpenSuse
        );
        assert_eq!(family("ID=void\n"), Family::Unknown);
        assert_eq!(family(""), Family::Unknown);
    }

    #[test]
    fn ids_are_exact_not_substrings() {
        // "ID_LIKE" must not be read as "ID".
        let d = Distro::from_os_release("ID_LIKE=arch\nID=endeavouros\n", false);
        assert_eq!(d.id, "endeavouros");
        assert_eq!(d.family, Family::Arch);
    }

    #[test]
    fn install_commands() {
        assert_eq!(
            Family::Arch.install_command("x", false).as_deref(),
            Some("sudo pacman -S x")
        );
        assert_eq!(
            Family::Fedora.install_command("x", true).as_deref(),
            Some("rpm-ostree install x")
        );
        assert_eq!(Family::Arch.install_command("x", true), None);
        assert_eq!(Family::Unknown.install_command("x", false), None);
    }
}
