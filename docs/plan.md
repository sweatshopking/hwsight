# hwsight work plan

Tasks are marked **Copilot** (well-specified, pattern-following) or **Claude** (design,
diagnosis logic, security, reviews). Claude reviews every Copilot task before it's pushed.

## Claude 1: PCI diagnosis edge cases (done 2026-11-02)

Blocked drivers (modprobe.d `blacklist` / `install ... /bin/false`, kernel command line),
devices reserved for virtual machines (vfio-pci, pci-stub).

## Copilot 1: USB ID names (`usb_ids.rs`)

**Goal:** names for USB vendors, products, and classes, loaded the same layered way as
PCI names, ready for the USB scanner.

**Do:**
1. Create `hwsight-core/src/usb_ids.rs` modelled closely on `pci_ids.rs`. Read that file
   first and follow its structure: a private parsed database, `UsbNames` with
   `load(&DataDirs)` (uses `dirs.id_files("usb.ids")`), `from_files(&[PathBuf])`,
   `embedded()`, `sources()`, newest file first by its `Version:` header (reuse
   `pci_ids::header_version`), and older files filling gaps per ID.
2. Lookups: `vendor(vid) -> Option<String>`, `product(vid, pid) -> Option<String>`,
   `class(class, subclass, protocol) -> Option<String>` (most specific name known).
3. `usb.ids` format: vendor lines `vvvv  Name`, product lines `\tpppp  Name`; class
   section lines `C cc  Name`, `\tss  Name`, `\t\tpp  Name`. After the class section
   come other sections (`AT`, `HID`, `R`, `BIAS`, `PHY`, `HUT`, `L`, `HCC`, `VT`):
   ignore all of them. Their indented lines must not be parsed as products or classes.
4. Embedded fallback: there's no crate yet, so `embedded()` has no names (every lookup
   returns `None`). Leave a `// TODO: embedded usb.ids` comment where it would go. Don't
   add a dependency.
5. Add `pub mod usb_ids;` and `pub use usb_ids::UsbNames;` in `lib.rs`. Don't wire it
   into `Scanner` (the USB scanner task will).

**Tests** (in the file): vendors/products parse; class/subclass/protocol most-specific
lookup; lines in the `AT`/`HID`/`L` sections are ignored; newest file wins and older
files fill gaps; malformed lines are skipped; real-file smoke test that loads
`/usr/share/hwdata/usb.ids` *only if it exists* and checks vendor `1d6b` is
"Linux Foundation" (skip silently otherwise).

**Done notes:**

## Claude 2: USB scanner

Read `/sys/bus/usb/devices`, devices and interfaces (drivers bind per interface), hubs,
root hubs. Uses `UsbNames` from Copilot 1.

## Claude 3: explain failures from the kernel log (done 2026-11-02)

Why a driver didn't start; missing firmware mapped to a package per distro
(`firmware.toml`, distro detection, `--kernel-log FILE`).

## Copilot 2: colour in the CLI

**Goal:** easier-to-scan `hwsight` output in a terminal.

**Do:** in `hwsight-cli/src/main.rs` only. Colour the status symbol and the "Why:"/"Fix:"
labels with plain ANSI escape codes (no new dependency): green for working, yellow for
`!`, red for `✗`, dim for "no driver needed" and the reserved-for-VM `•`. Category
headings bold. Only colour when stdout is a terminal (`std::io::IsTerminal`) and the
`NO_COLOR` environment variable is unset or empty (see no-color.org). Add `--no-color`
to turn it off and list it in `HELP`. Never colour `--json` output.

**Tests:** move the colour decision into a small pure function
`fn use_color(is_terminal: bool, no_color_env: Option<&str>, flag: bool) -> bool` and test
it; test that the text with colour off is byte-for-byte what it is today.

**Done notes:**

## Claude 4: verify firmware.toml for other distros

Check Debian, Ubuntu, Fedora, and openSUSE package names in containers (dnf repoquery,
apt-file, zypper search --provides). Needs a few hundred MB of downloads: ask josh
first (Wi-Fi only, shares bandwidth with Plex).

## Claude 5: "driver attached but not working" checks

A bound driver doesn't always mean a working device (e.g. Wi-Fi driver loaded but its
firmware failed, so there's no network interface). Per device type, check the interface
the driver should create (`net/`, `sound/`, `drm/`, `nvme/`...), as data in rules.toml.
