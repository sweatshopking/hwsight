//! What the graphics driver says about the card: the NVIDIA driver version and video BIOS,
//! or an AMD card's video memory and load. All plain files; the NVIDIA serial/UUID is
//! never read.

use std::fs;
use std::path::Path;

use crate::details::{Detail, DetailGroup};
use crate::device::{Category, Device};

fn read(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  615.78.08  Release..."
fn nvidia_driver(version_text: &str) -> Option<(String, bool)> {
    let first = version_text.lines().next()?;
    let number = first.split_whitespace().find(|w| {
        w.contains('.')
            && w.split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    })?;
    Some((number.to_owned(), first.contains("Open Kernel Module")))
}

fn size_text(bytes: u64) -> String {
    let mib = bytes >> 20;
    if mib >= 1024 {
        crate::tr!("{} GB", mib / 1024)
    } else {
        crate::tr!("{} MB", mib)
    }
}

fn nvidia_items(device: &Device, proc_dir: &Path) -> Vec<Detail> {
    let mut items = Vec::new();
    let version = proc_dir.join("version");
    if let Some((number, open)) = read(&version).and_then(|t| nvidia_driver(&t)) {
        let source = version.display().to_string();
        items.push(Detail::new(
            crate::tr!("Driver version"),
            number,
            source.clone(),
        ));
        items.push(
            Detail::new(
                crate::tr!("Driver type"),
                if open {
                    crate::tr!("Open kernel module")
                } else {
                    crate::tr!("Proprietary kernel module")
                },
                source,
            )
            .technical(),
        );
    }
    let info = proc_dir
        .join("gpus")
        .join(device.address.to_lowercase())
        .join("information");
    if let Some(text) = read(&info)
        && let Some(bios) = text
            .lines()
            .find_map(|l| l.strip_prefix("Video BIOS:"))
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    {
        items.push(Detail::new(
            crate::tr!("Video BIOS version"),
            bios,
            info.display().to_string(),
        ));
    }
    items
}

fn amd_items(device: &Device) -> Vec<Detail> {
    let dir = &device.sysfs_path;
    let mut items = Vec::new();
    let number = |name: &str| read(&dir.join(name)).and_then(|t| t.parse::<u64>().ok());
    let source = |name: &str| dir.join(name).display().to_string();
    if let Some(total) = number("mem_info_vram_total") {
        items.push(Detail::new(
            crate::tr!("Video memory"),
            size_text(total),
            source("mem_info_vram_total"),
        ));
        if let Some(used) = number("mem_info_vram_used") {
            items.push(Detail::new(
                crate::tr!("Video memory in use"),
                size_text(used),
                source("mem_info_vram_used"),
            ));
        }
    }
    if let Some(busy) = number("gpu_busy_percent") {
        items.push(Detail::new(
            crate::tr!("Graphics load"),
            format!("{busy}%"),
            source("gpu_busy_percent"),
        ));
    }
    if let Some(bios) = read(&dir.join("vbios_version")) {
        items.push(Detail::new(
            crate::tr!("Video BIOS version"),
            bios,
            source("vbios_version"),
        ));
    }
    items
}

pub(crate) fn attach(devices: &mut [Device], nvidia_proc: &Path) {
    for device in devices
        .iter_mut()
        .filter(|d| d.category == Category::Graphics)
    {
        let items = match device.driver.as_deref() {
            Some("nvidia") => nvidia_items(device, nvidia_proc),
            Some("amdgpu") => amd_items(device),
            _ => Vec::new(),
        };
        if !items.is_empty() {
            device.details.push(DetailGroup {
                title: crate::tr!("Graphics driver"),
                items,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nvidia_driver_version_is_found_in_the_version_line() {
        let line = "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  615.78.08  Release Build  (notroot@)\nGCC version: x\n";
        assert_eq!(nvidia_driver(line), Some(("615.78.08".to_owned(), true)));
        let closed = "NVRM version: NVIDIA UNIX x86_64 Kernel Module  550.54.14  Fri\n";
        assert_eq!(nvidia_driver(closed), Some(("550.54.14".to_owned(), false)));
        assert_eq!(nvidia_driver("nonsense"), None);
    }
}
