//! Pinchy relay: a mailbox server that Pinchy clients reach over HTTPS.
//!
//! The first device joins with a code the relay prints while it has no
//! devices; later devices are invited by connected ones (see `pairing`). An
//! item sent by one device is queued for every other device until each has
//! acknowledged it or it expires. Contents are encrypted by the clients.

mod api;
mod pairing;
mod store;

use clap::Parser;
use rustls_acme::{caches::DirCache, AcmeConfig};
use std::{net::SocketAddr, path::PathBuf, time::Duration};
use tokio_stream::StreamExt;

const DAY_MS: i64 = 86_400_000;
/// Blob items whose upload has not finished by then are dropped.
const STALE_UPLOAD_MS: i64 = 3_600_000;

#[derive(Parser, Debug)]
#[command(version, about = "Pinchy relay server")]
struct Args {
    /// Where the database, blobs and certificates are kept.
    #[arg(long, env = "PINCHY_RELAY_DATA_DIR", default_value_os_t = default_data_dir())]
    data_dir: PathBuf,

    /// Address to listen on.
    #[arg(long, env = "PINCHY_RELAY_LISTEN", default_value = "127.0.0.1:8787")]
    listen: SocketAddr,

    /// Public domain. When set, the relay serves HTTPS and obtains its
    /// certificate from Let's Encrypt (TLS-ALPN-01, so the domain's port 443
    /// must reach `--listen`). Without it, the relay serves plain HTTP for
    /// local testing or for running behind a reverse proxy.
    #[arg(long, env = "PINCHY_RELAY_DOMAIN")]
    domain: Option<String>,

    /// Where certificates and the ACME account are kept. Reusing it across
    /// restarts avoids Let's Encrypt's duplicate-certificate rate limit.
    /// Defaults to `acme` inside the data directory.
    #[arg(long, env = "PINCHY_RELAY_CACHE_DIR")]
    cache_dir: Option<PathBuf>,

    /// Contact email for Let's Encrypt expiry notices.
    #[arg(long, env = "PINCHY_RELAY_CONTACT")]
    contact: Option<String>,

    /// Use the Let's Encrypt staging directory (untrusted test certificates).
    #[arg(long, env = "PINCHY_RELAY_STAGING")]
    staging: bool,

    /// Days an undelivered item is kept.
    #[arg(long, env = "PINCHY_RELAY_RETENTION_DAYS", default_value_t = 7)]
    retention_days: u32,

    /// Largest accepted image or file, in megabytes.
    #[arg(long, env = "PINCHY_RELAY_MAX_BLOB_MB", default_value_t = 64)]
    max_blob_mb: u64,
}

fn default_data_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/pinchy-relay")
    } else {
        home.join(".local/share/pinchy-relay")
    }
}

fn open_store(data_dir: &PathBuf) -> store::Store {
    store::Store::open(data_dir).unwrap_or_else(|e| {
        eprintln!("[pinchy-relay] cannot open data directory {}: {e}", data_dir.display());
        std::process::exit(1);
    })
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let state = api::AppState::new(
        open_store(&args.data_dir),
        api::Limits { max_envelope_bytes: 8 * 1024 * 1024, max_blob_bytes: args.max_blob_mb * 1024 * 1024 },
    );
    let retention_ms = i64::from(args.retention_days) * DAY_MS;
    let maintaining = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        for minute in 0u64.. {
            tick.tick().await;
            maintaining.announce_first_code();
            if minute % 10 != 0 {
                continue;
            }
            match maintaining.store.purge(store::now_ms(), retention_ms, STALE_UPLOAD_MS) {
                Ok(0) => {}
                Ok(n) => eprintln!("[pinchy-relay] removed {n} expired item(s)"),
                Err(e) => eprintln!("[pinchy-relay] purge failed: {e}"),
            }
        }
    });
    let app = api::router(state).into_make_service_with_connect_info::<SocketAddr>();

    let Some(domain) = args.domain else {
        let listener = tokio::net::TcpListener::bind(args.listen)
            .await
            .unwrap_or_else(|e| panic!("[pinchy-relay] cannot listen on {}: {e}", args.listen));
        eprintln!("[pinchy-relay] serving HTTP on {}", args.listen);
        axum::serve(listener, app).await.expect("serve");
        return;
    };

    let cache_dir = args.cache_dir.unwrap_or_else(|| args.data_dir.join("acme"));
    let mut acme = AcmeConfig::new([domain.clone()])
        .contact(args.contact.iter().map(|e| format!("mailto:{e}")))
        .cache(DirCache::new(cache_dir))
        .directory_lets_encrypt(!args.staging)
        .state();
    let acceptor = acme.axum_acceptor(acme.default_rustls_config());
    tokio::spawn(async move {
        while let Some(event) = acme.next().await {
            match event {
                Ok(ok) => eprintln!("[pinchy-relay] acme: {ok:?}"),
                Err(err) => eprintln!("[pinchy-relay] acme error: {err:?}"),
            }
        }
    });

    eprintln!("[pinchy-relay] serving HTTPS for {domain} on {}", args.listen);
    axum_server::bind(args.listen).acceptor(acceptor).serve(app).await.expect("serve");
}
