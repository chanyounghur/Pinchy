//! Pinchy relay: a mailbox server that Pinchy clients reach over HTTPS.
//!
//! For now it only answers `/health`, so a network path (DDNS, port
//! forwarding, corporate proxies) can be verified before the mailbox exists.

use axum::{routing::get, Json, Router};
use clap::Parser;
use rustls_acme::{caches::DirCache, AcmeConfig};
use serde_json::{json, Value};
use std::{net::SocketAddr, path::PathBuf};
use tokio_stream::StreamExt;

#[derive(Parser, Debug)]
#[command(version, about = "Pinchy relay server")]
struct Args {
    /// Address to listen on.
    #[arg(long, env = "PINCHY_RELAY_LISTEN", default_value = "127.0.0.1:8787")]
    listen: SocketAddr,

    /// Public domain. When set, the relay serves HTTPS and obtains its
    /// certificate from Let's Encrypt (TLS-ALPN-01, so the domain's port 443
    /// must reach `--listen`). Without it, the relay serves plain HTTP for
    /// local testing or for running behind a reverse proxy.
    #[arg(long, env = "PINCHY_RELAY_DOMAIN", requires = "cache_dir")]
    domain: Option<String>,

    /// Where certificates and the ACME account are kept. Reusing it across
    /// restarts avoids Let's Encrypt's duplicate-certificate rate limit.
    #[arg(long, env = "PINCHY_RELAY_CACHE_DIR")]
    cache_dir: Option<PathBuf>,

    /// Contact email for Let's Encrypt expiry notices.
    #[arg(long, env = "PINCHY_RELAY_CONTACT")]
    contact: Option<String>,

    /// Use the Let's Encrypt staging directory (untrusted test certificates).
    #[arg(long, env = "PINCHY_RELAY_STAGING")]
    staging: bool,
}

async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "pinchy-relay",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

fn app() -> Router {
    Router::new().route("/health", get(health))
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let Some(domain) = args.domain else {
        let listener = tokio::net::TcpListener::bind(args.listen)
            .await
            .unwrap_or_else(|e| panic!("[pinchy-relay] cannot listen on {}: {e}", args.listen));
        eprintln!("[pinchy-relay] serving HTTP on {}", args.listen);
        axum::serve(listener, app()).await.expect("serve");
        return;
    };

    let mut state = AcmeConfig::new([domain.clone()])
        .contact(args.contact.iter().map(|e| format!("mailto:{e}")))
        .cache(DirCache::new(args.cache_dir.expect("required by clap")))
        .directory_lets_encrypt(!args.staging)
        .state();
    let acceptor = state.axum_acceptor(state.default_rustls_config());
    tokio::spawn(async move {
        while let Some(event) = state.next().await {
            match event {
                Ok(ok) => eprintln!("[pinchy-relay] acme: {ok:?}"),
                Err(err) => eprintln!("[pinchy-relay] acme error: {err:?}"),
            }
        }
    });

    eprintln!("[pinchy-relay] serving HTTPS for {domain} on {}", args.listen);
    axum_server::bind(args.listen)
        .acceptor(acceptor)
        .serve(app().into_make_service())
        .await
        .expect("serve");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn get(path: &str) -> (u16, Vec<u8>) {
        let response = app()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status().as_u16();
        (status, response.into_body().collect().await.unwrap().to_bytes().to_vec())
    }

    #[tokio::test]
    async fn health_reports_service_and_version() {
        let (status, body) = get("/health").await;
        assert_eq!(status, 200);
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["service"], "pinchy-relay");
        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn other_paths_are_not_found() {
        assert_eq!(get("/").await.0, 404);
        assert_eq!(get("/admin").await.0, 404);
    }
}
