//! Turns a scan report into what the window shows. Pure functions, so they're tested
//! without a display.

use hwsight_core::{Device, LogIssue, ScanReport, Status, short_vendor};

use crate::{DeviceItem, Row};

/// Everything the window needs from one scan.
pub struct View {
    pub devices: Vec<DeviceItem>,
    pub rows: Vec<Row>,
    pub summary: String,
    pub problem_count: i32,
    pub notes: Vec<String>,
}

pub fn view(report: &ScanReport) -> View {
    let devices: Vec<DeviceItem> = report.devices.iter().map(device_item).collect();
    let rows = filter_rows(&report.devices, "", false);

    let problems = report
        .devices
        .iter()
        .filter(|d| d.status.is_problem())
        .count()
        + report.port_issues.len();
    let total = report.devices.len();
    let summary = match (total, problems) {
        (0, _) => "No devices found".to_owned(),
        (_, 0) => format!("{total} devices, no problems found"),
        (_, 1) => format!("{total} devices, 1 problem"),
        (_, n) => format!("{total} devices, {n} problems"),
    };

    let mut notes: Vec<String> = report
        .port_issues
        .iter()
        .map(|p| format!("{}: {}", p.port, p.issue.message()))
        .collect();
    notes.extend(report.firmware_note.iter().cloned());
    notes.extend(report.notes.iter().map(|n| n.message()));

    View {
        devices,
        rows,
        summary,
        problem_count: problems as i32,
        notes,
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
        let matches = (!only_problems || device.status.is_problem())
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

/// "ok", "warn", "error", or "neutral": picks the status colour in the UI.
pub fn status_kind(status: Status) -> &'static str {
    match status {
        Status::Working => "ok",
        Status::NoDriverNeeded | Status::ReservedForVirtualMachine => "neutral",
        Status::DriverAvailableNotLoaded | Status::DriverNotAttached | Status::DriverNotReady => {
            "warn"
        }
        Status::NoDriver | Status::DriverBlocked => "error",
    }
}

fn device_item(d: &Device) -> DeviceItem {
    let mut why: Vec<String> = Vec::new();
    if d.status.is_problem() {
        if let Some(what) = &d.missing_interface {
            why.push(format!("The driver is loaded, but no {what} was created."));
        }
        why.extend(d.log_issues.iter().map(LogIssue::message));
    }
    let fix = d
        .log_issues
        .iter()
        .find_map(|i| match i {
            LogIssue::MissingFirmware {
                install_command: Some(cmd),
                installed_now: false,
                ..
            } if d.status.is_problem() => Some(cmd.clone()),
            _ => None,
        })
        .unwrap_or_default();

    DeviceItem {
        name: d.display_name.clone().into(),
        vendor: d
            .vendor_name
            .as_deref()
            .map(short_vendor)
            .unwrap_or_default()
            .into(),
        category: d.category.label().into(),
        status_kind: status_kind(d.status).into(),
        status_text: d.status.summary().into(),
        driver: d.driver.clone().unwrap_or_default().into(),
        hardware_id: d.hardware_id.clone().into(),
        address: d.address.clone().into(),
        bus: match d.bus {
            hwsight_core::Bus::Pci => "PCI",
            hwsight_core::Bus::Usb => "USB",
            _ => "Other",
        }
        .into(),
        why: why.join("\n").into(),
        fix: fix.into(),
        note: d.note.clone().unwrap_or_default().into(),
        unrecognized: d.unrecognized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwsight_core::{KernelLogInput, Scanner};
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
        assert_eq!(nic.fix.as_str(), "sudo pacman -S linux-firmware-realtek");
        let gpu = &v.devices[0];
        assert_eq!(gpu.status_kind.as_str(), "ok");
        assert!(gpu.why.is_empty() && gpu.fix.is_empty());
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
            (Status::DriverBlocked, "error"),
        ] {
            assert_eq!(status_kind(status), kind, "{status:?}");
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
}
