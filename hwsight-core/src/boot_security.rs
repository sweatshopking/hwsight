//! Whether Secure Boot is on and whether the computer has a TPM (security chip), shown on
//! the motherboard. Both are plain facts, not faults: some games' anti-cheat and Windows
//! 11 ask for them, which is why they're worth knowing.

use std::fs;
use std::path::Path;

use crate::details::{Detail, DetailGroup, Verdict};
use crate::device::Device;

const EFI_GLOBAL: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";

/// An EFI variable that is a single yes/no byte: the 4 attribute bytes come first.
fn efi_flag(efi_dir: &Path, name: &str) -> Option<bool> {
    let bytes = fs::read(efi_dir.join("efivars").join(format!("{name}-{EFI_GLOBAL}"))).ok()?;
    (bytes.len() == 5).then(|| bytes[4] == 1)
}

fn items(efi_dir: &Path, tpm_dir: &Path) -> Vec<Detail> {
    let mut items = Vec::new();
    let source = efi_dir.join("efivars").display().to_string();
    if let Some(on) = efi_flag(efi_dir, "SecureBoot") {
        let setup = efi_flag(efi_dir, "SetupMode") == Some(true);
        let (value, why) = if on {
            (
                crate::tr!("On"),
                crate::tr!(
                    "Only signed, trusted software can start the computer. Some programs, such as certain anti-cheat systems and Windows 11, ask for it."
                ),
            )
        } else if setup {
            (
                crate::tr!("Off (no keys set up)"),
                crate::tr!(
                    "Secure Boot has no keys enrolled yet, so it can't protect the boot. Some programs, such as certain anti-cheat systems and Windows 11, ask for it."
                ),
            )
        } else {
            (
                crate::tr!("Off"),
                crate::tr!(
                    "Anything can start the computer. That's fine for most people, but some programs, such as certain anti-cheat systems and Windows 11, ask for Secure Boot to be on."
                ),
            )
        };
        items.push(
            Detail::new(crate::tr!("Secure Boot"), value, source).verdict(Verdict::Info, Some(why)),
        );
    }
    let tpm = tpm_dir.join("tpm0").join("tpm_version_major");
    if let Some(major) = fs::read_to_string(&tpm)
        .ok()
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
    {
        items.push(
            Detail::new(
                crate::tr!("Security chip (TPM)"),
                crate::tr!("Version {}", format!("{major}.0")),
                tpm.display().to_string(),
            )
            .verdict(
                Verdict::Info,
                Some(crate::tr!(
                    "A chip that keeps encryption keys safe. Disk encryption and Windows 11 can use it."
                )),
            ),
        );
    }
    items
}

pub(crate) fn attach(devices: &mut [Device], efi_dir: &Path, tpm_dir: &Path) {
    let items = items(efi_dir, tpm_dir);
    if items.is_empty() {
        return;
    }
    if let Some(board) = devices.iter_mut().find(|d| d.address == "motherboard") {
        board.details.push(DetailGroup {
            title: crate::tr!("Boot security"),
            items,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::in_english;

    fn fake(secure: Option<u8>, setup: u8, tpm: Option<&str>) -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("tempdir");
        let vars = root.path().join("efi/efivars");
        fs::create_dir_all(&vars).expect("mkdir");
        if let Some(v) = secure {
            fs::write(
                vars.join(format!("SecureBoot-{EFI_GLOBAL}")),
                [6, 0, 0, 0, v],
            )
            .expect("write");
            fs::write(
                vars.join(format!("SetupMode-{EFI_GLOBAL}")),
                [6, 0, 0, 0, setup],
            )
            .expect("write");
        }
        if let Some(t) = tpm {
            fs::create_dir_all(root.path().join("tpm/tpm0")).expect("mkdir");
            fs::write(root.path().join("tpm/tpm0/tpm_version_major"), t).expect("write");
        }
        root
    }

    #[test]
    fn secure_boot_and_the_tpm_are_reported_in_plain_words() {
        let root = fake(Some(1), 0, Some("2\n"));
        let items = in_english(|| items(&root.path().join("efi"), &root.path().join("tpm")));
        assert_eq!(items[0].value, "On");
        assert_eq!(items[1].value, "Version 2.0");
        let off = fake(Some(0), 1, None);
        let items = in_english(|| items_for(&off));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].value, "Off (no keys set up)");
        let none = fake(None, 0, None);
        assert!(
            in_english(|| items_for(&none)).is_empty(),
            "a BIOS-mode computer says nothing"
        );
    }

    fn items_for(root: &tempfile::TempDir) -> Vec<Detail> {
        items(&root.path().join("efi"), &root.path().join("tpm"))
    }
}
