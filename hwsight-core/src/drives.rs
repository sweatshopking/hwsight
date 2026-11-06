//! Disk drives: what each one is, and (from UDisks2) how healthy it is.
//!
//! The drives themselves come from `/sys/block`. Health (SMART) comes from `udisks.rs`.
//! A drive that says it's failing becomes a real problem in the app, with a "back up now"
//! step, because that is the most useful thing a device manager can tell a beginner.

use std::fs;
use std::path::Path;

use crate::descriptions::Descriptions;
use crate::details::{Detail, DetailGroup, Verdict};
use crate::device::{BuiltIn, Bus, Category, Device, Status};
use crate::udisks::{AtaHealth, Health, NvmeHealth, UdisksDrive};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Nvme,
    Sata,
    Usb,
    Other,
}

struct Disk {
    block: String,
    model: String,
    firmware: Option<String>,
    size_bytes: u64,
    rotational: bool,
    transport: Transport,
}

fn read(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Every real disk under `/sys/block`, skipping virtual ones (loop, RAM, compressed
/// memory, device-mapper, CD drives).
fn read_disks(dir: &Path) -> Vec<Disk> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut disks: Vec<Disk> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let block = entry.file_name().to_string_lossy().into_owned();
            if ["loop", "ram", "zram", "dm-", "md", "nbd", "sr", "fd"]
                .iter()
                .any(|prefix| block.starts_with(prefix))
            {
                return None;
            }
            let path = entry.path();
            let sectors: u64 = read(&path.join("size"))?.parse().ok()?;
            if sectors == 0 {
                return None;
            }
            let model = read(&path.join("device/model"))?;
            let real = fs::canonicalize(&path)
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let transport = if real.contains("/nvme/") || block.starts_with("nvme") {
                Transport::Nvme
            } else if real.contains("/usb") {
                Transport::Usb
            } else if real.contains("/ata") {
                Transport::Sata
            } else {
                Transport::Other
            };
            Some(Disk {
                firmware: read(&path.join("device/firmware_rev"))
                    .or_else(|| read(&path.join("device/rev"))),
                size_bytes: sectors * 512,
                rotational: read(&path.join("queue/rotational")).is_some_and(|r| r == "1"),
                block,
                model,
                transport,
            })
        })
        .collect();
    disks.sort_by(|a, b| a.block.cmp(&b.block));
    disks
}

/// Capacity the way drives are sold (powers of ten): "1 TB", "500 GB".
fn size_text(bytes: u64) -> String {
    let tb = bytes as f64 / 1e12;
    if tb >= 1.0 {
        let rounded = (tb * 10.0).round() / 10.0;
        if rounded.fract() == 0.0 {
            format!("{rounded:.0} TB")
        } else {
            format!("{rounded:.1} TB")
        }
    } else {
        format!("{:.0} GB", bytes as f64 / 1e9)
    }
}

fn celsius(kelvin: f64) -> i64 {
    (kelvin - 273.15).round() as i64
}

/// "265 days (6351 hours)" or "4.1 years (36028 hours)".
fn powered_on_text(hours: u64) -> String {
    let days = hours / 24;
    if days >= 730 {
        crate::tr!(
            "{} years ({} hours)",
            format!("{:.1}", hours as f64 / 8766.0),
            hours
        )
    } else {
        crate::tr!("{} days ({} hours)", days, hours)
    }
}

fn powered_on(source: &str, hours: u64) -> Detail {
    Detail::new(
        crate::tr!("Time powered on"),
        powered_on_text(hours),
        source.to_owned(),
    )
    .raw(hours.to_string())
    .verdict(
        Verdict::Info,
        Some(crate::tr!(
            "How long the drive has been switched on in its whole life."
        )),
    )
}

fn temperature(source: &str, celsius: i64, warm: i64, hot: i64) -> Detail {
    let (verdict, why) = if celsius >= hot {
        (
            Verdict::Problem,
            Some(crate::tr!(
                "The drive is dangerously hot. Check the computer's airflow and fans."
            )),
        )
    } else if celsius >= warm {
        (
            Verdict::Reduced,
            Some(crate::tr!(
                "The drive is running warm. Better airflow around it would help it last longer."
            )),
        )
    } else {
        (Verdict::Good, None)
    };
    Detail::new(
        crate::tr!("Temperature"),
        format!("{celsius} °C"),
        source.to_owned(),
    )
    .verdict(verdict, why)
}

/// The health rows for an NVMe drive, and the reasons it counts as failing, if any.
fn nvme_rows(health: &NvmeHealth, source: &str) -> (Vec<Detail>, Vec<String>) {
    let counter = |key: &str| health.counters.get(key).copied();
    let mut failing = Vec::new();
    if !health.critical_warning.is_empty() {
        failing.push(crate::tr!(
            "The drive has raised a critical warning ({}).",
            health.critical_warning.join(", ")
        ));
    }
    if let (Some(spare), Some(limit)) = (counter("avail_spare"), counter("spare_thresh"))
        && spare <= limit
    {
        failing.push(crate::tr!(
            "Its spare storage for replacing worn-out areas has run out."
        ));
    }

    let mut rows = Vec::new();
    if health.temperature_k > 0 {
        let now = celsius(f64::from(health.temperature_k));
        let warm = counter("wctemp").map_or(70, |k| celsius(k as f64));
        let hot = counter("cctemp").map_or(80, |k| celsius(k as f64));
        rows.push(temperature(source, now, warm, hot));
    }
    rows.push(powered_on(source, health.power_on_hours));
    if let Some(used) = counter("percent_used") {
        let (verdict, why) = if used >= 100 {
            (
                Verdict::Problem,
                Some(crate::tr!(
                    "The drive has used up its rated lifetime of writes. It can keep working, but it's time to plan a replacement."
                )),
            )
        } else if used >= 80 {
            (
                Verdict::Reduced,
                Some(crate::tr!(
                    "The drive is getting close to the end of its rated lifetime of writes. Plan a replacement."
                )),
            )
        } else {
            (
                Verdict::Good,
                Some(crate::tr!(
                    "How much of the drive's rated lifetime of writes has been used."
                )),
            )
        };
        rows.push(
            Detail::new(
                crate::tr!("Wear"),
                crate::tr!("{}% used", used),
                source.to_owned(),
            )
            .raw(used.to_string())
            .verdict(verdict, why),
        );
    }
    if let Some(spare) = counter("avail_spare") {
        let low = counter("spare_thresh").is_some_and(|limit| spare <= limit);
        rows.push(
            Detail::new(
                crate::tr!("Spare capacity left"),
                format!("{spare}%"),
                source.to_owned(),
            )
            .verdict(
                if low { Verdict::Problem } else { Verdict::Good },
                Some(crate::tr!(
                    "Extra storage the drive keeps to replace worn-out areas."
                )),
            ),
        );
    }
    if let Some(written) = counter("total_data_written") {
        rows.push(
            Detail::new(
                crate::tr!("Data written"),
                format!("{:.1} TB", written as f64 / 1e12),
                source.to_owned(),
            )
            .verdict(Verdict::Info, None),
        );
    }
    if let Some(errors) = counter("media_errors") {
        let (value, verdict, why) = if errors == 0 {
            (crate::tr!("None"), Verdict::Good, None)
        } else {
            (
                errors.to_string(),
                Verdict::Reduced,
                Some(crate::tr!(
                    "The drive has found data it couldn't read back correctly. A few can happen; a growing number means it is wearing out."
                )),
            )
        };
        rows.push(
            Detail::new(crate::tr!("Data errors"), value, source.to_owned()).verdict(verdict, why),
        );
    }
    for (key, label) in [
        ("total_data_read", crate::tr!("Data read")),
        ("power_cycles", crate::tr!("Times switched on")),
        (
            "unsafe_shutdowns",
            crate::tr!("Power cuts without a shutdown"),
        ),
    ] {
        if let Some(value) = counter(key) {
            let text = if key == "total_data_read" {
                format!("{:.1} TB", value as f64 / 1e12)
            } else {
                value.to_string()
            };
            rows.push(Detail::new(label, text, source.to_owned()).technical());
        }
    }
    (rows, failing)
}

/// The health rows for a SATA or IDE drive, and why it counts as failing, if it does.
fn ata_rows(health: &AtaHealth, solid_state: bool, source: &str) -> (Vec<Detail>, Vec<String>) {
    let mut failing = Vec::new();
    if health.failing {
        failing.push(crate::tr!("The drive reports that it is failing."));
    }
    if health.attributes_failing > 0 {
        failing.push(crate::tr!(
            "{} of its own health checks are failing.",
            health.attributes_failing
        ));
    }

    let mut rows = Vec::new();
    if health.temperature_k > 0.0 {
        let (warm, hot) = if solid_state { (70, 80) } else { (55, 60) };
        rows.push(temperature(
            source,
            celsius(health.temperature_k),
            warm,
            hot,
        ));
    }
    rows.push(powered_on(source, health.power_on_seconds / 3600));
    let (value, verdict, why) = if health.bad_sectors == 0 {
        (crate::tr!("None"), Verdict::Good, None)
    } else {
        (
            health.bad_sectors.to_string(),
            Verdict::Reduced,
            Some(crate::tr!(
                "Parts of the drive have stopped working. A few are survivable, but if the number keeps growing the drive is failing."
            )),
        )
    };
    rows.push(
        Detail::new(crate::tr!("Bad sectors"), value, source.to_owned()).verdict(verdict, why),
    );
    if health.attributes_failed_in_the_past > 0 {
        rows.push(
            Detail::new(
                crate::tr!("Past warnings"),
                health.attributes_failed_in_the_past.to_string(),
                source.to_owned(),
            )
            .verdict(
                Verdict::Reduced,
                Some(crate::tr!(
                    "Some of the drive's health checks have failed in the past, though they pass now."
                )),
            ),
        );
    }
    // Attribute 199 counts bad data on the cable, which is cheap to fix.
    if let Some(crc) = health.attributes.iter().find(|a| a.id == 199 && a.raw > 0) {
        rows.push(
            Detail::new(
                crate::tr!("Cable errors"),
                crc.raw.to_string(),
                source.to_owned(),
            )
            .verdict(
                Verdict::Reduced,
                Some(crate::tr!(
                    "Data was corrupted on its way along the cable. Reseating or replacing the cable usually fixes this."
                )),
            ),
        );
    }
    for attribute in &health.attributes {
        let bad = attribute.threshold > 0 && attribute.value <= attribute.threshold;
        rows.push(
            Detail::new(
                attribute.name.clone(),
                attribute.raw.to_string(),
                source.to_owned(),
            )
            .raw(format!(
                "id {}, value {}, worst {}, threshold {}",
                attribute.id, attribute.value, attribute.worst, attribute.threshold
            ))
            .verdict(if bad { Verdict::Problem } else { Verdict::Info }, None)
            .technical(),
        );
    }
    (rows, failing)
}

fn drive_type(disk: &Disk, rotation_rate: i32) -> String {
    match (disk.transport, disk.rotational) {
        (Transport::Nvme, _) => crate::tr!("Solid-state drive (NVMe)"),
        (_, true) if rotation_rate > 0 => crate::tr!("Hard drive ({} rpm)", rotation_rate),
        (_, true) => crate::tr!("Hard drive"),
        (Transport::Sata, false) => crate::tr!("Solid-state drive (SATA)"),
        (_, false) => crate::tr!("Solid-state drive"),
    }
}

fn build(
    disk: &Disk,
    udisks: Option<&UdisksDrive>,
    dir: &Path,
    descriptions: &Descriptions,
) -> Device {
    let source = crate::tr!("The UDisks2 drive health service");
    let (health_rows, failing, has_health) = match udisks.and_then(|u| u.health.as_ref()) {
        Some(Health::Nvme(h)) => {
            let (rows, failing) = nvme_rows(h, &source);
            (rows, failing, true)
        }
        Some(Health::Ata(h)) => {
            let (rows, failing) = ata_rows(h, !disk.rotational, &source);
            (rows, failing, true)
        }
        None => (Vec::new(), Vec::new(), false),
    };

    let summary = if !has_health {
        Detail::new(crate::tr!("Health"), crate::tr!("Not available"), source.clone()).verdict(
            Verdict::Info,
            Some(crate::tr!(
                "hwsight couldn't read this drive's health. That needs the UDisks service, which may not be running, or the drive may not report it (some USB drives don't)."
            )),
        )
    } else if failing.is_empty() {
        Detail::new(
            crate::tr!("Health"),
            crate::tr!("No problems reported"),
            source.clone(),
        )
        .verdict(
            Verdict::Good,
            Some(crate::tr!(
                "The drive's own self-check reports nothing wrong."
            )),
        )
    } else {
        Detail::new(crate::tr!("Health"), crate::tr!("Failing"), source.clone())
            .verdict(Verdict::Problem, Some(failing.join(" ")))
    };
    let mut health = vec![summary];
    health.extend(health_rows);

    let rotation = udisks.map_or(0, |u| u.rotation_rate);
    let here = dir.join(&disk.block).display().to_string();
    let mut about = vec![
        Detail::new(crate::tr!("Type"), drive_type(disk, rotation), here.clone()),
        Detail::new(
            crate::tr!("Capacity"),
            size_text(disk.size_bytes),
            here.clone(),
        )
        .raw(format!("{} bytes", disk.size_bytes)),
        Detail::new(crate::tr!("Model"), disk.model.clone(), here.clone()),
    ];
    if let Some(firmware) = &disk.firmware {
        about.push(Detail::new(
            crate::tr!("Firmware version"),
            firmware.clone(),
            here.clone(),
        ));
    }
    about.push(Detail::new(
        crate::tr!("Connection"),
        match disk.transport {
            Transport::Nvme => "NVMe".to_owned(),
            Transport::Sata => "SATA".to_owned(),
            Transport::Usb => "USB".to_owned(),
            Transport::Other => crate::tr!("Other"),
        },
        here,
    ));

    let mut device = Device::built_in(BuiltIn {
        address: "",
        sysfs_path: dir.join(&disk.block),
        display_name: format!("{} ({})", disk.model, size_text(disk.size_bytes)),
        vendor_name: None,
        category: Category::Drives,
        hardware_id: disk.model.clone(),
        description: descriptions
            .describe(Bus::System, &[], 0, 0, Category::Drives)
            .unwrap_or_default(),
        details: vec![
            DetailGroup {
                title: crate::tr!("Health"),
                items: health,
            },
            DetailGroup {
                title: crate::tr!("Drive"),
                items: about,
            },
        ],
    });
    device.address = disk.block.clone();
    if !failing.is_empty() {
        device.status = Status::DriveFailing;
        device.note = Some(crate::tr!(
            "This drive reports a serious health problem. Back up anything important right away."
        ));
    }
    device
}

/// A device for each disk. `health` is what UDisks2 reported (empty when it isn't running).
pub(crate) fn devices(
    block_dir: &Path,
    health: &[UdisksDrive],
    descriptions: &Descriptions,
) -> Vec<Device> {
    read_disks(block_dir)
        .iter()
        .map(|disk| {
            let udisks = health.iter().find(|u| u.block == disk.block);
            build(disk, udisks, block_dir, descriptions)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::in_english;
    use crate::udisks::AtaAttribute;

    fn write(root: &Path, name: &str, text: &str) {
        let file = root.join(name);
        fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        fs::write(file, format!("{text}\n")).expect("write");
    }

    /// Fake `/sys/block` with an NVMe drive, a hard drive, and things to skip.
    fn blocks() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write(root, "nvme0n1/size", "1953525168");
        write(root, "nvme0n1/device/model", "CT1000P310SSD8   ");
        write(root, "nvme0n1/device/firmware_rev", "VACR001");
        write(root, "nvme0n1/queue/rotational", "0");
        write(root, "sda/size", "15628053168");
        write(root, "sda/device/model", "ST8000DM004-2CX1");
        write(root, "sda/device/rev", "0001");
        write(root, "sda/queue/rotational", "1");
        write(root, "zram0/size", "100");
        write(root, "loop0/size", "100");
        write(root, "sr0/size", "100");
        write(root, "sdb/size", "0");
        temp
    }

    fn nvme(counters: &[(&str, u64)], warning: &[&str]) -> UdisksDrive {
        UdisksDrive {
            block: "nvme0n1".into(),
            rotation_rate: 0,
            health: Some(Health::Nvme(NvmeHealth {
                critical_warning: warning.iter().map(|s| (*s).to_owned()).collect(),
                power_on_hours: 6351,
                temperature_k: 313,
                counters: counters
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), *v))
                    .collect(),
            })),
        }
    }

    fn healthy_nvme() -> UdisksDrive {
        nvme(
            &[
                ("percent_used", 2),
                ("avail_spare", 100),
                ("spare_thresh", 5),
                ("total_data_written", 12_184_705_536_000),
                ("media_errors", 0),
                ("unsafe_shutdowns", 14),
                ("wctemp", 356),
                ("cctemp", 358),
            ],
            &[],
        )
    }

    fn ata(failing: bool, bad: i64, attrs_failing: i32, temp_k: f64) -> UdisksDrive {
        UdisksDrive {
            block: "sda".into(),
            rotation_rate: 5425,
            health: Some(Health::Ata(AtaHealth {
                failing,
                bad_sectors: bad,
                attributes_failing: attrs_failing,
                attributes_failed_in_the_past: 0,
                power_on_seconds: 129_700_800,
                temperature_k: temp_k,
                attributes: vec![AtaAttribute {
                    id: 5,
                    name: "reallocated-sector-count".into(),
                    value: 100,
                    worst: 100,
                    threshold: 10,
                    raw: 0,
                }],
            })),
        }
    }

    fn find(dir: &Path, health: &[UdisksDrive], address: &str) -> Device {
        in_english(|| devices(dir, health, &Descriptions::embedded()))
            .into_iter()
            .find(|d| d.address == address)
            .expect("drive")
    }

    fn item<'a>(device: &'a Device, label: &str) -> &'a Detail {
        device
            .details
            .iter()
            .flat_map(|g| &g.items)
            .find(|d| d.label == label)
            .unwrap_or_else(|| panic!("no detail {label:?}"))
    }

    #[test]
    fn lists_real_drives_and_skips_virtual_ones() {
        let temp = blocks();
        let all = in_english(|| devices(temp.path(), &[], &Descriptions::embedded()));
        let names: Vec<_> = all.iter().map(|d| d.address.as_str()).collect();
        assert_eq!(names, ["nvme0n1", "sda"]);
        assert_eq!(all[0].display_name, "CT1000P310SSD8 (1 TB)");
        assert_eq!(all[1].display_name, "ST8000DM004-2CX1 (8 TB)");
        assert_eq!(all[0].category, Category::Drives);
        assert_eq!(item(&all[0], "Firmware version").value, "VACR001");
        assert_eq!(item(&all[1], "Firmware version").value, "0001");
        assert_eq!(item(&all[0], "Type").value, "Solid-state drive (NVMe)");
        assert_eq!(item(&all[1], "Type").value, "Hard drive");
    }

    #[test]
    fn without_udisks_a_drive_says_health_is_not_available_and_is_not_a_problem() {
        let temp = blocks();
        let d = find(temp.path(), &[], "sda");
        assert_eq!(item(&d, "Health").value, "Not available");
        assert_eq!(item(&d, "Health").verdict, Verdict::Info);
        assert_eq!(d.status, Status::Working);
    }

    #[test]
    fn a_healthy_nvme_drive_shows_temperature_hours_and_wear() {
        let temp = blocks();
        let d = find(temp.path(), &[healthy_nvme()], "nvme0n1");
        assert_eq!(d.status, Status::Working);
        assert_eq!(item(&d, "Health").value, "No problems reported");
        assert_eq!(item(&d, "Health").verdict, Verdict::Good);
        let t = item(&d, "Temperature");
        assert_eq!(t.value, "40 °C", "313 K is 39.85 °C");
        assert_eq!(t.verdict, Verdict::Good);
        assert_eq!(item(&d, "Time powered on").value, "264 days (6351 hours)");
        assert_eq!(item(&d, "Wear").value, "2% used");
        assert_eq!(item(&d, "Wear").verdict, Verdict::Good);
        assert_eq!(item(&d, "Spare capacity left").value, "100%");
        assert_eq!(item(&d, "Data written").value, "12.2 TB");
        assert_eq!(item(&d, "Data errors").value, "None");
        assert!(item(&d, "Power cuts without a shutdown").technical);
        assert_eq!(item(&d, "Type").value, "Solid-state drive (NVMe)");
    }

    #[test]
    fn heat_and_wear_are_judged_but_do_not_make_a_drive_failing() {
        let temp = blocks();
        let mut hot = healthy_nvme();
        if let Some(Health::Nvme(h)) = &mut hot.health {
            h.temperature_k = 360; // above the drive's own critical temperature (358 K)
            h.counters.insert("percent_used".into(), 85);
        }
        let d = find(temp.path(), &[hot], "nvme0n1");
        assert_eq!(item(&d, "Temperature").verdict, Verdict::Problem);
        assert_eq!(item(&d, "Wear").verdict, Verdict::Reduced);
        assert_eq!(d.status, Status::Working);
        let mut worn = healthy_nvme();
        if let Some(Health::Nvme(h)) = &mut worn.health {
            h.counters.insert("percent_used".into(), 100);
        }
        let d = find(temp.path(), &[worn], "nvme0n1");
        assert_eq!(item(&d, "Wear").verdict, Verdict::Problem);
    }

    #[test]
    fn an_nvme_drive_with_a_critical_warning_or_no_spare_is_failing() {
        let temp = blocks();
        let warned = nvme(
            &[("avail_spare", 100), ("spare_thresh", 5)],
            &["spare", "reliability"],
        );
        let d = find(temp.path(), &[warned], "nvme0n1");
        assert_eq!(d.status, Status::DriveFailing);
        assert!(d.status.is_problem());
        assert_eq!(item(&d, "Health").value, "Failing");
        assert!(
            item(&d, "Health")
                .explanation
                .as_deref()
                .is_some_and(|t| t.contains("critical warning (spare, reliability)"))
        );
        assert!(d.note.as_deref().is_some_and(|t| t.contains("Back up")));
        let no_spare = nvme(&[("avail_spare", 4), ("spare_thresh", 5)], &[]);
        let d = find(temp.path(), &[no_spare], "nvme0n1");
        assert_eq!(d.status, Status::DriveFailing);
        assert_eq!(item(&d, "Spare capacity left").verdict, Verdict::Problem);
    }

    #[test]
    fn a_healthy_hard_drive_shows_its_years_and_a_cool_temperature() {
        let temp = blocks();
        let d = find(temp.path(), &[ata(false, 0, 0, 315.0)], "sda");
        assert_eq!(d.status, Status::Working);
        assert_eq!(item(&d, "Temperature").value, "42 °C");
        assert_eq!(item(&d, "Temperature").verdict, Verdict::Good);
        assert_eq!(item(&d, "Time powered on").value, "4.1 years (36028 hours)");
        assert_eq!(item(&d, "Bad sectors").value, "None");
        assert_eq!(item(&d, "Type").value, "Hard drive (5425 rpm)");
        let attribute = item(&d, "reallocated-sector-count");
        assert!(attribute.technical);
        assert_eq!(
            attribute.raw.as_deref(),
            Some("id 5, value 100, worst 100, threshold 10")
        );
    }

    #[test]
    fn a_hard_drive_that_reports_failing_is_a_problem_with_its_reason() {
        let temp = blocks();
        let d = find(temp.path(), &[ata(true, 8, 2, 335.0)], "sda");
        assert_eq!(d.status, Status::DriveFailing);
        let why = item(&d, "Health").explanation.clone().expect("why");
        assert!(why.contains("reports that it is failing"));
        assert!(why.contains("2 of its own health checks are failing"));
        assert_eq!(item(&d, "Bad sectors").value, "8");
        assert_eq!(item(&d, "Bad sectors").verdict, Verdict::Reduced);
        assert_eq!(
            item(&d, "Temperature").verdict,
            Verdict::Problem,
            "62 °C is too hot for a hard drive"
        );
    }

    #[test]
    fn a_few_bad_sectors_warn_without_calling_the_drive_failing() {
        let temp = blocks();
        let d = find(temp.path(), &[ata(false, 3, 0, 310.0)], "sda");
        assert_eq!(d.status, Status::Working);
        assert_eq!(item(&d, "Bad sectors").verdict, Verdict::Reduced);
        assert_eq!(item(&d, "Health").verdict, Verdict::Good);
    }

    #[test]
    fn cable_errors_are_called_out() {
        let temp = blocks();
        let mut drive = ata(false, 0, 0, 310.0);
        if let Some(Health::Ata(h)) = &mut drive.health {
            h.attributes.push(AtaAttribute {
                id: 199,
                name: "udma-crc-error-count".into(),
                value: 200,
                worst: 200,
                threshold: 0,
                raw: 12,
            });
        }
        let d = find(temp.path(), &[drive], "sda");
        assert_eq!(item(&d, "Cable errors").value, "12");
        assert_eq!(item(&d, "Cable errors").verdict, Verdict::Reduced);
    }

    #[test]
    fn sizes_are_shown_the_way_drives_are_sold() {
        assert_eq!(size_text(1_000_204_886_016), "1 TB");
        assert_eq!(size_text(8_001_563_222_016), "8 TB");
        assert_eq!(size_text(2_000_398_934_016), "2 TB");
        assert_eq!(size_text(1_500_301_910_016), "1.5 TB");
        assert_eq!(size_text(500_107_862_016), "500 GB");
        assert_eq!(celsius(273.15), 0);
    }
}
