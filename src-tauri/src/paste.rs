//! Put an item back on the clipboard and paste it into the previously active app.

use crate::db::Item;
use crate::panel::{self, PanelState};
use crate::platform;
use clipboard_rs::common::RustImage;
use clipboard_rs::{Clipboard, ClipboardContext, RustImageData};
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::time::Duration;
use tauri::{AppHandle, Manager};

pub fn copy_to_clipboard(item: &Item) -> Result<(), String> {
    let ctx = ClipboardContext::new().map_err(|e| e.to_string())?;
    match item.kind.as_str() {
        "image" => {
            let img = RustImageData::from_path(&item.content).map_err(|e| e.to_string())?;
            ctx.set_image(img).map_err(|e| e.to_string())
        }
        "files" => {
            let files: Vec<String> =
                serde_json::from_str(&item.content).map_err(|e| e.to_string())?;
            ctx.set_files(files).map_err(|e| e.to_string())
        }
        _ => ctx.set_text(item.content.clone()).map_err(|e| e.to_string()),
    }
}

/// Hides the panel, re-activates the target app, and sends the paste shortcut.
pub fn paste(app: &AppHandle, item: &Item) -> Result<(), String> {
    copy_to_clipboard(item)?;
    let target = *app.state::<PanelState>().target.lock().unwrap();
    panel::hide(app);

    std::thread::spawn(move || {
        if let Some(pid) = target {
            platform::activate_app(pid);
        }
        std::thread::sleep(Duration::from_millis(120));
        if let Err(e) = send_paste_shortcut() {
            eprintln!("[pastel] paste shortcut failed: {e}");
        }
    });
    Ok(())
}

fn send_paste_shortcut() -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    let modifier = Key::Meta;
    #[cfg(not(target_os = "macos"))]
    let modifier = Key::Control;

    enigo.key(modifier, Direction::Press).map_err(|e| e.to_string())?;
    enigo.key(Key::Unicode('v'), Direction::Click).map_err(|e| e.to_string())?;
    enigo.key(modifier, Direction::Release).map_err(|e| e.to_string())?;
    Ok(())
}
