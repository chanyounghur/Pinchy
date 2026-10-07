use serde::Serialize;
use std::{sync::Mutex, time::Duration};
use tauri::{menu::MenuItem, AppHandle, Emitter, Manager, State};
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
}

pub struct UpdateState(Mutex<Inner>);
pub struct UpdateMenu(pub MenuItem<tauri::Wry>);

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
        }))
    }
}

fn publish(app: &AppHandle) {
    let status = app.state::<UpdateState>().0.lock().unwrap().status.clone();
    let title = match status.phase {
        "ready" => "업데이트 준비됨 · 설정 열기…",
        "checking" => "업데이트 확인 중…",
        "downloading" => "업데이트 다운로드 중…",
        _ => "업데이트…",
    };
    if let Some(menu) = app.try_state::<UpdateMenu>() {
        let _ = menu.0.set_text(title);
    }
    let _ = app.emit("update-status", status);
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
