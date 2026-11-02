#!/usr/bin/env python3
"""Check hwsight-core/data/firmware.toml against real distro packages.

For each target release, a container asks the distro's own file search which packages
provide firmware under each [[firmware]] path (apt-file, dnf, pacman -F). Then, using
the same lookup rules as FirmwarePackages::package_for, we check that the package hwsight
would suggest really provides it (directly, or as a metapackage that pulls it in), and
print the right package when it doesn't. Run from the repo root:

    tools/verify-firmware.py                 # every target
    tools/verify-firmware.py debian-12       # just one

Needs podman. Downloads package metadata (and on openSUSE the packages themselves).
Results are cached in tools/.firmware-cache/; delete it to re-check newer releases.
"""

import json
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "hwsight-core/data/firmware.toml"
CACHE = ROOT / "tools/.firmware-cache"

# Every script reads $PKGS (packages the data names) and $PATHS (firmware paths), and
# prints tab-separated lines:
#   EXISTS pkg | MISSING pkg | DEP pkg dependency | PROV path package-providing-it
DEB_SCRIPT = r"""
export DEBIAN_FRONTEND=noninteractive
f=/etc/apt/sources.list.d/debian.sources
[ -f $f ] && sed -i 's/^Components: main$/Components: main contrib non-free non-free-firmware/' $f
[ -f /etc/apt/sources.list ] && grep -q '^deb .*debian' /etc/apt/sources.list && \
  sed -i 's/ main$/ main contrib non-free non-free-firmware/' /etc/apt/sources.list
f=/etc/apt/sources.list.d/ubuntu.sources
[ -f $f ] && sed -i 's/^Components: .*$/Components: main restricted universe multiverse/' $f
[ -f /etc/apt/sources.list ] && grep -q ubuntu /etc/apt/sources.list && \
  sed -i 's/ main restricted$/ main restricted universe multiverse/' /etc/apt/sources.list
apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq apt-file >/dev/null 2>&1
apt-file update >/dev/null 2>&1
for p in $PKGS; do
  if apt-cache show "$p" >/dev/null 2>&1; then
    printf 'EXISTS\t%s\n' "$p"
    apt-cache depends --no-recommends --no-suggests "$p" 2>/dev/null \
      | sed -n 's/^ *Depends: //p' | grep -v '[<>|]' \
      | while read -r d; do printf 'DEP\t%s\t%s\n' "$p" "$d"; done
  else printf 'MISSING\t%s\n' "$p"; fi
done
for path in $PATHS; do
  apt-file search --regexp "^/(usr/)?lib/firmware/$path" 2>/dev/null | cut -d: -f1 | sort -u \
    | while read -r k; do printf 'PROV\t%s\t%s\n' "$path" "$k"; done
done
"""

ARCH_SCRIPT = r"""
pacman -Sy --noconfirm >/dev/null 2>&1
pacman -Fy --noconfirm >/dev/null 2>&1
for p in $PKGS; do
  if pacman -Si "$p" >/dev/null 2>&1; then
    printf 'EXISTS\t%s\n' "$p"
    pacman -Si "$p" | sed -n 's/^Depends On *: //p' | tr ' ' '\n' | sed 's/[<>=].*//' \
      | grep -v -e '^None$' -e '^$' | while read -r d; do printf 'DEP\t%s\t%s\n' "$p" "$d"; done
  else printf 'MISSING\t%s\n' "$p"; fi
done
for path in $PATHS; do
  pacman -Fqx "^usr/lib/firmware/$path" 2>/dev/null | sed 's#.*/##' | sort -u \
    | while read -r k; do printf 'PROV\t%s\t%s\n' "$path" "$k"; done
done
"""

FEDORA_SCRIPT = r"""
q() { dnf -q repoquery --setopt=optional_metadata_types=filelists "$@" 2>/dev/null; }
for p in $PKGS; do
  if q --qf '%{name}\n' "$p" | grep -qx "$p"; then
    printf 'EXISTS\t%s\n' "$p"
    q --requires --resolve --qf '%{name}\n' "$p" | sort -u \
      | while read -r d; do printf 'DEP\t%s\t%s\n' "$p" "$d"; done
  else printf 'MISSING\t%s\n' "$p"; fi
done
for path in $PATHS; do
  q --whatprovides "/usr/lib/firmware/${path}*" --qf '%{name}\n' | sort -u \
    | while read -r k; do printf 'PROV\t%s\t%s\n' "$path" "$k"; done
done
"""

# openSUSE has no quick "which package has files under this folder" search, so download
# the named packages and read their file lists; providers are found among those only.
SUSE_SCRIPT = r"""
zypper -q --non-interactive --gpg-auto-import-keys refresh >/dev/null 2>&1
for p in $PKGS; do
  rm -rf /var/cache/zypp/packages
  if zypper -q --non-interactive download "$p" >/dev/null 2>&1; then
    printf 'EXISTS\t%s\n' "$p"
    for f in /var/cache/zypp/packages/*/*/"$p"-[0-9]*.rpm; do
      rpm -qlp "$f" 2>/dev/null | sed -n 's#^/usr/lib/firmware/##p' | while read -r l; do
        for path in $PATHS; do
          case "$l" in "$path"*) printf 'PROV\t%s\t%s\n' "$path" "$p" ;; esac
        done
      done
      break
    done
  else printf 'MISSING\t%s\n' "$p"; fi
done
"""

# target: (image, script, data keys most specific first; mirrors Distro::data_keys)
TARGETS = {
    "arch": ("docker.io/library/archlinux:latest", ARCH_SCRIPT, ["arch"]),
    "debian-12": ("docker.io/library/debian:12", DEB_SCRIPT, ["debian-bookworm", "debian"]),
    "debian-13": ("docker.io/library/debian:trixie", DEB_SCRIPT, ["debian-trixie", "debian"]),
    "ubuntu-22.04": ("docker.io/library/ubuntu:22.04", DEB_SCRIPT, ["ubuntu-jammy", "ubuntu"]),
    "ubuntu-24.04": ("docker.io/library/ubuntu:24.04", DEB_SCRIPT, ["ubuntu-noble", "ubuntu"]),
    "fedora-43": ("registry.fedoraproject.org/fedora:43", FEDORA_SCRIPT, ["fedora-43", "fedora"]),
    "opensuse-tw": (
        "registry.opensuse.org/opensuse/tumbleweed:latest",
        SUSE_SCRIPT,
        ["opensuse"],
    ),
}


# Targets whose search only covers the packages we name, so "nothing provides it" there
# may just mean we didn't look in the right package.
PARTIAL_SEARCH = {"opensuse-tw"}


def lookup(entries: list[dict], file: str, keys: list[str]) -> str | None:
    """The same rule as FirmwarePackages::package_for."""
    for e in sorted(entries, key=lambda e: len(e["path"]), reverse=True):
        if file.startswith(e["path"]):
            for k in keys:
                if k in e:
                    return None if e[k] == "none" else e[k]
    return None


def query(target: str, packages: list[str], paths: list[str]) -> dict:
    cache = CACHE / f"{target}.json"
    if cache.exists():
        cached = json.loads(cache.read_text())
        if set(cached["asked_pkgs"]) >= set(packages) and set(cached["asked_paths"]) >= set(paths):
            return cached
    image, script, _ = TARGETS[target]
    print(f"[{target}] asking {image} ...", flush=True)
    run = subprocess.run(
        [
            "podman", "run", "--rm",
            "-e", "PKGS=" + " ".join(packages),
            "-e", "PATHS=" + " ".join(paths),
            image, "sh", "-c", script,
        ],
        capture_output=True,
        text=True,
    )
    if run.returncode != 0:
        sys.exit(f"[{target}] container failed:\n{run.stderr[-2000:]}")
    result = {"asked_pkgs": packages, "asked_paths": paths, "exists": [], "deps": {}, "prov": {}}
    for line in run.stdout.splitlines():
        kind, *rest = line.split("\t")
        if kind == "EXISTS":
            result["exists"].append(rest[0])
        elif kind == "DEP" and len(rest) == 2:
            result["deps"].setdefault(rest[0], []).append(rest[1])
        elif kind == "PROV" and len(rest) == 2:
            result["prov"].setdefault(rest[0], []).append(rest[1])
    CACHE.mkdir(parents=True, exist_ok=True)
    cache.write_text(json.dumps(result))
    return result


def main() -> int:
    entries = tomllib.loads(DATA.read_text())["firmware"]
    paths = sorted({e["path"] for e in entries if e["path"]})
    failures = 0
    for target in sys.argv[1:] or list(TARGETS):
        keys = TARGETS[target][2]
        chosen = {p: lookup(entries, p, keys) for p in [""] + paths}
        packages = sorted({c for c in chosen.values() if c})
        r = query(target, packages, paths)
        bad, absent = 0, []
        for path, pkg in chosen.items():
            if pkg is None:
                continue
            if pkg not in r["exists"]:
                hint = ", ".join(r["prov"].get(path, [])) or "nothing"
                print(f"  FAIL {target}: '{path or '(fallback)'}' -> {pkg}: no such package (provided by: {hint})")
                bad += 1
                continue
            if not path:
                continue
            providers = set(r["prov"].get(path, []))
            if not providers:
                if target in PARTIAL_SEARCH:
                    absent.append(path)
                else:
                    # Nothing in this release has it, so suggesting a package is wrong.
                    print(f"  FAIL {target}: '{path}' -> {pkg}: not packaged in this release; set its key to \"none\"")
                    bad += 1
                continue
            if pkg in providers or providers & set(r["deps"].get(pkg, [])):
                continue
            print(f"  FAIL {target}: '{path}' -> {pkg}: wrong package (provided by: {', '.join(sorted(providers))})")
            bad += 1
        note = f"; not shipped in this release: {' '.join(absent)}" if absent else ""
        print(f"[{target}] {'ok' if bad == 0 else f'{bad} problem(s)'}{note}", flush=True)
        failures += bad
    print("all good" if failures == 0 else f"{failures} problem(s) in total")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
