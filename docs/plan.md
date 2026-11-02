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
- `tools/check.sh` runs every check, including that the app opens and stays open and
  that the rules above are followed (no hex colours or colour properties in components,
  Theme only in theme.slint, every list item has a height). It must print
  `ALL CHECKS PASSED` before any box is ticked; paste its output into Done notes.

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
- Claude review (2026-11-03): REJECTED as delivered, fixed by Claude. All boxes were
  ticked but two user corrections were not applied (Theme still in app.slint; list items
  still 0px tall, so every row drew on top of the others). Copilot also worked around
  the import problem by passing every colour into components as properties, deleted the
  Theme comments, and the count pill had no width. Claude moved Theme and DeviceItem to
  ui/theme.slint, rewrote the three components to read Theme, gave list items heights,
  fixed the pill, and verified with a screenshot. Added tools/check.sh so rules are
  checked mechanically from now on.
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

- [x] both components exist; the panel shows every item a to h exactly as described
- [x] no hex colours or `Theme`-covered sizes inside the components
- [x] `tools/check.sh` printed `ALL CHECKS PASSED` after the last edit (output pasted below)
- [x] one `Step N: done in <file> - <what>` line per numbered step in Done notes
- [x] Done notes filled in; nothing pushed, no co-author line

**Done notes:**
- Claude review (2026-11-03): accepted with fixes. Followed the process well (no
  commit, per-step notes, check output). Fixed: it redefined `TextInput` as a `LineEdit`
  (boxed input fields instead of plain selectable text; check.sh now catches built-in
  redefinitions); the status pill stretched across the panel; hidden vendor/note still
  took space (`visible:` -> `if`); details spread over the whole panel height
  (`alignment: start`); pill dot not centred; unused import. Verified by screenshot.
- Step 1: done in `hwsight-gui/ui/components/status-pill.slint` - Added the
  status-colored pill, border, typography, and status dot.
- Step 2: done in `hwsight-gui/ui/components/details-panel.slint` - Added the
  scrollable panel with empty state, device heading, status, explanation, fix command,
  note, details grid, and unrecognized-device message.
- Step 3: done in `hwsight-gui/ui/app.slint` - Replaced the right-hand placeholder with
  `DetailsPanel`, passing the selected device and selection state.
- `tools/check.sh` output after the last edit:
  ```
  PASS  cargo fmt --check
  PASS  cargo clippy (no warnings)
  PASS  cargo test
  PASS  GUI builds
  PASS  GUI opens and stays open
  PASS  no hex colours in components
  PASS  Theme and DeviceItem defined only in ui/theme.slint
  PASS  components never import app.slint
  PASS  no colours passed into components as properties (use Theme)
  PASS  every list item sets a height

  ALL CHECKS PASSED
  ```
- Test result: `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`
- Test result: `test result: ok. 72 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s`
- Test result: `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`
- Test result: `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`
- `hwsight-gui/ui/components/status-pill.slint`: Added the status pill with themed
  color, border, status dot, and readable status text.
- `hwsight-gui/ui/components/details-panel.slint`: Added the full device details view,
  including problem and fix cards, selectable values, and long-text scrolling.
- `hwsight-gui/ui/app.slint`: Connected the selected device to the details panel and
  preserved the empty selection state.
- Nothing was pushed, and no co-author line was added.

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

- [x] both components exist; header and notes look exactly as described
- [x] no hex colours or `Theme`-covered sizes inside the components (white text in
      `AccentButton` is allowed)
- [x] `tools/check.sh` printed `ALL CHECKS PASSED` after the last edit (output pasted below)
- [x] one `Step N: done in <file> - <what>` line per numbered step in Done notes
- [x] Done notes filled in; nothing pushed, no co-author line

**Done notes:**
- Claude review (2026-11-03): accepted with fixes; process followed well. Fixed after
  checking on screen: gradient-filled title text rendered as nothing (renderer
  limitation, the task text's fault), so the gradient is now a glowing bar beside a solid
  title; the status pill and Scan again button stretched (now sized to their text); the
  header items weren't vertically centred and the pill floated mid-header (stretch).
  Note messages now name just the file, not the full path. Added HWSIGHT_SNAPSHOT /
  HWSIGHT_SELECT so the app can save a picture of itself for reviews.
- Step 1: done in `hwsight-gui/ui/components/accent-button.slint` - Added the
  themed gradient button, hover glow, disabled state, and click callback.
- Step 2: done in `hwsight-gui/ui/app.slint` - Replaced the header with the gradient
  title, scan summary/status pill, flexible spacer, and scan-again button.
- Step 3: done in `hwsight-gui/ui/components/notes-banner.slint` and
  `hwsight-gui/ui/app.slint` - Added the conditional notes card and placed it below
  the two panels.
- `tools/check.sh` output after the last edit:
  ```
  PASS  cargo fmt --check
  PASS  cargo clippy (no warnings)
  PASS  cargo test
  PASS  GUI builds
  PASS  GUI opens and stays open
  PASS  no hex colours in components
  PASS  Theme and DeviceItem defined only in ui/theme.slint
  PASS  components never import app.slint
  PASS  no colours passed into components as properties (use Theme)
  PASS  no built-in elements redefined
  PASS  every list item sets a height

  ALL CHECKS PASSED
  ```
- Nothing was pushed, and no co-author line was added.

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

- [x] steps 1 to 6 done, all six tests exist and pass
- [x] `tools/check.sh` printed `ALL CHECKS PASSED` after the last edit (output pasted below)
- [x] one `Step N: done in <file> - <what>` line per numbered step in Done notes
- [x] Done notes filled in; nothing pushed, no co-author line

**Done notes:**
- Claude review (2026-11-03): accepted with fixes; filter logic correct and process
  followed. Fixed: "Nothing matches." was inside the list loop, so it could never show
  (now an overlay on the empty list); the search box wrapped a framed LineEdit (double
  outline, unthemed placeholder), now a plain TextInput with a Theme placeholder; search
  and switch moved into the device panel as specified; a doc comment had moved onto the
  wrong function. Tests strengthened: each field matched on its own, words spread over
  different devices, and partial matches inside a shared category.

Step 1: done in `hwsight-gui/src/model.rs` - Added case-insensitive, multi-word device filtering with problem filtering, category headings, counts, and full-list indexes.
Step 2: done in `hwsight-gui/src/model.rs` - Refactored `view()` to build rows through `filter_rows`.
Step 3: done in `hwsight-gui/src/main.rs` - Retained the latest scan devices in shared state for filter-only updates.
Step 4: done in `hwsight-gui/src/main.rs` and `hwsight-gui/ui/app.slint` - Added query and problem-toggle properties, filter callback wiring, and re-filtering after scans.
Step 5: done in `hwsight-gui/ui/components/search-box.slint` and `hwsight-gui/ui/app.slint` - Added the themed search box and Only problems switch above the device list.
Step 6: done in `hwsight-gui/ui/app.slint` - Added the centered Nothing matches empty-results message.

`tools/check.sh` output:
```text
PASS  cargo fmt --check
PASS  cargo clippy (no warnings)
PASS  cargo test
PASS  GUI builds
PASS  GUI opens and stays open
PASS  no hex colours in components
PASS  Theme and DeviceItem defined only in ui/theme.slint
PASS  components never import app.slint
PASS  no colours passed into components as properties (use Theme)
PASS  no built-in elements redefined
PASS  every list item sets a height

ALL CHECKS PASSED
```

## Claude 8: GUI skeleton (done 2026-11-03)

`hwsight-gui` crate, Theme tokens, background scans, `model::view` with tests. Next for
Claude after the Copilot GUI tasks: "match my desktop" theme from `~/.config/kdeglobals`,
theme switching, packaging (deb/rpm/AppImage on old bases), daily data update check.

## Claude 9: "What it does" descriptions (done 2026-11-03)

Owner's idea. Updatable `data/descriptions.toml` (own file, so it can be translated):
most specific `[[describe]]` match wins (exact device > protocol > subclass > class),
USB devices matched by their interfaces (earlier interface wins ties), and a
`[category]` fallback so every device gets text. Shown as "What it does" in the
details panel and in `--json`. Specific entries so far: NZXT Kraken, ASUS AURA, Logi
Bolt receiver, Xbox-style pads (USB ff/5d, ff/47). Good Copilot data-entry task later:
more specific device entries, following the writing style in the file header.

## Claude 10: "How to fix" steps (done 2026-11-03)

Updatable `data/fixes.toml`: one template per situation (missing firmware, firmware
installed since boot, driver timed out / I/O error / busy / failed, not loaded, blocked
by a file or the kernel command line, not attached, not ready, no driver, USB blocked).
`Fixes::suggest` builds `Device.fixes` (log-based steps first, then status). Values in
commands are checked (driver/package names, install command shape, hardware ID); `{file}`
can never appear in a command; a step with an unsafe value is dropped, not shown. CLI
prints numbered Fix lines; GUI shows numbered steps with copyable commands.
Next: a "Fix it" button that runs the command through polkit after asking.

## Claude 11: automatic app updater (core DONE 2026-11-03; front ends = Copilot 7 and 8)

Owner's rule: every app gets an automatic updater. `hwsight-core/src/app_update.rs`
(adapted from DriveDeck's, same author): `latest_release()` (GitHub API, drafts and
pre-releases ignored, https assets only), `detect_install(exe, APPIMAGE env)` ->
AppImage / Package (deb via apt-get, rpm via dnf, zypper, or rpm, all through pkexec) /
Aur / Unmanaged, `asset_for`, `sum_for`, `install_update(release, install, current)` ->
`Outcome` (Updated{version, restart}, Cancelled, UpToDate, UpdateElsewhere, Failed) with
plain `message()`. Every download is checked against SHA256SUMS.txt. Behaviour wanted:
AppImage updates itself silently (then "restart to use it"); deb/rpm get an "Update
now" banner (one password prompt); AUR/source builds just say a new version is out.

**Release file names (packaging MUST use exactly these):** `hwsight-<v>-x86_64.AppImage`,
`hwsight-<v>-aarch64.AppImage`, `hwsight_<v>_amd64.deb`, `hwsight_<v>_arm64.deb`,
`hwsight-<v>-<release>.x86_64.rpm`, `hwsight-<v>-<release>.aarch64.rpm`, plus
`SHA256SUMS.txt` (`<sha256>  <file name>` per line). Tags `v<major>.<minor>.<patch>`.
Not testable end to end until the first release exists (needs packaging).

Next for Claude (Friday): review Copilot 7 and 8, then packaging (deb/rpm/AppImage on
Ubuntu 22.04 / Fedora 41 bases, x86_64 + arm64, CI) with the names above, then a first
release to test the updater for real, then laptop internals (I2C/ACPI/platform devices).

## Copilot 7: `hwsight --update` and `--check-update`

**Files you may change:** `hwsight-cli/src/main.rs` and this task's Done notes. Nothing else.

**Steps (do all of them):**

1. Add two options to `HELP`, in the same style as the others:
   `--check-update   See whether a newer version of hwsight is out` and
   `--update         Download and install the newest version of hwsight`.
2. Add `fn check_update_text(result: Result<Option<hwsight_core::app_update::Release>, String>, current: &str) -> String`
   returning exactly one of:
   - newer release: `"hwsight {latest} is available (you have {current}). Run hwsight --update to install it."`
   - same or older release, or `Ok(None)`: `"hwsight {current} is the newest version."`
   - `Err(e)`: `"Couldn't check for updates: {e}"`
   Use `hwsight_core::app_update::is_newer` for the comparison.
3. `--check-update`: call `hwsight_core::app_update::latest_release()`, print
   `check_update_text(...)` with `env!("CARGO_PKG_VERSION")`, exit 0 on success and 1 on
   `Err`.
4. `--update`: call `latest_release()`. On `Err(e)` print the same error text and exit 1.
   On `Ok(None)` print the "newest version" text and exit 0. On `Ok(Some(release))`:
   `let install = hwsight_core::app_update::detect_install(&std::env::current_exe()...,
   std::env::var_os("APPIMAGE"))` (if `current_exe()` fails, print
   `"Couldn't find where hwsight is installed."` and exit 1), then
   `install_update(&release, &install, env!("CARGO_PKG_VERSION"))`, print
   `outcome.message()`, exit 0 for `Updated`, `UpToDate`, `UpdateElsewhere`, `Cancelled`,
   and 1 for `Failed`.
5. Both options run on their own and then exit, like `--update-data` does now.

**Tests (all required, in `main.rs`'s `mod tests`):**
- `check_update_text_newer` (release "9.9.9" vs current "0.1.0")
- `check_update_text_same_and_none` (same version, and `Ok(None)`)
- `check_update_text_error`
Build `Release` values directly: `Release { version: "9.9.9".into(), assets: vec![] }`.

**Done when:**

- [x] steps 1 to 5 done; the three tests exist and pass
- [x] `tools/check.sh` printed `ALL CHECKS PASSED` after the last edit (output pasted below)
- [x] one `Step N: done in <file> - <what>` line per numbered step in Done notes
- [x] nothing pushed, and no co-author line

**Done notes:**
- Claude review (2026-11-06): accepted as is. Matches the spec; real run with no
  releases yet prints "hwsight 0.1.0 is the newest version."
- Step 1: done in `hwsight-cli/src/main.rs` - added `--check-update` and `--update` help entries.
- Step 2: done in `hwsight-cli/src/main.rs` - added exact-version comparison text using `is_newer`.
- Step 3: done in `hwsight-cli/src/main.rs` - added the check-update request, output, and exit status.
- Step 4: done in `hwsight-cli/src/main.rs` - added release detection, installation, outcome messages, and exit statuses.
- Step 5: done in `hwsight-cli/src/main.rs` - both update options run independently and exit.
- `check_update_text_newer`, `check_update_text_same_and_none`, and `check_update_text_error` pass.
- `tools/check.sh` output:
  ```
  PASS  cargo fmt --check
  PASS  cargo clippy (no warnings)
  PASS  cargo test
  PASS  GUI builds
  PASS  GUI opens and stays open
  PASS  no hex colours in components
  PASS  Theme and DeviceItem defined only in ui/theme.slint
  PASS  components never import app.slint
  PASS  no colours passed into components as properties (use Theme)
  PASS  no built-in elements redefined
  PASS  every list item sets a height

  ALL CHECKS PASSED
  ```

## Copilot 8: update banner in the app

**Only after Copilot 7 is reviewed.** Follow the "GUI batch" rules above (Theme only,
components in `ui/components/`, `tools/check.sh`).

**Files you may change:** new `hwsight-gui/src/app_updates.rs`, new
`hwsight-gui/ui/components/update-banner.slint`, `hwsight-gui/src/main.rs`,
`hwsight-gui/ui/app.slint`, this task's Done notes.

**Steps (do all of them):**

1. `src/app_updates.rs`: a pure function (no network) that decides what to show:
   ```rust
   pub enum Banner { Hidden, Offer { text: String }, Info { text: String }, AutoInstall }
   pub fn banner_for(install: &Install, latest: &str, current: &str) -> Banner
   ```
   - not newer (`is_newer` false): `Hidden`
   - `Install::AppImage(_)`: `AutoInstall` (it updates itself without asking)
   - `Install::Package { .. }`: `Offer { text: "hwsight {latest} is available." }`
   - `Install::Aur` / `Install::Unmanaged`: `Info { text: "hwsight {latest} is available. Update it with your package manager, or download it from the website." }`
2. `ui/components/update-banner.slint`, `UpdateBanner`: `in property <string> text`,
   `in property <bool> can-update`, `in property <bool> busy`, `in property <bool>
   can-restart`, `callback update-now()`, `callback restart-now()`. Hidden (height 0,
   not visible) when `text` is empty. Otherwise a card like `NotesBanner` but with a 1px
   border in `Theme.accent-start.with-alpha(0.6)`: the text (`Theme.text`, word-wrap),
   then on the right an `AccentButton` "Update now" (when `can-update && !busy`),
   "Updating..." text in `Theme.text-muted` (when `busy`), or an `AccentButton`
   "Restart" (when `can-restart`).
3. `app.slint`: properties `update-text`, `update-can-update`, `update-busy`,
   `update-can-restart` (all `in`), callbacks `update-now()` and `restart-now()`; place
   the `UpdateBanner` directly under the header row.
4. `main.rs`: on start, unless `HWSIGHT_NO_UPDATE` or `HWSIGHT_SNAPSHOT` is set, and
   only if `update::check_due(app_update::default_state_file(), now, 1 day)` is true:
   on a background thread call `latest_release()`, then `update::mark_checked` on the
   state file whatever happened. If there's a release, run `banner_for` with
   `detect_install(current_exe, APPIMAGE)`:
   - `AutoInstall`: run `install_update` on that same thread; when it returns `Updated`,
     show `outcome.message()` with `can-restart` true; on `Failed`, show
     `outcome.message()` with nothing to click.
   - `Offer`: show the text with `can-update` true.
   - `Info`: show the text, nothing to click.
   Send results to the window with `slint::invoke_from_event_loop` (like the scan does).
5. `update-now`: set `busy`, run `install_update` on a background thread, then show
   `outcome.message()`; `can-restart` true only for `Updated`; for `Cancelled` keep
   `can-update` true so the user can try again.
6. `restart-now`: start the `restart` path from `Outcome::Updated` with
   `std::process::Command::new(path).spawn()`, then `slint::quit_event_loop()`. Keep the
   path from the outcome (for example in an `Rc<RefCell<Option<PathBuf>>>`).

**Tests (all required, in `app_updates.rs`):**
- `hidden_when_not_newer`
- `appimage_installs_automatically`
- `packages_get_an_offer` (deb and rpm)
- `aur_and_source_builds_get_info_only`

**Done when:**

- [x] steps 1 to 6 done; the four tests exist and pass
- [x] `tools/check.sh` printed `ALL CHECKS PASSED` after the last edit (output pasted below)
- [x] one `Step N: done in <file> - <what>` line per numbered step in Done notes
- [x] nothing pushed, and no co-author line

**Done notes:**
- Claude review (2026-11-06): accepted with one real fix: the app's daily check used
  the data updater's state file, so whichever daily check ran first silently cancelled
  the other (spec said app_update::default_state_file). Also moved install detection
  (package-manager queries) off the UI thread.
- Step 1: done in `hwsight-gui/src/app_updates.rs` - added pure banner decisions for each install type and version state.
- Step 2: done in `hwsight-gui/ui/components/update-banner.slint` - added the themed update card with update, busy, and restart actions.
- Step 3: done in `hwsight-gui/ui/app.slint` - added update properties, callbacks, and the banner below the header.
- Step 4: done in `hwsight-gui/src/main.rs` - added the daily background release check, installation behavior, and UI-thread results.
- Step 5: done in `hwsight-gui/src/main.rs` - added background manual installation with busy, retry, and restart state.
- Step 6: done in `hwsight-gui/src/main.rs` - added safe restart spawning from the completed update path.
- `hidden_when_not_newer`, `appimage_installs_automatically`, `packages_get_an_offer`, and `aur_and_source_builds_get_info_only` pass.
- `tools/check.sh` output:
  ```
  PASS  cargo fmt --check
  PASS  cargo clippy (no warnings)
  PASS  cargo test
  PASS  GUI builds
  PASS  GUI opens and stays open
  PASS  no hex colours in components
  PASS  Theme and DeviceItem defined only in ui/theme.slint
  PASS  components never import app.slint
  PASS  no colours passed into components as properties (use Theme)
  PASS  no built-in elements redefined
  PASS  every list item sets a height

  ALL CHECKS PASSED
  ```

## Copilot 9: more "What it does" descriptions

**Files you may change:** `hwsight-core/data/descriptions.toml` (add entries only; do
not change or remove existing ones), `hwsight-core/src/descriptions.rs` (tests only),
this task's Done notes. This task is allowed to edit `hwsight-core/data/`.

**Steps (do all of them):**

1. Read the header of `descriptions.toml`: follow its writing style exactly (one or two
   short plain sentences, what it's for in everyday terms, no jargon unless explained,
   no brand names, no emoji).
2. Add a `[[describe]]` entry (PCI, the default bus) for each of these class/subclass
   pairs: 01/00 (SCSI), 01/01 (IDE), 01/04 (RAID), 01/05 (ATA), 01/07 (SAS),
   01/80 (other storage), 03/02 (3D controller: laptop second GPUs), 04/00 (video
   capture), 04/01 (older sound cards), 07/00 (serial port), 07/01 (parallel port),
   07/03 (modem), 08/05 (SD card reader), 0c/00 (FireWire), 0c/06 (InfiniBand),
   0c/80 (other serial bus), 0d/80 (other wireless), 11/80 (signal processing:
   usually power or performance helpers), 13/00 (debug function).
3. Add `bus = "usb"` entries for USB classes: 0f (personal healthcare), 10 (audio/video
   device), 11 (billboard: shown by USB-C docks and adapters when a mode isn't
   supported), dc (diagnostic), ef (miscellaneous), fe with subclass 01 (firmware
   update mode, DFU).
4. Check every class/subclass code against the class list at the end of
   `/usr/share/hwdata/pci.ids` and `/usr/share/hwdata/usb.ids` (lines starting `C `).
   Do not add any code that isn't listed there. Write in Done notes which file line
   confirmed each code (for example `pci.ids: C 01 / 05 ATA controller`).
5. Bump `version` at the top of `descriptions.toml` to today's date (YYYY.MM.DD).

**Tests (all required, in `descriptions.rs`):**
- `every_new_pci_entry_matches` : for each class/subclass in step 2, `describe(Bus::Pci,
  &[code], 0, 0, Category::Other)` returns text that is not the category fallback.
- `every_new_usb_entry_matches` : the same for step 3 with `Bus::Usb`.

**Done when:**

- [ ] all entries from steps 2 and 3 added in the file's style; each code confirmed in Done notes
- [ ] both tests exist and pass
- [ ] `tools/check.sh` printed `ALL CHECKS PASSED` after the last edit (output pasted below)
- [ ] one `Step N: done in <file> - <what>` line per numbered step in Done notes
- [ ] nothing pushed, and no co-author line

**Done notes:**

## Copilot 10: keyboard shortcuts in the app

**Only after Copilot 9 is reviewed.** Follow the "GUI batch" rules above.

**Files you may change:** `hwsight-gui/ui/app.slint`,
`hwsight-gui/ui/components/search-box.slint`, `hwsight-gui/src/model.rs`,
`hwsight-gui/src/main.rs`, this task's Done notes.

**Steps (do all of them):**

1. In `model.rs` add `pub fn next_device(rows: &[Row], selected: i32, step: i32) -> i32`:
   the device index of the next (`step` = 1) or previous (`step` = -1) *device* row
   after the row showing `selected`, skipping headings. If nothing is selected, Down
   picks the first device row and Up the last. At either end it stays where it is.
   With no device rows it returns -1.
2. Expose it to the window: `callback move-selection(int)` in `app.slint` (argument is
   the step); in `main.rs` handle it by reading the current `rows`, calling
   `next_device`, and setting `selected`.
3. Keyboard handling in `app.slint` with a `FocusScope` around the window content:
   Up / Down call `move-selection(-1 / 1)`; F5 calls `refresh()` (only when not
   scanning); Ctrl+F focuses the search box (add a `public function focus-search()` to
   `SearchBox` that focuses its input); Escape in the search box clears `query`.
   Arrow keys must still work for typing inside the search box (only handle them when
   the search box doesn't have focus).
4. When the selection moves with the keyboard, scroll the list so the selected row is
   visible (`ListView` viewport: adjust `viewport-y` so the row's top is inside the
   visible area; rows are 40px for headings and 52px for devices).

**Tests (all required, in `model.rs`):**
- `next_device_skips_headings`
- `next_device_from_nothing_selected`
- `next_device_stays_at_the_ends`
- `next_device_with_no_devices`

**Done when:**

- [ ] steps 1 to 4 done; the four tests exist and pass
- [ ] `tools/check.sh` printed `ALL CHECKS PASSED` after the last edit (output pasted below)
- [ ] one `Step N: done in <file> - <what>` line per numbered step in Done notes
- [ ] nothing pushed, and no co-author line

**Done notes:**
