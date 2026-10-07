//! The bottom overlay panel.
//!
//! The whole window is animated natively (the way Paste does it): its bottom
//! edge stays pinned to the screen bottom while its height grows from ~0 to
//! `PANEL_HEIGHT`. The web content has a fixed height, so a shorter window
//! simply clips it — the panel appears to rise from the screen edge and never
//! strays onto a monitor placed below. Once closed, the window is ordered out.

use crate::platform::{self, Rect};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

pub const WINDOW: &str = "main";
/// Logical pixels; must match `.panel { height }` in App.css.
pub const PANEL_HEIGHT: f64 = 340.0;
/// Horizontal inset from the screen edges, logical pixels.
const INSET: f64 = 12.0;
const SLIDE_SECONDS: f64 = 0.22;
pub const SHOWN_EVENT: &str = "panel-shown";

#[derive(Default)]
pub struct PanelState {
    /// The app that was in front right before the panel opened.
    pub target: Mutex<Option<i64>>,
    pub open: Mutex<bool>,
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(WINDOW)
}

fn frames(win: &WebviewWindow, screen: Rect) -> (Rect, Rect) {
    let scale = platform::frame_scale(win);
    platform::panel_frames(screen, PANEL_HEIGHT * scale, INSET * scale)
}

/// Called once at startup (main thread): style the (still hidden) window.
pub fn init(app: &AppHandle) {
    let Some(win) = window(app) else { return };
    platform::configure_overlay_window(&win);
    if let (Some(h), Some(screen)) = (platform::handle(&win), platform::cursor_screen_visible_frame()) {
        platform::set_window_frame(h, frames(&win, screen).1);
    }
}

pub fn show(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let Some(win) = window(&app) else { return };
        let state = app.state::<PanelState>();
        if let Some(info) = platform::frontmost_app() {
            *state.target.lock().unwrap() = Some(info.app_ref);
        }
        *state.open.lock().unwrap() = true;

        if let (Some(h), Some(screen)) = (platform::handle(&win), platform::cursor_screen_visible_frame()) {
            let (open, closed) = frames(&win, screen);
            platform::set_window_frame(h, closed);
            let _ = win.show();
            platform::animate_window_frame(h, open, SLIDE_SECONDS);
        } else {
            let _ = win.show();
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
        if let Some(h) = window(&app).and_then(|w| platform::handle(&w)) {
            if let Some(current) = platform::window_frame(h) {
                platform::animate_window_frame(h, platform::collapse_frame(current), SLIDE_SECONDS);
            }
        }
        // Only give focus back if we still have it (not when the user clicked elsewhere).
        let target = *state.target.lock().unwrap();
        if let (Some(app_ref), true) = (target, platform::is_self_active()) {
            platform::activate_app(app_ref);
        }
        // Order the window out once the animation has finished (unless reopened).
        let app2 = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs_f64(SLIDE_SECONDS + 0.05));
            let _ = app2.clone().run_on_main_thread(move || {
                let still_closed = !*app2.state::<PanelState>().open.lock().unwrap();
                if still_closed {
                    if let Some(h) = window(&app2).and_then(|w| platform::handle(&w)) {
                        platform::order_out(h);
                    }
                }
            });
        });
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
