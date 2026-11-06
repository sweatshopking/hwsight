//! hwsight graphical app.

mod app_updates;
mod model;
mod settings;

use std::{
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

// Code generated from ui/*.slint. Our no-unwrap rule is for our code; the generator
// uses unwrap internally, so the rule is relaxed for this module only.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod ui {
    slint::include_modules!();
}
use ui::{AppWindow, DetailGroupItem, DetailItem, DeviceItem, FixItem, Row, SoftwareItem};

struct UpdateState {
    release: Option<hwsight_core::app_update::Release>,
    install: Option<hwsight_core::app_update::Install>,
    restart: Option<PathBuf>,
}

fn main() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    // Slint only accepts this once a component exists.
    let language = hwsight_core::i18n::language();
    if language != "en"
        && let Err(e) = slint::select_bundled_translation(language)
    {
        eprintln!("hwsight: couldn't select the {language} translation: {e}");
    }
    let devices = Arc::new(Mutex::new(Vec::new()));
    let last_report = Arc::new(Mutex::new(None));
    let update_state = Arc::new(Mutex::new(UpdateState {
        release: None,
        install: None,
        restart: None,
    }));

    let weak = window.as_weak();
    let scan_devices = devices.clone();
    let scan_report = last_report.clone();
    window.on_refresh(move || start_scan(weak.clone(), scan_devices.clone(), scan_report.clone()));
    let filter_window = window.as_weak();
    let filter_devices = devices.clone();
    window.on_filter_changed(move || apply_filter(&filter_window, &filter_devices));
    let selection_window = window.as_weak();
    window.on_move_selection(move |step| {
        let Some(w) = selection_window.upgrade() else {
            return;
        };
        let model = w.get_rows();
        let rows: Vec<Row> = (0..model.row_count())
            .filter_map(|index| model.row_data(index))
            .collect();
        let selected = model::next_device(&rows, w.get_selected(), step);
        w.set_selected(selected);
        scroll_to_selected(&w, &rows, selected);
    });
    let config_dir = settings::config_dir();
    if let Some(dir) = &config_dir {
        window.set_detail_level(settings::load_from(dir).level().index());
    }
    window.on_level_changed(move |level| {
        if let Some(dir) = &config_dir {
            settings::save_to(
                dir,
                &settings::Settings {
                    detail_level: level,
                },
            );
        }
    });
    if let Ok(level) = std::env::var("HWSIGHT_LEVEL")
        && let Ok(level) = level.parse::<i32>()
    {
        // Developer aid with HWSIGHT_SNAPSHOT: show a level without touching the saved one.
        window.set_detail_level(level);
    }
    let report_state = last_report.clone();
    window.on_device_report(move |address| {
        let Ok(report) = report_state.lock() else {
            return SharedString::default();
        };
        let Some(report) = report.as_ref() else {
            return SharedString::default();
        };
        hwsight_core::report::markdown(
            report,
            (!address.is_empty()).then_some(address.as_str()),
            &hwsight_core::report::Privacy::system(),
        )
        .into()
    });
    start_scan(window.as_weak(), devices.clone(), last_report.clone());
    check_for_data_update(window.as_weak(), devices, last_report);
    check_for_app_update(window.as_weak(), update_state.clone());
    connect_update_actions(window.as_weak(), update_state);

    // Developer aid for reviews: HWSIGHT_SNAPSHOT=out.ppm saves a picture of the window
    // after the first scan and quits; HWSIGHT_SELECT=<n> selects device n first. This
    // works without taking focus or touching the screen.
    if let Some(path) = std::env::var_os("HWSIGHT_SNAPSHOT") {
        if let Some(n) = std::env::var("HWSIGHT_SELECT")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            window.set_selected(n);
        }

        snapshot_when_ready(window.as_weak(), path.into());
    }

    window.run()
}

fn connect_update_actions(window: slint::Weak<AppWindow>, state: Arc<Mutex<UpdateState>>) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let update_window = window.as_weak();
    let update_state = state.clone();
    window.on_update_now(move || {
        let Ok(state) = update_state.lock() else {
            return;
        };
        let (Some(release), Some(install)) = (state.release.clone(), state.install.clone()) else {
            return;
        };
        drop(state);
        let Some(w) = update_window.upgrade() else {
            return;
        };
        w.set_update_busy(true);
        w.set_update_can_update(false);
        let thread_window = update_window.clone();
        let thread_state = update_state.clone();
        std::thread::spawn(move || {
            let outcome = hwsight_core::app_update::install_update(
                &release,
                &install,
                env!("CARGO_PKG_VERSION"),
            );
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = thread_window.upgrade() else {
                    return;
                };
                let can_restart =
                    matches!(outcome, hwsight_core::app_update::Outcome::Updated { .. });
                let can_update = matches!(outcome, hwsight_core::app_update::Outcome::Cancelled);
                if let hwsight_core::app_update::Outcome::Updated { restart, .. } = &outcome
                    && let Ok(mut state) = thread_state.lock()
                {
                    state.restart = Some(restart.clone());
                }
                w.set_update_text(outcome.message().into());
                w.set_update_busy(false);
                w.set_update_can_update(can_update);
                w.set_update_can_restart(can_restart);
            });
        });
    });

    let restart_window = window.as_weak();
    window.on_restart_now(move || {
        let Ok(mut state) = state.lock() else {
            return;
        };
        let Some(path) = state.restart.take() else {
            return;
        };
        drop(state);
        if let Err(e) = std::process::Command::new(&path).spawn() {
            eprintln!("hwsight-gui: couldn't restart hwsight: {e}");
            return;
        }
        if let Some(window) = restart_window.upgrade() {
            let _ = window.hide();
        }
        let _ = slint::quit_event_loop();
    });
}

fn check_for_app_update(window: slint::Weak<AppWindow>, state: Arc<Mutex<UpdateState>>) {
    if std::env::var_os("HWSIGHT_NO_UPDATE").is_some()
        || std::env::var_os("HWSIGHT_SNAPSHOT").is_some()
    {
        return;
    }
    // Its own state file: sharing the data updater's would let one daily check
    // silently cancel the other.
    let Some(state_file) = hwsight_core::app_update::default_state_file() else {
        return;
    };
    if !hwsight_core::update::check_due(
        &state_file,
        SystemTime::now(),
        Duration::from_secs(24 * 60 * 60),
    ) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    std::thread::spawn(move || {
        // Runs package-manager queries, so off the window's thread.
        let install = hwsight_core::app_update::detect_install(&exe, std::env::var_os("APPIMAGE"));
        let result = hwsight_core::app_update::latest_release();
        let _ = hwsight_core::update::mark_checked(&state_file);
        let Ok(Some(release)) = result else {
            return;
        };
        match app_updates::banner_for(&install, &release.version, env!("CARGO_PKG_VERSION")) {
            app_updates::Banner::Hidden => {}
            app_updates::Banner::AutoInstall => {
                let outcome = hwsight_core::app_update::install_update(
                    &release,
                    &install,
                    env!("CARGO_PKG_VERSION"),
                );
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = window.upgrade() else {
                        return;
                    };
                    if let hwsight_core::app_update::Outcome::Updated { restart, .. } = &outcome
                        && let Ok(mut state) = state.lock()
                    {
                        state.restart = Some(restart.clone());
                    }
                    w.set_update_text(outcome.message().into());
                    w.set_update_can_restart(matches!(
                        outcome,
                        hwsight_core::app_update::Outcome::Updated { .. }
                    ));
                });
            }
            app_updates::Banner::Offer { text } => {
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = window.upgrade() else {
                        return;
                    };
                    if let Ok(mut state) = state.lock() {
                        state.release = Some(release);
                        state.install = Some(install);
                    }
                    w.set_update_text(text.into());
                    w.set_update_can_update(true);
                });
            }
            app_updates::Banner::Info { text } => {
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = window.upgrade() else {
                        return;
                    };
                    if let Ok(mut state) = state.lock() {
                        state.release = Some(release);
                        state.install = Some(install);
                    }
                    w.set_update_text(text.into());
                    w.set_update_can_update(false);
                });
            }
        }
    });
}

fn scroll_to_selected(window: &AppWindow, rows: &[Row], selected: i32) {
    let Some(index) = rows
        .iter()
        .position(|row| !row.is_header && row.device == selected)
    else {
        return;
    };
    // Slint scroll positions are negative when scrolled down: content-y = -offset.
    let top: i32 = rows
        .iter()
        .take(index)
        .map(|row| if row.is_header { 40 } else { 52 })
        .sum();
    let row_height = 52;
    let offset = -window.get_list_viewport_y() as i32;
    let visible = window.get_list_height() as i32;
    let next = if top < offset {
        top
    } else if top + row_height > offset + visible {
        top + row_height - visible
    } else {
        offset
    };
    window.set_list_viewport_y(-(next.max(0) as f32));
}

/// At most once a day, quietly download newer device data in the background. If
/// anything new was installed, scan again so it's used straight away. Never blocks the
/// window and never asks anything; problems just mean we try again tomorrow.
/// HWSIGHT_NO_UPDATE=1 turns it off (tests, review snapshots).
fn check_for_data_update(
    window: slint::Weak<AppWindow>,
    devices: Arc<Mutex<Vec<hwsight_core::Device>>>,
    last_report: Arc<Mutex<Option<hwsight_core::ScanReport>>>,
) {
    if std::env::var_os("HWSIGHT_NO_UPDATE").is_some()
        || std::env::var_os("HWSIGHT_SNAPSHOT").is_some()
    {
        return;
    }
    let Some(state) = hwsight_core::update::default_state_file() else {
        return;
    };
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    if !hwsight_core::update::check_due(&state, std::time::SystemTime::now(), day) {
        return;
    }
    std::thread::spawn(move || {
        let dirs = hwsight_core::DataDirs::standard();
        let Some(dest) = dirs.download.clone() else {
            return;
        };
        let fetch = hwsight_core::update::HttpFetch {
            base_url: hwsight_core::update::DEFAULT_SOURCE.to_owned(),
        };
        let current = hwsight_core::update::current_versions(&dirs);
        let result = hwsight_core::update::update(
            &fetch,
            hwsight_core::update::TRUSTED_KEYS,
            &dest,
            &current,
        );
        // Record the attempt even if it failed (e.g. offline), so a broken connection
        // doesn't mean a download attempt on every start.
        let _ = hwsight_core::update::mark_checked(&state);
        if let Ok(outcome) = result
            && !outcome.installed.is_empty()
        {
            let _ = slint::invoke_from_event_loop(move || start_scan(window, devices, last_report));
        }
    });
}

fn snapshot_when_ready(window: slint::Weak<AppWindow>, path: std::path::PathBuf) {
    let timer = Box::leak(Box::new(slint::Timer::default()));
    let keys_sent = std::cell::Cell::new(false);
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(300),
        move || {
            let Some(w) = window.upgrade() else { return };
            if w.get_scanning() {
                return;
            }
            // HWSIGHT_KEYS=down,down,up presses those keys (once), then the picture is
            // taken on the next tick so it shows the result.
            if !keys_sent.replace(true) && std::env::var_os("HWSIGHT_KEYS").is_some() {
                for key in std::env::var("HWSIGHT_KEYS").unwrap_or_default().split(',') {
                    let k = match key.trim() {
                        "down" => slint::platform::Key::DownArrow,
                        "up" => slint::platform::Key::UpArrow,
                        _ => continue,
                    };
                    let text: slint::SharedString = k.into();
                    let _ = w.window().dispatch_event_with_result(
                        slint::platform::WindowEvent::KeyPressed { text: text.clone() },
                    );
                    let _ = w.window().dispatch_event_with_result(
                        slint::platform::WindowEvent::KeyReleased { text },
                    );
                }
                return;
            }
            let result = w
                .window()
                .take_snapshot()
                .map_err(|e| e.to_string())
                .and_then(|img| write_ppm(&path, img.width(), img.height(), img.as_bytes()));
            if let Err(e) = result {
                eprintln!("hwsight-gui: snapshot failed: {e}");
            }
            let _ = slint::quit_event_loop();
        },
    );
}

/// A plain PPM image (no extra dependency); convert with any image tool.
fn write_ppm(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
    out.extend(
        rgba.as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]]),
    );
    std::fs::write(path, out).map_err(|e| e.to_string())
}

/// Rebuild the list rows from the last scan using the current search and toggle.
fn apply_filter(window: &slint::Weak<AppWindow>, devices: &Arc<Mutex<Vec<hwsight_core::Device>>>) {
    let Some(w) = window.upgrade() else { return };
    let Ok(devices) = devices.lock() else { return };
    let rows = model::filter_rows(&devices, w.get_query().as_str(), w.get_only_problems());
    w.set_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
}

/// Scan on a background thread so the window stays responsive, then show the result.
fn start_scan(
    window: slint::Weak<AppWindow>,
    devices: Arc<Mutex<Vec<hwsight_core::Device>>>,
    last_report: Arc<Mutex<Option<hwsight_core::ScanReport>>>,
) {
    if let Some(w) = window.upgrade() {
        if w.get_scanning() {
            return;
        }
        w.set_scanning(true);
    }
    std::thread::spawn(move || {
        let report = hwsight_core::scan();
        // Slint models (like each device's list of fix steps) can't cross threads, so
        // only the plain report is sent; it's turned into window data on the UI thread.
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = window.upgrade() else { return };
            let view = model::view(&report);
            let scanned_devices = report.devices.clone();
            if let Ok(mut stored_report) = last_report.lock() {
                *stored_report = Some(report);
            }
            let device_count = view.devices.len() as i32;
            if let Ok(mut stored_devices) = devices.lock() {
                *stored_devices = scanned_devices;
            }
            w.set_devices(ModelRc::from(Rc::new(VecModel::from(view.devices))));
            w.set_rows(ModelRc::from(Rc::new(VecModel::from(view.rows))));
            w.set_summary(view.summary.into());
            w.set_problem_count(view.problem_count);
            let notes: Vec<SharedString> = view.notes.into_iter().map(Into::into).collect();
            w.set_notes(ModelRc::from(Rc::new(VecModel::from(notes))));
            if w.get_selected() >= device_count {
                w.set_selected(-1);
            }
            apply_filter(&window, &devices);
            w.set_scanning(false);
        });
    });
}
