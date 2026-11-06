#!/usr/bin/env python3
"""Check hwsight-core/data/drivers.toml against real distro packages and the AUR.

Reuses tools/verify-firmware.py's containers: for each target release, checks that the
repo package hwsight would suggest (same key rules as Distro::data_keys) really exists
there. `aur:` packages are checked against the AUR itself. Also checks that every
hardware ID is a real ID in the installed pci.ids / usb.ids. Run from the repo root:

    tools/verify-drivers.py              # every target
    tools/verify-drivers.py debian-12    # just one

Needs podman. Results are cached in tools/.drivers-cache/.
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
vf.CACHE = ROOT / "tools/.drivers-cache"

DATA = ROOT / "hwsight-core/data/drivers.toml"


def chosen_package(entry: dict, keys: list[str]) -> str | None:
    """The same rule as Drivers::source: the most specific key wins."""
    packages = entry.get("packages", {})
    for k in keys:
        if k in packages:
            return None if packages[k] == "none" else packages[k]
    return None


def aur_ok(name: str) -> bool:
    url = f"https://aur.archlinux.org/rpc/v5/info?arg[]={name}"
    try:
        with urllib.request.urlopen(url, timeout=20) as r:
            return json.load(r).get("resultcount") == 1
    except Exception:
        return False


def id_known(bus: str, vendor: str, device: str) -> bool:
    for base in ("/usr/share/hwdata", "/usr/share/misc"):
        path = Path(base) / ("usb.ids" if bus == "usb" else "pci.ids")
        if path.exists():
            in_vendor = False
            for line in path.read_text(errors="replace").splitlines():
                if line and not line.startswith(("\t", "#")):
                    in_vendor = line.startswith(vendor)
                elif in_vendor and line.startswith("\t") and not line.startswith("\t\t"):
                    if line.split()[0] == device:
                        return True
            return False
    return True  # no ID list on this machine: can't check


def main() -> int:
    entries = tomllib.loads(DATA.read_text())["driver"]
    failures = 0
    for e in entries:
        for m in e["match"]:
            if not id_known(m.get("bus", "pci"), m["vendor"], m["device"]):
                print(f"  FAIL id: {e['id']} -> {m['vendor']}:{m['device']} is not in the ID list")
                failures += 1
        for family, pkg in e.get("packages", {}).items():
            if pkg.startswith("aur:") and not aur_ok(pkg[4:]):
                print(f"  FAIL aur: {e['id']} -> {pkg[4:]}: not on the AUR")
                failures += 1
    for target in sys.argv[1:] or list(vf.TARGETS):
        keys = vf.TARGETS[target][2]
        chosen = {e["id"]: chosen_package(e, keys) for e in entries}
        packages = sorted({p for p in chosen.values() if p and not p.startswith("aur:")})
        r = vf.query(target, packages, [])
        bad = 0
        for did, pkg in chosen.items():
            if pkg and not pkg.startswith("aur:") and pkg not in r["exists"]:
                print(f"  FAIL {target}: {did} -> {pkg}: no such package (set the key to \"none\" or fix the name)")
                bad += 1
        print(f"{target}: {'ok' if not bad else f'{bad} problems'}")
        failures += bad
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
