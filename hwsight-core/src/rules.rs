//! hwsight's own knowledge about devices, loaded from an updatable `rules.toml`.
//!
//! A copy is built into the program. A newer valid copy in a data folder replaces it as a
//! whole (newer data files are always complete supersets, so no merging is needed).

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::data::DataDirs;

/// The rules file format this version of hwsight understands.
pub const RULES_SCHEMA: u32 = 1;

const EMBEDDED_RULES: &str = include_str!("../data/rules.toml");

/// Where the rules in use came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataSource {
    Embedded,
    File(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Rules {
    pub version: String,
    pub source: DataSource,
    no_driver_needed: Vec<NoDriverRule>,
}

#[derive(Debug, Clone)]
struct NoDriverRule {
    class: Option<u8>,
    subclasses: Option<Vec<u8>>,
    vendor: Option<u16>,
    device: Option<u16>,
    reason: String,
}

/// Raw file layout. Hex IDs are strings so the file stays readable.
#[derive(Debug, Deserialize)]
struct RulesFile {
    schema: u32,
    version: String,
    #[serde(default)]
    no_driver_needed: Vec<RawNoDriverRule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNoDriverRule {
    class: Option<String>,
    subclasses: Option<Vec<String>>,
    vendor: Option<String>,
    device: Option<String>,
    reason: String,
}

/// Why a rules file wasn't used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RulesProblem {
    /// Written for a newer hwsight; the user should update the app.
    NewerSchema { path: PathBuf, schema: u32 },
    /// Unreadable or broken.
    Invalid { path: PathBuf, error: String },
}

impl Rules {
    /// The rules built into the program.
    pub fn embedded() -> Self {
        match parse(EMBEDDED_RULES, DataSource::Embedded) {
            Ok(rules) => rules,
            // The embedded file is checked by a test, so this can't happen in a release.
            // Still, never panic: run with no rules rather than not at all.
            Err(_) => Self {
                version: String::new(),
                source: DataSource::Embedded,
                no_driver_needed: Vec::new(),
            },
        }
    }

    /// The newest valid rules from the data folders or the built-in copy, plus any
    /// files that were skipped and why.
    pub fn load(dirs: &DataDirs) -> (Self, Vec<RulesProblem>) {
        let mut best = Self::embedded();
        let mut problems = Vec::new();
        for path in dirs.hwsight_files("rules.toml") {
            match load_file(&path) {
                Ok(rules) if rules.version > best.version => best = rules,
                Ok(_) => {}
                Err(problem) => problems.push(problem),
            }
        }
        (best, problems)
    }

    /// If this device normally needs no driver, the plain-English reason why.
    pub fn no_driver_needed(&self, class_code: u32, vendor: u16, device: u16) -> Option<&str> {
        let class = (class_code >> 16) as u8;
        let subclass = (class_code >> 8) as u8;
        self.no_driver_needed
            .iter()
            .find(|r| {
                r.class.is_none_or(|c| c == class)
                    && r.subclasses.as_ref().is_none_or(|s| s.contains(&subclass))
                    && r.vendor.is_none_or(|v| v == vendor)
                    && r.device.is_none_or(|d| d == device)
            })
            .map(|r| r.reason.as_str())
    }
}

fn load_file(path: &Path) -> Result<Rules, RulesProblem> {
    let invalid = |error: String| RulesProblem::Invalid { path: path.to_path_buf(), error };
    let text = fs::read_to_string(path).map_err(|e| invalid(e.to_string()))?;
    parse(&text, DataSource::File(path.to_path_buf())).map_err(|e| match e {
        ParseError::NewerSchema(schema) => RulesProblem::NewerSchema { path: path.to_path_buf(), schema },
        ParseError::Invalid(error) => invalid(error),
    })
}

#[derive(Debug)]
enum ParseError {
    NewerSchema(u32),
    Invalid(String),
}

fn parse(text: &str, source: DataSource) -> Result<Rules, ParseError> {
    // Check the schema first, so a newer file is reported as "newer" even if its other
    // fields have changed shape.
    #[derive(Deserialize)]
    struct SchemaOnly {
        schema: u32,
    }
    let schema = toml::from_str::<SchemaOnly>(text)
        .map_err(|e| ParseError::Invalid(e.to_string()))?
        .schema;
    if schema > RULES_SCHEMA {
        return Err(ParseError::NewerSchema(schema));
    }
    if schema < RULES_SCHEMA {
        return Err(ParseError::Invalid(format!("old format (schema {schema})")));
    }

    let raw: RulesFile = toml::from_str(text).map_err(|e| ParseError::Invalid(e.to_string()))?;
    debug_assert_eq!(raw.schema, schema);
    if !is_date_version(&raw.version) {
        // Versions are compared as text to pick the newest, which only works for one format.
        return Err(ParseError::Invalid(format!(
            "version `{}` should look like 2026.11.02",
            raw.version
        )));
    }
    let no_driver_needed = raw
        .no_driver_needed
        .into_iter()
        .map(compile_rule)
        .collect::<Result<_, _>>()
        .map_err(ParseError::Invalid)?;
    Ok(Rules {
        version: raw.version,
        source,
        no_driver_needed,
    })
}

/// A rule with a bad ID rejects the whole file, so a broken download falls back to the
/// built-in rules instead of half-working.
fn compile_rule(raw: RawNoDriverRule) -> Result<NoDriverRule, String> {
    let rule = NoDriverRule {
        class: raw.class.as_deref().map(hex_u8).transpose()?,
        subclasses: raw
            .subclasses
            .map(|list| list.iter().map(|h| hex_u8(h)).collect())
            .transpose()?,
        vendor: raw.vendor.as_deref().map(hex_u16).transpose()?,
        device: raw.device.as_deref().map(hex_u16).transpose()?,
        reason: raw.reason,
    };
    if rule.device.is_some() && rule.vendor.is_none() {
        return Err("a rule with `device` must also have `vendor`".into());
    }
    if rule.subclasses.is_some() && rule.class.is_none() {
        return Err("a rule with `subclasses` must also have `class`".into());
    }
    if rule.class.is_none() && rule.vendor.is_none() {
        return Err("a rule must match on `class` or `vendor`, not everything".into());
    }
    Ok(rule)
}

/// `YYYY.MM.DD`, the format shared by every hwsight data file and `pci.ids`.
pub(crate) fn is_date_version(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 10
        && b[4] == b'.'
        && b[7] == b'.'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
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

    #[test]
    fn embedded_rules_are_valid() {
        let rules = parse(EMBEDDED_RULES, DataSource::Embedded).expect("embedded rules parse");
        assert!(!rules.version.is_empty());
        // Host bridge: no driver needed. VGA controller: needs one.
        assert!(rules.no_driver_needed(0x060000, 0x1022, 0x14d8).is_some());
        assert!(rules.no_driver_needed(0x080600, 0x1022, 0x149e).is_some());
        assert!(rules.no_driver_needed(0x030000, 0x10de, 0x2c05).is_none());
        // SMBus (0c05) carries sensors, so it does need a driver.
        assert!(rules.no_driver_needed(0x0c0500, 0x1022, 0x790b).is_none());
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
        assert_eq!(rules.no_driver_needed(0x030000, 0, 0), Some("test"));
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
        write(tmp.path(), "schema = 2\nversion = \"2099.01.01\"\nnew_shape = { x = 1 }\n");
        let (rules, problems) = Rules::load(&DataDirs::only(tmp.path()));
        assert_eq!(rules.source, DataSource::Embedded);
        assert!(matches!(problems.as_slice(), [RulesProblem::NewerSchema { schema: 2, .. }]));
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
        assert!(matches!(problems.as_slice(), [RulesProblem::Invalid { .. }]));
    }

    #[test]
    fn match_everything_rule_is_rejected() {
        let raw = RawNoDriverRule {
            class: None,
            subclasses: None,
            vendor: None,
            device: None,
            reason: "x".into(),
        };
        assert!(compile_rule(raw).is_err());
    }
}
