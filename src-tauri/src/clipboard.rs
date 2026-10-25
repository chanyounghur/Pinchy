//! Watches the system clipboard and stores every change in the DB.

use crate::appicon::{AppIconCache, AppLook};
use crate::db::{Db, NewItem};
use crate::platform;
use clipboard_rs::{
    Clipboard, ClipboardContext, ClipboardHandler, ClipboardWatcher, ClipboardWatcherContext,
    ContentFormat,
};
use clipboard_rs::common::RustImage;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

pub const CHANGED_EVENT: &str = "clipboard-changed";
pub const PREVIEW_LEN: usize = 300;
/// Pasteboard types password managers add so clipboard tools leave the copy
/// alone: macOS's shared convention and Windows' documented flag.
const CONCEALED_FORMATS: [&str; 2] = ["org.nspasteboard.ConcealedType", "ExcludeClipboardContentFromMonitorProcessing"];

struct Handler {
    app: AppHandle,
    ctx: ClipboardContext,
    images_dir: PathBuf,
    icons: AppIconCache,
}

struct Source {
    name: Option<String>,
    look: AppLook,
}


impl ClipboardHandler for Handler {
    fn on_clipboard_change(&mut self) {
        let app = self.app.clone();
        let db = app.state::<Db>();
        let _files = db.1.lock().unwrap();
        if let Some(item) = self.read_item() {
            match db.upsert(item) {
                Ok(_) => {
                    let _ = self.app.emit(CHANGED_EVENT, ());
                    // Pinchy's own paste only bumps an item it already
                    // synced; the bump still reaches the other devices.
                    crate::share::changed(&app);
                }
                Err(e) => eprintln!("[pinchy] db upsert failed: {e}"),
            }
        }
    }
}

impl Handler {
    fn source(&mut self) -> Source {
        let info = platform::frontmost_app();
        let look = match &info {
            Some(i) => match (&i.bundle_id, &i.bundle_path) {
                (Some(id), Some(path)) => self.icons.look(id, path),
                _ => AppLook { icon: None, color: None },
            },
            None => AppLook { icon: None, color: None },
        };
        Source { name: info.map(|i| i.name), look }
    }

    fn read_item(&mut self) -> Option<NewItem> {
        let source = self.source();
        let mut item = self.read_content()?;
        item.source_app = source.name;
        item.app_icon = source.look.icon;
        item.app_color = source.look.color;
        item.concealed = self.concealed();
        Some(item)
    }

    /// Whether the source app asked clipboard tools not to keep this copy.
    fn concealed(&self) -> bool {
        self.ctx
            .available_formats()
            .map(|formats| formats.iter().any(|f| CONCEALED_FORMATS.contains(&f.as_str())))
            .unwrap_or(false)
    }

    fn read_content(&self) -> Option<NewItem> {
        if self.ctx.has(ContentFormat::Files) {
            if let Ok(files) = self.ctx.get_files() {
                if !files.is_empty() {
                    return Some(files_item(files));
                }
            }
        }
        if self.ctx.has(ContentFormat::Image) {
            if let Ok(img) = self.ctx.get_image() {
                if !img.is_empty() {
                    return self.image_item(img);
                }
            }
        }
        if self.ctx.has(ContentFormat::Text) {
            if let Ok(text) = self.ctx.get_text() {
                if !text.trim().is_empty() {
                    return Some(text_item(text));
                }
            }
        }
        None
    }

    fn image_item(&self, img: clipboard_rs::RustImageData) -> Option<NewItem> {
        let png = img.to_png().ok()?;
        let (w, h) = img.get_size();
        png_item(&self.images_dir, png.get_bytes(), w, h)
    }
}

/// Stores PNG bytes in the image cache, named by content hash.
pub fn png_item(images_dir: &Path, bytes: &[u8], w: u32, h: u32) -> Option<NewItem> {
    let hash = sha256(bytes);
    let path = images_dir.join(format!("{hash}.png"));
    if !path.exists() {
        std::fs::write(&path, bytes).ok()?;
    }
    Some(NewItem {
        kind: "image".into(),
        content: path.to_string_lossy().into_owned(),
        preview: format!("{w} × {h} 이미지"),
        hash,
        source_app: None,
        app_icon: None,
        app_color: None,
        width: Some(w as i64),
        height: Some(h as i64),
        size: bytes.len() as i64,
        origin_device: None,
        concealed: false,
    })
}

pub fn text_item(text: String) -> NewItem {
    let trimmed = text.trim();
    let is_link = (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        && !trimmed.contains(char::is_whitespace);
    NewItem {
        kind: if is_link { "link".into() } else { "text".into() },
        preview: text.chars().take(PREVIEW_LEN).collect(),
        hash: sha256(text.as_bytes()),
        size: text.chars().count() as i64,
        content: text,
        source_app: None,
        app_icon: None,
        app_color: None,
        width: None,
        height: None,
        origin_device: None,
        concealed: false,
    }
}

/// Stores files received from another device in their own folder under
/// `files_dir`, named by content hash, and lists them as a `files` item.
pub fn received_files_item(files_dir: &Path, files: &[(String, Vec<u8>)]) -> Option<NewItem> {
    let mut h = Sha256::new();
    for (name, bytes) in files {
        h.update(name.as_bytes());
        h.update([0]);
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    }
    let folder = files_dir.join(format!("{:x}", h.finalize()));
    let paths: Vec<PathBuf> = files.iter().map(|(name, _)| folder.join(name)).collect();
    if !paths.iter().all(|p| p.is_file()) {
        // Write next to the final folder, then move it in whole.
        let partial = folder.with_extension("part");
        let _ = std::fs::remove_dir_all(&partial);
        std::fs::create_dir_all(&partial).ok()?;
        for (name, bytes) in files {
            std::fs::write(partial.join(name), bytes).ok()?;
        }
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::rename(&partial, &folder).ok()?;
    }
    Some(files_item(paths.iter().map(|p| p.to_string_lossy().into_owned()).collect()))
}

fn files_item(files: Vec<String>) -> NewItem {
    let names: Vec<String> = files
        .iter()
        .map(|f| {
            std::path::Path::new(f)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| f.clone())
        })
        .collect();
    let joined = files.join("\n");
    NewItem {
        kind: "files".into(),
        preview: names.join("\n"),
        hash: sha256(joined.as_bytes()),
        size: files.len() as i64,
        content: serde_json::to_string(&files).unwrap_or_default(),
        source_app: None,
        app_icon: None,
        app_color: None,
        width: None,
        height: None,
        origin_device: None,
        concealed: false,
    }
}

/// Rows saved before icons existed only have the app name; resolve what we can.
fn backfill_icons(app: &AppHandle, icons: &mut AppIconCache) {
    let db = app.state::<Db>();
    let _files = db.1.lock().unwrap();
    let Ok(names) = db.apps_missing_icon() else { return };
    let mut changed = false;
    for name in names {
        if let Some(AppLook { icon: Some(icon), color: Some(color) }) = icons.look_by_name(&name) {
            if db.set_app_look(&name, &icon, &color).is_ok() {
                changed = true;
            }
        }
    }
    if changed {
        let _ = app.emit(CHANGED_EVENT, ());
    }
}

pub fn sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

pub fn start(app: AppHandle, images_dir: PathBuf, icons_dir: PathBuf) {
    std::thread::Builder::new()
        .name("clipboard-watcher".into())
        .spawn(move || {
            let ctx = match ClipboardContext::new() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[pinchy] clipboard context failed: {e}");
                    return;
                }
            };
            let mut watcher = match ClipboardWatcherContext::new_with_interval(Duration::from_millis(250)) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("[pinchy] clipboard watcher failed: {e}");
                    return;
                }
            };
            let mut icons = AppIconCache::new(icons_dir);
            backfill_icons(&app, &mut icons);
            watcher.add_handler(Handler { app, ctx, images_dir, icons });
            watcher.start_watch();
        })
        .expect("spawn clipboard watcher");
}
