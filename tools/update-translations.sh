#!/bin/sh
# Rebuilds po/hwsight.pot from the Slint UI (@tr) and the Rust crates (tr!), then merges
# it into every language catalog. See po/README.md. Needs gettext and
# `cargo install slint-tr-extractor --version 1.18.1`.
set -eu
cd "$(dirname "$0")/.."
slint-tr-extractor --no-default-translation-context -o po/hwsight.pot \
    hwsight-gui/ui/app.slint hwsight-gui/ui/theme.slint hwsight-gui/ui/components/*.slint
python3 -I tools/extract-rust-translations.py po/hwsight.pot
for catalog in po/*/LC_MESSAGES/hwsight-gui.po; do
    msgmerge --quiet --update --backup=none --no-fuzzy-matching "$catalog" po/hwsight.pot
done
