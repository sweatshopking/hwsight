//! Whether Debian's `non-free` section is switched on in the apt software sources, and
//! the command that switches it on. Third-party drivers such as Broadcom's live there, so
//! installing one while it is off fails with "Unable to locate package".
//!
//! hwsight only reads these files. Changing them is left to the user (a copyable
//! command), because they are the system's own configuration.

use std::fs;
use std::path::{Path, PathBuf};

/// What the software sources say about one archive section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionStatus {
    /// Some Debian source already includes the section.
    pub enabled: bool,
    /// A command that adds the section to the Debian sources, if the format is one we
    /// know how to edit (the usual `debian.sources` or `sources.list`).
    pub enable_command: Option<String>,
}

/// Look in `apt_dir` (`/etc/apt`). `None` when nothing there looks like Debian's sources,
/// so hwsight never guesses on systems it can't read.
pub fn section_status(apt_dir: &Path, section: &str) -> Option<SectionStatus> {
    if !section.chars().all(|c| c.is_ascii_lowercase() || c == '-') || section.is_empty() {
        return None;
    }
    let mut enabled = false;
    let mut editable: Option<PathBuf> = None;
    let mut seen_debian = false;

    let list = apt_dir.join("sources.list");
    let mut files: Vec<PathBuf> = vec![list];
    if let Ok(entries) = fs::read_dir(apt_dir.join("sources.list.d")) {
        let mut found: Vec<PathBuf> = entries.filter_map(|e| Some(e.ok()?.path())).collect();
        found.sort();
        files.extend(found);
    }
    for path in files {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_sources = name.ends_with(".sources");
        if !is_sources && !(name == "sources.list" || name.ends_with(".list")) {
            continue;
        }
        let (debian, has_section) = if is_sources {
            scan_deb822(&text, section)
        } else {
            scan_one_line(&text, section)
        };
        if debian {
            seen_debian = true;
            enabled |= has_section;
            // Prefer the file Debian itself writes; any other Debian file will do.
            if editable.is_none() || name == "debian.sources" {
                editable = Some(path);
            }
        }
    }
    if !seen_debian {
        return None;
    }
    let enable_command = editable.and_then(|path| {
        let quoted = shell_quote(path.to_str()?)?;
        let is_sources = path.extension().is_some_and(|e| e == "sources");
        let edit = if is_sources {
            // Only the stanzas whose URIs line names a Debian archive host.
            format!(
                "sudo sed -i '/^URIs:.*debian\\.\\(org\\|net\\)/,/^$/ {{ /^Components:/ s/$/ {section}/ }}' {quoted}"
            )
        } else {
            // Only Debian archive lines; the section goes before any trailing comment.
            format!(
                "sudo sed -i -E '/^deb .*debian\\.(org|net)/ s/^([^#]*[^# ])( *#.*)?$/\\1 {section}\\2/' {quoted}"
            )
        };
        Some(format!("{edit} && sudo apt update"))
    });
    Some(SectionStatus {
        enabled,
        enable_command,
    })
}

/// A path that goes into a command we show: only plain characters, no quoting tricks.
fn shell_quote(path: &str) -> Option<String> {
    path.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
        .then(|| path.to_owned())
}

/// Debian's own archive hosts (`deb.debian.org`, `security.debian.org`, `ftp.de.debian.org`,
/// `deb.debian.net`), not third-party repos that merely have "debian" in their path.
fn is_debian_uri(uris: &str) -> bool {
    uris.split_whitespace().any(|uri| {
        let host = uri
            .split_once("://")
            .map_or(uri, |(_, rest)| rest)
            .split('/')
            .next()
            .unwrap_or("");
        host == "debian.org"
            || host == "debian.net"
            || host.ends_with(".debian.org")
            || host.ends_with(".debian.net")
    })
}

/// `Types/URIs/Suites/Components` stanzas. Returns (mentions a Debian archive, section on).
fn scan_deb822(text: &str, section: &str) -> (bool, bool) {
    let (mut debian, mut has) = (false, false);
    for stanza in text.split("\n\n") {
        let mut uris = "";
        let mut components = "";
        let mut types = "deb";
        let mut disabled = false;
        for line in stanza.lines() {
            if let Some(v) = line.strip_prefix("Types:") {
                types = v;
            } else if let Some(v) = line.strip_prefix("URIs:") {
                uris = v;
            } else if let Some(v) = line.strip_prefix("Components:") {
                components = v;
            } else if let Some(v) = line.strip_prefix("Enabled:") {
                disabled = v.trim().eq_ignore_ascii_case("no");
            }
        }
        if disabled || !is_debian_uri(uris) || !types.split_whitespace().any(|t| t == "deb") {
            continue;
        }
        debian = true;
        has |= components.split_whitespace().any(|c| c == section);
    }
    (debian, has)
}

/// `deb [options] URI suite component...` lines.
fn scan_one_line(text: &str, section: &str) -> (bool, bool) {
    let (mut debian, mut has) = (false, false);
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("deb ") else {
            continue;
        };
        let mut words = rest.split_whitespace().peekable();
        if words.peek().is_some_and(|w| w.starts_with('[')) {
            for w in words.by_ref() {
                if w.ends_with(']') {
                    break;
                }
            }
        }
        let Some(uri) = words.next() else { continue };
        if !is_debian_uri(uri) {
            continue;
        }
        debian = true;
        // Skip the suite; what follows are components (a `#` starts a comment).
        has |= words
            .skip(1)
            .take_while(|w| !w.starts_with('#'))
            .any(|c| c == section);
    }
    (debian, has)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apt(files: &[(&str, &str)]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        for (name, text) in files {
            let path = tmp.path().join(name);
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            fs::write(path, text).expect("write");
        }
        tmp
    }

    const MAIN_ONLY: &str = "Types: deb\nURIs: http://deb.debian.org/debian\nSuites: trixie trixie-updates\nComponents: main\n\nTypes: deb\nURIs: http://deb.debian.org/debian-security\nSuites: trixie-security\nComponents: main\n";

    #[test]
    fn deb822_without_the_section_gets_a_command() {
        let tmp = apt(&[("sources.list.d/debian.sources", MAIN_ONLY)]);
        let s = section_status(tmp.path(), "non-free").expect("debian");
        assert!(!s.enabled);
        let command = s.enable_command.expect("command");
        assert!(command.starts_with("sudo sed -i '/^URIs:.*debian"));
        assert!(command.contains("s/$/ non-free/"));
        assert!(command.ends_with("debian.sources && sudo apt update"));
    }

    #[test]
    fn deb822_with_the_section_is_enabled() {
        let tmp = apt(&[(
            "sources.list.d/debian.sources",
            &MAIN_ONLY.replace("Components: main", "Components: main contrib non-free"),
        )]);
        assert!(
            section_status(tmp.path(), "non-free")
                .expect("debian")
                .enabled
        );
        // non-free-firmware is a different section.
        let tmp = apt(&[(
            "sources.list.d/debian.sources",
            &MAIN_ONLY.replace("Components: main", "Components: main non-free-firmware"),
        )]);
        assert!(
            !section_status(tmp.path(), "non-free")
                .expect("debian")
                .enabled
        );
    }

    #[test]
    fn one_line_sources_are_read_too() {
        let off = "deb http://deb.debian.org/debian bookworm main non-free-firmware\ndeb-src http://deb.debian.org/debian bookworm main\n# deb http://deb.debian.org/debian bookworm main non-free\n";
        let tmp = apt(&[("sources.list", off)]);
        let s = section_status(tmp.path(), "non-free").expect("debian");
        assert!(!s.enabled);
        assert!(
            s.enable_command
                .expect("command")
                .starts_with("sudo sed -i -E '/^deb .*debian\\.(org|net)/ ")
        );
        let on = "deb [arch=amd64 signed-by=/x.gpg] http://deb.debian.org/debian bookworm main non-free # comment\n";
        let tmp = apt(&[("sources.list", on)]);
        assert!(
            section_status(tmp.path(), "non-free")
                .expect("debian")
                .enabled
        );
    }

    #[test]
    fn third_party_debian_paths_and_source_only_stanzas_do_not_count() {
        let docker =
            "deb [arch=amd64] https://download.docker.com/linux/debian bookworm stable non-free\n";
        let tmp = apt(&[("sources.list.d/docker.list", docker)]);
        assert_eq!(section_status(tmp.path(), "non-free"), None);
        let src_only = MAIN_ONLY.replace("Components: main", "Components: main non-free");
        let src_only = src_only.replace("Types: deb", "Types: deb-src");
        let tmp = apt(&[("sources.list.d/debian.sources", &src_only)]);
        assert_eq!(section_status(tmp.path(), "non-free"), None);
    }

    #[test]
    fn unreadable_or_non_debian_sources_are_not_guessed_at() {
        let tmp = apt(&[]);
        assert_eq!(section_status(tmp.path(), "non-free"), None);
        let tmp = apt(&[(
            "sources.list",
            "deb http://archive.ubuntu.com/ubuntu noble main\n",
        )]);
        assert_eq!(section_status(tmp.path(), "non-free"), None);
        let tmp = apt(&[("sources.list.d/debian.sources", MAIN_ONLY)]);
        assert_eq!(section_status(tmp.path(), "Non Free; rm"), None);
    }
}
