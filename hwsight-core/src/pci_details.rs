//! PCI details: link speed and width, power state, errors, and resources.
//!
//! Everything here comes from the device's sysfs folder and needs no admin rights. The
//! idea of judging the link against the slot above it, and of treating a graphics card's
//! idle slowdown as normal, follows what QLinuxDeviceManager (GPL-3.0) does.

use std::fs;
use std::path::Path;

use crate::details::{Detail, DetailGroup, Verdict, format_bytes};
use crate::device::Category;

fn read(path: &Path, name: &str) -> Option<String> {
    let text = fs::read_to_string(path.join(name)).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn source(path: &Path, name: &str) -> String {
    path.join(name).display().to_string()
}

/// "32.0 GT/s PCIe" -> 32.0
fn parse_speed(text: &str) -> Option<f64> {
    text.split_whitespace().next()?.parse().ok()
}

fn speed_text(gigatransfers: f64) -> String {
    const GENERATIONS: [(f64, &str); 6] = [
        (2.5, "1.0"),
        (5.0, "2.0"),
        (8.0, "3.0"),
        (16.0, "4.0"),
        (32.0, "5.0"),
        (64.0, "6.0"),
    ];
    let rate = format!("{gigatransfers}");
    match GENERATIONS
        .iter()
        .find(|(speed, _)| (gigatransfers - speed).abs() < 0.01)
    {
        Some((_, generation)) => crate::tr!("PCIe {} ({} GT/s)", *generation, rate),
        None => crate::tr!("{} GT/s", rate),
    }
}

/// The fastest speed and widest width the slot above this device allows, if it's a PCI
/// bridge or port we can read.
fn slot_limit(path: &Path) -> Option<(f64, u32)> {
    let real = fs::canonicalize(path).ok()?;
    let parent = real.parent()?;
    let speed = parse_speed(&read(parent, "max_link_speed")?)?;
    let width = read(parent, "max_link_width")?.parse().ok()?;
    Some((speed, width))
}

/// A card's extra functions (its HDMI audio, its USB-C controller) share the card's one
/// physical connection, so they slow down and speed up with it. True when this is function
/// 1 to 7 of a slot whose function 0 is a display controller (PCI class 03xx).
fn shares_link_with_graphics(path: &Path) -> bool {
    let Ok(real) = fs::canonicalize(path) else {
        return false;
    };
    let (Some(name), Some(parent)) = (real.file_name().and_then(|n| n.to_str()), real.parent())
    else {
        return false;
    };
    let Some((slot, function)) = name.rsplit_once('.') else {
        return false;
    };
    if function == "0" {
        return false;
    }
    read(&parent.join(format!("{slot}.0")), "class").is_some_and(|c| c.starts_with("0x03"))
}

fn connection(path: &Path, category: Category) -> Vec<Detail> {
    let mut items = Vec::new();
    let graphics_like = matches!(category, Category::Graphics | Category::Accelerators)
        || shares_link_with_graphics(path);
    let slot = slot_limit(path);

    let current = read(path, "current_link_speed");
    let maximum = read(path, "max_link_speed");
    if let (Some(current_raw), Some(max_raw)) = (current, maximum)
        && let (Some(now), Some(device_max)) = (parse_speed(&current_raw), parse_speed(&max_raw))
    {
        let limit = slot.map_or(device_max, |(speed, _)| speed.min(device_max));
        let (verdict, why) = if now >= limit - 0.01 {
            let why = (limit < device_max - 0.01).then(|| {
                crate::tr!(
                    "The slot this is plugged into only supports {}, so this is as fast as it can go here.",
                    speed_text(limit)
                )
            });
            (Verdict::Good, why)
        } else if category == Category::SystemDevices {
            // Bridges and chipset ports inside the board slow down when nothing is using
            // them. There's nothing for a person to fix, so it must never look like a fault.
            (
                Verdict::Info,
                Some(crate::tr!(
                    "This is a connection inside the computer. It slows down when nothing is using it, to save power. This is normal."
                )),
            )
        } else if graphics_like {
            (
                Verdict::Info,
                Some(crate::tr!(
                    "Graphics cards slow this connection down when nothing is asking much of them, to save power, and speed it up when needed. This is normal. Its full speed can only be seen while it's busy."
                )),
            )
        } else {
            (
                Verdict::Reduced,
                Some(crate::tr!(
                    "This is slower than both the device and its slot can go. Power saving can cause this. If the device feels slow, check its PCIe speed setting in the BIOS."
                )),
            )
        };
        items.push(
            Detail::new(
                crate::tr!("Link speed"),
                speed_text(now),
                source(path, "current_link_speed"),
            )
            .raw(format!("{current_raw} (device maximum {max_raw})"))
            .verdict(verdict, why),
        );
    }

    let current = read(path, "current_link_width").and_then(|t| t.parse::<u32>().ok());
    let maximum = read(path, "max_link_width").and_then(|t| t.parse::<u32>().ok());
    if let (Some(now), Some(device_max)) = (current, maximum) {
        let limit = slot.map_or(device_max, |(_, width)| width.min(device_max));
        let (verdict, why) = if now >= limit {
            (Verdict::Good, None)
        } else if category == Category::SystemDevices {
            (
                Verdict::Info,
                Some(crate::tr!(
                    "This is a connection inside the computer. It slows down when nothing is using it, to save power. This is normal."
                )),
            )
        } else {
            (
                Verdict::Reduced,
                Some(crate::tr!(
                    "Using {} lanes where {} are possible. For a card in a slot this usually means it isn't pushed in fully, or the slot shares lanes with another slot or drive. The motherboard manual says which.",
                    now,
                    limit
                )),
            )
        };
        items.push(
            Detail::new(
                crate::tr!("Link width"),
                format!("x{now}"),
                source(path, "current_link_width"),
            )
            .raw(format!("{now} (device maximum {device_max})"))
            .verdict(verdict, why),
        );
    }
    items
}

fn power(path: &Path) -> Vec<Detail> {
    let mut items = Vec::new();
    if let Some(raw) = read(path, "power_state") {
        let value = match raw.as_str() {
            "D0" => crate::tr!("Active (D0)"),
            "D3hot" => crate::tr!("Powered down (D3hot)"),
            "D3cold" => crate::tr!("Powered off (D3cold)"),
            other => other.to_owned(),
        };
        items.push(
            Detail::new(
                crate::tr!("Power state"),
                value,
                source(path, "power_state"),
            )
            .raw(raw)
            .technical(),
        );
    }
    if let Some(raw) = read(path, "power/runtime_status") {
        let value = match raw.as_str() {
            "active" => crate::tr!("Active"),
            "suspended" => crate::tr!("Suspended to save power"),
            other => other.to_owned(),
        };
        items.push(
            Detail::new(
                crate::tr!("Runtime power saving"),
                value,
                source(path, "power/runtime_status"),
            )
            .raw(raw)
            .technical(),
        );
    }
    items
}

/// The number after `key` in an AER counter file ("TOTAL_ERR_COR 3").
fn error_total(path: &Path, file: &str, key: &str) -> Option<u64> {
    read(path, file)?.lines().find_map(|line| {
        let (name, count) = line.split_once(' ')?;
        (name == key).then(|| count.trim().parse().ok()).flatten()
    })
}

fn errors(path: &Path) -> Vec<Detail> {
    let mut items = Vec::new();
    if let Some(count) = error_total(path, "aer_dev_correctable", "TOTAL_ERR_COR") {
        let (verdict, why) = if count == 0 {
            (Verdict::Good, None)
        } else {
            (
                Verdict::Reduced,
                Some(crate::tr!(
                    "The connection had to repeat some messages. A few are normal; many can mean a card that isn't pushed in fully or a failing slot."
                )),
            )
        };
        let value = if count == 0 {
            crate::tr!("None")
        } else {
            count.to_string()
        };
        items.push(
            Detail::new(
                crate::tr!("Corrected connection errors"),
                value,
                source(path, "aer_dev_correctable"),
            )
            .raw(count.to_string())
            .verdict(verdict, why),
        );
    }
    let serious = ["aer_dev_nonfatal", "aer_dev_fatal"]
        .iter()
        .zip(["TOTAL_ERR_NONFATAL", "TOTAL_ERR_FATAL"])
        .filter_map(|(file, key)| error_total(path, file, key))
        .reduce(u64::saturating_add);
    if let Some(count) = serious {
        let (verdict, why) = if count == 0 {
            (Verdict::Good, None)
        } else {
            (
                Verdict::Problem,
                Some(crate::tr!(
                    "The connection reported errors it couldn't recover from. Try pushing the card in again, or another slot."
                )),
            )
        };
        let value = if count == 0 {
            crate::tr!("None")
        } else {
            count.to_string()
        };
        items.push(
            Detail::new(
                crate::tr!("Serious connection errors"),
                value,
                source(path, "aer_dev_nonfatal"),
            )
            .raw(count.to_string())
            .verdict(verdict, why),
        );
    }
    items
}

fn parse_hex(text: &str) -> Option<u64> {
    u64::from_str_radix(text.trim_start_matches("0x"), 16).ok()
}

/// Memory and I/O windows from the `resource` file: six BARs, then the expansion ROM.
fn windows(path: &Path) -> Vec<Detail> {
    let Some(text) = read(path, "resource") else {
        return Vec::new();
    };
    let explanation = crate::tr!("A window the device uses to exchange data with the computer.");
    let mut items = Vec::new();
    for (index, line) in text.lines().take(7).enumerate() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let [start, end, flags] = parts.as_slice() else {
            continue;
        };
        let (Some(start_value), Some(end_value), Some(flags_value)) =
            (parse_hex(start), parse_hex(end), parse_hex(flags))
        else {
            continue;
        };
        if (start_value == 0 && end_value == 0) || end_value < start_value {
            continue;
        }
        let size = format_bytes(end_value - start_value + 1);
        let label = if index == 6 {
            crate::tr!("Expansion ROM")
        } else if flags_value & 0x100 != 0 {
            crate::tr!("I/O ports window {}", index)
        } else {
            crate::tr!("Memory window {}", index)
        };
        let value = size;
        items.push(
            Detail::new(label, value, source(path, "resource"))
                .raw(format!("{start} {end} {flags}"))
                .verdict(Verdict::Info, Some(explanation.clone()))
                .technical(),
        );
    }
    items
}

fn resources(path: &Path) -> Vec<Detail> {
    let mut items = Vec::new();
    if let Some(irq) = read(path, "irq").and_then(|t| t.parse::<u32>().ok())
        && irq != 0
    {
        items.push(
            Detail::new(
                crate::tr!("Interrupt (IRQ)"),
                irq.to_string(),
                source(path, "irq"),
            )
            .verdict(
                Verdict::Info,
                Some(crate::tr!(
                    "The line the device uses to get the processor's attention."
                )),
            )
            .technical(),
        );
    }
    if let Some(group) = fs::read_link(path.join("iommu_group"))
        .ok()
        .and_then(|target| target.file_name().map(|n| n.to_string_lossy().into_owned()))
    {
        items.push(
            Detail::new(
                crate::tr!("IOMMU group"),
                group,
                source(path, "iommu_group"),
            )
            .verdict(
                Verdict::Info,
                Some(crate::tr!(
                    "Devices in the same group can only be passed through to a virtual machine together."
                )),
            )
            .technical(),
        );
        // The group's other members decide whether this device can be passed to a
        // virtual machine on its own (idea from Chassis, GPL-3.0).
        let mut others: Vec<String> = fs::read_dir(path.join("iommu_group/devices"))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        others.sort();
        others.retain(|name| {
            path.file_name()
                .is_none_or(|own| own.to_string_lossy() != *name)
        });
        if !others.is_empty() {
            items.push(
                Detail::new(
                    crate::tr!("Shares its IOMMU group with"),
                    others.join(", "),
                    source(path, "iommu_group/devices"),
                )
                .verdict(
                    Verdict::Info,
                    Some(crate::tr!(
                        "To pass this device to a virtual machine, these have to be passed along with it."
                    )),
                )
                .technical(),
            );
        }
    }
    if let Some(node) = read(path, "numa_node").and_then(|t| t.parse::<i32>().ok())
        && node >= 0
    {
        items.push(
            Detail::new(
                crate::tr!("NUMA node"),
                node.to_string(),
                source(path, "numa_node"),
            )
            .verdict(
                Verdict::Info,
                Some(crate::tr!(
                    "Which group of processor cores and memory this device sits closest to."
                )),
            )
            .technical(),
        );
    }
    items.extend(windows(path));
    items
}

/// Every group of details for the PCI device whose sysfs folder is `path`. Groups with
/// nothing in them are left out.
/// "256 MB" / "16 GB" for a size in bytes.
fn size_text(bytes: u64) -> String {
    let mib = bytes >> 20;
    if mib >= 1024 {
        crate::tr!("{} GB", mib / 1024)
    } else {
        crate::tr!("{} MB", mib)
    }
}

/// Resizable BAR on a graphics card: whether the processor can reach all of the card's
/// video memory at once. The card lists the window sizes it supports in
/// `resourceN_resize` (bit 0 is 1 MB, bit 1 is 2 MB, and so on) and the window now in use
/// is the size of BAR N in `resource`. A card that supports bigger windows but runs at
/// 256 MB or less has the BIOS feature switched off.
fn resizable_bar(path: &Path, category: Category) -> Vec<Detail> {
    if category != Category::Graphics {
        return Vec::new();
    }
    let Some(resource) = read(path, "resource") else {
        return Vec::new();
    };
    let lines: Vec<&str> = resource.lines().collect();
    for bar in 0..6 {
        let name = format!("resource{bar}_resize");
        let Some(mask) = read(path, &name).and_then(|t| parse_hex(&t)) else {
            continue;
        };
        let Some(max_bit) = (0..64).rev().find(|bit| mask >> bit & 1 == 1) else {
            continue;
        };
        let max = 1u64 << (20 + max_bit);
        let current = lines.get(bar).and_then(|line| {
            let mut parts = line.split_whitespace();
            let start = parse_hex(parts.next()?)?;
            let end = parse_hex(parts.next()?)?;
            (end > start).then(|| end - start + 1)
        });
        let Some(current) = current else { continue };
        let value = crate::tr!("{} of {}", size_text(current), size_text(max));
        let detail = Detail::new(crate::tr!("Resizable BAR"), value, source(path, &name));
        let detail = if current >= max {
            detail.verdict(
                Verdict::Good,
                Some(crate::tr!(
                    "The processor can reach all of the card's video memory at once, which is what games want."
                )),
            )
        } else if current <= 256 << 20 {
            detail.verdict(
                Verdict::Reduced,
                Some(crate::tr!(
                    "Resizable BAR (also called Smart Access Memory) is switched off, so the processor can only reach a small part of the card's video memory at a time. Games can run a little slower. To turn it on, look in the BIOS for \"Resizable BAR\" and \"Above 4G Decoding\" and enable both."
                )),
            )
        } else {
            detail.verdict(
                Verdict::Info,
                Some(crate::tr!(
                    "The processor reaches part of the card's video memory at once, which is more than the old limit but less than the card allows."
                )),
            )
        };
        return vec![detail];
    }
    Vec::new()
}

pub(crate) fn read_details(path: &Path, category: Category) -> Vec<DetailGroup> {
    [
        (crate::tr!("PCIe connection"), connection(path, category)),
        (crate::tr!("Connection errors"), errors(path)),
        (
            crate::tr!("Video memory access"),
            resizable_bar(path, category),
        ),
        (crate::tr!("Power"), power(path)),
        (crate::tr!("Resources"), resources(path)),
    ]
    .into_iter()
    .filter(|(_, items)| !items.is_empty())
    .map(|(title, items)| DetailGroup { title, items })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::in_english;

    fn device(files: &[(&str, &str)]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        for (name, text) in files {
            let file = temp.path().join(name);
            if let Some(dir) = file.parent() {
                fs::create_dir_all(dir).expect("mkdir");
            }
            fs::write(file, text).expect("write");
        }
        temp
    }

    fn item<'a>(groups: &'a [DetailGroup], label: &str) -> &'a Detail {
        groups
            .iter()
            .flat_map(|g| &g.items)
            .find(|d| d.label == label)
            .unwrap_or_else(|| panic!("no detail {label:?} in {groups:#?}"))
    }

    const FULL: [(&str, &str); 4] = [
        ("current_link_speed", "32.0 GT/s PCIe\n"),
        ("current_link_width", "16\n"),
        ("max_link_speed", "32.0 GT/s PCIe\n"),
        ("max_link_width", "16\n"),
    ];

    #[test]
    fn a_link_at_full_speed_is_good() {
        let temp = device(&FULL);
        let groups = in_english(|| read_details(temp.path(), Category::Storage));
        let speed = item(&groups, "Link speed");
        assert_eq!(speed.value, "PCIe 5.0 (32 GT/s)");
        assert_eq!(speed.verdict, Verdict::Good);
        assert_eq!(speed.explanation, None);
        assert_eq!(
            speed.raw.as_deref(),
            Some("32.0 GT/s PCIe (device maximum 32.0 GT/s PCIe)")
        );
        assert_eq!(item(&groups, "Link width").value, "x16");
        assert_eq!(item(&groups, "Link width").verdict, Verdict::Good);
        assert!(!speed.technical);
    }

    #[test]
    fn a_graphics_card_at_idle_speed_is_not_a_fault_but_other_devices_are_reduced() {
        let temp = device(&[
            ("current_link_speed", "2.5 GT/s PCIe\n"),
            ("current_link_width", "16\n"),
            ("max_link_speed", "32.0 GT/s PCIe\n"),
            ("max_link_width", "16\n"),
        ]);
        let gpu = in_english(|| read_details(temp.path(), Category::Graphics));
        let speed = item(&gpu, "Link speed");
        assert_eq!(speed.verdict, Verdict::Info);
        assert!(
            speed
                .explanation
                .as_deref()
                .is_some_and(|t| t.contains("This is normal"))
        );
        let drive = in_english(|| read_details(temp.path(), Category::Storage));
        assert_eq!(item(&drive, "Link speed").verdict, Verdict::Reduced);
    }

    #[test]
    fn a_graphics_cards_audio_function_gets_the_same_idle_pass_as_the_card() {
        let root = tempfile::tempdir().expect("tempdir");
        for function in ["0", "1"] {
            let dir = root.path().join(format!("0000:01:00.{function}"));
            fs::create_dir_all(&dir).expect("mkdir");
            let class = if function == "0" {
                "0x030000\n"
            } else {
                "0x040300\n"
            };
            for (name, text) in [
                ("class", class),
                ("current_link_speed", "2.5 GT/s PCIe\n"),
                ("max_link_speed", "32.0 GT/s PCIe\n"),
            ] {
                fs::write(dir.join(name), text).expect("write");
            }
        }
        let audio = root.path().join("0000:01:00.1");
        let groups = in_english(|| read_details(&audio, Category::Sound));
        let speed = item(&groups, "Link speed");
        assert_eq!(speed.verdict, Verdict::Info, "{speed:#?}");
        assert!(
            speed
                .explanation
                .as_deref()
                .unwrap_or("")
                .contains("Graphics cards slow")
        );
        // A sound card with no graphics sibling is still flagged.
        let lone = root.path().join("0000:02:00.1");
        fs::create_dir_all(&lone).expect("mkdir");
        for (name, text) in [
            ("class", "0x040300\n"),
            ("current_link_speed", "2.5 GT/s PCIe\n"),
            ("max_link_speed", "32.0 GT/s PCIe\n"),
        ] {
            fs::write(lone.join(name), text).expect("write");
        }
        let groups = in_english(|| read_details(&lone, Category::Sound));
        assert_eq!(item(&groups, "Link speed").verdict, Verdict::Reduced);
    }

    #[test]
    fn links_inside_the_board_are_never_flagged() {
        let temp = device(&[
            ("current_link_speed", "2.5 GT/s PCIe\n"),
            ("current_link_width", "1\n"),
            ("max_link_speed", "16.0 GT/s PCIe\n"),
            ("max_link_width", "4\n"),
        ]);
        let groups = in_english(|| read_details(temp.path(), Category::SystemDevices));
        assert_eq!(item(&groups, "Link speed").verdict, Verdict::Info);
        assert_eq!(item(&groups, "Link width").verdict, Verdict::Info);
        // The same readings on a card in a slot are worth a look.
        let card = in_english(|| read_details(temp.path(), Category::Network));
        assert_eq!(item(&card, "Link speed").verdict, Verdict::Reduced);
        assert_eq!(item(&card, "Link width").verdict, Verdict::Reduced);
    }

    #[test]
    fn a_narrow_link_is_reduced_and_says_how_many_lanes() {
        let temp = device(&[
            ("current_link_speed", "16.0 GT/s PCIe\n"),
            ("current_link_width", "4\n"),
            ("max_link_speed", "16.0 GT/s PCIe\n"),
            ("max_link_width", "16\n"),
        ]);
        let groups = in_english(|| read_details(temp.path(), Category::Graphics));
        let width = item(&groups, "Link width");
        assert_eq!(width.value, "x4");
        assert_eq!(width.verdict, Verdict::Reduced);
        assert!(
            width
                .explanation
                .as_deref()
                .is_some_and(|t| t.contains("Using 4 lanes where 16 are possible"))
        );
    }

    #[test]
    fn the_slot_above_limits_what_counts_as_full_speed() {
        // A PCIe 5.0 device in a PCIe 4.0 slot, running at the slot's speed.
        let temp = device(&[
            ("slot/max_link_speed", "16.0 GT/s PCIe\n"),
            ("slot/max_link_width", "8\n"),
            ("slot/card/current_link_speed", "16.0 GT/s PCIe\n"),
            ("slot/card/current_link_width", "8\n"),
            ("slot/card/max_link_speed", "32.0 GT/s PCIe\n"),
            ("slot/card/max_link_width", "16\n"),
        ]);
        let groups = in_english(|| read_details(&temp.path().join("slot/card"), Category::Storage));
        let speed = item(&groups, "Link speed");
        assert_eq!(speed.verdict, Verdict::Good);
        assert!(
            speed
                .explanation
                .as_deref()
                .is_some_and(|t| t.contains("only supports PCIe 4.0 (16 GT/s)"))
        );
        assert_eq!(item(&groups, "Link width").verdict, Verdict::Good);
    }

    #[test]
    fn connection_errors_are_judged() {
        let temp = device(&[
            (
                "aer_dev_correctable",
                "RxErr 0\nBadTLP 2\nTOTAL_ERR_COR 3\n",
            ),
            ("aer_dev_nonfatal", "Undefined 0\nTOTAL_ERR_NONFATAL 0\n"),
            ("aer_dev_fatal", "Undefined 0\nTOTAL_ERR_FATAL 1\n"),
        ]);
        let groups = in_english(|| read_details(temp.path(), Category::Network));
        let corrected = item(&groups, "Corrected connection errors");
        assert_eq!(corrected.value, "3");
        assert_eq!(corrected.verdict, Verdict::Reduced);
        let serious = item(&groups, "Serious connection errors");
        assert_eq!(serious.value, "1");
        assert_eq!(serious.verdict, Verdict::Problem);

        let clean = device(&[
            ("aer_dev_correctable", "TOTAL_ERR_COR 0\n"),
            ("aer_dev_nonfatal", "TOTAL_ERR_NONFATAL 0\n"),
            ("aer_dev_fatal", "TOTAL_ERR_FATAL 0\n"),
        ]);
        let groups = in_english(|| read_details(clean.path(), Category::Network));
        assert_eq!(item(&groups, "Corrected connection errors").value, "None");
        assert_eq!(
            item(&groups, "Serious connection errors").verdict,
            Verdict::Good
        );
    }

    #[test]
    fn power_and_resources_are_technical() {
        let temp = device(&[
            ("power_state", "D3hot\n"),
            ("power/runtime_status", "suspended\n"),
            ("irq", "168\n"),
            ("numa_node", "-1\n"),
            (
                "resource",
                "0x00000000f6000000 0x00000000f6ffffff 0x0000000000040200\n\
                 0x0000000000000000 0x0000000000000000 0x0000000000000000\n\
                 0x000000000000e000 0x000000000000e07f 0x0000000000040101\n\
                 0x00000000f7000000 0x00000000f707ffff 0x0000000000000200\n",
            ),
        ]);
        fs::create_dir_all(temp.path().join("groups/14/devices")).expect("mkdir");
        fs::write(temp.path().join("groups/14/devices/0000:01:00.1"), "").expect("write");
        let own_name = temp.path().file_name().expect("name").to_owned();
        fs::write(temp.path().join("groups/14/devices").join(own_name), "").expect("write");
        std::os::unix::fs::symlink(
            temp.path().join("groups/14"),
            temp.path().join("iommu_group"),
        )
        .expect("link");
        let groups = in_english(|| read_details(temp.path(), Category::Graphics));
        assert_eq!(item(&groups, "Power state").value, "Powered down (D3hot)");
        assert_eq!(
            item(&groups, "Runtime power saving").value,
            "Suspended to save power"
        );
        assert_eq!(item(&groups, "Interrupt (IRQ)").value, "168");
        assert_eq!(item(&groups, "IOMMU group").value, "14");
        assert_eq!(
            item(&groups, "Shares its IOMMU group with").value,
            "0000:01:00.1",
            "the group's other members, not the device itself"
        );
        assert_eq!(item(&groups, "Memory window 0").value, "16 MiB");
        assert_eq!(item(&groups, "I/O ports window 2").value, "128 bytes");
        assert_eq!(item(&groups, "Memory window 3").value, "512 KiB");
        assert!(
            groups
                .iter()
                .flat_map(|g| &g.items)
                .all(|d| d.label != "Memory window 1" && d.label != "NUMA node"),
            "empty windows and an unknown NUMA node are left out"
        );
        assert!(
            groups
                .iter()
                .flat_map(|g| &g.items)
                .filter(|d| d.label != "Link speed" && d.label != "Link width")
                .all(|d| d.technical || d.label.contains("errors"))
        );
    }

    #[test]
    fn resizable_bar_is_good_when_on_and_reduced_when_off() {
        let on = "0x00000000fb000000 0x00000000fbffffff 0x40200\n0x0000004000000000 0x00000043ffffffff 0x14220c\n";
        let off = "0x00000000fb000000 0x00000000fbffffff 0x40200\n0x0000004000000000 0x000000400fffffff 0x14220c\n";
        for (resource, verdict, value) in [
            (on, Verdict::Good, "16 GB of 16 GB"),
            (off, Verdict::Reduced, "256 MB of 16 GB"),
        ] {
            let d = device(&[
                ("resource", resource),
                ("resource1_resize", "0000000000007fc0\n"),
            ]);
            let groups = in_english(|| read_details(d.path(), Category::Graphics));
            let bar = item(&groups, "Resizable BAR");
            assert_eq!(bar.value, value);
            assert_eq!(bar.verdict, verdict);
        }
        let d = device(&[("resource", on), ("resource1_resize", "0000000000007fc0\n")]);
        let groups = in_english(|| read_details(d.path(), Category::Network));
        assert!(groups.iter().all(|g| g.title != "Video memory access"));
    }

    #[test]
    fn a_device_with_nothing_readable_has_no_details() {
        let temp = device(&[]);
        assert!(read_details(temp.path(), Category::Other).is_empty());
        let junk = device(&[
            ("current_link_speed", "Unknown speed\n"),
            ("max_link_speed", "Unknown speed\n"),
            ("resource", "garbage\n"),
        ]);
        assert!(read_details(junk.path(), Category::Other).is_empty());
    }
}
