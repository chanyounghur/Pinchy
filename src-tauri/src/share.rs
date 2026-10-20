//! Sends history items to the user's other devices through a Pinchy relay,
//! and receives theirs into the history.
//!
//! Contents are sealed with XChaCha20-Poly1305 under a key that only the
//! user's devices know; the relay sees ciphertext, sizes and device names.
//! Each part is bound to its item id so the relay cannot swap them.

use crate::clipboard::{self, CHANGED_EVENT};
use crate::db::{Db, Item, NewItem};
use crate::platform;
use crate::settings::{ShareConfig, SettingsStore};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

pub const STATUS_EVENT: &str = "share-status";
/// Header color for cards received from another device.
const RECEIVED_COLOR: &str = "#c64735";
const NONCE_LEN: usize = 24;
const POLL_SECS: u64 = 25;
const MAX_NAME_CHARS: usize = 64;
const KEY_MISMATCH: &str = "다른 기기와 공유 키가 달라서 받은 항목을 열 수 없어요. 모든 기기에 같은 공유 키를 입력해 주세요.";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    Off,
    Connecting,
    Connected { name: String, peers: Vec<String> },
    Error { message: String },
}

/// Receiver status, plus a generation counter that wakes the receiver when
/// the connection settings change.
pub struct ShareState {
    status: Mutex<Status>,
    generation: Mutex<u64>,
    changed: Condvar,
}

impl Default for ShareState {
    fn default() -> Self {
        ShareState { status: Mutex::new(Status::Connecting), generation: Mutex::new(0), changed: Condvar::new() }
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

    fn settings_changed(&self) {
        *self.generation.lock().unwrap() += 1;
        self.changed.notify_all();
    }

    /// Sleeps until the settings change or `timeout` passes.
    fn wait(&self, seen: u64, timeout: Duration) {
        let generation = self.generation.lock().unwrap();
        let _ = self.changed.wait_timeout_while(generation, timeout, |g| *g == seen);
    }
}

// ---------------------------------------------------------------------------
// Sealing

struct Key(XChaCha20Poly1305);

pub fn generate_key() -> String {
    URL_SAFE_NO_PAD.encode(XChaCha20Poly1305::generate_key(&mut OsRng))
}

impl Key {
    fn parse(text: &str) -> Result<Self, String> {
        let bytes = URL_SAFE_NO_PAD
            .decode(text.trim())
            .ok()
            .filter(|b| b.len() == 32)
            .ok_or("공유 키 형식이 올바르지 않아요. 다른 기기에서 복사한 키를 그대로 붙여넣어 주세요.")?;
        Ok(Key(XChaCha20Poly1305::new_from_slice(&bytes).expect("32-byte key")))
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
}

/// The sealed description of an item. Image bytes travel as a separate blob.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Meta {
    v: u32,
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    /// The sender's own name for itself. Sealed, so the relay never sees it.
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
    let bytes: [u8; 16] = rand_bytes();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn rand_bytes<const N: usize>() -> [u8; N] {
    use chacha20poly1305::aead::rand_core::RngCore;
    let mut bytes = [0u8; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

// ---------------------------------------------------------------------------
// Relay client

pub fn normalize(config: &ShareConfig) -> Result<ShareConfig, String> {
    let server = config.server.trim().trim_end_matches('/').to_string();
    let local = server.starts_with("http://127.0.0.1:") || server.starts_with("http://localhost:");
    if !(server.starts_with("https://") || local) || server.contains(char::is_whitespace) {
        return Err("서버 주소는 https://로 시작해야 해요.".into());
    }
    let token = config.token.trim().to_string();
    if !token.starts_with("pr1_") {
        return Err("기기 토큰은 pr1_로 시작해요. 서버에서 add-device로 만든 토큰을 입력해 주세요.".into());
    }
    let key = config.key.trim().to_string();
    Key::parse(&key)?;
    let name = clean_name(&config.name).or_else(platform::computer_name).unwrap_or_default();
    Ok(ShareConfig { name, server, token, key })
}

struct Relay {
    http: Client,
    name: Option<String>,
    server: String,
    token: String,
    key: Key,
}

#[derive(Debug, Deserialize)]
struct Me {
    name: String,
    peers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Incoming {
    id: String,
    from: String,
    envelope: String,
    blob_size: Option<i64>,
}

#[derive(Deserialize)]
struct Inbox {
    items: Vec<Incoming>,
}

#[derive(Deserialize)]
struct Created {
    recipients: usize,
}

enum ReceiveError {
    /// Sealed with a different key. Kept on the relay in case the key is fixed.
    Locked,
    /// Can never be stored, e.g. an unknown kind. Dropped.
    Invalid(String),
    /// Retry later.
    Network(String),
}

impl Relay {
    fn new(config: &ShareConfig) -> Result<Self, String> {
        let config = normalize(config)?;
        let http = Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("Pinchy/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Relay {
            http,
            name: clean_name(&config.name),
            server: config.server,
            token: config.token,
            key: Key::parse(&config.key)?,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.server)
    }

    fn send(&self, request: RequestBuilder) -> Result<Response, String> {
        let response = request
            .bearer_auth(&self.token)
            .send()
            .map_err(|e| format!("중계 서버에 연결할 수 없어요. ({e})"))?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        Err(match status.as_u16() {
            401 => "기기 토큰이 올바르지 않아요. 서버에서 이 기기를 다시 등록해 주세요.".into(),
            413 => "너무 커서 보낼 수 없어요.".into(),
            422 => "보낼 다른 기기가 없어요. 서버에 기기를 더 등록해 주세요.".into(),
            429 => "인증 실패가 많아 잠시 차단됐어요. 10분 뒤 다시 시도해 주세요.".into(),
            code => format!("중계 서버 오류예요. (HTTP {code})"),
        })
    }

    fn json<T: for<'de> Deserialize<'de>>(response: Response) -> Result<T, String> {
        let bytes = response.bytes().map_err(|e| format!("중계 서버 응답을 받지 못했어요. ({e})"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("중계 서버 응답을 읽지 못했어요. ({e})"))
    }

    fn me(&self) -> Result<Me, String> {
        Self::json(self.send(self.http.get(self.url("/v1/me")))?)
    }

    /// Seals and queues `item` for every other device. Returns how many.
    fn push(&self, item: &Item) -> Result<usize, String> {
        let (meta, blob) = match item.kind.as_str() {
            "text" | "link" => {
                (Meta { v: 1, kind: item.kind.clone(), text: Some(item.content.clone()), from: self.name.clone() }, None)
            }
            "image" => {
                let bytes = std::fs::read(&item.content).map_err(|_| "이미지 파일을 찾을 수 없어요.")?;
                (Meta { v: 1, kind: "image".into(), text: None, from: self.name.clone() }, Some(bytes))
            }
            _ => return Err("파일은 아직 보낼 수 없어요. 텍스트, 링크, 이미지만 보낼 수 있어요.".into()),
        };
        let id = random_id();
        let envelope = self.key.seal("meta", &id, &serde_json::to_vec(&meta).map_err(|e| e.to_string())?);
        let body = serde_json::json!({ "id": id, "envelope": STANDARD.encode(envelope), "has_blob": blob.is_some() });
        let created: Created = Self::json(self.send(
            self.http
                .post(self.url("/v1/items"))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_string()),
        )?)?;
        if let Some(bytes) = blob {
            self.send(
                self.http
                    .put(self.url(&format!("/v1/items/{id}/blob")))
                    .timeout(Duration::from_secs(600))
                    .body(self.key.seal("blob", &id, &bytes)),
            )?;
        }
        Ok(created.recipients)
    }

    fn inbox(&self, wait: u64) -> Result<Vec<Incoming>, String> {
        let request = self.http.get(self.url(&format!("/v1/inbox?wait={wait}"))).timeout(Duration::from_secs(wait + 30));
        Ok(Self::json::<Inbox>(self.send(request)?)?.items)
    }

    fn ack(&self, id: &str) -> Result<(), String> {
        self.send(self.http.post(self.url(&format!("/v1/items/{id}/ack"))))?;
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
        // Senders name themselves; fall back to the name the relay registered.
        let from = meta.from.as_deref().and_then(clean_name).unwrap_or_else(|| incoming.from.clone());
        let content = match (meta.kind.as_str(), meta.text) {
            ("text" | "link", Some(text)) if !text.trim().is_empty() => Content::Text(text),
            ("image", _) if incoming.blob_size.is_some() => {
                let response = self
                    .send(self.http.get(self.url(&format!("/v1/items/{}/blob", incoming.id))).timeout(Duration::from_secs(600)))
                    .map_err(ReceiveError::Network)?;
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

/// Renames this device for items it sends from now on.
#[tauri::command]
pub fn set_share_name(store: tauri::State<SettingsStore>, name: String) -> Result<String, String> {
    let name = clean_name(&name).ok_or("기기 이름을 입력해 주세요.")?;
    let mut saved = false;
    store.update(|s| {
        if let Some(share) = s.share.as_mut() {
            share.name = name.clone();
            saved = true;
        }
    })?;
    if !saved {
        return Err("기기 간 공유를 먼저 연결해 주세요.".into());
    }
    Ok(name)
}

#[tauri::command]
pub fn generate_share_key() -> String {
    generate_key()
}

/// Copies a secret such as the sharing key without recording it in the history.
#[tauri::command]
pub fn copy_secret(text: String) -> Result<(), String> {
    clipboard::copy_unrecorded(&text)
}

/// Checks the settings against the relay, then saves them.
#[tauri::command]
pub async fn connect_share(app: AppHandle, config: ShareConfig) -> Result<Status, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let config = normalize(&config)?;
        let me = Relay::new(&config)?.me()?;
        app.state::<SettingsStore>().update(|s| s.share = Some(config))?;
        let state = app.state::<ShareState>();
        let status = Status::Connected { name: me.name, peers: me.peers };
        state.set(&app, status.clone());
        state.settings_changed();
        Ok(status)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn disconnect_share(app: AppHandle) -> Result<(), String> {
    app.state::<SettingsStore>().update(|s| s.share = None)?;
    let state = app.state::<ShareState>();
    state.set(&app, Status::Off);
    state.settings_changed();
    Ok(())
}

/// Sends a history item to every other device. Returns how many.
#[tauri::command]
pub async fn send_item(app: AppHandle, id: i64) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let config = app
            .state::<SettingsStore>()
            .get()
            .share
            .ok_or("설정에서 기기 간 공유를 먼저 연결해 주세요.")?;
        let item = app.state::<Db>().get(id).map_err(|e| e.to_string())?.ok_or("항목을 찾을 수 없어요.")?;
        Relay::new(&config)?.push(&item)
    })
    .await
    .map_err(|e| e.to_string())?
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
        let Some(config) = app.state::<SettingsStore>().get().share else {
            state.set(&app, Status::Off);
            state.wait(seen, Duration::from_secs(3600));
            continue;
        };
        let relay = match Relay::new(&config) {
            Ok(relay) => relay,
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
                Ok(me) => state.set(&app, Status::Connected { name: me.name, peers: me.peers }),
                Err(message) => state.set(&app, Status::Error { message }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(key: &str) -> ShareConfig {
        ShareConfig {
            name: "  집 맥  ".into(),
            server: " https://relay.example.org/ ".into(),
            token: " pr1_abc ".into(),
            key: key.into(),
        }
    }

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
    }

    #[test]
    fn ids_and_keys_have_the_expected_shape() {
        let id = random_id();
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert_eq!(generate_key().len(), 43);
        assert!(Key::parse("short").is_err());
        assert!(Key::parse(&format!(" {} ", generate_key())).is_ok(), "whitespace from copying is ignored");
    }

    #[test]
    fn settings_are_normalized_and_validated() {
        let key = generate_key();
        let normalized = normalize(&config(&key)).unwrap();
        assert_eq!(normalized.server, "https://relay.example.org");
        assert_eq!(normalized.token, "pr1_abc");
        assert_eq!(normalized.name, "집 맥");
        assert_eq!(normalize(&ShareConfig { name: "".into(), ..config(&key) }).unwrap().name, platform::computer_name().unwrap_or_default());
        assert!(normalize(&ShareConfig { server: "http://relay.example.org".into(), ..config(&key) }).is_err());
        assert!(normalize(&ShareConfig { server: "http://127.0.0.1:8787".into(), ..config(&key) }).is_ok());
        assert!(normalize(&ShareConfig { token: "abc".into(), ..config(&key) }).is_err());
        assert!(normalize(&config("not a key")).is_err());
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
        let old: Meta = serde_json::from_str(r#"{"v":1,"kind":"text","text":"x"}"#).unwrap();
        assert_eq!(old.from, None, "items from older senders still open");
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

    /// Talks to a real relay. Run with:
    /// PINCHY_TEST_RELAY=http://127.0.0.1:18787 PINCHY_TEST_TOKEN_A=pr1_.. PINCHY_TEST_TOKEN_B=pr1_..
    /// cargo test share::tests::relay_round_trip -- --ignored
    #[test]
    #[ignore]
    fn relay_round_trip() {
        let server = std::env::var("PINCHY_TEST_RELAY").unwrap();
        let key = generate_key();
        let relay = |token: &str| {
            Relay::new(&ShareConfig {
                name: format!("{token} 이름"),
                server: server.clone(),
                token: std::env::var(token).unwrap(),
                key: key.clone(),
            })
            .unwrap()
        };
        let (a, b) = (relay("PINCHY_TEST_TOKEN_A"), relay("PINCHY_TEST_TOKEN_B"));
        let images = tempfile_dir();
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
        assert_eq!(a.push(&item("text", "회사에서 보낸 문장".into())).unwrap(), 1);
        assert_eq!(a.push(&item("link", "https://example.com/a".into())).unwrap(), 1);
        assert_eq!(a.push(&item("image", source.to_string_lossy().into_owned())).unwrap(), 1);
        assert!(a.push(&item("files", "[]".into())).is_err());
        let incoming = b.inbox(5).unwrap();
        assert_eq!(incoming.len(), 3);
        let opened: Vec<NewItem> =
            incoming.iter().map(|i| b.open(i).ok().unwrap().into_item(&images).unwrap()).collect();
        assert_eq!(opened[0].content, "회사에서 보낸 문장");
        assert_eq!(opened[1].kind, "link");
        assert_eq!((opened[2].kind.as_str(), opened[2].width, opened[2].height), ("image", Some(4), Some(5)));
        assert_eq!(std::fs::read(&opened[2].content).unwrap(), png);
        assert!(opened.iter().all(|i| i.origin_device.as_deref() == Some("PINCHY_TEST_TOKEN_A 이름")));
        let wrong = Relay::new(&ShareConfig {
            name: String::new(),
            server: server.clone(),
            token: std::env::var("PINCHY_TEST_TOKEN_B").unwrap(),
            key: generate_key(),
        })
        .unwrap();
        assert!(matches!(wrong.open(&incoming[0]), Err(ReceiveError::Locked)));
        for i in &incoming {
            b.ack(&i.id).unwrap();
        }
        assert!(b.inbox(0).unwrap().is_empty());
        std::fs::remove_dir_all(images).unwrap();
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pinchy-share-{}-{}", std::process::id(), crate::db::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
