use serde::Serialize;
use std::{sync::Mutex, time::Duration};
use tauri::{menu::MenuItem, AppHandle, Emitter, Manager, State, Wry};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Clone, Serialize)]
pub struct Status {
    phase: &'static str,
    current_version: String,
    version: Option<String>,
    error: Option<String>,
}

struct Inner {
    status: Status,
    pending: Option<(Update, Vec<u8>)>,
    /// The tray menu item that mirrors the current phase.
    menu_item: Option<MenuItem<Wry>>,
}

pub struct UpdateState(Mutex<Inner>);

impl Default for UpdateState {
    fn default() -> Self {
        Self(Mutex::new(Inner {
            status: Status {
                phase: "idle",
                current_version: env!("CARGO_PKG_VERSION").into(),
                version: None,
                error: None,
            },
            pending: None,
            menu_item: None,
        }))
    }
}

fn menu_label(phase: &str) -> &'static str {
    match phase {
        "checking" => "업데이트 확인 중…",
        "downloading" => "업데이트 다운로드 중…",
        "ready" => "설치하고 재시작",
        "installing" => "설치 중…",
        _ => "업데이트 확인",
    }
}

fn publish(app: &AppHandle) {
    let (status, menu_item) = {
        let state = app.state::<UpdateState>();
        let inner = state.0.lock().unwrap();
        (inner.status.clone(), inner.menu_item.clone())
    };
    if let Some(item) = menu_item {
        let _ = item.set_text(menu_label(status.phase));
        let _ = item.set_enabled(!matches!(
            status.phase,
            "checking" | "downloading" | "installing"
        ));
    }
    let _ = app.emit("update-status", status);
}

/// Keeps `item`'s text and enabled state in step with the update phase.
pub fn attach_menu_item(app: &AppHandle, item: MenuItem<Wry>) {
    app.state::<UpdateState>().0.lock().unwrap().menu_item = Some(item);
    publish(app);
}

/// The tray item installs a downloaded update, or checks for one otherwise.
pub fn run_menu_item(app: AppHandle) {
    let ready = app.state::<UpdateState>().0.lock().unwrap().status.phase == "ready";
    tauri::async_runtime::spawn(async move {
        let _ = if ready {
            install_update(app).await
        } else {
            check_update(app).await
        };
    });
}

#[tauri::command]
pub fn update_status(state: State<UpdateState>) -> Status {
    state.0.lock().unwrap().status.clone()
}

#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<(), String> {
    {
        let state = app.state::<UpdateState>();
        let mut inner = state.0.lock().unwrap();
        if matches!(
            inner.status.phase,
            "checking" | "downloading" | "ready" | "installing"
        ) {
            return Ok(());
        }
        inner.status.phase = "checking";
        inner.status.error = None;
        inner.status.version = None;
    }
    publish(&app);
    let result = fetch(&app).await;
    if let Err(error) = &result {
        let state = app.state::<UpdateState>();
        let mut inner = state.0.lock().unwrap();
        inner.status.phase = "error";
        inner.status.error = Some(error.clone());
    }
    publish(&app);
    result
}

async fn fetch(app: &AppHandle) -> Result<(), String> {
    let updater = app
        .updater_builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let update = updater.check().await.map_err(|e| e.to_string())?;
    if let Some(update) = update {
        {
            let state = app.state::<UpdateState>();
            let mut inner = state.0.lock().unwrap();
            inner.status.phase = "downloading";
            inner.status.version = Some(update.version.clone());
        }
        publish(app);
        // The plugin verifies the signature before returning these bytes.
        let bytes = update
            .download(|_, _| {}, || {})
            .await
            .map_err(|e| e.to_string())?;
        let state = app.state::<UpdateState>();
        let mut inner = state.0.lock().unwrap();
        inner.pending = Some((update, bytes));
        inner.status.phase = "ready";
    } else {
        app.state::<UpdateState>().0.lock().unwrap().status.phase = "latest";
    }
    Ok(())
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let (update, bytes) = {
        let state = app.state::<UpdateState>();
        let mut inner = state.0.lock().unwrap();
        let pending = inner.pending.take().ok_or("설치할 업데이트가 없어요")?;
        inner.status.phase = "installing";
        pending
    };
    publish(&app);
    // Windows exits here and the installer relaunches the app. macOS restarts below.
    if let Err(error) = update.install(bytes) {
        let message = error.to_string();
        {
            let state = app.state::<UpdateState>();
            let mut inner = state.0.lock().unwrap();
            inner.status.phase = "error";
            inner.status.error = Some(message.clone());
        }
        publish(&app);
        return Err(message);
    }
    app.restart();
}

pub fn start(app: AppHandle) {
    // A native timer keeps checking even while every webview is hidden.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(30));
        loop {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = check_update(handle).await;
            });
            std::thread::sleep(Duration::from_secs(6 * 60 * 60));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::menu_label;

    #[test]
    fn menu_label_offers_install_only_when_ready() {
        assert_eq!(menu_label("ready"), "설치하고 재시작");
        for phase in ["idle", "latest", "error"] {
            assert_eq!(menu_label(phase), "업데이트 확인");
        }
    }
}
