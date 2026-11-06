//! hwsight command line: lists devices and their status. Mainly for testing the core and
//! for bug reports (`--json`).

use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use hwsight_core::{Category, ScanReport, Status, short_vendor};

const HELP: &str = "\
hwsight: see every device in your computer and whether it's working

Usage: hwsight [OPTIONS]

Options:
  --problems   Only show devices that have a problem
  --json       Print everything as JSON (for bug reports and scripts)
  --no-color   Never use color in terminal output
  --ids        Also show hardware IDs and bus addresses
  --kernel-log FILE
               Explain problems using this saved kernel log (from `journalctl -k -b`
               or `dmesg`) instead of reading this computer's log
  --update-data
               Download the newest device data (names, rules, firmware packages)
               and exit. Updates are signed and checked before use.
  --check-update
               See whether a newer version of hwsight is out
  --update     Download and install the newest version of hwsight
  --report [ADDRESS]
               Print a report to paste into a forum post or bug report. Private details are removed.
  -h, --help   Show this help
  -V, --version
";

struct Options {
    command: Command,
    kernel_log: Option<std::path::PathBuf>,
    json: bool,
    no_color: bool,
    problems_only: bool,
    show_ids: bool,
    report: Option<Option<String>>,
}

enum Command {
    Scan,
    Help,
    Version,
    UpdateData,
    CheckUpdate,
    Update,
}

fn main() -> ExitCode {
    let opts = match parse_args(std::env::args().skip(1)) {
        Ok(opts) => opts,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    match opts.command {
        Command::Help => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Command::Version => {
            println!("hwsight {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Command::UpdateData => return update_data(),
        Command::CheckUpdate => return check_update(),
        Command::Update => return update_app(),
        Command::Scan => {}
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
    // --json and --report are for tools and for helpers on forums: always English.
    let report = if opts.json || opts.report.is_some() {
        hwsight_core::i18n::in_english(|| scanner.scan())
    } else {
        scanner.scan()
    };
    if let Some(address) = &opts.report {
        print!(
            "{}",
            hwsight_core::report::markdown(
                &report,
                address.as_deref(),
                &hwsight_core::report::Privacy::system()
            )
        );
        return ExitCode::SUCCESS;
    }
    let color = use_color(
        io::stdout().is_terminal(),
        std::env::var("NO_COLOR").ok().as_deref(),
        opts.no_color,
    );
    let result = if opts.json {
        print_json(&report)
    } else {
        print_text(&report, &opts, color)
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

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut opts = Options {
        command: Command::Scan,
        kernel_log: None,
        json: false,
        no_color: false,
        problems_only: false,
        show_ids: false,
        report: None,
    };
    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--kernel-log" => match args.next() {
                Some(path) => opts.kernel_log = Some(path.into()),
                None => return Err("hwsight: --kernel-log needs a file name".to_owned()),
            },
            "--json" => opts.json = true,
            "--no-color" => opts.no_color = true,
            "--problems" => opts.problems_only = true,
            "--ids" => opts.show_ids = true,
            "--report" => {
                let address = args.peek().filter(|next| !next.starts_with('-')).cloned();
                if address.is_some() {
                    args.next();
                }
                opts.report = Some(address);
            }
            "-h" | "--help" => opts.command = Command::Help,
            "--update-data" => opts.command = Command::UpdateData,
            "--check-update" => opts.command = Command::CheckUpdate,
            "--update" => opts.command = Command::Update,
            "-V" | "--version" => opts.command = Command::Version,
            other => return Err(format!("hwsight: unknown option `{other}`\n\n{HELP}")),
        }
    }
    if opts.json && opts.report.is_some() {
        return Err("hwsight: --report and --json can't be used together".to_owned());
    }
    Ok(opts)
}

fn check_update_text(
    result: Result<Option<hwsight_core::app_update::Release>, String>,
    current: &str,
) -> String {
    match result {
        Ok(Some(release)) if hwsight_core::app_update::is_newer(&release.version, current) => {
            hwsight_core::tr!(
                "hwsight {} is available (you have {}). Run hwsight --update to install it.",
                release.version,
                current
            )
        }
        Ok(_) => hwsight_core::tr!("hwsight {} is the newest version.", current),
        Err(e) => hwsight_core::tr!("Couldn't check for updates: {}", e),
    }
}

fn check_update() -> ExitCode {
    let result = hwsight_core::app_update::latest_release();
    let failed = result.is_err();
    println!("{}", check_update_text(result, env!("CARGO_PKG_VERSION")));
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn update_app() -> ExitCode {
    let release = match hwsight_core::app_update::latest_release() {
        Ok(Some(release)) => release,
        Ok(None) => {
            println!("{}", check_update_text(Ok(None), env!("CARGO_PKG_VERSION")));
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            println!("{}", check_update_text(Err(e), env!("CARGO_PKG_VERSION")));
            return ExitCode::FAILURE;
        }
    };
    let current_exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(_) => {
            println!(
                "{}",
                hwsight_core::tr!("Couldn't find where hwsight is installed.")
            );
            return ExitCode::FAILURE;
        }
    };
    let install =
        hwsight_core::app_update::detect_install(&current_exe, std::env::var_os("APPIMAGE"));
    let outcome =
        hwsight_core::app_update::install_update(&release, &install, env!("CARGO_PKG_VERSION"));
    let failed = matches!(outcome, hwsight_core::app_update::Outcome::Failed(_));
    println!("{}", outcome.message());
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn print_json(report: &ScanReport) -> io::Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, report)?;
    writeln!(out)
}

fn print_text(report: &ScanReport, opts: &Options, color: bool) -> io::Result<()> {
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
            writeln!(out, "{}", paint(&device.category.label(), "\x1b[1m", color))?;
            current = Some(device.category);
        }
        let status_code = match device.status {
            Status::Working => "\x1b[32m",
            Status::NoDriverNeeded | Status::ReservedForVirtualMachine => "\x1b[2m",
            Status::DriverAvailableNotLoaded
            | Status::DriverNotAttached
            | Status::DriverNotReady => "\x1b[33m",
            Status::NoDriver | Status::DriverBlocked => "\x1b[31m",
        };
        let mut line = format!(
            "  {} {}",
            paint(symbol(device.status), status_code, color),
            device.display_name
        );
        if let Some(vendor) = &device.vendor_name {
            line.push_str(&format!("  ({})", short_vendor(vendor)));
        }
        writeln!(out, "{line}")?;

        let mut detail = device.status.summary();
        match (&device.driver, device.status) {
            (Some(driver), _) => {
                detail = hwsight_core::tr!("{}, driver: {}", detail, driver);
            }
            (None, Status::DriverBlocked) => {
                for c in &device.driver_candidates {
                    if let Some(by) = &c.blocked_by {
                        detail = hwsight_core::tr!("{}, {} blocked in {}", detail, c.module, by);
                    }
                }
            }
            (None, Status::DriverAvailableNotLoaded | Status::DriverNotAttached) => {
                let names: Vec<&str> = device
                    .driver_candidates
                    .iter()
                    .map(|c| c.module.as_str())
                    .collect();
                detail = hwsight_core::tr!("{}, available: {}", detail, names.join(", "));
            }
            _ => {}
        }
        if device.unrecognized {
            detail = hwsight_core::tr!("{}, not in the device database yet", detail);
        }
        writeln!(out, "      {detail}")?;
        if device.status.is_problem() {
            if let Some(what) = &device.missing_interface {
                writeln!(
                    out,
                    "      {} {}",
                    paint(&hwsight_core::tr!("Why:"), "\x1b[1m", color),
                    hwsight_core::tr!("The driver is loaded, but no {} was created.", what)
                )?;
            }
            for issue in &device.log_issues {
                writeln!(
                    out,
                    "      {} {}",
                    paint(&hwsight_core::tr!("Why:"), "\x1b[1m", color),
                    issue.message()
                )?;
            }
            for (n, fix) in device.fixes.iter().enumerate() {
                let label = if device.fixes.len() == 1 {
                    hwsight_core::tr!("Fix:")
                } else {
                    hwsight_core::tr!("Fix {}:", n + 1)
                };
                writeln!(
                    out,
                    "      {} {}",
                    paint(&label, "\x1b[1m", color),
                    fix.text
                )?;
                if let Some(cmd) = &fix.command {
                    writeln!(out, "             {cmd}")?;
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
        writeln!(
            out,
            "{}",
            paint(&hwsight_core::tr!("USB ports"), "\x1b[1m", color)
        )?;
        for p in &report.port_issues {
            writeln!(out, "  {} {}", paint("✗", "\x1b[31m", color), p.port)?;
            writeln!(
                out,
                "      {} {}",
                paint(&hwsight_core::tr!("Why:"), "\x1b[1m", color),
                p.issue.message()
            )?;
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
        (0, 0) => writeln!(out, "{}", hwsight_core::tr!("No devices found."))?,
        (_, 0) if opts.problems_only => {
            writeln!(out, "{}", hwsight_core::tr!("No problems found."))?
        }
        (_, 0) => {
            let device_text = hwsight_core::tr!("1 device" | "{n} devices" % devices);
            writeln!(
                out,
                "{}",
                hwsight_core::tr!("{}, no problems found.", device_text)
            )?
        }
        (_, _) => {
            let device_text = hwsight_core::tr!("1 device" | "{n} devices" % devices);
            let problem_text = hwsight_core::tr!("1 problem" | "{n} problems" % problems);
            writeln!(
                out,
                "{}",
                hwsight_core::tr!("{}, {}.", device_text, problem_text)
            )?
        }
    }
    if let Some(note) = &report.firmware_note {
        writeln!(out, "{}", hwsight_core::tr!("Note: {}", note))?;
    }
    for note in &report.notes {
        writeln!(out, "{}", hwsight_core::tr!("Note: {}", note.message()))?;
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

fn use_color(is_terminal: bool, no_color_env: Option<&str>, flag_off: bool) -> bool {
    is_terminal && no_color_env.is_none_or(str::is_empty) && !flag_off
}

fn paint(text: &str, code: &str, on: bool) -> String {
    if on {
        format!("{code}{text}\x1b[0m")
    } else {
        text.to_owned()
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
            println!("{}", hwsight_core::tr!("Device data is up to date."));
            ExitCode::SUCCESS
        }
        Ok(outcome) => {
            for (name, version) in &outcome.installed {
                println!("{}", hwsight_core::tr!("Updated {} to {}", name, version));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("hwsight: {}", e.message());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_rules() {
        for (is_terminal, no_color_env, flag_off, expected) in [
            (false, None, false, false),
            (false, None, true, false),
            (false, Some("1"), false, false),
            (false, Some("1"), true, false),
            (true, None, false, true),
            (true, None, true, false),
            (true, Some("1"), false, false),
            (true, Some("1"), true, false),
        ] {
            assert_eq!(
                use_color(is_terminal, no_color_env, flag_off),
                expected,
                "terminal={is_terminal}, env={no_color_env:?}, flag={flag_off}"
            );
        }
        assert!(use_color(true, Some(""), false));
    }

    #[test]
    fn paint_off_is_plain() {
        assert_eq!(paint("hello", "\x1b[32m", false), "hello");
    }

    #[test]
    fn paint_on_wraps_and_resets() {
        assert_eq!(paint("hello", "\x1b[32m", true), "\x1b[32mhello\x1b[0m");
    }

    #[test]
    fn report_and_json_conflict() {
        let result = parse_args(["--report", "--json"].into_iter().map(str::to_owned));
        assert!(matches!(
            result,
            Err(e) if e == "hwsight: --report and --json can't be used together"
        ));
    }

    #[test]
    fn check_update_text_newer() {
        let release = hwsight_core::app_update::Release {
            version: "9.9.9".into(),
            assets: vec![],
        };
        assert_eq!(
            check_update_text(Ok(Some(release)), "0.1.0"),
            "hwsight 9.9.9 is available (you have 0.1.0). Run hwsight --update to install it."
        );
    }

    #[test]
    fn check_update_text_same_and_none() {
        let release = hwsight_core::app_update::Release {
            version: "9.9.9".into(),
            assets: vec![],
        };
        assert_eq!(
            check_update_text(Ok(Some(release)), "9.9.9"),
            "hwsight 9.9.9 is the newest version."
        );
        assert_eq!(
            check_update_text(Ok(None), "0.1.0"),
            "hwsight 0.1.0 is the newest version."
        );
    }

    #[test]
    fn check_update_text_error() {
        assert_eq!(
            check_update_text(Err("offline".to_owned()), "0.1.0"),
            "Couldn't check for updates: offline"
        );
    }
}
