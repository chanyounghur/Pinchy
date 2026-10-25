//! Keeps this device's history identical to the user's other devices
//! through a Pinchy relay, which holds the history.
//!
//! Every local change (a new copy, a recopy that moves an item up, a
//! deletion) is queued in the database and pushed to the relay; a second
//! thread follows the relay's change feed and applies what the other devices
//! did. The relay's retention policy decides what stays; its deletions arrive
//! through the feed like any other.
//!
//! Contents are sealed with XChaCha20-Poly1305 under a key that only the
//! user's devices know; the relay sees ciphertext, sizes and times. Each part
//! is bound to its item id so the relay cannot swap them. Device names travel
//! sealed too.
//!
//! Joining: the first device enters the one-time code the relay prints and
//! creates the sharing key. A later computer enters a code shown by a
//! connected device; the two run SPAKE2 over the relay with it, so the
//! sharing key reaches the new device without the relay being able to read
//! it. A phone scans a QR code that carries the key itself together with a
//! one-time join code. The device token and sharing key are kept in the OS
//! keychain.
//!
//! Files travel as one blob, their contents back to back, and are capped at
//! `MAX_FILES_BYTES` per item; larger ones and folders stay on this device.
//! Received files are kept in the app's own `files` folder, one subfolder per
//! item. Copies the source app marked as concealed (password managers) stay
//! local unless the relay's policy says otherwise.

use crate::appicon::AppIconCache;
use crate::clipboard::{self, CHANGED_EVENT};
use crate::db::{Db, Item, NewItem, Op, Pending};
use crate::platform;
use crate::settings::{SettingsStore, ShareConfig};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use chacha20poly1305::aead::{rand_core::RngCore, Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::{Deserialize, Serialize};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::collections::HashSet;
use std::path::PathBuf;
#[cfg(test)]
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub const STATUS_EVENT: &str = "share-status";
pub const INVITE_EVENT: &str = "share-invite";
/// Header color for items first copied on another device, until the app
/// they came from is found here.
const RECEIVED_COLOR: &str = "#c64735";
const NONCE_LEN: usize = 24;
const POLL_SECS: u64 = 25;
const PAGE: i64 = 200;
const POLICY_REFRESH: Duration = Duration::from_secs(600);
const MAX_NAME_CHARS: usize = 64;
const INVITE_TTL: Duration = Duration::from_secs(600);
const JOIN_TIMEOUT: Duration = Duration::from_secs(60);
const KEYCHAIN_SERVICE: &str = "com.chanyounghur.pastel.share";
const KEYCHAIN_ACCOUNT: &str = "relay";
const INVITER: &[u8] = b"pinchy-inviter";
const JOINER: &[u8] = b"pinchy-joiner";
const KEY_MISMATCH: &str =
    "다른 공유 키로 만든 항목이 있어 열 수 없어요. 서버를 초기화했다면 모든 기기를 다시 연결해 주세요.";
const UNKNOWN_DEVICE: &str = "다른 기기";
const SINCE_KEY: &str = "since";
/// Set once the first sync after connecting has brought the history down
/// and queued this device's own items.
const INITIAL_KEY: &str = "initial_sync_done";
const MB: u64 = 1024 * 1024;
/// Largest total size of the files in one item. Below the relay's default
/// blob limit of 64MB.
pub const MAX_FILES_BYTES: u64 = 50 * MB;
/// Longest received file name, in UTF-8 bytes. File systems allow 255.
const MAX_FILE_NAME_BYTES: usize = 200;

/// The relay's rules, which every device follows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    pub retention_days: u32,
    pub max_storage_mb: u64,
    pub max_blob_mb: u64,
    pub sync_concealed: bool,
    #[serde(default)]
    pub usage: Option<RelayUsage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayUsage {
    pub items: i64,
    pub bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    Off,
    Connecting,
    Connected { devices: i64, policy: Policy },
    Error { message: String },
}

/// Progress of a code this device is showing to invite another one.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum InviteEvent {
    Joined { name: String },
    Failed { message: String },
    Expired,
}

#[derive(Clone, Serialize, Deserialize)]
struct Secrets {
    token: String,
    /// Base64url sharing key.
    key: String,
}

/// Sync status, cached secrets and policy, the running invitation, and
/// generation counters that wake the sync threads.
pub struct ShareState {
    status: Mutex<Status>,
    policy: Mutex<Option<Policy>>,
    generation: Mutex<u64>,
    changed: Condvar,
    outbox: Mutex<u64>,
    outbox_changed: Condvar,
    secrets: Mutex<Option<Secrets>>,
    invite: Mutex<Option<Arc<AtomicBool>>>,
}

impl Default for ShareState {
    fn default() -> Self {
        ShareState {
            status: Mutex::new(Status::Connecting),
            policy: Mutex::new(None),
            generation: Mutex::new(0),
            changed: Condvar::new(),
            outbox: Mutex::new(0),
            outbox_changed: Condvar::new(),
            secrets: Mutex::new(None),
            invite: Mutex::new(None),
        }
    }
}

impl ShareState {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// The relay's policy while this device is connected.
    pub fn policy(&self) -> Option<Policy> {
        self.policy.lock().unwrap().clone()
    }

    fn set(&self, app: &AppHandle, status: Status) {
        let mut current = self.status.lock().unwrap();
        if *current != status {
            *current = status.clone();
            let _ = app.emit(STATUS_EVENT, status);
        }
    }

    fn set_policy(&self, app: &AppHandle, devices: i64, policy: Policy) {
        *self.policy.lock().unwrap() = Some(policy.clone());
        self.set(app, Status::Connected { devices, policy });
    }

    fn generation(&self) -> u64 {
        *self.generation.lock().unwrap()
    }

    fn connection_changed(&self) {
        *self.generation.lock().unwrap() += 1;
        self.changed.notify_all();
        self.wake_pusher();
    }

    /// Sleeps until the connection changes or `timeout` passes.
    fn wait(&self, seen: u64, timeout: Duration) {
        let generation = self.generation.lock().unwrap();
        let _ = self.changed.wait_timeout_while(generation, timeout, |g| *g == seen);
    }

    fn wake_pusher(&self) {
        *self.outbox.lock().unwrap() += 1;
        self.outbox_changed.notify_all();
    }

    fn outbox_generation(&self) -> u64 {
        *self.outbox.lock().unwrap()
    }

    /// Sleeps until something is queued, the connection changes or `timeout`
    /// passes.
    fn wait_outbox(&self, seen: u64, timeout: Duration) {
        let outbox = self.outbox.lock().unwrap();
        let _ = self.outbox_changed.wait_timeout_while(outbox, timeout, |g| *g == seen);
    }

    fn stop_invite(&self) {
        if let Some(cancel) = self.invite.lock().unwrap().take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// Keychain

fn keychain() -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).map_err(|e| format!("키체인을 열 수 없어요. ({e})"))
}

fn load_secrets(state: &ShareState) -> Result<Secrets, String> {
    let mut cached = state.secrets.lock().unwrap();
    if let Some(secrets) = cached.as_ref() {
        return Ok(secrets.clone());
    }
    let stored = match keychain()?.get_password() {
        Ok(stored) => stored,
        Err(keyring::Error::NoEntry) => return Err("저장된 연결 정보가 없어요. 연결을 해제하고 다시 연결해 주세요.".into()),
        Err(e) => return Err(format!("키체인에서 연결 정보를 읽지 못했어요. ({e})")),
    };
    let secrets: Secrets = serde_json::from_str(&stored).map_err(|_| "키체인의 연결 정보가 손상됐어요.")?;
    *cached = Some(secrets.clone());
    Ok(secrets)
}

fn save_secrets(state: &ShareState, secrets: &Secrets) -> Result<(), String> {
    let json = serde_json::to_string(secrets).map_err(|e| e.to_string())?;
    keychain()?.set_password(&json).map_err(|e| format!("키체인에 연결 정보를 저장하지 못했어요. ({e})"))?;
    *state.secrets.lock().unwrap() = Some(secrets.clone());
    Ok(())
}

fn forget_secrets(state: &ShareState) {
    *state.secrets.lock().unwrap() = None;
    if let Ok(entry) = keychain() {
        let _ = entry.delete_credential();
    }
}

// ---------------------------------------------------------------------------
// Sealing

struct Key(XChaCha20Poly1305);

fn generate_key() -> String {
    URL_SAFE_NO_PAD.encode(XChaCha20Poly1305::generate_key(&mut OsRng))
}

impl Key {
    fn parse(text: &str) -> Result<Self, String> {
        let bytes = URL_SAFE_NO_PAD.decode(text.trim()).map_err(|_| "공유 키 형식이 올바르지 않아요.")?;
        Self::from_bytes(&bytes)
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        XChaCha20Poly1305::new_from_slice(bytes).map(Key).map_err(|_| "공유 키 길이가 올바르지 않아요.".into())
    }

    fn seal(&self, part: &str, id: &str, plaintext: &[u8]) -> Vec<u8> {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let aad = format!("pinchy/v1/{part}/{id}");
        let sealed = self
            .0
            .encrypt(&nonce, Payload { msg: plaintext, aad: aad.as_bytes() })
            .expect("encryption cannot fail for in-memory buffers");
        [nonce.as_slice(), &sealed].concat()
    }

    fn open(&self, part: &str, id: &str, sealed: &[u8]) -> Option<Vec<u8>> {
        if sealed.len() < NONCE_LEN {
            return None;
        }
        let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
        let aad = format!("pinchy/v1/{part}/{id}");
        self.0.decrypt(XNonce::from_slice(nonce), Payload { msg: ciphertext, aad: aad.as_bytes() }).ok()
    }

    fn seal_label(&self, name: &str) -> String {
        STANDARD.encode(self.seal("label", "", name.as_bytes()))
    }

    fn open_label(&self, label: &str) -> Option<String> {
        let sealed = STANDARD.decode(label).ok()?;
        String::from_utf8(self.open("label", "", &sealed)?).ok().and_then(|n| clean_name(&n))
    }
}

/// The sealed description of an item. Image and file bytes travel as a
/// separate blob that never changes; the envelope is replaced on every
/// recopy so the copy time travels with it.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Meta {
    v: u32,
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    /// For files, their names and sizes in blob order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    files: Option<Vec<FileMeta>>,
    /// Name of the device the item was first copied on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from: Option<String>,
    /// Name of the app it was copied from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app: Option<String>,
    /// When it was last copied, ms since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    copied_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct FileMeta {
    name: String,
    size: u64,
}

/// The files of a `files` item, checked against the size cap without
/// reading them.
fn file_list(content: &str) -> Result<Vec<(PathBuf, FileMeta)>, String> {
    let paths: Vec<String> = serde_json::from_str(content).map_err(|_| "파일 목록을 읽지 못했어요.")?;
    let mut files = Vec::with_capacity(paths.len());
    let mut total = 0;
    for path in paths {
        let path = PathBuf::from(path);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let meta = std::fs::metadata(&path).map_err(|_| format!("파일을 찾을 수 없어요. ({name})"))?;
        if !meta.is_file() {
            return Err(format!("폴더는 보낼 수 없어요. 파일만 보낼 수 있어요. ({name})"));
        }
        total += meta.len();
        files.push((path, FileMeta { name, size: meta.len() }));
    }
    if files.is_empty() {
        return Err("보낼 파일이 없어요.".into());
    }
    if total > MAX_FILES_BYTES {
        return Err(format!(
            "파일이 너무 커요. 한 번에 {}MB까지 보낼 수 있어요. (지금 {:.1}MB)",
            MAX_FILES_BYTES / MB,
            total as f64 / MB as f64
        ));
    }
    Ok(files)
}

/// Reads the files of a `files` item into one blob.
fn read_files(content: &str) -> Result<(Vec<FileMeta>, Vec<u8>), String> {
    let list = file_list(content)?;
    let mut blob = Vec::with_capacity(list.iter().map(|(_, f)| f.size as usize).sum());
    let mut files = Vec::with_capacity(list.len());
    for (path, mut file) in list {
        let bytes = std::fs::read(&path).map_err(|_| format!("파일을 읽지 못했어요. ({})", file.name))?;
        file.size = bytes.len() as u64;
        blob.extend_from_slice(&bytes);
        files.push(file);
    }
    if blob.len() as u64 > MAX_FILES_BYTES {
        return Err(format!("파일이 너무 커요. 한 번에 {}MB까지 보낼 수 있어요.", MAX_FILES_BYTES / MB));
    }
    Ok((files, blob))
}

/// Splits a received blob back into named files. Names are made safe to
/// create on macOS and Windows, and unique within the item.
fn split_files(files: &[FileMeta], blob: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    if files.is_empty() || files.iter().map(|f| f.size).sum::<u64>() != blob.len() as u64 {
        return None;
    }
    let mut out: Vec<(String, Vec<u8>)> = Vec::with_capacity(files.len());
    let mut offset = 0;
    for file in files {
        let end = offset + file.size as usize;
        let base = safe_file_name(&file.name);
        let mut name = base.clone();
        let mut n = 2;
        while out.iter().any(|(taken, _)| taken.eq_ignore_ascii_case(&name)) {
            name = numbered(&base, n);
            n += 1;
        }
        out.push((name, blob[offset..end].to_vec()));
        offset = end;
    }
    Some(out)
}

fn safe_file_name(name: &str) -> String {
    let mut name: String = name
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    // Windows drops trailing dots and spaces.
    name = name.trim_end_matches(['.', ' ']).trim_start().to_string();
    if name.is_empty() {
        name = "파일".into();
    }
    let stem = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.ends_with(|c: char| c.is_ascii_digit()));
    if reserved {
        name.insert(0, '_');
    }
    if name.len() > MAX_FILE_NAME_BYTES {
        let (stem, ext) = split_extension(&name);
        let keep = MAX_FILE_NAME_BYTES.saturating_sub(ext.len());
        let mut cut = keep.min(stem.len());
        while !stem.is_char_boundary(cut) {
            cut -= 1;
        }
        name = format!("{}{ext}", &stem[..cut]);
    }
    name
}

/// `report.pdf` → `report (2).pdf`.
fn numbered(name: &str, n: u32) -> String {
    let (stem, ext) = split_extension(name);
    format!("{stem} ({n}){ext}")
}

/// Splits off the extension, dot included; short extensions only.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 && name.len() - dot <= 16 => name.split_at(dot),
        _ => (name, ""),
    }
}

/// Trims a device name and caps its length; empty means none.
fn clean_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().take(MAX_NAME_CHARS).collect();
    let name = name.trim().to_string();
    (!name.is_empty()).then_some(name)
}

fn random_digits(n: usize) -> String {
    let mut out = String::with_capacity(n);
    while out.len() < n {
        let mut byte = [0u8; 1];
        OsRng.fill_bytes(&mut byte);
        if byte[0] < 250 {
            out.push(char::from(b'0' + byte[0] % 10));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Codes

#[derive(Debug, PartialEq)]
enum Code {
    /// The relay's first-device code, `1234-5678-9012`.
    First(String),
    /// A connected device's invitation, `37-1234-5678`.
    Invite { nameplate: String, password: String },
}

fn parse_code(text: &str) -> Result<Code, String> {
    let groups: Vec<&str> = text.split(|c: char| !c.is_ascii_digit()).filter(|g| !g.is_empty()).collect();
    match groups.as_slice() {
        [a, b, c] if a.len() == 4 && b.len() == 4 && c.len() == 4 => Ok(Code::First(format!("{a}{b}{c}"))),
        [np, b, c] if (2..=3).contains(&np.len()) && b.len() == 4 && c.len() == 4 => {
            Ok(Code::Invite { nameplate: np.to_string(), password: format!("{b}{c}") })
        }
        _ => Err("연결 코드 형식이 올바르지 않아요. 예: 37-1234-5678".into()),
    }
}

pub fn normalize_server(server: &str) -> Result<String, String> {
    let server = server.trim().trim_end_matches('/').to_string();
    let local = server.starts_with("http://127.0.0.1:") || server.starts_with("http://localhost:");
    if !(server.starts_with("https://") || local) || server.contains(char::is_whitespace) {
        return Err("서버 주소는 https://로 시작해야 해요.".into());
    }
    Ok(server)
}

// ---------------------------------------------------------------------------
// HTTP

struct HttpError {
    status: Option<u16>,
    message: String,
}

impl From<HttpError> for String {
    fn from(e: HttpError) -> String {
        e.message
    }
}

struct Http {
    client: Client,
    server: String,
    token: Option<String>,
}

impl Http {
    fn new(server: &str, token: Option<String>) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("Pinchy/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Http { client, server: normalize_server(server)?, token })
    }

    fn get(&self, path: &str) -> RequestBuilder {
        self.client.get(format!("{}{path}", self.server))
    }

    fn post(&self, path: &str, body: serde_json::Value) -> RequestBuilder {
        self.client
            .post(format!("{}{path}", self.server))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
    }

    fn put(&self, path: &str) -> RequestBuilder {
        self.client.put(format!("{}{path}", self.server))
    }

    fn put_json(&self, path: &str, body: serde_json::Value) -> RequestBuilder {
        self.put(path).header(reqwest::header::CONTENT_TYPE, "application/json").body(body.to_string())
    }

    fn delete(&self, path: &str) -> RequestBuilder {
        self.client.delete(format!("{}{path}", self.server))
    }

    fn send(&self, request: RequestBuilder) -> Result<Response, HttpError> {
        let request = match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        let response = request
            .send()
            .map_err(|e| HttpError { status: None, message: format!("서버에 연결할 수 없어요. ({e})") })?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let code = status.as_u16();
        let message = match code {
            401 => "이 기기의 연결이 해제됐어요. 연결을 해제한 뒤 새 코드로 다시 연결해 주세요.".into(),
            413 => "너무 커서 올릴 수 없어요.".into(),
            429 => "실패한 시도가 많아 잠시 막혔어요. 10분 뒤 다시 시도해 주세요.".into(),
            code => format!("서버 오류예요. (HTTP {code})"),
        };
        Err(HttpError { status: Some(code), message })
    }

    fn json<T: for<'de> Deserialize<'de>>(response: Response) -> Result<T, String> {
        let bytes = response.bytes().map_err(|e| format!("서버 응답을 받지 못했어요. ({e})"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("서버 응답을 읽지 못했어요. ({e})"))
    }
}

fn decode(field: &str) -> Result<Vec<u8>, String> {
    STANDARD.decode(field).map_err(|_| "서버 응답을 읽지 못했어요.".into())
}

// ---------------------------------------------------------------------------
// Joining

#[derive(Deserialize)]
struct Joined {
    device_id: String,
    token: String,
}

#[derive(Deserialize)]
struct PairingOffer {
    msg: String,
    ticket: String,
}

#[derive(Deserialize)]
struct PairingResult {
    device_id: String,
    token: String,
    payload: String,
}

/// Joins with the relay's first-device code and creates the sharing key.
fn join_first(server: &str, name: &str, code: &str) -> Result<(String, Secrets), String> {
    let http = Http::new(server, None)?;
    let key = generate_key();
    let label = Key::parse(&key)?.seal_label(name);
    let response = http
        .send(http.post("/v1/first", serde_json::json!({ "code": code, "label": label })))
        .map_err(|e| match e.status {
            Some(403) => "첫 연결 코드가 맞지 않거나 만료됐어요. 서버 로그의 최신 코드를 확인해 주세요.".into(),
            _ => e.message,
        })?;
    let joined: Joined = Http::json(response)?;
    Ok((joined.device_id, Secrets { token: joined.token, key }))
}

/// Joins with a code shown on a connected device.
fn join_invited(server: &str, name: &str, nameplate: &str, password: &str) -> Result<(String, Secrets), String> {
    let http = Http::new(server, None)?;
    let gone = |e: HttpError| match e.status {
        Some(404) => "코드가 맞지 않거나 만료됐어요. 연결된 기기에서 새 코드를 만들어 주세요.".to_string(),
        Some(409) => "이미 사용된 코드예요. 연결된 기기에서 새 코드를 만들어 주세요.".to_string(),
        _ => e.message,
    };
    let offer: PairingOffer =
        Http::json(http.send(http.post(&format!("/v1/pairings/{nameplate}/join"), serde_json::json!({}))).map_err(gone)?)?;
    let (spake, msg_b) =
        Spake2::<Ed25519Group>::start_b(&Password::new(password.as_bytes()), &Identity::new(INVITER), &Identity::new(JOINER));
    let pake = spake.finish(&decode(&offer.msg)?).map_err(|_| "코드가 맞지 않거나 만료됐어요.")?;
    let pake = Key::from_bytes(&pake)?;
    http.send(http.post(
        &format!("/v1/pairings/{nameplate}/answer"),
        serde_json::json!({
            "ticket": offer.ticket,
            "msg": STANDARD.encode(msg_b),
            "confirm": STANDARD.encode(pake.seal("pair-confirm", nameplate, JOINER)),
            "name": STANDARD.encode(pake.seal("pair-name", nameplate, name.as_bytes())),
        }),
    ))
    .map_err(gone)?;
    let deadline = Instant::now() + JOIN_TIMEOUT;
    while Instant::now() < deadline {
        let response = http
            .send(http.get(&format!("/v1/pairings/{nameplate}/result?ticket={}", offer.ticket)))
            .map_err(gone)?;
        if response.status() == reqwest::StatusCode::ACCEPTED {
            std::thread::sleep(Duration::from_millis(700));
            continue;
        }
        let result: PairingResult = Http::json(response)?;
        let key = pake
            .open("pair-key", nameplate, &decode(&result.payload)?)
            .and_then(|k| String::from_utf8(k).ok())
            .ok_or("공유 키를 받지 못했어요. 새 코드로 다시 시도해 주세요.")?;
        Key::parse(&key)?;
        return Ok((result.device_id, Secrets { token: result.token, key }));
    }
    Err("연결된 기기가 응답하지 않아요. 그 기기에서 코드 화면이 열려 있는지 확인해 주세요.".into())
}

#[derive(Deserialize)]
struct Created {
    nameplate: String,
}

#[derive(Deserialize)]
struct AnswerView {
    msg: String,
    confirm: String,
    name: String,
}

#[derive(Deserialize)]
struct PairingStatus {
    answer: Option<AnswerView>,
}

/// A code this device shows so another device can join.
struct Invitation {
    nameplate: String,
    spake: Option<Spake2<Ed25519Group>>,
    deadline: Instant,
}

enum InviteStep {
    Waiting,
    Joined(String),
}

impl Invitation {
    /// Opens a pairing on the relay and returns it with the code to show.
    fn start(relay: &Relay) -> Result<(Self, String), String> {
        let password = random_digits(8);
        let (spake, msg_a) =
            Spake2::<Ed25519Group>::start_a(&Password::new(password.as_bytes()), &Identity::new(INVITER), &Identity::new(JOINER));
        let created: Created = Http::json(
            relay.http.send(relay.http.post("/v1/pairings", serde_json::json!({ "msg": STANDARD.encode(msg_a) })))?,
        )?;
        let code = format!("{}-{}-{}", created.nameplate, &password[..4], &password[4..]);
        Ok((Invitation { nameplate: created.nameplate, spake: Some(spake), deadline: Instant::now() + INVITE_TTL }, code))
    }

    /// Checks once for the joiner. Approves it if it proved it knew the code;
    /// otherwise ends the pairing so the code cannot be guessed again.
    fn poll(&mut self, relay: &Relay) -> Result<InviteStep, InviteEvent> {
        if Instant::now() >= self.deadline {
            self.cancel(relay);
            return Err(InviteEvent::Expired);
        }
        let failed = |message: String| InviteEvent::Failed { message };
        let response = relay.http.send(relay.http.get(&format!("/v1/pairings/{}", self.nameplate))).map_err(|e| {
            if e.status == Some(404) { InviteEvent::Expired } else { failed(e.message) }
        })?;
        let Some(answer) = Http::json::<PairingStatus>(response).map_err(failed)?.answer else {
            return Ok(InviteStep::Waiting);
        };
        let spake = self.spake.take().ok_or_else(|| failed("이미 처리한 코드예요.".into()))?;
        let np = self.nameplate.clone();
        let verified = decode(&answer.msg)
            .ok()
            .and_then(|msg| spake.finish(&msg).ok())
            .and_then(|k| Key::from_bytes(&k).ok())
            .filter(|pake| {
                decode(&answer.confirm).ok().and_then(|c| pake.open("pair-confirm", &np, &c)).as_deref() == Some(JOINER)
            });
        let Some(pake) = verified else {
            self.cancel(relay);
            return Err(failed("입력한 코드가 맞지 않아 연결을 취소했어요. 새 코드로 다시 시도해 주세요.".into()));
        };
        let name = decode(&answer.name)
            .ok()
            .and_then(|n| pake.open("pair-name", &np, &n))
            .and_then(|n| String::from_utf8(n).ok())
            .and_then(|n| clean_name(&n))
            .unwrap_or_else(|| UNKNOWN_DEVICE.into());
        relay
            .http
            .send(relay.http.post(
                &format!("/v1/pairings/{np}/approve"),
                serde_json::json!({
                    "payload": STANDARD.encode(pake.seal("pair-key", &np, relay.key_text.as_bytes())),
                    "label": relay.key.seal_label(&name),
                }),
            ))
            .map_err(|e| failed(e.message))?;
        Ok(InviteStep::Joined(name))
    }

    fn cancel(&self, relay: &Relay) {
        let _ = relay.http.send(relay.http.delete(&format!("/v1/pairings/{}", self.nameplate)));
    }
}

/// What a phone scans: the relay, a one-time join code and the sharing key.
/// The key rides in the URL fragment, which browsers never send to the
/// server.
fn join_link(server: &str, code: &str, key: &str) -> String {
    format!("{server}/#join?code={code}&key={key}")
}

fn qr_svg(text: &str) -> Result<String, String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| format!("QR 코드를 만들지 못했어요. ({e})"))?;
    Ok(code
        .render::<svg::Color>()
        .quiet_zone(false)
        .min_dimensions(240, 240)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

// ---------------------------------------------------------------------------
// Connected session

struct Relay {
    http: Http,
    key: Key,
    key_text: String,
    name: Option<String>,
    device_id: String,
}

#[derive(Debug, Deserialize)]
struct Me {
    devices: i64,
}

#[derive(Deserialize)]
struct DeviceRecord {
    id: String,
    label: String,
    created_at: i64,
    last_seen: Option<i64>,
}

#[derive(Deserialize)]
struct DeviceList {
    me: String,
    devices: Vec<DeviceRecord>,
}

#[derive(Debug, Serialize)]
pub struct DeviceView {
    id: String,
    name: String,
    created_at: i64,
    last_seen: Option<i64>,
    me: bool,
}

/// One entry of the relay's change feed.
#[derive(Debug, Deserialize)]
struct Change {
    id: String,
    seq: i64,
    origin: String,
    updated_at: i64,
    deleted: bool,
    #[serde(default)]
    envelope: Option<String>,
    #[serde(default)]
    blob_size: Option<i64>,
}

#[derive(Deserialize)]
struct ChangePage {
    items: Vec<Change>,
    next: i64,
    more: bool,
}

#[derive(Deserialize)]
struct PutResponse {
    blob_pending: bool,
}

#[derive(Deserialize)]
struct JoinCode {
    code: String,
    expires_in: u64,
}

enum ReceiveError {
    /// Sealed with a different key. Skipped; the relay keeps it.
    Locked,
    /// Can never be stored, e.g. an unknown kind. Skipped.
    Invalid(String),
    /// Retry later.
    Network(String),
}

impl Relay {
    fn new(config: &ShareConfig, secrets: &Secrets) -> Result<Self, String> {
        Ok(Relay {
            http: Http::new(&config.server, Some(secrets.token.clone()))?,
            key: Key::parse(&secrets.key)?,
            key_text: secrets.key.clone(),
            name: clean_name(&config.name),
            device_id: config.device_id.clone(),
        })
    }

    fn me(&self) -> Result<Me, String> {
        Ok(Http::json(self.http.send(self.http.get("/v1/me"))?)?)
    }

    fn policy(&self) -> Result<Policy, String> {
        Ok(Http::json(self.http.send(self.http.get("/v1/policy"))?)?)
    }

    fn rename(&self, name: &str) -> Result<(), String> {
        let body = serde_json::json!({ "label": self.key.seal_label(name) });
        self.http.send(self.http.put_json("/v1/me", body))?;
        Ok(())
    }

    fn devices(&self) -> Result<Vec<DeviceView>, String> {
        let list: DeviceList = Http::json(self.http.send(self.http.get("/v1/devices"))?)?;
        Ok(list
            .devices
            .into_iter()
            .map(|d| DeviceView {
                name: self.key.open_label(&d.label).unwrap_or_else(|| UNKNOWN_DEVICE.into()),
                me: d.id == list.me,
                id: d.id,
                created_at: d.created_at,
                last_seen: d.last_seen,
            })
            .collect())
    }

    fn remove_device(&self, id: &str) -> Result<(), String> {
        self.http.send(self.http.delete(&format!("/v1/devices/{id}")))?;
        Ok(())
    }

    fn join_code(&self) -> Result<JoinCode, String> {
        Ok(Http::json(self.http.send(self.http.post("/v1/join-codes", serde_json::json!({})))?)?)
    }

    fn cancel_join_code(&self) {
        let _ = self.http.send(self.http.delete("/v1/join-codes"));
    }

    /// Seals `item` and puts it on the relay under its id, with its blob the
    /// first time. `Ok(false)` when the relay deleted the item meanwhile.
    fn put(&self, item: &Item) -> Result<bool, String> {
        let from = item.origin_device.clone().or_else(|| self.name.clone());
        let base = Meta {
            v: 2,
            kind: item.kind.clone(),
            text: None,
            files: None,
            from,
            app: item.source_app.clone(),
            copied_at: Some(item.created_at),
        };
        let (meta, blob) = match item.kind.as_str() {
            "text" | "link" => (Meta { text: Some(item.content.clone()), ..base }, None),
            "image" => {
                let bytes = std::fs::read(&item.content).map_err(|_| "이미지 파일을 찾을 수 없어요.")?;
                (base, Some(bytes))
            }
            "files" => {
                let (files, bytes) = read_files(&item.content)?;
                (Meta { files: Some(files), ..base }, Some(bytes))
            }
            kind => return Err(format!("동기화할 수 없는 항목이에요. ({kind})")),
        };
        let id = &item.sync_id;
        let envelope = self.key.seal("meta", id, &serde_json::to_vec(&meta).map_err(|e| e.to_string())?);
        let body = serde_json::json!({ "envelope": STANDARD.encode(envelope), "has_blob": blob.is_some() });
        let response = match self.http.send(self.http.put_json(&format!("/v1/items/{id}"), body)) {
            Ok(response) => response,
            Err(HttpError { status: Some(410), .. }) => return Ok(false),
            Err(HttpError { status: Some(409), .. }) => {
                return Err("서버의 항목과 모양이 달라 올리지 못했어요.".into())
            }
            Err(e) => return Err(e.message),
        };
        let put: PutResponse = Http::json(response)?;
        if let (true, Some(bytes)) = (put.blob_pending, blob) {
            self.http.send(
                self.http
                    .put(&format!("/v1/items/{id}/blob"))
                    .timeout(Duration::from_secs(600))
                    .body(self.key.seal("blob", id, &bytes)),
            )?;
        }
        Ok(true)
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        self.http.send(self.http.delete(&format!("/v1/items/{id}")))?;
        Ok(())
    }

    fn delete_all(&self) -> Result<(), String> {
        self.http.send(self.http.delete("/v1/items"))?;
        Ok(())
    }

    /// The change feed after `since`. `Ok(None)` when the relay no longer
    /// remembers that far back and the history must be fetched from zero.
    fn changes(&self, since: i64, wait: u64) -> Result<Option<ChangePage>, String> {
        let request = self
            .http
            .get(&format!("/v1/changes?since={since}&wait={wait}&limit={PAGE}"))
            .timeout(Duration::from_secs(wait + 30));
        match self.http.send(request) {
            Ok(response) => Ok(Some(Http::json(response)?)),
            Err(HttpError { status: Some(409), .. }) => Ok(None),
            Err(e) => Err(e.message),
        }
    }

    /// Opens a change's envelope: everything but the blob.
    fn open_meta(&self, change: &Change) -> Result<Head, ReceiveError> {
        let envelope = change.envelope.as_deref().ok_or_else(|| ReceiveError::Invalid("no envelope".into()))?;
        let envelope = STANDARD.decode(envelope).map_err(|_| ReceiveError::Invalid("envelope is not base64".into()))?;
        let meta = self.key.open("meta", &change.id, &envelope).ok_or(ReceiveError::Locked)?;
        let meta: Meta = serde_json::from_slice(&meta).map_err(|e| ReceiveError::Invalid(e.to_string()))?;
        Ok(Head {
            from: meta.from.as_deref().and_then(clean_name).unwrap_or_else(|| UNKNOWN_DEVICE.into()),
            app: meta.app.as_deref().and_then(clean_name),
            copied_at: meta.copied_at.unwrap_or(change.updated_at),
            meta,
        })
    }

    /// Downloads and opens a change. Images and files are written to disk by
    /// `into_item`, under the history's file lock.
    fn open(&self, change: &Change) -> Result<Opened, ReceiveError> {
        let Head { meta, from, app, copied_at } = self.open_meta(change)?;
        let content = match (meta.kind.as_str(), meta.text, meta.files) {
            ("text" | "link", Some(text), _) if !text.trim().is_empty() => Content::Text(text),
            ("image", _, _) if change.blob_size.is_some() => {
                let png = self.blob(&change.id)?;
                let (w, h) = png_size(&png).ok_or_else(|| ReceiveError::Invalid("blob is not a PNG image".into()))?;
                Content::Png(png, w, h)
            }
            ("files", _, Some(files)) if change.blob_size.is_some() => {
                let blob = self.blob(&change.id)?;
                let files = split_files(&files, &blob)
                    .ok_or_else(|| ReceiveError::Invalid("file sizes do not match the blob".into()))?;
                Content::Files(files)
            }
            (kind, _, _) => return Err(ReceiveError::Invalid(format!("unsupported item kind {kind:?}"))),
        };
        Ok(Opened { from, app, copied_at, content })
    }

    fn blob(&self, id: &str) -> Result<Vec<u8>, ReceiveError> {
        let response = self
            .http
            .send(self.http.get(&format!("/v1/items/{id}/blob")).timeout(Duration::from_secs(600)))
            .map_err(|e| ReceiveError::Network(e.message))?;
        let sealed = response.bytes().map_err(|e| ReceiveError::Network(e.to_string()))?;
        self.key.open("blob", id, &sealed).ok_or(ReceiveError::Locked)
    }
}

/// A change's envelope, opened.
struct Head {
    meta: Meta,
    from: String,
    app: Option<String>,
    copied_at: i64,
}

enum Content {
    Text(String),
    Png(Vec<u8>, u32, u32),
    Files(Vec<(String, Vec<u8>)>),
}

struct Opened {
    from: String,
    app: Option<String>,
    copied_at: i64,
    content: Content,
}

impl Opened {
    /// Builds the local row. Items first copied on this device keep their
    /// own look; others show the app they came from when it is installed
    /// here too, else the device they came from.
    fn into_item(self, dirs: &Dirs, icons: &mut AppIconCache, own_name: Option<&str>) -> Option<NewItem> {
        let mut item = match self.content {
            Content::Text(text) => clipboard::text_item(text),
            Content::Png(png, w, h) => clipboard::png_item(&dirs.images, &png, w, h)?,
            Content::Files(files) => clipboard::received_files_item(&dirs.files, &files)?,
        };
        let look = self.app.as_deref().and_then(|app| icons.look_by_name(app));
        item.source_app = self.app.clone().or_else(|| Some(self.from.clone()));
        match look {
            Some(look) => {
                item.app_icon = look.icon;
                item.app_color = look.color;
            }
            None => item.app_color = Some(RECEIVED_COLOR.into()),
        }
        if own_name != Some(self.from.as_str()) {
            item.origin_device = Some(self.from);
        }
        Some(item)
    }
}

fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    reader.into_dimensions().ok()
}

/// The saved connection, if this device is connected.
fn session(app: &AppHandle) -> Result<Option<Relay>, String> {
    let Some(config) = app.state::<SettingsStore>().get().share.filter(|c| !c.device_id.is_empty()) else {
        return Ok(None);
    };
    let secrets = load_secrets(&app.state::<ShareState>())?;
    Relay::new(&config, &secrets).map(Some)
}

fn connected(app: &AppHandle) -> Result<Relay, String> {
    session(app)?.ok_or_else(|| "기기 간 동기화를 먼저 연결해 주세요.".into())
}

/// Drops this device's connection locally. The history stays as it is.
fn forget(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<ShareState>();
    state.stop_invite();
    app.state::<SettingsStore>().update(|s| s.share = None)?;
    forget_secrets(&state);
    *state.policy.lock().unwrap() = None;
    let db = app.state::<Db>();
    let _ = db.set_sync_value(SINCE_KEY, None);
    let _ = db.set_sync_value(INITIAL_KEY, None);
    let _ = db.clear_outbox();
    state.set(app, Status::Off);
    state.connection_changed();
    Ok(())
}

/// Starts following the relay from scratch: the whole history comes down,
/// then everything here goes up.
fn begin_sync(app: &AppHandle) -> Result<(), String> {
    let db = app.state::<Db>();
    db.clear_outbox().map_err(|e| e.to_string())?;
    db.set_sync_value(SINCE_KEY, None).map_err(|e| e.to_string())?;
    db.set_sync_value(INITIAL_KEY, None).map_err(|e| e.to_string())?;
    Ok(())
}

fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> impl std::future::Future<Output = Result<T, String>> {
    async move { tauri::async_runtime::spawn_blocking(work).await.map_err(|e| e.to_string())? }
}

// ---------------------------------------------------------------------------
// Commands

#[tauri::command]
pub fn share_status(state: tauri::State<ShareState>) -> Status {
    state.status()
}

/// Default name for this device: the computer's name.
#[tauri::command]
pub fn default_device_name() -> String {
    platform::computer_name().unwrap_or_default()
}

/// Connects this device with a first-device code or an invitation code.
#[tauri::command]
pub async fn join_share(app: AppHandle, name: String, server: String, code: String) -> Result<Status, String> {
    blocking(move || {
        let server = normalize_server(&server)?;
        let name = clean_name(&name).or_else(platform::computer_name).unwrap_or_else(|| UNKNOWN_DEVICE.into());
        let (device_id, secrets) = match parse_code(&code)? {
            Code::First(code) => join_first(&server, &name, &code)?,
            Code::Invite { nameplate, password } => join_invited(&server, &name, &nameplate, &password)?,
        };
        let state = app.state::<ShareState>();
        save_secrets(&state, &secrets)?;
        let config = ShareConfig { name, server, device_id };
        begin_sync(&app)?;
        app.state::<SettingsStore>().update(|s| s.share = Some(config.clone()))?;
        let relay = Relay::new(&config, &secrets)?;
        let status = match (relay.me(), relay.policy()) {
            (Ok(me), Ok(policy)) => {
                *state.policy.lock().unwrap() = Some(policy.clone());
                Status::Connected { devices: me.devices, policy }
            }
            (Err(message), _) | (_, Err(message)) => Status::Error { message },
        };
        state.set(&app, status.clone());
        state.connection_changed();
        Ok(status)
    })
    .await
}

/// Shows a code another computer can join with. Progress arrives as
/// `share-invite` events until the code is used, fails or expires.
#[tauri::command]
pub async fn start_invite(app: AppHandle) -> Result<String, String> {
    blocking(move || {
        let relay = connected(&app)?;
        let (mut invitation, code) = Invitation::start(&relay)?;
        let state = app.state::<ShareState>();
        state.stop_invite();
        let cancel = Arc::new(AtomicBool::new(false));
        *state.invite.lock().unwrap() = Some(cancel.clone());
        std::thread::spawn(move || loop {
            if cancel.load(Ordering::Relaxed) {
                invitation.cancel(&relay);
                return;
            }
            match invitation.poll(&relay) {
                Ok(InviteStep::Waiting) => std::thread::sleep(Duration::from_millis(800)),
                Ok(InviteStep::Joined(name)) => {
                    refresh_connected(&app, &relay);
                    let _ = app.emit(INVITE_EVENT, InviteEvent::Joined { name });
                    return;
                }
                Err(event) => {
                    let _ = app.emit(INVITE_EVENT, event);
                    return;
                }
            }
        });
        Ok(code)
    })
    .await
}

#[tauri::command]
pub fn cancel_invite(state: tauri::State<ShareState>) {
    state.stop_invite();
}

/// What a phone scans to join: a link with a one-time code and the sharing
/// key, rendered as a QR code.
#[derive(Debug, Serialize)]
pub struct PhoneInvite {
    link: String,
    svg: String,
    expires_in: u64,
}

#[tauri::command]
pub async fn start_phone_invite(app: AppHandle) -> Result<PhoneInvite, String> {
    blocking(move || {
        let relay = connected(&app)?;
        let code = relay.join_code()?;
        let link = join_link(&relay.http.server, &code.code, &relay.key_text);
        Ok(PhoneInvite { svg: qr_svg(&link)?, link, expires_in: code.expires_in })
    })
    .await
}

#[tauri::command]
pub async fn cancel_phone_invite(app: AppHandle) -> Result<(), String> {
    blocking(move || {
        if let Ok(Some(relay)) = session(&app) {
            relay.cancel_join_code();
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn share_devices(app: AppHandle) -> Result<Vec<DeviceView>, String> {
    blocking(move || connected(&app)?.devices()).await
}

/// Disconnects another device, or this one when `id` is its own.
#[tauri::command]
pub async fn remove_share_device(app: AppHandle, id: String) -> Result<(), String> {
    blocking(move || {
        let relay = connected(&app)?;
        relay.remove_device(&id)?;
        if id == relay.device_id {
            return forget(&app);
        }
        refresh_connected(&app, &relay);
        Ok(())
    })
    .await
}

/// Disconnects this device. Returns a warning when the relay could not be
/// told, in which case another device should remove this one.
#[tauri::command]
pub async fn disconnect_share(app: AppHandle) -> Result<Option<String>, String> {
    blocking(move || {
        let warning = match session(&app) {
            Ok(Some(relay)) => relay.remove_device(&relay.device_id).err(),
            Ok(None) => None,
            Err(e) => Some(e),
        };
        forget(&app)?;
        Ok(warning.map(|e| format!("이 기기의 연결 정보는 지웠지만 서버에 알리지 못했어요. 다른 기기에서 이 기기를 해제해 주세요. ({e})")))
    })
    .await
}

/// Renames this device for the device list and for items it copies.
#[tauri::command]
pub async fn set_share_name(app: AppHandle, name: String) -> Result<String, String> {
    blocking(move || {
        let name = clean_name(&name).ok_or("기기 이름을 입력해 주세요.")?;
        let relay = connected(&app)?;
        relay.rename(&name)?;
        app.state::<SettingsStore>().update(|s| {
            if let Some(share) = s.share.as_mut() {
                share.name = name.clone();
            }
        })?;
        Ok(name)
    })
    .await
}

/// Deletes the history on the relay, for every device. Nothing to do when
/// this device is not connected.
pub fn clear_remote(app: &AppHandle) -> Result<(), String> {
    if let Some(relay) = session(app)? {
        relay.delete_all().map_err(|e| format!("서버의 히스토리를 지우지 못했어요. {e}"))?;
    }
    Ok(())
}

/// Called after the history changed locally so the pusher sends the queue.
pub fn changed(app: &AppHandle) {
    app.state::<ShareState>().wake_pusher();
}

fn refresh_connected(app: &AppHandle, relay: &Relay) {
    let state = app.state::<ShareState>();
    match (relay.me(), relay.policy()) {
        (Ok(me), Ok(policy)) => state.set_policy(app, me.devices, policy),
        (Err(message), _) | (_, Err(message)) => state.set(app, Status::Error { message }),
    }
}

// ---------------------------------------------------------------------------
// Sync

/// Where received images and files are written, and app icons are cached.
struct Dirs {
    images: PathBuf,
    files: PathBuf,
    icons: PathBuf,
}

pub fn start(app: AppHandle, images_dir: PathBuf, files_dir: PathBuf, icons_dir: PathBuf) {
    let pusher = app.clone();
    std::thread::Builder::new()
        .name("share-push".into())
        .spawn(move || push_loop(pusher))
        .expect("spawn share pusher");
    let dirs = Dirs { images: images_dir, files: files_dir, icons: icons_dir };
    std::thread::Builder::new()
        .name("share-pull".into())
        .spawn(move || pull_loop(app, dirs))
        .expect("spawn share puller");
}

/// Whether a queued item leaves this device at all.
fn syncable(item: &Item, policy: &Policy) -> bool {
    if item.concealed && !policy.sync_concealed {
        return false;
    }
    if item.kind == "files" && file_list(&item.content).is_err() {
        return false;
    }
    matches!(item.kind.as_str(), "text" | "link" | "image" | "files")
}

/// Sends what the outbox holds, oldest first, whenever something is queued.
fn push_loop(app: AppHandle) {
    let state = app.state::<ShareState>();
    let db = app.state::<Db>();
    let mut backoff = 2;
    loop {
        let seen = state.outbox_generation();
        let relay = match session(&app) {
            Ok(Some(relay)) => relay,
            _ => {
                state.wait_outbox(seen, Duration::from_secs(3600));
                continue;
            }
        };
        let policy = match state.policy() {
            Some(policy) => policy,
            None => match relay.policy() {
                Ok(policy) => {
                    *state.policy.lock().unwrap() = Some(policy.clone());
                    policy
                }
                Err(message) => {
                    state.set(&app, Status::Error { message });
                    state.wait_outbox(seen, Duration::from_secs(backoff));
                    backoff = (backoff * 2).min(60);
                    continue;
                }
            },
        };
        let pending = match db.outbox(50) {
            Ok(pending) => pending,
            Err(e) => {
                eprintln!("[pinchy] outbox read failed: {e}");
                state.wait_outbox(seen, Duration::from_secs(10));
                continue;
            }
        };
        if pending.is_empty() {
            state.wait_outbox(seen, Duration::from_secs(3600));
            continue;
        }
        let mut failed = None;
        for entry in &pending {
            match push_one(&relay, &db, entry, &policy) {
                Ok(()) => {
                    let _ = db.dequeue(entry);
                }
                Err(message) => {
                    failed = Some(message);
                    break;
                }
            }
        }
        match failed {
            Some(message) => {
                state.set(&app, Status::Error { message });
                state.wait_outbox(seen, Duration::from_secs(backoff));
                backoff = (backoff * 2).min(60);
            }
            None => {
                backoff = 2;
                if !matches!(state.status(), Status::Connected { .. }) {
                    refresh_connected(&app, &relay);
                }
            }
        }
    }
}

fn push_one(relay: &Relay, db: &Db, entry: &Pending, policy: &Policy) -> Result<(), String> {
    match entry.op {
        Op::Delete => relay.delete(&entry.sync_id),
        Op::Upsert => {
            let Some(item) = db.get_by_sync_id(&entry.sync_id).map_err(|e| e.to_string())? else {
                return Ok(());
            };
            if !syncable(&item, policy) {
                return Ok(());
            }
            match relay.put(&item) {
                Ok(_) => Ok(()),
                // A local problem with this item must not block the queue.
                Err(e) if e.contains("찾을 수 없어요") || e.contains("모양이 달라") => {
                    eprintln!("[pinchy] skipping item {}: {e}", entry.sync_id);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
    }
}

/// Why the whole feed is being read from zero.
#[derive(Clone, Copy, PartialEq)]
enum FullSync {
    /// Right after connecting: the history comes down, then this device's
    /// own items go up.
    Initial,
    /// The relay forgot how far this device had got: rows it no longer has
    /// were deleted meanwhile.
    Reset,
}

/// Follows the relay's change feed and applies it to the local history.
fn pull_loop(app: AppHandle, dirs: Dirs) {
    let state = app.state::<ShareState>();
    let db = app.state::<Db>();
    let mut icons = AppIconCache::new(dirs.icons.clone());
    let mut seen = u64::MAX;
    let mut backoff = 5;
    let mut policy_checked: Option<Instant> = None;
    let mut key_mismatch_reported = false;
    // While reading the feed from zero: why, and the ids seen so far.
    let mut full_sync: Option<(FullSync, HashSet<String>)> = None;
    loop {
        let generation = state.generation();
        if generation != seen {
            seen = generation;
            backoff = 5;
            policy_checked = None;
            key_mismatch_reported = false;
            full_sync = None;
        }
        let relay = match session(&app) {
            Ok(Some(relay)) => relay,
            Ok(None) => {
                state.set(&app, Status::Off);
                state.wait(seen, Duration::from_secs(3600));
                continue;
            }
            Err(message) => {
                state.set(&app, Status::Error { message });
                state.wait(seen, Duration::from_secs(3600));
                continue;
            }
        };
        if policy_checked.is_none_or(|at| at.elapsed() >= POLICY_REFRESH) {
            refresh_connected(&app, &relay);
            policy_checked = Some(Instant::now());
        }
        if full_sync.is_none() && db.sync_value(INITIAL_KEY).ok().flatten().is_none() {
            // The first sync never finished (or never started): start over.
            let _ = db.set_sync_value(SINCE_KEY, None);
            full_sync = Some((FullSync::Initial, HashSet::new()));
        }
        let since: i64 = db.sync_value(SINCE_KEY).ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(0);
        let page = match relay.changes(since, if full_sync.is_some() { 0 } else { POLL_SECS }) {
            Ok(Some(page)) => page,
            Ok(None) => {
                // Behind the relay's memory: read everything again.
                let _ = db.set_sync_value(SINCE_KEY, None);
                full_sync = Some((FullSync::Reset, HashSet::new()));
                continue;
            }
            Err(message) => {
                state.set(&app, Status::Error { message });
                state.wait(seen, Duration::from_secs(backoff));
                backoff = (backoff * 2).min(60);
                continue;
            }
        };
        backoff = 5;
        let mut changed = false;
        let mut stopped = None;
        for change in &page.items {
            if let Some((_, ids)) = full_sync.as_mut() {
                if !change.deleted {
                    ids.insert(change.id.clone());
                }
            } else if change.origin == relay.device_id {
                // Our own change coming back; the history already has it.
                continue;
            }
            match apply(&app, &relay, &db, change, &dirs, &mut icons) {
                Ok(did) => changed |= did,
                Err(ReceiveError::Locked) => {
                    if !key_mismatch_reported {
                        eprintln!("[pinchy] item {} is sealed with another key", change.id);
                        state.set(&app, Status::Error { message: KEY_MISMATCH.into() });
                        key_mismatch_reported = true;
                    }
                }
                Err(ReceiveError::Invalid(e)) => eprintln!("[pinchy] skipping synced item {}: {e}", change.id),
                Err(ReceiveError::Network(e)) => {
                    stopped = Some((change.seq, e));
                    break;
                }
            }
        }
        if changed {
            let _ = app.emit(CHANGED_EVENT, ());
        }
        match stopped {
            Some((seq, message)) => {
                // Keep what was applied; retry from the failed change.
                let _ = db.set_sync_value(SINCE_KEY, Some(&(seq - 1).to_string()));
                state.set(&app, Status::Error { message });
                state.wait(seen, Duration::from_secs(backoff));
                backoff = (backoff * 2).min(60);
                continue;
            }
            None => {
                let _ = db.set_sync_value(SINCE_KEY, Some(&page.next.to_string()));
            }
        }
        if page.more {
            continue;
        }
        if let Some((why, ids)) = full_sync.take() {
            finish_full_sync(&app, &db, why, &ids);
            let _ = db.set_sync_value(INITIAL_KEY, Some("1"));
            refresh_connected(&app, &relay);
        } else if !matches!(state.status(), Status::Connected { .. }) && !key_mismatch_reported {
            refresh_connected(&app, &relay);
        }
    }
}

/// Applies one change. Returns whether the visible history changed.
fn apply(app: &AppHandle, relay: &Relay, db: &Db, change: &Change, dirs: &Dirs, icons: &mut AppIconCache) -> Result<bool, ReceiveError> {
    if change.deleted {
        return db.apply_remote_delete(&change.id).map_err(|e| ReceiveError::Network(e.to_string()));
    }
    // An item already here only needs its new copy time, not its blob again.
    let head = relay.open_meta(change)?;
    let origin = (relay.name.as_deref() != Some(head.from.as_str())).then_some(head.from.as_str());
    if let Some(changed) = db
        .bump_remote(&change.id, head.copied_at, head.app.as_deref(), origin)
        .map_err(|e| ReceiveError::Network(e.to_string()))?
    {
        return Ok(changed);
    }
    let opened = relay.open(change)?;
    let copied_at = opened.copied_at;
    let applied = {
        let _files = db.1.lock().unwrap();
        let item = opened
            .into_item(dirs, icons, relay.name.as_deref())
            .ok_or_else(|| ReceiveError::Network("받은 이미지나 파일을 저장하지 못했어요.".into()))?;
        drop(_files);
        db.apply_remote(&change.id, item, copied_at).map_err(|e| ReceiveError::Network(e.to_string()))?
    };
    if let Some(duplicate) = &applied.delete_remote {
        let _ = db.queue_delete(duplicate);
    }
    if applied.delete_remote.is_some() || applied.push.is_some() {
        app.state::<ShareState>().wake_pusher();
    }
    Ok(applied.changed)
}

/// After the whole history came down. On a reset, rows the relay does not
/// have and that are not about to be pushed were deleted while this device
/// was away. Either way, items only this device has go up.
fn finish_full_sync(app: &AppHandle, db: &Db, why: FullSync, on_relay: &HashSet<String>) {
    if why == FullSync::Reset {
        let pending: HashSet<String> =
            db.outbox(i64::MAX).unwrap_or_default().into_iter().map(|p| p.sync_id).collect();
        let mut removed = false;
        if let Ok(items) = db.list("", i64::MAX) {
            for item in items {
                if !on_relay.contains(&item.sync_id) && !pending.contains(&item.sync_id) && !item.concealed {
                    if let Ok(true) = db.apply_remote_delete(&item.sync_id) {
                        removed = true;
                    }
                }
            }
        }
        if removed {
            let _ = app.emit(CHANGED_EVENT, ());
        }
    }
    match db.queue_missing(on_relay) {
        Ok(0) => {}
        Ok(_) => app.state::<ShareState>().wake_pusher(),
        Err(e) => eprintln!("[pinchy] queueing the history failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_parts_open_only_with_the_same_key_part_and_id() {
        let key = Key::parse(&generate_key()).unwrap();
        let other = Key::parse(&generate_key()).unwrap();
        let id = crate::db::new_sync_id();
        let sealed = key.seal("meta", &id, b"hello");
        assert_eq!(key.open("meta", &id, &sealed).unwrap(), b"hello");
        assert!(other.open("meta", &id, &sealed).is_none(), "different key");
        assert!(key.open("blob", &id, &sealed).is_none(), "different part");
        assert!(key.open("meta", &crate::db::new_sync_id(), &sealed).is_none(), "different item");
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(key.open("meta", &id, &tampered).is_none());
        assert!(key.open("meta", &id, &sealed[..10]).is_none());
        assert_ne!(key.seal("meta", &id, b"hello"), sealed, "nonces are random");
        assert_eq!(key.open_label(&key.seal_label(" 집 맥 ")).as_deref(), Some("집 맥"));
        assert_eq!(other.open_label(&key.seal_label("집 맥")), None);
    }

    #[test]
    fn ids_keys_and_codes_have_the_expected_shape() {
        let id = crate::db::new_sync_id();
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert_eq!(generate_key().len(), 43);
        assert!(Key::parse("short").is_err());
        assert_eq!(random_digits(8).len(), 8);
        assert!(random_digits(30).bytes().all(|b| b.is_ascii_digit()));
    }

    #[test]
    fn codes_are_told_apart_by_shape() {
        assert_eq!(parse_code(" 1234-5678-9012 ").unwrap(), Code::First("123456789012".into()));
        assert_eq!(parse_code("1234 5678 9012").unwrap(), Code::First("123456789012".into()));
        assert_eq!(
            parse_code("37-1234-5678").unwrap(),
            Code::Invite { nameplate: "37".into(), password: "12345678".into() }
        );
        assert_eq!(
            parse_code("512 1234 5678").unwrap(),
            Code::Invite { nameplate: "512".into(), password: "12345678".into() }
        );
        assert!(parse_code("37-1234").is_err());
        assert!(parse_code("12345-1234-1234").is_err());
        assert!(parse_code("abc").is_err());
    }

    #[test]
    fn spake2_agrees_only_with_the_same_password() {
        let start = |pw: &str| {
            Spake2::<Ed25519Group>::start_a(&Password::new(pw.as_bytes()), &Identity::new(INVITER), &Identity::new(JOINER))
        };
        let join = |pw: &str| {
            Spake2::<Ed25519Group>::start_b(&Password::new(pw.as_bytes()), &Identity::new(INVITER), &Identity::new(JOINER))
        };
        let ((a, msg_a), (b, msg_b)) = (start("12345678"), join("12345678"));
        let (ka, kb) = (a.finish(&msg_b).unwrap(), b.finish(&msg_a).unwrap());
        assert_eq!(ka, kb);
        assert_eq!(ka.len(), 32);
        let ((a, msg_a), (b, msg_b)) = (start("12345678"), join("12345679"));
        let (ka, kb) = (a.finish(&msg_b).unwrap(), b.finish(&msg_a).unwrap());
        let (inviter, joiner) = (Key::from_bytes(&ka).unwrap(), Key::from_bytes(&kb).unwrap());
        assert!(inviter.open("pair-confirm", "37", &joiner.seal("pair-confirm", "37", JOINER)).is_none());
    }

    #[test]
    fn servers_are_normalized_and_validated() {
        assert_eq!(normalize_server(" https://relay.example.org/ ").unwrap(), "https://relay.example.org");
        assert!(normalize_server("http://relay.example.org").is_err());
        assert!(normalize_server("http://127.0.0.1:8787").is_ok());
        assert!(normalize_server("https://a b").is_err());
    }

    #[test]
    fn phone_links_keep_the_key_in_the_fragment_and_render_as_qr() {
        let link = join_link("https://relay.example.org", "abc123", "k_-ey");
        assert_eq!(link, "https://relay.example.org/#join?code=abc123&key=k_-ey");
        let svg = qr_svg(&link).unwrap();
        assert!(svg.starts_with("<?xml") || svg.starts_with("<svg"));
        assert!(svg.contains("<svg"));
    }

    #[test]
    fn meta_keeps_text_and_omits_it_for_images() {
        let text = Meta {
            v: 2, kind: "text".into(), text: Some("안녕".into()), files: None,
            from: Some("집 맥".into()), app: Some("Notes".into()), copied_at: Some(1_700_000_000_000),
        };
        let image = Meta { v: 2, kind: "image".into(), text: None, files: None, from: None, app: None, copied_at: None };
        assert_eq!(serde_json::to_string(&image).unwrap(), r#"{"v":2,"kind":"image"}"#);
        let restored: Meta = serde_json::from_str(&serde_json::to_string(&text).unwrap()).unwrap();
        assert_eq!(restored, text);
        let files = Meta {
            v: 2, kind: "files".into(), text: None, from: None, app: None, copied_at: None,
            files: Some(vec![FileMeta { name: "보고서.pdf".into(), size: 3 }]),
        };
        let restored: Meta = serde_json::from_str(&serde_json::to_string(&files).unwrap()).unwrap();
        assert_eq!(restored, files);
        let old: Meta = serde_json::from_str(r#"{"v":1,"kind":"text","text":"x","from":"회사 맥"}"#).unwrap();
        assert_eq!(old.copied_at, None, "0.x envelopes fall back to the relay's time");
    }

    #[test]
    fn files_are_capped_and_folders_refused() {
        let dir = std::env::temp_dir().join(format!("pinchy-files-{}-{}", std::process::id(), crate::db::now_ms()));
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        let (a, b, big) = (dir.join("a.txt"), dir.join("b.bin"), dir.join("big.bin"));
        std::fs::write(&a, b"hello").unwrap();
        std::fs::write(&b, [0u8, 1, 2]).unwrap();
        std::fs::File::create(&big).unwrap().set_len(MAX_FILES_BYTES + 1).unwrap();
        let list = |paths: &[&Path]| serde_json::to_string(&paths).unwrap();

        let (files, blob) = read_files(&list(&[&a, &b])).unwrap();
        assert_eq!(files, vec![FileMeta { name: "a.txt".into(), size: 5 }, FileMeta { name: "b.bin".into(), size: 3 }]);
        assert_eq!(blob, b"hello\x00\x01\x02");
        assert!(file_list(&list(&[&big])).unwrap_err().contains("50MB"));
        assert!(file_list(&list(&[&a, &dir.join("folder")])).unwrap_err().contains("폴더"));
        assert!(file_list(&list(&[&dir.join("missing")])).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn received_files_are_split_with_safe_unique_names() {
        let meta = |name: &str, size| FileMeta { name: name.into(), size };
        let files = [meta("a.txt", 2), meta("../../A.txt", 1), meta("a.txt", 0)];
        let split = split_files(&files, b"hi!").unwrap();
        let names: Vec<&str> = split.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["a.txt", ".._.._A.txt", "a (2).txt"]);
        assert_eq!(split[0].1, b"hi");
        assert_eq!(split[1].1, b"!");
        assert!(split[2].1.is_empty());
        assert!(split_files(&files, b"too long").is_none());
        assert!(split_files(&[], b"").is_none());

        assert_eq!(safe_file_name("a/b\\c:d?.txt"), "a_b_c_d_.txt");
        assert_eq!(safe_file_name(" 메모. . "), "메모");
        assert_eq!(safe_file_name(".."), "파일");
        assert_eq!(safe_file_name("con.txt"), "_con.txt");
        assert_eq!(safe_file_name("COM1"), "_COM1");
        assert_eq!(safe_file_name("computer.txt"), "computer.txt");
        let long = safe_file_name(&format!("{}.pdf", "가".repeat(100)));
        assert!(long.len() <= MAX_FILE_NAME_BYTES && long.ends_with("가.pdf"));
    }

    #[test]
    fn names_are_trimmed_capped_and_never_empty() {
        assert_eq!(clean_name("  회사 맥 "), Some("회사 맥".into()));
        assert_eq!(clean_name("   "), None);
        assert_eq!(clean_name(&"가".repeat(100)).unwrap().chars().count(), MAX_NAME_CHARS);
    }

    fn item(kind: &str, content: &str, concealed: bool) -> Item {
        Item {
            id: 1, kind: kind.into(), content: content.into(), preview: String::new(), source_app: None,
            app_icon: None, app_color: None, width: None, height: None, size: 0, created_at: 0, og_title: None,
            og_image: None, origin_device: None, sync_id: crate::db::new_sync_id(), concealed,
        }
    }

    #[test]
    fn concealed_copies_stay_local_unless_the_relay_allows_them() {
        let policy = |sync_concealed| Policy {
            retention_days: 30, max_storage_mb: 1024, max_blob_mb: 64, sync_concealed, usage: None,
        };
        assert!(syncable(&item("text", "x", false), &policy(false)));
        assert!(syncable(&item("link", "https://a", false), &policy(false)));
        assert!(syncable(&item("image", "/nope.png", false), &policy(false)));
        assert!(!syncable(&item("text", "hunter2", true), &policy(false)));
        assert!(syncable(&item("text", "hunter2", true), &policy(true)));
        assert!(!syncable(&item("files", "[]", false), &policy(true)), "nothing to send");
        assert!(!syncable(&item("files", "[\"/definitely/missing\"]", false), &policy(true)));
    }

    #[test]
    fn only_png_blobs_are_accepted() {
        let mut png = Vec::new();
        image::RgbaImage::new(3, 2)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert_eq!(png_size(&png), Some((3, 2)));
        assert_eq!(png_size(b"<script>"), None);
    }

    /// Talks to a fresh relay. Start one with an empty data directory, then:
    /// PINCHY_TEST_RELAY=http://127.0.0.1:18787 PINCHY_TEST_FIRST_CODE=1234-5678-9012
    /// cargo test share::tests::relay_round_trip -- --ignored
    #[test]
    #[ignore]
    fn relay_round_trip() {
        let server = std::env::var("PINCHY_TEST_RELAY").unwrap();
        let config = |name: &str, device_id: String| ShareConfig { name: name.into(), server: server.clone(), device_id };

        // First device, then an invited one.
        let Code::First(first) = parse_code(&std::env::var("PINCHY_TEST_FIRST_CODE").unwrap()).unwrap() else {
            panic!("not a first code")
        };
        let (a_id, a_secrets) = join_first(&server, "집 맥", &first).unwrap();
        let a = Relay::new(&config("집 맥", a_id.clone()), &a_secrets).unwrap();
        assert!(join_first(&server, "x", &first).is_err(), "first code is single use");
        let (mut invitation, code) = Invitation::start(&a).unwrap();
        let Code::Invite { nameplate, password } = parse_code(&code).unwrap() else { panic!("not an invite") };
        let joiner = {
            let server = server.clone();
            std::thread::spawn(move || join_invited(&server, "회사 맥", &nameplate, &password))
        };
        let joined = loop {
            match invitation.poll(&a) {
                Ok(InviteStep::Waiting) => std::thread::sleep(Duration::from_millis(100)),
                Ok(InviteStep::Joined(name)) => break name,
                Err(_) => panic!("invitation failed"),
            }
        };
        assert_eq!(joined, "회사 맥");
        let (b_id, b_secrets) = joiner.join().unwrap().unwrap();
        assert_eq!(b_secrets.key, a_secrets.key, "the sharing key arrived");
        let b = Relay::new(&config("회사 맥", b_id.clone()), &b_secrets).unwrap();
        assert_eq!(a.policy().unwrap().retention_days, 30);

        // Names and items.
        let names: Vec<(String, bool)> = b.devices().unwrap().into_iter().map(|d| (d.name, d.me)).collect();
        assert_eq!(names, vec![("집 맥".into(), false), ("회사 맥".into(), true)]);
        b.rename("회사 노트북").unwrap();
        assert_eq!(a.devices().unwrap()[1].name, "회사 노트북");

        let dir = std::env::temp_dir().join(format!("pinchy-share-{}-{}", std::process::id(), crate::db::now_ms()));
        let dirs = Dirs { images: dir.join("images"), files: dir.join("files"), icons: dir.join("icons") };
        std::fs::create_dir_all(&dirs.images).unwrap();
        std::fs::create_dir_all(&dirs.icons).unwrap();
        let mut png = Vec::new();
        image::RgbaImage::new(4, 5)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let source = dir.join("source.png");
        std::fs::write(&source, &png).unwrap();
        let mut text = item("text", "집에서 보낸 문장", false);
        text.created_at = 1_700_000_000_000;
        assert!(a.put(&text).unwrap());
        assert!(a.put(&item("image", &source.to_string_lossy(), false)).unwrap());
        let note = dir.join("메모.txt");
        std::fs::write(&note, "파일 내용").unwrap();
        let files = serde_json::to_string(&[&note, &source]).unwrap();
        assert!(a.put(&item("files", &files, false)).unwrap());
        assert!(a.put(&item("files", "[]", false)).is_err());
        // The relay may hold earlier history; look at what this run created.
        let page = b.changes(0, 5).unwrap().unwrap();
        let ours: Vec<&Change> = page.items.iter().filter(|c| c.origin == a_id && !c.deleted).collect();
        assert_eq!(ours.len(), 3);
        let mut icons = AppIconCache::new(dirs.icons.clone());
        let opened: Vec<(i64, NewItem)> = ours
            .iter()
            .map(|c| {
                let o = b.open(c).ok().unwrap();
                (o.copied_at, o.into_item(&dirs, &mut icons, Some("회사 노트북")).unwrap())
            })
            .collect();
        assert_eq!(opened[0].0, 1_700_000_000_000);
        assert_eq!(opened[0].1.content, "집에서 보낸 문장");
        assert_eq!(opened[0].1.origin_device.as_deref(), Some("집 맥"));
        assert_eq!(std::fs::read(&opened[1].1.content).unwrap(), png);
        let paths: Vec<PathBuf> = serde_json::from_str(&opened[2].1.content).unwrap();
        assert_eq!(paths[0].file_name().unwrap(), "메모.txt");
        assert_eq!(std::fs::read_to_string(&paths[0]).unwrap(), "파일 내용");
        assert!(paths[0].starts_with(&dirs.files));

        // Deletions and the tail of the feed.
        b.delete(&text.sync_id).unwrap();
        let page = a.changes(page.next, 0).unwrap().unwrap();
        assert!(page.items[0].deleted && page.items[0].id == text.sync_id);
        assert!(!a.put(&text).unwrap(), "deleted items stay deleted");
        a.delete_all().unwrap();
        let page = b.changes(page.next, 0).unwrap().unwrap();
        assert!(page.items.iter().all(|c| c.deleted));

        // Removing every device brings the first code back.
        a.remove_device(&b_id).unwrap();
        assert!(b.me().is_err());
        a.remove_device(&a_id).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
