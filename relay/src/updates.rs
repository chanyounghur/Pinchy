//! Over-the-air updates for the phone app (the expo-updates protocol,
//! version 1). The release workflow publishes a signed manifest per runtime
//! and platform to GitHub; the relay fetches it, caches it briefly and
//! serves it with the headers expo-updates insists on. The relay never signs
//! anything, so it cannot push code of its own, and the bundle and assets the
//! manifest points at are downloaded by the phone from GitHub directly.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// How long a fetched manifest is reused before GitHub is asked again.
const TTL: Duration = Duration::from_secs(300);
/// A manifest is a few kilobytes; anything larger is not ours.
const MAX_MANIFEST_BYTES: usize = 512 * 1024;

pub struct Manifest {
    pub body: Vec<u8>,
    /// Base64 RSA signature over `body`, when the release was signed.
    pub signature: Option<String>,
}

struct Cached {
    at: Instant,
    manifest: Option<Arc<Manifest>>,
}

pub struct Updates {
    source: String,
    client: reqwest::Client,
    cache: Mutex<HashMap<String, Cached>>,
}

impl Updates {
    /// `source` is the directory URL that holds `<runtime>/<platform>/manifest.json`.
    pub fn new(source: &str) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent("pinchy-relay")
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Updates { source: source.trim_end_matches('/').to_string(), client, cache: Mutex::default() })
    }

    /// The manifest for a runtime and platform, or `None` when the release
    /// workflow has not published one. A stale copy is served while GitHub
    /// is unreachable.
    pub async fn manifest(&self, runtime: &str, platform: &str) -> Result<Option<Arc<Manifest>>, String> {
        if !valid_segment(runtime) || !matches!(platform, "ios" | "android") {
            return Ok(None);
        }
        let key = format!("{runtime}/{platform}");
        let stale = {
            let cache = self.cache.lock().unwrap();
            match cache.get(&key) {
                Some(c) if c.at.elapsed() < TTL => return Ok(c.manifest.clone()),
                Some(c) => Some(c.manifest.clone()),
                None => None,
            }
        };
        match self.fetch(&key).await {
            Ok(manifest) => {
                let manifest = manifest.map(Arc::new);
                self.cache.lock().unwrap().insert(key, Cached { at: Instant::now(), manifest: manifest.clone() });
                Ok(manifest)
            }
            Err(e) => match stale {
                Some(manifest) => Ok(manifest),
                None => Err(e),
            },
        }
    }

    async fn fetch(&self, key: &str) -> Result<Option<Manifest>, String> {
        let Some(body) = self.get(&format!("{}/{key}/manifest.json", self.source)).await? else {
            return Ok(None);
        };
        let signature = self
            .get(&format!("{}/{key}/manifest.sig", self.source))
            .await?
            .map(|s| String::from_utf8_lossy(&s).trim().to_string())
            .filter(|s| !s.is_empty());
        Ok(Some(Manifest { body, signature }))
    }

    async fn get(&self, url: &str) -> Result<Option<Vec<u8>>, String> {
        let response = self.client.get(url).send().await.map_err(|e| format!("{url}: {e}"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(format!("{url}: HTTP {}", response.status()));
        }
        let bytes = response.bytes().await.map_err(|e| format!("{url}: {e}"))?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(format!("{url}: too large"));
        }
        Ok(Some(bytes.to_vec()))
    }
}

/// Runtime versions are short identifiers; anything else is not looked up.
fn valid_segment(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A stand-in for raw.githubusercontent.com with a signed ios manifest,
    /// an unsigned android one and nothing for other runtimes.
    async fn github(hits: Arc<AtomicUsize>) -> String {
        let counting = hits.clone();
        let app = Router::new()
            .route(
                "/updates/1/ios/manifest.json",
                get(move || {
                    counting.fetch_add(1, Ordering::SeqCst);
                    async { r#"{"id":"ios"}"# }
                }),
            )
            .route("/updates/1/ios/manifest.sig", get(|| async { "c2ln\n" }))
            .route("/updates/1/android/manifest.json", get(|| async { r#"{"id":"android"}"# }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/updates")
    }

    #[tokio::test]
    async fn manifests_are_fetched_with_signatures_and_cached() {
        let hits = Arc::new(AtomicUsize::new(0));
        let updates = Updates::new(&github(hits.clone()).await).unwrap();
        let ios = updates.manifest("1", "ios").await.unwrap().unwrap();
        assert_eq!(ios.body, br#"{"id":"ios"}"#);
        assert_eq!(ios.signature.as_deref(), Some("c2ln"));
        let android = updates.manifest("1", "android").await.unwrap().unwrap();
        assert_eq!(android.signature, None, "unsigned releases are served without a signature");
        assert!(updates.manifest("2", "ios").await.unwrap().is_none(), "unknown runtime");
        assert!(updates.manifest("1", "web").await.unwrap().is_none(), "unknown platform");
        assert!(updates.manifest("../1", "ios").await.unwrap().is_none(), "no path games");
        updates.manifest("1", "ios").await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1, "second lookup within the TTL is served from cache");
    }

    #[tokio::test]
    async fn unreachable_sources_fail_without_a_cached_copy() {
        let updates = Updates::new("http://127.0.0.1:1/updates").unwrap();
        assert!(updates.manifest("1", "ios").await.is_err());
    }
}
