//! Thin OS-specific helpers: which app is in front, and re-activating it.

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

#[cfg(not(target_os = "macos"))]
pub fn frontmost_app() -> Option<(i32, String)> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn activate_app(_pid: i32) {}
