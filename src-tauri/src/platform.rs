//! Thin OS-specific helpers: which app is in front, re-activating it, and
//! which screen the cursor is on.

/// Logical coordinates, top-left origin (what Tauri's `LogicalPosition` expects).
pub struct WorkArea {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[cfg(target_os = "macos")]
pub fn frontmost_app() -> Option<(i32, String)> {
    use objc2_app_kit::NSWorkspace;
    let ws = NSWorkspace::sharedWorkspace();
    let app = ws.frontmostApplication()?;
    let name = app
        .localizedName()
        .map(|s| s.to_string())
        .unwrap_or_default();
    Some((app.processIdentifier(), name))
}

#[cfg(target_os = "macos")]
pub fn activate_app(pid: i32) {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
        app.activateWithOptions(NSApplicationActivationOptions::empty());
    }
}

/// Brings this app to the front so the panel's webview gets keyboard focus.
#[cfg(target_os = "macos")]
pub fn activate_self() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).activate();
    }
}

/// Work area of the screen under the mouse cursor. tao's `cursor_position`
/// mixes physical and logical units, which picks the wrong screen on mixed-DPI
/// setups, so this asks AppKit directly.
#[cfg(target_os = "macos")]
pub fn cursor_screen_work_area() -> Option<WorkArea> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSEvent, NSScreen};
    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm);
    // AppKit's global coordinates have the primary screen at origin, y up.
    let primary_height = screens.firstObject()?.frame().size.height;
    let p = NSEvent::mouseLocation();
    let screen = screens
        .iter()
        .find(|s| {
            let f = s.frame();
            p.x >= f.origin.x
                && p.x < f.origin.x + f.size.width
                && p.y >= f.origin.y
                && p.y < f.origin.y + f.size.height
        })
        .or_else(|| NSScreen::mainScreen(mtm))?;
    let v = screen.visibleFrame();
    Some(WorkArea {
        x: v.origin.x,
        y: primary_height - (v.origin.y + v.size.height),
        w: v.size.width,
        h: v.size.height,
    })
}

#[cfg(not(target_os = "macos"))]
pub fn frontmost_app() -> Option<(i32, String)> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn activate_app(_pid: i32) {}

#[cfg(not(target_os = "macos"))]
pub fn activate_self() {}

#[cfg(not(target_os = "macos"))]
pub fn cursor_screen_work_area() -> Option<WorkArea> {
    None
}

/// Keep the always-present overlay window out of Mission Control / window
/// cycling and let it float over full-screen apps.
#[cfg(target_os = "macos")]
pub fn configure_overlay_window(ns_window: *mut std::ffi::c_void) {
    use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};
    if ns_window.is_null() {
        return;
    }
    let win: &NSWindow = unsafe { &*(ns_window as *const NSWindow) };
    let behavior = win.collectionBehavior()
        | NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::Transient
        | NSWindowCollectionBehavior::Stationary
        | NSWindowCollectionBehavior::IgnoresCycle
        | NSWindowCollectionBehavior::FullScreenAuxiliary;
    win.setCollectionBehavior(behavior);
}

#[cfg(not(target_os = "macos"))]
pub fn configure_overlay_window(_ns_window: *mut std::ffi::c_void) {}
