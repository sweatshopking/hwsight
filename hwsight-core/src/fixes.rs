//! "How to fix": plain-English steps (and safe commands) for each problem, from the
//! updatable `fixes.toml`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::data::{self, DataDirs, DataFile, DataProblem, DataSource};
use crate::data_text::DataText;
use crate::device::{Bus, Device, Status};
use crate::kernel_log::LogIssue;

/// One step towards fixing a problem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fix {
    pub text: String,
    /// A command the user can copy (and a later version may run after asking).
    pub command: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Fixes {
    pub version: String,
    pub source: DataSource,
    table: Table,
    /// Translations for the user's language (English by default).
    text: DataText,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Table {
    fixes: HashMap<String, Template>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Template {
    text: String,
    command: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawFile {
    version: String,
    #[serde(default)]
    fix: HashMap<String, Template>,
}

impl DataFile for Table {
    const FILE_NAME: &'static str = "fixes.toml";
    const SCHEMA: u32 = 1;
    const EMBEDDED: &'static str = include_str!("../data/fixes.toml");

    fn parse_body(text: &str) -> Result<(String, Self), String> {
        let raw: RawFile = toml::from_str(text).map_err(|e| e.to_string())?;
        for (name, t) in &raw.fix {
            if t.text.trim().is_empty() {
                return Err(format!("fix.{name} has no text"));
            }
            // A file path could contain anything, so it must never reach a command.
            if t.command.as_deref().is_some_and(|c| c.contains("{file}")) {
                return Err(format!("fix.{name}: {{file}} isn't allowed in a command"));
            }
        }
        Ok((raw.version, Self { fixes: raw.fix }))
    }

    fn empty() -> Self {
        Self::default()
    }
}

/// Values that may be put into a fix's text and command.
#[derive(Default)]
struct Values<'a> {
    module: Option<&'a str>,
    package: Option<&'a str>,
    install: Option<&'a str>,
    file: Option<&'a str>,
    hardware_id: Option<&'a str>,
    homepage: Option<&'a str>,
}

impl Fixes {
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

    /// Every English step in the built-in file, for the translation checks.
    #[cfg(test)]
    pub(crate) fn english_texts() -> Vec<String> {
        Self::embedded()
            .table
            .fixes
            .into_values()
            .map(|t| t.text)
            .collect()
    }

    /// Write steps in the language of `text`; English where it has no translation.
    pub fn with_text(mut self, text: DataText) -> Self {
        self.text = text;
        self
    }

    /// Steps to fix this device's problem: specific ones from the kernel log first, then
    /// the one for its status. Empty for devices without a problem.
    pub fn suggest(&self, device: &Device) -> Vec<Fix> {
        if !device.status.is_problem() {
            return Vec::new();
        }
        let id = Some(device.hardware_id.as_str());
        let mut out: Vec<Fix> = Vec::new();
        let mut push = |name: &str, values: Values| {
            if let Some(fix) = self.render(name, &values)
                && !out.contains(&fix)
            {
                out.push(fix);
            }
        };

        for issue in &device.log_issues {
            match issue {
                LogIssue::MissingFirmware {
                    installed_now: true,
                    ..
                } => {
                    push("firmware_installed_since_boot", Values::default());
                }
                LogIssue::MissingFirmware {
                    package: Some(p),
                    install_command,
                    ..
                } => push(
                    "missing_firmware",
                    Values {
                        package: Some(p),
                        install: install_command.as_deref(),
                        ..Values::default()
                    },
                ),
                LogIssue::MissingFirmware { file, .. } => push(
                    "missing_firmware_unknown_package",
                    Values {
                        file: Some(file),
                        ..Values::default()
                    },
                ),
                LogIssue::DriverFailed { error, .. } => {
                    let name = match error.abs() {
                        110 => "driver_timed_out",
                        5 | 71 => "driver_io_error",
                        16 => "driver_busy",
                        // Firmware errors (2) are explained by their own log line.
                        2 => continue,
                        _ => "driver_failed",
                    };
                    push(
                        name,
                        Values {
                            hardware_id: id,
                            ..Values::default()
                        },
                    );
                }
                LogIssue::UsbConnectFailed { .. } | LogIssue::UsbOverCurrent => {}
            }
        }

        let first = |want: &dyn Fn(&crate::DriverCandidate) -> bool| {
            device.driver_candidates.iter().find(|c| want(c))
        };
        match device.status {
            Status::DriverAvailableNotLoaded => {
                if let Some(c) = first(&|c| !c.loaded && c.blocked_by.is_none()) {
                    push(
                        "driver_not_loaded",
                        Values {
                            module: Some(&c.module),
                            ..Values::default()
                        },
                    );
                }
            }
            Status::DriverBlocked
                if device.bus == Bus::Usb && device.driver_candidates.is_empty() =>
            {
                push("usb_blocked", Values::default());
            }
            Status::DriverBlocked => {
                for c in &device.driver_candidates {
                    let Some(by) = c.blocked_by.as_deref() else {
                        continue;
                    };
                    let module = Some(c.module.as_str());
                    if by == "kernel command line" {
                        push(
                            "driver_blocked_on_kernel_command_line",
                            Values {
                                module,
                                ..Values::default()
                            },
                        );
                    } else {
                        push(
                            "driver_blocked_by_file",
                            Values {
                                module,
                                file: Some(by),
                                ..Values::default()
                            },
                        );
                    }
                }
            }
            Status::DriverNotAttached => {
                if let Some(c) = first(&|c| c.loaded) {
                    push(
                        "driver_not_attached",
                        Values {
                            module: Some(&c.module),
                            ..Values::default()
                        },
                    );
                }
            }
            Status::DriverNotReady => push("driver_not_ready", Values::default()),
            Status::DriveFailing => push("drive_failing", Values::default()),
            Status::NoDriver => push(
                "no_driver",
                Values {
                    hardware_id: id,
                    ..Values::default()
                },
            ),
            Status::Working | Status::NoDriverNeeded | Status::ReservedForVirtualMachine => {}
        }
        out
    }

    /// The step that offers a third-party driver, led by what the driver is for. `None` if
    /// the data has no matching template.
    pub fn driver_fix(
        &self,
        what: &str,
        source: &crate::drivers::Source,
        homepage: &str,
        install_command: Option<&str>,
    ) -> Option<Fix> {
        use crate::drivers::Source;
        let (name, package) = match source {
            Source::Repo(p) => ("driver_package", Some(*p)),
            Source::Aur(p) => ("driver_aur", Some(*p)),
            Source::ByHand => ("driver_by_hand", None),
        };
        let mut fix = self.render(
            name,
            &Values {
                package,
                install: install_command,
                homepage: Some(homepage),
                ..Values::default()
            },
        )?;
        fix.text = format!("{what} {}", fix.text);
        Some(fix)
    }

    /// Fill in a template. `None` if the template is missing or a value it needs is
    /// missing or unsafe, so a step is never shown half-filled or with a risky command.
    fn render(&self, name: &str, v: &Values) -> Option<Fix> {
        let t = self.table.fixes.get(name)?;
        // The translated step, or the English one if the translation can't be filled in.
        let text = fill(self.text.get(&t.text), v, false).or_else(|| fill(&t.text, v, false))?;
        let command = match &t.command {
            Some(c) => Some(fill(c, v, true)?),
            None => None,
        };
        Some(Fix { text, command })
    }
}

fn fill(template: &str, v: &Values, for_command: bool) -> Option<String> {
    let mut out = template.to_owned();
    // (placeholder, its value, the check a value must pass to go into a command)
    type Slot<'a> = (&'a str, Option<&'a str>, fn(&str) -> bool);
    let pairs: [Slot; 6] = [
        ("{module}", v.module, is_name),
        ("{package}", v.package, is_name),
        ("{install}", v.install, is_install_command),
        ("{file}", v.file, |_| true),
        ("{hardware_id}", v.hardware_id, is_hardware_id),
        // A link is text for the user to read, never part of a command.
        ("{homepage}", v.homepage, |_| false),
    ];
    for (key, value, safe) in pairs {
        if !out.contains(key) {
            continue;
        }
        let value = value?;
        if for_command && !safe(value) {
            return None;
        }
        out = out.replace(key, value);
    }
    // An unknown placeholder means the data and the app disagree: don't show it.
    (!out.contains('{')).then_some(out)
}

/// Driver and package names: letters, digits, and - _ . + only.
fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
        && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
}

/// Install commands come from hwsight itself (`Family::install_command`) with a checked
/// package name; this re-checks the shape so nothing else slips through.
fn is_install_command(s: &str) -> bool {
    s.split(' ').all(|w| is_name(w) || w == "-S")
}

fn is_hardware_id(s: &str) -> bool {
    s.len() == 9
        && s.as_bytes()[4] == b':'
        && s.chars()
            .filter(|c| *c != ':')
            .all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Category;
    use crate::modules::DriverCandidate;

    fn device(status: Status) -> Device {
        Device {
            bus: Bus::Pci,
            address: "0000:02:00.0".into(),
            sysfs_path: "/sys/devices/x".into(),
            display_name: "Wi-Fi card".into(),
            vendor_name: None,
            product_name: None,
            model_name: None,
            class_name: None,
            category: Category::Network,
            hardware_id: "8086:2725".into(),
            unrecognized: false,
            modalias: None,
            driver: None,
            driver_candidates: Vec::new(),
            status,
            log_issues: Vec::new(),
            missing_interface: None,
            note: None,
            description: None,
            details: vec![],
            software: vec![],
            class_codes: vec![],
            fixes: Vec::new(),
        }
    }

    fn candidate(module: &str, loaded: bool, blocked_by: Option<&str>) -> DriverCandidate {
        DriverCandidate {
            module: module.into(),
            builtin: false,
            loaded,
            blocked_by: blocked_by.map(str::to_owned),
        }
    }

    #[test]
    fn embedded_file_is_valid_and_complete() {
        assert!(data::parse_data::<Table>(Table::EMBEDDED).is_ok());
        let f = Fixes::embedded();
        for name in [
            "missing_firmware",
            "missing_firmware_unknown_package",
            "firmware_installed_since_boot",
            "driver_timed_out",
            "driver_io_error",
            "driver_busy",
            "driver_failed",
            "driver_not_loaded",
            "driver_blocked_by_file",
            "driver_blocked_on_kernel_command_line",
            "driver_not_attached",
            "driver_not_ready",
            "no_driver",
            "driver_package",
            "driver_aur",
            "driver_by_hand",
            "usb_blocked",
        ] {
            assert!(
                f.table.fixes.contains_key(name),
                "fixes.toml has no fix.{name}"
            );
        }
    }

    #[test]
    fn steps_are_translated_and_filled_in() {
        let english = "A driver for this device ({module}) is installed but isn't running. Start it now with this command. It normally starts by itself after a restart.";
        let f = Fixes::embedded();
        assert_eq!(f.table.fixes["driver_not_loaded"].text, english);
        let text = crate::data_text::DataText::from_toml(&format!(
            "schema = 1\nversion = \"2026.11.06\"\nlanguage = \"de\"\n[text]\n{english:?} = \"Starte den Treiber {{module}}.\"\n"
        ));
        let f = f.with_text(text);
        let mut d = device(Status::DriverAvailableNotLoaded);
        d.driver_candidates = vec![candidate("iwlwifi", false, None)];
        let fixes = f.suggest(&d);
        assert_eq!(fixes[0].text, "Starte den Treiber iwlwifi.");
        assert_eq!(fixes[0].command.as_deref(), Some("sudo modprobe iwlwifi"));
    }

    #[test]
    fn working_devices_get_no_steps() {
        let f = Fixes::embedded();
        for s in [
            Status::Working,
            Status::NoDriverNeeded,
            Status::ReservedForVirtualMachine,
        ] {
            assert!(f.suggest(&device(s)).is_empty());
        }
    }

    #[test]
    fn driver_not_loaded_gives_a_command() {
        let f = Fixes::embedded();
        let mut d = device(Status::DriverAvailableNotLoaded);
        d.driver_candidates = vec![
            candidate("blocked_one", false, Some("x")),
            candidate("iwlwifi", false, None),
        ];
        let fixes = f.suggest(&d);
        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0].command.as_deref(), Some("sudo modprobe iwlwifi"));
        assert!(fixes[0].text.contains("iwlwifi"));
    }

    #[test]
    fn blocked_drivers_name_the_setting() {
        let f = Fixes::embedded();
        let mut d = device(Status::DriverBlocked);
        d.driver_candidates = vec![
            candidate(
                "nouveau",
                false,
                Some("/usr/lib/modprobe.d/nvidia-utils.conf"),
            ),
            candidate("nova_core", false, Some("kernel command line")),
        ];
        let fixes = f.suggest(&d);
        assert_eq!(fixes.len(), 2);
        assert!(
            fixes[0]
                .text
                .contains("/usr/lib/modprobe.d/nvidia-utils.conf")
        );
        assert!(fixes[1].text.contains("boot settings"));
        assert!(
            fixes.iter().all(|f| f.command.is_none()),
            "no commands that edit settings"
        );
    }

    #[test]
    fn log_issues_come_first_and_firmware_gets_the_install_command() {
        let f = Fixes::embedded();
        let mut d = device(Status::DriverNotAttached);
        d.driver_candidates = vec![candidate("r8169", true, None)];
        d.log_issues = vec![
            LogIssue::MissingFirmware {
                file: "rtl_nic/rtl8125b-2.fw".into(),
                error: -2,
                installed_now: false,
                package: Some("linux-firmware-realtek".into()),
                install_command: Some("sudo pacman -S linux-firmware-realtek".into()),
            },
            LogIssue::DriverFailed {
                driver: "r8169".into(),
                error: -2,
            },
        ];
        let fixes = f.suggest(&d);
        assert_eq!(fixes.len(), 2, "{fixes:?}");
        assert_eq!(
            fixes[0].command.as_deref(),
            Some("sudo pacman -S linux-firmware-realtek")
        );
        assert!(fixes[1].text.contains("r8169"));
    }

    #[test]
    fn timeouts_and_unknown_failures() {
        let f = Fixes::embedded();
        let mut d = device(Status::DriverNotAttached);
        d.log_issues = vec![LogIssue::DriverFailed {
            driver: "amdgpu".into(),
            error: -110,
        }];
        assert!(f.suggest(&d)[0].text.contains("didn't answer"));
        d.log_issues = vec![LogIssue::DriverFailed {
            driver: "amdgpu".into(),
            error: -99,
        }];
        assert!(f.suggest(&d)[0].text.contains("8086:2725"));
    }

    #[test]
    fn no_driver_and_usb_blocked() {
        let f = Fixes::embedded();
        assert!(
            f.suggest(&device(Status::NoDriver))[0]
                .text
                .contains("8086:2725")
        );
        let mut usb = device(Status::DriverBlocked);
        usb.bus = Bus::Usb;
        assert!(f.suggest(&usb)[0].text.contains("USB security"));
    }

    #[test]
    fn a_failing_drive_is_told_to_back_up_with_no_command() {
        let f = Fixes::embedded();
        let steps = f.suggest(&device(Status::DriveFailing));
        assert_eq!(steps.len(), 1);
        assert!(
            steps[0]
                .text
                .starts_with("Back up your important files now")
        );
        assert_eq!(steps[0].command, None, "nothing is run on a failing drive");
        assert!(Status::DriveFailing.is_problem());
    }

    #[test]
    fn unsafe_values_never_reach_a_command() {
        let f = Fixes::embedded();
        let mut d = device(Status::DriverAvailableNotLoaded);
        d.driver_candidates = vec![candidate("evil; rm -rf ~", false, None)];
        assert!(
            f.suggest(&d).is_empty(),
            "the step is dropped, not shown with a bad command"
        );
    }

    #[test]
    fn file_paths_are_rejected_in_commands() {
        let text = "schema = 1\nversion = \"2099.01.01\"\n[fix.x]\ntext = \"t\"\ncommand = \"cat {file}\"\n";
        assert!(data::parse_data::<Table>(text).is_err());
    }
}
