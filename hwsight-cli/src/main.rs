//! hwsight command line: lists devices and their status. Mainly for testing the core and
//! for bug reports (`--json`).

use std::io::{self, Write};
use std::process::ExitCode;

use hwsight_core::{Category, ScanReport, Status, short_vendor};

const HELP: &str = "\
hwsight: see every device in your computer and whether it's working

Usage: hwsight [OPTIONS]

Options:
  --problems   Only show devices that have a problem
  --json       Print everything as JSON (for bug reports and scripts)
  --ids        Also show hardware IDs and bus addresses
  -h, --help   Show this help
  -V, --version
";

struct Options {
    json: bool,
    problems_only: bool,
    show_ids: bool,
}

fn main() -> ExitCode {
    let mut opts = Options {
        json: false,
        problems_only: false,
        show_ids: false,
    };
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--json" => opts.json = true,
            "--problems" => opts.problems_only = true,
            "--ids" => opts.show_ids = true,
            "-h" | "--help" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
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

    let report = hwsight_core::scan();
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
        if opts.show_ids {
            writeln!(
                out,
                "      id {}  at {}",
                device.hardware_id, device.address
            )?;
        }
    }

    let problems = report
        .devices
        .iter()
        .filter(|d| d.status.is_problem())
        .count();
    writeln!(out)?;
    match (shown.is_empty(), problems) {
        (true, 0) if opts.problems_only => writeln!(out, "No problems found.")?,
        (true, _) => writeln!(out, "No devices found.")?,
        (false, 0) => writeln!(out, "{} devices, no problems found.", report.devices.len())?,
        (false, 1) => writeln!(out, "{} devices, 1 problem.", report.devices.len())?,
        (false, n) => writeln!(out, "{} devices, {n} problems.", report.devices.len())?,
    }
    for note in &report.notes {
        writeln!(out, "Note: {}", note.message())?;
    }
    Ok(())
}

fn symbol(status: Status) -> &'static str {
    match status {
        Status::Working | Status::NoDriverNeeded => "✓",
        Status::DriverAvailableNotLoaded | Status::DriverNotAttached => "!",
        Status::NoDriver | Status::DriverBlocked => "✗",
        Status::ReservedForVirtualMachine => "•",
    }
}
