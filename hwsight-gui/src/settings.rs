//! The few choices the app remembers between runs, in `~/.config/hwsight/settings.toml`.

use std::fs;
use std::path::{Path, PathBuf};

use hwsight_core::details::Level;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// 0 = Simple, 1 = More details, 2 = Technical.
    #[serde(default)]
    pub detail_level: i32,
}

impl Settings {
    pub fn level(&self) -> Level {
        Level::from_index(self.detail_level)
    }
}

/// The folder for the settings file: `$XDG_CONFIG_HOME/hwsight`, else `~/.config/hwsight`.
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("hwsight"))
}

/// The saved settings, or the defaults if there's no file or it can't be read. A broken
/// settings file must never stop the app from starting.
pub fn load_from(dir: &Path) -> Settings {
    fs::read_to_string(dir.join("settings.toml"))
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

/// Save the settings; failing to (a read-only home, say) is silently fine.
pub fn save_to(dir: &Path, settings: &Settings) {
    let Ok(text) = toml::to_string(settings) else {
        return;
    };
    if fs::create_dir_all(dir).is_ok() {
        let _ = fs::write(dir.join("settings.toml"), text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_level_is_read_back() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("hwsight");
        assert_eq!(load_from(&dir), Settings::default());
        save_to(&dir, &Settings { detail_level: 2 });
        assert_eq!(load_from(&dir).level(), Level::Technical);
    }

    #[test]
    fn a_broken_or_odd_file_gives_the_defaults() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("settings.toml"), "detail_level = [[[").expect("write");
        assert_eq!(load_from(temp.path()), Settings::default());
        fs::write(temp.path().join("settings.toml"), "detail_level = 99\n").expect("write");
        assert_eq!(load_from(temp.path()).level(), Level::Technical);
        fs::write(temp.path().join("settings.toml"), "other = 1\n").expect("write");
        assert_eq!(load_from(temp.path()).level(), Level::Simple);
    }
}
