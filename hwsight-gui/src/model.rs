//! Turns a scan report into what the window shows. Pure functions, so they're tested
//! without a display.

use hwsight_core::{Device, LogIssue, ScanReport, Status, short_vendor};

use std::rc::Rc;

use hwsight_core::device::Attention;
use slint::{ModelRc, VecModel};

use crate::{
    DetailGroupItem, DetailItem, DeviceItem, FirmwareItem, FixItem, Row, SoftwareItem, ToolItem,
};

/// Everything the window needs from one scan.
pub struct View {
    pub devices: Vec<DeviceItem>,
    pub rows: Vec<Row>,
    pub summary: String,
    pub problem_count: i32,
    /// Working devices that have something that could be better.
    pub improve_count: i32,
    pub notes: Vec<String>,
    /// Missing tools hwsight can install for the user (shown in their own banner).
    pub tools: Vec<ToolItem>,
    /// Headline for the firmware banner, or "" when there is nothing to say.
    pub firmware_text: String,
    pub firmware_items: Vec<FirmwareItem>,
}

/// The firmware banner: how many updates can be installed, or that the update list is old.
pub fn firmware_banner(report: &ScanReport) -> (String, Vec<FirmwareItem>) {
    let items: Vec<FirmwareItem> = report
        .pending_firmware
        .iter()
        .map(|u| FirmwareItem {
            id: u.device_id.clone().into(),
            name: u.name.clone().into(),
            version: u.version.clone().into(),
            checked: true,
        })
        .collect();
    let n = items.len();
    let text = if n > 0 {
        hwsight_core::tr!(
            "1 firmware update is available" | "{n} firmware updates are available" % n
        )
    } else if report.firmware_catalog_old {
        hwsight_core::tr!("The list of available firmware updates is out of date")
    } else {
        String::new()
    };
    (text, items)
}

pub fn view(report: &ScanReport) -> View {
    let (firmware_text, firmware_items) = firmware_banner(report);
    let devices: Vec<DeviceItem> = report.devices.iter().map(device_item).collect();
    let rows = filter_rows(&report.devices, "", false);

    let problems = report
        .devices
        .iter()
        .filter(|d| d.status.is_problem())
        .count()
        + report.port_issues.len();
    let improve = report
        .devices
        .iter()
        .filter(|d| !d.status.is_problem() && d.attention() != Attention::Fine)
        .count();
    let total = report.devices.len();
    let summary = match (total, problems, improve) {
        (0, _, _) => hwsight_core::tr!("No devices found"),
        (_, 0, 0) => {
            let devices = hwsight_core::tr!("1 device" | "{n} devices" % total);
            hwsight_core::tr!("{}, no problems found", devices)
        }
        (_, 0, _) => {
            let devices = hwsight_core::tr!("1 device" | "{n} devices" % total);
            let better = hwsight_core::tr!("1 could be better" | "{n} could be better" % improve);
            hwsight_core::tr!("{}, {}", devices, better)
        }
        (_, _, _) => {
            let devices = hwsight_core::tr!("1 device" | "{n} devices" % total);
            let problems = hwsight_core::tr!("1 problem" | "{n} problems" % problems);
            hwsight_core::tr!("{}, {}", devices, problems)
        }
    };

    let mut notes: Vec<String> = report
        .port_issues
        .iter()
        .map(|p| hwsight_core::tr!("{}: {}", p.port, p.issue.message()))
        .collect();
    notes.extend(report.firmware_note.iter().cloned());
    let mut tools = Vec::new();
    for note in &report.notes {
        match note {
            hwsight_core::ScanNote::MissingTool { tool } if tool.installable => {
                tools.push(ToolItem {
                    name: tool.name.clone().into(),
                    benefit: tool.benefit.clone().into(),
                    checked: true,
                });
            }
            other => notes.push(other.message()),
        }
    }

    View {
        devices,
        rows,
        summary,
        problem_count: problems as i32,
        improve_count: improve as i32,
        notes,
        tools,
        firmware_text,
        firmware_items,
    }
}

/// List rows (a heading per category, then its devices) for the devices that match.
/// Every word of `query` must appear, ignoring case, in the name, maker, category,
/// hardware ID, or driver. `Row.device` always indexes the full `devices` list.
pub fn filter_rows(devices: &[Device], query: &str, only_problems: bool) -> Vec<Row> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut rows: Vec<Row> = Vec::new();
    let mut category: Option<hwsight_core::Category> = None;
    let mut category_start = 0;
    let mut category_count = 0;

    for (index, device) in devices.iter().enumerate() {
        let fields = [
            device.display_name.to_lowercase(),
            device
                .vendor_name
                .as_deref()
                .unwrap_or_default()
                .to_lowercase(),
            device.category.label().to_lowercase(),
            device.hardware_id.to_lowercase(),
            device.driver.as_deref().unwrap_or_default().to_lowercase(),
        ];
        let matches =
            (!only_problems || device.status.is_problem() || device.attention() != Attention::Fine)
                && words
                    .iter()
                    .all(|word| fields.iter().any(|field| field.contains(word)));
        if !matches {
            continue;
        }
        if category != Some(device.category) {
            if category.is_some() {
                rows[category_start].header_count = category_count;
            }
            category = Some(device.category);
            category_start = rows.len();
            category_count = 0;
            rows.push(Row {
                is_header: true,
                header_text: device.category.label().into(),
                header_count: 0,
                device: -1,
            });
        }
        rows.push(Row {
            is_header: false,
            header_text: Default::default(),
            header_count: 0,
            device: index as i32,
        });
        category_count += 1;
    }
    if category.is_some() {
        rows[category_start].header_count = category_count;
    }
    rows
}

pub fn next_device(rows: &[Row], selected: i32, step: i32) -> i32 {
    let devices: Vec<i32> = rows
        .iter()
        .filter(|row| !row.is_header)
        .map(|row| row.device)
        .collect();
    if devices.is_empty() {
        return -1;
    }
    let Some(current) = devices.iter().position(|&device| device == selected) else {
        return if step < 0 {
            devices[devices.len() - 1]
        } else {
            devices[0]
        };
    };
    let next = if step < 0 {
        current.saturating_sub(1)
    } else {
        (current + 1).min(devices.len() - 1)
    };
    devices[next]
}

/// "ok", "warn", "error", or "neutral": picks the status colour in the UI. A device whose
/// drivers are fine but that has an amber or red finding shows that colour instead.
pub fn status_kind(status: Status, attention: Attention) -> &'static str {
    if !status.is_problem() {
        match attention {
            Attention::NeedsAttention => return "error",
            Attention::CouldBeBetter => return "warn",
            Attention::Fine => {}
        }
    }
    match status {
        Status::Working => "ok",
        Status::NoDriverNeeded | Status::ReservedForVirtualMachine => "neutral",
        Status::DriverAvailableNotLoaded | Status::DriverNotAttached | Status::DriverNotReady => {
            "warn"
        }
        Status::NoDriver | Status::DriverBlocked | Status::DriveFailing => "error",
    }
}

/// The words next to a device's colour: its driver status, or, when that's fine but
/// something could be better, say so plainly.
fn status_text(d: &Device) -> String {
    if d.status.is_problem() {
        return d.status.summary();
    }
    match d.attention() {
        Attention::NeedsAttention => hwsight_core::tr!("Needs attention"),
        Attention::CouldBeBetter => hwsight_core::tr!("Could be better"),
        Attention::Fine => d.status.summary(),
    }
}

fn device_item(d: &Device) -> DeviceItem {
    let mut why: Vec<String> = Vec::new();
    if d.status.is_problem() {
        if let Some(what) = &d.missing_interface {
            why.push(hwsight_core::tr!(
                "The driver is loaded, but no {} was created.",
                what
            ));
        }
        why.extend(d.log_issues.iter().map(LogIssue::message));
    }
    let fixes: Vec<FixItem> = d
        .fixes
        .iter()
        .map(|f| FixItem {
            text: f.text.clone().into(),
            command: f.command.clone().unwrap_or_default().into(),
        })
        .collect();
    let software: Vec<SoftwareItem> = d
        .software
        .iter()
        .map(|s| SoftwareItem {
            name: s.name.clone().into(),
            what: s.what.clone().into(),
            installed: s.installed,
            command: s.install_command.clone().unwrap_or_default().into(),
            how_to_get: s.how_to_get.clone().unwrap_or_default().into(),
            homepage: s.homepage.clone().into(),
        })
        .collect();

    let finding_items = |detail: &hwsight_core::details::Detail| DetailItem {
        label: detail.label.clone().into(),
        // "Label: value" as one line, written the way the language writes it.
        value: hwsight_core::tr!("{}: {}", detail.label, detail.value).into(),
        raw: Default::default(),
        explanation: detail.explanation.clone().unwrap_or_default().into(),
        source: Default::default(),
        kind: detail.verdict.kind().into(),
        technical: false,
        command: detail.command.clone().unwrap_or_default().into(),
    };
    let findings: Vec<DetailItem> = d.findings().into_iter().map(finding_items).collect();

    let details: Vec<DetailGroupItem> = d
        .details
        .iter()
        .map(|g| DetailGroupItem {
            title: g.title.clone().into(),
            more_count: g.items.iter().filter(|i| !i.technical).count() as i32,
            items: ModelRc::from(Rc::new(VecModel::from(
                g.items
                    .iter()
                    .map(|i| DetailItem {
                        label: i.label.clone().into(),
                        value: i.value.clone().into(),
                        raw: i
                            .raw
                            .clone()
                            .filter(|r| *r != i.value)
                            .unwrap_or_default()
                            .into(),
                        explanation: i.explanation.clone().unwrap_or_default().into(),
                        source: i.source.clone().into(),
                        kind: i.verdict.kind().into(),
                        technical: i.technical,
                        command: i.command.clone().unwrap_or_default().into(),
                    })
                    .collect::<Vec<_>>(),
            ))),
        })
        .collect();

    DeviceItem {
        name: d.display_name.clone().into(),
        vendor: d
            .vendor_name
            .as_deref()
            .map(short_vendor)
            .unwrap_or_default()
            .into(),
        category: d.category.label().into(),
        status_kind: status_kind(d.status, d.attention()).into(),
        status_text: status_text(d).into(),
        driver: d.driver.clone().unwrap_or_default().into(),
        hardware_id: d.hardware_id.clone().into(),
        address: d.address.clone().into(),
        bus: match d.bus {
            hwsight_core::Bus::Pci => hwsight_core::tr!("PCI"),
            hwsight_core::Bus::Usb => hwsight_core::tr!("USB"),
            hwsight_core::Bus::System => hwsight_core::tr!("Built in"),
            _ => hwsight_core::tr!("Other"),
        }
        .into(),
        why: why.join("\n").into(),
        fixes: ModelRc::from(Rc::new(VecModel::from(fixes))),
        software: ModelRc::from(Rc::new(VecModel::from(software))),
        details: ModelRc::from(Rc::new(VecModel::from(details))),
        findings: ModelRc::from(Rc::new(VecModel::from(findings))),
        note: d.note.clone().unwrap_or_default().into(),
        description: d.description.clone().unwrap_or_default().into(),
        unrecognized: d.unrecognized,
        built_in: d.bus == hwsight_core::Bus::System,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwsight_core::{KernelLogInput, Scanner};
    use slint::Model;
    use std::fs;

    /// A fake machine with a working GPU and a network card missing firmware.
    fn report() -> ScanReport {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let add = |addr: &str, vendor: &str, device: &str, class: &str, alias: &str| {
            let dir = root.join("pci").join(addr);
            fs::create_dir_all(&dir).expect("mkdir");
            for (f, v) in [
                ("vendor", vendor),
                ("device", device),
                ("class", class),
                ("modalias", alias),
            ] {
                fs::write(dir.join(f), v).expect("write");
            }
        };
        add(
            "0000:01:00.0",
            "0x10de",
            "0x2c05",
            "0x030000",
            "pci:v000010DEd00002C05sv0sd0bc03sc00i00",
        );
        add(
            "0000:07:00.0",
            "0x10ec",
            "0x8125",
            "0x020000",
            "pci:v000010ECd00008125sv0sd0bc02sc00i00",
        );
        let gpu = root.join("pci/0000:01:00.0");
        fs::create_dir_all(root.join("drivers/nvidia")).expect("mkdir");
        std::os::unix::fs::symlink(root.join("drivers/nvidia"), gpu.join("driver")).expect("link");
        fs::create_dir_all(gpu.join("drm")).expect("mkdir");
        let modules = root.join("modules");
        fs::create_dir_all(modules.join("r8169")).expect("mkdir");
        fs::write(
            modules.join("modules.alias"),
            "alias pci:v000010ECd00008125* r8169\n",
        )
        .expect("write");

        let scanner = Scanner {
            data_dirs: hwsight_core::DataDirs::only(&root.join("data")),
            pci_devices: root.join("pci"),
            usb_devices: root.join("usb"),
            kernel_modules: Some(modules.clone()),
            loaded_modules: modules,
            modprobe_dirs: Vec::new(),
            kernel_cmdline: root.join("cmdline"),
            kernel_log: KernelLogInput::Lines(vec![
                "r8169 0000:07:00.0: Direct firmware load for rtl_nic/rtl8125b-2.fw failed with error -2".into(),
            ]),
            firmware_dirs: Vec::new(),
            distro: hwsight_core::Distro::from_os_release("ID=arch\n", false),
            install_check: hwsight_core::software::InstallCheck {
                path_dirs: Vec::new(),
                flatpak_dirs: Vec::new(),
            },
            system: hwsight_core::SystemPaths::none(),
        };
        scanner.scan()
    }

    #[test]
    fn rows_have_a_heading_per_category() {
        let v = view(&report());
        assert_eq!(v.devices.len(), 2);
        let headings: Vec<String> = v
            .rows
            .iter()
            .filter(|r| r.is_header)
            .map(|r| r.header_text.to_string())
            .collect();
        assert_eq!(headings, ["Graphics", "Network"]);
        assert_eq!(v.rows.len(), 4);
        assert_eq!(v.rows[0].header_count, 1);
        assert_eq!(v.rows[1].device, 0);
        assert_eq!(v.rows[3].device, 1);
    }

    #[test]
    fn problems_carry_why_and_fix() {
        let v = view(&report());
        assert_eq!(v.problem_count, 1);
        assert_eq!(v.summary, "2 devices, 1 problem");
        let nic = &v.devices[1];
        assert_eq!(nic.status_kind.as_str(), "warn");
        assert!(nic.why.contains("rtl_nic/rtl8125b-2.fw"));
        assert_eq!(
            nic.fixes
                .row_data(0)
                .map(|f| f.command.to_string())
                .as_deref(),
            Some("sudo pacman -S linux-firmware-realtek")
        );
        assert!(
            nic.fixes.row_count() >= 2,
            "firmware step plus the status step"
        );
        let gpu = &v.devices[0];
        assert_eq!(gpu.status_kind.as_str(), "ok");
        assert!(gpu.why.is_empty() && gpu.fixes.row_count() == 0);
    }

    fn tip(installed: bool) -> hwsight_core::software::Tip {
        hwsight_core::software::Tip {
            id: "solaar".into(),
            name: "Solaar".into(),
            what: "Pairs Logitech mice.".into(),
            homepage: "https://pwr-solaar.github.io/Solaar/".into(),
            installed,
            install_command: (!installed).then(|| "sudo pacman -S solaar".into()),
            how_to_get: None,
        }
    }

    #[test]
    fn installed_software_has_no_command() {
        let mut r = report();
        r.devices[0].software = vec![tip(true)];
        let item = view(&r).devices[0].software.row_data(0).expect("item");
        assert_eq!(item.name.as_str(), "Solaar");
        assert!(item.installed);
        assert_eq!(item.command.as_str(), "");
        assert_eq!(item.how_to_get.as_str(), "");
        assert_eq!(
            item.homepage.as_str(),
            "https://pwr-solaar.github.io/Solaar/"
        );
    }

    #[test]
    fn missing_software_carries_its_command() {
        let mut r = report();
        r.devices[0].software = vec![tip(false)];
        let v = view(&r);
        let item = v.devices[0].software.row_data(0).expect("item");
        assert!(!item.installed);
        assert_eq!(item.what.as_str(), "Pairs Logitech mice.");
        assert_eq!(item.command.as_str(), "sudo pacman -S solaar");
        assert_eq!(v.devices[1].software.row_count(), 0);
    }

    #[test]
    fn details_keep_verdicts_levels_and_sources() {
        use hwsight_core::details::{Detail, DetailGroup, Verdict};
        let mut r = report();
        r.devices[0].details = vec![DetailGroup {
            title: "PCIe connection".into(),
            items: vec![
                Detail::new("Link width".into(), "x4".into(), "/sys/w".into())
                    .raw("4 (device maximum 16)")
                    .verdict(Verdict::Reduced, Some("Using 4 lanes.".into())),
                Detail::new("Interrupt".into(), "75".into(), "/sys/irq".into())
                    .raw("75")
                    .technical(),
            ],
        }];
        let v = view(&r);
        let group = v.devices[0].details.row_data(0).expect("group");
        assert_eq!(group.title.as_str(), "PCIe connection");
        assert_eq!(group.more_count, 1, "only the first shows in More details");
        let width = group.items.row_data(0).expect("item");
        assert_eq!(width.kind.as_str(), "warn");
        assert_eq!(width.raw.as_str(), "4 (device maximum 16)");
        assert_eq!(width.explanation.as_str(), "Using 4 lanes.");
        assert_eq!(width.source.as_str(), "/sys/w");
        assert!(!width.technical);
        let irq = group.items.row_data(1).expect("item");
        assert!(irq.technical);
        assert_eq!(
            irq.raw.as_str(),
            "",
            "a raw value equal to the value is dropped"
        );
        assert_eq!(v.devices[1].details.row_count(), 0);
    }

    fn with_amber_finding(
        r: &mut ScanReport,
        device: usize,
        verdict: hwsight_core::details::Verdict,
    ) {
        use hwsight_core::details::{Detail, DetailGroup};
        r.devices[device].details = vec![DetailGroup {
            title: "Memory".into(),
            items: vec![
                Detail::new("Running speed".into(), "4800 MT/s".into(), "x".into())
                    .verdict(verdict, Some("Slower than rated.".into())),
            ],
        }];
    }

    #[test]
    fn a_working_device_with_a_finding_shows_it_at_the_simple_level() {
        use hwsight_core::details::Verdict;
        let mut r = report();
        with_amber_finding(&mut r, 0, Verdict::Reduced);
        let v = view(&r);
        let gpu = &v.devices[0];
        assert_eq!(gpu.status_kind.as_str(), "warn", "amber, not green");
        assert_eq!(gpu.status_text.as_str(), "Could be better");
        let finding = gpu.findings.row_data(0).expect("finding");
        assert_eq!(finding.value.as_str(), "Running speed: 4800 MT/s");
        assert_eq!(finding.explanation.as_str(), "Slower than rated.");
        assert_eq!(finding.kind.as_str(), "warn");
        assert_eq!(v.improve_count, 1);
        assert_eq!(
            v.problem_count, 1,
            "the missing-firmware card is still the one problem"
        );
        assert_eq!(v.devices[1].findings.row_count(), 0);

        with_amber_finding(&mut r, 0, Verdict::Problem);
        let v = view(&r);
        assert_eq!(v.devices[0].status_kind.as_str(), "error");
        assert_eq!(v.devices[0].status_text.as_str(), "Needs attention");
    }

    #[test]
    fn the_header_says_how_many_could_be_better_when_nothing_is_broken() {
        use hwsight_core::details::Verdict;
        let mut r = report();
        r.devices.retain(|d| !d.status.is_problem());
        assert_eq!(view(&r).summary, "1 device, no problems found");
        with_amber_finding(&mut r, 0, Verdict::Reduced);
        let v = view(&r);
        assert_eq!(v.problem_count, 0);
        assert_eq!(v.improve_count, 1);
        assert_eq!(v.summary, "1 device, 1 could be better");
    }

    #[test]
    fn only_problems_also_keeps_devices_with_findings() {
        use hwsight_core::details::Verdict;
        let mut r = report();
        with_amber_finding(&mut r, 0, Verdict::Reduced);
        let shown: Vec<i32> = filter_rows(&r.devices, "", true)
            .iter()
            .filter(|row| !row.is_header)
            .map(|row| row.device)
            .collect();
        assert_eq!(
            shown,
            [0, 1],
            "the card with a finding and the one with a problem"
        );
    }

    #[test]
    fn every_status_has_a_kind() {
        for (status, kind) in [
            (Status::Working, "ok"),
            (Status::NoDriverNeeded, "neutral"),
            (Status::ReservedForVirtualMachine, "neutral"),
            (Status::DriverAvailableNotLoaded, "warn"),
            (Status::DriverNotAttached, "warn"),
            (Status::DriverNotReady, "warn"),
            (Status::NoDriver, "error"),
            (Status::DriveFailing, "error"),
            (Status::DriverBlocked, "error"),
        ] {
            assert_eq!(status_kind(status, Attention::Fine), kind, "{status:?}");
        }
    }

    #[test]
    fn empty_query_matches_everything() {
        let report = report();
        let rows = filter_rows(&report.devices, "", false);
        assert_eq!(rows.len(), 4);
    }

    /// Each word must be matched through the field under test only.
    #[test]
    fn each_field_is_searched_on_its_own() {
        let report = report();
        let mut devices = report.devices.clone();
        devices[0].display_name = "Video card".into();
        devices[0].vendor_name = Some("NVIDIA Corporation".into());
        devices[0].driver = Some("nvidia".into());
        devices[1].display_name = "Wired port".into();
        devices[1].vendor_name = Some("Realtek".into());
        devices[1].driver = Some("r8169".into());
        let only = |q: &str| -> Vec<i32> {
            filter_rows(&devices, q, false)
                .iter()
                .filter(|r| !r.is_header)
                .map(|r| r.device)
                .collect()
        };
        assert_eq!(only("corporation"), [0]); // maker only
        assert_eq!(only("GRAPHICS"), [0]); // category only, any case
        assert_eq!(only("10de:2c05"), [0]); // hardware ID only
        assert_eq!(only("r8169"), [1]); // driver only
        assert_eq!(only("realtek wired"), [1]); // both words, different fields
        assert_eq!(only("realtek video"), Vec::<i32>::new()); // words on different devices
        assert_eq!(only("   "), [0, 1]); // blank query matches everything
    }

    #[test]
    fn counts_only_matching_devices_in_a_shared_category() {
        let report = report();
        let mut devices = report.devices.clone();
        let mut second_gpu = devices[0].clone();
        second_gpu.display_name = "Other GPU".into();
        devices.insert(1, second_gpu);
        let all = filter_rows(&devices, "", false);
        assert_eq!(all[0].header_count, 2);
        let some = filter_rows(&devices, "other", false);
        assert_eq!(some.len(), 2);
        assert_eq!(some[0].header_count, 1);
        assert_eq!(some[1].device, 1);
    }

    #[test]
    fn words_match_any_field_ignoring_case() {
        let report = report();
        let mut devices = report.devices.clone();
        devices[0].display_name = "NVIDIA graphics".into();
        devices[0].vendor_name = Some("NVIDIA".into());
        devices[0].driver = Some("nvidia".into());
        devices[1].vendor_name = Some("Realtek".into());
        devices[1].display_name = "RTL8125 network controller".into();
        devices[1].driver = Some("r8169".into());
        assert_eq!(filter_rows(&devices, "nvidia", false)[1].device, 0);
        assert_eq!(filter_rows(&devices, "realtek 8125", false)[1].device, 1);
        assert_eq!(filter_rows(&devices, "graphics", false)[1].device, 0);
        assert_eq!(filter_rows(&devices, "GRAPHICS", false)[1].device, 0);
        assert_eq!(filter_rows(&devices, "10de:2c05", false)[1].device, 0);
        assert_eq!(filter_rows(&devices, "r8169", false)[1].device, 1);
    }

    #[test]
    fn only_problems_hides_working_devices() {
        let report = report();
        let rows = filter_rows(&report.devices, "", true);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].device, 1);
    }

    #[test]
    fn headings_only_for_categories_with_matches_and_counts_are_right() {
        let report = report();
        let rows = filter_rows(&report.devices, "nvidia", false);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].header_text, "Graphics");
        assert_eq!(rows[0].header_count, 1);
    }

    #[test]
    fn row_indexes_point_into_the_full_list() {
        let report = report();
        let rows = filter_rows(&report.devices, "network", false);
        assert_eq!(rows[1].device, 1);
        assert!(rows[1].device < report.devices.len() as i32);
    }

    #[test]
    fn view_uses_filter_rows() {
        let report = report();
        let v = view(&report);
        assert_eq!(v.rows, filter_rows(&report.devices, "", false));
    }

    #[test]
    fn next_device_skips_headings() {
        let rows = vec![
            Row {
                is_header: true,
                header_text: "Graphics".into(),
                header_count: 2,
                device: -1,
            },
            Row {
                is_header: false,
                header_text: Default::default(),
                header_count: 0,
                device: 3,
            },
            Row {
                is_header: false,
                header_text: Default::default(),
                header_count: 0,
                device: 7,
            },
            Row {
                is_header: true,
                header_text: "Network".into(),
                header_count: 1,
                device: -1,
            },
            Row {
                is_header: false,
                header_text: Default::default(),
                header_count: 0,
                device: 9,
            },
        ];
        assert_eq!(next_device(&rows, 3, 1), 7);
        assert_eq!(next_device(&rows, 7, -1), 3);
    }

    #[test]
    fn next_device_from_nothing_selected() {
        let rows = vec![
            Row {
                is_header: true,
                header_text: "Graphics".into(),
                header_count: 2,
                device: -1,
            },
            Row {
                is_header: false,
                header_text: Default::default(),
                header_count: 0,
                device: 3,
            },
            Row {
                is_header: false,
                header_text: Default::default(),
                header_count: 0,
                device: 7,
            },
        ];
        assert_eq!(next_device(&rows, -1, 1), 3);
        assert_eq!(next_device(&rows, -1, -1), 7);
    }

    #[test]
    fn next_device_stays_at_the_ends() {
        let rows = vec![
            Row {
                is_header: false,
                header_text: Default::default(),
                header_count: 0,
                device: 3,
            },
            Row {
                is_header: false,
                header_text: Default::default(),
                header_count: 0,
                device: 7,
            },
        ];
        assert_eq!(next_device(&rows, 3, -1), 3);
        assert_eq!(next_device(&rows, 7, 1), 7);
    }

    #[test]
    fn next_device_with_no_devices() {
        let rows = vec![Row {
            is_header: true,
            header_text: "Graphics".into(),
            header_count: 0,
            device: -1,
        }];
        assert_eq!(next_device(&rows, -1, 1), -1);
    }
}
