# hwsight work plan

Tasks are marked **Copilot** (well-specified, pattern-following) or **Claude** (design,
diagnosis logic, security, reviews). Claude reviews every Copilot task before it's pushed.

## Claude 1: PCI diagnosis edge cases (done 2026-11-02)

Blocked drivers (modprobe.d `blacklist` / `install ... /bin/false`, kernel command line),
devices reserved for virtual machines (vfio-pci, pci-stub).

## Copilot 1: USB ID names (`usb_ids.rs`)

**Goal:** names for USB vendors, products, and classes, loaded the same layered way as
PCI names. The USB scanner will use them later; you do not wire them into it.

**Files you may change:** `hwsight-core/src/usb_ids.rs` (new), `hwsight-core/src/lib.rs`
(two lines only, step 5), `hwsight-core/src/update.rs` (step 6 only), and this task's
Done notes. Nothing else.

**Steps (do all of them):**

1. Read `hwsight-core/src/pci_ids.rs` completely before writing anything. Your file must
   follow its structure: a private parsed database struct, a public `UsbNames` with
   `load(&DataDirs)` (uses `dirs.id_files("usb.ids")`), `from_files(&[PathBuf])`,
   `embedded()`, and `sources() -> Vec<IdsSource>` (reuse `crate::pci_ids::IdsSource`).
   Files are ordered newest first by their `Version:` header (call
   `crate::pci_ids::header_version`), and an ID missing from the newest file is looked
   up in the older ones.
2. Public lookups, each returning `Option<String>`:
   `vendor(vid: u16)`, `product(vid: u16, pid: u16)`,
   `class(class: u8, subclass: u8, protocol: u8)` (most specific name known: protocol,
   else subclass, else class).
3. The `usb.ids` format: vendor lines `vvvv  Name`; product lines `\tpppp  Name`; class
   section lines `C cc  Name`, `\tss  Name`, `\t\tpp  Name`. After the class section come
   other sections whose top-level lines start with `AT`, `HID`, `R`, `BIAS`, `PHY`, `HUT`,
   `L`, `HCC`, or `VT`. Ignore those sections entirely: none of their indented lines may
   be stored as products or classes. Malformed lines are skipped, never fatal.
4. `embedded()` has no built-in database yet: it returns a `UsbNames` with no files, so
   every lookup returns `None`. Write exactly one comment there:
   `// TODO: embed a usb.ids snapshot as a last-resort fallback.`
5. In `lib.rs` add `pub mod usb_ids;` and `pub use usb_ids::UsbNames;`. Change nothing
   else in `lib.rs`.
6. Add `pub(crate) fn looks_valid(text: &str) -> bool` to `usb_ids.rs` (true when the text
   parses with at least one vendor). In `update.rs`, in `file_version`, make the
   `"usb.ids"` branch call it and return `Err("usb.ids has no devices".into())` when it is
   false, exactly like the `"pci.ids"` branch. Change nothing else in `update.rs`.

**Tests (all required, in `usb_ids.rs` unless noted):**

- `parses_vendors_and_products`: a sample with two vendors and three products; check
  every name.
- `class_lookup_is_most_specific`: class only, class+subclass, class+subclass+protocol,
  and an unknown protocol falling back to the subclass name.
- `other_sections_are_ignored`: a sample with lines from `AT`, `HID`, `HUT`, and `L`
  sections placed after the class section; assert none of them became vendors,
  products, or classes, and that the counts are exactly what the sample defines.
- `newest_file_wins_and_older_files_fill_gaps`: two temp files with different
  `Version:` headers, passed in the wrong order; the newer name wins and an ID only in
  the older file is still found; `sources()[0]` is the newer file.
- `malformed_lines_are_skipped`: bad hex, missing names, orphan product lines.
- `real_system_file_if_present`: if `/usr/share/hwdata/usb.ids` exists, load it and
  assert vendor `0x1d6b` is `"Linux Foundation"`; if it doesn't exist, return early
  (this is the only test allowed to read a real system file).
- In `update.rs`'s tests: `usb_ids_without_devices_is_rejected`:
  `file_version("usb.ids", b"# Version: 2026.01.01\n")` returns `Err`.

**Done when:**

- [ ] `usb_ids.rs` exists with every function in steps 1, 2, 4, and 6
- [ ] `lib.rs` and `update.rs` changed exactly as steps 5 and 6 say, nothing more
- [ ] all seven tests above exist and pass
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`
      all pass with zero warnings (paste the final `test result:` lines below)
- [ ] nothing committed

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

**Do this only after Copilot 1 is reviewed and committed.**

**Goal:** easier-to-scan `hwsight` output in a terminal.

**Files you may change:** `hwsight-cli/src/main.rs` and this task's Done notes. Nothing else.

**Steps (do all of them):**

1. Add `fn use_color(is_terminal: bool, no_color_env: Option<&str>, flag_off: bool) -> bool`:
   true only when `is_terminal` is true, `no_color_env` is `None` or empty, and `flag_off`
   is false (rules from no-color.org).
2. Add a `--no-color` option and list it in `HELP` in the same style as the others.
3. In `main`, compute colour with `std::io::IsTerminal` on stdout and
   `std::env::var("NO_COLOR").ok()`, and pass it to `print_text`. `--json` output is
   never coloured.
4. Use plain ANSI escape codes (no new crate): status symbol green `\x1b[32m` for
   working, yellow `\x1b[33m` for `!`, red `\x1b[31m` for `✗`, dim `\x1b[2m` for
   "no driver needed" and the reserved-for-VM `•`; category headings and the
   "USB ports" heading bold `\x1b[1m`; the `Why:` and `Fix:` labels bold. Every coloured
   piece ends with `\x1b[0m`.
5. Put the escape codes in one small helper (for example `fn paint(text, code, on) ->
   String`) so that when colour is off the output is byte-for-byte what it is today.

**Tests (all required, in `main.rs`):**

- `color_rules`: every combination of the three inputs to `use_color` (8 cases).
- `paint_off_is_plain`: with colour off, `paint` returns the text unchanged.
- `paint_on_wraps_and_resets`: with colour on, the result starts with the code and ends
  with `\x1b[0m`.

**Done when:**

- [ ] steps 1 to 5 done; `--no-color` in `HELP`
- [ ] the three tests exist and pass
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`
      all pass with zero warnings (paste the final `test result:` lines below)
- [ ] running `hwsight --no-color` prints exactly what `hwsight` printed before this task
- [ ] nothing committed

**Done notes:**

## Claude 4: verify firmware.toml for other distros (done 2026-11-02)

`tools/verify-firmware.py` asks each release's own file search (apt-file, dnf,
pacman -F; openSUSE by downloading packages) which package provides each path, and
checks hwsight's choice. Arch, Debian 12 and 13, Ubuntu 22.04 and 24.04, Fedora 43, and
openSUSE Tumbleweed all pass. It found 23 wrong names, all fixed. Release-specific keys
(`debian-bookworm`, `ubuntu-jammy`, `ubuntu-noble`) and `"none"` for firmware a release
doesn't package. `tools/release-data.sh` refuses to publish if the check fails.

## Claude 7: signed data updates (done 2026-11-02, waiting for the data repo)

`update.rs`, `hwsight --update-data`, `tools/hwsight-data-tool`, `tools/release-data.sh`.
Keys: `~/.local/share/hwsight-signing/{main,backup}.key` (backup to go offline).
Left: josh creates the public `sweatshopking/hwsight-data` repo (local copy ready in
`~/Projects/hwsight-data`), then run `tools/release-data.sh` and test
`hwsight --update-data` end to end. Automatic daily check belongs in the GUI.

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
