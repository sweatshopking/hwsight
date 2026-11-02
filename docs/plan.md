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

## Claude 2: USB scanner (done 2026-11-02, names integration waits for Copilot 1)

Done: devices + interfaces, any bound interface = working (vendor-specific interfaces
used by apps, e.g. RGB/cooler controllers, aren't flagged), driver candidates per
interface, USBGuard-blocked devices, categories from interface classes then from what
the driver created, per-bus rules (`bus = "usb"`), devices that couldn't connect and
over-current ports from the kernel log (`port_issues`).

Left for after Copilot 1:
- Wire `UsbNames` in: vendor name when the device gives none; prefer the database
  product name when the device's own string is generic ("Controller",
  "Wireless_Device", "USB Receiver").
- Devices whose only bound interface is HID but that are really coolers or lighting
  (NZXT Kraken, ASUS AURA) land in "Input devices": add data rules for category
  overrides by vendor:product.
- A device with only vendor-specific interfaces and no driver is `NoDriver`; if it's
  actually app-driven, add a `bus = "usb"` vendor/device `no_driver_needed` rule.

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

## Claude 5: "driver attached but not working" checks (done 2026-11-02)

A bound driver doesn't always mean a working device (e.g. Wi-Fi driver loaded but its
firmware failed, so there's no network interface). Per device type, check the interface
the driver should create (`net/`, `sound/`, `drm/`, `nvme/`...), as data in rules.toml.

## Claude 6: RGB and LED diagnosis and control (planned, owner asked 2026-11-02)

RGB on Linux is hit or miss because every maker uses its own protocol. Don't write
protocols ourselves (thousands of devices, real risk of bricking controllers). Instead:

1. **Kernel LEDs** (`/sys/class/leds`): keyboard backlights, laptop lights, controller
   LEDs (e.g. `xpad0`), lock-key lights. Standard, no extra software. Brightness via
   UPower's D-Bus API where it applies (no root), otherwise polkit.
2. **OpenRGB** (GPL-2.0, supports most RGB hardware): talk to its SDK server over its
   network protocol (port 6742, separate program, so no licence mixing). Basic control:
   list devices, set a colour/mode, turn off. Check a Rust client crate's licence first.
3. **Diagnosis, the part nobody does:** why RGB isn't detected or won't change.
   - OpenRGB not installed / SDK server not running.
   - Missing udev rules (`60-openrgb.rules`), so normal users can't reach the device.
   - `i2c-dev` not loaded, user can't open `/dev/i2c-*` (RAM and motherboard RGB on SMBus).
   - Boards whose firmware blocks SMBus access (`acpi_enforce_resources=lax` advice,
     explained carefully, never applied automatically).
   - Other software holding the device (liquidctl, OpenRazer, vendor tools in Wine).
   - Known-quirky controllers as data (`rgb.toml`, updatable), e.g. Gigabyte RGB Fusion 2
     USB (ITE 048d:5702, on the owner's LVbox B650M board) that changed colour once and
     then ignored further commands with OpenRGB.
