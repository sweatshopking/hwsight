#!/usr/bin/env bash
# Publish a signed hwsight data release to github.com/sweatshopking/hwsight-data.
#
# Takes rules.toml and firmware.toml from hwsight-core/data, fetches the newest pci.ids
# and usb.ids from upstream, signs a manifest with the main key, checks the signature
# against the keys built into hwsight, and uploads everything as the latest release.
#
#   tools/release-data.sh            # build, sign, verify, publish
#   tools/release-data.sh --dry-run  # build, sign, verify only
set -euo pipefail
cd "$(dirname "$0")/.."

KEY="${HWSIGHT_SIGNING_KEY:-$HOME/.local/share/hwsight-signing/main.key}"
REPO="sweatshopking/hwsight-data"
OUT="tools/.release"

python3 tools/verify-firmware.py   # never publish firmware data that fails the check

rm -rf "$OUT" && mkdir -p "$OUT"
cp hwsight-core/data/rules.toml hwsight-core/data/firmware.toml \
    hwsight-core/data/descriptions.toml hwsight-core/data/fixes.toml "$OUT/"
curl -fsSL --retry 3 https://pci-ids.ucw.cz/v2.2/pci.ids -o "$OUT/pci.ids"
curl -fsSL --retry 3 https://usb-ids.gowdy.us/usb.ids -o "$OUT/usb.ids"

cargo build -q --release -p hwsight-data-tool
./target/release/hwsight-data-tool release "$OUT" "$KEY"
./target/release/hwsight-data-tool verify "$OUT"

if [[ "${1:-}" == "--dry-run" ]]; then
    echo "dry run: release files are in $OUT"
    exit 0
fi

tag="data-$(date -u +%Y.%m.%d)"
n=1
while gh release view "$tag" -R "$REPO" >/dev/null 2>&1; do
    n=$((n + 1))
    tag="data-$(date -u +%Y.%m.%d)-$n"
done
gh release create "$tag" "$OUT"/* -R "$REPO" --latest \
    --title "Data $tag" \
    --notes "Device data for hwsight. Signed: hwsight checks manifest.toml.sig before using anything here."
echo "published $tag"
