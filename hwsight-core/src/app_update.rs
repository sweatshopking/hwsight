//! Updating hwsight itself (the data updater in `update.rs` is separate).
//!
//! Once a day the app asks GitHub for the latest release. How it then updates depends on
//! how it was installed:
//!
//! - **AppImage**: the new AppImage is downloaded next to the running one, checked, and
//!   swapped in with one rename. No password, since it's the user's own file.
//! - **.deb / .rpm**: the package is downloaded, checked, and installed with the system's
//!   package manager through `pkexec` (one password prompt).
//! - **AUR** (pacman owns the binary): updated through the AUR, so we only say so.
//! - **Anything else** (a source build): we only point to the release page.
//!
//! Every download is checked against the release's `SHA256SUMS.txt` before use.
//! Adapted from DriveDeck's updater (same author, GPL-3.0-or-later).

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use sha2::{Digest, Sha256};

pub const RELEASES_URL: &str = "https://github.com/sweatshopking/hwsight/releases/latest";
const API_URL: &str = "https://api.github.com/repos/sweatshopking/hwsight/releases/latest";
/// Package name, and the start of every release file name.
pub const APP: &str = "hwsight";
const SUMS_FILE: &str = "SHA256SUMS.txt";
/// No release file is anywhere near this; it caps what a bad server can make us download.
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;

/// "1.2.3" or "v1.2.3" as numbers; anything else (a pre-release tag, say) is `None`.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim();
    let v = v.strip_prefix('v').unwrap_or(v);
    let mut it = v.split('.');
    let t = (
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    );
    it.next().is_none().then_some(t)
}

/// Whether `latest` is newer than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(l), Some(c)) if l > c)
}

/// A published release: its version and its files (name, https download address).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub assets: Vec<(String, String)>,
}

/// How this copy of hwsight was installed, which decides how it's updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    AppImage(PathBuf),
    /// Installed from our .deb or .rpm: install the new file with this command (the
    /// file's path is added last).
    Package {
        asset: Asset,
        install: Vec<String>,
    },
    /// pacman owns it (the AUR package): updated by the AUR helper, not by us.
    Aur,
    /// Built from source or copied by hand.
    Unmanaged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asset {
    Deb,
    Rpm,
}

/// How an update attempt ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The new version is in place; start this program to run it.
    Updated {
        version: String,
        restart: PathBuf,
    },
    /// The password prompt was closed.
    Cancelled,
    /// This is already the newest version.
    UpToDate,
    /// This kind of install is updated another way (AUR, source build).
    UpdateElsewhere,
    Failed(String),
}

impl Outcome {
    pub fn message(&self) -> String {
        match self {
            Self::Updated { version, .. } => {
                format!("hwsight {version} is installed. Restart hwsight to use it.")
            }
            Self::Cancelled => "The update was cancelled.".to_owned(),
            Self::UpToDate => "hwsight is up to date.".to_owned(),
            Self::UpdateElsewhere => format!(
                "This copy of hwsight is updated through your package manager or from \
                 {RELEASES_URL}."
            ),
            Self::Failed(e) => format!("Couldn't update hwsight: {e}"),
        }
    }
}

// ---------------------------------------------------------------- finding the release

/// The latest ordinary release. `Ok(None)` when GitHub has none to offer (none yet, or
/// only drafts and pre-releases); `Err` when it couldn't be asked (offline, busy).
#[cfg(feature = "updater")]
pub fn latest_release() -> Result<Option<Release>, String> {
    let url = std::env::var("HWSIGHT_RELEASES_API").unwrap_or_else(|_| API_URL.to_owned());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .https_only(true)
        .build()
        .into();
    let mut resp = agent
        .get(&url)
        // GitHub requires one; it says nothing about the user.
        .header("User-Agent", APP)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    match resp.status().as_u16() {
        200 => {}
        404 => return Ok(None),
        code => return Err(format!("GitHub answered {code}")),
    }
    let text = resp
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(release_from_json(&json))
}

/// Reads GitHub's release JSON. Drafts, pre-releases, unparsable versions, and
/// non-https files are ignored.
#[cfg(feature = "updater")]
pub fn release_from_json(v: &serde_json::Value) -> Option<Release> {
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return None;
    }
    let (a, b, c) = parse_version(v["tag_name"].as_str()?)?;
    let assets = v["assets"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    let name = a["name"].as_str()?.to_owned();
                    let url = a["browser_download_url"].as_str()?;
                    url.starts_with("https://").then(|| (name, url.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Release {
        version: format!("{a}.{b}.{c}"),
        assets,
    })
}

// ---------------------------------------------------------------- how we were installed

/// Work out how this copy was installed. `appimage` is the `APPIMAGE` variable, which the
/// AppImage runtime sets to the AppImage's own path.
pub fn detect_install(exe: &Path, appimage: Option<OsString>) -> Install {
    if let Some(path) = appimage.map(PathBuf::from)
        && path.is_file()
    {
        return Install::AppImage(path);
    }
    let succeeds = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    let exe_str = exe.to_string_lossy();
    let owned_by = |tool: &str| match tool {
        "dpkg" => succeeds("dpkg-query", &["-S", &exe_str]),
        "rpm" => succeeds("rpm", &["-qf", &exe_str]),
        "pacman" => succeeds("pacman", &["-Qo", &exe_str]),
        _ => false,
    };
    let has_tool = |tool: &str| {
        std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(tool).is_file()))
    };
    install_from(&owned_by, &has_tool)
}

/// The decision itself, separate from running commands so it can be tested.
fn install_from(owned_by: &dyn Fn(&str) -> bool, has_tool: &dyn Fn(&str) -> bool) -> Install {
    let cmd = |parts: &[&str]| parts.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    if owned_by("pacman") {
        Install::Aur
    } else if owned_by("dpkg") {
        Install::Package {
            asset: Asset::Deb,
            install: cmd(&["pkexec", "apt-get", "install", "-y"]),
        }
    } else if owned_by("rpm") {
        let install = if has_tool("dnf") {
            cmd(&["pkexec", "dnf", "install", "-y"])
        } else if has_tool("zypper") {
            cmd(&[
                "pkexec",
                "zypper",
                "--non-interactive",
                "install",
                "--allow-unsigned-rpm",
            ])
        } else {
            cmd(&["pkexec", "rpm", "-U"])
        };
        Install::Package {
            asset: Asset::Rpm,
            install,
        }
    } else {
        Install::Unmanaged
    }
}

// ---------------------------------------------------------------- release file names

/// The release file for this kind of install:
/// `hwsight-1.2.0-x86_64.AppImage`, `hwsight_1.2.0_amd64.deb`,
/// `hwsight-1.2.0-<release>.x86_64.rpm` (the rpm release number can vary).
pub fn asset_for(install: &Install, release: &Release, arch: &str) -> Option<String> {
    let v = &release.version;
    match install {
        Install::AppImage(_) => {
            matches!(arch, "x86_64" | "aarch64").then(|| format!("{APP}-{v}-{arch}.AppImage"))
        }
        Install::Package {
            asset: Asset::Deb, ..
        } => {
            let a = match arch {
                "x86_64" => "amd64",
                "aarch64" => "arm64",
                _ => return None,
            };
            Some(format!("{APP}_{v}_{a}.deb"))
        }
        Install::Package {
            asset: Asset::Rpm, ..
        } => {
            let (pre, post) = (format!("{APP}-{v}-"), format!(".{arch}.rpm"));
            release
                .assets
                .iter()
                .map(|(n, _)| n)
                .find(|n| n.starts_with(&pre) && n.ends_with(&post))
                .cloned()
        }
        Install::Aur | Install::Unmanaged => None,
    }
}

/// The checksum `SHA256SUMS.txt` lists for `name` (`<hex>  <name>` or `<hex> *<name>`).
pub fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, file) = l.trim().split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hash.to_lowercase())
    })
}

// ---------------------------------------------------------------- installing

/// Download, check, and install `release` for this kind of install. Blocking: run it on a
/// background thread.
#[cfg(feature = "updater")]
pub fn install_update(release: &Release, install: &Install, current: &str) -> Outcome {
    if !is_newer(&release.version, current) {
        return Outcome::UpToDate;
    }
    let Some(name) = asset_for(install, release, std::env::consts::ARCH) else {
        return match install {
            Install::Aur | Install::Unmanaged => Outcome::UpdateElsewhere,
            _ => Outcome::Failed("this release has no file for this computer".to_owned()),
        };
    };
    let find = |n: &str| {
        release
            .assets
            .iter()
            .find(|(a, _)| a == n)
            .map(|(_, u)| u.clone())
    };
    let (Some(url), Some(sums_url)) = (find(&name), find(SUMS_FILE)) else {
        return Outcome::Failed(format!("the release is missing {name} or its checksums"));
    };
    let sums =
        match download(&sums_url, 1024 * 1024).map(|b| String::from_utf8_lossy(&b).into_owned()) {
            Ok(s) => s,
            Err(e) => return Outcome::Failed(e),
        };
    let Some(expected) = sum_for(&sums, &name) else {
        return Outcome::Failed(format!("the release doesn't list a checksum for {name}"));
    };

    match install {
        Install::AppImage(target) => replace_appimage(target, &url, &expected, &release.version),
        Install::Package { install, .. } => {
            install_package(install, &url, &name, &expected, &release.version)
        }
        Install::Aur | Install::Unmanaged => Outcome::UpdateElsewhere,
    }
}

#[cfg(feature = "updater")]
fn download(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_global(Some(Duration::from_secs(30 * 60)))
        .https_only(true)
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .header("User-Agent", APP)
        .call()
        .map_err(|e| format!("couldn't download the update ({e})"))?;
    let mut bytes = Vec::new();
    resp.body_mut()
        .with_config()
        .limit(limit.saturating_add(1))
        .reader()
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| format!("couldn't download the update ({e})"))?;
    if bytes.len() as u64 > limit {
        return Err("the download was bigger than expected".to_owned());
    }
    Ok(bytes)
}

/// Download, check the checksum, and write to `dest` (a new file only we can read).
/// Nothing is left behind if anything goes wrong.
#[cfg(feature = "updater")]
fn download_checked(url: &str, dest: &Path, expected: &str) -> Result<(), String> {
    let bytes = download(url, MAX_DOWNLOAD)?;
    if hex(&Sha256::digest(&bytes)) != expected {
        return Err(
            "the download didn't match the release's checksum, so it wasn't used".to_owned(),
        );
    }
    write_new_file(dest, &bytes, 0o600)
}

fn write_new_file(dest: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::remove_file(dest);
    let result = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(dest)
        .and_then(|mut f| f.write_all(bytes).and_then(|()| f.sync_all()));
    if let Err(e) = result {
        let _ = std::fs::remove_file(dest);
        return Err(format!("couldn't save the update ({e})"));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Download next to the AppImage, then swap it in with one rename. The running copy
/// keeps working from its old file until it's restarted.
#[cfg(feature = "updater")]
fn replace_appimage(target: &Path, url: &str, expected: &str, version: &str) -> Outcome {
    let Some(dir) = target.parent() else {
        return Outcome::Failed("couldn't find the AppImage's folder".to_owned());
    };
    let tmp = dir.join(format!(".{APP}.update-{}", std::process::id()));
    if let Err(e) = download_checked(url, &tmp, expected) {
        return Outcome::Failed(e);
    }
    use std::os::unix::fs::PermissionsExt;
    let done = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
        .and_then(|()| std::fs::rename(&tmp, target));
    match done {
        Ok(()) => Outcome::Updated {
            version: version.to_owned(),
            restart: target.to_path_buf(),
        },
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Outcome::Failed(format!("couldn't put the new version in place ({e})"))
        }
    }
}

/// Download the package to a private folder and install it with the system's package
/// manager (one password prompt through pkexec).
#[cfg(feature = "updater")]
fn install_package(
    install: &[String],
    url: &str,
    name: &str,
    expected: &str,
    version: &str,
) -> Outcome {
    let Some(dir) = private_dir() else {
        return Outcome::Failed("couldn't make a private folder for the download".to_owned());
    };
    let file = dir.join(name);
    if let Err(e) = download_checked(url, &file, expected) {
        return Outcome::Failed(e);
    }
    let Some((program, args)) = install.split_first() else {
        return Outcome::Failed("no install command".to_owned());
    };
    let status = Command::new(program)
        .args(args)
        .arg(&file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = std::fs::remove_file(&file);
    match status.map(|s| s.code()) {
        Ok(Some(0)) => Outcome::Updated {
            version: version.to_owned(),
            restart: std::env::current_exe().unwrap_or_else(|_| PathBuf::from(APP)),
        },
        // pkexec: the prompt was closed (126) or the user isn't allowed (127).
        Ok(Some(126 | 127)) => Outcome::Cancelled,
        _ => Outcome::Failed(format!(
            "the package manager couldn't install version {version}"
        )),
    }
}

/// `$XDG_RUNTIME_DIR/hwsight` (only the user can read it), created if needed.
#[cfg(feature = "updater")]
fn private_dir() -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
    let dir = base.join(APP);
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok()?;
    Some(dir)
}

/// `$XDG_STATE_HOME/hwsight/last-app-update-check`, for the once-a-day timing (see
/// `update::check_due`).
pub fn default_state_file() -> Option<PathBuf> {
    crate::update::default_state_file().map(|p| p.with_file_name("last-app-update-check"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version(" 0.10.0 "), Some((0, 10, 0)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3-beta"), None);
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[cfg(feature = "updater")]
    #[test]
    fn release_json() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"tag_name":"v1.4.0","draft":false,"prerelease":false,"assets":[
                {"name":"hwsight_1.4.0_amd64.deb","browser_download_url":"https://x/a.deb"},
                {"name":"evil.deb","browser_download_url":"http://x/evil.deb"}]}"#,
        )
        .expect("json");
        let r = release_from_json(&json).expect("release");
        assert_eq!(r.version, "1.4.0");
        assert_eq!(
            r.assets,
            vec![("hwsight_1.4.0_amd64.deb".into(), "https://x/a.deb".into())]
        );
        for flag in ["draft", "prerelease"] {
            let mut j = json.clone();
            j[flag] = serde_json::Value::Bool(true);
            assert!(release_from_json(&j).is_none(), "{flag}");
        }
    }

    #[test]
    fn install_kinds() {
        let none = |_: &str| false;
        assert_eq!(install_from(&none, &none), Install::Unmanaged);
        assert_eq!(install_from(&|t| t == "pacman", &none), Install::Aur);
        let Install::Package { asset, install } = install_from(&|t| t == "dpkg", &none) else {
            panic!("expected a package");
        };
        assert_eq!(asset, Asset::Deb);
        assert_eq!(install, ["pkexec", "apt-get", "install", "-y"]);
        let rpm = |tools: &'static [&'static str]| match install_from(&|t| t == "rpm", &move |t| {
            tools.contains(&t)
        }) {
            Install::Package { install, .. } => install[1].clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(rpm(&["dnf", "zypper"]), "dnf");
        assert_eq!(rpm(&["zypper"]), "zypper");
        assert_eq!(rpm(&[]), "rpm");
    }

    #[test]
    fn appimage_needs_an_existing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let image = tmp.path().join("hwsight.AppImage");
        std::fs::write(&image, b"x").expect("write");
        assert_eq!(
            detect_install(Path::new("/x"), Some(image.clone().into_os_string())),
            Install::AppImage(image)
        );
    }

    #[test]
    fn file_names() {
        let release = Release {
            version: "1.4.0".into(),
            assets: vec![
                ("hwsight-1.4.0-1.x86_64.rpm".into(), "https://x".into()),
                ("hwsight-1.4.0-1.aarch64.rpm".into(), "https://x".into()),
            ],
        };
        let pkg = |asset| Install::Package {
            asset,
            install: vec![],
        };
        let app = Install::AppImage("/a".into());
        assert_eq!(
            asset_for(&app, &release, "x86_64").as_deref(),
            Some("hwsight-1.4.0-x86_64.AppImage")
        );
        assert_eq!(
            asset_for(&pkg(Asset::Deb), &release, "aarch64").as_deref(),
            Some("hwsight_1.4.0_arm64.deb")
        );
        assert_eq!(
            asset_for(&pkg(Asset::Rpm), &release, "x86_64").as_deref(),
            Some("hwsight-1.4.0-1.x86_64.rpm")
        );
        assert_eq!(asset_for(&app, &release, "riscv64"), None);
        assert_eq!(asset_for(&Install::Aur, &release, "x86_64"), None);
    }

    #[test]
    fn checksums() {
        let h = "a".repeat(64);
        let sums = format!(
            "{h}  hwsight-1.4.0-x86_64.AppImage\n{}  *hwsight_1.4.0_amd64.deb\n",
            "B".repeat(64)
        );
        assert_eq!(sum_for(&sums, "hwsight-1.4.0-x86_64.AppImage"), Some(h));
        assert_eq!(
            sum_for(&sums, "hwsight_1.4.0_amd64.deb"),
            Some("b".repeat(64))
        );
        assert_eq!(sum_for(&sums, "missing"), None);
        assert_eq!(sum_for("zz  bad\n", "bad"), None);
    }

    #[test]
    fn writes_private_files_and_cleans_up() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let f = tmp.path().join("dl");
        write_new_file(&f, b"data", 0o600).expect("write");
        assert_eq!(
            std::fs::metadata(&f).expect("meta").permissions().mode() & 0o777,
            0o600
        );
        assert!(write_new_file(&tmp.path().join("no/such/dir/f"), b"x", 0o600).is_err());
    }

    #[test]
    fn outcome_messages_are_plain() {
        let m = Outcome::Updated {
            version: "1.4.0".into(),
            restart: "/x".into(),
        }
        .message();
        assert_eq!(m, "hwsight 1.4.0 is installed. Restart hwsight to use it.");
    }
}
