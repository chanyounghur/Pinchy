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
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

pub const CHANGED_EVENT: &str = "clipboard-changed";
pub const PREVIEW_LEN: usize = 300;

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
        if let Some(item) = self.read_item() {
            let db = self.app.state::<Db>();
            match db.upsert(item) {
                Ok(_) => {
                    let _ = self.app.emit(CHANGED_EVENT, ());
                }
                Err(e) => eprintln!("[pastel] db upsert failed: {e}"),
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
        Some(item)
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
        let bytes = png.get_bytes();
        let hash = sha256(bytes);
        let path = self.images_dir.join(format!("{hash}.png"));
        if !path.exists() {
            std::fs::write(&path, bytes).ok()?;
        }
        let (w, h) = img.get_size();
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
        })
    }
}

fn text_item(text: String) -> NewItem {
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
    }
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
                    eprintln!("[pastel] clipboard context failed: {e}");
                    return;
                }
            };
            let mut watcher = match ClipboardWatcherContext::new_with_interval(Duration::from_millis(250)) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("[pastel] clipboard watcher failed: {e}");
                    return;
                }
            };
            watcher.add_handler(Handler { app, ctx, images_dir, icons: AppIconCache::new(icons_dir) });
            watcher.start_watch();
        })
        .expect("spawn clipboard watcher");
}
