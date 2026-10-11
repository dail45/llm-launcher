//! Management REST API (+ SSE events, static UI).
use crate::app::AppRef;
use crate::args;
use crate::auth::Principal;
use crate::downloads::{self, Download};
use crate::events::Event;
use crate::help_parser::ParamSchema;
use crate::http_util::{lan_ip, req_meta, resolve_principal, Admin, ApiError, R};
use crate::memory;
use crate::types::*;
use axum::body::Bytes;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post, put};
use axum::{Json, Router};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

type J = Json<Value>;
fn ok() -> R<J> {
    Ok(Json(json!({"ok": true})))
}
fn j<T: serde::Serialize>(v: T) -> R<J> {
    Ok(Json(serde_json::to_value(v).map_err(|e| ApiError::bad(e.to_string()))?))
}

#[derive(rust_embed::Embed)]
#[folder = "$CARGO_MANIFEST_DIR/../../ui/dist"]
struct UiAssets;

pub fn router(app: AppRef) -> Router {
    Router::new()
        // auth
        .route("/api/version", get(version))
        .route("/api/auth/status", get(auth_status))
        .route("/api/auth/login", post(login))
        .route("/api/auth/refresh", post(refresh))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/password", post(set_password))
        .route("/api/auth/sessions", get(sessions))
        .route("/api/auth/sessions/{id}", delete(delete_session))
        .route("/api/security-log", get(security_log))
        .route("/api/keys", get(list_keys).post(create_key))
        .route("/api/keys/{id}", put(update_key).delete(delete_key))
        // system
        .route("/api/status", get(status))
        .route("/api/system", get(system))
        .route("/api/system/autostart", get(get_autostart).put(put_autostart))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/events", get(events))
        .route("/api/jobs", get(jobs))
        .route("/api/backup", get(backup))
        .route("/api/restore", post(restore))
        // backends
        .route("/api/backends", get(list_backends).post(add_backend))
        .route("/api/backends/{id}", put(update_backend).delete(delete_backend))
        .route("/api/backends/{id}/rescan", post(rescan_backend))
        .route("/api/backends/{id}/schema", get(backend_schema))
        .route("/api/backends/{id}/default", post(default_backend))
        .route("/api/github/sources", get(gh_sources).post(gh_add_source))
        .route("/api/github/sources/{id}", delete(gh_del_source))
        .route("/api/github/releases", get(gh_releases))
        .route("/api/github/install", post(gh_install))
        .route("/api/github/updates", get(gh_updates))
        // models
        .route("/api/model-dirs", get(list_dirs).post(add_dir))
        .route("/api/model-dirs/{id}", put(update_dir).delete(delete_dir))
        .route("/api/models", get(list_models))
        .route("/api/models/scan", post(scan_models))
        .route("/api/models/add-file", post(add_model_file))
        .route("/api/models/{id}", get(get_model).put(update_model).delete(delete_model))
        // presets
        .route("/api/presets", get(list_presets).post(create_preset))
        .route("/api/presets/import-command", post(import_command))
        .route("/api/presets/export", get(export_presets))
        .route("/api/presets/import", post(import_presets))
        .route("/api/presets/{id}", get(get_preset).put(update_preset).delete(delete_preset))
        .route("/api/presets/{id}/clone", post(clone_preset))
        .route("/api/presets/{id}/history", get(preset_history))
        .route("/api/presets/{id}/restore/{version}", post(restore_preset))
        .route("/api/presets/{id}/command", get(preset_command))
        .route("/api/presets/{id}/start", post(start_preset))
        .route("/api/presets/{id}/stop", post(stop_preset))
        .route("/api/presets/{id}/calibrate", post(calibrate))
        .route("/api/preview-command", post(preview_command))
        // aliases
        .route("/api/aliases", get(list_aliases).post(create_alias).delete(delete_alias))
        .route("/api/alias-groups", get(list_groups).post(create_group))
        .route("/api/alias-groups/{id}", put(update_group).delete(delete_group))
        // memory
        .route("/api/memory/estimate", post(mem_estimate))
        // instances
        .route("/api/instances", get(list_instances))
        .route("/api/instances/{id}", delete(remove_instance))
        .route("/api/instances/{id}/logs", get(instance_logs))
        .route("/api/instances/{id}/stop", post(stop_instance))
        .route("/api/instances/{id}/pin", post(pin_instance))
        .route("/api/instances/stop-all", post(stop_all))
        // hf + downloads
        .route("/api/hf/search", get(hf_search))
        .route("/api/hf/files", get(hf_files))
        .route("/api/hf/readme", get(hf_readme))
        .route("/api/hf/estimate", post(hf_estimate))
        .route("/api/hf/download", post(hf_download))
        .route("/api/downloads", get(list_downloads))
        .route("/api/downloads/{id}", delete(delete_download))
        .route("/api/downloads/{id}/pause", post(pause_download))
        .route("/api/downloads/{id}/resume", post(resume_download))
        // bench
        .route("/api/bench", get(list_bench).post(start_bench))
        .route("/api/bench/{id}", delete(delete_bench))
        // router info & chat proxy
        .route("/api/router/requests", get(router_requests))
        .route("/api/analytics", get(analytics).delete(analytics_clear))
        .route("/api/analytics/export", get(analytics_export))
        .route("/api/router/models", get(router_models))
        .route("/api/v1/{*rest}", any(chat_proxy))
        .fallback(static_ui)
        .with_state(app)
}

// ---------------- static UI
async fn static_ui(req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    if path.starts_with("api/") {
        return ApiError::not_found("unknown endpoint").into_response();
    }
    let (file, data) = match UiAssets::get(if path.is_empty() { "index.html" } else { path }) {
        Some(d) => (path, d),
        None => match UiAssets::get("index.html") {
            Some(d) => ("index.html", d),
            None => return (StatusCode::NOT_FOUND, "UI not built").into_response(),
        },
    };
    let file = if file.is_empty() { "index.html" } else { file };
    let mime = mime_guess::from_path(file).first_or_octet_stream();
    let cache = if file == "index.html" { "no-cache" } else { "public, max-age=31536000, immutable" };
    ([(header::CONTENT_TYPE, mime.as_ref().to_string()), (header::CACHE_CONTROL, cache.to_string())], data.data.into_owned()).into_response()
}

// ---------------- auth
async fn version() -> R<J> {
    j(json!({ "version": crate::VERSION, "commit": crate::GIT_SHA, "build_date": crate::BUILD_DATE }))
}

async fn auth_status(State(app): State<AppRef>, req: Request) -> R<J> {
    let (parts, _) = req.into_parts();
    let meta = req_meta(&app, &parts);
    let p = resolve_principal(&app, &parts, false).ok();
    let s = app.settings();
    j(json!({
        "authenticated": p.as_ref().map(|x| x.0.can_manage()).unwrap_or(false),
        "principal": p.map(|x| x.0.label()),
        "local": meta.local,
        "has_password": app.auth.has_password(),
        "remote_enabled": s.ui_bind != "127.0.0.1" && s.ui_bind != "localhost",
        "version": crate::VERSION,
    }))
}

#[derive(Deserialize)]
struct LoginReq {
    password: String,
}
async fn login(State(app): State<AppRef>, req: Request) -> R<J> {
    let (parts, body) = req.into_parts();
    let meta = req_meta(&app, &parts);
    let ua = parts.headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let bytes = axum::body::to_bytes(body, 1 << 16).await.map_err(|e| ApiError::bad(e.to_string()))?;
    let r: LoginReq = serde_json::from_slice(&bytes).map_err(|e| ApiError::bad(e.to_string()))?;
    let s = app.settings();
    match app.auth.login(&r.password, meta.ip, &ua, s.access_ttl_secs, s.refresh_ttl_days) {
        Ok(t) => j(t),
        Err(e) => Err(ApiError::unauthorized(e.to_string())),
    }
}
#[derive(Deserialize)]
struct RefreshReq {
    refresh_token: String,
}
async fn refresh(State(app): State<AppRef>, Json(r): Json<RefreshReq>) -> R<J> {
    let s = app.settings();
    app.auth.refresh(&r.refresh_token, s.access_ttl_secs, s.refresh_ttl_days).map(|t| Json(serde_json::to_value(t).unwrap())).map_err(|e| ApiError::unauthorized(e.to_string()))
}
async fn logout(State(app): State<AppRef>, Admin(p, _): Admin) -> R<J> {
    if let Principal::Admin { sid } = p {
        app.auth.logout(&sid)?;
    }
    ok()
}
#[derive(Deserialize)]
struct PwReq {
    password: String,
}
async fn set_password(State(app): State<AppRef>, Admin(p, ip): Admin, Json(r): Json<PwReq>) -> R<J> {
    if matches!(p, Principal::Key(_)) {
        return Err(ApiError::forbidden("пароль меняется только из интерфейса"));
    }
    app.auth.set_password(&r.password)?;
    app.auth.log("password_set", &ip.to_string(), "");
    ok()
}
async fn sessions(State(app): State<AppRef>, _a: Admin) -> R<J> {
    let mut v = app.auth.sessions()?;
    v.iter_mut().for_each(|s| s.refresh_hash.clear());
    j(v)
}
async fn delete_session(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    app.auth.logout(&id)?;
    ok()
}
async fn security_log(State(app): State<AppRef>, _a: Admin) -> R<J> {
    j(app.store.log_list("security_log", 300)?)
}
async fn list_keys(State(app): State<AppRef>, _a: Admin) -> R<J> {
    let mut v: Vec<ApiKey> = app.store.list("api_keys")?;
    v.iter_mut().for_each(|k| k.hash.clear());
    j(v)
}
#[derive(Deserialize)]
struct KeyReq {
    name: String,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default = "yes")]
    can_load: bool,
    #[serde(default)]
    can_manage: bool,
    expires_at: Option<i64>,
}
fn yes() -> bool {
    true
}
async fn create_key(State(app): State<AppRef>, Admin(p, _): Admin, Json(r): Json<KeyReq>) -> R<J> {
    if matches!(p, Principal::Key(_)) {
        return Err(ApiError::forbidden("ключи создаются только из интерфейса"));
    }
    let (mut k, plain) = app.auth.create_key(&r.name, r.scopes, r.can_load, r.can_manage, r.expires_at)?;
    k.hash.clear();
    j(json!({"key": k, "secret": plain}))
}
async fn update_key(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>, Json(r): Json<KeyReq>) -> R<J> {
    let mut k: ApiKey = app.store.get("api_keys", &id)?.ok_or_else(|| ApiError::not_found("ключ не найден"))?;
    k.name = r.name;
    k.scopes = if r.scopes.is_empty() { vec!["*".into()] } else { r.scopes };
    k.can_load = r.can_load;
    k.can_manage = r.can_manage;
    k.expires_at = r.expires_at;
    app.store.put("api_keys", &id, &k)?;
    ok()
}
async fn delete_key(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    app.store.delete("api_keys", &id)?;
    app.auth.log("api_key_deleted", "", &id);
    ok()
}

// ---------------- system
async fn status(State(app): State<AppRef>, _a: Admin) -> R<J> {
    let s = app.settings();
    let lan = lan_ip().map(|i| i.to_string());
    j(json!({
        "version": crate::VERSION,
        "commit": crate::GIT_SHA,
        "build_date": crate::BUILD_DATE,
        "data_dir": crate::paths::data_dir(),
        "ui_port": s.ui_port, "router_port": s.router_port, "ui_bind": s.ui_bind, "router_bind": s.router_bind,
        "lan_ip": lan,
        "os": std::env::consts::OS,
        "counts": {
            "backends": app.store.list::<Value>("backends")?.len(),
            "models": app.store.list::<Value>("models")?.len(),
            "presets": app.store.list::<Value>("presets")?.len(),
            "instances": app.pm.alive().len(),
        },
        "has_password": app.auth.has_password(),
    }))
}
async fn system(State(app): State<AppRef>, _a: Admin) -> R<J> {
    j(app.snapshot.read().clone())
}

// ---------------- autostart with Windows (HKCU Run key)
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "LlamaLauncher";
async fn get_autostart(_a: Admin) -> R<J> {
    if !cfg!(windows) {
        return Err(ApiError::not_found("только Windows"));
    }
    let out = crate::proc_util::command(std::path::Path::new("reg")).args(["query", RUN_KEY, "/v", RUN_NAME]).output().await.map_err(|e| ApiError::bad(e.to_string()))?;
    j(json!({"enabled": out.status.success()}))
}
#[derive(Deserialize)]
struct AutostartReq {
    enabled: bool,
}
async fn put_autostart(_a: Admin, Json(r): Json<AutostartReq>) -> R<J> {
    if !cfg!(windows) {
        return Err(ApiError::not_found("только Windows"));
    }
    let exe = std::env::current_exe().map_err(|e| ApiError::bad(e.to_string()))?;
    let val = format!("\"{}\" --minimized", exe.display());
    let mut c = crate::proc_util::command(std::path::Path::new("reg"));
    if r.enabled {
        c.args(["add", RUN_KEY, "/v", RUN_NAME, "/t", "REG_SZ", "/d", &val, "/f"]);
    } else {
        c.args(["delete", RUN_KEY, "/v", RUN_NAME, "/f"]);
    }
    let out = c.output().await.map_err(|e| ApiError::bad(e.to_string()))?;
    if !out.status.success() && r.enabled {
        return Err(ApiError::bad(String::from_utf8_lossy(&out.stderr).to_string()));
    }
    ok()
}

const MASK: &str = "••••••••";
async fn get_settings(State(app): State<AppRef>, _a: Admin) -> R<J> {
    let mut s = app.settings();
    if !s.hf_token.is_empty() {
        s.hf_token = MASK.into();
    }
    if !s.github_token.is_empty() {
        s.github_token = MASK.into();
    }
    j(s)
}
async fn put_settings(State(app): State<AppRef>, _a: Admin, Json(v): Json<Value>) -> R<J> {
    let old = app.settings();
    let mut merged = serde_json::to_value(&old).unwrap();
    if let (Some(m), Some(o)) = (merged.as_object_mut(), v.as_object()) {
        for (k, val) in o {
            if (k == "hf_token" || k == "github_token") && val.as_str() == Some(MASK) {
                continue;
            }
            m.insert(k.clone(), val.clone());
        }
    }
    let new: Settings = serde_json::from_value(merged).map_err(|e| ApiError::bad(format!("некорректные настройки: {e}")))?;
    if new.internal_port_start >= new.internal_port_end {
        return Err(ApiError::bad("некорректный диапазон портов"));
    }
    let restart = new.ui_bind != old.ui_bind || new.ui_port != old.ui_port || new.router_bind != old.router_bind || new.router_port != old.router_port;
    app.save_settings(new)?;
    if restart {
        let a = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            if let Err(e) = crate::server::restart(a.clone()).await {
                a.bus.toast("error", "Не удалось перезапустить сервер", e.to_string());
            }
        });
    }
    j(json!({"ok": true, "restarting": restart}))
}

async fn events(State(app): State<AppRef>, _a: Admin) -> impl IntoResponse {
    let rx = app.bus.subscribe();
    let init = Event::System { snapshot: serde_json::to_value(app.snapshot.read().clone()).unwrap_or_default() };
    let first = futures::stream::once(async move { Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default().data(serde_json::to_string(&init).unwrap_or_default())) });
    let rest = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(|e| async move {
        match e {
            Ok(ev) => Some(Ok(axum::response::sse::Event::default().data(serde_json::to_string(&ev).unwrap_or_default()))),
            Err(_) => None,
        }
    });
    Sse::new(first.chain(rest)).keep_alive(KeepAlive::default())
}
async fn jobs(State(app): State<AppRef>, _a: Admin) -> R<J> {
    j(app.jobs.list())
}

async fn backup(State(app): State<AppRef>, _a: Admin) -> R<Response> {
    let tmp = std::env::temp_dir().join(format!("lll-backup-{}.db", new_id()));
    app.store.backup_to(&tmp)?;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file("launcher.db", opts).map_err(|e| ApiError::bad(e.to_string()))?;
        std::io::copy(&mut std::fs::File::open(&tmp).map_err(|e| ApiError::bad(e.to_string()))?, &mut z).map_err(|e| ApiError::bad(e.to_string()))?;
        z.start_file("manifest.json", opts).map_err(|e| ApiError::bad(e.to_string()))?;
        use std::io::Write;
        z.write_all(json!({"version": env!("CARGO_PKG_VERSION"), "created": now()}).to_string().as_bytes()).map_err(|e| ApiError::bad(e.to_string()))?;
        z.finish().map_err(|e| ApiError::bad(e.to_string()))?;
    }
    let _ = std::fs::remove_file(&tmp);
    let name = format!("llama-launcher-backup-{}.zip", chrono::Local::now().format("%Y%m%d-%H%M"));
    Ok(([(header::CONTENT_TYPE, "application/zip".to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\""))], buf.into_inner()).into_response())
}
async fn restore(State(_app): State<AppRef>, _a: Admin, body: Bytes) -> R<J> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(body.to_vec())).map_err(|e| ApiError::bad(format!("не zip: {e}")))?;
    let mut f = z.by_name("launcher.db").map_err(|_| ApiError::bad("в архиве нет launcher.db"))?;
    let dest = crate::paths::data_dir().join("launcher.db.restore");
    let mut o = std::fs::File::create(&dest).map_err(|e| ApiError::bad(e.to_string()))?;
    std::io::copy(&mut f, &mut o).map_err(|e| ApiError::bad(e.to_string()))?;
    j(json!({"ok": true, "message": "Бэкап будет применён при следующем запуске лаунчера"}))
}

// ---------------- backends
async fn list_backends(State(app): State<AppRef>, _a: Admin) -> R<J> {
    j(app.store.list::<Backend>("backends")?)
}
#[derive(Deserialize)]
struct BackendReq {
    name: String,
    dir: String,
    kind: Option<BackendKind>,
    color: Option<String>,
    notes: Option<String>,
}
async fn add_backend(State(app): State<AppRef>, _a: Admin, Json(r): Json<BackendReq>) -> R<J> {
    let mut b = crate::backends::new_backend(&r.name, &r.dir);
    if let Some(k) = r.kind {
        b.kind = k;
    }
    b.color = r.color;
    b.notes = r.notes.unwrap_or_default();
    if app.store.list::<Backend>("backends")?.is_empty() {
        b.is_default = true;
    }
    app.store.put("backends", &b.id, &b)?;
    match app.rescan_backend(&b.id).await {
        Ok(b) => j(b),
        Err(e) => Err(ApiError::bad(format!("сборка добавлена, но сканирование не удалось: {e}"))),
    }
}
async fn update_backend(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>, Json(r): Json<BackendReq>) -> R<J> {
    let mut b = app.backend(&id)?;
    let dir_changed = b.dir != r.dir;
    b.name = r.name;
    b.dir = r.dir;
    if let Some(k) = r.kind {
        b.kind = k;
    }
    b.color = r.color;
    b.notes = r.notes.unwrap_or_default();
    app.store.put("backends", &id, &b)?;
    if dir_changed {
        return j(app.rescan_backend(&id).await?);
    }
    j(b)
}
#[derive(Deserialize, Default)]
struct DeleteQ {
    #[serde(default)]
    delete_files: bool,
    #[serde(default)]
    delete_partial: bool,
}
async fn delete_backend(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>, Query(q): Query<DeleteQ>) -> R<J> {
    let b = app.backend(&id)?;
    let used: Vec<Preset> = app.store.list::<Preset>("presets")?.into_iter().filter(|p| p.backend_id == id).collect();
    if !used.is_empty() {
        return Err(ApiError::bad(format!("сборка используется пресетами: {}", used.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", "))));
    }
    if q.delete_files && b.managed {
        let _ = std::fs::remove_dir_all(&b.dir);
    }
    app.store.delete("backends", &id)?;
    ok()
}
async fn rescan_backend(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    j(app.rescan_backend(&id).await?)
}
async fn backend_schema(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    let b = app.backend(&id)?;
    let s = app.schema(&b).map(|s| (*s).clone()).unwrap_or_default();
    j(s)
}
async fn default_backend(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    for mut b in app.store.list::<Backend>("backends")? {
        b.is_default = b.id == id;
        app.store.put("backends", &b.id.clone(), &b)?;
    }
    ok()
}
async fn gh_sources(State(app): State<AppRef>, _a: Admin) -> R<J> {
    let mut v: Vec<GhSource> = app.store.list("gh_sources")?;
    if v.is_empty() {
        for s in crate::github::default_sources() {
            app.store.put("gh_sources", &s.id, &s)?;
        }
        v = app.store.list("gh_sources")?;
    }
    j(v)
}
async fn gh_add_source(State(app): State<AppRef>, _a: Admin, Json(mut s): Json<GhSource>) -> R<J> {
    if !s.repo.contains('/') {
        return Err(ApiError::bad("формат: owner/repo"));
    }
    s.repo = s.repo.trim().trim_start_matches("https://github.com/").trim_end_matches('/').to_string();
    if s.id.is_empty() {
        s.id = new_id();
    }
    if s.name.is_empty() {
        s.name = s.repo.clone();
    }
    app.store.put("gh_sources", &s.id, &s)?;
    j(s)
}
async fn gh_del_source(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    app.store.delete("gh_sources", &id)?;
    ok()
}
#[derive(Deserialize)]
struct RepoQ {
    repo: String,
}
async fn gh_releases(State(app): State<AppRef>, _a: Admin, Query(q): Query<RepoQ>) -> R<J> {
    j(crate::github::releases(&app, &q.repo).await?)
}
#[derive(Deserialize)]
struct GhInstallReq {
    repo: String,
    tag: String,
    asset_name: String,
    asset_url: String,
    #[serde(default)]
    extra_urls: Vec<String>,
    name: Option<String>,
}
async fn gh_install(State(app): State<AppRef>, _a: Admin, Json(r): Json<GhInstallReq>) -> R<J> {
    for u in std::iter::once(&r.asset_url).chain(r.extra_urls.iter()) {
        if !u.starts_with("https://github.com/") && !u.starts_with("https://objects.githubusercontent.com/") {
            return Err(ApiError::bad("разрешены только ссылки GitHub"));
        }
    }
    let id = crate::github::install(app.clone(), crate::github::InstallReq { repo: r.repo, tag: r.tag, asset_name: r.asset_name, asset_url: r.asset_url, extra_urls: r.extra_urls, name: r.name });
    j(json!({"job_id": id}))
}
async fn gh_updates(State(app): State<AppRef>, _a: Admin) -> R<J> {
    j(crate::github::check_updates(&app).await?)
}

// ---------------- models
async fn list_dirs(State(app): State<AppRef>, _a: Admin) -> R<J> {
    j(app.store.list::<ModelDir>("model_dirs")?)
}
async fn add_dir(State(app): State<AppRef>, _a: Admin, Json(mut d): Json<ModelDir>) -> R<J> {
    if !std::path::Path::new(&d.path).is_dir() {
        return Err(ApiError::bad("папка не существует"));
    }
    d.id = new_id();
    d.enabled = true;
    app.store.put("model_dirs", &d.id, &d)?;
    j(d)
}
async fn update_dir(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>, Json(mut d): Json<ModelDir>) -> R<J> {
    d.id = id.clone();
    app.store.put("model_dirs", &id, &d)?;
    j(d)
}
async fn delete_dir(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    app.store.delete("model_dirs", &id)?;
    ok()
}
fn model_summary(m: &Model) -> Value {
    let mut v = serde_json::to_value(m).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.remove("tensors");
        o.remove("meta");
        if let Some(i) = o.get_mut("info").and_then(|i| i.as_object_mut()) {
            i.remove("chat_template");
        }
    }
    v
}
async fn list_models(State(app): State<AppRef>, _a: Admin) -> R<J> {
    let v: Vec<Model> = app.store.list("models")?;
    j(v.iter().map(model_summary).collect::<Vec<_>>())
}
async fn get_model(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    let m = app.model(&id)?;
    let mut by_type: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut experts = 0u64;
    for t in &m.tensors {
        let kind = if t.n.contains("_exps") { "experts" } else if t.n.starts_with("blk.") { "layers" } else { "other" };
        let e = by_type.entry(kind.to_string()).or_default();
        e.0 += 1;
        e.1 += t.b;
        if t.n.contains("_exps") {
            experts += t.b;
        }
    }
    let mut v = serde_json::to_value(&m).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.remove("tensors");
        o.insert("tensor_summary".into(), json!({"groups": by_type, "expert_bytes": experts, "count": m.tensors.len()}));
    }
    j(v)
}
#[derive(Deserialize)]
struct ModelUpd {
    alias: Option<String>,
    tags: Option<Vec<String>>,
    favorite: Option<bool>,
    notes: Option<String>,
    mmproj_id: Option<Option<String>>,
}
async fn update_model(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>, Json(u): Json<ModelUpd>) -> R<J> {
    let mut m = app.model(&id)?;
    if let Some(a) = u.alias {
        crate::app::App::validate_alias_name(&a)?;
        let all: Vec<Model> = app.store.list("models")?;
        if all.iter().any(|x| x.id != id && x.alias == a) {
            return Err(ApiError::bad("такой алиас модели уже есть"));
        }
        m.alias = a;
    }
    if let Some(t) = u.tags {
        m.tags = t;
    }
    if let Some(f) = u.favorite {
        m.favorite = f;
    }
    if let Some(n) = u.notes {
        m.notes = n;
    }
    if let Some(mp) = u.mmproj_id {
        m.mmproj_id = mp;
    }
    app.store.put("models", &id, &m)?;
    j(model_summary(&m))
}
async fn delete_model(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>, Query(q): Query<DeleteQ>) -> R<J> {
    let m = app.model(&id)?;
    let used: Vec<Preset> = app.store.list::<Preset>("presets")?.into_iter().filter(|p| p.model_id == id).collect();
    if !used.is_empty() {
        return Err(ApiError::bad(format!("модель используется пресетами: {}", used.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", "))));
    }
    if q.delete_files {
        if app.pm.alive().iter().any(|i| i.info.read().model_id == id) {
            return Err(ApiError::bad("модель сейчас загружена"));
        }
        for p in &m.paths {
            std::fs::remove_file(p).map_err(|e| ApiError::bad(format!("{p}: {e}")))?;
        }
    }
    app.store.delete("models", &id)?;
    ok()
}
async fn scan_models(State(app): State<AppRef>, _a: Admin) -> R<J> {
    let st = app.store.clone();
    let rep = tokio::task::spawn_blocking(move || crate::models::scan_all(&st)).await.map_err(|e| ApiError::bad(e.to_string()))??;
    j(rep)
}
#[derive(Deserialize)]
struct PathReq {
    path: String,
}
async fn add_model_file(State(app): State<AppRef>, _a: Admin, Json(r): Json<PathReq>) -> R<J> {
    let st = app.store.clone();
    let m = tokio::task::spawn_blocking(move || crate::models::add_file(&st, std::path::Path::new(&r.path), None)).await.map_err(|e| ApiError::bad(e.to_string()))??;
    j(model_summary(&m))
}

// ---------------- presets
async fn list_presets(State(app): State<AppRef>, _a: Admin) -> R<J> {
    let mut v: Vec<Preset> = app.store.list("presets")?;
    v.sort_by_key(|p| (p.order, p.created_at));
    let aliases: Vec<Alias> = app.store.list("aliases")?;
    let out: Vec<Value> = v
        .iter()
        .map(|p| {
            let mut x = serde_json::to_value(p).unwrap();
            let al: Vec<&String> = aliases.iter().filter(|a| a.target_type == AliasTarget::Preset && a.target_id == p.id).map(|a| &a.name).collect();
            x["aliases"] = json!(al);
            x["instance"] = app.pm.get(&p.id).map(|i| serde_json::to_value(i.snapshot()).unwrap()).unwrap_or(Value::Null);
            x
        })
        .collect();
    j(out)
}
async fn create_preset(State(app): State<AppRef>, _a: Admin, Json(mut p): Json<Preset>) -> R<J> {
    p.id = String::new();
    p.version = 1;
    app.model(&p.model_id)?;
    app.backend(&p.backend_id)?;
    j(app.save_preset(p)?)
}
async fn get_preset(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    j(app.preset(&id)?)
}
async fn update_preset(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>, Json(mut p): Json<Preset>) -> R<J> {
    app.preset(&id)?;
    p.id = id;
    j(app.save_preset(p)?)
}
async fn delete_preset(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    let _ = app.pm.stop(&id).await;
    app.pm.remove(&id);
    app.store.delete("presets", &id)?;
    for a in app.store.list::<Alias>("aliases")? {
        if a.target_type == AliasTarget::Preset && a.target_id == id {
            app.store.delete("aliases", &a.name)?;
        }
    }
    for mut g in app.store.list::<AliasGroup>("alias_groups")? {
        if g.preset_ids.contains(&id) {
            g.preset_ids.retain(|x| x != &id);
            app.store.put("alias_groups", &g.id.clone(), &g)?;
        }
    }
    ok()
}
async fn clone_preset(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    let mut p = app.preset(&id)?;
    p.id = String::new();
    p.name = format!("{} (копия)", p.name);
    p.version = 1;
    p.autostart = false;
    p.hotkey = None;
    j(app.save_preset(p)?)
}
async fn preset_history(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    let mut v: Vec<Preset> = app.store.list::<Preset>("preset_history")?.into_iter().filter(|p| p.id == id).collect();
    v.sort_by_key(|p| std::cmp::Reverse(p.version));
    j(v)
}
async fn restore_preset(State(app): State<AppRef>, _a: Admin, Path((id, version)): Path<(String, u64)>) -> R<J> {
    let old: Preset = app.store.get("preset_history", &format!("{id}:{version}"))?.ok_or_else(|| ApiError::not_found("версия не найдена"))?;
    j(app.save_preset(old)?)
}
async fn preset_command(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    let p = app.preset(&id)?;
    command_json(&app, &p)
}
fn command_json(app: &AppRef, p: &Preset) -> R<J> {
    let spec = app.launch_spec(p, app.pm.get(&p.id).map(|i| i.info.read().port).unwrap_or(app.settings().internal_port_start))?;
    j(json!({"cmd": spec.preview("cmd"), "powershell": spec.preview("powershell"), "args": spec.args, "env": spec.env, "warnings": spec.warnings}))
}
async fn preview_command(State(app): State<AppRef>, _a: Admin, Json(p): Json<Preset>) -> R<J> {
    command_json(&app, &p)
}
#[derive(Deserialize)]
struct ImportReq {
    backend_id: String,
    command: String,
}
async fn import_command(State(app): State<AppRef>, _a: Admin, Json(r): Json<ImportReq>) -> R<J> {
    let b = app.backend(&r.backend_id)?;
    let schema: Arc<ParamSchema> = app.schema(&b).ok_or_else(|| ApiError::bad("сборка не просканирована"))?;
    let imp = args::import_command(&r.command, &schema);
    let models: Vec<Model> = app.store.list("models")?;
    let find = |p: &Option<String>| -> Option<String> {
        let p = p.as_ref()?;
        let norm = |s: &str| s.replace('\\', "/").to_lowercase();
        let file = norm(p).rsplit('/').next().unwrap_or("").to_string();
        models.iter().find(|m| m.paths.iter().any(|x| norm(x) == norm(p))).or_else(|| models.iter().find(|m| m.file_name.to_lowercase() == file)).map(|m| m.id.clone())
    };
    j(json!({
        "imported": imp,
        "model_id": find(&imp.model_path),
        "mmproj_id": find(&imp.mmproj_path),
        "draft_model_id": find(&imp.draft_path),
        "lora_ids": imp.lora_paths.iter().filter_map(|p| find(&Some(p.clone()))).collect::<Vec<_>>(),
    }))
}
async fn export_presets(State(app): State<AppRef>, _a: Admin) -> R<J> {
    let presets: Vec<Preset> = app.store.list("presets")?;
    let aliases: Vec<Alias> = app.store.list("aliases")?;
    let groups: Vec<AliasGroup> = app.store.list("alias_groups")?;
    let models: Vec<Model> = app.store.list("models")?;
    let backends: Vec<Backend> = app.store.list("backends")?;
    // include model file names / backend names so import can re-map ids on another PC
    let mref: BTreeMap<String, String> = models.iter().map(|m| (m.id.clone(), m.file_name.clone())).collect();
    let bref: BTreeMap<String, String> = backends.iter().map(|b| (b.id.clone(), b.name.clone())).collect();
    j(json!({"format": "llama-launcher-presets", "version": 1, "presets": presets, "aliases": aliases, "groups": groups, "model_files": mref, "backend_names": bref}))
}
async fn import_presets(State(app): State<AppRef>, _a: Admin, Json(v): Json<Value>) -> R<J> {
    if v["format"] != "llama-launcher-presets" {
        return Err(ApiError::bad("неизвестный формат файла"));
    }
    let presets: Vec<Preset> = serde_json::from_value(v["presets"].clone()).map_err(|e| ApiError::bad(e.to_string()))?;
    let models: Vec<Model> = app.store.list("models")?;
    let backends: Vec<Backend> = app.store.list("backends")?;
    let mut idmap: BTreeMap<String, String> = BTreeMap::new();
    let mut warnings = vec![];
    let mut n = 0;
    for mut p in presets {
        let old_id = p.id.clone();
        if app.store.get::<Preset>("presets", &p.id)?.is_some() {
            p.id = new_id();
        }
        if app.store.get::<Model>("models", &p.model_id)?.is_none() {
            let fname = v["model_files"][&p.model_id].as_str().unwrap_or_default();
            match models.iter().find(|m| m.file_name == fname) {
                Some(m) => p.model_id = m.id.clone(),
                None => warnings.push(format!("{}: модель {fname} не найдена", p.name)),
            }
        }
        if app.store.get::<Backend>("backends", &p.backend_id)?.is_none() {
            let bname = v["backend_names"][&p.backend_id].as_str().unwrap_or_default();
            match backends.iter().find(|b| b.name == bname).or(backends.first()) {
                Some(b) => p.backend_id = b.id.clone(),
                None => warnings.push(format!("{}: нет сборок", p.name)),
            }
        }
        app.store.put("presets", &p.id, &p)?;
        idmap.insert(old_id, p.id.clone());
        n += 1;
    }
    let groups: Vec<AliasGroup> = serde_json::from_value(v["groups"].clone()).unwrap_or_default();
    for mut g in groups {
        g.preset_ids = g.preset_ids.iter().filter_map(|x| idmap.get(x).cloned()).collect();
        app.store.put("alias_groups", &g.id.clone(), &g)?;
    }
    let aliases: Vec<Alias> = serde_json::from_value(v["aliases"].clone()).unwrap_or_default();
    for mut a in aliases {
        if app.store.get::<Alias>("aliases", &a.name)?.is_some() {
            warnings.push(format!("алиас {} уже есть — пропущен", a.name));
            continue;
        }
        if a.target_type == AliasTarget::Preset {
            match idmap.get(&a.target_id) {
                Some(x) => a.target_id = x.clone(),
                None => continue,
            }
        }
        app.store.put("aliases", &a.name.clone(), &a)?;
    }
    j(json!({"imported": n, "warnings": warnings}))
}
#[derive(Deserialize, Default)]
struct StartReq {
    #[serde(default)]
    force: bool,
}
async fn start_preset(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>, body: Option<Json<StartReq>>) -> R<J> {
    let force = body.map(|b| b.force).unwrap_or(false);
    j(app.start_preset(&id, force).await?)
}
async fn stop_preset(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    app.pm.stop(&id).await?;
    ok()
}
async fn calibrate(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    let p = app.preset(&id)?;
    let inst = app.pm.get(&id).ok_or_else(|| ApiError::bad("пресет не запущен"))?;
    let actual = inst.info.read().vram_bytes.ok_or_else(|| ApiError::bad("фактическое потребление VRAM недоступно (нужен NVML)"))?;
    let est = app.estimate_preset(&p, true)?;
    let (mut fixed, mut compute) = (0u64, 0u64);
    for d in &est.devices {
        fixed += d.usage.weights + d.usage.kv + d.usage.extra;
        compute += d.usage.compute + d.usage.overhead;
    }
    if compute == 0 {
        return Err(ApiError::bad("нечего калибровать"));
    }
    let old: Value = app.store.get("mem_calibration", &p.backend_id)?.unwrap_or(json!({"compute_coef": 1.0}));
    let old_coef = old["compute_coef"].as_f64().unwrap_or(1.0);
    let coef = ((actual as f64 - fixed as f64) / (compute as f64 / old_coef)).clamp(0.3, 4.0);
    let blended = old_coef * 0.5 + coef * 0.5;
    app.store.put("mem_calibration", &p.backend_id, &json!({"compute_coef": blended, "samples": old["samples"].as_u64().unwrap_or(0) + 1}))?;
    j(json!({"actual": actual, "estimated": fixed + compute, "compute_coef": blended}))
}

// ---------------- aliases
async fn list_aliases(State(app): State<AppRef>, _a: Admin) -> R<J> {
    j(app.store.list::<Alias>("aliases")?)
}
async fn create_alias(State(app): State<AppRef>, _a: Admin, Json(a): Json<Alias>) -> R<J> {
    crate::app::App::validate_alias_name(&a.name)?;
    match a.target_type {
        AliasTarget::Preset => {
            app.preset(&a.target_id)?;
        }
        AliasTarget::Group => {
            app.store.get::<AliasGroup>("alias_groups", &a.target_id)?.ok_or_else(|| ApiError::not_found("группа не найдена"))?;
        }
    }
    if let Some(existing) = app.store.get::<Alias>("aliases", &a.name)? {
        if existing.target_id != a.target_id {
            // re-pointing an alias is allowed (that's the point), just log it
            tracing::info!("alias {} re-pointed {} -> {}", a.name, existing.target_id, a.target_id);
        }
    }
    if a.is_primary {
        for mut o in app.store.list::<Alias>("aliases")? {
            if o.target_id == a.target_id && o.name != a.name && o.is_primary {
                o.is_primary = false;
                app.store.put("aliases", &o.name.clone(), &o)?;
            }
        }
    }
    app.store.put("aliases", &a.name, &a)?;
    j(a)
}
#[derive(Deserialize)]
struct NameQ {
    name: String,
}
async fn delete_alias(State(app): State<AppRef>, _a: Admin, Query(q): Query<NameQ>) -> R<J> {
    app.store.delete("aliases", &q.name)?;
    ok()
}
async fn list_groups(State(app): State<AppRef>, _a: Admin) -> R<J> {
    j(app.store.list::<AliasGroup>("alias_groups")?)
}
async fn create_group(State(app): State<AppRef>, _a: Admin, Json(mut g): Json<AliasGroup>) -> R<J> {
    g.id = new_id();
    app.store.put("alias_groups", &g.id, &g)?;
    j(g)
}
async fn update_group(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>, Json(mut g): Json<AliasGroup>) -> R<J> {
    g.id = id.clone();
    app.store.put("alias_groups", &id, &g)?;
    j(g)
}
async fn delete_group(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    for a in app.store.list::<Alias>("aliases")? {
        if a.target_type == AliasTarget::Group && a.target_id == id {
            app.store.delete("aliases", &a.name)?;
        }
    }
    app.store.delete("alias_groups", &id)?;
    ok()
}

// ---------------- memory
#[derive(Deserialize)]
struct MemReq {
    preset: Preset,
}
async fn mem_estimate(State(app): State<AppRef>, _a: Admin, Json(r): Json<MemReq>) -> R<J> {
    let p = r.preset;
    let running = !p.id.is_empty() && app.pm.get(&p.id).map(|i| i.state().is_alive()).unwrap_or(false);
    let est = app.estimate_preset(&p, running)?;
    let backend = app.backend(&p.backend_id)?;
    let model = app.model(&p.model_id)?;
    let schema = app.schema(&backend);
    let mut devices = app.device_slots(&backend, &p.devices);
    if running {
        if let Some(i) = app.pm.get(&p.id) {
            let e = i.info.read().mem_estimate.clone();
            for d in devices.iter_mut() {
                d.free_bytes += e.get(&d.id).copied().unwrap_or(0);
            }
        }
    }
    let mut inp = args::mem_input_from_preset(&p, schema.as_deref(), devices);
    app.fill_mem_defaults(&mut inp, &backend);
    inp.extra_gpu_bytes = est.devices.iter().map(|d| d.usage.extra).sum();
    let (info, t, size) = (&model.info, &model.tensors, model.size);
    let has_gpu = !inp.devices.is_empty();
    j(json!({
        "estimate": est,
        "autofit": {
            "ngl": if has_gpu { Some(memory::autofit_ngl(info, t, size, &inp)) } else { None },
            "ctx": memory::autofit_ctx(info, t, size, &inp),
            "n_cpu_moe": if has_gpu { memory::autofit_ncpumoe(info, t, size, &inp) } else { None },
            "tensor_split": if inp.devices.len() > 1 { Some(memory::autofit_split(&inp)) } else { None },
        },
        "model": { "n_layers": info.n_layers, "ctx_max": info.ctx_max, "n_expert": info.n_expert, "arch": info.arch },
        "running": running,
    }))
}

// ---------------- instances
async fn list_instances(State(app): State<AppRef>, _a: Admin) -> R<J> {
    j(app.pm.list().iter().map(|i| i.snapshot()).collect::<Vec<_>>())
}
#[derive(Deserialize)]
struct TailQ {
    tail: Option<usize>,
}
async fn instance_logs(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>, Query(q): Query<TailQ>) -> R<J> {
    j(app.instance_logs(&id, q.tail.unwrap_or(1000))?)
}
async fn stop_instance(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    app.pm.stop(&id).await?;
    ok()
}
async fn remove_instance(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    app.pm.remove(&id);
    ok()
}
#[derive(Deserialize)]
struct PinReq {
    pinned: bool,
}
async fn pin_instance(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>, Json(r): Json<PinReq>) -> R<J> {
    if let Some(i) = app.pm.get(&id) {
        i.info.write().pinned = r.pinned;
    }
    ok()
}
async fn stop_all(State(app): State<AppRef>, _a: Admin) -> R<J> {
    app.pm.stop_all().await;
    ok()
}

// ---------------- HF + downloads
#[derive(Deserialize)]
struct HfSearchQ {
    q: String,
    sort: Option<String>,
    author: Option<String>,
    limit: Option<u32>,
}
async fn hf_search(State(app): State<AppRef>, _a: Admin, Query(q): Query<HfSearchQ>) -> R<J> {
    j(crate::hf::search(&app, &q.q, q.sort.as_deref().unwrap_or("downloads"), q.limit.unwrap_or(40), q.author.as_deref()).await?)
}
async fn hf_files(State(app): State<AppRef>, _a: Admin, Query(q): Query<RepoQ>) -> R<J> {
    j(crate::hf::files(&app, &q.repo).await?)
}
async fn hf_readme(State(app): State<AppRef>, _a: Admin, Query(q): Query<RepoQ>) -> R<J> {
    j(json!({"readme": crate::hf::readme(&app, &q.repo).await?}))
}
async fn hf_estimate(State(app): State<AppRef>, _a: Admin, Json(r): Json<crate::hf::EstimateReq>) -> R<J> {
    j(crate::hf::estimate(&app, &r).await?)
}
#[derive(Deserialize)]
struct HfDlReq {
    repo: String,
    parts: Vec<crate::hf::HfPart>,
    subdir: Option<String>,
}
async fn hf_download(State(app): State<AppRef>, _a: Admin, Json(r): Json<HfDlReq>) -> R<J> {
    let base = std::path::PathBuf::from(app.settings().download_dir);
    let sub = r.subdir.unwrap_or_else(|| r.repo.replace('/', "_"));
    if sub.contains("..") {
        return Err(ApiError::bad("bad subdir"));
    }
    let dir = base.join(sub);
    let mut out = vec![];
    for p in r.parts {
        let fname = p.path.rsplit('/').next().unwrap_or(&p.path).to_string();
        if fname.contains("..") || fname.contains('\\') {
            return Err(ApiError::bad("bad file name"));
        }
        let dest = dir.join(&fname);
        if dest.exists() {
            continue;
        }
        out.push(downloads::enqueue(&app, crate::hf::resolve_url(&app, &r.repo, &p.path), Some(r.repo.clone()), fname, dest, p.size, p.sha256)?);
    }
    downloads::ensure_download_dir(&app)?;
    j(out)
}
async fn list_downloads(State(app): State<AppRef>, _a: Admin) -> R<J> {
    let mut v: Vec<Download> = app.store.list("downloads")?;
    v.sort_by_key(|d| std::cmp::Reverse(d.created_at));
    j(v)
}
async fn delete_download(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>, Query(q): Query<DeleteQ>) -> R<J> {
    downloads::remove(&app, &id, q.delete_partial)?;
    ok()
}
async fn pause_download(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    downloads::pause(&app, &id)?;
    ok()
}
async fn resume_download(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    downloads::resume(&app, &id)?;
    ok()
}

// ---------------- bench
#[derive(Deserialize)]
struct BenchQ {
    model_id: Option<String>,
}
async fn list_bench(State(app): State<AppRef>, _a: Admin, Query(q): Query<BenchQ>) -> R<J> {
    let mut v: Vec<BenchRun> = app.store.list("bench_runs")?;
    if let Some(m) = q.model_id {
        v.retain(|r| r.model_id == m);
    }
    v.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    for r in v.iter_mut() {
        r.raw = Value::Null;
    }
    j(v)
}
async fn start_bench(State(app): State<AppRef>, _a: Admin, Json(r): Json<crate::bench::BenchReq>) -> R<J> {
    let id = crate::bench::start(app.clone(), r)?;
    j(json!({"job_id": id}))
}
async fn delete_bench(State(app): State<AppRef>, _a: Admin, Path(id): Path<String>) -> R<J> {
    app.store.delete("bench_runs", &id)?;
    ok()
}

// ---------------- router info + chat proxy
#[derive(Deserialize)]
struct LimitQ {
    limit: Option<i64>,
}
async fn router_requests(State(app): State<AppRef>, _a: Admin, Query(q): Query<LimitQ>) -> R<J> {
    j(app.store.log_list("request_log", q.limit.unwrap_or(200).clamp(1, 5000))?)
}
async fn analytics(State(app): State<AppRef>, _a: Admin, Query(q): Query<crate::store::UsageQuery>) -> R<J> {
    j(app.store.usage_query(&q)?)
}
async fn analytics_clear(State(app): State<AppRef>, _a: Admin) -> R<J> {
    j(json!({"deleted": app.store.usage_clear()?}))
}
#[derive(Deserialize)]
struct RangeQ {
    from: i64,
    to: i64,
}
async fn analytics_export(State(app): State<AppRef>, _a: Admin, Query(q): Query<RangeQ>) -> R<Response> {
    let rows = app.store.usage_rows(q.from, q.to)?;
    let cols = ["ts", "preset", "model", "alias", "user", "ip", "path", "status", "prompt", "cached", "completion", "ms", "tps"];
    let mut out = cols.join(",") + "\n";
    for r in rows {
        let line: Vec<String> = cols
            .iter()
            .map(|c| match &r[*c] {
                Value::String(s) => format!("\"{}\"", s.replace('"', "\"\"")),
                Value::Null => String::new(),
                v => v.to_string(),
            })
            .collect();
        out += &line.join(",");
        out.push('\n');
    }
    Ok(([(header::CONTENT_TYPE, "text/csv; charset=utf-8"), (header::CONTENT_DISPOSITION, "attachment; filename=\"usage.csv\"")], out).into_response())
}
async fn router_models(State(app): State<AppRef>, Admin(p, _): Admin) -> R<J> {
    Ok(Json(crate::router::models_payload(&app, &p)))
}
async fn chat_proxy(State(app): State<AppRef>, Admin(p, ip): Admin, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let headers: HeaderMap = parts.headers.clone();
    let path = parts.uri.path_and_query().map(|x| x.as_str().to_string()).unwrap_or_default();
    let bytes = match axum::body::to_bytes(body, 512 << 20).await {
        Ok(b) => b,
        Err(e) => return ApiError::bad(e.to_string()).into_response(),
    };
    crate::router::proxy_inner(app, p, ip.to_string(), parts.method, crate::router::strip_api_prefix(&path), headers, bytes).await
}
