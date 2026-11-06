//! The sound chip (codec) on each sound card, e.g. "Realtek ALC1220", from the kernel's
//! `/proc/asound` and matched to the sound device through `/sys/class/sound`.

use std::fs;
use std::path::Path;

use crate::details::{Detail, DetailGroup};
use crate::device::{Category, Device};

fn codecs(card_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(card_dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("codec#"))
        .map(|e| e.path())
        .collect();
    files.sort();
    files
        .iter()
        .filter_map(|f| fs::read_to_string(f).ok())
        .filter_map(|text| {
            text.lines()
                .next()?
                .strip_prefix("Codec:")
                .map(|name| name.trim().to_owned())
        })
        .filter(|name| !name.is_empty())
        .collect()
}

pub(crate) fn attach(devices: &mut [Device], sound_dir: &Path, asound_dir: &Path) {
    let Ok(cards) = fs::read_dir(sound_dir) else {
        return;
    };
    for card in cards.filter_map(Result::ok) {
        let name = card.file_name().to_string_lossy().into_owned();
        if !name.starts_with("card") || name[4..].parse::<u32>().is_err() {
            continue;
        }
        let Ok(owner) = fs::canonicalize(card.path().join("device")) else {
            continue;
        };
        let names = codecs(&asound_dir.join(&name));
        if names.is_empty() {
            continue;
        }
        let Some(device) = devices.iter_mut().find(|d| {
            d.category == Category::Sound
                && !d.sysfs_path.as_os_str().is_empty()
                && fs::canonicalize(&d.sysfs_path).is_ok_and(|p| p == owner)
        }) else {
            continue;
        };
        let source = asound_dir.join(&name).display().to_string();
        device.details.push(DetailGroup {
            title: crate::tr!("Sound chip"),
            items: names
                .into_iter()
                .map(|n| Detail::new(crate::tr!("Chip (codec)"), n, source.clone()))
                .collect(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::BuiltIn;
    use std::os::unix::fs::symlink;

    #[test]
    fn a_sound_device_gets_its_codec_name() {
        let root = tempfile::tempdir().expect("tempdir");
        let pci = root.path().join("pci/0000:0c:00.6");
        fs::create_dir_all(&pci).expect("mkdir");
        let sound = root.path().join("sound/card2");
        fs::create_dir_all(&sound).expect("mkdir");
        symlink(&pci, sound.join("device")).expect("symlink");
        let asound = root.path().join("asound/card2");
        fs::create_dir_all(&asound).expect("mkdir");
        fs::write(
            asound.join("codec#0"),
            "Codec: Realtek ALC1220\nAddress: 0\n",
        )
        .expect("write");
        let mut devices = vec![Device::built_in(BuiltIn {
            address: "x",
            sysfs_path: pci,
            display_name: "Ryzen HD Audio".into(),
            vendor_name: None,
            category: Category::Sound,
            hardware_id: String::new(),
            description: String::new(),
            details: Vec::new(),
        })];
        attach(
            &mut devices,
            &root.path().join("sound"),
            &root.path().join("asound"),
        );
        assert_eq!(devices[0].details[0].items[0].value, "Realtek ALC1220");
    }
}
