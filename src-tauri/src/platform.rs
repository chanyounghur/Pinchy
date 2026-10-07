//! OS-specific helpers behind one small interface. `Handle` is the native
//! window handle (NSWindow pointer on macOS, HWND on Windows) as an integer.
//! AppKit calls must run on the main thread; Win32 calls here are thread-safe.



pub type Handle = isize;

pub struct AppInfo {
    /// What `activate_app` needs: pid on macOS, HWND on Windows.
    pub app_ref: i64,
    pub name: String,
    pub bundle_id: Option<String>,
    pub bundle_path: Option<String>,
}

/// A rect in native window coordinates: AppKit points with a bottom-left
/// origin on macOS, physical pixels with a top-left origin on Windows.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Open/closed frames of a panel of `height` docked to the bottom of `screen`,
/// inset horizontally. Windows keeps a full-sized hidden window because a
/// one-pixel outer height can produce a negative client height in Tauri.
pub fn panel_frames(screen: Rect, height: f64, inset: f64) -> (Rect, Rect) {
    let open = Rect {
        x: screen.x + inset,
        y: if cfg!(target_os = "macos") { screen.y } else { screen.y + screen.h - height },
        w: screen.w - inset * 2.0,
        h: height,
    };
    (open, collapse_frame(open))
}

/// Collapse on macOS; preserve a valid client area on Windows.
pub fn collapse_frame(r: Rect) -> Rect {
    if cfg!(target_os = "macos") {
        Rect { h: 1.0, ..r }
    } else {
        r
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::{AppInfo, Handle, Rect};
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{
        NSAnimatablePropertyContainer, NSAnimationContext, NSApplication,
        NSApplicationActivationOptions, NSBitmapImageFileType, NSBitmapImageRep, NSEvent,
        NSRunningApplication, NSScreen, NSWindow, NSWindowCollectionBehavior, NSWorkspace,
    };
    use objc2_foundation::{NSBundle, NSDictionary, NSPoint, NSRect, NSSize, NSString};
    use tauri::WebviewWindow;

    pub fn handle(win: &WebviewWindow) -> Option<Handle> {
        win.ns_window().ok().map(|p| p as Handle)
    }

    /// Native units per logical pixel (AppKit frames are in points).
    pub fn frame_scale(_win: &WebviewWindow) -> f64 {
        1.0
    }

    fn ns_window(h: Handle) -> Option<&'static NSWindow> {
        if h == 0 {
            None
        } else {
            Some(unsafe { &*(h as *const NSWindow) })
        }
    }

    pub fn frontmost_app() -> Option<AppInfo> {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        Some(AppInfo {
            app_ref: app.processIdentifier() as i64,
            name: app.localizedName().map(|s| s.to_string()).unwrap_or_default(),
            bundle_id: app.bundleIdentifier().map(|s| s.to_string()),
            bundle_path: app.bundleURL().and_then(|u| u.path()).map(|s| s.to_string()),
        })
    }

    pub fn activate_app(app_ref: i64) {
        if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(app_ref as i32) {
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

    pub fn window_frame(h: Handle) -> Option<Rect> {
        let f = ns_window(h)?.frame();
        Some(Rect { x: f.origin.x, y: f.origin.y, w: f.size.width, h: f.size.height })
    }

    pub fn set_window_frame(h: Handle, r: Rect) {
        if let Some(win) = ns_window(h) {
            win.setFrame_display(ns_rect(r), true);
        }
    }

    pub fn animate_window_frame(h: Handle, r: Rect, seconds: f64) {
        let Some(win) = ns_window(h) else { return };
        NSAnimationContext::beginGrouping();
        NSAnimationContext::currentContext().setDuration(seconds);
        win.animator().setFrame_display(ns_rect(r), true);
        NSAnimationContext::endGrouping();
    }

    pub fn order_out(h: Handle) {
        if let Some(win) = ns_window(h) {
            win.orderOut(None);
        }
    }

    /// Keep the overlay out of Mission Control / window cycling and let it
    /// float over full-screen apps.
    pub fn configure_overlay_window(win: &WebviewWindow) {
        use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
        let _ = apply_vibrancy(win, NSVisualEffectMaterial::Popover, Some(NSVisualEffectState::Active), Some(18.0));
        let Some(ns) = handle(win).and_then(ns_window) else { return };
        let behavior = ns.collectionBehavior()
            | NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Transient
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::FullScreenAuxiliary;
        ns.setCollectionBehavior(behavior);
    }

    /// Bundle path of an app given its display name (for rows saved before
    /// icons existed). Prefers a running instance, then Launch Services.
    pub fn app_path_for_name(name: &str) -> Option<String> {
        let ws = NSWorkspace::sharedWorkspace();
        let running = ws.runningApplications();
        let hit = running.iter().find(|a| {
            a.localizedName().map(|n| n.to_string() == name).unwrap_or(false)
        });
        if let Some(path) = hit.and_then(|a| a.bundleURL()).and_then(|u| u.path()) {
            return Some(path.to_string());
        }
        #[allow(deprecated)]
        ws.fullPathForApplication(&NSString::from_str(name)).map(|s| s.to_string())
    }

    pub fn bundle_id_for_path(path: &str) -> Option<String> {
        NSBundle::bundleWithPath(&NSString::from_str(path))?
            .bundleIdentifier()
            .map(|s| s.to_string())
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

#[cfg(windows)]
mod win {
    use super::{AppInfo, Handle, Rect};
    use tauri::WebviewWindow;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, HWND, POINT, RECT};
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    use windows::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, GetDIBits, GetMonitorInfoW, MonitorFromPoint, ReleaseDC, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Shell::SHDefExtractIconW;
    use windows::Win32::UI::WindowsAndMessaging::{
        DestroyIcon, GetCursorPos, GetForegroundWindow, GetIconInfo, GetWindowLongPtrW,
        GetWindowRect, GetWindowThreadProcessId, SetForegroundWindow, SetWindowLongPtrW,
        SetWindowPos, GWL_EXSTYLE, HICON, ICONINFO, SWP_NOACTIVATE, SWP_NOZORDER,
        WS_EX_TOOLWINDOW,
    };

    fn hwnd(h: Handle) -> HWND {
        HWND(h as *mut core::ffi::c_void)
    }

    pub fn handle(win: &WebviewWindow) -> Option<Handle> {
        win.hwnd().ok().map(|h| h.0 as Handle)
    }

    /// Win32 frames are physical pixels.
    pub fn frame_scale(win: &WebviewWindow) -> f64 {
        win.scale_factor().unwrap_or(1.0)
    }

    fn window_pid(h: HWND) -> u32 {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
        pid
    }

    fn process_path(pid: u32) -> Option<String> {
        unsafe {
            let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(proc, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
            let _ = CloseHandle(proc);
            ok.ok()?;
            Some(String::from_utf16_lossy(&buf[..len as usize]))
        }
    }

    fn display_name(path: &str) -> String {
        std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn frontmost_app() -> Option<AppInfo> {
        let fg = unsafe { GetForegroundWindow() };
        if fg.0.is_null() {
            return None;
        }
        let path = process_path(window_pid(fg));
        Some(AppInfo {
            app_ref: fg.0 as isize as i64,
            name: path.as_deref().map(display_name).unwrap_or_default(),
            bundle_id: path.as_deref().map(|p| display_name(p).to_lowercase()),
            bundle_path: path,
        })
    }

    pub fn activate_app(app_ref: i64) {
        unsafe {
            let _ = SetForegroundWindow(hwnd(app_ref as isize));
        }
    }

    /// Tauri's `set_focus` already calls SetForegroundWindow.
    pub fn activate_self() {}

    pub fn is_self_active() -> bool {
        unsafe { window_pid(GetForegroundWindow()) == GetCurrentProcessId() }
    }

    /// Work area (minus taskbar) of the monitor under the cursor, physical px.
    pub fn cursor_screen_visible_frame() -> Option<Rect> {
        unsafe {
            let mut p = POINT::default();
            GetCursorPos(&mut p).ok()?;
            let mon = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if !GetMonitorInfoW(mon, &mut info).as_bool() {
                return None;
            }
            let r = info.rcWork;
            Some(Rect {
                x: r.left as f64,
                y: r.top as f64,
                w: (r.right - r.left) as f64,
                h: (r.bottom - r.top) as f64,
            })
        }
    }

    pub fn window_frame(h: Handle) -> Option<Rect> {
        unsafe {
            let mut r = RECT::default();
            GetWindowRect(hwnd(h), &mut r).ok()?;
            Some(Rect {
                x: r.left as f64,
                y: r.top as f64,
                w: (r.right - r.left) as f64,
                h: (r.bottom - r.top) as f64,
            })
        }
    }

    pub fn set_window_frame(h: Handle, r: Rect) {
        unsafe {
            let _ = SetWindowPos(
                hwnd(h),
                None,
                r.x.round() as i32,
                r.y.round() as i32,
                r.w.round().max(1.0) as i32,
                r.h.round().max(1.0) as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    /// Keep resizing on the caller's UI thread, without collapsing the client area.
    pub fn animate_window_frame(h: Handle, to: Rect, _seconds: f64) {
        set_window_frame(h, to);
    }

    /// Acrylic backdrop, rounded corners, and no Alt-Tab entry.
    pub fn configure_overlay_window(win: &WebviewWindow) {
        let _ = window_vibrancy::apply_acrylic(win, Some((255, 255, 255, 160)));
        let Some(h) = handle(win) else { return };
        unsafe {
            let hw = hwnd(h);
            let ex = GetWindowLongPtrW(hw, GWL_EXSTYLE);
            SetWindowLongPtrW(hw, GWL_EXSTYLE, ex | WS_EX_TOOLWINDOW.0 as isize);
            let pref = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hw,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const _ as *const core::ffi::c_void,
                std::mem::size_of_val(&pref) as u32,
            );
        }
    }

    pub fn app_path_for_name(_name: &str) -> Option<String> {
        None
    }

    pub fn bundle_id_for_path(path: &str) -> Option<String> {
        Some(display_name(path).to_lowercase())
    }

    /// PNG bytes of an executable's icon (128px when available).
    pub fn app_icon_bytes(exe_path: &str) -> Option<Vec<u8>> {
        let wide: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut icon = HICON::default();
        unsafe {
            let hr = SHDefExtractIconW(windows::core::PCWSTR(wide.as_ptr()), 0, 0, Some(&mut icon), None, 128);
            if hr.is_err() || icon.0.is_null() {
                return None;
            }
            let png = icon_to_png(icon);
            let _ = DestroyIcon(icon);
            png
        }
    }

    unsafe fn icon_to_png(icon: HICON) -> Option<Vec<u8>> {
        let mut info = ICONINFO::default();
        GetIconInfo(icon, &mut info).ok()?;
        let hdc = GetDC(None);
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        // First call fills in the dimensions.
        GetDIBits(hdc, info.hbmColor, 0, 0, None, &mut bmi, DIB_RGB_COLORS);
        let w = bmi.bmiHeader.biWidth;
        let h = bmi.bmiHeader.biHeight.abs();
        let mut result = None;
        if w > 0 && h > 0 {
            bmi.bmiHeader.biHeight = -h; // top-down
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB.0;
            bmi.bmiHeader.biPlanes = 1;
            let mut buf = vec![0u8; (w * h * 4) as usize];
            let lines = GetDIBits(hdc, info.hbmColor, 0, h as u32, Some(buf.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
            if lines > 0 {
                let has_alpha = buf.chunks_exact(4).any(|p| p[3] != 0);
                for p in buf.chunks_exact_mut(4) {
                    p.swap(0, 2); // BGRA -> RGBA
                    if !has_alpha {
                        p[3] = 255;
                    }
                }
                if let Some(img) = image::RgbaImage::from_raw(w as u32, h as u32, buf) {
                    let mut out = std::io::Cursor::new(Vec::new());
                    if image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).is_ok() {
                        result = Some(out.into_inner());
                    }
                }
            }
        }
        let _ = DeleteObject(info.hbmColor.into());
        let _ = DeleteObject(info.hbmMask.into());
        ReleaseDC(None, hdc);
        result
    }
}

#[cfg(windows)]
pub use win::*;

#[cfg(not(any(target_os = "macos", windows)))]
mod other {
    use super::{AppInfo, Handle, Rect};
    use tauri::WebviewWindow;
    pub fn handle(_win: &WebviewWindow) -> Option<Handle> { None }
    pub fn frame_scale(win: &WebviewWindow) -> f64 { win.scale_factor().unwrap_or(1.0) }
    pub fn frontmost_app() -> Option<AppInfo> { None }
    pub fn activate_app(_app_ref: i64) {}
    pub fn activate_self() {}
    pub fn is_self_active() -> bool { true }
    pub fn cursor_screen_visible_frame() -> Option<Rect> { None }
    pub fn window_frame(_h: Handle) -> Option<Rect> { None }
    pub fn set_window_frame(_h: Handle, _r: Rect) {}
    pub fn animate_window_frame(_h: Handle, _r: Rect, _seconds: f64) {}
    pub fn order_out(_h: Handle) {}
    pub fn configure_overlay_window(_win: &WebviewWindow) {}
    pub fn app_path_for_name(_name: &str) -> Option<String> { None }
    pub fn bundle_id_for_path(_path: &str) -> Option<String> { None }
    pub fn app_icon_bytes(_path: &str) -> Option<Vec<u8>> { None }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub use other::*;
