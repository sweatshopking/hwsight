#!/usr/bin/env bash
# Installs a built package on a fresh system the way a user would, then checks that the
# command line runs and that the window opens and draws. Run as root in a clean container:
#
#   packaging/smoke-test.sh deb        on Ubuntu/Debian   (installs dist/*.deb with apt)
#   packaging/smoke-test.sh rpm        on Fedora/openSUSE (installs dist/*.rpm with dnf)
#   packaging/smoke-test.sh appimage   on Ubuntu          (runs dist/*.AppImage)
#
# Saves a screenshot of the first window as dist/smoke-<kind>-<arch>.png.
# Adapted from DriveDeck's packaging.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="$ROOT/dist"
ARCH="$(uname -m)"
case "$ARCH" in
    x86_64) DEB_ARCH=amd64 ;;
    aarch64) DEB_ARCH=arm64 ;;
esac
kind="${1:-}"
log() { printf '\n==> %s\n' "$*"; }

# Only test tools are installed here; everything the app needs must come from the package.
cli=()
case "$kind" in
    deb)
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq
        log "Installing the .deb with apt"
        apt-get install -y -qq "$DIST"/hwsight_*_"$DEB_ARCH".deb >/dev/null
        apt-get install -y -qq --no-install-recommends xvfb imagemagick >/dev/null
        app=(hwsight-gui)
        cli=(hwsight)
        ;;
    rpm)
        log "Installing the .rpm with dnf"
        dnf install -y -q "$DIST"/hwsight-*."$ARCH".rpm >/dev/null
        dnf install -y -q xorg-x11-server-Xvfb ImageMagick >/dev/null
        app=(hwsight-gui)
        cli=(hwsight)
        ;;
    appimage)
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq
        # What every Linux desktop already has, and AppImages deliberately take from the system
        # rather than bundling (fonts, X11/Wayland and EGL must match the desktop), plus the
        # test tools. libxkbcommon(-x11) is left out on purpose: the AppImage must bring it.
        apt-get install -y -qq --no-install-recommends fontconfig fonts-dejavu-core \
            libx11-6 libx11-xcb1 libxcursor1 libxrandr2 libxi6 \
            libwayland-client0 libwayland-cursor0 libwayland-egl1 libegl1 \
            xvfb imagemagick >/dev/null
        image=("$DIST"/hwsight-*-"$ARCH".AppImage)
        chmod +x "${image[0]}"
        # containers have no FUSE; on a desktop the AppImage mounts itself instead
        export APPIMAGE_EXTRACT_AND_RUN=1
        app=("${image[0]}")
        ;;
    *)
        echo "usage: $0 deb|rpm|appimage" >&2
        exit 2
        ;;
esac

export XDG_RUNTIME_DIR=/tmp/smoke-runtime HOME=/tmp/smoke-home HWSIGHT_NO_UPDATE=1
rm -rf "$XDG_RUNTIME_DIR" "$HOME"
mkdir -p -m 700 "$XDG_RUNTIME_DIR" "$HOME"

if [ ${#cli[@]} -gt 0 ]; then
    log "Command line"
    "${cli[@]}" --version
    "${cli[@]}" --json >/tmp/smoke-scan.json
    python3 -c 'import json,sys; json.load(open("/tmp/smoke-scan.json"))' 2>/dev/null \
        || head -c 300 /tmp/smoke-scan.json
    # the menu entry and icon must be installed
    test -f /usr/share/applications/hwsight.desktop
    test -f /usr/share/icons/hicolor/256x256/apps/hwsight.png
fi

log "Opening the window"
Xvfb :99 -screen 0 1280x800x24 >/dev/null 2>&1 &
xvfb=$!
sleep 2
DISPLAY=:99 "${app[@]}" >/tmp/smoke-app.log 2>&1 &
pid=$!
shot="$DIST/smoke-$kind-$ARCH.png"
# the window is dark navy, the empty Xvfb screen is pure black: wait until it has drawn
for _ in $(seq 30); do
    sleep 1
    kill -0 "$pid" 2>/dev/null || { cat /tmp/smoke-app.log; echo "error: the app exited" >&2; exit 1; }
    DISPLAY=:99 import -window root "$shot"
    brightness="$(convert "$shot" -colorspace Gray -format '%[fx:mean]' info:)"
    if awk -v b="$brightness" 'BEGIN { exit !(b > 0.04) }'; then
        log "Window drawn (brightness $brightness), screenshot: $shot"
        kill "$pid" "$xvfb"
        exit 0
    fi
done
cat /tmp/smoke-app.log
echo "error: no window appeared within 30 seconds" >&2
exit 1
