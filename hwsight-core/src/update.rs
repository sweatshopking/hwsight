//! Downloading newer data files, safely.
//!
//! A data release is a signed `manifest.toml` listing each file with its version, size,
//! and SHA-256 checksum. We only install a file when:
//!
//! 1. the manifest's ed25519 signature matches a key built into hwsight,
//! 2. the file is newer than what we already use,
//! 3. its size and checksum match the manifest, and
//! 4. it loads correctly (a file hwsight can't read is never installed).
//!
//! Files are written next to their final name and renamed into place, so a crash or
//! power cut never leaves a half-written data file.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::data::{self, DataDirs, is_date_version};

/// Public keys whose signatures we trust: the main release key, then a backup key kept
/// offline so releases can continue if the main key is ever lost or exposed. The private
/// keys never go in any repository.
pub const TRUSTED_KEYS: &[[u8; 32]] = &[
    // main (2026-11-02)
    [
        0xc4, 0xfd, 0x2f, 0x4b, 0x61, 0xf6, 0x5a, 0x5b, 0x13, 0x2a, 0x11, 0xa6, 0x07, 0xf7, 0xed,
        0x36, 0x39, 0x5e, 0xa8, 0x5f, 0x4d, 0x22, 0xef, 0xfa, 0x02, 0xb1, 0xf7, 0x56, 0xb7, 0x04,
        0xd2, 0x4c,
    ],
    // backup (2026-11-02)
    [
        0x6a, 0x91, 0x61, 0x5a, 0x5e, 0xac, 0xc3, 0x62, 0x36, 0x16, 0x69, 0x65, 0x6c, 0x7b, 0x62,
        0x31, 0xde, 0x8e, 0xa8, 0xea, 0x57, 0x8b, 0x05, 0x78, 0x73, 0x62, 0xcb, 0x15, 0x8d, 0x9e,
        0x05, 0xf1,
    ],
];

/// Where releases are published.
pub const DEFAULT_SOURCE: &str =
    "https://github.com/sweatshopking/hwsight-data/releases/latest/download";

/// The manifest format this build understands.
const MANIFEST_FORMAT: u32 = 1;

/// No data file is anywhere near this big; it caps what a bad server can make us download.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Files an update may write. Anything else in a manifest is ignored, so a manifest can
/// never write outside the data folder.
const KNOWN_FILES: &[&str] = &["rules.toml", "firmware.toml", "pci.ids", "usb.ids"];

/// Gets release files by name. The real one uses HTTPS; tests use a folder.
pub trait Fetch {
    fn fetch(&self, name: &str, max_bytes: u64) -> Result<Vec<u8>, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// No signing key is built in yet, so updates are off.
    NotConfigured,
    /// Couldn't download something (offline, server down...).
    Download(String),
    /// The signature didn't match: the files may have been tampered with.
    BadSignature,
    /// The release needs a newer hwsight.
    NeedsNewerApp,
    /// The manifest or a file was broken.
    Invalid(String),
    /// Couldn't write to the data folder.
    Write(String),
}

impl UpdateError {
    pub fn message(&self) -> String {
        match self {
            Self::NotConfigured => "Data updates aren't available in this version.".to_owned(),
            Self::Download(e) => format!("Couldn't download the update: {e}"),
            Self::BadSignature => "The downloaded update failed its security check, so it \
                wasn't installed."
                .to_owned(),
            Self::NeedsNewerApp => "A data update needs a newer version of hwsight.".to_owned(),
            Self::Invalid(e) => format!(
                "The downloaded update was damaged ({e}), so it wasn't \
                installed."
            ),
            Self::Write(e) => format!("Couldn't save the update: {e}"),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct UpdateOutcome {
    /// Files installed, with their new versions.
    pub installed: Vec<(String, String)>,
    /// Files already up to date.
    pub current: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    format: u32,
    #[serde(default, rename = "file")]
    files: Vec<ManifestFile>,
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    name: String,
    version: String,
    size: u64,
    sha256: String,
}

/// Download and install newer data into `dest` (normally the download data folder).
/// `current` gives the version in use for each file name, if any.
pub fn update(
    fetch: &dyn Fetch,
    keys: &[[u8; 32]],
    dest: &Path,
    current: &dyn Fn(&str) -> Option<String>,
) -> Result<UpdateOutcome, UpdateError> {
    if keys.is_empty() {
        return Err(UpdateError::NotConfigured);
    }
    let manifest_bytes = fetch
        .fetch("manifest.toml", 1024 * 1024)
        .map_err(UpdateError::Download)?;
    let signature = fetch
        .fetch("manifest.toml.sig", 1024)
        .map_err(UpdateError::Download)?;
    if !signed_by_any(&manifest_bytes, &signature, keys) {
        return Err(UpdateError::BadSignature);
    }
    let text = std::str::from_utf8(&manifest_bytes)
        .map_err(|_| UpdateError::Invalid("manifest isn't text".into()))?;
    let manifest: Manifest =
        toml::from_str(text).map_err(|e| UpdateError::Invalid(e.to_string()))?;
    if manifest.format > MANIFEST_FORMAT {
        return Err(UpdateError::NeedsNewerApp);
    }

    // Download and check everything first; only then install, so one bad file doesn't
    // leave a mix of old and new data.
    let mut outcome = UpdateOutcome::default();
    let mut ready: Vec<(&ManifestFile, Vec<u8>)> = Vec::new();
    for file in &manifest.files {
        if !KNOWN_FILES.contains(&file.name.as_str()) || !is_date_version(&file.version) {
            continue;
        }
        if current(&file.name).is_some_and(|v| v >= file.version) {
            outcome.current.push(file.name.clone());
            continue;
        }
        if file.size > MAX_FILE_BYTES {
            return Err(UpdateError::Invalid(format!("{} is too big", file.name)));
        }
        let bytes = fetch
            // Never read more than the manifest promised.
            .fetch(&file.name, file.size)
            .map_err(UpdateError::Download)?;
        if bytes.len() as u64 != file.size || sha256_hex(&bytes) != file.sha256.to_lowercase() {
            return Err(UpdateError::Invalid(format!(
                "{} doesn't match its checksum",
                file.name
            )));
        }
        validate(&file.name, &file.version, &bytes).map_err(UpdateError::Invalid)?;
        ready.push((file, bytes));
    }

    fs::create_dir_all(dest).map_err(|e| UpdateError::Write(e.to_string()))?;
    for (file, bytes) in ready {
        write_atomically(&dest.join(&file.name), &bytes).map_err(UpdateError::Write)?;
        outcome
            .installed
            .push((file.name.clone(), file.version.clone()));
    }
    Ok(outcome)
}

fn signed_by_any(message: &[u8], signature: &[u8], keys: &[[u8; 32]]) -> bool {
    let Ok(signature) = Signature::from_slice(signature) else {
        return false;
    };
    keys.iter().any(|key| {
        VerifyingKey::from_bytes(key).is_ok_and(|k| k.verify_strict(message, &signature).is_ok())
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Load the file exactly as a scan would, and check it's the version the manifest says.
fn validate(name: &str, version: &str, bytes: &[u8]) -> Result<(), String> {
    let found = file_version(name, bytes)?;
    if found != version {
        return Err(format!(
            "{name} says version {found}, manifest says {version}"
        ));
    }
    Ok(())
}

/// Load a data file exactly as a scan would and return its version. Used when checking
/// downloads and by the release tool when building a manifest, so the two always agree.
pub fn file_version(name: &str, bytes: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| format!("{name} isn't text"))?;
    Ok(match name {
        "rules.toml" => data::parse_data::<crate::rules::RuleSet>(text)
            .map(|(v, _)| v)
            .map_err(|e| format!("{name}: {e:?}"))?,
        "firmware.toml" => data::parse_data::<crate::firmware::Table>(text)
            .map(|(v, _)| v)
            .map_err(|e| format!("{name}: {e:?}"))?,
        "pci.ids" => {
            if !crate::pci_ids::looks_valid(text) {
                return Err("pci.ids has no devices".into());
            }
            crate::pci_ids::header_version(text).ok_or("pci.ids has no version")?
        }
        // usb.ids: the same format; checked fully once the USB name reader exists.
        "usb.ids" => crate::pci_ids::header_version(text).ok_or("usb.ids has no version")?,
        _ => return Err(format!("unexpected file {name}")),
    })
}

/// Names of the files a data release may contain.
pub fn known_files() -> &'static [&'static str] {
    KNOWN_FILES
}

/// SHA-256 of `bytes` as lowercase hex, as written in manifests.
pub fn checksum(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("data");
    let tmp = path.with_file_name(format!(".{name}.part"));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|e| e.to_string())
}

/// The versions currently in use, for `update`'s `current` argument.
pub fn current_versions(dirs: &DataDirs) -> impl Fn(&str) -> Option<String> {
    let rules = crate::rules::Rules::load(dirs).0.version;
    let firmware = crate::firmware::FirmwarePackages::load(dirs).0.version;
    let ids = |name: &str| {
        dirs.id_files(name)
            .iter()
            .filter_map(|p| fs::read(p).ok())
            .filter_map(|b| crate::pci_ids::header_version(&String::from_utf8_lossy(&b)))
            .max()
    };
    let pci = ids("pci.ids");
    let usb = ids("usb.ids");
    move |name: &str| match name {
        "rules.toml" => Some(rules.clone()),
        "firmware.toml" => Some(firmware.clone()),
        "pci.ids" => pci.clone(),
        "usb.ids" => usb.clone(),
        _ => None,
    }
}

/// Whether it's time for the daily check. The time of the last check lives in `state`.
pub fn check_due(state: &Path, now: SystemTime, every: Duration) -> bool {
    fs::metadata(state)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|last| now.duration_since(last).ok())
        .is_none_or(|age| age >= every)
}

/// Record that a check happened now.
pub fn mark_checked(state: &Path) -> Result<(), String> {
    if let Some(dir) = state.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(state, b"").map_err(|e| e.to_string())
}

/// `$XDG_STATE_HOME/hwsight/last-update-check`.
pub fn default_state_file() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .map(|base| base.join("hwsight/last-update-check"))
}

/// Downloads over HTTPS from a release folder.
#[cfg(feature = "updater")]
pub struct HttpFetch {
    pub base_url: String,
}

#[cfg(feature = "updater")]
impl Fetch for HttpFetch {
    fn fetch(&self, name: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(60)))
            .https_only(true)
            .user_agent(concat!("hwsight/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        let url = format!("{}/{name}", self.base_url.trim_end_matches('/'));
        let mut response = agent.get(&url).call().map_err(|e| e.to_string())?;
        response
            .body_mut()
            .with_config()
            .limit(max_bytes)
            .read_to_vec()
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::cell::RefCell;
    use std::collections::HashMap;

    struct FakeServer {
        files: HashMap<String, Vec<u8>>,
        fetched: RefCell<Vec<String>>,
    }

    impl Fetch for FakeServer {
        fn fetch(&self, name: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
            self.fetched.borrow_mut().push(name.to_owned());
            let bytes = self.files.get(name).cloned().ok_or("404")?;
            if bytes.len() as u64 > max_bytes {
                return Err("too big".into());
            }
            Ok(bytes)
        }
    }

    const RULES: &str = "schema = 1\nversion = \"2099.01.01\"\n";
    const FIRMWARE: &str = "schema = 1\nversion = \"2099.01.01\"\n";

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7; 32])
    }

    fn entry(name: &str, version: &str, bytes: &[u8]) -> String {
        format!(
            "[[file]]\nname = \"{name}\"\nversion = \"{version}\"\nsize = {}\nsha256 = \"{}\"\n",
            bytes.len(),
            sha256_hex(bytes)
        )
    }

    fn server(manifest: &str, signer: &SigningKey, files: &[(&str, &[u8])]) -> FakeServer {
        let mut map: HashMap<String, Vec<u8>> = files
            .iter()
            .map(|(n, b)| ((*n).to_owned(), b.to_vec()))
            .collect();
        map.insert("manifest.toml".into(), manifest.as_bytes().to_vec());
        map.insert(
            "manifest.toml.sig".into(),
            signer.sign(manifest.as_bytes()).to_bytes().to_vec(),
        );
        FakeServer {
            files: map,
            fetched: RefCell::new(Vec::new()),
        }
    }

    fn keys() -> Vec<[u8; 32]> {
        vec![key().verifying_key().to_bytes()]
    }

    fn manifest() -> String {
        format!(
            "format = 1\n{}{}",
            entry("rules.toml", "2099.01.01", RULES.as_bytes()),
            entry("firmware.toml", "2099.01.01", FIRMWARE.as_bytes())
        )
    }

    #[test]
    fn installs_signed_newer_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let srv = server(
            &manifest(),
            &key(),
            &[
                ("rules.toml", RULES.as_bytes()),
                ("firmware.toml", FIRMWARE.as_bytes()),
            ],
        );
        let current = |name: &str| (name == "firmware.toml").then(|| "2099.01.01".to_owned());
        let out = update(&srv, &keys(), tmp.path(), &current).expect("update");
        assert_eq!(
            out.installed,
            vec![("rules.toml".into(), "2099.01.01".into())]
        );
        assert_eq!(out.current, vec!["firmware.toml".to_owned()]);
        assert_eq!(
            fs::read_to_string(tmp.path().join("rules.toml")).expect("read"),
            RULES
        );
        assert!(
            !srv.fetched.borrow().contains(&"firmware.toml".to_owned()),
            "up-to-date file not downloaded"
        );
        // The installed file is what a scan now uses.
        let (rules, _) = crate::rules::Rules::load(&DataDirs::only(tmp.path()));
        assert_eq!(rules.version, "2099.01.01");
    }

    #[test]
    fn wrong_key_installs_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let attacker = SigningKey::from_bytes(&[9; 32]);
        let srv = server(&manifest(), &attacker, &[("rules.toml", RULES.as_bytes())]);
        let err = update(&srv, &keys(), tmp.path(), &|_| None).expect_err("must fail");
        assert_eq!(err, UpdateError::BadSignature);
        assert!(fs::read_dir(tmp.path()).expect("dir").next().is_none());
    }

    #[test]
    fn tampered_file_installs_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Same size as the real file, different content: only the checksum catches it.
        let evil = "schema = 1\nversion = \"2099.01.02\"\n";
        assert_eq!(evil.len(), RULES.len());
        let srv = server(
            &manifest(),
            &key(),
            &[
                ("rules.toml", evil.as_bytes()),
                ("firmware.toml", FIRMWARE.as_bytes()),
            ],
        );
        let err = update(&srv, &keys(), tmp.path(), &|_| None).expect_err("must fail");
        assert!(matches!(err, UpdateError::Invalid(_)));
        assert!(
            !tmp.path().join("firmware.toml").exists(),
            "no partial install"
        );
    }

    #[test]
    fn oversized_file_installs_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let big = format!("{RULES}# padding to make it bigger than the manifest says\n");
        let srv = server(
            &manifest(),
            &key(),
            &[
                ("rules.toml", big.as_bytes()),
                ("firmware.toml", FIRMWARE.as_bytes()),
            ],
        );
        assert!(update(&srv, &keys(), tmp.path(), &|_| None).is_err());
        assert!(fs::read_dir(tmp.path()).expect("dir").next().is_none());
    }

    #[test]
    fn unloadable_file_is_rejected_even_if_signed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let broken = "schema = 1\nversion = \"2099.01.01\"\n[[no_driver_needed]]\nclass = \"zz\"\nreason = \"x\"\n";
        let m = format!(
            "format = 1\n{}",
            entry("rules.toml", "2099.01.01", broken.as_bytes())
        );
        let srv = server(&m, &key(), &[("rules.toml", broken.as_bytes())]);
        assert!(matches!(
            update(&srv, &keys(), tmp.path(), &|_| None),
            Err(UpdateError::Invalid(_))
        ));
    }

    #[test]
    fn unknown_names_and_paths_are_ignored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let m = format!("format = 1\n{}", entry("../../.bashrc", "2099.01.01", b"x"));
        let srv = server(&m, &key(), &[("../../.bashrc", b"x")]);
        let out = update(&srv, &keys(), tmp.path(), &|_| None).expect("ok");
        assert!(out.installed.is_empty());
        assert!(!srv.fetched.borrow().contains(&"../../.bashrc".to_owned()));
    }

    #[test]
    fn newer_manifest_format_asks_for_app_update() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let srv = server("format = 2\n", &key(), &[]);
        assert_eq!(
            update(&srv, &keys(), tmp.path(), &|_| None),
            Err(UpdateError::NeedsNewerApp)
        );
    }

    #[test]
    fn off_without_keys() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let srv = server(&manifest(), &key(), &[]);
        assert_eq!(
            update(&srv, &[], tmp.path(), &|_| None),
            Err(UpdateError::NotConfigured)
        );
        assert!(
            srv.fetched.borrow().is_empty(),
            "nothing downloaded when off"
        );
    }

    #[test]
    fn daily_check_timing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = tmp.path().join("state/last");
        let day = Duration::from_secs(86_400);
        assert!(check_due(&state, SystemTime::now(), day), "never checked");
        mark_checked(&state).expect("mark");
        assert!(!check_due(&state, SystemTime::now(), day));
        assert!(check_due(&state, SystemTime::now() + day, day));
    }
}
