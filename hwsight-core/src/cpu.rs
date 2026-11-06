//! The processor: cores and threads, speeds, caches, security status, and temperature.
//!
//! Everything comes from `/sys/devices/system/cpu`, `/proc/cpuinfo`, and the kernel's
//! temperature sensors, so no admin rights are needed. Grouping cores by their top speed
//! to spot performance and efficiency cores, and counting each shared cache once, follow
//! QLinuxDeviceManager (GPL-3.0).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::details::{Detail, DetailGroup, Verdict, format_bytes};
use crate::device::{BuiltIn, Category, Device};

fn read(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn number(path: &Path) -> Option<u64> {
    read(path)?.parse().ok()
}

/// The first processor's entry in `/proc/cpuinfo`.
#[derive(Debug, Default, PartialEq, Eq)]
struct CpuInfo {
    vendor: Option<String>,
    model_name: Option<String>,
    family: Option<String>,
    model: Option<String>,
    stepping: Option<String>,
    microcode: Option<String>,
    flags: Vec<String>,
}

fn parse_cpuinfo(text: &str) -> CpuInfo {
    let mut info = CpuInfo::default();
    for line in text.lines() {
        if line.trim().is_empty() && info.model_name.is_some() {
            break; // only the first processor
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_owned();
        match key.trim() {
            "vendor_id" => info.vendor = Some(value),
            "model name" | "Model Name" => {
                info.model_name = Some(value.split_whitespace().collect::<Vec<_>>().join(" "));
            }
            "cpu family" => info.family = Some(value),
            "model" => info.model = Some(value),
            "stepping" => info.stepping = Some(value),
            "microcode" => info.microcode = Some(value),
            "flags" | "Features" => {
                info.flags = value.split_whitespace().map(str::to_owned).collect()
            }
            _ => {}
        }
    }
    info
}

/// "32K" -> 32768, "1M" -> 1048576.
fn parse_cache_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let (digits, unit) = text.split_at(text.find(|c: char| !c.is_ascii_digit())?);
    let value: u64 = digits.parse().ok()?;
    match unit {
        "K" => Some(value * 1024),
        "M" => Some(value * 1024 * 1024),
        "G" => Some(value * 1024 * 1024 * 1024),
        _ => None,
    }
}

fn ghz(kilohertz: u64) -> String {
    format!("{:.2} GHz", kilohertz as f64 / 1_000_000.0)
}

/// One logical processor (a thread).
struct Cpu {
    index: u32,
    package: Option<u64>,
    core: Option<u64>,
    max_khz: Option<u64>,
}

fn read_cpus(dir: &Path) -> Vec<Cpu> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut cpus: Vec<Cpu> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let index: u32 = e.file_name().to_str()?.strip_prefix("cpu")?.parse().ok()?;
            let path = e.path();
            // cpu0 usually has no `online` file because it can't be turned off.
            if read(&path.join("online")).is_some_and(|v| v == "0") {
                return None;
            }
            Some(Cpu {
                index,
                package: number(&path.join("topology/physical_package_id")),
                core: number(&path.join("topology/core_id")),
                max_khz: number(&path.join("cpufreq/cpuinfo_max_freq")),
            })
        })
        .collect();
    cpus.sort_by_key(|c| c.index);
    cpus
}

/// Every different cache of each kind, counted once however many cores share it:
/// (level, kind) -> (how many there are, size of each).
fn read_caches(dir: &Path, cpus: &[Cpu]) -> BTreeMap<(u32, String), (usize, u64)> {
    let mut seen: BTreeMap<(u32, String), BTreeMap<String, u64>> = BTreeMap::new();
    for cpu in cpus {
        let cache = dir.join(format!("cpu{}/cache", cpu.index));
        let Ok(entries) = fs::read_dir(&cache) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let (Some(level), Some(kind), Some(size), Some(shared)) = (
                number(&path.join("level")),
                read(&path.join("type")),
                read(&path.join("size")).and_then(|s| parse_cache_size(&s)),
                read(&path.join("shared_cpu_list")),
            ) else {
                continue;
            };
            seen.entry((level as u32, kind))
                .or_default()
                .insert(shared, size);
        }
    }
    seen.into_iter()
        .filter_map(|(key, caches)| {
            let size = *caches.values().next()?;
            Some((key, (caches.len(), size)))
        })
        .collect()
}

fn cache_label(level: u32, kind: &str) -> String {
    match (level, kind) {
        (1, "Data") => crate::tr!("Level 1 data cache"),
        (1, "Instruction") => crate::tr!("Level 1 instruction cache"),
        (2, _) => crate::tr!("Level 2 cache"),
        (3, _) => crate::tr!("Level 3 cache"),
        _ => format!("Level {level} {} cache", kind.to_lowercase()),
    }
}

fn governor_text(governor: &str) -> String {
    match governor {
        "performance" => crate::tr!("Performance (always fast)"),
        "powersave" => crate::tr!("Power saving"),
        "schedutil" | "ondemand" | "conservative" => {
            crate::tr!("Balanced (speeds up when busy)")
        }
        other => other.to_owned(),
    }
}

/// Plain meaning of each state of the kernel's `smt/control` file.
fn smt_text(state: &str) -> String {
    match state {
        "on" => crate::tr!("On"),
        "off" | "forceoff" => crate::tr!("Off"),
        "notsupported" => crate::tr!("Not available"),
        other => other.to_owned(),
    }
}

fn speed_group(dir: &Path, cpus: &[Cpu]) -> Vec<Detail> {
    let mut items = Vec::new();
    let first = cpus.first().map_or(0, |c| c.index);
    let freq = dir.join(format!("cpu{first}/cpufreq"));
    let driver = read(&freq.join("scaling_driver"));

    if let Some(governor) = read(&freq.join("scaling_governor")) {
        // "powersave" with a plain frequency driver pins the processor at its slowest
        // speed. With amd-pstate or intel_pstate it's the normal, speed-as-needed mode.
        let pinned = governor == "powersave"
            && !driver
                .as_deref()
                .is_some_and(|d| d.contains("pstate") || d.contains("epp"));
        let (verdict, why) = if pinned {
            (
                Verdict::Reduced,
                Some(crate::tr!(
                    "The processor is set to save power and may be held at its slowest speed. Switching the power mode to balanced or performance in your system's power settings would make it faster."
                )),
            )
        } else {
            (Verdict::Info, None)
        };
        items.push(
            Detail::new(
                crate::tr!("Power mode"),
                governor_text(&governor),
                freq.join("scaling_governor").display().to_string(),
            )
            .raw(governor)
            .verdict(verdict, why),
        );
    }

    let boost = number(&dir.join("cpufreq/boost"))
        .map(|b| b != 0)
        .or_else(|| {
            // Intel's driver has the opposite switch: no_turbo = 1 means boost is off.
            number(&dir.join("intel_pstate/no_turbo")).map(|n| n == 0)
        });
    if let Some(on) = boost {
        let (value, verdict, why) = if on {
            (crate::tr!("On"), Verdict::Good, None)
        } else {
            (
                crate::tr!("Off"),
                Verdict::Reduced,
                Some(crate::tr!(
                    "The processor isn't allowed to go above its normal speed. This is usually a setting in the BIOS or in a power-saving tool."
                )),
            )
        };
        items.push(
            Detail::new(
                crate::tr!("Speed boost"),
                value,
                dir.join("cpufreq/boost").display().to_string(),
            )
            .verdict(verdict, why),
        );
    }

    if let Some(state) = read(&dir.join("smt/control")) {
        items.push(
            Detail::new(
                crate::tr!("Hyper-threading (SMT)"),
                smt_text(&state),
                dir.join("smt/control").display().to_string(),
            )
            .raw(state)
            .verdict(
                Verdict::Info,
                Some(crate::tr!(
                    "Lets each core work on two tasks at once, which helps when many programs run together."
                )),
            ),
        );
    }
    if let Some(driver) = driver {
        items.push(
            Detail::new(
                crate::tr!("Speed control driver"),
                driver,
                freq.join("scaling_driver").display().to_string(),
            )
            .technical(),
        );
    }
    if let Some(preference) = read(&freq.join("energy_performance_preference")) {
        items.push(
            Detail::new(
                crate::tr!("Energy preference"),
                preference,
                freq.join("energy_performance_preference")
                    .display()
                    .to_string(),
            )
            .technical(),
        );
    }
    if let Some(min) = number(&freq.join("cpuinfo_min_freq")) {
        items.push(
            Detail::new(
                crate::tr!("Lowest speed"),
                ghz(min),
                freq.join("cpuinfo_min_freq").display().to_string(),
            )
            .raw(format!("{min} kHz"))
            .technical(),
        );
    }
    items
}

/// What the kernel says about each known processor weakness, and a plain verdict.
fn security_group(dir: &Path, microcode: Option<&str>) -> Vec<Detail> {
    let folder = dir.join("vulnerabilities");
    let mut states: Vec<(String, String)> = fs::read_dir(&folder)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    Some((name, read(&e.path())?))
                })
                .collect()
        })
        .unwrap_or_default();
    states.sort();

    let mut items = Vec::new();
    if !states.is_empty() {
        let open: Vec<&str> = states
            .iter()
            .filter(|(_, s)| s.starts_with("Vulnerable"))
            .map(|(n, _)| n.as_str())
            .collect();
        let protected = states
            .iter()
            .filter(|(_, s)| s.starts_with("Mitigation"))
            .count();
        let (value, verdict, why) = if open.is_empty() {
            let why = (protected > 0).then(|| {
                crate::tr!(
                    "{} known security weaknesses affect this processor. The system protects against all of them. Some of those protections cost a little speed.",
                    protected
                )
            });
            (crate::tr!("None left open"), Verdict::Good, why)
        } else {
            (
                crate::tr!("{} not fully protected", open.len()),
                Verdict::Reduced,
                Some(crate::tr!(
                    "These are not fully fixed: {}. Updating the system, the BIOS, or the processor's microcode may help.",
                    open.join(", ")
                )),
            )
        };
        items.push(
            Detail::new(
                crate::tr!("Known processor weaknesses"),
                value,
                folder.display().to_string(),
            )
            .verdict(verdict, why),
        );
    }
    if let Some(version) = microcode {
        items.push(
            Detail::new(
                crate::tr!("Microcode version"),
                version.to_owned(),
                "/proc/cpuinfo".to_owned(),
            )
            .verdict(
                Verdict::Info,
                Some(crate::tr!(
                    "The processor's built-in fixes, updated by the system or the BIOS."
                )),
            )
            .technical(),
        );
    }
    for (name, state) in states {
        let verdict = if state.starts_with("Vulnerable") {
            Verdict::Reduced
        } else {
            Verdict::Info
        };
        items.push(
            Detail::new(
                name.clone(),
                state,
                folder.join(&name).display().to_string(),
            )
            .verdict(verdict, None)
            .technical(),
        );
    }
    items
}

/// The processor's temperature right now, from the kernel's sensors.
fn temperature(hwmon: &Path) -> Option<Detail> {
    let entries = fs::read_dir(hwmon).ok()?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !matches!(
            read(&path.join("name")).as_deref(),
            Some("k10temp" | "coretemp" | "zenpower" | "cpu_thermal")
        ) {
            continue;
        }
        // Prefer the whole-processor reading ("Tctl" on AMD, "Package id 0" on Intel).
        let mut chosen = path.join("temp1_input");
        for n in 1..=8 {
            let label = read(&path.join(format!("temp{n}_label")));
            if matches!(label.as_deref(), Some("Tctl" | "Package id 0")) {
                chosen = path.join(format!("temp{n}_input"));
                break;
            }
        }
        if let Some(milli) = read(&chosen).and_then(|t| t.parse::<i64>().ok()) {
            return Some(
                Detail::new(
                    crate::tr!("Temperature when scanned"),
                    format!("{:.1} °C", milli as f64 / 1000.0),
                    chosen.display().to_string(),
                )
                .verdict(
                    Verdict::Info,
                    Some(crate::tr!(
                        "Idle temperatures are usually 30 to 55 °C. Under heavy load many modern processors safely reach 90 °C or more."
                    )),
                ),
            );
        }
    }
    None
}

/// The notable instruction sets this processor supports.
fn features(flags: &[String]) -> Option<String> {
    const NOTABLE: [(&str, &str); 10] = [
        ("sse4_2", "SSE4.2"),
        ("avx", "AVX"),
        ("avx2", "AVX2"),
        ("fma", "FMA"),
        ("avx512f", "AVX-512"),
        ("aes", "AES"),
        ("sha_ni", "SHA"),
        ("vaes", "VAES"),
        ("amx_tile", "AMX"),
        ("rdrand", "RDRAND"),
    ];
    let found: Vec<&str> = NOTABLE
        .iter()
        .filter(|(flag, _)| flags.iter().any(|f| f == flag))
        .map(|(_, name)| *name)
        .collect();
    (!found.is_empty()).then(|| found.join(", "))
}

/// The processor as one device, or `None` when the system shows nothing about it.
pub(crate) fn device(cpu_dir: &Path, cpuinfo_path: &Path, hwmon_dir: &Path) -> Option<Device> {
    let info = fs::read_to_string(cpuinfo_path)
        .map(|t| parse_cpuinfo(&t))
        .unwrap_or_default();
    let cpus = read_cpus(cpu_dir);
    if cpus.is_empty() && info.model_name.is_none() {
        return None;
    }

    // Cores are the distinct (socket, core) pairs; each can run several threads.
    let cores: BTreeSet<(Option<u64>, Option<u64>)> =
        cpus.iter().map(|c| (c.package, c.core)).collect();
    let sockets: BTreeSet<Option<u64>> = cpus.iter().map(|c| c.package).collect();
    let mut summary = Vec::new();
    if !cpus.is_empty() {
        let c = crate::tr!("1 core" | "{n} cores" % cores.len());
        let t = crate::tr!("1 thread" | "{n} threads" % cpus.len());
        summary.push(
            Detail::new(
                crate::tr!("Cores and threads"),
                crate::tr!("{}, {}", c, t),
                cpu_dir.join("cpu*/topology").display().to_string(),
            )
            .verdict(
                Verdict::Info,
                Some(crate::tr!(
                    "More cores let the computer do more things at once. Threads are how many tasks it can work on at the same moment."
                )),
            ),
        );
    }
    if sockets.len() > 1 {
        summary.push(
            Detail::new(
                crate::tr!("Processor sockets"),
                sockets.len().to_string(),
                cpu_dir
                    .join("cpu*/topology/physical_package_id")
                    .display()
                    .to_string(),
            )
            .technical(),
        );
    }

    // Performance and efficiency cores differ clearly in their top speed.
    let mut core_speed: BTreeMap<(Option<u64>, Option<u64>), u64> = BTreeMap::new();
    for cpu in &cpus {
        if let Some(khz) = cpu.max_khz {
            let entry = core_speed.entry((cpu.package, cpu.core)).or_default();
            *entry = (*entry).max(khz);
        }
    }
    if let Some(&fastest) = core_speed.values().max() {
        let fast = core_speed
            .values()
            .filter(|s| **s * 10 >= fastest * 8)
            .count();
        let slow = core_speed.len() - fast;
        if slow > 0 {
            summary.push(
                Detail::new(
                    crate::tr!("Core types"),
                    crate::tr!("{} performance, {} efficiency", fast, slow),
                    cpu_dir.join("cpu*/cpufreq/cpuinfo_max_freq").display().to_string(),
                )
                .verdict(
                    Verdict::Info,
                    Some(crate::tr!(
                        "Performance cores are faster. Efficiency cores use less power for lighter work."
                    )),
                ),
            );
        }
        summary.push(
            Detail::new(
                crate::tr!("Highest speed"),
                ghz(fastest),
                cpu_dir.join("cpu*/cpufreq/cpuinfo_max_freq").display().to_string(),
            )
            .raw(format!("{fastest} kHz"))
            .verdict(
                Verdict::Info,
                Some(crate::tr!(
                    "The fastest this processor runs when it's busy. It runs slower when idle to save power."
                )),
            ),
        );
    }
    let virtualization = if info.flags.iter().any(|f| f == "svm") {
        Some("AMD-V")
    } else if info.flags.iter().any(|f| f == "vmx") {
        Some("Intel VT-x")
    } else {
        None
    };
    if !info.flags.is_empty() {
        let item = Detail::new(
            crate::tr!("Virtualization"),
            match virtualization {
                Some(name) => crate::tr!("Available ({})", name),
                None => crate::tr!("Not available"),
            },
            "/proc/cpuinfo".to_owned(),
        );
        summary.push(match virtualization {
            Some(_) => item.verdict(
                Verdict::Good,
                Some(crate::tr!(
                    "Lets virtual machines run at close to full speed."
                )),
            ),
            None => item.verdict(
                Verdict::Info,
                Some(crate::tr!(
                    "Either this processor can't do it, or it's turned off in the BIOS (look for SVM or VT-x). It's needed to run virtual machines quickly."
                )),
            ),
        });
    }
    if let Some(item) = temperature(hwmon_dir) {
        summary.push(item);
    }

    let mut groups = vec![DetailGroup {
        title: crate::tr!("Processor"),
        items: summary,
    }];
    groups.push(DetailGroup {
        title: crate::tr!("Speed and power"),
        items: speed_group(cpu_dir, &cpus),
    });
    groups.push(DetailGroup {
        title: crate::tr!("Security"),
        items: security_group(cpu_dir, info.microcode.as_deref()),
    });
    let caches = read_caches(cpu_dir, &cpus);
    groups.push(DetailGroup {
        title: crate::tr!("Caches"),
        items: caches
            .iter()
            .map(|((level, kind), (count, size))| {
                let value = if *count > 1 {
                    format!("{count} × {}", format_bytes(*size))
                } else {
                    format_bytes(*size)
                };
                Detail::new(
                    cache_label(*level, kind),
                    value,
                    cpu_dir.join("cpu*/cache").display().to_string(),
                )
                .technical()
            })
            .collect(),
    });
    let mut identity = Vec::new();
    if let Some(list) = features(&info.flags) {
        identity.push(
            Detail::new(
                crate::tr!("Instruction sets"),
                list,
                "/proc/cpuinfo".to_owned(),
            )
            .technical(),
        );
    }
    if let (Some(family), Some(model), Some(stepping)) = (&info.family, &info.model, &info.stepping)
    {
        identity.push(
            Detail::new(
                crate::tr!("Family, model, stepping"),
                format!("{family}, {model}, {stepping}"),
                "/proc/cpuinfo".to_owned(),
            )
            .technical(),
        );
    }
    groups.push(DetailGroup {
        title: crate::tr!("Identity"),
        items: identity,
    });
    groups.retain(|g| !g.items.is_empty());

    let vendor = info.vendor.as_deref().map(|v| match v {
        "AuthenticAMD" => "AMD".to_owned(),
        "GenuineIntel" => "Intel".to_owned(),
        other => other.to_owned(),
    });
    let name = info
        .model_name
        .clone()
        .unwrap_or_else(|| crate::tr!("Processor"));
    Some(Device::built_in(BuiltIn {
        address: "processor",
        sysfs_path: cpu_dir.to_path_buf(),
        display_name: name,
        vendor_name: vendor,
        category: Category::Processors,
        hardware_id: match (&info.family, &info.model, &info.stepping) {
            (Some(f), Some(m), Some(s)) => format!("family {f} model {m} stepping {s}"),
            _ => String::new(),
        },
        description: crate::tr!(
            "The processor (CPU), the computer's main brain. It does the thinking for every program you run. More cores let it do more at once; a higher speed makes each task faster."
        ),
        details: groups,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::in_english;

    fn write(root: &Path, name: &str, text: &str) {
        let file = root.join(name);
        fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        fs::write(file, format!("{text}\n")).expect("write");
    }

    /// A fake processor: `cores` cores of 2 threads each, every core at `max_khz`.
    fn fake_cpu(root: &Path, cores: u32, max_khz: u64) {
        for thread in 0..cores * 2 {
            let core = thread % cores;
            let cpu = format!("cpu{thread}");
            write(root, &format!("{cpu}/topology/physical_package_id"), "0");
            write(root, &format!("{cpu}/topology/core_id"), &core.to_string());
            write(
                root,
                &format!("{cpu}/cpufreq/cpuinfo_max_freq"),
                &max_khz.to_string(),
            );
            if thread > 0 {
                write(root, &format!("{cpu}/online"), "1");
            }
            let shared = format!("{core},{}", core + cores);
            write(root, &format!("{cpu}/cache/index0/level"), "1");
            write(root, &format!("{cpu}/cache/index0/type"), "Data");
            write(root, &format!("{cpu}/cache/index0/size"), "32K");
            write(
                root,
                &format!("{cpu}/cache/index0/shared_cpu_list"),
                &shared,
            );
            write(root, &format!("{cpu}/cache/index1/level"), "3");
            write(root, &format!("{cpu}/cache/index1/type"), "Unified");
            write(root, &format!("{cpu}/cache/index1/size"), "32768K");
            write(
                root,
                &format!("{cpu}/cache/index1/shared_cpu_list"),
                &format!("0-{}", cores * 2 - 1),
            );
        }
        write(root, "cpu0/cpufreq/cpuinfo_min_freq", "400000");
        write(root, "cpu0/cpufreq/scaling_driver", "amd-pstate-epp");
        write(root, "cpu0/cpufreq/scaling_governor", "powersave");
        write(
            root,
            "cpu0/cpufreq/energy_performance_preference",
            "performance",
        );
        write(root, "cpufreq/boost", "1");
        write(root, "smt/control", "on");
        write(root, "vulnerabilities/meltdown", "Not affected");
        write(root, "vulnerabilities/spectre_v2", "Mitigation: IBRS");
        write(root, "vulnerabilities/spectre_v1", "Mitigation: barriers");
    }

    const CPUINFO: &str = "processor\t: 0\nvendor_id\t: AuthenticAMD\ncpu family\t: 25\nmodel\t\t: 97\nmodel name\t: AMD  Ryzen 9 7900X 12-Core Processor\nstepping\t: 2\nmicrocode\t: 0xa60120c\nflags\t\t: fpu sse4_2 avx avx2 avx512f aes svm\n\nprocessor\t: 1\nmodel name\t: should not be read\n";

    fn build(cpu: &Path, extra: &[(&str, &str)]) -> Option<Device> {
        write(cpu.parent().expect("parent"), "cpuinfo", CPUINFO.trim_end());
        for (name, text) in extra {
            write(cpu.parent().expect("parent"), name, text);
        }
        in_english(|| {
            device(
                cpu,
                &cpu.parent().expect("parent").join("cpuinfo"),
                &cpu.parent().expect("parent").join("hwmon"),
            )
        })
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
    fn reads_the_first_processor_and_cleans_up_the_name() {
        let info = parse_cpuinfo(CPUINFO);
        assert_eq!(
            info.model_name.as_deref(),
            Some("AMD Ryzen 9 7900X 12-Core Processor")
        );
        assert_eq!(info.vendor.as_deref(), Some("AuthenticAMD"));
        assert_eq!(info.microcode.as_deref(), Some("0xa60120c"));
        assert!(info.flags.contains(&"svm".to_owned()));
        assert_eq!(parse_cache_size("32K"), Some(32768));
        assert_eq!(parse_cache_size("1M"), Some(1 << 20));
        assert_eq!(parse_cache_size("lots"), None);
    }

    #[test]
    fn counts_cores_threads_and_caches_once() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cpu = temp.path().join("cpu");
        fake_cpu(&cpu, 6, 5_737_986);
        let d = build(&cpu, &[]).expect("cpu");
        assert_eq!(d.display_name, "AMD Ryzen 9 7900X 12-Core Processor");
        assert_eq!(d.vendor_name.as_deref(), Some("AMD"));
        assert_eq!(d.category, Category::Processors);
        assert_eq!(item(&d, "Cores and threads").value, "6 cores, 12 threads");
        assert_eq!(item(&d, "Highest speed").value, "5.74 GHz");
        assert_eq!(item(&d, "Level 1 data cache").value, "6 × 32 KiB");
        assert_eq!(
            item(&d, "Level 3 cache").value,
            "32 MiB",
            "one L3 shared by every thread"
        );
        assert!(
            !d.details
                .iter()
                .flat_map(|g| &g.items)
                .any(|i| i.label == "Core types"),
            "identical cores are not split into types"
        );
        assert_eq!(item(&d, "Virtualization").value, "Available (AMD-V)");
        assert_eq!(item(&d, "Virtualization").verdict, Verdict::Good);
        assert_eq!(
            item(&d, "Instruction sets").value,
            "SSE4.2, AVX, AVX2, AVX-512, AES"
        );
        assert_eq!(item(&d, "Family, model, stepping").value, "25, 97, 2");
    }

    #[test]
    fn speed_settings_are_judged_by_driver() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cpu = temp.path().join("cpu");
        fake_cpu(&cpu, 2, 4_000_000);
        // powersave under amd-pstate is the normal mode.
        let d = build(&cpu, &[]).expect("cpu");
        assert_eq!(item(&d, "Power mode").value, "Power saving");
        assert_eq!(item(&d, "Power mode").verdict, Verdict::Info);
        assert_eq!(item(&d, "Speed boost").verdict, Verdict::Good);
        assert_eq!(item(&d, "Hyper-threading (SMT)").value, "On");
        // powersave under the plain driver pins the processor at its slowest speed.
        write(&cpu, "cpu0/cpufreq/scaling_driver", "acpi-cpufreq");
        write(&cpu, "cpufreq/boost", "0");
        let d = build(&cpu, &[]).expect("cpu");
        assert_eq!(item(&d, "Power mode").verdict, Verdict::Reduced);
        assert_eq!(item(&d, "Speed boost").value, "Off");
        assert_eq!(item(&d, "Speed boost").verdict, Verdict::Reduced);
    }

    #[test]
    fn intel_turbo_switch_is_read_the_other_way_round() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cpu = temp.path().join("cpu");
        fake_cpu(&cpu, 2, 4_000_000);
        fs::remove_file(cpu.join("cpufreq/boost")).expect("remove");
        write(&cpu, "intel_pstate/no_turbo", "1");
        let d = build(&cpu, &[]).expect("cpu");
        assert_eq!(item(&d, "Speed boost").value, "Off");
        write(&cpu, "intel_pstate/no_turbo", "0");
        let d = build(&cpu, &[]).expect("cpu");
        assert_eq!(item(&d, "Speed boost").value, "On");
    }

    #[test]
    fn hybrid_processors_report_performance_and_efficiency_cores() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cpu = temp.path().join("cpu");
        fake_cpu(&cpu, 4, 5_400_000);
        // Cores 2 and 3 are the slower efficiency cores.
        for thread in [2u32, 3, 6, 7] {
            write(
                &cpu,
                &format!("cpu{thread}/cpufreq/cpuinfo_max_freq"),
                "3800000",
            );
        }
        let d = build(&cpu, &[]).expect("cpu");
        assert_eq!(item(&d, "Core types").value, "2 performance, 2 efficiency");
        assert_eq!(item(&d, "Highest speed").value, "5.40 GHz");
    }

    #[test]
    fn security_status_is_summarised_and_open_weaknesses_are_named() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cpu = temp.path().join("cpu");
        fake_cpu(&cpu, 2, 4_000_000);
        let d = build(&cpu, &[]).expect("cpu");
        let weak = item(&d, "Known processor weaknesses");
        assert_eq!(weak.value, "None left open");
        assert_eq!(weak.verdict, Verdict::Good);
        assert!(
            weak.explanation
                .as_deref()
                .is_some_and(|t| t.starts_with("2 known security"))
        );
        assert_eq!(item(&d, "spectre_v2").value, "Mitigation: IBRS");
        assert!(item(&d, "spectre_v2").technical);
        assert_eq!(item(&d, "Microcode version").value, "0xa60120c");

        write(&cpu, "vulnerabilities/retbleed", "Vulnerable: no microcode");
        let d = build(&cpu, &[]).expect("cpu");
        let weak = item(&d, "Known processor weaknesses");
        assert_eq!(weak.value, "1 not fully protected");
        assert_eq!(weak.verdict, Verdict::Reduced);
        assert!(
            weak.explanation
                .as_deref()
                .is_some_and(|t| t.contains("retbleed"))
        );
        assert_eq!(item(&d, "retbleed").verdict, Verdict::Reduced);
    }

    #[test]
    fn temperature_prefers_the_whole_processor_reading() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cpu = temp.path().join("cpu");
        fake_cpu(&cpu, 2, 4_000_000);
        let d = build(
            &cpu,
            &[
                ("hwmon/hwmon0/name", "nvme"),
                ("hwmon/hwmon0/temp1_input", "99000"),
                ("hwmon/hwmon3/name", "k10temp"),
                ("hwmon/hwmon3/temp1_label", "Tctl"),
                ("hwmon/hwmon3/temp1_input", "47500"),
                ("hwmon/hwmon3/temp3_label", "Tccd1"),
                ("hwmon/hwmon3/temp3_input", "52000"),
            ],
        )
        .expect("cpu");
        assert_eq!(item(&d, "Temperature when scanned").value, "47.5 °C");
    }

    #[test]
    fn offline_threads_are_not_counted_and_nothing_readable_gives_no_device() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cpu = temp.path().join("cpu");
        fake_cpu(&cpu, 2, 4_000_000);
        write(&cpu, "cpu3/online", "0");
        let d = build(&cpu, &[]).expect("cpu");
        assert_eq!(item(&d, "Cores and threads").value, "2 cores, 3 threads");
        let none = tempfile::tempdir().expect("tempdir");
        assert!(
            in_english(|| device(
                &none.path().join("cpu"),
                &none.path().join("cpuinfo"),
                &none.path().join("hwmon"),
            ))
            .is_none()
        );
    }
}
