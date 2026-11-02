# Instructions for Copilot working on hwsight

hwsight is a Linux device manager for non-technical users, written in Rust (Slint GUI later).
It lists every device, says whether it works, and explains problems in plain English.
The work list is `docs/plan.md`. You only do tasks marked **Copilot** there.

## How you work (mandatory)

These are requirements, not suggestions. A task that breaks any of them is not done.

1. **Do the whole task in this session.** Every numbered step, every listed test. Do not
   stop partway to ask "should I continue?", do not offer to "do the rest later", do not
   split the task into follow-ups. Keep going until every item in the task's
   **Done when** checklist is true.
2. **No placeholders.** No `todo!()`, `unimplemented!()`, empty function bodies, stubbed
   tests, `#[ignore]`, or `// TODO` comments, except a TODO the task text explicitly
   tells you to write.
3. **No skipped tests.** Write every test the task lists, with real assertions. Do not
   weaken, delete, or `#[ignore]` an existing test to make the build pass. If an existing
   test fails because of your change, your change is wrong: fix your code.
4. **Stay inside the task.** Change only the files the task names. Do not refactor,
   rename, or "tidy" anything else. Do not edit `CLAUDE.md`, `Cargo.toml` versions,
   `.github/workflows/`, or `hwsight-core/data/`.
5. **Never push, and never create branches.** Commit only if the user asks you to, as a
   single local commit on the current branch, with no `Co-authored-by` line. Every change
   is reviewed before it's pushed.
6. **Run the checks yourself, in the terminal, and read the output.** Do not claim they
   pass without running them:

   ```
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   ```

   All three must finish with **zero errors and zero warnings**. If one fails, fix it and
   run all three again. Repeat until clean.
7. **If you are truly blocked** (something in the task is impossible or contradicts the
   code), do not guess and do not silently skip it. Do everything else, then write
   `BLOCKED:` and the exact reason in the task's **Done notes**.
8. **Finish by filling in Done notes** in `docs/plan.md` under your task: the files you
   changed, what each change does, and the last lines of the `cargo test` output showing
   the pass count. Then tick every box in the task's **Done when** list (`- [x]`).

## Code rules (breaking any of these is a bug)

1. **No `unwrap()` or `expect()` outside tests.** Clippy denies them. Return `Option`/`Result`
   or fall back gracefully. A scan must never crash: report problems, keep going.
2. **Works on every distro.** Never assume Arch (or any one distro) paths or tools. Check
   several known locations, as `data.rs` does.
3. **Never hide a device.** Unknown hardware still gets shown, with whatever we can infer.
4. **Everything that ages is data, not code.** Names, rules, and known issues come from
   updatable data files with layered loading (see `data.rs`, `pci_ids.rs`, `rules.rs`).
5. **Plain words** in any text a user sees. Short sentences, no jargon, no emoji.
6. **No new dependencies** unless the task says so.
7. Match the existing style: `cargo fmt`, short doc comments, comments only where the code
   doesn't explain itself.
8. Tests go in the same file's `#[cfg(test)] mod tests` (tests may use `expect`). Use fake
   files in a `tempfile::tempdir()`, never the real `/sys`, unless the task says otherwise.

## Map of the code

- `hwsight-core/src/lib.rs`: `Scanner` (all paths injectable for tests), `ScanReport`, notes.
- `device.rs`: bus-generic `Device`, `Category`, `Status`, `diagnose()`, naming helpers.
- `pci.rs`: reads `/sys/bus/pci/devices`. `pci_ids.rs`: layered `pci.ids` names.
- `usb.rs`: reads `/sys/bus/usb/devices` (devices and their interfaces).
- `modules.rs`: matches modaliases against the kernel's driver alias lists; blocklists.
- `kernel_log.rs`: reasons for problems from the kernel log.
- `rules.rs` + `data/rules.toml`, `firmware.rs` + `data/firmware.toml`: updatable data.
- `data.rs`: data folders, and the shared loader for versioned data files.
- `update.rs`: signed data updates. `distro.rs`: distro detection.
- `hwsight-cli/src/main.rs`: the `hwsight` command.
