//! What the graphics driver says about the card: the NVIDIA driver version and video BIOS,
//! or an AMD card's video memory and load. All plain files; the NVIDIA serial/UUID is
//! never read.

use std::fs;
use std::path::Path;

use crate::details::{Detail, DetailGroup, Verdict};
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

/// Live readings from `nvidia-smi` (it ships with the NVIDIA driver and reads the card
/// through NVML, so it needs no password). Each is `None` when the card doesn't report it.
#[derive(Debug, Default, PartialEq)]
struct Smi {
    temperature: Option<u64>,
    power_w: Option<f64>,
    power_limit_w: Option<f64>,
    graphics_mhz: Option<u64>,
    graphics_max_mhz: Option<u64>,
    memory_mhz: Option<u64>,
    load_percent: Option<u64>,
    memory_used_mib: Option<u64>,
    memory_total_mib: Option<u64>,
    fan_percent: Option<u64>,
    pstate: Option<String>,
}

const SMI_QUERY: &str = "pci.bus_id,temperature.gpu,power.draw,power.limit,clocks.gr,clocks.max.gr,clocks.mem,utilization.gpu,memory.used,memory.total,fan.speed,pstate";

/// One CSV line per card, as `--format=csv,noheader,nounits` prints them. Returns the
/// card's PCI address (like `0000:01:00.0`) with its readings.
fn parse_smi(output: &str) -> Vec<(String, Smi)> {
    output
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            if fields.len() != 12 {
                return None;
            }
            // "00000000:01:00.0" -> "0000:01:00.0"
            let id = fields[0].to_lowercase();
            let address = id.get(id.len().checked_sub(12)?..)?.to_owned();
            let int = |i: usize| fields[i].parse::<f64>().ok().map(|v| v as u64);
            let float = |i: usize| fields[i].parse::<f64>().ok();
            Some((
                address,
                Smi {
                    temperature: int(1),
                    power_w: float(2),
                    power_limit_w: float(3),
                    graphics_mhz: int(4),
                    graphics_max_mhz: int(5),
                    memory_mhz: int(6),
                    load_percent: int(7),
                    memory_used_mib: int(8),
                    memory_total_mib: int(9),
                    fan_percent: int(10),
                    pstate: Some(fields[11].to_owned()).filter(|p| p.starts_with('P')),
                },
            ))
        })
        .collect()
}

fn run_smi() -> Vec<(String, Smi)> {
    std::process::Command::new("nvidia-smi")
        .arg(format!("--query-gpu={SMI_QUERY}"))
        .arg("--format=csv,noheader,nounits")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_smi(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// The readings as "Sensors" items (heat, fan, power) and "Graphics driver" items
/// (clocks, memory, load).
fn smi_items(smi: &Smi) -> (Vec<Detail>, Vec<Detail>) {
    let source = crate::tr!("The NVIDIA driver (nvidia-smi)");
    let mut sensors = Vec::new();
    let mut driver = Vec::new();
    if let Some(t) = smi.temperature {
        let detail = Detail::new(
            crate::tr!("Temperature"),
            crate::tr!("{} °C", t),
            source.clone(),
        );
        sensors.push(if t >= 90 {
            detail.verdict(
                Verdict::Reduced,
                Some(crate::tr!(
                    "The card is very hot and may slow itself down. Check that its fans are spinning and the case has airflow."
                )),
            )
        } else {
            detail
        });
    }
    if let Some(fan) = smi.fan_percent {
        sensors.push(Detail::new(
            crate::tr!("Fan speed"),
            format!("{fan}%"),
            source.clone(),
        ));
    }
    if let Some(w) = smi.power_w {
        let value = match smi.power_limit_w {
            Some(limit) => crate::tr!("{} W of {} W", w.round(), limit.round()),
            None => crate::tr!("{} W", w.round()),
        };
        sensors.push(Detail::new(crate::tr!("Power use"), value, source.clone()));
    }
    if let Some(mhz) = smi.graphics_mhz {
        let value = match smi.graphics_max_mhz {
            Some(max) => crate::tr!("{} MHz (up to {} MHz)", mhz, max),
            None => crate::tr!("{} MHz", mhz),
        };
        driver.push(Detail::new(
            crate::tr!("Graphics speed"),
            value,
            source.clone(),
        ));
    }
    if let Some(mhz) = smi.memory_mhz {
        driver.push(
            Detail::new(
                crate::tr!("Memory speed"),
                crate::tr!("{} MHz", mhz),
                source.clone(),
            )
            .technical(),
        );
    }
    if let (Some(used), Some(total)) = (smi.memory_used_mib, smi.memory_total_mib) {
        driver.push(Detail::new(
            crate::tr!("Video memory in use"),
            crate::tr!("{} of {}", size_text(used << 20), size_text(total << 20)),
            source.clone(),
        ));
    }
    if let Some(load) = smi.load_percent {
        driver.push(Detail::new(
            crate::tr!("Graphics load"),
            format!("{load}%"),
            source.clone(),
        ));
    }
    if let Some(p) = &smi.pstate {
        driver.push(Detail::new(crate::tr!("Power state"), p.clone(), source).technical());
    }
    (sensors, driver)
}

fn size_text(bytes: u64) -> String {
    let mib = bytes >> 20;
    if mib >= 1024 {
        crate::tr!("{} GB", (mib + 512) / 1024)
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
    let smi = if nvidia_proc.is_dir() {
        run_smi()
    } else {
        Vec::new()
    };
    attach_with(devices, nvidia_proc, &smi);
}

fn attach_with(devices: &mut [Device], nvidia_proc: &Path, smi: &[(String, Smi)]) {
    for device in devices
        .iter_mut()
        .filter(|d| d.category == Category::Graphics)
    {
        let live = smi
            .iter()
            .find(|(address, _)| address.eq_ignore_ascii_case(&device.address))
            .map(|(_, readings)| smi_items(readings));
        if let Some((sensors, _)) = &live
            && !sensors.is_empty()
        {
            device.details.push(DetailGroup {
                title: crate::tr!("Sensors"),
                items: sensors.clone(),
            });
        }
        let mut items = match device.driver.as_deref() {
            Some("nvidia") => nvidia_items(device, nvidia_proc),
            Some("amdgpu") => amd_items(device),
            _ => Vec::new(),
        };
        if let Some((_, driver)) = live {
            items.extend(driver);
        }
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
    fn nvidia_smi_lines_become_readings() {
        let line = "00000000:01:00.0, 49, 13.19, 250.00, 630, 3435, 405, 10, 2217, 16303, 0, P8\n";
        let cards = parse_smi(line);
        assert_eq!(cards[0].0, "0000:01:00.0");
        let smi = &cards[0].1;
        assert_eq!(smi.temperature, Some(49));
        assert_eq!(smi.power_limit_w, Some(250.0));
        assert_eq!(smi.pstate.as_deref(), Some("P8"));
        let (sensors, driver) = crate::i18n::in_english(|| smi_items(smi));
        assert_eq!(sensors[0].value, "49 °C");
        assert_eq!(sensors[2].value, "13 W of 250 W");
        assert_eq!(driver[0].value, "630 MHz (up to 3435 MHz)");
        assert_eq!(driver[2].value, "2 GB of 16 GB");
        let na = "00000000:02:00.0, [N/A], [Not Supported], 100, 1, 2, 3, 4, 5, 6, [N/A], [N/A]";
        assert_eq!(parse_smi(na)[0].1.temperature, None);
        assert_eq!(parse_smi(na)[0].1.fan_percent, None);
        assert_eq!(parse_smi(na)[0].1.pstate, None);
        assert!(parse_smi("garbage, line").is_empty());
    }

    #[test]
    fn the_nvidia_driver_version_is_found_in_the_version_line() {
        let line = "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  615.78.08  Release Build  (notroot@)\nGCC version: x\n";
        assert_eq!(nvidia_driver(line), Some(("615.78.08".to_owned(), true)));
        let closed = "NVRM version: NVIDIA UNIX x86_64 Kernel Module  550.54.14  Fri\n";
        assert_eq!(nvidia_driver(closed), Some(("550.54.14".to_owned(), false)));
        assert_eq!(nvidia_driver("nonsense"), None);
    }
}
