mod clipboard;
mod db;
mod panel;
mod paste;
mod platform;

use db::{Db, Item};
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};

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
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_shortcut(Shortcut::new(Some(Modifiers::SHIFT | Modifiers::SUPER), Code::KeyV))
                .expect("register shortcut")
                .with_handler(|app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        panel::toggle(app);
                    }
                })
                .build(),
        )
        .manage(panel::Target::default())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let data_dir = app.path().app_data_dir()?;
            let images_dir = data_dir.join("images");
            std::fs::create_dir_all(&images_dir)?;
            app.manage(Db::open(&data_dir.join("pastel.db"))?);

            clipboard::start(app.handle().clone(), images_dir);

            let open = MenuItemBuilder::with_id("open", "열기  ⇧⌘V").build(app)?;
            let clear = MenuItemBuilder::with_id("clear", "히스토리 비우기").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "종료").build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&open)
                .separator()
                .item(&clear)
                .separator()
                .item(&quit)
                .build()?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().expect("default icon"))
                .icon_as_template(true)
                .tooltip("Pastel")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, e| match e.id().as_ref() {
                    "open" => panel::show(app),
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
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::Focused(false) = event {
                if window.label() == panel::WINDOW {
                    panel::hide(window.app_handle());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_items,
            paste_item,
            copy_item,
            delete_item,
            clear_history,
            hide_panel
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
