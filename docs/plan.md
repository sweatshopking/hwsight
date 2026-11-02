# hwsight work plan

Tasks are marked **Copilot** (well-specified, pattern-following) or **Claude** (design,
diagnosis logic, security, reviews). Claude reviews every Copilot task before it's pushed.

## Claude 1: PCI diagnosis edge cases (in progress)

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

## Claude 3: explain failures from the kernel log

Why a driver didn't start; missing firmware mapped to a package per distro.
