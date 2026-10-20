//! HTTP API. Every `/v1` route needs `Authorization: Bearer <device token>`.

use crate::store::{valid_item_id, CreateError, Device, InboxItem, Store};
use axum::{
    body::Body,
    extract::{ConnectInfo, DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use base64::Engine;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{io::AsyncWriteExt, sync::broadcast};

/// Longest a client may wait for new mail in one request.
const MAX_WAIT_SECS: u64 = 30;
const FAILURE_WINDOW: Duration = Duration::from_secs(600);
const MAX_FAILURES: u32 = 20;

pub struct Limits {
    pub max_envelope_bytes: usize,
    pub max_blob_bytes: u64,
}

pub struct AppState {
    pub store: Store,
    pub limits: Limits,
    /// Wakes long-polling inbox requests when mail becomes visible.
    changed: broadcast::Sender<()>,
    failures: Mutex<HashMap<IpAddr, (u32, Instant)>>,
}

impl AppState {
    pub fn new(store: Store, limits: Limits) -> Arc<Self> {
        Arc::new(AppState { store, limits, changed: broadcast::channel(16).0, failures: Mutex::default() })
    }
}

pub struct ApiError(StatusCode, &'static str);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    eprintln!("[pinchy-relay] {e}");
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

/// Resolves the bearer token, refusing addresses that keep guessing.
fn auth(state: &AppState, headers: &HeaderMap, ip: IpAddr) -> Result<Device, ApiError> {
    {
        let failures = state.failures.lock().unwrap();
        if let Some((count, since)) = failures.get(&ip) {
            if *count >= MAX_FAILURES && since.elapsed() < FAILURE_WINDOW {
                return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "too many failed attempts"));
            }
        }
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if let Some(device) = token.and_then(|t| state.store.authenticate(t)) {
        return Ok(device);
    }
    let mut failures = state.failures.lock().unwrap();
    failures.retain(|_, (_, since)| since.elapsed() < FAILURE_WINDOW);
    failures.entry(ip).or_insert((0, Instant::now())).0 += 1;
    Err(ApiError(StatusCode::UNAUTHORIZED, "invalid device token"))
}

async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "pinchy-relay",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn me(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    let peers = state.store.peers(&device).map_err(internal)?;
    Ok(Json(json!({ "id": device.id, "name": device.name, "peers": peers })))
}

#[derive(Deserialize)]
struct NewItem {
    id: String,
    /// Base64 of the encrypted envelope.
    envelope: String,
    #[serde(default)]
    has_blob: bool,
}

async fn create_item(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(item): Json<NewItem>,
) -> Result<Response, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    if !valid_item_id(&item.id) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "item id must be 32 lowercase hex characters"));
    }
    let envelope = base64::engine::general_purpose::STANDARD
        .decode(&item.envelope)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "envelope must be base64"))?;
    if envelope.is_empty() || envelope.len() > state.limits.max_envelope_bytes {
        return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "envelope is empty or too large"));
    }
    let recipients = match state.store.create_item(&device, &item.id, &envelope, item.has_blob) {
        Ok(n) => n,
        Err(CreateError::Duplicate) => return Err(ApiError(StatusCode::CONFLICT, "item id already used")),
        Err(CreateError::NoRecipients) => {
            return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, "no other devices to deliver to"))
        }
        Err(CreateError::Db(e)) => return Err(internal(e)),
    };
    if !item.has_blob {
        let _ = state.changed.send(());
    }
    Ok((StatusCode::CREATED, Json(json!({ "recipients": recipients }))).into_response())
}

async fn upload_blob(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<StatusCode, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    let Some(path) = state.store.pending_blob(&device, &id) else {
        return Err(ApiError(StatusCode::NOT_FOUND, "no pending upload for this item"));
    };
    let partial = path.with_extension("part");
    let mut file = tokio::fs::File::create(&partial).await.map_err(internal)?;
    let mut stream = body.into_data_stream();
    let mut size: u64 = 0;
    let result: Result<(), ApiError> = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "upload interrupted"))?;
            size += chunk.len() as u64;
            if size > state.limits.max_blob_bytes {
                return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "blob is too large"));
            }
            file.write_all(&chunk).await.map_err(internal)?;
        }
        if size == 0 {
            return Err(ApiError(StatusCode::BAD_REQUEST, "blob is empty"));
        }
        file.sync_all().await.map_err(internal)?;
        tokio::fs::rename(&partial, &path).await.map_err(internal)
    }
    .await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(e);
    }
    state.store.mark_ready(&id, size as i64).map_err(internal)?;
    let _ = state.changed.send(());
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct InboxQuery {
    #[serde(default)]
    wait: u64,
}

async fn inbox(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<InboxQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    // Subscribe before reading so mail arriving in between still wakes us.
    let mut changed = state.changed.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(query.wait.min(MAX_WAIT_SECS));
    loop {
        let items: Vec<InboxItem> = state.store.inbox(&device, 50).map_err(internal)?;
        if !items.is_empty() || tokio::time::Instant::now() >= deadline {
            return Ok(Json(json!({ "items": items })));
        }
        let _ = tokio::time::timeout_at(deadline, changed.recv()).await;
    }
}

async fn download_blob(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    let path = state.store.blob_for(&device, &id).ok_or(ApiError(StatusCode::NOT_FOUND, "no such blob"))?;
    let file = tokio::fs::File::open(&path).await.map_err(internal)?;
    let size = file.metadata().await.map_err(internal)?.len();
    Ok((
        [(header::CONTENT_TYPE, "application/octet-stream".to_string()), (header::CONTENT_LENGTH, size.to_string())],
        Body::from_stream(tokio_util::io::ReaderStream::new(file)),
    )
        .into_response())
}

async fn ack(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    state.store.ack(&device, &id).map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn router(state: Arc<AppState>) -> Router {
    // Base64 inflates the envelope by a third, plus room for the JSON wrapper.
    let json_limit = state.limits.max_envelope_bytes / 3 * 4 + 1024;
    Router::new()
        .route("/health", get(health))
        .route("/v1/me", get(me))
        .route("/v1/items", post(create_item).layer(DefaultBodyLimit::max(json_limit)))
        .route("/v1/items/{id}/blob", put(upload_blob).get(download_blob).layer(DefaultBodyLimit::disable()))
        .route("/v1/items/{id}/ack", post(ack))
        .route("/v1/inbox", get(inbox))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::connect_info::MockConnectInfo, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    struct Fixture {
        _dir: tempfile::TempDir,
        app: Router,
        state: Arc<AppState>,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let state = AppState::new(store, Limits { max_envelope_bytes: 1024, max_blob_bytes: 16 });
        let app = router(state.clone()).layer(MockConnectInfo(SocketAddr::from(([203, 0, 113, 7], 1000))));
        Fixture { _dir: dir, app, state }
    }

    async fn call(app: &Router, method: &str, path: &str, token: &str, body: Body) -> (u16, Vec<u8>) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status().as_u16();
        (status, response.into_body().collect().await.unwrap().to_bytes().to_vec())
    }

    fn json_body(value: Value) -> Body {
        Body::from(value.to_string())
    }

    fn parse(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).unwrap()
    }

    const ID: &str = "0123456789abcdef0123456789abcdef";

    #[tokio::test]
    async fn health_is_public_and_unknown_paths_are_not_found() {
        let f = fixture();
        let (status, body) = call(&f.app, "GET", "/health", "", Body::empty()).await;
        assert_eq!(status, 200);
        assert_eq!(parse(&body)["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(call(&f.app, "GET", "/", "", Body::empty()).await.0, 404);
    }

    #[tokio::test]
    async fn me_lists_peers_and_bad_tokens_are_rejected_then_throttled() {
        let f = fixture();
        let (_, a) = f.state.store.add_device("집 맥").unwrap();
        f.state.store.add_device("회사 맥").unwrap();
        let (status, body) = call(&f.app, "GET", "/v1/me", &a, Body::empty()).await;
        assert_eq!(status, 200);
        assert_eq!(parse(&body)["name"], "집 맥");
        assert_eq!(parse(&body)["peers"], json!(["회사 맥"]));
        for _ in 0..MAX_FAILURES {
            assert_eq!(call(&f.app, "GET", "/v1/me", "pr1_guess", Body::empty()).await.0, 401);
        }
        assert_eq!(call(&f.app, "GET", "/v1/me", "pr1_guess", Body::empty()).await.0, 429);
        assert_eq!(call(&f.app, "GET", "/v1/me", &a, Body::empty()).await.0, 429, "the address is locked out");
    }

    #[tokio::test]
    async fn text_item_round_trip() {
        let f = fixture();
        let (_, a) = f.state.store.add_device("a").unwrap();
        let (_, b) = f.state.store.add_device("b").unwrap();
        let sealed = base64::engine::general_purpose::STANDARD.encode(b"sealed");
        let (status, body) =
            call(&f.app, "POST", "/v1/items", &a, json_body(json!({ "id": ID, "envelope": sealed }))).await;
        assert_eq!((status, parse(&body)["recipients"].clone()), (201, json!(1)));
        let (_, body) = call(&f.app, "GET", "/v1/inbox", &b, Body::empty()).await;
        let items = parse(&body)["items"].clone();
        assert_eq!(items[0]["id"], ID);
        assert_eq!(items[0]["from"], "a");
        assert_eq!(items[0]["envelope"], sealed);
        assert_eq!(call(&f.app, "POST", &format!("/v1/items/{ID}/ack"), &b, Body::empty()).await.0, 204);
        let (_, body) = call(&f.app, "GET", "/v1/inbox", &b, Body::empty()).await;
        assert_eq!(parse(&body)["items"], json!([]));
    }

    #[tokio::test]
    async fn blob_upload_download_and_limits() {
        let f = fixture();
        let (_, a) = f.state.store.add_device("a").unwrap();
        let (_, b) = f.state.store.add_device("b").unwrap();
        let sealed = base64::engine::general_purpose::STANDARD.encode(b"meta");
        let blob = format!("/v1/items/{ID}/blob");
        call(&f.app, "POST", "/v1/items", &a, json_body(json!({ "id": ID, "envelope": sealed, "has_blob": true }))).await;
        assert_eq!(call(&f.app, "GET", &blob, &b, Body::empty()).await.0, 404, "not ready yet");
        assert_eq!(call(&f.app, "PUT", &blob, &b, Body::from("x")).await.0, 404, "only the sender uploads");
        assert_eq!(call(&f.app, "PUT", &blob, &a, Body::from(vec![1u8; 17])).await.0, 413);
        assert_eq!(call(&f.app, "PUT", &blob, &a, Body::from("blob bytes")).await.0, 204);
        assert_eq!(call(&f.app, "PUT", &blob, &a, Body::from("again")).await.0, 404, "blobs are write-once");
        let (_, body) = call(&f.app, "GET", "/v1/inbox", &b, Body::empty()).await;
        assert_eq!(parse(&body)["items"][0]["blob_size"], 10);
        assert_eq!(call(&f.app, "GET", &blob, &b, Body::empty()).await, (200, b"blob bytes".to_vec()));
        assert_eq!(call(&f.app, "GET", &blob, &a, Body::empty()).await.0, 404, "the sender is not a recipient");
    }

    #[tokio::test]
    async fn invalid_items_are_rejected() {
        let f = fixture();
        let (_, a) = f.state.store.add_device("a").unwrap();
        let ok = base64::engine::general_purpose::STANDARD.encode(b"x");
        let post = |body: Value| call(&f.app, "POST", "/v1/items", &a, json_body(body));
        assert_eq!(post(json!({ "id": ID, "envelope": ok })).await.0, 422, "nobody to deliver to");
        f.state.store.add_device("b").unwrap();
        assert_eq!(post(json!({ "id": "../x", "envelope": ok })).await.0, 400);
        assert_eq!(post(json!({ "id": ID, "envelope": "not base64!" })).await.0, 400);
        let big = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 1025]);
        assert_eq!(post(json!({ "id": ID, "envelope": big })).await.0, 413);
        assert_eq!(post(json!({ "id": ID, "envelope": ok })).await.0, 201);
        assert_eq!(post(json!({ "id": ID, "envelope": ok })).await.0, 409);
    }

    #[tokio::test]
    async fn waiting_inbox_wakes_when_mail_arrives() {
        let f = fixture();
        let (_, a) = f.state.store.add_device("a").unwrap();
        let (_, b) = f.state.store.add_device("b").unwrap();
        let app = f.app.clone();
        let waiting = tokio::spawn(async move { call(&app, "GET", "/v1/inbox?wait=10", &b, Body::empty()).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let started = Instant::now();
        let sealed = base64::engine::general_purpose::STANDARD.encode(b"sealed");
        call(&f.app, "POST", "/v1/items", &a, json_body(json!({ "id": ID, "envelope": sealed }))).await;
        let (_, body) = waiting.await.unwrap();
        assert_eq!(parse(&body)["items"][0]["id"], ID);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
