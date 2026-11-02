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

    // Devices arrive sorted by category, so a heading goes before each new category.
    let mut rows = Vec::new();
    let mut i = 0;
    while i < report.devices.len() {
        let category = report.devices[i].category;
        let count = report.devices[i..]
            .iter()
            .take_while(|d| d.category == category)
            .count();
        rows.push(Row {
            is_header: true,
            header_text: category.label().into(),
            header_count: count as i32,
            device: -1,
        });
        for index in i..i + count {
            rows.push(Row {
                is_header: false,
                header_text: Default::default(),
                header_count: 0,
                device: index as i32,
            });
        }
        i += count;
    }

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
}
