# Instructions for Copilot working on hwsight

hwsight is a Linux device manager for non-technical users, written in Rust (Slint GUI later).
It lists every device, says whether it works, and explains problems in plain English.
The work list is `docs/plan.md`. You only do tasks marked **Copilot** there.

## How you work (mandatory)

These are requirements, not suggestions. Breaking any of them means the task is
rejected and done again from the start.

1. **Do the whole task in this session.** Every numbered step, every listed test. Do not
   stop partway to ask "should I continue?", do not offer to "do the rest later", do not
   split the task into follow-ups.
2. **Corrections from the user override the task text.** If the user sends a correction
   while you work, apply it before anything else, even if you think the task was already
   finished. In Done notes, add a "Corrections applied" list: each correction and the
   file and line where you applied it. Never silently skip a correction.
3. **No workarounds.** Follow every rule exactly as written. If a rule seems impossible
   (for example an import that doesn't compile), do not invent a different approach that
   breaks another rule. Stop, do everything else, and write `BLOCKED:` with the exact
   problem and compiler error in Done notes.
4. **No placeholders.** No `todo!()`, `unimplemented!()`, empty function bodies, stubbed
   tests, `#[ignore]`, or `// TODO` comments, except a TODO the task text explicitly
   tells you to write.
5. **No skipped or weakened tests.** Write every test the task lists, with real
   assertions. Do not weaken, delete, or `#[ignore]` an existing test to make the build
   pass. If an existing test fails because of your change, your change is wrong.
6. **Stay inside the task.** Change only the files the task names. Do not refactor,
   rename, delete comments, or "tidy" anything else. Do not edit `CLAUDE.md`, `Cargo.toml`
   versions, `.github/`, `tools/`, or `hwsight-core/data/`.
7. **Never push, and never create branches.** Commit only if the user asks you to, as a
   single local commit on the current branch, with no `Co-authored-by` line. Every change
   is reviewed before it's pushed.
8. **Verify with the check script, never from memory.** Run `tools/check.sh` in the
   terminal after your last edit. It must end with `ALL CHECKS PASSED`. If anything
   fails, fix it and run it again. Do not tick any "Done when" box until it passes, and
   paste its complete output into Done notes. A ticked box that isn't true is the worst
   possible failure: the reviewer re-runs the script, and a false tick means the task is
   rejected.
9. **Account for every step.** Before ticking boxes, re-read the task text from the top
   and write one line per numbered step in Done notes: `Step N: done in <file> - <what>`.
   If you find a step you missed, do it now.
10. **If you are truly blocked**, do everything else, then write `BLOCKED:` and the exact
    reason in Done notes. Never skip anything silently.

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
- `hwsight-gui/`: the Slint app. `ui/app.slint` holds `Theme` (all colours, sizes, fonts)
  and the window; components go in `ui/components/`. `src/model.rs` turns a scan report
  into UI data (tested); `src/main.rs` runs scans on a background thread. GUI tasks have
  extra rules at the top of the "GUI batch" section in `docs/plan.md`.
