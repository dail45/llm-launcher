//! Starts (and restarts) the management/UI server and the router server.
use crate::app::AppRef;
use anyhow::{Context, Result};
use axum::http::{HeaderValue, Method};
use parking_lot::Mutex;
use std::net::SocketAddr;
use std::sync::OnceLock;
use tokio::sync::oneshot;
use tower_http::cors::{AllowOrigin, CorsLayer};

struct Running {
    stop: Vec<oneshot::Sender<()>>,
}
/// port of the running HTTPS listener (0 = off)
pub static HTTPS_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
static RUNNING: OnceLock<Mutex<Option<Running>>> = OnceLock::new();

fn cors(app: &AppRef) -> CorsLayer {
    let origins = app.settings().cors_origins;
    let base = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
        .allow_headers(tower_http::cors::Any);
    // Tauri webview origins + configured ones
    let mut list: Vec<HeaderValue> = ["tauri://localhost", "http://tauri.localhost", "https://tauri.localhost", "http://localhost:5173", "http://127.0.0.1:5173"].iter().filter_map(|s| HeaderValue::from_str(s).ok()).collect();
    for o in origins {
        if o == "*" {
            return base.allow_origin(tower_http::cors::Any);
        }
        if let Ok(h) = HeaderValue::from_str(&o) {
            list.push(h);
        }
    }
    base.allow_origin(AllowOrigin::list(list))
}

async fn bind(addr: &str, port: u16) -> Result<tokio::net::TcpListener> {
    let a = format!("{addr}:{port}");
    tokio::net::TcpListener::bind(&a).await.with_context(|| format!("не удалось занять {a}"))
}

pub async fn start(app: AppRef) -> Result<()> {
    let s = app.settings();
    let ui = bind(&s.ui_bind, s.ui_port).await?;
    let rt = bind(&s.router_bind, s.router_port).await?;
    let (tx1, rx1) = oneshot::channel::<()>();
    let (tx2, rx2) = oneshot::channel::<()>();
    let api = crate::api::router(app.clone()).layer(cors(&app));
    let router = crate::router::router(app.clone()).layer(CorsLayer::permissive());
    tokio::spawn(async move {
        let r = axum::serve(ui, api.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(async {
            let _ = rx1.await;
        });
        if let Err(e) = r.await {
            tracing::error!("ui server: {e}");
        }
    });
    tokio::spawn(async move {
        let r = axum::serve(rt, router.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(async {
            let _ = rx2.await;
        });
        if let Err(e) = r.await {
            tracing::error!("router server: {e}");
        }
    });
    let mut stop = vec![tx1, tx2];
    HTTPS_PORT.store(0, std::sync::atomic::Ordering::Relaxed);
    if s.https_enabled {
        match start_https(&app).await {
            Ok(tx) => stop.push(tx),
            Err(e) => {
                tracing::error!("https: {e:#}");
                app.bus.toast("error", "HTTPS не запущен", format!("{e:#}"));
            }
        }
    }
    tracing::info!("UI/API on http://{}:{}, router on http://{}:{}", s.ui_bind, s.ui_port, s.router_bind, s.router_port);
    *RUNNING.get_or_init(Default::default).lock() = Some(Running { stop });
    Ok(())
}

/// Optional HTTPS listener for the same UI/API (service workers / PWA install need a secure origin).
async fn start_https(app: &AppRef) -> Result<oneshot::Sender<()>> {
    let s = app.settings();
    anyhow::ensure!(!s.https_cert.trim().is_empty() && !s.https_key.trim().is_empty(), "не указаны файлы сертификата и ключа");
    anyhow::ensure!(s.https_port != s.ui_port && s.https_port != s.router_port, "порт HTTPS совпадает с другим портом");
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cfg = axum_server::tls_rustls::RustlsConfig::from_pem_file(s.https_cert.trim(), s.https_key.trim())
        .await
        .context("не удалось прочитать сертификат/ключ (нужны PEM-файлы)")?;
    let l = bind(&s.ui_bind, s.https_port).await?.into_std()?;
    let api = crate::api::router(app.clone()).layer(cors(app));
    let handle = axum_server::Handle::new();
    let h2 = handle.clone();
    let (tx, rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = rx.await;
        h2.graceful_shutdown(Some(std::time::Duration::from_secs(1)));
    });
    let srv = axum_server::from_tcp_rustls(l, cfg)?.handle(handle);
    tokio::spawn(async move {
        if let Err(e) = srv.serve(api.into_make_service_with_connect_info::<SocketAddr>()).await {
            tracing::error!("https server: {e}");
        }
    });
    HTTPS_PORT.store(s.https_port, std::sync::atomic::Ordering::Relaxed);
    tracing::info!("UI/API also on https://{}:{}", s.ui_bind, s.https_port);
    Ok(tx)
}

pub async fn restart(app: AppRef) -> Result<()> {
    if let Some(r) = RUNNING.get_or_init(Default::default).lock().take() {
        for s in r.stop {
            let _ = s.send(());
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    start(app).await
}

/// Full boot: logging, restore pending backup, app, background tasks, servers.
pub async fn boot() -> Result<AppRef> {
    let dir = crate::paths::data_dir();
    std::fs::create_dir_all(&dir)?;
    let restore = dir.join("launcher.db.restore");
    if restore.exists() {
        let db = crate::paths::db_path();
        let _ = std::fs::rename(&db, dir.join(format!("launcher.db.bak-{}", chrono::Local::now().format("%Y%m%d%H%M%S"))));
        let _ = std::fs::remove_file(dir.join("launcher.db-wal"));
        let _ = std::fs::remove_file(dir.join("launcher.db-shm"));
        std::fs::rename(&restore, &db)?;
    }
    let app = crate::app::App::open()?;
    crate::downloads::ensure_download_dir(&app).ok();
    app.spawn_background();
    start(app.clone()).await?;
    Ok(app)
}

pub fn init_logging() -> tracing_appender::non_blocking::WorkerGuard {
    use tracing_subscriber::prelude::*;
    let dir = crate::paths::logs_dir();
    let _ = std::fs::create_dir_all(&dir);
    let file = tracing_appender::rolling::Builder::new().rotation(tracing_appender::rolling::Rotation::DAILY).filename_prefix("launcher").filename_suffix("log").max_log_files(14).build(&dir).expect("log dir");
    let (nb, guard) = tracing_appender::non_blocking(file);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,hyper=warn,reqwest=warn".into());
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(nb))
        .try_init();
    guard
}
