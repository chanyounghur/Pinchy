//! The bottom overlay panel: show/hide/toggle and remembering which app to paste into.

use crate::platform;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewWindow};

pub const WINDOW: &str = "main";
pub const PANEL_HEIGHT: f64 = 340.0;
pub const SHOWN_EVENT: &str = "panel-shown";

/// pid of the app that was in front right before the panel opened.
#[derive(Default)]
pub struct Target(pub Mutex<Option<i32>>);

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(WINDOW)
}

pub fn show(app: &AppHandle) {
    let Some(win) = window(app) else { return };

    if let Some((pid, _)) = platform::frontmost_app() {
        *app.state::<Target>().0.lock().unwrap() = Some(pid);
    }

    // Dock the panel to the bottom of the monitor the cursor is on.
    let monitor = win
        .cursor_position()
        .ok()
        .and_then(|p| win.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| win.primary_monitor().ok().flatten());
    if let Some(m) = monitor {
        let scale = m.scale_factor();
        let area = m.work_area();
        let x = area.position.x as f64 / scale;
        let y = area.position.y as f64 / scale;
        let w = area.size.width as f64 / scale;
        let h = area.size.height as f64 / scale;
        let _ = win.set_size(LogicalSize::new(w, PANEL_HEIGHT));
        let _ = win.set_position(LogicalPosition::new(x, y + h - PANEL_HEIGHT));
    }

    #[cfg(target_os = "macos")]
    {
        // Undo a previous `AppHandle::hide` so the window can come back.
        let _ = tauri::AppHandle::show(app);
    }
    let _ = win.show();
    let _ = win.set_focus();
    let _ = app.emit(SHOWN_EVENT, ());
}

pub fn hide(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        // Hiding the whole app hands focus back to the previous app.
        let _ = tauri::AppHandle::hide(app);
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(win) = window(app) {
            let _ = win.hide();
        }
    }
}

pub fn toggle(app: &AppHandle) {
    let visible = window(app)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if visible {
        hide(app);
    } else {
        show(app);
    }
}
