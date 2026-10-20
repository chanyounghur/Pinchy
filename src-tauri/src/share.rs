//! Sends history items to the user's other devices through a Pinchy relay,
//! and receives theirs into the history.
//!
//! Contents are sealed with XChaCha20-Poly1305 under a key that only the
//! user's devices know; the relay sees ciphertext and sizes. Each part is
//! bound to its item id so the relay cannot swap them. Device names travel
//! sealed too.
//!
//! Joining: the first device enters the one-time code the relay prints and
//! creates the sharing key. Every later device enters a code shown by a
//! connected device; the two run SPAKE2 over the relay with it, so the
//! sharing key reaches the new device without the relay being able to read
//! it. The device token and sharing key are kept in the OS keychain.

use crate::clipboard::{self, CHANGED_EVENT};
use crate::db::{Db, Item, NewItem};
use crate::platform;
use crate::settings::{ShareConfig, SettingsStore};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use chacha20poly1305::aead::{rand_core::RngCore, Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::{Deserialize, Serialize};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub const STATUS_EVENT: &str = "share-status";
pub const INVITE_EVENT: &str = "share-invite";
/// Header color for cards received from another device.
const RECEIVED_COLOR: &str = "#c64735";
const NONCE_LEN: usize = 24;
const POLL_SECS: u64 = 25;
const MAX_NAME_CHARS: usize = 64;
const INVITE_TTL: Duration = Duration::from_secs(600);
const JOIN_TIMEOUT: Duration = Duration::from_secs(60);
const KEYCHAIN_SERVICE: &str = "com.chanyounghur.pastel.share";
const KEYCHAIN_ACCOUNT: &str = "relay";
const INVITER: &[u8] = b"pinchy-inviter";
const JOINER: &[u8] = b"pinchy-joiner";
const KEY_MISMATCH: &str =
    "다른 공유 키로 보낸 항목이 있어 열 수 없어요. 서버를 초기화했다면 보낸 기기를 다시 연결해 주세요.";
const UNKNOWN_DEVICE: &str = "다른 기기";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    Off,
    Connecting,
    Connected { devices: i64 },
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

/// Receiver status, cached secrets, the running invitation, and a
/// generation counter that wakes the receiver when the connection changes.
pub struct ShareState {
    status: Mutex<Status>,
    generation: Mutex<u64>,
    changed: Condvar,
    secrets: Mutex<Option<Secrets>>,
    invite: Mutex<Option<Arc<AtomicBool>>>,
}

impl Default for ShareState {
    fn default() -> Self {
        ShareState {
            status: Mutex::new(Status::Connecting),
            generation: Mutex::new(0),
            changed: Condvar::new(),
            secrets: Mutex::new(None),
            invite: Mutex::new(None),
        }
    }
}

impl ShareState {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    fn set(&self, app: &AppHandle, status: Status) {
        let mut current = self.status.lock().unwrap();
        if *current != status {
            *current = status.clone();
            let _ = app.emit(STATUS_EVENT, status);
        }
    }

    fn generation(&self) -> u64 {
        *self.generation.lock().unwrap()
    }

    fn connection_changed(&self) {
        *self.generation.lock().unwrap() += 1;
        self.changed.notify_all();
    }

    /// Sleeps until the connection changes or `timeout` passes.
    fn wait(&self, seen: u64, timeout: Duration) {
        let generation = self.generation.lock().unwrap();
        let _ = self.changed.wait_timeout_while(generation, timeout, |g| *g == seen);
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

/// The sealed description of an item. Image bytes travel as a separate blob.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Meta {
    v: u32,
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    /// The sender's own name for itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from: Option<String>,
}

/// Trims a device name and caps its length; empty means none.
fn clean_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().take(MAX_NAME_CHARS).collect();
    let name = name.trim().to_string();
    (!name.is_empty()).then_some(name)
}

fn random_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
            .map_err(|e| HttpError { status: None, message: format!("중계 서버에 연결할 수 없어요. ({e})") })?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let code = status.as_u16();
        let message = match code {
            401 => "이 기기의 연결이 해제됐어요. 연결을 해제한 뒤 새 코드로 다시 연결해 주세요.".into(),
            413 => "너무 커서 보낼 수 없어요.".into(),
            422 => "보낼 다른 기기가 없어요. 설정에서 기기를 추가해 주세요.".into(),
            429 => "실패한 시도가 많아 잠시 막혔어요. 10분 뒤 다시 시도해 주세요.".into(),
            code => format!("중계 서버 오류예요. (HTTP {code})"),
        };
        Err(HttpError { status: Some(code), message })
    }

    fn json<T: for<'de> Deserialize<'de>>(response: Response) -> Result<T, String> {
        let bytes = response.bytes().map_err(|e| format!("중계 서버 응답을 받지 못했어요. ({e})"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("중계 서버 응답을 읽지 못했어요. ({e})"))
    }
}

fn decode(field: &str) -> Result<Vec<u8>, String> {
    STANDARD.decode(field).map_err(|_| "중계 서버 응답을 읽지 못했어요.".into())
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

#[derive(Debug, Deserialize)]
struct Incoming {
    id: String,
    envelope: String,
    blob_size: Option<i64>,
}

#[derive(Deserialize)]
struct Inbox {
    items: Vec<Incoming>,
}

#[derive(Deserialize)]
struct Sent {
    recipients: usize,
}

enum ReceiveError {
    /// Sealed with a different key. Kept on the relay in case it is fixed.
    Locked,
    /// Can never be stored, e.g. an unknown kind. Dropped.
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

    fn rename(&self, name: &str) -> Result<(), String> {
        let body = serde_json::json!({ "label": self.key.seal_label(name) });
        self.http.send(
            self.http
                .put("/v1/me")
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_string()),
        )?;
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

    /// Seals and queues `item` for every other device. Returns how many.
    fn push(&self, item: &Item) -> Result<usize, String> {
        let from = self.name.clone();
        let (meta, blob) = match item.kind.as_str() {
            "text" | "link" => (Meta { v: 1, kind: item.kind.clone(), text: Some(item.content.clone()), from }, None),
            "image" => {
                let bytes = std::fs::read(&item.content).map_err(|_| "이미지 파일을 찾을 수 없어요.")?;
                (Meta { v: 1, kind: "image".into(), text: None, from }, Some(bytes))
            }
            _ => return Err("파일은 아직 보낼 수 없어요. 텍스트, 링크, 이미지만 보낼 수 있어요.".into()),
        };
        let id = random_id();
        let envelope = self.key.seal("meta", &id, &serde_json::to_vec(&meta).map_err(|e| e.to_string())?);
        let body = serde_json::json!({ "id": id, "envelope": STANDARD.encode(envelope), "has_blob": blob.is_some() });
        let sent: Sent = Http::json(self.http.send(self.http.post("/v1/items", body))?)?;
        if let Some(bytes) = blob {
            self.http.send(
                self.http
                    .put(&format!("/v1/items/{id}/blob"))
                    .timeout(Duration::from_secs(600))
                    .body(self.key.seal("blob", &id, &bytes)),
            )?;
        }
        Ok(sent.recipients)
    }

    fn inbox(&self, wait: u64) -> Result<Vec<Incoming>, String> {
        let request = self.http.get(&format!("/v1/inbox?wait={wait}")).timeout(Duration::from_secs(wait + 30));
        Ok(Http::json::<Inbox>(self.http.send(request)?)?.items)
    }

    fn ack(&self, id: &str) -> Result<(), String> {
        self.http.send(self.http.post(&format!("/v1/items/{id}/ack"), serde_json::json!({})))?;
        Ok(())
    }

    /// Downloads and opens an incoming item. Images are written to the cache
    /// by `into_item`, under the history's file lock.
    fn open(&self, incoming: &Incoming) -> Result<Opened, ReceiveError> {
        let envelope = STANDARD
            .decode(&incoming.envelope)
            .map_err(|_| ReceiveError::Invalid("envelope is not base64".into()))?;
        let meta = self.key.open("meta", &incoming.id, &envelope).ok_or(ReceiveError::Locked)?;
        let meta: Meta = serde_json::from_slice(&meta).map_err(|e| ReceiveError::Invalid(e.to_string()))?;
        let from = meta.from.as_deref().and_then(clean_name).unwrap_or_else(|| UNKNOWN_DEVICE.into());
        let content = match (meta.kind.as_str(), meta.text) {
            ("text" | "link", Some(text)) if !text.trim().is_empty() => Content::Text(text),
            ("image", _) if incoming.blob_size.is_some() => {
                let response = self
                    .http
                    .send(self.http.get(&format!("/v1/items/{}/blob", incoming.id)).timeout(Duration::from_secs(600)))
                    .map_err(|e| ReceiveError::Network(e.message))?;
                let sealed = response.bytes().map_err(|e| ReceiveError::Network(e.to_string()))?;
                let png = self.key.open("blob", &incoming.id, &sealed).ok_or(ReceiveError::Locked)?;
                let (w, h) = png_size(&png).ok_or_else(|| ReceiveError::Invalid("blob is not a PNG image".into()))?;
                Content::Png(png, w, h)
            }
            (kind, _) => return Err(ReceiveError::Invalid(format!("unsupported item kind {kind:?}"))),
        };
        Ok(Opened { from, content })
    }
}

enum Content {
    Text(String),
    Png(Vec<u8>, u32, u32),
}

struct Opened {
    from: String,
    content: Content,
}

impl Opened {
    fn into_item(self, images_dir: &Path) -> Option<NewItem> {
        let mut item = match self.content {
            Content::Text(text) => clipboard::text_item(text),
            Content::Png(png, w, h) => clipboard::png_item(images_dir, &png, w, h)?,
        };
        item.source_app = Some(self.from.clone());
        item.app_color = Some(RECEIVED_COLOR.into());
        item.origin_device = Some(self.from);
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
    session(app)?.ok_or_else(|| "기기 간 공유를 먼저 연결해 주세요.".into())
}

/// Drops this device's connection locally.
fn forget(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<ShareState>();
    state.stop_invite();
    app.state::<SettingsStore>().update(|s| s.share = None)?;
    forget_secrets(&state);
    state.set(app, Status::Off);
    state.connection_changed();
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
        app.state::<SettingsStore>().update(|s| s.share = Some(config.clone()))?;
        let status = match Relay::new(&config, &secrets)?.me() {
            Ok(me) => Status::Connected { devices: me.devices },
            Err(message) => Status::Error { message },
        };
        state.set(&app, status.clone());
        state.connection_changed();
        Ok(status)
    })
    .await
}

/// Shows a code another device can join with. Progress arrives as
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
                    if let Ok(me) = relay.me() {
                        app.state::<ShareState>().set(&app, Status::Connected { devices: me.devices });
                    }
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
        if let Ok(me) = relay.me() {
            app.state::<ShareState>().set(&app, Status::Connected { devices: me.devices });
        }
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

/// Renames this device for the device list and for items it sends.
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

/// Sends a history item to every other device. Returns how many.
#[tauri::command]
pub async fn send_item(app: AppHandle, id: i64) -> Result<usize, String> {
    blocking(move || {
        let relay = connected(&app)?;
        let item = app.state::<Db>().get(id).map_err(|e| e.to_string())?.ok_or("항목을 찾을 수 없어요.")?;
        relay.push(&item)
    })
    .await
}

// ---------------------------------------------------------------------------
// Receiver

fn store(app: &AppHandle, relay: &Relay, incoming: &Incoming, images_dir: &Path) -> Result<(), ReceiveError> {
    let opened = relay.open(incoming)?;
    let db = app.state::<Db>();
    {
        let _files = db.1.lock().unwrap();
        let item = opened.into_item(images_dir).ok_or_else(|| ReceiveError::Network("이미지를 저장하지 못했어요.".into()))?;
        db.upsert(item).map_err(|e| ReceiveError::Network(e.to_string()))?;
    }
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(())
}

pub fn start(app: AppHandle, images_dir: PathBuf) {
    std::thread::Builder::new()
        .name("share-receiver".into())
        .spawn(move || receive_loop(app, images_dir))
        .expect("spawn share receiver");
}

fn receive_loop(app: AppHandle, images_dir: PathBuf) {
    let state = app.state::<ShareState>();
    let mut seen = u64::MAX;
    let mut locked: HashSet<String> = HashSet::new();
    let mut backoff = 5;
    loop {
        let generation = state.generation();
        if generation != seen {
            seen = generation;
            locked.clear();
            backoff = 5;
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
        let items = match relay.inbox(POLL_SECS) {
            Ok(items) => items,
            Err(message) => {
                state.set(&app, Status::Error { message });
                state.wait(seen, Duration::from_secs(backoff));
                backoff = (backoff * 2).min(60);
                continue;
            }
        };
        backoff = 5;
        let mut failed = None;
        let fresh: Vec<&Incoming> = items.iter().filter(|i| !locked.contains(&i.id)).collect();
        for incoming in fresh {
            match store(&app, &relay, incoming, &images_dir) {
                Ok(()) => {
                    if let Err(e) = relay.ack(&incoming.id) {
                        failed = Some(e);
                        break;
                    }
                }
                Err(ReceiveError::Locked) => {
                    locked.insert(incoming.id.clone());
                }
                Err(ReceiveError::Invalid(e)) => {
                    eprintln!("[pinchy] dropping shared item {}: {e}", incoming.id);
                    let _ = relay.ack(&incoming.id);
                }
                Err(ReceiveError::Network(e)) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        if let Some(message) = failed {
            state.set(&app, Status::Error { message });
            state.wait(seen, Duration::from_secs(backoff));
        } else if items.iter().any(|i| locked.contains(&i.id)) {
            // The relay keeps returning these at once; don't spin on them.
            state.set(&app, Status::Error { message: KEY_MISMATCH.into() });
            state.wait(seen, Duration::from_secs(30));
        } else if !matches!(state.status(), Status::Connected { .. }) {
            match relay.me() {
                Ok(me) => state.set(&app, Status::Connected { devices: me.devices }),
                Err(message) => state.set(&app, Status::Error { message }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_parts_open_only_with_the_same_key_part_and_id() {
        let key = Key::parse(&generate_key()).unwrap();
        let other = Key::parse(&generate_key()).unwrap();
        let id = random_id();
        let sealed = key.seal("meta", &id, b"hello");
        assert_eq!(key.open("meta", &id, &sealed).unwrap(), b"hello");
        assert!(other.open("meta", &id, &sealed).is_none(), "different key");
        assert!(key.open("blob", &id, &sealed).is_none(), "different part");
        assert!(key.open("meta", &random_id(), &sealed).is_none(), "different item");
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
        let id = random_id();
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
    fn meta_keeps_text_and_omits_it_for_images() {
        let text = Meta { v: 1, kind: "text".into(), text: Some("안녕".into()), from: Some("집 맥".into()) };
        let image = Meta { v: 1, kind: "image".into(), text: None, from: None };
        assert_eq!(serde_json::to_string(&image).unwrap(), r#"{"v":1,"kind":"image"}"#);
        let restored: Meta = serde_json::from_str(&serde_json::to_string(&text).unwrap()).unwrap();
        assert_eq!(restored, text);
    }

    #[test]
    fn names_are_trimmed_capped_and_never_empty() {
        assert_eq!(clean_name("  회사 맥 "), Some("회사 맥".into()));
        assert_eq!(clean_name("   "), None);
        assert_eq!(clean_name(&"가".repeat(100)).unwrap().chars().count(), MAX_NAME_CHARS);
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

        // A wrong guess ends the invitation.
        let (mut guessed, code) = Invitation::start(&a).unwrap();
        let Code::Invite { nameplate, .. } = parse_code(&code).unwrap() else { panic!() };
        let attacker = {
            let server = server.clone();
            std::thread::spawn(move || join_invited(&server, "x", &nameplate, "00000000"))
        };
        let outcome = loop {
            match guessed.poll(&a) {
                Ok(InviteStep::Waiting) => std::thread::sleep(Duration::from_millis(100)),
                other => break other,
            }
        };
        assert!(matches!(outcome, Err(InviteEvent::Failed { .. })));
        assert!(attacker.join().unwrap().is_err());

        // Names and items.
        let names: Vec<(String, bool)> = b.devices().unwrap().into_iter().map(|d| (d.name, d.me)).collect();
        assert_eq!(names, vec![("집 맥".into(), false), ("회사 맥".into(), true)]);
        b.rename("회사 노트북").unwrap();
        assert_eq!(a.devices().unwrap()[1].name, "회사 노트북");

        let images = std::env::temp_dir().join(format!("pinchy-share-{}-{}", std::process::id(), crate::db::now_ms()));
        std::fs::create_dir_all(&images).unwrap();
        let mut png = Vec::new();
        image::RgbaImage::new(4, 5)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let source = images.join("source.png");
        std::fs::write(&source, &png).unwrap();
        let item = |kind: &str, content: String| Item {
            id: 0, kind: kind.into(), content, preview: String::new(), source_app: None, app_icon: None,
            app_color: None, width: None, height: None, size: 0, created_at: 0, og_title: None, og_image: None,
            origin_device: None,
        };
        assert_eq!(a.push(&item("text", "집에서 보낸 문장".into())).unwrap(), 1);
        assert_eq!(a.push(&item("image", source.to_string_lossy().into_owned())).unwrap(), 1);
        assert!(a.push(&item("files", "[]".into())).is_err());
        let incoming = b.inbox(5).unwrap();
        let opened: Vec<NewItem> =
            incoming.iter().map(|i| b.open(i).ok().unwrap().into_item(&images).unwrap()).collect();
        assert_eq!(opened[0].content, "집에서 보낸 문장");
        assert_eq!(opened[0].origin_device.as_deref(), Some("집 맥"));
        assert_eq!(std::fs::read(&opened[1].content).unwrap(), png);
        for i in &incoming {
            b.ack(&i.id).unwrap();
        }
        assert!(b.inbox(0).unwrap().is_empty());

        // Removing every device brings the first code back.
        a.remove_device(&b_id).unwrap();
        assert!(b.me().is_err());
        a.remove_device(&a_id).unwrap();
        std::fs::remove_dir_all(images).unwrap();
    }
}
