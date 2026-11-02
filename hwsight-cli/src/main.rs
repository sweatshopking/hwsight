//! hwsight command line: lists devices and their status. Mainly for testing the core and
//! for bug reports (`--json`).

use std::io::{self, Write};
use std::process::ExitCode;

use hwsight_core::{Category, LogIssue, ScanReport, Status, short_vendor};

const HELP: &str = "\
hwsight: see every device in your computer and whether it's working

Usage: hwsight [OPTIONS]

Options:
  --problems   Only show devices that have a problem
  --json       Print everything as JSON (for bug reports and scripts)
  --ids        Also show hardware IDs and bus addresses
  --kernel-log FILE
               Explain problems using this saved kernel log (from `journalctl -k -b`
               or `dmesg`) instead of reading this computer's log
  --update-data
               Download the newest device data (names, rules, firmware packages)
               and exit. Updates are signed and checked before use.
  -h, --help   Show this help
  -V, --version
";

struct Options {
    kernel_log: Option<std::path::PathBuf>,
    json: bool,
    problems_only: bool,
    show_ids: bool,
}

fn main() -> ExitCode {
    let mut opts = Options {
        kernel_log: None,
        json: false,
        problems_only: false,
        show_ids: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--kernel-log" => match args.next() {
                Some(path) => opts.kernel_log = Some(path.into()),
                None => {
                    eprintln!("hwsight: --kernel-log needs a file name");
                    return ExitCode::from(2);
                }
            },
            "--json" => opts.json = true,
            "--problems" => opts.problems_only = true,
            "--ids" => opts.show_ids = true,
            "-h" | "--help" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "--update-data" => return update_data(),
            "-V" | "--version" => {
                println!("hwsight {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("hwsight: unknown option `{other}`\n\n{HELP}");
                return ExitCode::from(2);
            }
        }
    }

    let mut scanner = hwsight_core::Scanner::system();
    if let Some(path) = &opts.kernel_log {
        match std::fs::read(path) {
            Ok(bytes) => {
                let lines = String::from_utf8_lossy(&bytes)
                    .lines()
                    .map(str::to_owned)
                    .collect();
                scanner.kernel_log = hwsight_core::KernelLogInput::Lines(lines);
            }
            Err(e) => {
                eprintln!("hwsight: can't read {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        }
    }
    let report = scanner.scan();
    let result = if opts.json {
        print_json(&report)
    } else {
        print_text(&report, &opts)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        // A closed pipe (e.g. `hwsight | head`) isn't worth an error message.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hwsight: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_json(report: &ScanReport) -> io::Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, report)?;
    writeln!(out)
}

fn print_text(report: &ScanReport, opts: &Options) -> io::Result<()> {
    let mut out = io::stdout().lock();
    let shown: Vec<_> = report
        .devices
        .iter()
        .filter(|d| !opts.problems_only || d.status.is_problem())
        .collect();

    let mut current: Option<Category> = None;
    for device in &shown {
        if current != Some(device.category) {
            if current.is_some() {
                writeln!(out)?;
            }
            writeln!(out, "{}", device.category.label())?;
            current = Some(device.category);
        }
        let mut line = format!("  {} {}", symbol(device.status), device.display_name);
        if let Some(vendor) = &device.vendor_name {
            line.push_str(&format!("  ({})", short_vendor(vendor)));
        }
        writeln!(out, "{line}")?;

        let mut detail = format!("      {}", device.status.summary());
        match (&device.driver, device.status) {
            (Some(driver), _) => detail.push_str(&format!(", driver: {driver}")),
            (None, Status::DriverBlocked) => {
                for c in &device.driver_candidates {
                    if let Some(by) = &c.blocked_by {
                        detail.push_str(&format!(", {} blocked in {by}", c.module));
                    }
                }
            }
            (None, Status::DriverAvailableNotLoaded | Status::DriverNotAttached) => {
                let names: Vec<&str> = device
                    .driver_candidates
                    .iter()
                    .map(|c| c.module.as_str())
                    .collect();
                detail.push_str(&format!(", available: {}", names.join(", ")));
            }
            _ => {}
        }
        if device.unrecognized {
            detail.push_str(", not in the device database yet");
        }
        writeln!(out, "{detail}")?;
        if device.status.is_problem() {
            if let Some(what) = &device.missing_interface {
                writeln!(
                    out,
                    "      Why: The driver is loaded, but no {what} was created."
                )?;
            }
            for issue in &device.log_issues {
                writeln!(out, "      Why: {}", issue.message())?;
                if let LogIssue::MissingFirmware {
                    install_command: Some(cmd),
                    installed_now: false,
                    ..
                } = issue
                {
                    writeln!(out, "      Fix: {cmd}")?;
                }
            }
        }
        if opts.show_ids {
            writeln!(
                out,
                "      id {}  at {}",
                device.hardware_id, device.address
            )?;
        }
    }

    // Devices that couldn't connect never appear in the list, so their ports are shown.
    if !report.port_issues.is_empty() {
        if !shown.is_empty() {
            writeln!(out)?;
        }
        writeln!(out, "USB ports")?;
        for p in &report.port_issues {
            writeln!(out, "  ✗ {}", p.port)?;
            writeln!(out, "      Why: {}", p.issue.message())?;
        }
    }

    let problems = report
        .devices
        .iter()
        .filter(|d| d.status.is_problem())
        .count()
        + report.port_issues.len();
    let devices = report.devices.len();
    writeln!(out)?;
    match (devices, problems) {
        (0, 0) => writeln!(out, "No devices found.")?,
        (_, 0) if opts.problems_only => writeln!(out, "No problems found.")?,
        (_, 0) => writeln!(out, "{devices} devices, no problems found.")?,
        (_, 1) => writeln!(out, "{devices} devices, 1 problem.")?,
        (_, n) => writeln!(out, "{devices} devices, {n} problems.")?,
    }
    if let Some(note) = &report.firmware_note {
        writeln!(out, "Note: {note}")?;
    }
    for note in &report.notes {
        writeln!(out, "Note: {}", note.message())?;
    }
    Ok(())
}

fn symbol(status: Status) -> &'static str {
    match status {
        Status::Working | Status::NoDriverNeeded => "✓",
        Status::DriverAvailableNotLoaded | Status::DriverNotAttached | Status::DriverNotReady => {
            "!"
        }
        Status::NoDriver | Status::DriverBlocked => "✗",
        Status::ReservedForVirtualMachine => "•",
    }
}

fn update_data() -> ExitCode {
    let dirs = hwsight_core::DataDirs::standard();
    let Some(dest) = dirs.download.clone() else {
        eprintln!("hwsight: can't find your home folder to save updates in");
        return ExitCode::FAILURE;
    };
    let fetch = hwsight_core::update::HttpFetch {
        base_url: std::env::var("HWSIGHT_DATA_URL")
            .unwrap_or_else(|_| hwsight_core::update::DEFAULT_SOURCE.to_owned()),
    };
    let current = hwsight_core::update::current_versions(&dirs);
    let result =
        hwsight_core::update::update(&fetch, hwsight_core::update::TRUSTED_KEYS, &dest, &current);
    if let Some(state) = hwsight_core::update::default_state_file() {
        let _ = hwsight_core::update::mark_checked(&state);
    }
    match result {
        Ok(outcome) if outcome.installed.is_empty() => {
            println!("Device data is up to date.");
            ExitCode::SUCCESS
        }
        Ok(outcome) => {
            for (name, version) in &outcome.installed {
                println!("Updated {name} to {version}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("hwsight: {}", e.message());
            ExitCode::FAILURE
        }
    }
}
