//! Memory (RAM): the sticks installed, what speed they run at and could run at, and how
//! they are spread across the memory channels.
//!
//! The sticks come from the firmware's memory table, which udev saves in a file anyone
//! can read (`/run/udev/data/+dmi:id`); what each stick can do comes from its SPD chip
//! (see `spd.rs`). Reading the table this way, and the rules for empty slots and
//! placeholder text, follow QLinuxDeviceManager (GPL-3.0).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::descriptions::Descriptions;
use crate::details::{Detail, DetailGroup, Verdict};
use crate::device::{BuiltIn, Bus, Category, Device, short_vendor};
use crate::spd::Spd;
use crate::system::clean;

/// One memory slot on the motherboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Slot {
    /// The label printed on the board, e.g. "DIMM 1".
    pub locator: String,
    /// The memory channel, e.g. "A". Empty when the firmware doesn't say.
    pub channel: String,
    /// `None` for an empty slot.
    pub size_bytes: Option<u64>,
    pub kind: Option<String>,
    pub manufacturer: Option<String>,
    pub part_number: Option<String>,
    /// The speed it's running at, in MT/s.
    pub configured_mts: Option<u32>,
}

/// "P0 CHANNEL A" -> "A".
fn channel_name(bank: &str) -> String {
    let upper = bank.to_uppercase();
    match upper.find("CHANNEL") {
        Some(at) => bank[at + "CHANNEL".len()..].trim().to_owned(),
        None => bank.trim().to_owned(),
    }
}

/// The slots, and the largest total the board supports in bytes, from the udev database
/// text for the `dmi/id` device.
pub(crate) fn parse_slots(text: &str) -> (Vec<Slot>, Option<u64>) {
    let properties: HashMap<String, String> = text
        .lines()
        .filter_map(|line| line.strip_prefix("E:")?.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    let number = |key: &str| {
        properties
            .get(key)
            .and_then(|v| v.trim().parse::<u64>().ok())
    };
    let count = number("MEMORY_ARRAY_NUM_DEVICES").unwrap_or(0).min(64);
    let slots = (0..count)
        .map(|index| {
            let get = |suffix: &str| properties.get(&format!("MEMORY_DEVICE_{index}_{suffix}"));
            let present = get("PRESENT").is_none_or(|p| p.trim() != "0");
            Slot {
                locator: clean(get("LOCATOR")).unwrap_or_else(|| format!("#{}", index + 1)),
                channel: clean(get("BANK_LOCATOR"))
                    .map(|b| channel_name(&b))
                    .unwrap_or_default(),
                size_bytes: get("SIZE")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .filter(|size| present && *size > 0),
                kind: clean(get("TYPE")),
                manufacturer: clean(get("MANUFACTURER")),
                part_number: clean(get("PART_NUMBER")),
                configured_mts: get("CONFIGURED_SPEED_MTS")
                    .and_then(|v| v.trim().parse().ok())
                    .filter(|mts| *mts > 0),
            }
        })
        .collect();
    (slots, number("MEMORY_ARRAY_MAX_CAPACITY"))
}

/// Bytes as gigabytes the way memory is sold: 17179869184 -> "16".
fn gigabytes(bytes: u64) -> String {
    const GB: u64 = 1 << 30;
    if bytes.is_multiple_of(GB) {
        (bytes / GB).to_string()
    } else {
        format!("{:.1}", bytes as f64 / GB as f64)
    }
}

fn speed_text(mts: u32) -> String {
    format!("{mts} MT/s")
}

/// The SPD chip for each stick, matched by part number (the chip's address says nothing
/// about which slot it's in). `None` where there's no chip to read.
fn match_spd<'a>(sticks: &[&Slot], spds: &'a [Spd]) -> Vec<Option<&'a Spd>> {
    let mut unused: Vec<Option<&Spd>> = spds.iter().map(Some).collect();
    sticks
        .iter()
        .map(|stick| {
            let part = stick.part_number.as_deref()?.trim();
            let found = unused.iter_mut().find(|candidate| {
                candidate.is_some_and(|spd| spd.part_number.eq_ignore_ascii_case(part))
            })?;
            found.take()
        })
        .collect()
}

fn summary(
    dmi: &Path,
    sticks: &[&Slot],
    slots: &[Slot],
    max_capacity: Option<u64>,
    rated: Option<u32>,
) -> DetailGroup {
    let source = format!("{} (MEMORY_DEVICE_*)", dmi.display());
    let total: u64 = sticks.iter().filter_map(|s| s.size_bytes).sum();
    let mut items = vec![Detail::new(
        crate::tr!("Installed memory"),
        format!("{} GB", gigabytes(total)),
        source.clone(),
    )];
    if let Some(kind) = sticks.iter().find_map(|s| s.kind.clone()) {
        items.push(Detail::new(crate::tr!("Memory type"), kind, source.clone()));
    }
    items.push(Detail::new(
        crate::tr!("Slots used"),
        crate::tr!("{} of {}", sticks.len(), slots.len()),
        source.clone(),
    ));

    let running = sticks.iter().filter_map(|s| s.configured_mts).min();
    if let Some(running) = running {
        let (verdict, why) = match rated {
            Some(rated) if running + 400 < rated => (
                Verdict::Reduced,
                Some(crate::tr!(
                    "Your memory is running at {} MT/s, but every stick in it is rated for at least {} MT/s. A memory profile (called XMP, EXPO, or DOCP) in the BIOS turns the faster speed on. If you set it lower on purpose to keep things stable, that's fine.",
                    running,
                    rated
                )),
            ),
            Some(_) => (Verdict::Good, None),
            None => (Verdict::Info, None),
        };
        items.push(
            Detail::new(
                crate::tr!("Running speed"),
                speed_text(running),
                source.clone(),
            )
            .verdict(verdict, why),
        );
    }
    if let Some(rated) = rated {
        items.push(
            Detail::new(
                crate::tr!("Rated speed"),
                speed_text(rated),
                crate::tr!("The memory sticks' own chips (SPD)"),
            )
            .verdict(
                Verdict::Info,
                Some(crate::tr!(
                    "The fastest speed every stick says it can run at, using its factory profile. The slowest stick sets this."
                )),
            ),
        );
    }

    if sticks.len() >= 2 {
        let parts: Vec<&str> = sticks
            .iter()
            .filter_map(|s| s.part_number.as_deref())
            .collect();
        let same = parts.len() == sticks.len() && parts.windows(2).all(|w| w[0] == w[1]);
        items.push(if same {
            Detail::new(
                crate::tr!("Matching sticks"),
                crate::tr!("All the same model"),
                source.clone(),
            )
            .verdict(Verdict::Good, None)
        } else {
            Detail::new(
                crate::tr!("Matching sticks"),
                crate::tr!("Different models"),
                source.clone(),
            )
            .verdict(
                Verdict::Info,
                Some(crate::tr!(
                    "These sticks are not all the same model. Mixed sticks can only run as fast as the slowest one, and sometimes need a lower speed to stay stable."
                )),
            )
        });
    }

    let mut channels: BTreeMap<&str, usize> = BTreeMap::new();
    for slot in slots.iter().filter(|s| !s.channel.is_empty()) {
        *channels.entry(slot.channel.as_str()).or_default() += 0;
    }
    for stick in sticks.iter().filter(|s| !s.channel.is_empty()) {
        *channels.entry(stick.channel.as_str()).or_default() += 1;
    }
    if !channels.is_empty() {
        let even = channels
            .values()
            .all(|n| *n == channels.values().copied().next().unwrap_or(0));
        let crowded = channels.values().any(|n| *n >= 2);
        let value = channels
            .iter()
            .map(|(channel, count)| format!("{channel}: {count}"))
            .collect::<Vec<_>>()
            .join(", ");
        let (verdict, why) = if even {
            (Verdict::Good, None)
        } else {
            let mut why = crate::tr!(
                "The sticks aren't spread evenly across the memory channels. The same number of sticks on each channel gives the most speed and memory bandwidth."
            );
            if crowded {
                why.push(' ');
                why.push_str(&crate::tr!(
                    "Two sticks on one channel can also make it harder to run at high speeds."
                ));
            }
            (Verdict::Reduced, Some(why))
        };
        items.push(
            Detail::new(
                crate::tr!("Sticks per memory channel"),
                value,
                source.clone(),
            )
            .verdict(verdict, why),
        );
    }
    if let Some(max) = max_capacity {
        items.push(
            Detail::new(
                crate::tr!("Most memory the board supports"),
                format!("{} GB", gigabytes(max)),
                source,
            )
            .technical(),
        );
    }
    DetailGroup {
        title: crate::tr!("Memory"),
        items,
    }
}

fn stick_group(stick: &Slot, spd: Option<&Spd>, dmi: &Path, spd_dir: &Path) -> DetailGroup {
    let source = format!("{} (MEMORY_DEVICE_*)", dmi.display());
    let title = if stick.channel.is_empty() {
        crate::tr!("Memory stick in {}", stick.locator)
    } else {
        crate::tr!(
            "Memory stick in {}, channel {}",
            stick.locator,
            stick.channel
        )
    };
    let mut items = Vec::new();
    if let Some(size) = stick.size_bytes {
        items.push(Detail::new(
            crate::tr!("Size"),
            format!("{} GB", gigabytes(size)),
            source.clone(),
        ));
    }
    if let Some(maker) = &stick.manufacturer {
        items.push(Detail::new(
            crate::tr!("Maker"),
            maker.clone(),
            source.clone(),
        ));
    }
    if let Some(part) = &stick.part_number {
        items.push(Detail::new(
            crate::tr!("Part number"),
            part.clone(),
            source.clone(),
        ));
    }
    if let Some(spd) = spd {
        let chip = spd_dir.display().to_string();
        let mut speeds = spd.xmp_mts.clone();
        if speeds.is_empty() {
            speeds.extend(spd.base_mts);
        }
        if !speeds.is_empty() {
            let list = speeds
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            items.push(
                Detail::new(crate::tr!("Rated speeds"), format!("{list} MT/s"), chip.clone())
                    .verdict(
                        Verdict::Info,
                        Some(crate::tr!(
                            "The speeds this stick was sold for. A memory profile (XMP, EXPO, or DOCP) in the BIOS turns them on."
                        )),
                    ),
            );
        }
        if let Some(base) = spd.base_mts {
            items.push(
                Detail::new(crate::tr!("Standard speed"), speed_text(base), chip.clone())
                    .technical(),
            );
        }
        let mut profiles = Vec::new();
        if !spd.xmp_mts.is_empty() {
            profiles.push("XMP");
        }
        if spd.has_expo {
            profiles.push("EXPO");
        }
        if !profiles.is_empty() {
            items.push(
                Detail::new(crate::tr!("Memory profiles"), profiles.join(", "), chip).technical(),
            );
        }
    }
    if let Some(mts) = stick.configured_mts {
        items.push(Detail::new(crate::tr!("Speed in use"), speed_text(mts), source).technical());
    }
    DetailGroup { title, items }
}

/// The memory as one device, or `None` when the firmware lists no sticks (a virtual
/// machine, or a computer whose firmware doesn't say).
pub(crate) fn device(
    dmi_data: &str,
    dmi_path: &Path,
    spd_dir: &Path,
    spds: &[Spd],
    descriptions: &Descriptions,
) -> Option<Device> {
    let (slots, max_capacity) = parse_slots(dmi_data);
    let sticks: Vec<&Slot> = slots.iter().filter(|s| s.size_bytes.is_some()).collect();
    if sticks.is_empty() {
        return None;
    }
    let matched = match_spd(&sticks, spds);
    let rated = matched
        .iter()
        .map(|spd| spd.and_then(Spd::best_mts))
        .collect::<Option<Vec<_>>>()
        .and_then(|speeds| speeds.into_iter().min());

    let mut groups = vec![summary(dmi_path, &sticks, &slots, max_capacity, rated)];
    for (stick, spd) in sticks.iter().zip(&matched) {
        groups.push(stick_group(stick, *spd, dmi_path, spd_dir));
    }

    let total: u64 = sticks.iter().filter_map(|s| s.size_bytes).sum();
    let kind = sticks.iter().find_map(|s| s.kind.clone());
    let display_name = match &kind {
        Some(kind) => crate::tr!("Memory ({} GB {})", gigabytes(total), kind),
        None => crate::tr!("Memory ({} GB)", gigabytes(total)),
    };
    let makers: Vec<&String> = sticks
        .iter()
        .filter_map(|s| s.manufacturer.as_ref())
        .collect();
    let vendor = (makers.len() == sticks.len() && makers.windows(2).all(|w| w[0] == w[1]))
        .then(|| makers.first().map(|m| short_vendor(m)))
        .flatten();
    let mut parts: Vec<String> = Vec::new();
    for part in sticks.iter().filter_map(|s| s.part_number.clone()) {
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    let description = descriptions
        .describe(Bus::System, &[], 0, 0, Category::Memory)
        .unwrap_or_default();
    Some(Device::built_in(BuiltIn {
        address: "memory",
        sysfs_path: dmi_path.to_path_buf(),
        display_name,
        vendor_name: vendor,
        category: Category::Memory,
        hardware_id: parts.join(", "),
        description,
        details: groups,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::in_english;
    use crate::spd::tests::fake_spd;

    /// The udev text for a memory table. Each slot: (locator, channel, size in GB or 0
    /// for empty, part number, running speed).
    fn table(slots: &[(&str, &str, u64, &str, u32)]) -> String {
        let mut text = format!(
            "E:MEMORY_ARRAY_MAX_CAPACITY=274877906944\nE:MEMORY_ARRAY_NUM_DEVICES={}\n",
            slots.len()
        );
        for (i, (locator, channel, gb, part, mts)) in slots.iter().enumerate() {
            text.push_str(&format!(
                "E:MEMORY_DEVICE_{i}_LOCATOR={locator}\nE:MEMORY_DEVICE_{i}_BANK_LOCATOR=P0 CHANNEL {channel}\n"
            ));
            if *gb == 0 {
                text.push_str(&format!("E:MEMORY_DEVICE_{i}_PRESENT=0\n"));
            } else {
                text.push_str(&format!(
                    "E:MEMORY_DEVICE_{i}_SIZE={}\nE:MEMORY_DEVICE_{i}_TYPE=DDR5\nE:MEMORY_DEVICE_{i}_MANUFACTURER=Team Group Inc\nE:MEMORY_DEVICE_{i}_PART_NUMBER={part}\nE:MEMORY_DEVICE_{i}_CONFIGURED_SPEED_MTS={mts}\n",
                    gb << 30
                ));
            }
        }
        text
    }

    fn build(text: &str, spds: &[Spd]) -> Option<Device> {
        in_english(|| {
            device(
                text,
                Path::new("/run/udev/data/+dmi:id"),
                Path::new("/sys/bus/i2c/drivers/spd5118"),
                spds,
                &Descriptions::embedded(),
            )
        })
    }

    fn spd(part: &str, xmp: &[u16]) -> Spd {
        crate::spd::parse_ddr5(&fake_spd(part, 416, xmp, false)).expect("ddr5")
    }

    fn item<'a>(device: &'a Device, label: &str) -> &'a Detail {
        device
            .details
            .iter()
            .flat_map(|g| &g.items)
            .find(|d| d.label == label)
            .unwrap_or_else(|| panic!("no detail {label:?}"))
    }

    /// The owner's memory: 3 sticks, 2 different models, running at 4800.
    fn owners_table() -> String {
        table(&[
            ("DIMM 0", "A", 0, "", 0),
            ("DIMM 1", "A", 16, "UD5-6400", 4800),
            ("DIMM 0", "B", 16, "UD5-6000", 4800),
            ("DIMM 1", "B", 16, "UD5-6400", 4800),
        ])
    }

    fn owners_spds() -> Vec<Spd> {
        vec![
            spd("UD5-6400", &[312, 333]),
            spd("UD5-6000", &[333, 357]),
            spd("UD5-6400", &[312, 333]),
        ]
    }

    #[test]
    fn reads_slots_skipping_empty_ones_and_placeholders() {
        let mut text = owners_table();
        text.push_str("E:MEMORY_DEVICE_0_MANUFACTURER=Unknown\nE:MEMORY_DEVICE_2_SERIAL_NUMBER=To Be Filled By O.E.M.\n");
        let (slots, max) = parse_slots(&text);
        assert_eq!(slots.len(), 4);
        assert_eq!(slots[0].size_bytes, None, "PRESENT=0 is an empty slot");
        assert_eq!(slots[0].manufacturer, None, "placeholders are dropped");
        assert_eq!(slots[1].size_bytes, Some(16 << 30));
        assert_eq!(slots[1].channel, "A");
        assert_eq!(slots[3].locator, "DIMM 1");
        assert_eq!(slots[2].configured_mts, Some(4800));
        assert_eq!(max, Some(256 << 30));
    }

    #[test]
    fn the_owners_memory_is_judged_slow_mixed_and_unbalanced() {
        let d = build(&owners_table(), &owners_spds()).expect("memory");
        assert_eq!(d.display_name, "Memory (48 GB DDR5)");
        assert_eq!(d.vendor_name.as_deref(), Some("Team Group"));
        assert_eq!(d.hardware_id, "UD5-6400, UD5-6000");
        assert_eq!(d.category, Category::Memory);
        assert_eq!(d.bus, Bus::System);
        assert!(
            d.description
                .as_deref()
                .is_some_and(|t| t.contains("working memory"))
        );

        assert_eq!(item(&d, "Installed memory").value, "48 GB");
        assert_eq!(item(&d, "Slots used").value, "3 of 4");
        let running = item(&d, "Running speed");
        assert_eq!(running.value, "4800 MT/s");
        assert_eq!(running.verdict, Verdict::Reduced);
        assert!(
            running
                .explanation
                .as_deref()
                .is_some_and(|t| t.contains("rated for at least 6000 MT/s"))
        );
        assert_eq!(
            item(&d, "Rated speed").value,
            "6000 MT/s",
            "the slowest stick sets it"
        );
        assert_eq!(item(&d, "Matching sticks").value, "Different models");
        let channels = item(&d, "Sticks per memory channel");
        assert_eq!(channels.value, "A: 1, B: 2");
        assert_eq!(channels.verdict, Verdict::Reduced);
        assert!(
            channels
                .explanation
                .as_deref()
                .is_some_and(|t| t.contains("Two sticks on one channel"))
        );
        assert!(item(&d, "Most memory the board supports").technical);
    }

    #[test]
    fn each_stick_has_its_own_group_with_its_rated_speeds() {
        let d = build(&owners_table(), &owners_spds()).expect("memory");
        let titles: Vec<_> = d.details.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Memory",
                "Memory stick in DIMM 1, channel A",
                "Memory stick in DIMM 0, channel B",
                "Memory stick in DIMM 1, channel B"
            ]
        );
        let slower = &d.details[2];
        let rated = slower
            .items
            .iter()
            .find(|i| i.label == "Rated speeds")
            .expect("rated");
        assert_eq!(rated.value, "6000, 5600 MT/s");
        let faster = &d.details[1];
        let rated = faster
            .items
            .iter()
            .find(|i| i.label == "Rated speeds")
            .expect("rated");
        assert_eq!(rated.value, "6400, 6000 MT/s");
    }

    #[test]
    fn a_balanced_matching_set_at_its_rated_speed_is_all_good() {
        let text = table(&[
            ("DIMM 0", "A", 0, "", 0),
            ("DIMM 1", "A", 16, "UD5-6000", 6000),
            ("DIMM 0", "B", 0, "", 0),
            ("DIMM 1", "B", 16, "UD5-6000", 6000),
        ]);
        let spds = vec![spd("UD5-6000", &[333]), spd("UD5-6000", &[333])];
        let d = build(&text, &spds).expect("memory");
        assert_eq!(item(&d, "Running speed").verdict, Verdict::Good);
        assert_eq!(item(&d, "Matching sticks").verdict, Verdict::Good);
        let channels = item(&d, "Sticks per memory channel");
        assert_eq!(channels.value, "A: 1, B: 1");
        assert_eq!(channels.verdict, Verdict::Good);
    }

    #[test]
    fn running_a_little_below_the_rating_is_not_flagged() {
        let text = table(&[
            ("DIMM 1", "A", 16, "UD5-6000", 5600),
            ("DIMM 1", "B", 16, "UD5-6000", 5600),
        ]);
        let spds = vec![spd("UD5-6000", &[333]), spd("UD5-6000", &[333])];
        let d = build(&text, &spds).expect("memory");
        assert_eq!(item(&d, "Running speed").verdict, Verdict::Good);
    }

    #[test]
    fn without_spd_chips_there_is_no_rated_speed_to_judge() {
        let d = build(&owners_table(), &[]).expect("memory");
        assert_eq!(item(&d, "Running speed").verdict, Verdict::Info);
        assert!(
            !d.details
                .iter()
                .flat_map(|g| &g.items)
                .any(|i| i.label == "Rated speed")
        );
        // One stick lacking a chip is enough to stop the rated speed being claimed.
        let partial = build(&owners_table(), &owners_spds()[..2]).expect("memory");
        assert!(
            !partial
                .details
                .iter()
                .flat_map(|g| &g.items)
                .any(|i| i.label == "Rated speed")
        );
    }

    #[test]
    fn a_single_stick_on_one_channel_of_two_is_unbalanced() {
        let text = table(&[("DIMM 1", "A", 16, "X", 4800), ("DIMM 1", "B", 0, "", 0)]);
        let d = build(&text, &[]).expect("memory");
        let channels = item(&d, "Sticks per memory channel");
        assert_eq!(channels.value, "A: 1, B: 0");
        assert_eq!(channels.verdict, Verdict::Reduced);
        assert!(
            !channels
                .explanation
                .as_deref()
                .is_some_and(|t| t.contains("Two sticks")),
            "no crowding advice when no channel has two"
        );
    }

    #[test]
    fn no_memory_table_means_no_memory_device() {
        assert!(build("", &[]).is_none());
        assert!(build("E:MEMORY_ARRAY_NUM_DEVICES=2\n", &[]).is_none());
        assert!(build(&table(&[("DIMM 0", "A", 0, "", 0)]), &[]).is_none());
    }
}
