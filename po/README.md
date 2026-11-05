# Translations

hwsight bundles its translation catalogs into the binary, so translations work in every
package type (deb, rpm, AppImage) without extra files.

- `po/hwsight.pot`: every translatable string (made by the script below, don't edit).
- `po/<language>/LC_MESSAGES/hwsight-gui.po`: one catalog per language. The file is named
  after the GUI crate because Slint requires that; the core and the CLI use the same file.

## Marking text for translation

- Rust (core, CLI, GUI): `tr!("Text")`, `tr!("{} is missing", name)`, and for counts
  `tr!("1 device" | "{n} devices" % count)`. The macro is `hwsight_core::tr!`
  (`crate::tr!` inside the core).
- Slint: `@tr("Text")`, `@tr("{} is missing", name)`.
- Placeholders `{}` (or `{0}`, `{1}`) must stay in every translation.

Not translated: the forum report and the bug report (`report.rs` runs inside
`i18n::in_english`, because the people who answer them read English), `--json`
output, log messages, file paths, driver names, and hardware IDs.

## Updating the catalogs

After adding or changing text, run:

```sh
tools/update-translations.sh
```

It needs gettext (`msgmerge`) and `cargo install slint-tr-extractor --version 1.18.1`.
`tools/check.sh` fails until the catalogs match the code.

## Languages

German, Spanish, French, Italian, Japanese, Korean, Dutch, Brazilian Portuguese, Russian,
Turkish, and Simplified Chinese. The language comes from `LANGUAGE`, `LC_ALL`,
`LC_MESSAGES`, then `LANG`; regional locales fall back to their base language, then
English. Translations are drafted by machine; corrections are welcome.

To add a language, copy a catalog to `po/<language>/LC_MESSAGES/hwsight-gui.po`, set its
`Language` and `Plural-Forms` headers, add it to the list in the `catalog_problems` test in
`hwsight-core/src/i18n.rs`, and translate each entry.
