#!/usr/bin/env python3
"""Check hwsight-core/data/software.toml against real distro packages and Flathub.

Reuses tools/verify-firmware.py's containers: for each target release, checks that the
package hwsight would suggest (same key rules as Distro::data_keys) really exists there,
and that every Flathub ID exists on Flathub. Run from the repo root:

    tools/verify-software.py              # every target
    tools/verify-software.py debian-12    # just one

Needs podman. Results are cached in tools/.software-cache/.
"""

import importlib.util
import sys

sys.dont_write_bytecode = True  # loading verify-firmware.py must not leave __pycache__
import json
import tomllib
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("vf", ROOT / "tools/verify-firmware.py")
vf = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vf)
vf.CACHE = ROOT / "tools/.software-cache"

DATA = ROOT / "hwsight-core/data/software.toml"


def chosen_package(entry: dict, keys: list[str]) -> str | None:
    """The same rule as the firmware lookup: the most specific key wins."""
    packages = entry.get("packages", {})
    for k in keys:
        if k in packages:
            return None if packages[k] == "none" else packages[k]
    return None


def flathub_ok(app_id: str) -> bool:
    url = f"https://flathub.org/api/v2/appstream/{app_id}"
    try:
        with urllib.request.urlopen(url, timeout=20) as r:
            return r.status == 200 and json.load(r).get("id") == app_id
    except Exception:
        return False


def main() -> int:
    entries = tomllib.loads(DATA.read_text())["software"]
    failures = 0
    for e in entries:
        if "flathub" in e and not flathub_ok(e["flathub"]):
            print(f"  FAIL flathub: {e['id']} -> {e['flathub']}: not on Flathub")
            failures += 1
    for target in sys.argv[1:] or list(vf.TARGETS):
        keys = vf.TARGETS[target][2]
        chosen = {e["id"]: chosen_package(e, keys) for e in entries}
        packages = sorted({p for p in chosen.values() if p})
        r = vf.query(target, packages, [])
        bad = 0
        for sid, pkg in chosen.items():
            if pkg and pkg not in r["exists"]:
                print(f"  FAIL {target}: {sid} -> {pkg}: no such package (set the key to \"none\" or fix the name)")
                bad += 1
        print(f"{target}: {'ok' if not bad else f'{bad} problems'}")
        failures += bad
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
