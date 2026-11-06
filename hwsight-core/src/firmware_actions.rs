//! Running fwupd for the user: refreshing its list of updates and installing one.
//!
//! Only ever started by a button the user pressed (after a confirmation for installs).
//! It runs the `fwupdmgr` program, which asks the desktop for the user's password through
//! polkit itself, downloads the signed update from the Linux Vendor Firmware Service, and
//! checks it before installing. hwsight never touches the firmware directly.
//!
//! Computer-wide firmware (the BIOS) is never installed from here: the scan only offers
//! devices listed in `ScanReport::pending_firmware`, and this refuses anything else.

use std::process::{Command, Stdio};

/// What went wrong, in words fit for the user.
pub type ActionResult = Result<String, String>;

/// fwupd device ids are 40 hex digits. Anything else could be taken for an option.
fn valid_device_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn run(args: &[&str]) -> ActionResult {
    let output = Command::new("fwupdmgr")
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                crate::tr!("The fwupdmgr program isn't installed.")
            } else {
                crate::tr!("Couldn't start fwupdmgr: {}", e)
            }
        })?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let errors = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if output.status.success() {
        Ok(text)
    } else {
        // fwupdmgr says "Nothing to do" / "No updatable devices" with a failing exit code.
        let reason = if errors.is_empty() { text } else { errors };
        Err(reason)
    }
}

/// Download fwupd's newest list of available updates.
pub fn refresh_catalog() -> ActionResult {
    run(&["refresh", "--force"])
}

/// Install the update for one device. `--assume-yes` skips fwupdmgr's own questions
/// because the user was already asked; `--no-reboot-check` leaves restarting to them.
pub fn install_update(device_id: &str) -> ActionResult {
    if !valid_device_id(device_id) {
        return Err(crate::tr!("That isn't a valid device id."));
    }
    run(&[
        "update",
        "--assume-yes",
        "--no-reboot-check",
        "--no-unreported-check",
        "--no-metadata-check",
        device_id,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_hex_ids_are_accepted() {
        assert!(valid_device_id("362301da643102b9f38477387e2193e57abaa590"));
        assert!(!valid_device_id(""));
        assert!(!valid_device_id("--force"));
        assert!(!valid_device_id("abc def"));
        assert!(!valid_device_id("abc;rm"));
    }

    #[test]
    fn a_bad_id_is_refused_before_anything_runs() {
        assert!(install_update("-y").is_err());
    }
}
