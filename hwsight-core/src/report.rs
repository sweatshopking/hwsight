use std::fs;

use crate::{Bus, Device, ScanReport};

pub struct Privacy {
    pub home: Option<String>,
    pub user: Option<String>,
    pub host: Option<String>,
}

impl Privacy {
    pub fn system() -> Self {
        let home = std::env::var("HOME").ok().filter(|v| !v.is_empty());
        let user = std::env::var("USER").ok().filter(|v| !v.is_empty());
        let host = fs::read_to_string("/etc/hostname")
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty());
        Self { home, user, host }
    }
}

pub fn scrub(text: &str, privacy: &Privacy) -> String {
    let text = replace_value(text, privacy.home.as_deref(), "~");
    let text = replace_word(&text, privacy.host.as_deref(), "<computer>");
    replace_word(&text, privacy.user.as_deref(), "<user>")
}

fn replace_value(text: &str, value: Option<&str>, replacement: &str) -> String {
    match value.filter(|v| v.len() >= 3) {
        Some(value) => text.replace(value, replacement),
        None => text.to_owned(),
    }
}

/// Replaces `word` only where it stands alone (no letter, digit, `_`, or `-` next to it).
fn replace_word(text: &str, word: Option<&str>, replacement: &str) -> String {
    let Some(user) = word.filter(|v| v.len() >= 3) else {
        return text.to_owned();
    };
    let mut result = String::with_capacity(text.len());
    let mut position = 0;
    while let Some(relative) = text[position..].find(user) {
        let start = position + relative;
        let end = start + user.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        let word_character =
            |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !word_character(before) && !word_character(after) {
            result.push_str(&text[position..start]);
            result.push_str(replacement);
            position = end;
        } else {
            result.push_str(&text[position..end]);
            position = end;
        }
    }
    result.push_str(&text[position..]);
    result
}

/// One device's report section. Always English (see `i18n::in_english`).
pub fn device_markdown(device: &Device) -> String {
    crate::i18n::in_english(|| device_section(device))
}

fn device_section(device: &Device) -> String {
    let mut lines = vec![
        format!("### {}", device.display_name),
        format!("- Status: {}", device.status.summary()),
        format!("- Type: {}", device.category.label()),
    ];
    if let Some(vendor) = &device.vendor_name
        && !vendor.is_empty()
    {
        lines.push(format!("- Maker: {vendor}"));
    }
    let bus = match device.bus {
        Bus::Pci => "PCI",
        Bus::Usb => "USB",
        Bus::System => "built in",
    };
    lines.push(format!(
        "- Hardware ID: `{}` ({bus}, address `{}`)",
        device.hardware_id, device.address
    ));
    lines.push(match &device.driver {
        Some(driver) if !driver.is_empty() => format!("- Driver in use: `{driver}`"),
        _ => "- Driver in use: none".to_owned(),
    });
    if !device.driver_candidates.is_empty() {
        let candidates = device
            .driver_candidates
            .iter()
            .map(|candidate| {
                let state = candidate
                    .blocked_by
                    .as_deref()
                    .map(|_| " (turned off by a setting)")
                    .or_else(|| candidate.loaded.then_some(" (loaded)"))
                    .unwrap_or_default();
                format!("`{}`{state}", candidate.module)
            })
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("- Drivers that support it: {candidates}"));
    }
    if let Some(interface) = &device.missing_interface {
        lines.push(format!("- What's wrong: {interface}"));
    }
    if let Some(note) = &device.note {
        lines.push(format!("- Note: {note}"));
    }
    for finding in device.findings() {
        let kind = match finding.verdict {
            crate::details::Verdict::Problem => "Needs attention",
            _ => "Could be better",
        };
        lines.push(format!("- {kind}: {}: {}", finding.label, finding.value));
    }
    if device.status.is_problem() && !device.log_issues.is_empty() {
        lines.push(String::new());
        lines.push("Kernel log:".to_owned());
        lines.extend(
            device
                .log_issues
                .iter()
                .map(|issue| format!("- {}", issue.message())),
        );
    }
    lines.join("\n")
}

pub fn markdown(report: &ScanReport, only: Option<&str>, privacy: &Privacy) -> String {
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty());
    markdown_with_kernel(report, only, privacy, kernel.as_deref())
}

/// Like `markdown`, with the kernel release given. Always English: the people who
/// answer forum posts and bug reports read English.
pub(crate) fn markdown_with_kernel(
    report: &ScanReport,
    only: Option<&str>,
    privacy: &Privacy,
    kernel: Option<&str>,
) -> String {
    crate::i18n::in_english(|| build_markdown(report, only, privacy, kernel))
}

fn build_markdown(
    report: &ScanReport,
    only: Option<&str>,
    privacy: &Privacy,
    kernel: Option<&str>,
) -> String {
    let mut lines = vec![
        "## hwsight report".to_owned(),
        String::new(),
        format!(
            "- Computer: {}{}",
            report.distro.name,
            if report.distro.immutable {
                " (read-only system)"
            } else {
                ""
            }
        ),
    ];
    if let Some(kernel) = kernel.filter(|v| !v.is_empty()) {
        lines.push(format!("- Kernel: {kernel}"));
    }
    lines.push(format!("- hwsight: {}", env!("CARGO_PKG_VERSION")));
    lines.push(format!("- Data: rules {}", report.data.rules_version));
    lines.push(String::new());
    // The header holds no private details, and it must survive scrubbing: many people
    // keep the distro's name as their computer's name.
    let header = lines.join("\n");
    let mut lines = Vec::new();

    match only {
        Some(address) => {
            if let Some(device) = report.devices.iter().find(|d| d.address == address) {
                lines.push(device_section(device));
            } else {
                lines.push(format!("No device with address {address} was found."));
            }
        }
        None => {
            let problems = report
                .devices
                .iter()
                .filter(|device| device.status.is_problem())
                .collect::<Vec<_>>();
            // Working devices with an amber or red finding (RAM below its rating, a hot
            // drive) are worth a forum helper's attention too.
            let better = report
                .devices
                .iter()
                .filter(|device| !device.status.is_problem() && !device.findings().is_empty())
                .collect::<Vec<_>>();
            let mut summary = format!(
                "{} devices, {} with problems.",
                report.devices.len(),
                problems.len()
            );
            if !better.is_empty() {
                summary.push_str(&format!(" {} could be better.", better.len()));
            }
            lines.push(summary);
            for device in problems {
                lines.push(String::new());
                lines.push(device_section(device));
            }
            for device in better {
                lines.push(String::new());
                lines.push(device_section(device));
            }
            lines.push(String::new());
            lines.push("#### Working devices".to_owned());
            // Identical lines (chipsets repeat the same bridge many times) are merged
            // into one with a count, in the order they first appear.
            let mut working: Vec<(String, usize)> = Vec::new();
            for device in report
                .devices
                .iter()
                .filter(|device| !device.status.is_problem() && device.findings().is_empty())
            {
                let line = format!(
                    "- {} ({}, `{}`)",
                    device.display_name,
                    device.category.label(),
                    device.hardware_id
                );
                match working.iter_mut().find(|(seen, _)| *seen == line) {
                    Some((_, count)) => *count += 1,
                    None => working.push((line, 1)),
                }
            }
            lines.extend(working.into_iter().map(|(line, count)| match count {
                1 => line,
                n => format!("{line} x{n}"),
            }));
            if !report.port_issues.is_empty() {
                lines.push(String::new());
                lines.push("#### USB port problems".to_owned());
                lines.extend(
                    report
                        .port_issues
                        .iter()
                        .map(|issue| format!("- {}: {}", issue.port, issue.issue.message())),
                );
            }
            if !report.notes.is_empty() {
                lines.push(String::new());
                lines.push("#### Scan notes".to_owned());
                lines.extend(
                    report
                        .notes
                        .iter()
                        .map(|note| format!("- {}", note.message())),
                );
            }
        }
    }
    let text = format!("{header}\n{}", scrub(&lines.join("\n"), privacy));
    format!("{}\n", text.trim_end_matches('\n'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Category, DataInfo, Distro, Family, LogIssue, Status, modules::DriverCandidate};
    use std::path::PathBuf;

    fn device(status: Status, address: &str) -> Device {
        Device {
            bus: Bus::Pci,
            address: address.to_owned(),
            sysfs_path: PathBuf::from("/sys/devices"),
            display_name: "Test device".to_owned(),
            vendor_name: Some("Test maker".to_owned()),
            product_name: None,
            model_name: None,
            class_name: None,
            category: Category::Network,
            hardware_id: "1234:5678".to_owned(),
            unrecognized: false,
            modalias: None,
            driver: Some("test_driver".to_owned()),
            driver_candidates: vec![],
            status,
            log_issues: vec![],
            missing_interface: None,
            note: None,
            description: None,
            details: vec![],
            software: vec![],
            class_codes: vec![],
            fixes: vec![],
        }
    }

    fn report(devices: Vec<Device>) -> ScanReport {
        ScanReport {
            devices,
            port_issues: vec![],
            notes: vec![],
            data: DataInfo {
                rules_version: "2026.11.03".to_owned(),
                firmware_version: "2026.11.03".to_owned(),
                descriptions_version: "2026.11.03".to_owned(),
                fixes_version: "2026.11.03".to_owned(),
                rules_path: None,
                pci_ids: vec![],
                usb_ids: vec![],
            },
            kernel_log: None,
            distro: Distro {
                id: "test".to_owned(),
                name: "Test Linux".to_owned(),
                family: Family::Unknown,
                immutable: false,
                base_release: None,
            },
            firmware_note: None,
        }
    }

    #[test]
    fn scrub_replaces_home_host_and_user() {
        let privacy = Privacy {
            home: Some("/home/josh".to_owned()),
            host: Some("my-computer".to_owned()),
            user: Some("josh".to_owned()),
        };
        assert_eq!(
            scrub("/home/josh on my-computer for josh", &privacy),
            "~ on <computer> for <user>"
        );
    }

    #[test]
    fn scrub_skips_short_values() {
        let privacy = Privacy {
            home: Some("/x".to_owned()),
            host: Some("pc".to_owned()),
            user: Some("jo".to_owned()),
        };
        assert_eq!(scrub("joystick /x pc", &privacy), "joystick /x pc");
    }

    #[test]
    fn scrub_user_only_as_whole_word() {
        let privacy = Privacy {
            home: None,
            host: None,
            user: Some("josh".to_owned()),
        };
        assert_eq!(
            scrub("joshua /josh/ josh-test", &privacy),
            "joshua /<user>/ josh-test"
        );
    }

    #[test]
    fn device_section_for_a_broken_device() {
        let mut broken = device(Status::DriverNotReady, "0000:01:00.0");
        broken.driver = None;
        broken.driver_candidates = vec![
            DriverCandidate {
                module: "good_driver".to_owned(),
                builtin: false,
                loaded: true,
                blocked_by: None,
            },
            DriverCandidate {
                module: "blocked_driver".to_owned(),
                builtin: false,
                loaded: false,
                blocked_by: Some("/etc/modprobe.d/blocked.conf".to_owned()),
            },
        ];
        broken.missing_interface = Some("network interface".to_owned());
        broken.note = Some("The device needs attention.".to_owned());
        broken.log_issues = vec![LogIssue::DriverFailed {
            driver: "good_driver".to_owned(),
            error: 2,
        }];
        assert_eq!(
            device_markdown(&broken),
            "### Test device\n\
             - Status: Driver loaded, but the device isn't ready\n\
             - Type: Network\n\
             - Maker: Test maker\n\
             - Hardware ID: `1234:5678` (PCI, address `0000:01:00.0`)\n\
             - Driver in use: none\n\
             - Drivers that support it: `good_driver` (loaded), `blocked_driver` (turned off by a setting)\n\
             - What's wrong: network interface\n\
             - Note: The device needs attention.\n\
             \n\
             Kernel log:\n\
             - The good_driver driver couldn't start this device (a file it needs is missing)."
        );
    }

    #[test]
    fn device_section_leaves_out_empty_lines() {
        let mut working = device(Status::Working, "0000:01:00.0");
        working.vendor_name = None;
        working.driver = None;
        assert_eq!(
            device_markdown(&working),
            "### Test device\n\
             - Status: Working\n\
             - Type: Network\n\
             - Hardware ID: `1234:5678` (PCI, address `0000:01:00.0`)\n\
             - Driver in use: none"
        );
    }

    #[test]
    fn whole_report_lists_problems_then_working() {
        let mut problem = device(Status::NoDriver, "problem");
        problem.display_name = "Broken".to_owned();
        let mut working = device(Status::Working, "working");
        working.display_name = "Fine".to_owned();
        let text = markdown_with_kernel(
            &report(vec![working, problem]),
            None,
            &Privacy {
                home: None,
                user: None,
                host: None,
            },
            Some("6.1"),
        );
        assert!(text.find("### Broken").unwrap() < text.find("#### Working devices").unwrap());
        assert!(
            text.find("- Fine (Network, `1234:5678`)").unwrap()
                > text.find("#### Working devices").unwrap()
        );
    }

    #[test]
    fn identical_working_devices_are_merged() {
        let mut other = device(Status::Working, "c");
        other.display_name = "Other".to_owned();
        let text = markdown_with_kernel(
            &report(vec![
                device(Status::Working, "a"),
                other,
                device(Status::Working, "b"),
            ]),
            None,
            &Privacy {
                home: None,
                user: None,
                host: None,
            },
            None,
        );
        assert!(text.contains(
            "#### Working devices\n- Test device (Network, `1234:5678`) x2\n- Other (Network, `1234:5678`)\n"
        ));
    }

    #[test]
    fn report_for_one_address() {
        let report = report(vec![device(Status::Working, "wanted")]);
        let text = markdown_with_kernel(
            &report,
            Some("wanted"),
            &Privacy {
                home: None,
                user: None,
                host: None,
            },
            None,
        );
        assert!(text.contains("### Test device"));
        assert!(!text.contains("Working devices"));
    }

    #[test]
    fn report_for_unknown_address() {
        let text = markdown_with_kernel(
            &report(vec![]),
            Some("missing"),
            &Privacy {
                home: None,
                user: None,
                host: None,
            },
            None,
        );
        assert!(text.contains("No device with address missing was found."));
    }

    #[test]
    fn computer_name_same_as_distro_keeps_the_distro() {
        let mut device = device(Status::Working, "0000:01:00.0");
        device.note = Some("Seen on CachyOS and CachyOS-box".to_owned());
        let mut report = report(vec![device]);
        report.distro.name = "CachyOS".to_owned();
        let text = markdown_with_kernel(
            &report,
            Some("0000:01:00.0"),
            &Privacy {
                home: None,
                user: None,
                host: Some("CachyOS".to_owned()),
            },
            None,
        );
        assert!(text.contains("- Computer: CachyOS\n"));
        assert!(text.contains("- Note: Seen on <computer> and CachyOS-box"));
    }

    #[test]
    fn report_ends_with_one_newline() {
        let text = markdown_with_kernel(
            &report(vec![]),
            None,
            &Privacy {
                home: None,
                user: None,
                host: None,
            },
            None,
        );
        assert!(text.ends_with('\n'));
        assert!(!text.ends_with("\n\n"));
    }

    fn no_privacy() -> Privacy {
        Privacy {
            home: None,
            user: None,
            host: None,
        }
    }

    fn with_finding(address: &str, verdict: crate::details::Verdict) -> Device {
        use crate::details::{Detail, DetailGroup};
        let mut d = device(Status::Working, address);
        d.details = vec![DetailGroup {
            title: "Memory".into(),
            items: vec![
                Detail::new("Fine thing".into(), "ok".into(), "x".into())
                    .verdict(crate::details::Verdict::Good, None),
                Detail::new("Running speed".into(), "4800 MT/s".into(), "x".into())
                    .verdict(verdict, Some("Slower than rated.".into())),
            ],
        }];
        d
    }

    #[test]
    fn attention_comes_from_the_worst_verdict() {
        use crate::device::Attention;
        assert_eq!(device(Status::Working, "a").attention(), Attention::Fine);
        let amber = with_finding("b", crate::details::Verdict::Reduced);
        assert_eq!(amber.attention(), Attention::CouldBeBetter);
        assert_eq!(
            amber.findings().len(),
            1,
            "good and info rows are not findings"
        );
        let red = with_finding("c", crate::details::Verdict::Problem);
        assert_eq!(red.attention(), Attention::NeedsAttention);
    }

    #[test]
    fn working_devices_with_findings_are_listed_in_the_report_but_not_as_problems() {
        let all = report(vec![
            device(Status::Working, "plain"),
            with_finding("ram", crate::details::Verdict::Reduced),
        ]);
        let text = markdown_with_kernel(&all, None, &no_privacy(), None);
        assert!(
            text.contains("2 devices, 0 with problems. 1 could be better."),
            "{text}"
        );
        assert!(
            text.contains("- Could be better: Running speed: 4800 MT/s"),
            "{text}"
        );
        let working = text
            .split("#### Working devices")
            .nth(1)
            .expect("working list");
        assert_eq!(
            working.matches("Test device").count(),
            1,
            "the one with a finding isn't repeated here"
        );
        let single = markdown_with_kernel(&all, Some("ram"), &no_privacy(), None);
        assert!(single.contains("- Could be better: Running speed: 4800 MT/s"));
        let red = markdown_with_kernel(
            &report(vec![with_finding("hot", crate::details::Verdict::Problem)]),
            Some("hot"),
            &no_privacy(),
            None,
        );
        assert!(red.contains("- Needs attention: Running speed: 4800 MT/s"));
    }
}
