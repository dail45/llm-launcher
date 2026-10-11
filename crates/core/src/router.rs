//! OpenAI-compatible router: one endpoint, model by alias, auto start, streaming proxy.
use crate::app::AppRef;
use crate::auth::Principal;
use crate::http_util::{resolve_principal, ApiError};
use crate::process::Instance;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router};
use futures::StreamExt;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

const MAX_BODY: usize = 512 * 1024 * 1024;

fn oai_error(status: StatusCode, msg: &str, code: &str) -> Response {
    (status, Json(json!({"error": {"message": msg, "type": "invalid_request_error", "code": code}}))).into_response()
}

pub fn router(app: AppRef) -> Router {
    Router::new()
        .route("/v1/models", get(list_models))
        .route("/health", get(|| async { Json(json!({"status": "ok"})) }))
        .route("/instance/{alias}/{*rest}", any(native_proxy))
        .route("/instance/{alias}/", any(native_proxy_root))
        .fallback(proxy)
        .with_state(app)
}

pub async fn list_models(State(app): State<AppRef>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let p = match resolve_principal(&app, &parts, true) {
        Ok((p, _)) => p,
        Err(e) => return oai_error(e.status, &e.message, "unauthorized"),
    };
    Json(models_payload(&app, &p)).into_response()
}

pub fn models_payload(app: &AppRef, p: &Principal) -> Value {
    let aliases: Vec<crate::types::Alias> = app.store.list("aliases").unwrap_or_default();
    let presets: Vec<crate::types::Preset> = app.store.list("presets").unwrap_or_default();
    let mut data = vec![];
    for a in &aliases {
        if !p.allows_alias(&a.name) {
            continue;
        }
        let (loaded, preset) = match a.target_type {
            crate::types::AliasTarget::Preset => (app.pm.get(&a.target_id).map(|i| i.state().is_alive()).unwrap_or(false), Some(a.target_id.clone())),
            _ => (false, None),
        };
        data.push(json!({"id": a.name, "object": "model", "created": 0, "owned_by": "llama-launcher", "meta": {"loaded": loaded, "preset_id": preset, "kind": a.target_type}}));
    }
    for pr in presets.iter().filter(|pr| !aliases.iter().any(|a| a.target_id == pr.id)) {
        if !p.allows_alias(&pr.name) {
            continue;
        }
        let loaded = app.pm.get(&pr.id).map(|i| i.state().is_alive()).unwrap_or(false);
        data.push(json!({"id": pr.name, "object": "model", "created": 0, "owned_by": "llama-launcher", "meta": {"loaded": loaded, "preset_id": pr.id, "kind": "preset"}}));
    }
    json!({"object": "list", "data": data})
}

async fn native_proxy_root(State(app): State<AppRef>, axum::extract::Path(alias): axum::extract::Path<String>, req: Request) -> Response {
    forward_native(app, alias, String::new(), req).await
}
async fn native_proxy(State(app): State<AppRef>, axum::extract::Path((alias, rest)): axum::extract::Path<(String, String)>, req: Request) -> Response {
    forward_native(app, alias, rest, req).await
}

async fn forward_native(app: AppRef, alias: String, rest: String, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let (p, meta) = match resolve_principal(&app, &parts, true) {
        Ok(x) => x,
        Err(e) => return e.into_response(),
    };
    let Some((name, cands)) = app.resolve(&alias) else { return ApiError::not_found(format!("алиас {alias} не найден")).into_response() };
    if !p.allows_alias(&name) {
        return ApiError::forbidden("нет доступа к этому алиасу").into_response();
    }
    let Some(pid) = app.choose_candidate(&cands) else { return ApiError::not_found("нет пресетов").into_response() };
    let inst = match app.ensure_ready(&pid, p.can_load()).await {
        Ok(i) => i,
        Err(e) => return ApiError::new(StatusCode::SERVICE_UNAVAILABLE, e.to_string()).into_response(),
    };
    let bytes = axum::body::to_bytes(body, MAX_BODY).await.unwrap_or_default();
    let q = parts.uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let path = format!("/{rest}{q}");
    forward(&app, inst, parts.method, &path, &parts.headers, bytes, LogCtx { alias: name, principal: p.label(), ip: meta.ip.to_string(), path: path.clone() }).await
}

/// Generic proxy for /v1/* and llama-server native endpoints. Model chosen from JSON body or ?model=.
pub async fn proxy(State(app): State<AppRef>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let path_q = parts.uri.path_and_query().map(|p| p.as_str().to_string()).unwrap_or_else(|| "/".into());
    let (p, meta) = match resolve_principal(&app, &parts, true) {
        Ok(x) => x,
        Err(e) => return oai_error(e.status, &e.message, "unauthorized"),
    };
    let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return oai_error(StatusCode::PAYLOAD_TOO_LARGE, "тело запроса слишком большое", "too_large"),
    };
    let path = strip_api_prefix(&path_q);
    proxy_inner(app, p, meta.ip.to_string(), parts.method, path, parts.headers, bytes).await
}

pub fn strip_api_prefix(p: &str) -> String {
    p.strip_prefix("/api").filter(|r| r.starts_with("/v1/")).map(String::from).unwrap_or_else(|| p.to_string())
}

pub async fn proxy_inner(app: AppRef, p: Principal, ip: String, method: Method, path: String, headers: HeaderMap, bytes: Bytes) -> Response {
    let model_name: Option<String> = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(String::from))
        .or_else(|| path.split_once('?').and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("model=").map(String::from))));

    let (alias, cands) = match model_name.as_deref().filter(|s| !s.is_empty()) {
        Some(m) => match app.resolve(m) {
            Some(x) => x,
            None => return oai_error(StatusCode::NOT_FOUND, &format!("модель «{m}» не найдена (нет такого алиаса)"), "model_not_found"),
        },
        None => {
            let alive = app.pm.alive();
            if alive.len() == 1 {
                let id = alive[0].info.read().id.clone();
                (alive[0].info.read().alias.clone().unwrap_or(id.clone()), vec![id])
            } else if let Some(x) = app.resolve("default") {
                x
            } else {
                return oai_error(StatusCode::BAD_REQUEST, "не указано поле model", "model_required");
            }
        }
    };
    if !p.allows_alias(&alias) {
        return oai_error(StatusCode::FORBIDDEN, "у ключа нет доступа к этой модели", "forbidden");
    }
    let Some(pid) = app.choose_candidate(&cands) else { return oai_error(StatusCode::NOT_FOUND, "у алиаса нет пресетов", "model_not_found") };
    let inst = match app.ensure_ready(&pid, p.can_load()).await {
        Ok(i) => i,
        Err(e) => return oai_error(StatusCode::SERVICE_UNAVAILABLE, &e.to_string(), "model_load_failed"),
    };
    forward(&app, inst, method, &path, &headers, bytes, LogCtx { alias, principal: p.label(), ip, path: path.clone() }).await
}

pub struct LogCtx {
    pub alias: String,
    pub principal: String,
    pub ip: String,
    pub path: String,
}

struct Guard {
    app: AppRef,
    inst: Arc<Instance>,
    started: Instant,
    status: u16,
    tail: Arc<Mutex<Vec<u8>>>,
    ctx: LogCtx,
    stream: bool,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.inst.active.fetch_sub(1, Ordering::Relaxed);
        self.inst.touch();
        let tail = self.tail.lock().clone();
        let text = String::from_utf8_lossy(&tail);
        let mut entry = json!({
            "alias": self.ctx.alias, "preset_id": self.inst.info.read().id, "principal": self.ctx.principal,
            "ip": self.ctx.ip, "path": self.ctx.path.split('?').next().unwrap_or(""), "status": self.status,
            "ms": self.started.elapsed().as_millis() as u64, "stream": self.stream,
        });
        // timings/usage from the final chunk or body
        if let Some(i) = text.rfind("\"timings\"").or_else(|| text.rfind("\"usage\"")) {
            let seg = &text[i..];
            let num = |k: &str| -> Option<f64> {
                let p = seg.find(&format!("\"{k}\""))?;
                let rest = &seg[p + k.len() + 2..];
                let rest = rest.trim_start_matches([':', ' ']);
                let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e')).unwrap_or(rest.len());
                rest[..end].parse().ok()
            };
            if let Some(o) = entry.as_object_mut() {
                for (k, alias) in [("prompt_n", "prompt_tokens"), ("predicted_n", "completion_tokens"), ("predicted_per_second", "tps"), ("prompt_per_second", "pp_tps"), ("prompt_tokens", "prompt_tokens"), ("completion_tokens", "completion_tokens")] {
                    if let Some(v) = num(k) {
                        o.entry(alias).or_insert(json!(v));
                    }
                }
            }
            if let Some(t) = entry.get("tps").and_then(|v| v.as_f64()) {
                self.inst.info.write().last_tps = Some(t);
            }
            if let Some(n) = entry.get("completion_tokens").and_then(|v| v.as_f64()) {
                self.inst.info.write().tokens_generated += n as u64;
            }
        }
        let _ = self.app.store.log_append("request_log", &entry, 5000);
    }
}

fn hop_by_hop(h: &str) -> bool {
    matches!(h, "connection" | "keep-alive" | "proxy-authenticate" | "proxy-authorization" | "te" | "trailers" | "transfer-encoding" | "upgrade" | "host" | "content-length" | "authorization" | "x-api-key")
}

pub async fn forward(app: &AppRef, inst: Arc<Instance>, method: Method, path: &str, headers: &HeaderMap, body: Bytes, ctx: LogCtx) -> Response {
    let url = format!("{}{}", inst.base_url(), path);
    inst.active.fetch_add(1, Ordering::Relaxed);
    inst.touch();
    inst.info.write().requests_total += 1;
    let is_stream = serde_json::from_slice::<Value>(&body).ok().and_then(|v| v.get("stream").and_then(|s| s.as_bool())).unwrap_or(false);
    let mut guard = Guard { app: app.clone(), inst: inst.clone(), started: Instant::now(), status: 0, tail: Arc::new(Mutex::new(vec![])), ctx, stream: is_stream };

    let client = reqwest::Client::new();
    let mut rb = client.request(reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::POST), &url);
    for (k, v) in headers {
        if !hop_by_hop(k.as_str()) {
            rb = rb.header(k.as_str(), v.as_bytes());
        }
    }
    let resp = match rb.body(body).send().await {
        Ok(r) => r,
        Err(e) => {
            guard.status = 502;
            return oai_error(StatusCode::BAD_GATEWAY, &format!("ошибка связи с llama-server: {e}"), "upstream_error");
        }
    };
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    guard.status = status.as_u16();
    let mut out_headers = HeaderMap::new();
    for (k, v) in resp.headers() {
        if hop_by_hop(k.as_str()) {
            continue;
        }
        if let (Ok(n), Ok(val)) = (HeaderName::from_bytes(k.as_str().as_bytes()), HeaderValue::from_bytes(v.as_bytes())) {
            out_headers.insert(n, val);
        }
    }
    out_headers.insert("x-llama-launcher-alias", HeaderValue::from_str(&guard.ctx.alias).unwrap_or(HeaderValue::from_static("")));
    if is_stream {
        // keep reverse proxies (nginx/Caddy) from buffering token streams
        out_headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
        out_headers.insert("cache-control", HeaderValue::from_static("no-cache"));
    }
    let tail = guard.tail.clone();
    let stream = resp.bytes_stream().map(move |chunk| {
        let _g = &guard;
        if let Ok(c) = &chunk {
            let mut t = tail.lock();
            t.extend_from_slice(c);
            let len = t.len();
            if len > 8192 {
                t.drain(..len - 8192);
            }
        }
        chunk
    });
    let mut r = Response::new(Body::from_stream(stream));
    *r.status_mut() = status;
    *r.headers_mut() = out_headers;
    r
}
