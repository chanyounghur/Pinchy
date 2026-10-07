//! The bottom overlay panel.
//!
//! The window is created once and never hidden: "closed" means it is parked
//! just below the screen edge, and opening slides the whole window up with an
//! AppKit animation (the way Paste does it). Hiding the window would suspend
//! WebKit and break any in-page animation.

use crate::platform::{self, Rect};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

pub const WINDOW: &str = "main";
pub const PANEL_HEIGHT: f64 = 340.0;
/// Horizontal inset from the screen edges.
const INSET: f64 = 12.0;
const SLIDE_SECONDS: f64 = 0.22;
pub const SHOWN_EVENT: &str = "panel-shown";

#[derive(Default)]
pub struct PanelState {
    /// pid of the app that was in front right before the panel opened.
    pub target: Mutex<Option<i32>>,
    pub open: Mutex<bool>,
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(WINDOW)
}

fn frames(screen: Rect) -> (Rect, Rect) {
    let open = Rect {
        x: screen.x + INSET,
        y: screen.y,
        w: screen.w - INSET * 2.0,
        h: PANEL_HEIGHT,
    };
    let closed = Rect { y: screen.y - PANEL_HEIGHT - 24.0, ..open };
    (open, closed)
}

/// Called once at startup (main thread): style the window and park it offscreen.
pub fn init(app: &AppHandle) {
    let Some(win) = window(app) else { return };
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
        let _ = apply_vibrancy(&win, NSVisualEffectMaterial::Popover, Some(NSVisualEffectState::Active), Some(18.0));
    }
    if let Ok(ptr) = win.ns_window() {
        platform::configure_overlay_window(ptr);
        if let Some(screen) = platform::cursor_screen_visible_frame() {
            platform::set_window_frame(ptr, frames(screen).1);
        }
    }
    let _ = win.show();
}

pub fn show(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let Some(win) = window(&app) else { return };
        let state = app.state::<PanelState>();
        if let Some(info) = platform::frontmost_app() {
            *state.target.lock().unwrap() = Some(info.pid);
        }
        *state.open.lock().unwrap() = true;

        if let (Ok(ptr), Some(screen)) = (win.ns_window(), platform::cursor_screen_visible_frame()) {
            let (open, closed) = frames(screen);
            platform::set_window_frame(ptr, closed);
            platform::animate_window_frame(ptr, open, SLIDE_SECONDS);
        }
        let _ = win.set_focus();
        platform::activate_self();
        let _ = app.emit(SHOWN_EVENT, ());
    });
}

/// Slide the panel away and hand focus back to the previous app.
pub fn hide(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let state = app.state::<PanelState>();
        let was_open = std::mem::replace(&mut *state.open.lock().unwrap(), false);
        if !was_open {
            return;
        }
        if let Some(win) = window(&app) {
            if let (Ok(ptr), Some(screen)) = (win.ns_window(), platform::cursor_screen_visible_frame()) {
                platform::animate_window_frame(ptr, frames(screen).1, SLIDE_SECONDS);
            }
        }
        // Only give focus back if we still have it (not when the user clicked elsewhere).
        let target = *state.target.lock().unwrap();
        if let (Some(pid), true) = (target, platform::is_self_active()) {
            platform::activate_app(pid);
        }
    });
}

pub fn toggle(app: &AppHandle) {
    let open = *app.state::<PanelState>().open.lock().unwrap();
    if open {
        hide(app);
    } else {
        show(app);
    }
}
