//! Translations of the text in hwsight's data files (descriptions, fix steps, software), one
//! updatable `text-<language>.toml` per language, like the other data files.
//!
//! Each file maps the English text, exactly as written in the data file, to its
//! translation. Looking text up by its English wording means a changed English text
//! simply shows in English until it's translated again: an old translation of different
//! words is never shown.

use std::collections::HashMap;

use serde::Deserialize;

use crate::data::{self, DataDirs, DataFile, DataProblem, DataSource};

/// The translated data text for one language. English (or a missing language) has no
/// entries, so every lookup returns the English text.
#[derive(Debug, Clone, Default)]
pub struct DataText {
    /// e.g. "de"; "en" when nothing is translated.
    pub language: String,
    /// Empty for English.
    pub version: String,
    pub source: Option<DataSource>,
    table: Table,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Table {
    language: String,
    by_english: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[allow(dead_code)] // checked by data::parse_data
    schema: u32,
    version: String,
    language: String,
    #[serde(default)]
    text: HashMap<String, String>,
}

/// Placeholders such as `{module}`, sorted, so a translation can be checked against its
/// English text.
fn placeholders(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open..];
        let Some(close) = after.find('}') else { break };
        found.push(&after[..=close]);
        rest = &after[close + 1..];
    }
    found.sort_unstable();
    found
}

impl DataFile for Table {
    // Loaded per language with `data::load_newest_named`; these two are not used.
    const FILE_NAME: &'static str = "text.toml";
    const EMBEDDED: &'static str = "schema = 1\nversion = \"2026.01.01\"\nlanguage = \"en\"\n";
    const SCHEMA: u32 = 1;

    fn parse_body(text: &str) -> Result<(String, Self), String> {
        let raw: RawFile = toml::from_str(text).map_err(|e| e.to_string())?;
        for (english, translated) in &raw.text {
            if translated.trim().is_empty() {
                return Err(format!("empty translation for {english:?}"));
            }
            if placeholders(english) != placeholders(translated) {
                return Err(format!(
                    "the translation of {english:?} must keep the placeholders {:?}",
                    placeholders(english)
                ));
            }
        }
        Ok((
            raw.version,
            Self {
                language: raw.language,
                by_english: raw.text,
            },
        ))
    }

    fn empty() -> Self {
        Self::default()
    }
}

/// The built-in copy for a language, if hwsight ships one.
fn embedded_text(language: &str) -> Option<&'static str> {
    Some(match language {
        "de" => include_str!("../data/text-de.toml"),
        "es" => include_str!("../data/text-es.toml"),
        "fr" => include_str!("../data/text-fr.toml"),
        "it" => include_str!("../data/text-it.toml"),
        "ja" => include_str!("../data/text-ja.toml"),
        "ko" => include_str!("../data/text-ko.toml"),
        "nl" => include_str!("../data/text-nl.toml"),
        "pt_BR" => include_str!("../data/text-pt_BR.toml"),
        "ru" => include_str!("../data/text-ru.toml"),
        "tr" => include_str!("../data/text-tr.toml"),
        "zh_CN" => include_str!("../data/text-zh_CN.toml"),
        _ => return None,
    })
}

/// The data file name for a language, e.g. `text-de.toml`.
pub fn file_name(language: &str) -> String {
    format!("text-{language}.toml")
}

impl DataText {
    /// No translations: every lookup returns English.
    pub fn english() -> Self {
        Self {
            language: "en".to_owned(),
            ..Self::default()
        }
    }

    /// The newest copy for `language` (built in or downloaded). English, a language
    /// hwsight has no data text for, and `i18n::in_english` all give `english()`.
    pub fn load(dirs: &DataDirs, language: &str) -> (Self, Vec<DataProblem>) {
        let Some(embedded) = embedded_text(language) else {
            return (Self::english(), Vec::new());
        };
        let (loaded, problems) =
            data::load_newest_named::<Table>(dirs, &file_name(language), embedded);
        // A file under the wrong name must not show another language's text: use the
        // built-in copy instead.
        let loaded = if loaded.data.language == language {
            loaded
        } else {
            data::embedded_from::<Table>(embedded)
        };
        (
            Self {
                language: language.to_owned(),
                version: loaded.version,
                source: Some(loaded.source),
                table: loaded.data,
            },
            problems,
        )
    }

    /// A language's text from a file's contents (tests in other modules).
    #[cfg(test)]
    pub(crate) fn from_toml(text: &str) -> Self {
        let loaded = data::embedded_from::<Table>(text);
        Self {
            language: loaded.data.language.clone(),
            version: loaded.version,
            source: None,
            table: loaded.data,
        }
    }

    /// The translation of `english`, or `english` itself.
    pub fn get<'a>(&'a self, english: &'a str) -> &'a str {
        self.table
            .by_english
            .get(english)
            .map_or(english, String::as_str)
    }
}

/// Every language with a built-in data text file (checked against the UI's languages).
pub(crate) const LANGUAGES: &[&str] = &[
    "de", "es", "fr", "it", "ja", "ko", "nl", "pt_BR", "ru", "tr", "zh_CN",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn parse(text: &str) -> Result<Table, String> {
        data::parse_data::<Table>(text)
            .map(|(_, t)| t)
            .map_err(|e| format!("{e:?}"))
    }

    fn english_texts() -> std::collections::HashSet<String> {
        crate::descriptions::Descriptions::english_texts()
            .into_iter()
            .chain(crate::fixes::Fixes::english_texts())
            .chain(crate::software::Software::english_texts())
            .collect()
    }

    /// Keys that match no English text are typos or leftovers from changed text: they
    /// would silently never be used.
    #[test]
    fn every_translation_belongs_to_a_real_english_text() {
        let english = english_texts();
        let mut problems = Vec::new();
        for language in LANGUAGES {
            let table = parse(embedded_text(language).expect("file")).expect("valid");
            for key in table.by_english.keys() {
                if !english.contains(key) {
                    problems.push(format!(
                        "text-{language}.toml: no such English text: {key:?}"
                    ));
                }
            }
        }
        problems.sort();
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    // On when the data translation round is done (cargo test -- --ignored).
    #[test]
    #[ignore = "needs every data text translated in every language"]
    fn every_data_text_is_translated() {
        let english = english_texts();
        let mut problems = Vec::new();
        for language in LANGUAGES {
            let table = parse(embedded_text(language).expect("file")).expect("valid");
            for text in &english {
                if !table.by_english.contains_key(text) {
                    problems.push(format!("text-{language}.toml: missing {text:?}"));
                }
            }
        }
        problems.sort();
        assert!(
            problems.is_empty(),
            "{} missing:\n{}",
            problems.len(),
            problems.join("\n")
        );
    }

    #[test]
    fn every_ui_language_has_valid_built_in_data_text() {
        assert_eq!(LANGUAGES, crate::i18n::AVAILABLE_LANGUAGES);
        for language in LANGUAGES {
            let text = embedded_text(language).expect("built-in file");
            let table = parse(text).expect("valid file");
            assert_eq!(table.language, *language);
        }
    }

    #[test]
    fn translation_found_or_english_returned() {
        let table = parse(
            "schema = 1\nversion = \"2026.11.06\"\nlanguage = \"de\"\n[text]\n\"Plays sound.\" = \"Spielt Ton ab.\"\n",
        )
        .expect("valid");
        let text = DataText {
            language: "de".into(),
            table,
            ..DataText::default()
        };
        assert_eq!(text.get("Plays sound."), "Spielt Ton ab.");
        assert_eq!(text.get("Something new."), "Something new.");
        assert_eq!(DataText::english().get("Plays sound."), "Plays sound.");
    }

    #[test]
    fn placeholders_must_match() {
        let ok = "schema = 1\nversion = \"2026.11.06\"\nlanguage = \"de\"\n[text]\n\"Install {package}.\" = \"Installiere {package}.\"\n";
        assert!(parse(ok).is_ok());
        let renamed = ok.replace("Installiere {package}", "Installiere {paket}");
        assert!(parse(&renamed).is_err());
        let dropped = ok.replace("Installiere {package}", "Installiere es");
        assert!(parse(&dropped).is_err());
    }

    #[test]
    fn empty_translation_is_rejected() {
        let text = "schema = 1\nversion = \"2026.11.06\"\nlanguage = \"de\"\n[text]\n\"Plays sound.\" = \" \"\n";
        assert!(parse(text).is_err());
    }

    #[test]
    fn newer_download_wins_and_wrong_language_is_ignored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dirs = DataDirs::only(tmp.path());
        fs::write(
            tmp.path().join("text-de.toml"),
            "schema = 1\nversion = \"2099.01.01\"\nlanguage = \"de\"\n[text]\n\"Plays sound.\" = \"Neu.\"\n",
        )
        .expect("write");
        let (text, problems) = DataText::load(&dirs, "de");
        assert!(problems.is_empty());
        assert_eq!(text.get("Plays sound."), "Neu.");
        assert_eq!(text.version, "2099.01.01");

        fs::write(
            tmp.path().join("text-de.toml"),
            "schema = 1\nversion = \"2099.01.01\"\nlanguage = \"fr\"\n[text]\n\"Plays sound.\" = \"Joue.\"\n",
        )
        .expect("write");
        let (text, _) = DataText::load(&dirs, "de");
        assert_eq!(text.get("Plays sound."), "Plays sound.");
    }

    #[test]
    fn english_and_unknown_languages_translate_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dirs = DataDirs::only(tmp.path());
        assert_eq!(DataText::load(&dirs, "en").0.language, "en");
        assert_eq!(DataText::load(&dirs, "xx").0.get("Hello."), "Hello.");
    }
}
