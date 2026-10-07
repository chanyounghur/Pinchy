//! OS-specific helpers. Everything AppKit-related must run on the main thread.

pub struct AppInfo {
    pub pid: i32,
    pub name: String,
    pub bundle_id: Option<String>,
    pub bundle_path: Option<String>,
}

/// A screen rect in the platform's native window coordinates
/// (AppKit: points, origin bottom-left of the primary screen).
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[cfg(target_os = "macos")]
mod mac {
    use super::{AppInfo, Rect};
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{
        NSAnimatablePropertyContainer, NSAnimationContext, NSApplication,
        NSApplicationActivationOptions, NSBitmapImageFileType, NSBitmapImageRep, NSEvent,
        NSRunningApplication, NSScreen, NSWindow, NSWindowCollectionBehavior, NSWorkspace,
    };
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

    fn ns_window(ptr: *mut std::ffi::c_void) -> Option<&'static NSWindow> {
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { &*(ptr as *const NSWindow) })
        }
    }

    pub fn frontmost_app() -> Option<AppInfo> {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        Some(AppInfo {
            pid: app.processIdentifier(),
            name: app.localizedName().map(|s| s.to_string()).unwrap_or_default(),
            bundle_id: app.bundleIdentifier().map(|s| s.to_string()),
            bundle_path: app.bundleURL().and_then(|u| u.path()).map(|s| s.to_string()),
        })
    }

    pub fn activate_app(pid: i32) {
        if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            app.activateWithOptions(NSApplicationActivationOptions::empty());
        }
    }

    pub fn activate_self() {
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).activate();
        }
    }

    pub fn is_self_active() -> bool {
        MainThreadMarker::new()
            .map(|mtm| NSApplication::sharedApplication(mtm).isActive())
            .unwrap_or(false)
    }

    /// Visible frame (minus menu bar and Dock) of the screen under the cursor.
    pub fn cursor_screen_visible_frame() -> Option<Rect> {
        let mtm = MainThreadMarker::new()?;
        let p = NSEvent::mouseLocation();
        let screens = NSScreen::screens(mtm);
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
        Some(Rect { x: v.origin.x, y: v.origin.y, w: v.size.width, h: v.size.height })
    }

    fn ns_rect(r: Rect) -> NSRect {
        NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))
    }

    pub fn set_window_frame(ptr: *mut std::ffi::c_void, r: Rect) {
        if let Some(win) = ns_window(ptr) {
            win.setFrame_display(ns_rect(r), true);
        }
    }

    pub fn animate_window_frame(ptr: *mut std::ffi::c_void, r: Rect, seconds: f64) {
        let Some(win) = ns_window(ptr) else { return };
        NSAnimationContext::beginGrouping();
        NSAnimationContext::currentContext().setDuration(seconds);
        win.animator().setFrame_display(ns_rect(r), true);
        NSAnimationContext::endGrouping();
    }

    /// Keep the overlay out of Mission Control / window cycling and let it
    /// float over full-screen apps.
    pub fn configure_overlay_window(ptr: *mut std::ffi::c_void) {
        let Some(win) = ns_window(ptr) else { return };
        let behavior = win.collectionBehavior()
            | NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Transient
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::FullScreenAuxiliary;
        win.setCollectionBehavior(behavior);
    }

    /// PNG bytes of an app's icon, rendered at ~128px via CGImage.
    pub fn app_icon_bytes(bundle_path: &str) -> Option<Vec<u8>> {
        let ws = NSWorkspace::sharedWorkspace();
        let img = ws.iconForFile(&NSString::from_str(bundle_path));
        let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(128.0, 128.0));
        let cg = unsafe { img.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
        let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
        let props = NSDictionary::new();
        let data = unsafe { bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }?;
        Some(data.to_vec())
    }
}

#[cfg(target_os = "macos")]
pub use mac::*;

#[cfg(not(target_os = "macos"))]
mod other {
    use super::{AppInfo, Rect};
    pub fn frontmost_app() -> Option<AppInfo> { None }
    pub fn activate_app(_pid: i32) {}
    pub fn activate_self() {}
    pub fn is_self_active() -> bool { true }
    pub fn cursor_screen_visible_frame() -> Option<Rect> { None }
    pub fn set_window_frame(_ptr: *mut std::ffi::c_void, _r: Rect) {}
    pub fn animate_window_frame(_ptr: *mut std::ffi::c_void, _r: Rect, _seconds: f64) {}
    pub fn configure_overlay_window(_ptr: *mut std::ffi::c_void) {}
    pub fn app_icon_bytes(_bundle_path: &str) -> Option<Vec<u8>> { None }
}

#[cfg(not(target_os = "macos"))]
pub use other::*;
