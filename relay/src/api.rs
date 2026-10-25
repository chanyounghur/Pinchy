//! HTTP API. Routes under `/v1` need `Authorization: Bearer <device token>`,
//! except the ones a joining device uses before it has a token.
//!
//! The history lives on the relay: devices `PUT` items, `DELETE` them and
//! follow `/v1/changes` to mirror each other.

use crate::pairing::{Answer, Joining, PairingError};
use crate::store::{valid_item_id, Device, PutError, PutOutcome, Store};
use axum::{
    body::Body,
    extract::{ConnectInfo, DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use base64::Engine;
use futures_util::StreamExt;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{io::AsyncWriteExt, sync::broadcast};

/// Longest a client may wait for new changes in one request.
const MAX_WAIT_SECS: u64 = 30;
/// Most changes returned per request.
const MAX_CHANGES: i64 = 200;
const FAILURE_WINDOW: Duration = Duration::from_secs(600);
const MAX_FAILURES: u32 = 20;
/// Sealed device names, SPAKE2 messages and sealed keys are all small.
const MAX_SMALL_FIELD: usize = 2048;

pub struct Limits {
    pub max_envelope_bytes: usize,
    pub max_blob_bytes: u64,
}

/// What every device follows. Set by the relay's options and handed to the
/// clients as `/v1/policy`; the relay enforces the retention itself.
#[derive(Clone, serde::Serialize)]
pub struct Policy {
    /// Items untouched for this long are removed from every device.
    pub retention_days: u32,
    /// Oldest items go first once the history grows past this.
    pub max_storage_mb: u64,
    pub max_blob_mb: u64,
    /// Whether devices sync copies their clipboard marks as concealed
    /// (password managers). The relay cannot tell; the clients enforce it.
    pub sync_concealed: bool,
}

pub struct AppState {
    pub store: Store,
    pub limits: Limits,
    pub policy: Policy,
    pub joining: Joining,
    /// Wakes long-polling change requests when the feed grows.
    changed: broadcast::Sender<()>,
    failures: Mutex<HashMap<IpAddr, (u32, Instant)>>,
}

impl AppState {
    pub fn new(store: Store, limits: Limits, policy: Policy) -> Arc<Self> {
        Arc::new(AppState {
            store,
            limits,
            policy,
            joining: Joining::default(),
            changed: broadcast::channel(16).0,
            failures: Mutex::default(),
        })
    }

    /// Tells waiting clients that the feed changed. Called after every
    /// change, including the relay's own purges.
    pub fn notify(&self) {
        let _ = self.changed.send(());
    }

    /// Prints a new first-device code when no device exists and the old one
    /// is gone. Called at start, periodically, and when devices change.
    pub fn announce_first_code(&self) {
        let Ok(count) = self.store.device_count() else { return };
        if let Some(code) = self.joining.refresh_first_code(count) {
            eprintln!(
                "[pinchy-relay] first device code: {code} (valid 30 minutes; enter it in Pinchy on your first device)"
            );
        }
    }

    fn throttled(&self, ip: IpAddr) -> bool {
        let failures = self.failures.lock().unwrap();
        failures.get(&ip).is_some_and(|(count, since)| *count >= MAX_FAILURES && since.elapsed() < FAILURE_WINDOW)
    }

    fn record_failure(&self, ip: IpAddr) {
        let mut failures = self.failures.lock().unwrap();
        failures.retain(|_, (_, since)| since.elapsed() < FAILURE_WINDOW);
        failures.entry(ip).or_insert((0, Instant::now())).0 += 1;
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
    if state.throttled(ip) {
        return Err(TOO_MANY);
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if let Some(device) = token.and_then(|t| state.store.authenticate(t)) {
        return Ok(device);
    }
    state.record_failure(ip);
    Err(ApiError(StatusCode::UNAUTHORIZED, "invalid device token"))
}

const TOO_MANY: ApiError = ApiError(StatusCode::TOO_MANY_REQUESTS, "too many failed attempts");
const GONE: ApiError = ApiError(StatusCode::NOT_FOUND, "no such pairing; it may have expired");

/// Decodes a base64 field of at most `max` bytes.
fn field(value: &str, max: usize) -> Result<Vec<u8>, ApiError> {
    STANDARD
        .decode(value)
        .ok()
        .filter(|b| !b.is_empty() && b.len() <= max)
        .ok_or(ApiError(StatusCode::BAD_REQUEST, "malformed field"))
}

fn pairing_error(e: PairingError) -> ApiError {
    match e {
        PairingError::Gone => GONE,
        PairingError::Taken => ApiError(StatusCode::CONFLICT, "this code was already used"),
        PairingError::Refused => ApiError(StatusCode::FORBIDDEN, "not allowed at this step"),
    }
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
    let devices = state.store.device_count().map_err(internal)?;
    Ok(Json(json!({ "id": device.id, "devices": devices })))
}

#[derive(Deserialize)]
struct Label {
    label: String,
}

async fn rename(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Label>,
) -> Result<StatusCode, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    state.store.set_label(&device, &field(&body.label, MAX_SMALL_FIELD)?).map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn devices(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    let devices = state.store.list_devices().map_err(internal)?;
    Ok(Json(json!({ "me": device.id, "devices": devices })))
}

/// Any connected device may remove any device, including itself.
async fn remove_device(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    auth(&state, &headers, addr.ip())?;
    if !state.store.remove_device(&id).map_err(internal)? {
        return Err(ApiError(StatusCode::NOT_FOUND, "no such device"));
    }
    state.announce_first_code();
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct FirstJoin {
    code: String,
    label: String,
}

async fn join_first(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<FirstJoin>,
) -> Result<Response, ApiError> {
    if state.throttled(addr.ip()) {
        return Err(TOO_MANY);
    }
    let label = field(&body.label, MAX_SMALL_FIELD)?;
    if !state.joining.take_first_code(&body.code) {
        state.record_failure(addr.ip());
        state.announce_first_code();
        return Err(ApiError(StatusCode::FORBIDDEN, "wrong or expired code"));
    }
    let (device, token) = state.store.add_device(&label).map_err(internal)?;
    eprintln!("[pinchy-relay] first device connected");
    Ok((StatusCode::CREATED, Json(json!({ "device_id": device.id, "token": token }))).into_response())
}

#[derive(Deserialize)]
struct Offer {
    msg: String,
}

async fn create_pairing(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Offer>,
) -> Result<Response, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    let nameplate = state.joining.create(&device.id, field(&body.msg, MAX_SMALL_FIELD)?);
    Ok((StatusCode::CREATED, Json(json!({ "nameplate": nameplate }))).into_response())
}

async fn pairing_answer(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(nameplate): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    let answer = state.joining.answer_for(&nameplate, &device.id).map_err(pairing_error)?;
    Ok(Json(json!({ "answer": answer.map(|a| json!({
        "msg": STANDARD.encode(a.msg_b),
        "confirm": STANDARD.encode(a.confirm),
        "name": STANDARD.encode(a.name),
    })) })))
}

#[derive(Deserialize)]
struct Approval {
    /// The sharing key, sealed under the SPAKE2 key.
    payload: String,
    /// The joiner's name, re-sealed with the sharing key.
    label: String,
}

async fn approve_pairing(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(nameplate): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Approval>,
) -> Result<Json<Value>, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    let payload = field(&body.payload, MAX_SMALL_FIELD)?;
    let label = field(&body.label, MAX_SMALL_FIELD)?;
    let registered = state
        .joining
        .approve(&nameplate, &device.id, payload, || {
            state.store.add_device(&label).map(|(device, token)| (device.id, token))
        })
        .map_err(pairing_error)?
        .map_err(internal)?;
    Ok(Json(json!({ "device_id": registered })))
}

async fn cancel_pairing(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(nameplate): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    state.joining.cancel(&nameplate, &device.id);
    Ok(StatusCode::NO_CONTENT)
}

async fn join_pairing(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(nameplate): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if state.throttled(addr.ip()) {
        return Err(TOO_MANY);
    }
    match state.joining.join(&nameplate) {
        Ok((msg, ticket)) => Ok(Json(json!({ "msg": STANDARD.encode(msg), "ticket": ticket }))),
        Err(e) => {
            state.record_failure(addr.ip());
            Err(pairing_error(e))
        }
    }
}

#[derive(Deserialize)]
struct JoinAnswer {
    ticket: String,
    msg: String,
    confirm: String,
    name: String,
}

async fn answer_pairing(
    State(state): State<Arc<AppState>>,
    Path(nameplate): Path<String>,
    Json(body): Json<JoinAnswer>,
) -> Result<StatusCode, ApiError> {
    let answer = Answer {
        msg_b: field(&body.msg, MAX_SMALL_FIELD)?,
        confirm: field(&body.confirm, MAX_SMALL_FIELD)?,
        name: field(&body.name, MAX_SMALL_FIELD)?,
    };
    state.joining.answer(&nameplate, &body.ticket, answer).map_err(pairing_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct Ticket {
    ticket: String,
}

async fn pairing_result(
    State(state): State<Arc<AppState>>,
    Path(nameplate): Path<String>,
    Query(query): Query<Ticket>,
) -> Result<Response, ApiError> {
    match state.joining.result(&nameplate, &query.ticket).map_err(pairing_error)? {
        None => Ok((StatusCode::ACCEPTED, Json(json!({}))).into_response()),
        Some(approved) => Ok(Json(json!({
            "device_id": approved.device_id,
            "token": approved.token,
            "payload": STANDARD.encode(approved.payload),
        }))
        .into_response()),
    }
}

async fn policy(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    auth(&state, &headers, addr.ip())?;
    let usage = state.store.usage().map_err(internal)?;
    Ok(Json(json!({
        "retention_days": state.policy.retention_days,
        "max_storage_mb": state.policy.max_storage_mb,
        "max_blob_mb": state.policy.max_blob_mb,
        "sync_concealed": state.policy.sync_concealed,
        "usage": usage,
    })))
}

/// Mints a one-time join code for a device that will scan it (with the
/// sharing key, which the relay never sees) instead of typing a code.
async fn create_join_code(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    let code = state.joining.create_join_code(&device.id);
    Ok((
        StatusCode::CREATED,
        Json(json!({ "code": code, "expires_in": crate::pairing::JOIN_CODE_TTL.as_secs() })),
    )
        .into_response())
}

async fn cancel_join_code(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    state.joining.cancel_join_code(&device.id);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct Join {
    code: String,
    label: String,
}

/// Registers the device that scanned a join code.
async fn join(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<Join>,
) -> Result<Response, ApiError> {
    if state.throttled(addr.ip()) {
        return Err(TOO_MANY);
    }
    let label = field(&body.label, MAX_SMALL_FIELD)?;
    if !state.joining.take_join_code(body.code.trim()) {
        state.record_failure(addr.ip());
        return Err(ApiError(StatusCode::FORBIDDEN, "wrong or expired code"));
    }
    let (device, token) = state.store.add_device(&label).map_err(internal)?;
    Ok((StatusCode::CREATED, Json(json!({ "device_id": device.id, "token": token }))).into_response())
}

#[derive(Deserialize)]
struct PutItem {
    /// Base64 of the encrypted envelope.
    envelope: String,
    #[serde(default)]
    has_blob: bool,
}

/// Creates an item or replaces its envelope. The response says whether a
/// blob upload must follow before the item becomes visible.
async fn put_item(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(item): Json<PutItem>,
) -> Result<Response, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    if !valid_item_id(&id) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "item id must be 32 lowercase hex characters"));
    }
    let envelope = STANDARD
        .decode(&item.envelope)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "envelope must be base64"))?;
    if envelope.is_empty() || envelope.len() > state.limits.max_envelope_bytes {
        return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "envelope is empty or too large"));
    }
    let outcome = match state.store.put_item(&device, &id, &envelope, item.has_blob) {
        Ok(outcome) => outcome,
        Err(PutError::Deleted) => return Err(ApiError(StatusCode::GONE, "item was deleted")),
        Err(PutError::BlobMismatch) => {
            return Err(ApiError(StatusCode::CONFLICT, "an item cannot gain or lose its blob"))
        }
        Err(PutError::Db(e)) => return Err(internal(e)),
    };
    let (status, blob_pending) = match outcome {
        PutOutcome::Created => (StatusCode::CREATED, false),
        PutOutcome::AwaitingBlob => (StatusCode::CREATED, true),
        PutOutcome::Updated => (StatusCode::OK, false),
    };
    if !blob_pending {
        state.notify();
    }
    Ok((status, Json(json!({ "blob_pending": blob_pending }))).into_response())
}

async fn delete_item(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    // Deleting what is already gone is a success: the client's intent holds.
    if state.store.delete_item(&device, &id).map_err(internal)? {
        state.notify();
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_all(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let device = auth(&state, &headers, addr.ip())?;
    let removed = state.store.delete_all(&device).map_err(internal)?;
    if removed > 0 {
        state.notify();
    }
    Ok(Json(json!({ "removed": removed })))
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
    state.notify();
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ChangesQuery {
    #[serde(default)]
    since: i64,
    #[serde(default)]
    wait: u64,
    #[serde(default)]
    limit: Option<i64>,
}

/// The change feed after `since`. With nothing new, waits up to `wait`
/// seconds for something to happen. A client behind the compaction floor
/// gets 409 and must start over from zero.
async fn changes(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<ChangesQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    auth(&state, &headers, addr.ip())?;
    if query.since < 0 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "since must not be negative"));
    }
    let limit = query.limit.unwrap_or(MAX_CHANGES).clamp(1, MAX_CHANGES);
    // Subscribe before reading so a change arriving in between still wakes us.
    let mut changed = state.changed.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(query.wait.min(MAX_WAIT_SECS));
    loop {
        let page = state
            .store
            .changes(query.since, limit)
            .map_err(internal)?
            .ok_or(ApiError(StatusCode::CONFLICT, "too far behind; sync again from zero"))?;
        if !page.items.is_empty() || tokio::time::Instant::now() >= deadline {
            return Ok(Json(serde_json::to_value(page).map_err(internal)?));
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
    auth(&state, &headers, addr.ip())?;
    let path = state.store.blob_for(&id).ok_or(ApiError(StatusCode::NOT_FOUND, "no such blob"))?;
    let file = tokio::fs::File::open(&path).await.map_err(internal)?;
    let size = file.metadata().await.map_err(internal)?.len();
    Ok((
        [(header::CONTENT_TYPE, "application/octet-stream".to_string()), (header::CONTENT_LENGTH, size.to_string())],
        Body::from_stream(tokio_util::io::ReaderStream::new(file)),
    )
        .into_response())
}

pub fn router(state: Arc<AppState>) -> Router {
    // Base64 inflates the envelope by a third, plus room for the JSON wrapper.
    let json_limit = state.limits.max_envelope_bytes / 3 * 4 + 1024;
    Router::new()
        .route("/health", get(health))
        .route("/v1/me", get(me).put(rename))
        .route("/v1/devices", get(devices))
        .route("/v1/devices/{id}", delete(remove_device))
        .route("/v1/first", post(join_first))
        .route("/v1/pairings", post(create_pairing))
        .route("/v1/pairings/{nameplate}", get(pairing_answer).delete(cancel_pairing))
        .route("/v1/pairings/{nameplate}/approve", post(approve_pairing))
        .route("/v1/pairings/{nameplate}/join", post(join_pairing))
        .route("/v1/pairings/{nameplate}/answer", post(answer_pairing))
        .route("/v1/pairings/{nameplate}/result", get(pairing_result))
        .route("/v1/join-codes", post(create_join_code).delete(cancel_join_code))
        .route("/v1/join", post(join))
        .route("/v1/policy", get(policy))
        .route("/v1/items", delete(delete_all))
        .route("/v1/items/{id}", put(put_item).delete(delete_item).layer(DefaultBodyLimit::max(json_limit)))
        .route("/v1/items/{id}/blob", put(upload_blob).get(download_blob).layer(DefaultBodyLimit::disable()))
        .route("/v1/changes", get(changes))
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
        let policy = Policy { retention_days: 30, max_storage_mb: 1024, max_blob_mb: 64, sync_concealed: false };
        let state = AppState::new(store, Limits { max_envelope_bytes: 1024, max_blob_bytes: 16 }, policy);
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

    fn b64(bytes: &[u8]) -> String {
        STANDARD.encode(bytes)
    }

    #[tokio::test]
    async fn bad_tokens_are_rejected_then_throttled() {
        let f = fixture();
        let (_, a) = f.state.store.add_device(b"a").unwrap();
        f.state.store.add_device(b"b").unwrap();
        let (status, body) = call(&f.app, "GET", "/v1/me", &a, Body::empty()).await;
        assert_eq!(status, 200);
        assert_eq!(parse(&body)["devices"], 2);
        for _ in 0..MAX_FAILURES {
            assert_eq!(call(&f.app, "GET", "/v1/me", "pr1_guess", Body::empty()).await.0, 401);
        }
        assert_eq!(call(&f.app, "GET", "/v1/me", "pr1_guess", Body::empty()).await.0, 429);
        assert_eq!(call(&f.app, "GET", "/v1/me", &a, Body::empty()).await.0, 429, "the address is locked out");
    }

    #[tokio::test]
    async fn items_are_put_updated_deleted_and_followed_through_changes() {
        let f = fixture();
        let (a_dev, a) = f.state.store.add_device(b"a").unwrap();
        let (_, b) = f.state.store.add_device(b"b").unwrap();
        let sealed = b64(b"sealed");
        let (status, body) =
            call(&f.app, "PUT", &format!("/v1/items/{ID}"), &a, json_body(json!({ "envelope": sealed }))).await;
        assert_eq!((status, parse(&body)["blob_pending"].clone()), (201, json!(false)));
        let (_, body) = call(&f.app, "GET", "/v1/changes", &b, Body::empty()).await;
        let page = parse(&body);
        assert_eq!(page["items"][0]["id"], ID);
        assert_eq!(page["items"][0]["origin"], a_dev.id);
        assert_eq!(page["items"][0]["envelope"], sealed);
        assert_eq!(page["items"][0]["deleted"], false);
        assert_eq!((page["next"].clone(), page["more"].clone()), (json!(1), json!(false)));

        let (status, _) =
            call(&f.app, "PUT", &format!("/v1/items/{ID}"), &b, json_body(json!({ "envelope": b64(b"bumped") }))).await;
        assert_eq!(status, 200, "updates are fine from any device");
        let (_, body) = call(&f.app, "GET", "/v1/changes?since=1", &a, Body::empty()).await;
        assert_eq!(parse(&body)["items"][0]["envelope"], b64(b"bumped"));

        assert_eq!(call(&f.app, "DELETE", &format!("/v1/items/{ID}"), &a, Body::empty()).await.0, 204);
        assert_eq!(call(&f.app, "DELETE", &format!("/v1/items/{ID}"), &a, Body::empty()).await.0, 204, "idempotent");
        let (_, body) = call(&f.app, "GET", "/v1/changes?since=2", &b, Body::empty()).await;
        let tombstone = parse(&body)["items"][0].clone();
        assert_eq!(tombstone["deleted"], true);
        assert!(tombstone.get("envelope").is_none());
        let (status, _) =
            call(&f.app, "PUT", &format!("/v1/items/{ID}"), &b, json_body(json!({ "envelope": sealed }))).await;
        assert_eq!(status, 410, "deletions win");
        let (_, body) = call(&f.app, "GET", "/v1/changes?since=3", &b, Body::empty()).await;
        assert_eq!(parse(&body)["items"], json!([]));
        assert_eq!(parse(&body)["next"], 3);
    }

    #[tokio::test]
    async fn blob_upload_download_and_limits() {
        let f = fixture();
        let (_, a) = f.state.store.add_device(b"a").unwrap();
        let (_, b) = f.state.store.add_device(b"b").unwrap();
        let blob = format!("/v1/items/{ID}/blob");
        let (status, body) = call(
            &f.app,
            "PUT",
            &format!("/v1/items/{ID}"),
            &a,
            json_body(json!({ "envelope": b64(b"meta"), "has_blob": true })),
        )
        .await;
        assert_eq!((status, parse(&body)["blob_pending"].clone()), (201, json!(true)));
        let (_, body) = call(&f.app, "GET", "/v1/changes", &b, Body::empty()).await;
        assert_eq!(parse(&body)["items"], json!([]), "not visible yet");
        assert_eq!(call(&f.app, "GET", &blob, &b, Body::empty()).await.0, 404, "not ready yet");
        assert_eq!(call(&f.app, "PUT", &blob, &b, Body::from("x")).await.0, 404, "only the creator uploads");
        assert_eq!(call(&f.app, "PUT", &blob, &a, Body::from(vec![1u8; 17])).await.0, 413);
        assert_eq!(call(&f.app, "PUT", &blob, &a, Body::from("blob bytes")).await.0, 204);
        assert_eq!(call(&f.app, "PUT", &blob, &a, Body::from("again")).await.0, 404, "blobs are write-once");
        let (_, body) = call(&f.app, "GET", "/v1/changes", &b, Body::empty()).await;
        assert_eq!(parse(&body)["items"][0]["blob_size"], 10);
        assert_eq!(call(&f.app, "GET", &blob, &b, Body::empty()).await, (200, b"blob bytes".to_vec()));
        assert_eq!(call(&f.app, "GET", &blob, &a, Body::empty()).await.0, 200, "the creator may fetch it too");
    }

    #[tokio::test]
    async fn invalid_items_are_rejected() {
        let f = fixture();
        let (_, a) = f.state.store.add_device(b"a").unwrap();
        let ok = b64(b"x");
        let app = &f.app;
        let a = &a;
        let put = move |id: String, body: Value| async move {
            call(app, "PUT", &format!("/v1/items/{id}"), a, json_body(body)).await
        };
        assert_eq!(put("..%2Fx".into(), json!({ "envelope": ok })).await.0, 400);
        assert_eq!(put(ID.into(), json!({ "envelope": "not base64!" })).await.0, 400);
        let big = b64(&vec![0u8; 1025]);
        assert_eq!(put(ID.into(), json!({ "envelope": big })).await.0, 413);
        assert_eq!(put(ID.into(), json!({ "envelope": ok })).await.0, 201);
        assert_eq!(put(ID.into(), json!({ "envelope": ok, "has_blob": true })).await.0, 409);
        assert_eq!(call(&f.app, "GET", "/v1/changes?since=-1", &a, Body::empty()).await.0, 400);
    }

    #[tokio::test]
    async fn waiting_changes_wake_when_something_happens() {
        let f = fixture();
        let (_, a) = f.state.store.add_device(b"a").unwrap();
        let (_, b) = f.state.store.add_device(b"b").unwrap();
        let app = f.app.clone();
        let waiting = tokio::spawn(async move { call(&app, "GET", "/v1/changes?wait=10", &b, Body::empty()).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let started = Instant::now();
        call(&f.app, "PUT", &format!("/v1/items/{ID}"), &a, json_body(json!({ "envelope": b64(b"sealed") }))).await;
        let (_, body) = waiting.await.unwrap();
        assert_eq!(parse(&body)["items"][0]["id"], ID);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn clients_behind_the_floor_start_over_and_delete_all_clears_the_history() {
        let f = fixture();
        let (_, a) = f.state.store.add_device(b"a").unwrap();
        for n in 1..=3u8 {
            let id = format!("{n:032x}");
            call(&f.app, "PUT", &format!("/v1/items/{id}"), &a, json_body(json!({ "envelope": b64(b"x") }))).await;
        }
        let (status, body) = call(&f.app, "DELETE", "/v1/items", &a, Body::empty()).await;
        assert_eq!((status, parse(&body)["removed"].clone()), (200, json!(3)));
        let (_, body) = call(&f.app, "GET", "/v1/changes", &a, Body::empty()).await;
        assert!(parse(&body)["items"].as_array().unwrap().iter().all(|c| c["deleted"] == true));
        f.state.store.compact(crate::store::now_ms() + 8 * 86_400_000, 7 * 86_400_000).unwrap();
        assert_eq!(call(&f.app, "GET", "/v1/changes?since=2", &a, Body::empty()).await.0, 409);
        let (status, body) = call(&f.app, "GET", "/v1/changes", &a, Body::empty()).await;
        assert_eq!((status, parse(&body)["items"].clone()), (200, json!([])));
        assert_eq!(parse(&body)["next"], 6);
    }

    #[tokio::test]
    async fn policy_is_served_to_devices_with_usage() {
        let f = fixture();
        let (_, a) = f.state.store.add_device(b"a").unwrap();
        assert_eq!(call(&f.app, "GET", "/v1/policy", "", Body::empty()).await.0, 401);
        call(&f.app, "PUT", &format!("/v1/items/{ID}"), &a, json_body(json!({ "envelope": b64(b"abcd") }))).await;
        let (status, body) = call(&f.app, "GET", "/v1/policy", &a, Body::empty()).await;
        assert_eq!(status, 200);
        let policy = parse(&body);
        assert_eq!(policy["retention_days"], 30);
        assert_eq!(policy["max_storage_mb"], 1024);
        assert_eq!(policy["sync_concealed"], false);
        assert_eq!(policy["usage"], json!({ "items": 1, "bytes": 4 }));
    }

    #[tokio::test]
    async fn join_codes_register_a_scanning_device_once() {
        let f = fixture();
        let (_, a) = f.state.store.add_device(b"a").unwrap();
        assert_eq!(call(&f.app, "POST", "/v1/join-codes", "", Body::empty()).await.0, 401);
        let (status, body) = call(&f.app, "POST", "/v1/join-codes", &a, Body::empty()).await;
        assert_eq!(status, 201);
        let code = parse(&body)["code"].as_str().unwrap().to_string();
        assert_eq!(parse(&body)["expires_in"], 600);
        let join = |code: &str| call(&f.app, "POST", "/v1/join", "", json_body(json!({ "code": code, "label": b64(b"phone") })));
        assert_eq!(join("wrong").await.0, 403);
        let (status, body) = join(&code).await;
        assert_eq!(status, 201);
        let token = parse(&body)["token"].as_str().unwrap().to_string();
        assert_eq!(call(&f.app, "GET", "/v1/me", &token, Body::empty()).await.0, 200);
        assert_eq!(join(&code).await.0, 403, "single use");
        let (_, body) = call(&f.app, "POST", "/v1/join-codes", &a, Body::empty()).await;
        let code = parse(&body)["code"].as_str().unwrap().to_string();
        assert_eq!(call(&f.app, "DELETE", "/v1/join-codes", &a, Body::empty()).await.0, 204);
        assert_eq!(join(&code).await.0, 403, "cancelled");
    }

    #[tokio::test]
    async fn first_device_joins_with_the_logged_code_only_while_none_exist() {
        let f = fixture();
        let code = f.state.joining.refresh_first_code(0).unwrap();
        let join = |code: &str| {
            call(&f.app, "POST", "/v1/first", "", json_body(json!({ "code": code, "label": b64(b"sealed") })))
        };
        assert_eq!(join("1111-2222-3333").await.0, 403);
        let (status, body) = join(&code).await;
        assert_eq!(status, 201);
        let token = parse(&body)["token"].as_str().unwrap().to_string();
        assert_eq!(call(&f.app, "GET", "/v1/me", &token, Body::empty()).await.0, 200);
        assert_eq!(join(&code).await.0, 403, "single use");
        assert_eq!(f.state.joining.refresh_first_code(1), None, "no first code once a device exists");
    }

    #[tokio::test]
    async fn invited_device_joins_after_the_inviter_approves() {
        let f = fixture();
        let (_, a) = f.state.store.add_device(b"a").unwrap();
        let (status, body) = call(&f.app, "POST", "/v1/pairings", &a, json_body(json!({ "msg": b64(b"msg a") }))).await;
        assert_eq!(status, 201);
        let np = parse(&body)["nameplate"].as_str().unwrap().to_string();
        let path = |rest: &str| format!("/v1/pairings/{np}{rest}");

        let (status, body) = call(&f.app, "POST", &path("/join"), "", Body::empty()).await;
        assert_eq!(status, 200);
        assert_eq!(parse(&body)["msg"], b64(b"msg a"));
        let ticket = parse(&body)["ticket"].as_str().unwrap().to_string();
        assert_eq!(call(&f.app, "POST", &path("/join"), "", Body::empty()).await.0, 409, "one joiner");
        let result = format!("{}?ticket={ticket}", path("/result"));
        assert_eq!(call(&f.app, "GET", &result, "", Body::empty()).await.0, 202);

        let answer = json!({ "ticket": ticket, "msg": b64(b"msg b"), "confirm": b64(b"proof"), "name": b64(b"k-sealed") });
        assert_eq!(call(&f.app, "POST", &path("/answer"), "", json_body(answer)).await.0, 204);
        let (_, body) = call(&f.app, "GET", &path(""), &a, Body::empty()).await;
        assert_eq!(parse(&body)["answer"]["confirm"], b64(b"proof"));
        assert_eq!(parse(&body)["answer"]["name"], b64(b"k-sealed"));
        let approval = json_body(json!({ "payload": b64(b"key"), "label": b64(b"new") }));
        let (status, _) = call(&f.app, "POST", &path("/approve"), &a, approval).await;
        assert_eq!(status, 200);

        let (status, body) = call(&f.app, "GET", &result, "", Body::empty()).await;
        assert_eq!(status, 200);
        assert_eq!(parse(&body)["payload"], b64(b"key"));
        let token = parse(&body)["token"].as_str().unwrap().to_string();
        let (_, body) = call(&f.app, "GET", "/v1/devices", &token, Body::empty()).await;
        let devices = parse(&body)["devices"].as_array().unwrap().clone();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[1]["label"], b64(b"new"));
        assert_eq!(call(&f.app, "GET", &result, "", Body::empty()).await.0, 404, "the pairing is over");
    }

    #[tokio::test]
    async fn only_the_owner_sees_or_approves_its_pairing_and_unknown_codes_count_as_failures() {
        let f = fixture();
        let (_, a) = f.state.store.add_device(b"a").unwrap();
        let (_, b) = f.state.store.add_device(b"b").unwrap();
        let (_, body) = call(&f.app, "POST", "/v1/pairings", &a, json_body(json!({ "msg": b64(b"m") }))).await;
        let np = parse(&body)["nameplate"].as_str().unwrap().to_string();
        assert_eq!(call(&f.app, "GET", &format!("/v1/pairings/{np}"), &b, Body::empty()).await.0, 404);
        let approve = json_body(json!({ "payload": b64(b"k"), "label": b64(b"l") }));
        assert_eq!(call(&f.app, "POST", &format!("/v1/pairings/{np}/approve"), &a, approve).await.0, 403, "no answer yet");
        for _ in 0..MAX_FAILURES {
            assert_eq!(call(&f.app, "POST", "/v1/pairings/1/join", "", Body::empty()).await.0, 404);
        }
        assert_eq!(call(&f.app, "POST", &format!("/v1/pairings/{np}/join"), "", Body::empty()).await.0, 429);
    }

    #[tokio::test]
    async fn devices_are_listed_and_removing_the_last_one_issues_a_new_first_code() {
        let f = fixture();
        let (a_dev, a) = f.state.store.add_device(b"a").unwrap();
        let (b_dev, _) = f.state.store.add_device(b"b").unwrap();
        assert_eq!(call(&f.app, "PUT", "/v1/me", &a, json_body(json!({ "label": b64(b"renamed") }))).await.0, 204);
        let (_, body) = call(&f.app, "GET", "/v1/devices", &a, Body::empty()).await;
        assert_eq!(parse(&body)["me"], a_dev.id);
        assert_eq!(parse(&body)["devices"][0]["label"], b64(b"renamed"));
        assert_eq!(call(&f.app, "DELETE", &format!("/v1/devices/{}", b_dev.id), &a, Body::empty()).await.0, 204);
        assert_eq!(call(&f.app, "DELETE", "/v1/devices/missing", &a, Body::empty()).await.0, 404);
        assert_eq!(call(&f.app, "DELETE", &format!("/v1/devices/{}", a_dev.id), &a, Body::empty()).await.0, 204);
        assert_eq!(call(&f.app, "GET", "/v1/me", &a, Body::empty()).await.0, 401, "removed itself");
        assert!(f.state.joining.refresh_first_code(0).is_none(), "already issued on removal");
    }
}
