//! hwsight graphical app.

mod model;

use std::{
    rc::Rc,
    sync::{Arc, Mutex},
};

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

// Code generated from ui/*.slint. Our no-unwrap rule is for our code; the generator
// uses unwrap internally, so the rule is relaxed for this module only.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod ui {
    slint::include_modules!();
}
use ui::{AppWindow, DeviceItem, FixItem, Row};

fn main() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    let devices = Arc::new(Mutex::new(Vec::new()));

    let weak = window.as_weak();
    let scan_devices = devices.clone();
    window.on_refresh(move || start_scan(weak.clone(), scan_devices.clone()));
    let filter_window = window.as_weak();
    let filter_devices = devices.clone();
    window.on_filter_changed(move || apply_filter(&filter_window, &filter_devices));
    start_scan(window.as_weak(), devices);

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

fn snapshot_when_ready(window: slint::Weak<AppWindow>, path: std::path::PathBuf) {
    let timer = Box::leak(Box::new(slint::Timer::default()));
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(300),
        move || {
            let Some(w) = window.upgrade() else { return };
            if w.get_scanning() {
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
fn start_scan(window: slint::Weak<AppWindow>, devices: Arc<Mutex<Vec<hwsight_core::Device>>>) {
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
            let scanned_devices = report.devices;
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
