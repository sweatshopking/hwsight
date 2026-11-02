//! hwsight graphical app.

mod model;

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

// Code generated from ui/*.slint. Our no-unwrap rule is for our code; the generator
// uses unwrap internally, so the rule is relaxed for this module only.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod ui {
    slint::include_modules!();
}
use ui::{AppWindow, DeviceItem, Row};

fn main() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;

    let weak = window.as_weak();
    window.on_refresh(move || start_scan(weak.clone()));
    start_scan(window.as_weak());

    window.run()
}

/// Scan on a background thread so the window stays responsive, then show the result.
fn start_scan(window: slint::Weak<AppWindow>) {
    if let Some(w) = window.upgrade() {
        if w.get_scanning() {
            return;
        }
        w.set_scanning(true);
    }
    std::thread::spawn(move || {
        let report = hwsight_core::scan();
        let view = model::view(&report);
        // Slint models must be built on the UI thread, so send plain data across.
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = window.upgrade() else { return };
            let device_count = view.devices.len() as i32;
            w.set_devices(ModelRc::from(Rc::new(VecModel::from(view.devices))));
            w.set_rows(ModelRc::from(Rc::new(VecModel::from(view.rows))));
            w.set_summary(view.summary.into());
            w.set_problem_count(view.problem_count);
            let notes: Vec<SharedString> = view.notes.into_iter().map(Into::into).collect();
            w.set_notes(ModelRc::from(Rc::new(VecModel::from(notes))));
            if w.get_selected() >= device_count {
                w.set_selected(-1);
            }
            w.set_scanning(false);
        });
    });
}
