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

- [x] `usb_ids.rs` exists with every function in steps 1, 2, 4, and 6
- [x] `lib.rs` and `update.rs` changed exactly as steps 5 and 6 say, nothing more
- [x] all seven tests above exist and pass
- [x] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`
      all pass with zero warnings (paste the final `test result:` lines below)
- [x] nothing committed

**Done notes:**
- Claude review (2026-11-03): accepted with fixes. Fixed protocols of a broken
  subclass line attaching to the previous subclass; strengthened tests (every name,
  real-format AT/HUT/L sections, real-file product and class checks, new
  broken-subclass test); doc comments; moved `pub use`. Committed at the user's request;
  its co-author line was removed before pushing.
- `hwsight-core/src/usb_ids.rs`: Added layered USB vendor, product, and class name
  parsing, source reporting, validity checking, and all six USB ID tests.
- `hwsight-core/src/lib.rs`: Exported the `usb_ids` module and `UsbNames`.
- `hwsight-core/src/update.rs`: Rejected `usb.ids` files that contain no vendors and
  added `usb_ids_without_devices_is_rejected`.
- Test result: `test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s`
- Test result: `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`
- Test result: `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`
- Test result: `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`

## Claude 2: USB scanner (done 2026-11-02; usb.ids names wired in 2026-11-03)

Done: devices + interfaces, any bound interface = working (vendor-specific interfaces
used by apps, e.g. RGB/cooler controllers, aren't flagged), driver candidates per
interface, USBGuard-blocked devices, categories from interface classes then from what
the driver created, per-bus rules (`bus = "usb"`), devices that couldn't connect and
over-current ports from the kernel log (`port_issues`).

Names (done 2026-11-03): device's own name when specific, usb.ids name when the
device's is generic (`usb_generic_names` in rules.toml) or missing, else a
description ("MediaTek Bluetooth device").

Still left:
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

- [x] steps 1 to 5 done; `--no-color` in `HELP`
- [x] the three tests exist and pass
- [x] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`
      all pass with zero warnings (paste the final `test result:` lines below)
- [x] running `hwsight --no-color` prints exactly what `hwsight` printed before this task
- [x] nothing committed

**Done notes:**
- Claude review (2026-11-03): accepted. Verified `--no-color` output byte-identical to
  the previous build (default, --problems, --ids); colour only in a real terminal, none
  when piped or with NO_COLOR. Removed one stray blank line outside the task. Committed
  at the user's request; co-author line removed before pushing.
- `hwsight-cli/src/main.rs`: Added terminal-aware color selection, `--no-color`,
  ANSI styling for statuses, headings, and labels, while preserving plain output when
  color is disabled.
- `hwsight-cli/src/main.rs`: Added `color_rules`, `paint_off_is_plain`, and
  `paint_on_wraps_and_resets`.
- Test result: `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`
- Test result: `test result: ok. 70 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s`
- Test result: `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`
- Test result: `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`

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
Live: https://github.com/sweatshopking/hwsight-data, first release data-2026.11.02.
End-to-end tested 2026-11-03 (downloads only newer files; second run "up to date"; scan
uses the download first). Found and fixed an exact-size limit bug in the HTTPS fetch.
Automatic daily check belongs in the GUI.

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

---

# GUI batch (2026-11-03)

The skeleton is in `hwsight-gui/` (Claude). The Copilot tasks below build the window on
top of it, **one at a time, in order**: each depends on the one before.

Rules for every GUI task, in addition to `.github/copilot-instructions.md`:

- **Every colour, size, radius, gap, and font size comes from `Theme`** in
  `ui/theme.slint` (for example `Theme.surface`, `Theme.radius`, `Theme.font-body`).
  Never write a hex colour or a pixel size for these in a component. Fixed sizes the task
  gives in px (like a row height) are fine.
- `Theme` and the shared structs (`DeviceItem`) live in `ui/theme.slint`. Components
  import them with `import { Theme, DeviceItem } from "../theme.slint";`. Never import
  from `app.slint` (it imports the components, so that would be a loop), and never
  define `Theme` or `DeviceItem` anywhere else.
- Put each component in its own file under `hwsight-gui/ui/components/` and `import` it
  into `ui/app.slint`. Components must not read `AppWindow` properties directly: pass
  data in through `in` properties and report clicks through callbacks.
- Items inside a `ListView` or a `for` loop need an explicit `height`; children placed
  with `if` don't give their parent a size, so without one every item is 0px tall and
  they draw on top of each other.
- Text a user reads follows the plain-words rule. No emoji, no icons from fonts.
- Long text must never overflow: use `overflow: elide;` on single lines and
  `wrap: word-wrap;` on paragraphs.
- Besides the three usual checks, run the app and confirm it opens and stays open:
  `cargo build -p hwsight-gui && timeout 6 target/debug/hwsight-gui; echo $?` must print
  `124` (still running when the timeout closed it). Any other number means it crashed.

## Copilot 3: device list

**Files you may change:** new `hwsight-gui/ui/components/status-dot.slint`,
`category-header.slint`, `device-row.slint`; the layout part of `hwsight-gui/ui/app.slint`
(below the `// ---- Placeholder layout` comment only); this task's Done notes.

**Steps (do all of them):**

1. `StatusDot`: `in property <string> kind` ("ok", "warn", "error", "neutral"). A circle
   10px wide, filled with `Theme.ok` / `Theme.warn` / `Theme.error` / `Theme.neutral`. For
   "warn" and "error" add a glow: `drop-shadow-blur: 8px;` with `drop-shadow-color` set to
   the same colour with `.with-alpha(0.6)`.
2. `CategoryHeader`: `in property <string> text`, `in property <int> count`. Height 40px.
   The text in `Theme.font-small`, weight 700, `Theme.text-muted`, uppercase (use the
   text as given; the Rust side may pass it mixed-case, so convert with
   `text.to-uppercase()`), letter-spacing 1px, aligned to the bottom with 8px below it.
   On the right, a pill showing the count: height 18px, horizontal padding 8px, radius
   9px, background `Theme.surface-raised`, text `Theme.font-small` in `Theme.text-faint`.
3. `DeviceRow`: `in property <DeviceItem> device`, `in property <bool> selected`,
   `callback clicked()`. Height 52px, radius `Theme.radius-small`.
   - Background: `Theme.surface-raised` when selected, `Theme.surface-hover` when the
     mouse is over it (use a `TouchArea` and its `has-hover`), otherwise transparent.
     Animate the background change over 120ms (`animate background { duration: 120ms; }`).
   - When selected, a bar on the left edge: 3px wide, full height minus 12px, centred,
     radius 2px, filled with
     `@linear-gradient(180deg, Theme.accent-start 0%, Theme.accent-end 100%)`.
   - Left padding 14px: a `StatusDot` for `device.status-kind`, then 12px gap, then two
     lines of text stacked:
     line 1: `device.name`, `Theme.font-body`, weight 600, `Theme.text`, elided;
     line 2: `device.vendor` and `device.status-text` joined by "  ·  " (if vendor is
     empty show only the status text), `Theme.font-small`, `Theme.text-muted`, elided.
     When `status-kind` is "warn" or "error", line 2 uses `Theme.warn` / `Theme.error`.
   - Clicking anywhere on the row calls `clicked()`.
4. In `app.slint`, replace the placeholder `ScrollView` list with a left panel:
   a `Rectangle` with background `Theme.surface`, radius `Theme.radius`,
   `horizontal-stretch: 2`, `min-width: 340px`, containing a `ListView` (from
   `std-widgets.slint`) over `root.rows` with 8px padding inside the panel. Each row shows
   a `CategoryHeader` when `row.is-header`, otherwise a `DeviceRow` for
   `root.devices[row.device]` with `selected: root.selected == row.device`, and
   `clicked => { root.selected = row.device; }`.
5. To the right of the panel (same `HorizontalLayout`, `spacing: Theme.gap`), add a
   placeholder `Rectangle` with background `Theme.surface`, radius `Theme.radius`,
   `horizontal-stretch: 3`, and a centred `Text` "Select a device to see its details" in
   `Theme.text-faint`. Copilot 4 replaces it. Keep the existing header row as it is.

**Done when:**

- [x] the three component files exist and are used by `app.slint`
- [x] no hex colours or `Theme`-covered sizes written inside the components
- [x] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` all
      pass with zero warnings; the app-opens check prints `124`
- [x] Done notes filled in (files, what each does, final `test result:` lines)
- [x] nothing pushed, and no co-author line

**Done notes:**
- `hwsight-gui/ui/components/status-dot.slint`: Added status-colored dots and
  warning/error glow styling, with theme values passed in from `AppWindow`.
- `hwsight-gui/ui/components/category-header.slint`: Added uppercase category headings
  with device-count pills.
- `hwsight-gui/ui/components/device-row.slint`: Added selectable, hoverable device rows
  with status, vendor, and plain status text.
- `hwsight-gui/ui/app.slint`: Replaced the placeholder list with the grouped device
  list panel and the Copilot 4 details placeholder.
- Test result: `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`
- Test result: `test result: ok. 72 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s`
- Test result: `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`
- App-opens check: `124`

## Copilot 4: details panel

**Only after Copilot 3 is reviewed.**

**Files you may change:** new `hwsight-gui/ui/components/details-panel.slint`,
`status-pill.slint`; the right-hand placeholder in `hwsight-gui/ui/app.slint`; this
task's Done notes.

**Steps (do all of them):**

1. `StatusPill`: `in property <string> kind`, `in property <string> text`. Height 26px,
   horizontal padding 12px, radius 13px. Background is the kind's colour (`Theme.ok`,
   `Theme.warn`, `Theme.error`, `Theme.neutral`) `.with-alpha(0.18)`; a 1px border in the
   same colour `.with-alpha(0.5)`; text `Theme.font-small`, weight 600, in the full colour.
   A `StatusDot` (from Copilot 3) 8px before the text.
2. `DetailsPanel`: `in property <DeviceItem> device`, `in property <bool> has-device`.
   Background `Theme.surface`, radius `Theme.radius`, padding 24px, contents in a
   `ScrollView` so long explanations scroll.
   - When `has-device` is false: only a centred `Text` "Select a device to see its
     details" in `Theme.text-faint`.
   - Otherwise, top to bottom, `spacing: Theme.gap`:
     a. `device.name`, `Theme.font-hero`, weight 700, `Theme.text`, word-wrapped.
     b. `device.vendor` in `Theme.text-muted` (hidden when empty).
     c. A `StatusPill` for `device.status-kind` / `device.status-text`.
     d. **What's wrong** card (only when `device.why` is not empty): background is the
        status colour `.with-alpha(0.10)`, 1px border in the status colour
        `.with-alpha(0.35)`, radius `Theme.radius-small`, padding 16px. Title "What's
        wrong" (`Theme.font-body`, weight 700, `Theme.text`), then `device.why`
        word-wrapped in `Theme.text`.
     e. **How to fix** card (only when `device.fix` is not empty): background
        `Theme.surface-raised`, radius `Theme.radius-small`, padding 16px. Title "How to
        fix" (weight 700), the sentence "Run this in a terminal:" in `Theme.text-muted`,
        then the command in a read-only `TextInput` (so it can be selected and copied)
        with `font-family: "monospace";`, `Theme.highlight` colour, on a
        `Theme.background` rectangle with radius 6px and padding 10px.
     f. `device.note` in `Theme.text-muted`, word-wrapped (hidden when empty).
     g. A **Details** section: title "Details" (`Theme.font-small`, weight 700,
        uppercase, letter-spacing 1px, `Theme.text-muted`), then a `GridLayout` with two
        columns (label in `Theme.text-muted`, value in `Theme.text`, values selectable
        via read-only `TextInput`): "Type" = `device.category`; "Driver" =
        `device.driver`, or "None" when empty; "Hardware ID" = `device.hardware-id`;
        "Connection" = `device.bus`; "Address" = `device.address`.
     h. When `device.unrecognized` is true, under the grid in `Theme.text-faint`:
        "This device isn't in the device database yet. Its hardware ID above can be
        searched for online."
3. In `app.slint`, replace the right-hand placeholder with
   `DetailsPanel { device: root.selected >= 0 ? root.devices[root.selected] : { };
   has-device: root.selected >= 0; horizontal-stretch: 3; }` (adapt the empty-struct
   syntax if the compiler needs it).

**Done when:**

- [ ] both components exist; the panel shows every item a to h exactly as described
- [ ] no hex colours or `Theme`-covered sizes inside the components
- [ ] the three checks pass with zero warnings; the app-opens check prints `124`
- [ ] Done notes filled in; nothing pushed, no co-author line

**Done notes:**

## Copilot 5: header, refresh button, notes

**Only after Copilot 4 is reviewed.**

**Files you may change:** new `hwsight-gui/ui/components/accent-button.slint`,
`notes-banner.slint`; the header row and bottom of the layout in
`hwsight-gui/ui/app.slint`; this task's Done notes.

**Steps (do all of them):**

1. `AccentButton`: `in property <string> text`, `in property <bool> enabled: true`,
   `callback clicked()`. Height 36px, horizontal padding 18px, radius 18px, background
   `@linear-gradient(90deg, Theme.accent-start 0%, Theme.accent-end 100%)`, text
   `Theme.font-body`, weight 600, white. On hover: `drop-shadow-blur: 14px;`
   `drop-shadow-color: Theme.accent-glow;` (animate 150ms). When not enabled: opacity
   0.45, no hover effect, clicks ignored.
2. Header row (replace the existing one in `app.slint`), height 56px:
   - "hwsight" in `Theme.font-hero`, weight 800, coloured with
     `@linear-gradient(90deg, Theme.accent-start 0%, Theme.accent-end 100%)`.
   - 16px gap, then the summary: while `root.scanning`, "Scanning your hardware..." in
     `Theme.text-muted`; otherwise a `StatusPill` (Copilot 4) with kind "ok" and text
     "No problems found" when `root.problem-count == 0`, else kind "warn" and text
     `root.summary`.
   - A flexible spacer, then `AccentButton { text: root.scanning ? "Scanning..." :
     "Scan again"; enabled: !root.scanning; clicked => { root.refresh(); } }`.
3. `NotesBanner`: `in property <[string]> notes`. Hidden (height 0, not visible) when
   empty. Otherwise a card: background `Theme.surface-raised`, 1px border `Theme.border`,
   radius `Theme.radius-small`, padding 12px 16px; title "Good to know" (`Theme.font-small`,
   weight 700, `Theme.text-muted`); each note on its own line, word-wrapped, `Theme.text`,
   `Theme.font-small`, 6px between notes. Place it under the two panels in `app.slint`.

**Done when:**

- [ ] both components exist; header and notes look exactly as described
- [ ] no hex colours or `Theme`-covered sizes inside the components (white text in
      `AccentButton` is allowed)
- [ ] the three checks pass with zero warnings; the app-opens check prints `124`
- [ ] Done notes filled in; nothing pushed, no co-author line

**Done notes:**

## Copilot 6: search and "only problems" filter

**Only after Copilot 5 is reviewed.**

**Files you may change:** `hwsight-gui/src/model.rs`, `hwsight-gui/src/main.rs`, new
`hwsight-gui/ui/components/search-box.slint`, `hwsight-gui/ui/app.slint`, this task's
Done notes.

**Steps (do all of them):**

1. In `model.rs`, add `pub fn filter_rows(devices: &[hwsight_core::Device], query: &str,
   only_problems: bool) -> Vec<Row>`: the same heading-plus-devices rows as `view()`
   builds, but only for devices that match. A device matches when (`only_problems` is
   false or `device.status.is_problem()`) and the query matches. The query matches when
   it is empty after trimming, or every whitespace-separated word of it appears,
   ignoring case, in at least one of: display name, vendor name, category label,
   hardware ID, driver. A category heading appears only if at least one of its devices
   matches, and its `header_count` is the number of matching devices. `Row.device` keeps
   indexing the full device list (so the details panel still works).
2. Refactor `view()` to build its rows with `filter_rows(&report.devices, "", false)`, so
   there is one implementation.
3. Keep the last scan's devices in `main.rs` (for example in an
   `Rc<RefCell<Vec<hwsight_core::Device>>>` updated when a scan finishes) so the filter
   can be re-run without scanning again.
4. In `app.slint` add `in-out property <string> query;`, `in-out property <bool>
   only-problems;`, and `callback filter-changed();`. Call `filter-changed()` when
   either changes. In `main.rs`, handle it by rebuilding rows with `filter_rows` and
   setting `rows`. After a new scan, apply the current query and toggle too.
5. `SearchBox` component: `in-out property <string> text`, `callback edited()`. Height
   36px, radius 18px, background `Theme.surface-raised`, 1px border `Theme.border`
   (`Theme.accent-start` while focused), horizontal padding 14px, placeholder
   "Search devices" in `Theme.text-faint` when empty. Put it at the top of the left
   panel, above the list, followed by a `Switch` (std-widgets) labelled "Only problems"
   bound to `only-problems`.
6. When the filter leaves no rows, the list area shows "Nothing matches." in
   `Theme.text-faint`, centred.

**Tests (all required, in `model.rs`):**

- `empty_query_matches_everything`
- `words_match_any_field_ignoring_case`: e.g. "nvidia" matches by vendor, "realtek
  8125" matches only the device with both words, "GRAPHICS" matches by category,
  "10de:2c05" matches by hardware ID, "r8169" matches by driver.
- `only_problems_hides_working_devices`
- `headings_only_for_categories_with_matches_and_counts_are_right`
- `row_indexes_point_into_the_full_list`
- `view_uses_filter_rows` (view's rows equal `filter_rows(.., "", false)`)

**Done when:**

- [ ] steps 1 to 6 done, all six tests exist and pass
- [ ] the three checks pass with zero warnings; the app-opens check prints `124`
- [ ] Done notes filled in; nothing pushed, no co-author line

**Done notes:**

## Claude 8: GUI skeleton (done 2026-11-03)

`hwsight-gui` crate, Theme tokens, background scans, `model::view` with tests. Next for
Claude after the Copilot GUI tasks: "match my desktop" theme from `~/.config/kdeglobals`,
theme switching, packaging (deb/rpm/AppImage on old bases), daily data update check.
