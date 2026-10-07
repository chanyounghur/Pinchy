//! The bottom overlay panel.
//!
//! The window is created once, kept visible and transparent, and never hidden:
//! hiding a window suspends WebKit, which breaks the slide animation. "Closed"
//! means the webview has slid the panel out and the window ignores clicks.

use crate::platform::{self, WorkArea};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewWindow};

pub const WINDOW: &str = "main";
pub const PANEL_HEIGHT: f64 = 340.0;
pub const SHOWN_EVENT: &str = "panel-shown";
/// Asks the webview to play its slide-out animation and then call `hide_panel`.
pub const HIDE_EVENT: &str = "panel-hide";

#[derive(Default)]
pub struct PanelState {
    /// pid of the app that was in front right before the panel opened.
    pub target: Mutex<Option<i32>>,
    pub open: Mutex<bool>,
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(WINDOW)
}

/// Called once at startup: show the (transparent, click-through) window.
pub fn init(app: &AppHandle) {
    let Some(win) = window(app) else { return };
    if let Ok(ptr) = win.ns_window() {
        platform::configure_overlay_window(ptr);
    }
    let _ = win.set_ignore_cursor_events(true);
    let _ = win.show();
}

fn cursor_work_area(win: &WebviewWindow) -> Option<WorkArea> {
    if let Some(area) = platform::cursor_screen_work_area() {
        return Some(area);
    }
    let m = win
        .cursor_position()
        .ok()
        .and_then(|p| win.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| win.primary_monitor().ok().flatten())?;
    let scale = m.scale_factor();
    let area = m.work_area();
    Some(WorkArea {
        x: area.position.x as f64 / scale,
        y: area.position.y as f64 / scale,
        w: area.size.width as f64 / scale,
        h: area.size.height as f64 / scale,
    })
}

pub fn show(app: &AppHandle) {
    let Some(win) = window(app) else { return };
    let state = app.state::<PanelState>();

    if let Some((pid, _)) = platform::frontmost_app() {
        *state.target.lock().unwrap() = Some(pid);
    }
    *state.open.lock().unwrap() = true;

    // Dock the panel to the bottom of the screen the cursor is on.
    if let Some(a) = cursor_work_area(&win) {
        let _ = win.set_size(LogicalSize::new(a.w, PANEL_HEIGHT));
        let _ = win.set_position(LogicalPosition::new(a.x, a.y + a.h - PANEL_HEIGHT));
    }

    let _ = win.set_ignore_cursor_events(false);
    let _ = win.show();
    let _ = win.set_focus();
    platform::activate_self();
    let _ = app.emit(SHOWN_EVENT, ());
}

/// Immediate "hide": make the window click-through and give focus back.
/// The webview calls this after its slide-out animation.
pub fn hide(app: &AppHandle) {
    let state = app.state::<PanelState>();
    *state.open.lock().unwrap() = false;
    if let Some(win) = window(app) {
        let _ = win.set_ignore_cursor_events(true);
    }
    let target = *state.target.lock().unwrap();
    if let Some(pid) = target {
        platform::activate_app(pid);
    }
}

/// Animated hide, driven by the webview.
pub fn request_hide(app: &AppHandle) {
    let _ = app.emit(HIDE_EVENT, ());
}

pub fn toggle(app: &AppHandle) {
    let open = *app.state::<PanelState>().open.lock().unwrap();
    if open {
        request_hide(app);
    } else {
        show(app);
    }
}
