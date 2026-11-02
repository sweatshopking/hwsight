//! hwsight's own knowledge about devices, loaded from an updatable `rules.toml`.
//!
//! A copy is built into the program. A newer valid copy in a data folder replaces it as a
//! whole (newer data files are always complete supersets, so no merging is needed).

use serde::Deserialize;

use crate::data::{self, DataDirs, DataFile, DataProblem, DataSource};
use crate::device::Bus;

#[derive(Debug, Clone)]
pub struct Rules {
    pub version: String,
    pub source: DataSource,
    set: RuleSet,
}

/// Which devices a rule applies to. Every given field must match.
#[derive(Debug, Clone)]
struct Matcher {
    /// Class codes mean different things on different buses (09 is input on PCI but a
    /// hub on USB), so every rule belongs to one bus. Defaults to PCI.
    bus: Bus,
    class: Option<u8>,
    subclasses: Option<Vec<u8>>,
    prog_ifs: Option<Vec<u8>>,
    vendor: Option<u16>,
    device: Option<u16>,
}

impl Matcher {
    /// For USB, `class_code` is the interface's class, subclass, and protocol.
    fn matches(&self, bus: Bus, class_code: u32, vendor: u16, device: u16) -> bool {
        if bus != self.bus {
            return false;
        }
        let class = (class_code >> 16) as u8;
        let subclass = (class_code >> 8) as u8;
        let prog_if = class_code as u8;
        self.class.is_none_or(|c| c == class)
            && self
                .subclasses
                .as_ref()
                .is_none_or(|s| s.contains(&subclass))
            && self.prog_ifs.as_ref().is_none_or(|p| p.contains(&prog_if))
            && self.vendor.is_none_or(|v| v == vendor)
            && self.device.is_none_or(|d| d == device)
    }
}

#[derive(Debug, Clone)]
struct NoDriverRule {
    matcher: Matcher,
    reason: String,
}

/// What a working driver should create for a device, e.g. a network interface.
#[derive(Debug, Clone)]
struct InterfaceRule {
    matcher: Matcher,
    /// Folder name prefix under the device in sysfs, e.g. `net`, `drm`, `ata`.
    interface: String,
    /// Plain-English name for it, e.g. "network connection".
    what: String,
}

/// What a working driver creates, when it's missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedInterface<'a> {
    pub interface: &'a str,
    pub what: &'a str,
}

/// Raw file layout. Hex IDs are strings so the file stays readable.
#[derive(Debug, Deserialize)]
struct RulesFile {
    version: String,
    #[serde(default)]
    no_driver_needed: Vec<RawNoDriverRule>,
    #[serde(default)]
    expect_interface: Vec<RawInterfaceRule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNoDriverRule {
    bus: Option<String>,
    class: Option<String>,
    subclasses: Option<Vec<String>>,
    prog_ifs: Option<Vec<String>>,
    vendor: Option<String>,
    device: Option<String>,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInterfaceRule {
    bus: Option<String>,
    class: Option<String>,
    subclasses: Option<Vec<String>>,
    prog_ifs: Option<Vec<String>>,
    vendor: Option<String>,
    device: Option<String>,
    interface: String,
    what: String,
}

/// The parsed rule lists, without version info.
#[derive(Debug, Clone)]
pub(crate) struct RuleSet {
    no_driver_needed: Vec<NoDriverRule>,
    expect_interface: Vec<InterfaceRule>,
}

impl DataFile for RuleSet {
    const FILE_NAME: &'static str = "rules.toml";
    const SCHEMA: u32 = 1;
    const EMBEDDED: &'static str = include_str!("../data/rules.toml");

    fn parse_body(text: &str) -> Result<(String, Self), String> {
        let raw: RulesFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let no_driver_needed = raw
            .no_driver_needed
            .into_iter()
            .map(|r| {
                Ok(NoDriverRule {
                    matcher: compile_matcher(
                        r.bus,
                        r.class,
                        r.subclasses,
                        r.prog_ifs,
                        r.vendor,
                        r.device,
                    )?,
                    reason: r.reason,
                })
            })
            .collect::<Result<_, String>>()?;
        let expect_interface = raw
            .expect_interface
            .into_iter()
            .map(|r| {
                let valid = !r.interface.is_empty()
                    && r.interface
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_');
                if !valid {
                    return Err(format!("bad interface name `{}`", r.interface));
                }
                Ok(InterfaceRule {
                    matcher: compile_matcher(
                        r.bus,
                        r.class,
                        r.subclasses,
                        r.prog_ifs,
                        r.vendor,
                        r.device,
                    )?,
                    interface: r.interface,
                    what: r.what,
                })
            })
            .collect::<Result<_, String>>()?;
        Ok((
            raw.version,
            Self {
                no_driver_needed,
                expect_interface,
            },
        ))
    }

    fn empty() -> Self {
        Self {
            no_driver_needed: Vec::new(),
            expect_interface: Vec::new(),
        }
    }
}

impl Rules {
    /// The rules built into the program.
    pub fn embedded() -> Self {
        Self::from_loaded(data::embedded::<RuleSet>())
    }

    /// The newest valid rules from the data folders or the built-in copy, plus any
    /// files that were skipped and why.
    pub fn load(dirs: &DataDirs) -> (Self, Vec<DataProblem>) {
        let (loaded, problems) = data::load_newest::<RuleSet>(dirs);
        (Self::from_loaded(loaded), problems)
    }

    fn from_loaded(loaded: data::Loaded<RuleSet>) -> Self {
        Self {
            version: loaded.version,
            source: loaded.source,
            set: loaded.data,
        }
    }

    /// If this device normally needs no driver, the plain-English reason why.
    pub fn no_driver_needed(
        &self,
        bus: Bus,
        class_code: u32,
        vendor: u16,
        device: u16,
    ) -> Option<&str> {
        self.set
            .no_driver_needed
            .iter()
            .find(|r| r.matcher.matches(bus, class_code, vendor, device))
            .map(|r| r.reason.as_str())
    }

    /// What a working driver should have created for this device, if we know.
    pub fn expected_interface(
        &self,
        class_code: u32,
        vendor: u16,
        device: u16,
    ) -> Option<ExpectedInterface<'_>> {
        self.set
            .expect_interface
            .iter()
            .find(|r| r.matcher.matches(Bus::Pci, class_code, vendor, device))
            .map(|r| ExpectedInterface {
                interface: &r.interface,
                what: &r.what,
            })
    }
}

/// A rule with a bad ID rejects the whole file, so a broken download falls back to the
/// built-in rules instead of half-working.
fn compile_matcher(
    bus: Option<String>,
    class: Option<String>,
    subclasses: Option<Vec<String>>,
    prog_ifs: Option<Vec<String>>,
    vendor: Option<String>,
    device: Option<String>,
) -> Result<Matcher, String> {
    let bus = match bus.as_deref() {
        None | Some("pci") => Bus::Pci,
        Some("usb") => Bus::Usb,
        Some(other) => return Err(format!("unknown bus `{other}`")),
    };
    let m = Matcher {
        bus,
        class: class.as_deref().map(hex_u8).transpose()?,
        subclasses: subclasses
            .map(|list| list.iter().map(|h| hex_u8(h)).collect())
            .transpose()?,
        prog_ifs: prog_ifs
            .map(|list| list.iter().map(|h| hex_u8(h)).collect())
            .transpose()?,
        vendor: vendor.as_deref().map(hex_u16).transpose()?,
        device: device.as_deref().map(hex_u16).transpose()?,
    };
    if m.device.is_some() && m.vendor.is_none() {
        return Err("a rule with `device` must also have `vendor`".into());
    }
    if m.prog_ifs.is_some() && m.subclasses.is_none() {
        return Err("a rule with `prog_ifs` must also have `subclasses`".into());
    }
    if m.subclasses.is_some() && m.class.is_none() {
        return Err("a rule with `subclasses` must also have `class`".into());
    }
    if m.class.is_none() && m.vendor.is_none() {
        return Err("a rule must match on `class` or `vendor`, not everything".into());
    }
    Ok(m)
}

fn hex_u8(s: &str) -> Result<u8, String> {
    if s.len() != 2 {
        return Err(format!("`{s}` should be 2 hex digits"));
    }
    u8::from_str_radix(s, 16).map_err(|_| format!("`{s}` isn't hex"))
}

fn hex_u16(s: &str) -> Result<u16, String> {
    if s.len() != 4 {
        return Err(format!("`{s}` should be 4 hex digits"));
    }
    u16::from_str_radix(s, 16).map_err(|_| format!("`{s}` isn't hex"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn embedded_rules_are_valid() {
        assert!(
            data::parse_data::<RuleSet>(RuleSet::EMBEDDED).is_ok(),
            "embedded rules parse"
        );
        let rules = Rules::embedded();
        assert!(!rules.version.is_empty());
        // Host bridge: no driver needed. VGA controller: needs one.
        assert!(
            rules
                .no_driver_needed(Bus::Pci, 0x060000, 0x1022, 0x14d8)
                .is_some()
        );
        assert!(
            rules
                .no_driver_needed(Bus::Pci, 0x080600, 0x1022, 0x149e)
                .is_some()
        );
        assert!(
            rules
                .no_driver_needed(Bus::Pci, 0x030000, 0x10de, 0x2c05)
                .is_none()
        );
        // SMBus (0c05) carries sensors, so it does need a driver.
        assert!(
            rules
                .no_driver_needed(Bus::Pci, 0x0c0500, 0x1022, 0x790b)
                .is_none()
        );
    }

    fn write(dir: &Path, text: &str) {
        fs::write(dir.join("rules.toml"), text).expect("write");
    }

    #[test]
    fn newer_download_replaces_embedded() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "schema = 1\nversion = \"2099.01.01\"\n[[no_driver_needed]]\nclass = \"03\"\nreason = \"test\"\n",
        );
        let (rules, problems) = Rules::load(&DataDirs::only(tmp.path()));
        assert!(problems.is_empty());
        assert_eq!(rules.version, "2099.01.01");
        assert!(matches!(rules.source, DataSource::File(_)));
        assert_eq!(
            rules.no_driver_needed(Bus::Pci, 0x030000, 0, 0),
            Some("test")
        );
    }

    #[test]
    fn older_download_is_ignored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "schema = 1\nversion = \"2000.01.01\"\n");
        let (rules, _) = Rules::load(&DataDirs::only(tmp.path()));
        assert_eq!(rules.source, DataSource::Embedded);
    }

    #[test]
    fn newer_schema_is_reported_not_used() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "schema = 2\nversion = \"2099.01.01\"\nnew_shape = { x = 1 }\n",
        );
        let (rules, problems) = Rules::load(&DataDirs::only(tmp.path()));
        assert_eq!(rules.source, DataSource::Embedded);
        assert!(matches!(
            problems.as_slice(),
            [DataProblem::NewerSchema { schema: 2, .. }]
        ));
    }

    #[test]
    fn broken_file_falls_back_to_embedded() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "schema = 1\nversion = \"2099.01.01\"\n[[no_driver_needed]]\nclass = \"zz\"\nreason = \"x\"\n",
        );
        let (rules, problems) = Rules::load(&DataDirs::only(tmp.path()));
        assert_eq!(rules.source, DataSource::Embedded);
        assert!(matches!(problems.as_slice(), [DataProblem::Invalid { .. }]));
    }

    #[test]
    fn match_everything_rule_is_rejected() {
        assert!(compile_matcher(None, None, None, None, None, None).is_err());
        assert!(compile_matcher(None, None, None, None, None, Some("1234".into())).is_err());
        assert!(
            compile_matcher(
                None,
                Some("0c".into()),
                None,
                Some(vec!["30".into()]),
                None,
                None
            )
            .is_err()
        );
        assert!(compile_matcher(None, Some("02".into()), None, None, None, None).is_ok());
        assert!(
            compile_matcher(
                Some("usb".into()),
                Some("ff".into()),
                None,
                None,
                None,
                None
            )
            .is_ok()
        );
        assert!(
            compile_matcher(
                Some("isa".into()),
                Some("02".into()),
                None,
                None,
                None,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn expected_interfaces_from_embedded_rules() {
        let rules = Rules::embedded();
        let iface = |class| {
            rules
                .expected_interface(class, 0x1234, 0x5678)
                .map(|e| e.interface)
        };
        assert_eq!(iface(0x020000), Some("net")); // Ethernet
        assert_eq!(iface(0x028000), Some("net")); // Wi-Fi
        assert_eq!(iface(0x030000), Some("drm"));
        assert_eq!(iface(0x040300), Some("sound"));
        assert_eq!(iface(0x010802), Some("nvme"));
        assert_eq!(iface(0x010601), Some("ata"));
        assert_eq!(iface(0x0c0330), Some("usb"));
        assert_eq!(
            iface(0x0c0340),
            None,
            "USB4/Thunderbolt hosts don't create USB buses"
        );
        assert_eq!(iface(0x060000), None);
    }

    #[test]
    fn bad_interface_names_reject_the_file() {
        let text = "schema = 1\nversion = \"2099.01.01\"\n[[expect_interface]]\nclass = \"02\"\ninterface = \"../x\"\nwhat = \"x\"\n";
        assert!(data::parse_data::<RuleSet>(text).is_err());
    }
}
