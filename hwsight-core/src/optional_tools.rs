//! Programs and services hwsight can use to show more, when the computer has them.
//!
//! hwsight works without any of these. When one is missing and the computer has the
//! hardware it would help with, a note says what you'd gain and which command installs
//! it. hwsight never installs anything by itself here.

use std::path::{Path, PathBuf};

use crate::device::{Category, Device};
use crate::distro::{Distro, Family};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    /// Shows which Wi-Fi generation a connection uses.
    Iw,
    /// NetworkManager's command; the service behind it gives Wi-Fi band and speed.
    NetworkManager,
    /// fwupd: firmware versions and updates.
    Fwupd,
    /// UDisks2: drive health.
    Udisks,
}

impl Tool {
    /// A program the tool installs, looked for on `$PATH`.
    fn program(self) -> &'static str {
        match self {
            Self::Iw => "iw",
            Self::NetworkManager => "nmcli",
            Self::Fwupd => "fwupdmgr",
            Self::Udisks => "udisksctl",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Iw => "iw",
            Self::NetworkManager => "NetworkManager",
            Self::Fwupd => "fwupd",
            Self::Udisks => "UDisks2",
        }
    }

    fn package(self, family: Family) -> &'static str {
        match (self, family) {
            (Self::Iw, _) => "iw",
            (Self::Fwupd, _) => "fwupd",
            (Self::Udisks, _) => "udisks2",
            (Self::NetworkManager, Family::Arch) => "networkmanager",
            (Self::NetworkManager, Family::Debian | Family::Ubuntu) => "network-manager",
            (Self::NetworkManager, _) => "NetworkManager",
        }
    }

    fn benefit(self) -> String {
        match self {
            Self::Iw => crate::tr!(
                "hwsight can show which Wi-Fi generation (4, 5, 6, or 7) your connection uses and whether your adapter could do better."
            ),
            Self::NetworkManager => {
                crate::tr!("hwsight can show your Wi-Fi band and connection speed.")
            }
            Self::Fwupd => crate::tr!(
                "hwsight can show firmware versions and tell you when drives, receivers, and other devices have updates."
            ),
            Self::Udisks => crate::tr!(
                "hwsight can show your drives' temperature, hours of use, and whether they're failing."
            ),
        }
    }
}

/// Something the user could install to see more.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MissingTool {
    pub name: String,
    pub benefit: String,
    /// The command that installs it, when the distro's package manager is known.
    pub command: Option<String>,
    /// hwsight can install it for the user (after they confirm and give their password).
    pub installable: bool,
}

impl MissingTool {
    pub fn message(&self) -> String {
        match &self.command {
            Some(command) => crate::tr!(
                "{} To turn this on, install {}: {}",
                self.benefit,
                self.name,
                command
            ),
            None => crate::tr!(
                "{} To turn this on, install {} with your system's package manager.",
                self.benefit,
                self.name
            ),
        }
    }
}

const ALL_TOOLS: [Tool; 4] = [Tool::Iw, Tool::NetworkManager, Tool::Fwupd, Tool::Udisks];

/// Install the named tools (the `name` of a `MissingTool`) with the distro's package
/// manager through pkexec, which asks the user for their password in the desktop's own
/// window: hwsight never sees it. Only tools in hwsight's own table are accepted.
pub fn install_tools(names: &[String], distro: &Distro) -> Result<String, String> {
    let mut packages = Vec::new();
    for name in names {
        let tool = ALL_TOOLS
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| crate::tr!("{} isn't a tool hwsight knows how to install.", name))?;
        packages.push(tool.package(distro.family));
    }
    if packages.is_empty() {
        return Err(crate::tr!("Nothing to install."));
    }
    let argv = distro
        .family
        .install_argv(&packages, distro.immutable)
        .ok_or_else(|| {
            crate::tr!("hwsight can't install software on this system. Use your system's package manager instead.")
        })?;
    let output = std::process::Command::new("pkexec")
        .args(&argv)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                crate::tr!("The pkexec program (polkit) isn't installed, so hwsight can't ask for your password.")
            } else {
                crate::tr!("Couldn't start the installer: {}", e)
            }
        })?;
    match output.status.code() {
        Some(0) => Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned()),
        // pkexec: 126 means the password window was dismissed, 127 that it was refused.
        Some(126) => Err(crate::tr!("Cancelled: the password wasn't entered.")),
        Some(127) => Err(crate::tr!("Not allowed: the password was refused.")),
        _ => {
            let text = String::from_utf8_lossy(&output.stderr);
            let last = text
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            Err(crate::tr!("The installer failed: {}", last.trim()))
        }
    }
}

fn on_path(program: &str, dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|dir| dir.join(program).is_file())
}

/// Folders on `$PATH`, plus the sbin folders (a normal user's `$PATH` often leaves them
/// out although the programs are installed).
pub(crate) fn path_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(["/usr/sbin", "/sbin"].map(PathBuf::from));
    dirs
}

/// Which of the wanted tools are missing. A tool is only wanted when the computer has
/// something it helps with, so nobody is told about a Wi-Fi tool on a desktop with no Wi-Fi.
pub(crate) fn missing(wanted: &[Tool], dirs: &[PathBuf], distro: &Distro) -> Vec<MissingTool> {
    wanted
        .iter()
        .filter(|tool| !on_path(tool.program(), dirs))
        .map(|tool| MissingTool {
            name: tool.name().to_owned(),
            benefit: tool.benefit(),
            command: distro
                .family
                .install_command(tool.package(distro.family), distro.immutable),
            installable: distro
                .family
                .install_argv(&[tool.package(distro.family)], distro.immutable)
                .is_some(),
        })
        .collect()
}

/// The tools worth having on this computer.
pub(crate) fn wanted(
    devices: &[Device],
    proc_net_wireless: &Path,
    check_services: bool,
) -> Vec<Tool> {
    let mut wanted = Vec::new();
    if !check_services {
        return wanted;
    }
    let has_wifi = std::fs::read_to_string(proc_net_wireless)
        .is_ok_and(|text| text.lines().skip(2).any(|l| !l.trim().is_empty()));
    if has_wifi {
        wanted.push(Tool::NetworkManager);
        wanted.push(Tool::Iw);
    }
    wanted.push(Tool::Fwupd);
    if devices.iter().any(|d| d.category == Category::Drives) {
        wanted.push(Tool::Udisks);
    }
    wanted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::in_english;

    fn distro(family: Family, immutable: bool) -> Distro {
        Distro {
            id: "x".into(),
            name: "X".into(),
            family,
            immutable,
            base_release: None,
        }
    }

    #[test]
    fn a_missing_tool_names_the_package_command_for_the_distro() {
        let dirs = vec![PathBuf::from("/nonexistent")];
        let found = in_english(|| {
            missing(
                &[Tool::Iw, Tool::NetworkManager],
                &dirs,
                &distro(Family::Debian, false),
            )
        });
        assert_eq!(found[0].command.as_deref(), Some("sudo apt install iw"));
        assert_eq!(
            found[1].command.as_deref(),
            Some("sudo apt install network-manager")
        );
        assert!(
            found[0]
                .message()
                .starts_with("hwsight can show which Wi-Fi generation")
        );
        assert!(
            found[0]
                .message()
                .ends_with("install iw: sudo apt install iw")
        );
        let arch =
            in_english(|| missing(&[Tool::NetworkManager], &dirs, &distro(Family::Arch, false)));
        assert_eq!(
            arch[0].command.as_deref(),
            Some("sudo pacman -S networkmanager")
        );
    }

    #[test]
    fn install_commands_are_built_per_distro_and_refused_where_they_cannot_work() {
        let args = |family, immutable| family_argv(family, immutable);
        assert_eq!(
            args(Family::Arch, false).expect("argv"),
            ["pacman", "-S", "--noconfirm", "--needed", "iw", "fwupd"]
        );
        assert_eq!(
            args(Family::Ubuntu, false).expect("argv"),
            ["apt-get", "install", "-y", "iw", "fwupd"]
        );
        assert!(args(Family::Fedora, true).is_none(), "read-only system");
        assert!(args(Family::Unknown, false).is_none());
    }

    fn family_argv(family: Family, immutable: bool) -> Option<Vec<String>> {
        family.install_argv(&["iw", "fwupd"], immutable)
    }

    #[test]
    fn only_tools_from_the_table_can_be_installed() {
        let d = distro(Family::Arch, false);
        let err = install_tools(&["bash; rm -rf /".to_owned()], &d).expect_err("refused");
        assert!(err.contains("isn't a tool hwsight knows"));
        assert!(install_tools(&[], &d).is_err());
        assert!(install_tools(&["iw".to_owned()], &distro(Family::Arch, true)).is_err());
    }

    #[test]
    fn an_immutable_system_gets_no_command_to_run() {
        let dirs = vec![PathBuf::from("/nonexistent")];
        let found = in_english(|| missing(&[Tool::Iw], &dirs, &distro(Family::Arch, true)));
        assert_eq!(found[0].command, None);
        assert!(found[0].message().contains("package manager"));
    }

    #[test]
    fn an_installed_tool_is_not_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("iw"), "").expect("write");
        let found = missing(
            &[Tool::Iw, Tool::Fwupd],
            &[dir.path().to_path_buf()],
            &distro(Family::Arch, false),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "fwupd");
    }

    #[test]
    fn nothing_is_wanted_without_the_hardware_or_in_tests() {
        let none = Path::new("/nonexistent");
        assert!(wanted(&[], none, false).is_empty());
        assert_eq!(
            wanted(&[], none, true),
            [Tool::Fwupd],
            "no Wi-Fi, no drives"
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("wireless");
        std::fs::write(&file, "Inter-|\n face |\n wlan0: 0000 51. -59. -256 0\n").expect("write");
        assert_eq!(
            wanted(&[], &file, true),
            [Tool::NetworkManager, Tool::Iw, Tool::Fwupd]
        );
    }
}
