# Copilot 16: "Useful software" for each device

(Kept in its own file while Copilot 15 edits docs/plan.md; Claude moves it into the plan
after reviewing Copilot 15.)

**Only after Copilot 15 is reviewed.** Read first, in full: `hwsight-core/src/software.rs`
(Claude wrote the loader, matching, and package lookup; read every doc comment),
`hwsight-core/data/software.toml` (the format is explained at the top), `po/README.md`,
and `.github/copilot-instructions.md`, plus the "GUI batch" rules near the top of
`docs/plan.md`.

Owner's idea: "if somebody has a Logitech Bolt, it should recommend software that can use
it." And: "if it's not packaged, put a write up about how to search for it and what it
does so a user can find it themselves."

**Files you may change:** `hwsight-core/src/software.rs` (add to it; don't change what's
there), `hwsight-core/src/device.rs`, `hwsight-core/src/lib.rs`, `hwsight-core/src/pci.rs`,
`hwsight-core/src/usb.rs`, `hwsight-cli/src/main.rs`, `hwsight-gui/src/model.rs`,
`hwsight-gui/src/main.rs`, `hwsight-gui/ui/theme.slint` (only to add the `SoftwareItem`
struct and the `software` field), `hwsight-gui/ui/components/details-panel.slint`, the 11
catalogs in `po/` (by running `tools/update-translations.sh`, then translating only the
new entries), and the Done notes at the end of this file. Nothing else. Never edit
`software.toml` or any other data file.

## Steps (do all of them)

1. **Install check** (`software.rs`): add
   `pub struct InstallCheck { pub path_dirs: Vec<PathBuf>, pub flatpak_dirs: Vec<PathBuf> }`
   with `pub fn system() -> Self` (`path_dirs`: `$PATH` split on `:`, empty parts
   skipped; `flatpak_dirs`: `/var/lib/flatpak/app` and `$HOME/.local/share/flatpak/app`)
   and `pub fn is_installed(&self, program: &Program) -> bool`: true if any of
   `program.commands` is a file with an execute bit in any `path_dirs` folder, or if
   `program.flathub` is set and `<flatpak_dir>/<app id>` is a folder in any
   `flatpak_dirs`. Never panics, never runs a program.
2. **Tip** (`software.rs`): add `#[derive(Debug, Clone, Serialize)] pub struct Tip` with
   `pub id: String, pub name: String, pub what: String, pub homepage: String,
   pub installed: bool, pub install_command: Option<String>, pub how_to_get: Option<String>`
   and `pub fn tip(&self, program: &Program, distro: &Distro, check: &InstallCheck) -> Tip`
   on `Software`:
   - `what` = `self.what(program)` (translated).
   - `install_command` (`None` when installed):
     - distro is immutable (`distro.immutable`) and `flathub` is set:
       `flatpak install flathub <id>`;
     - else a distro package exists (`self.package_for`):
       `distro.family.install_command(package, distro.immutable)`;
     - else `flathub` is set: `flatpak install flathub <id>`;
     - else `None`.
   - `how_to_get` (`None` when installed, and `None` when a distro package gave the command):
     - `self.get(program)` if the data has a `get` text;
     - else, when the command is the Flathub one:
       `tr!("{} isn't in your distro's software sources, but you can install it from Flathub with this command.", name)`;
     - else: `tr!("{} isn't in your distro's software sources. Look for \"{}\" in your app store (such as Discover or GNOME Software) or on Flathub, or follow the install steps on its website: {}", name, search, homepage)`.
3. **Device** (`device.rs`): add `pub software: Vec<crate::software::Tip>` (empty by
   default; included in `--json`), and `#[serde(skip)] pub(crate) class_codes: Vec<u32>`:
   the class codes the device offers, in the same order used for its description (one
   for PCI, one per interface for USB). Set `class_codes` in `pci.rs` and `usb.rs` where
   the description is worked out. Update every place that builds a `Device` (including
   tests) so it compiles.
4. **Scan** (`lib.rs`): add `pub install_check: software::InstallCheck` to `Scanner`
   (`InstallCheck::system()` in `Scanner::system()`; tests pass fake folders). In
   `scan()`, load `Software` (problems go into `notes` like the other data files), give
   it the same `DataText` as descriptions (`with_text`), and fill `device.software` for
   every device from `for_device(bus, &class_codes, vendor, device)` and `tip(...)`.
   Vendor and device come from the hardware ID the device already has.
5. **CLI** (`main.rs`): after a device's status line (for every device shown, working or
   not), one line per tip, indented like the status line:
   `      Useful software: Solaar (installed)`, or when not installed
   `      Useful software: liquidctl. <what>` followed, if there is one, by
   `        Install: <install_command>` and, if there is one, `        <how_to_get>`.
   Use `tr!` for "Useful software: {}", "{} (installed)", and "Install: {}" (whole
   pieces with placeholders, as in Copilot 13). Leave the colour code as it is.
6. **GUI model** (`theme.slint`, `model.rs`): add
   `export struct SoftwareItem { name: string, what: string, installed: bool, command: string, how-to-get: string, homepage: string }`
   next to `FixItem`, and `software: [SoftwareItem]` to `DeviceItem`. Fill it in
   `model.rs` (empty strings for `None`). Add tests in `model.rs` for an installed and a
   not-installed tip.
7. **GUI panel** (`details-panel.slint`): when `device.software` isn't empty, a section
   below "How to fix" and above the copy button, with the heading `@tr("USEFUL SOFTWARE")`
   in the same style as the other section headings. For each item: the name in the
   section's bold text style; when installed, a `StatusPill` with kind "ok" and
   `@tr("Installed")`; the `what` text (muted, word-wrapped); when `command` isn't
   empty, the command in the same monospace box the fix steps use for their commands;
   `how-to-get` (muted, word-wrapped) when not empty; and the homepage as plain muted text.
   Theme only, no hex colours, every list item sets a height (the check script tests
   these).
8. **Translations:** run `tools/update-translations.sh`, then translate every new entry in
   all 11 catalogs (same rules as Copilot 14: placeholders kept, all plural forms
   filled). `every_language_is_complete` must pass again.

## Tests (all required)

In `software.rs`:
- `install_check_finds_a_command_on_path` (fake folder with an executable `solaar`; a
  non-executable file doesn't count)
- `install_check_finds_a_flatpak` (fake `flatpak_dirs` with an `org.openrgb.OpenRGB` folder)
- `tip_uses_the_distro_package` (Fedora + liquidctl: `sudo dnf install liquidctl`, no
  `how_to_get`)
- `tip_prefers_flathub_on_an_immutable_distro` (immutable Fedora + OpenRGB: the flatpak
  command)
- `tip_falls_back_to_flathub` (Debian + OpenRGB: flatpak command and the Flathub sentence)
- `tip_uses_the_get_text` (openSUSE + liquidctl: no command, `how_to_get` is the data's
  `get` text)
- `tip_explains_how_to_search` (a program with no package, no Flathub, no `get`: the
  sentence contains its name, its search term, and its homepage)
- `installed_tip_has_no_command`
In `lib.rs` (or `usb.rs`), with the existing fake sysfs helpers:
- `bolt_receiver_gets_solaar` (a fake USB device 046d:c548 ends up with a `solaar` tip)
In `model.rs`: the two tests from step 6.

## Done when

- [ ] steps 1 to 8 done; every test above exists and passes
- [ ] `cargo run -q -p hwsight-cli | grep -A2 "Useful software"` shows tips for this
      computer's Bolt receiver, NZXT Kraken, and AURA LED controller (pasted)
- [ ] the app shows the "Useful software" section for the Bolt receiver: say exactly what
      it shows (name, installed or the command)
- [ ] `cargo test -p hwsight-core --lib -- --ignored every_language_is_complete` passes
      (pasted)
- [ ] `tools/check.sh` printed `ALL CHECKS PASSED` after the last edit (output pasted)
- [ ] one `Step N: done in <files> - <what>` line per step below
- [ ] nothing committed, nothing pushed, no co-author line

## Done notes
