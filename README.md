# hwsight

**A friendly Device Manager for Linux.**

hwsight shows every device in your computer, tells you which ones are working and which
aren't, and explains any problem in plain English. No terminal knowledge needed, and plenty
of depth for people who want it.

> **Status: early development.** There are no releases or packages yet. You can build it
> from source (see below). Watch or star the repo to follow along.

## Why it exists

Windows has Device Manager: one place that lists your hardware and says what's wrong. Linux
has the pieces (`lspci`, `lsusb`, kernel logs, `dmesg`), but they speak in hardware IDs and
error codes. hwsight puts them together and translates, so "no driver loaded" or "firmware
file missing" becomes a sentence a non-technical person can act on.

## What it does today

- **Lists every device**, grouped by type: graphics, network, sound, storage, USB, Bluetooth,
  displays, memory, processor, motherboard, and more.
- **Gives each one a status**, and tells apart the cases that matter: a driver is installed
  but not running, a driver is turned off by a setting, a driver failed to start, firmware is
  missing, or nothing on the computer supports the device.
- **Explains problems using the kernel log**, for example which firmware file a driver is
  waiting for, and shows the fix with the right package and install command for your distro.
- **Three levels of detail.** *Simple* is plain English. *More details* adds the key facts
  with a verdict (good, could be better, problem). *Technical* shows everything it can read,
  with the exact file each value came from.
- **Reads a lot, without needing a password**, including:
  - PCIe link speed and width against what the slot allows, and whether Resizable BAR is on
  - memory: sticks, rated speeds (XMP and EXPO) against the speed you're running
  - processor, caches, and security mitigations; drives and their health; monitors
    (refresh rate, VRR, HDR); USB connection speed
  - network: wired link speed, Wi-Fi signal, band, and Wi-Fi generation
  - temperatures, fan speeds, and power from the kernel's sensors; NVIDIA readings through
    the driver's own tool
  - Secure Boot and TPM state; the audio chip; firmware versions
- **Finds firmware updates** through fwupd and lets you install them one device at a time,
  with a confirmation first. **BIOS and other system firmware are never installed by
  hwsight.** It tells you an update exists, shows the command, and warns you to keep the
  computer plugged in, since a failed BIOS update can leave a computer unable to start.
- **Suggests software worth having** for a device, such as a Logitech receiver manager or an
  RGB controller, in plain words.
- **Offers to install tools that unlock more information**, when the computer has the
  hardware they help with. It always says that your password is needed, and the system asks
  for it in its own window. hwsight never sees it.
- **Copies a report for a forum post**, for one device, one group of details, or the whole
  computer. Your user name and computer name are removed.
- **Speaks your language:** English, German, Spanish, French, Italian, Japanese, Korean,
  Dutch, Brazilian Portuguese, Russian, Turkish, and Simplified Chinese. Reports are always
  in English so the people answering forum posts can read them.
- **Has a command line too:** `hwsight` for the list, `hwsight --problems` for only what
  needs attention, `hwsight --details <address>` for everything about one device, and
  `hwsight --json` for scripts and bug reports.

## Safe by design

- It reads. It doesn't change your system unless you press a button and confirm.
- Anything that needs administrator rights goes through your desktop's normal password
  window (polkit). hwsight never asks for or stores your password.
- It never reads serial numbers, MAC addresses, or Wi-Fi network names, and the copied
  reports remove your user name and computer name.
- Its knowledge (device names, rules, firmware packages, translations) lives in data files
  that can be updated without a new release. Updates are signed and checked before use.

## Works on any distro

Built for the major Linux distributions: Arch, Debian, Ubuntu, Fedora, and openSUSE and
their relatives. Advice and install commands adapt to your distro, and read-only systems
(Fedora Atomic, Bazzite, SteamOS) are recognised so hwsight doesn't suggest things that
can't work there. Packages are planned as .deb, .rpm, and AppImage, plus an AUR package.

## Build from source

You need a recent stable [Rust](https://www.rust-lang.org/) toolchain.

```sh
git clone https://github.com/sweatshopking/hwsight
cd hwsight
cargo run --release -p hwsight-gui    # the window
cargo run --release -p hwsight-cli    # the command line
```

`tools/check.sh` runs every check (formatting, lints, tests, translation catalogs, and a
test that the window opens).

## What's next

Packaging and a first release, help for devices that have no driver (suggesting the right
package for your distro), more per-device explanations, and more translations review.

## Built with

[Rust](https://www.rust-lang.org/) and [Slint](https://slint.dev/).

## Licence

[GPL-3.0-or-later](LICENSE)
