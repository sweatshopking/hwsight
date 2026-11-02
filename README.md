# hwsight

**A friendly Device Manager for Linux.**

hwsight shows every device in your computer, tells you which ones are working and which
aren't, and explains any problem in plain English. No terminal knowledge needed.

> **Status: early development.** Nothing to install yet. Watch or star the repo to follow along.

## What it will do

- **See everything:** every device in your PC, grouped by type: graphics, network, sound,
  storage, USB, and more.
- **Spot problems:** a clear status for each device, including when a driver exists but
  isn't loaded, or when firmware is missing.
- **Explain in plain English:** what's wrong, why it matters, and what you can do about it.
- **Fix things safely (later):** one-click fixes, such as loading a driver or installing
  missing firmware, always with your permission first.
- **Look good doing it:** a modern interface with built-in themes, or one that matches
  your desktop's colours.

## Works on any distro

hwsight is being built for every major Linux distribution, including Arch, Debian, Ubuntu,
Fedora, and openSUSE. Releases will come as .deb, .rpm, and AppImage packages, plus an AUR package.

## Built with

[Rust](https://www.rust-lang.org/) and [Slint](https://slint.dev/).

## Licence

[GPL-3.0-or-later](LICENSE)
