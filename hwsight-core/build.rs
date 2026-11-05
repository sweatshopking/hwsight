//! Bundles the translation catalogs in `po/` into a lookup table, so translations work
//! in every package type without separate files at run time. See `po/README.md`.

#[path = "build_support/po.rs"]
mod po;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;

/// The catalog file name inside `po/<language>/LC_MESSAGES/`. Slint names its catalogs
/// after the GUI crate, so the core, CLI, and GUI share this one file.
const CATALOG: &str = "hwsight-gui.po";

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let po_root = manifest.join("../po");
    println!("cargo:rerun-if-changed={}", po_root.display());
    println!("cargo:rerun-if-changed=build_support/po.rs");

    let mut catalogs = BTreeMap::new();
    let entries = fs::read_dir(&po_root)
        .unwrap_or_else(|e| panic!("couldn't read {}: {e}", po_root.display()));
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let language = entry.file_name().to_string_lossy().to_string();
        let catalog = entry.path().join("LC_MESSAGES").join(CATALOG);
        println!("cargo:rerun-if-changed={}", catalog.display());
        if !catalog.is_file() {
            continue;
        }
        let text = fs::read_to_string(&catalog)
            .unwrap_or_else(|e| panic!("couldn't read {}: {e}", catalog.display()));
        let messages = po::parse(&text)
            .unwrap_or_else(|e| panic!("couldn't parse {}: {e}", catalog.display()));
        catalogs.insert(language, messages);
    }

    let languages: Vec<String> = catalogs.keys().map(|l| format!("{l:?}")).collect();
    let mut out = format!(
        "pub const AVAILABLE_LANGUAGES: &[&str] = &[{}];\n",
        languages.join(", ")
    );

    out.push_str(
        "#[allow(unused_variables, clippy::match_single_binding)] // until a language has translations\npub fn lookup(language: &str, message: &str) -> Option<&'static str> {\n    match language {\n",
    );
    for (language, messages) in &catalogs {
        let mut arm = String::new();
        for entry in messages {
            if entry.fuzzy || entry.plural.is_some() {
                continue;
            }
            if let Some(text) = entry.translation.as_ref().filter(|t| !t.is_empty()) {
                arm.push_str(&format!("            {:?} => Some({text:?}),\n", entry.id));
            }
        }
        // A language with nothing translated yet has no arm: its messages show in English.
        if !arm.is_empty() {
            out.push_str(&format!(
                "        {language:?} => match message {{\n{arm}            _ => None,\n        }},\n"
            ));
        }
    }
    out.push_str("        _ => None,\n    }\n}\n");

    out.push_str(
        "#[allow(unused_variables, clippy::match_single_binding)]\npub fn lookup_plural_forms(language: &str, message: &str) -> Option<&'static [&'static str]> {\n    match language {\n",
    );
    for (language, messages) in &catalogs {
        let mut arm = String::new();
        for entry in messages {
            if entry.fuzzy
                || entry.plural.is_none()
                || entry.plural_translations.is_empty()
                || entry.plural_translations.iter().any(|(_, s)| s.is_empty())
            {
                continue;
            }
            // Every form the language has, in msgstr[n] order; i18n::plural_index picks one.
            let forms: BTreeMap<_, _> = entry.plural_translations.iter().cloned().collect();
            let forms: Vec<String> = forms.values().map(|s| format!("{s:?}")).collect();
            arm.push_str(&format!(
                "            {:?} => Some(&[{}]),\n",
                entry.id,
                forms.join(", ")
            ));
        }
        if !arm.is_empty() {
            out.push_str(&format!(
                "        {language:?} => match message {{\n{arm}            _ => None,\n        }},\n"
            ));
        }
    }
    out.push_str("        _ => None,\n    }\n}\n");

    let path = PathBuf::from(env::var_os("OUT_DIR").unwrap_or_default()).join("translations.rs");
    fs::write(&path, out).unwrap_or_else(|e| panic!("couldn't write {}: {e}", path.display()));
}
