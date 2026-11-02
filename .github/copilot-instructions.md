# Instructions for Copilot working on hwsight

hwsight is a Linux device manager for non-technical users, written in Rust (Slint GUI later).
It lists every device, says whether it works, and explains problems in plain English.
The work list is `docs/plan.md`. Only do tasks marked **Copilot** there, one task per
session, in the order given, unless the user says otherwise.

## Hard rules (breaking any of these is a bug)

1. **No `unwrap()` or `expect()` outside tests.** Clippy denies them. Return `Option`/`Result`
   or fall back gracefully. A scan must never crash: report problems, keep going.
2. **Works on every distro.** Never assume Arch (or any one distro) paths or tools. Check
   several known locations, as `data.rs` does.
3. **Never hide a device.** Unknown hardware still gets shown, with whatever we can infer.
4. **Everything that ages is data, not code.** Names, rules, and known issues come from
   updatable data files with layered loading (see `data.rs`, `pci_ids.rs`, `rules.rs`).
5. **Plain words** in any text a user sees. Short sentences, no jargon, no emoji.
6. **Don't add dependencies** unless the task says so. Any new crate must be
   GPL-3.0-compatible.
7. **Don't touch** `Cargo.toml` versions, `.github/workflows/`, or files another open
   task is about, unless your task says so.
8. Match the existing style: `cargo fmt`, short doc comments, comments only where the code
   doesn't explain itself.

## Before you say a task is done

Run all of these and fix everything they report:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Add unit tests for new logic in the same file's `#[cfg(test)] mod tests` (tests may use
`expect`). Use fake files in a `tempfile::tempdir()`, never the real `/sys`. Then write a
short summary under **Done notes** in the task's section of `docs/plan.md`: what changed,
which files, anything you're unsure about.

## Map of the code

- `hwsight-core/src/lib.rs`: `Scanner` (all paths injectable for tests), `ScanReport`, notes.
- `device.rs`: bus-generic `Device`, `Category`, `Status`, `diagnose()`, naming helpers.
- `pci.rs`: reads `/sys/bus/pci/devices`. `pci_ids.rs`: layered `pci.ids` names.
- `modules.rs`: matches modaliases against the kernel's driver alias lists.
- `rules.rs` + `hwsight-core/data/rules.toml`: updatable knowledge (schema + date version).
- `data.rs`: XDG data folders and distro ID file locations.
- `hwsight-cli/src/main.rs`: the `hwsight` command (`--problems`, `--ids`, `--json`).
