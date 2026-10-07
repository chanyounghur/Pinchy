mod appicon;
mod clipboard;
mod db;
mod panel;
mod paste;
mod platform;
mod settings;
mod updater;

use db::{Db, Item};
use settings::{Settings, SettingsStore};
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_autostart::ManagerExt as AutostartExt;

const SETTINGS_WINDOW: &str = "settings";

#[tauri::command]
fn get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let autostart = app.autolaunch();
    if enabled {
        autostart.enable()
    } else {
        autostart.disable()
    }.map_err(|e| e.to_string())?;
    autostart.is_enabled().map_err(|e| e.to_string())
}

/// Registers `shortcut` as the only global shortcut. On failure the previous
/// one is restored and the error returned.
fn apply_shortcut(app: &AppHandle, shortcut: &str, previous: Option<&str>) -> Result<(), String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    match gs.register(shortcut) {
        Ok(()) => Ok(()),
        Err(e) => {
            if let Some(prev) = previous {
                let _ = gs.register(prev);
            }
            Err(format!("단축키를 등록할 수 없어요: {e}"))
        }
    }
}

fn open_settings_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(SETTINGS_WINDOW) {
        let _ = win.show();
        let _ = win.set_focus();
        platform::activate_self();
    }
}

#[tauri::command]
fn get_settings(store: State<SettingsStore>) -> Settings {
    store.get()
}

#[tauri::command]
fn set_shortcut(app: AppHandle, store: State<SettingsStore>, shortcut: String) -> Result<(), String> {
    let shortcut = shortcut.trim().to_string();
    if shortcut.is_empty() {
        return Err("단축키가 비어 있어요".into());
    }
    let mut settings = store.get();
    apply_shortcut(&app, &shortcut, Some(&settings.shortcut))?;
    settings.shortcut = shortcut;
    store.save(settings)
}

/// While the settings page is capturing keys, the global shortcut is released
/// so pressing the current combo doesn't toggle the panel.
#[tauri::command]
fn set_shortcut_capturing(app: AppHandle, store: State<SettingsStore>, capturing: bool) {
    let gs = app.global_shortcut();
    if capturing {
        let _ = gs.unregister_all();
    } else {
        let _ = gs.register(store.get().shortcut.as_str());
    }
}

#[tauri::command]
fn list_items(db: State<Db>, query: Option<String>, limit: Option<i64>) -> Result<Vec<Item>, String> {
    db.list(query.as_deref().unwrap_or(""), limit.unwrap_or(200))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn paste_item(app: AppHandle, db: State<Db>, id: i64) -> Result<(), String> {
    let item = db.get(id).map_err(|e| e.to_string())?.ok_or("item not found")?;
    paste::paste(&app, &item)
}

#[tauri::command]
fn copy_item(app: AppHandle, db: State<Db>, id: i64) -> Result<(), String> {
    let item = db.get(id).map_err(|e| e.to_string())?.ok_or("item not found")?;
    paste::copy_to_clipboard(&item)?;
    panel::hide(&app);
    Ok(())
}

#[tauri::command]
fn delete_item(app: AppHandle, db: State<Db>, id: i64) -> Result<(), String> {
    if let Some(item) = db.delete(id).map_err(|e| e.to_string())? {
        remove_image_file(&item);
    }
    let _ = app.emit(clipboard::CHANGED_EVENT, ());
    Ok(())
}

#[tauri::command]
fn clear_history(app: AppHandle, db: State<Db>) -> Result<(), String> {
    for item in db.clear().map_err(|e| e.to_string())? {
        remove_image_file(&item);
    }
    let _ = app.emit(clipboard::CHANGED_EVENT, ());
    Ok(())
}

#[tauri::command]
fn hide_panel(app: AppHandle) {
    panel::hide(&app);
}

fn remove_image_file(item: &Item) {
    if item.kind == "image" {
        let _ = std::fs::remove_file(&item.content);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updater::UpdateState::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        panel::toggle(app);
                    }
                })
                .build(),
        )
        .manage(panel::PanelState::default())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let data_dir = app.path().app_data_dir()?;
            let images_dir = data_dir.join("images");
            let icons_dir = data_dir.join("icons");
            std::fs::create_dir_all(&images_dir)?;
            appicon::ensure_dir(&icons_dir);
            app.manage(Db::open(&data_dir.join("pastel.db"))?);

            let store = SettingsStore::load(data_dir.join("settings.json"));
            let shortcut = store.get().shortcut;
            app.manage(store);
            if let Err(e) = apply_shortcut(app.handle(), &shortcut, None) {
                eprintln!("[pinchy] {e}; falling back to {}", settings::DEFAULT_SHORTCUT);
                let _ = apply_shortcut(app.handle(), settings::DEFAULT_SHORTCUT, None);
            }

            clipboard::start(app.handle().clone(), images_dir, icons_dir);
            panel::init(app.handle());

            let open = MenuItemBuilder::with_id("open", "열기").build(app)?;
            let settings_item = MenuItemBuilder::with_id("settings", "설정 패널 열기").build(app)?;
            let clear = MenuItemBuilder::with_id("clear", "히스토리 비우기").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "종료").build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&open)
                .item(&settings_item)
                .separator()
                .item(&clear)
                .separator()
                .item(&quit)
                .build()?;
            #[cfg(target_os = "macos")]
            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray/32x32.png"))?;
            #[cfg(target_os = "windows")]
            let tray_icon = {
                // The 512px app icon has ~32px of transparent padding per side.
                // Trim only that padding before shrinking for the tray, keeping
                // the cream tile and artwork intact. Leave other app icons alone.
                let icon = image::load_from_memory(include_bytes!("../icons/icon.png"))?
                    .crop_imm(32, 32, 448, 448)
                    .resize_exact(32, 32, image::imageops::FilterType::Lanczos3)
                    .into_rgba8();
                tauri::image::Image::new_owned(icon.into_raw(), 32, 32)
            };
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let tray_icon = app.default_window_icon().cloned().expect("default icon");
            TrayIconBuilder::new()
                .icon(tray_icon)
                .icon_as_template(cfg!(target_os = "macos"))
                .tooltip("Pinchy")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, e| match e.id().as_ref() {
                    "open" => panel::show(app),
                    "settings" => open_settings_window(app),
                    "clear" => {
                        let db = app.state::<Db>();
                        if let Ok(items) = db.clear() {
                            items.iter().for_each(remove_image_file);
                        }
                        let _ = app.emit(clipboard::CHANGED_EVENT, ());
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            updater::start(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // The settings window is reused: closing just hides it.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == SETTINGS_WINDOW {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            if let WindowEvent::Focused(false) = event {
                let app = window.app_handle();
                let open = *app.state::<panel::PanelState>().open.lock().unwrap();
                if window.label() == panel::WINDOW && open {
                    panel::hide(app);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_items,
            paste_item,
            copy_item,
            delete_item,
            clear_history,
            hide_panel,
            get_settings,
            set_shortcut,
            set_shortcut_capturing,
            get_autostart,
            set_autostart,
            updater::update_status,
            updater::check_update,
            updater::install_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
