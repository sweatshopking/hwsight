#!/usr/bin/env bash
# Every check a task must pass. Run from anywhere: tools/check.sh
# Prints PASS or FAIL for each check and ends with "ALL CHECKS PASSED" only if every
# one passed. Exit code 0 means all passed.
set -uo pipefail
cd "$(dirname "$0")/.."

failed=0
pass() { printf 'PASS  %s\n' "$1"; }
fail() { printf 'FAIL  %s\n' "$1"; [[ -n ${2:-} ]] && printf '%s\n' "$2" | sed 's/^/      /'; failed=1; }
run() {  # run <name> <command...>
    local name=$1; shift
    local out
    if out=$("$@" 2>&1); then pass "$name"; else fail "$name" "$(printf '%s\n' "$out" | tail -25)"; fi
}

run "cargo fmt --check" cargo fmt --check
run "cargo clippy (no warnings)" cargo clippy --all-targets -- -D warnings
run "cargo test" cargo test -q
run "GUI builds" cargo build -q -p hwsight-gui

# The app must open and still be running when the timeout closes it (exit code 124).
if [[ -n ${WAYLAND_DISPLAY:-}${DISPLAY:-} ]]; then
    timeout 6 target/debug/hwsight-gui >/dev/null 2>&1
    code=$?
    if [[ $code -eq 124 ]]; then pass "GUI opens and stays open"; else fail "GUI opens and stays open" "exit code $code (expected 124: it crashed or quit)"; fi
else
    fail "GUI opens and stays open" "no display available to run the app"
fi

# ---- GUI rules (docs/plan.md, "GUI batch") ----
ui=hwsight-gui/ui
comp=$ui/components

hits=$(grep -rnE '#[0-9a-fA-F]{3,8}\b' "$comp" 2>/dev/null)
[[ -z $hits ]] && pass "no hex colours in components" || fail "no hex colours in components" "$hits"

hits=$(grep -rnE 'global Theme|struct DeviceItem' "$ui" | grep -v "^$ui/theme.slint:")
[[ -z $hits ]] && pass "Theme and DeviceItem defined only in ui/theme.slint" || fail "Theme and DeviceItem defined only in ui/theme.slint" "$hits"

hits=$(grep -rnE 'from "[^"]*app\.slint"' "$comp" 2>/dev/null)
[[ -z $hits ]] && pass "components never import app.slint" || fail "components never import app.slint" "$hits"

hits=$(grep -rnE 'in(-out)? property <(color|brush)>' "$comp" 2>/dev/null)
[[ -z $hits ]] && pass "no colours passed into components as properties (use Theme)" || fail "no colours passed into components as properties (use Theme)" "$hits"

# Every `for ... : Rectangle {` must set a height within its first 3 lines.
hits=$(awk '
    function report() { if (pending > 0) print where; pending = 0 }
    FNR == 1 { report() }
    /for .* in .*: *Rectangle *\{/ { report(); pending = 3; where = FILENAME ":" FNR ": " $0; next }
    pending > 0 { if ($0 ~ /height:/) pending = 0; else if (--pending == 0) print where }
    END { report() }
' "$ui"/*.slint "$comp"/*.slint 2>/dev/null)
[[ -z $hits ]] && pass "every list item sets a height" || fail "every list item sets a height" "$hits"

echo
if [[ $failed -eq 0 ]]; then echo "ALL CHECKS PASSED"; exit 0; else echo "SOME CHECKS FAILED"; exit 1; fi
