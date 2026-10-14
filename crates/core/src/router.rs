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
        .route("/v1/messages", axum::routing::post(crate::anthropic::messages))
        .route("/v1/messages/count_tokens", axum::routing::post(crate::anthropic::count_tokens))
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
    forward(&app, inst, parts.method, &path, &parts.headers, bytes, LogCtx { alias: name, principal: p.label(), user_id: p.user().0, user: p.user().1, ip: meta.ip.to_string(), path: path.clone() }).await
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

    let (alias, inst) = match pick_instance(&app, &p, model_name.as_deref()).await {
        Ok(x) => x,
        Err((st, msg, code)) => return oai_error(st, &msg, code),
    };
    let (user_id, user) = p.user();
    forward(&app, inst, method, &path, &headers, bytes, LogCtx { alias, principal: p.label(), user_id, user, ip, path: path.clone() }).await
}

/// Resolve the model name (alias / preset) to a ready instance, starting it if needed.
pub async fn pick_instance(app: &AppRef, p: &Principal, model: Option<&str>) -> Result<(String, Arc<Instance>), (StatusCode, String, &'static str)> {
    let (alias, cands) = match model.filter(|s| !s.is_empty()) {
        Some(m) => match app.resolve(m) {
            Some(x) => x,
            None => return Err((StatusCode::NOT_FOUND, format!("модель «{m}» не найдена (нет такого алиаса)"), "model_not_found")),
        },
        None => {
            let alive = app.pm.alive();
            if alive.len() == 1 {
                let id = alive[0].info.read().id.clone();
                (alive[0].info.read().alias.clone().unwrap_or(id.clone()), vec![id])
            } else if let Some(x) = app.resolve("default") {
                x
            } else {
                return Err((StatusCode::BAD_REQUEST, "не указано поле model".into(), "model_required"));
            }
        }
    };
    if !p.allows_alias(&alias) {
        return Err((StatusCode::FORBIDDEN, "у ключа нет доступа к этой модели".into(), "forbidden"));
    }
    let Some(pid) = app.choose_candidate(&cands) else { return Err((StatusCode::NOT_FOUND, "у алиаса нет пресетов".into(), "model_not_found")) };
    match app.ensure_ready(&pid, p.can_load()).await {
        Ok(i) => Ok((alias, i)),
        Err(e) => Err((StatusCode::SERVICE_UNAVAILABLE, e.to_string(), "model_load_failed")),
    }
}

pub struct LogCtx {
    pub alias: String,
    pub principal: String,
    /// API key id ("" when the request had no key)
    pub user_id: String,
    pub user: String,
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
        // token usage from the final SSE chunk or the JSON body
        let u = extract_usage(&text);
        if let Some(o) = entry.as_object_mut() {
            if let Some(v) = u.prompt {
                o.insert("prompt_tokens".into(), json!(v));
            }
            if let Some(v) = u.cached {
                o.insert("cached_tokens".into(), json!(v));
            }
            if let Some(v) = u.completion {
                o.insert("completion_tokens".into(), json!(v));
            }
            if let Some(v) = u.tps {
                o.insert("tps".into(), json!(v));
            }
            if let Some(v) = u.pp_tps {
                o.insert("pp_tps".into(), json!(v));
            }
            o.insert("user".into(), json!(self.ctx.user));
        }
        if let Some(t) = u.tps {
            self.inst.info.write().last_tps = Some(t);
        }
        if let Some(n) = u.completion {
            self.inst.info.write().tokens_generated += n as u64;
        }
        let (preset_id, model_id) = {
            let i = self.inst.info.read();
            (i.id.clone(), i.model_id.clone())
        };
        let preset = self.app.store.get::<crate::types::Preset>("presets", &preset_id).ok().flatten().map(|p| p.name).unwrap_or_default();
        let model = self.app.store.get::<crate::types::Model>("models", &model_id).ok().flatten().map(|m| m.file_name).unwrap_or(model_id);
        let row = crate::store::UsageRow {
            preset_id,
            preset,
            model,
            alias: self.ctx.alias.clone(),
            user_id: self.ctx.user_id.clone(),
            user: self.ctx.user.clone(),
            ip: self.ctx.ip.clone(),
            path: self.ctx.path.split('?').next().unwrap_or("").to_string(),
            status: self.status,
            prompt: u.prompt.unwrap_or(0) as i64,
            cached: u.cached.unwrap_or(0) as i64,
            completion: u.completion.unwrap_or(0) as i64,
            ms: self.started.elapsed().as_millis() as i64,
            tps: u.tps,
        };
        let days = self.app.settings.read().usage_retention_days;
        if let Err(e) = self.app.store.usage_insert(&row, days) {
            tracing::warn!("usage insert: {e}");
        }
        let _ = self.app.store.log_append("request_log", &entry, 5000);
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Usage {
    /// all prompt tokens, including the ones served from the KV cache
    pub prompt: Option<u64>,
    pub cached: Option<u64>,
    pub completion: Option<u64>,
    pub tps: Option<f64>,
    pub pp_tps: Option<f64>,
}

/// Parse the JSON value that follows the last occurrence of `"key":` in `text`.
fn last_obj(text: &str, key: &str) -> Option<Value> {
    let pat = format!("\"{key}\"");
    let mut end = text.len();
    while let Some(i) = text[..end].rfind(&pat) {
        let rest = text[i + pat.len()..].trim_start();
        if let Some(rest) = rest.strip_prefix(':') {
            if let Some(Ok(v)) = serde_json::Deserializer::from_str(rest).into_iter::<Value>().next() {
                if v.is_object() {
                    return Some(v);
                }
            }
        }
        end = i;
    }
    None
}

/// Token usage from a llama-server response tail (OpenAI `usage` and/or llama.cpp `timings`).
/// llama.cpp: usage.prompt_tokens = all prompt tokens; timings.prompt_n = newly processed, cache_n = reused.
pub fn extract_usage(text: &str) -> Usage {
    let usage = last_obj(text, "usage");
    let timings = last_obj(text, "timings");
    let u = |k: &str| usage.as_ref().and_then(|v| v.get(k)).and_then(|v| v.as_u64());
    let t = |k: &str| timings.as_ref().and_then(|v| v.get(k)).and_then(|v| v.as_f64());
    let cached = usage
        .as_ref()
        .and_then(|v| v.pointer("/prompt_tokens_details/cached_tokens"))
        .and_then(|v| v.as_u64())
        .or_else(|| t("cache_n").map(|x| x as u64));
    let prompt = u("prompt_tokens").or_else(|| t("prompt_n").map(|p| p as u64 + cached.unwrap_or(0)));
    Usage {
        prompt,
        cached: cached.or(prompt.map(|_| 0)),
        completion: u("completion_tokens").or_else(|| t("predicted_n").map(|x| x as u64)),
        tps: t("predicted_per_second"),
        pp_tps: t("prompt_per_second"),
    }
}

fn hop_by_hop(h: &str) -> bool {
    matches!(h, "connection" | "keep-alive" | "proxy-authenticate" | "proxy-authorization" | "te" | "trailers" | "transfer-encoding" | "upgrade" | "host" | "content-length" | "authorization" | "x-api-key")
}

pub type ByteStream = futures::stream::BoxStream<'static, Result<Bytes, std::io::Error>>;

/// Upstream response whose body stream keeps the request accounting guard alive.
pub struct Upstream {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: ByteStream,
}

/// Apply instructions to one request only. Never modify stored chat history or
/// collapse the client's other instruction messages, tools or multimodal parts.
pub(crate) fn apply_system_prompt(req: &mut Value, prompt: &str, append: bool) -> bool {
    if prompt.trim().is_empty() {
        return false;
    }
    let Some(messages) = req.get_mut("messages").and_then(Value::as_array_mut) else { return false };
    let instruction = messages.iter().position(|m| matches!(m.get("role").and_then(Value::as_str), Some("system" | "developer")));
    if let Some(i) = instruction {
        if !append {
            return false;
        }
        let Some(content) = messages[i].get_mut("content") else { return false };
        match content {
            Value::String(text) => {
                *text = if text.is_empty() { prompt.to_string() } else { format!("{prompt}\n\n{text}") };
            }
            Value::Array(parts) => parts.insert(0, json!({"type": "text", "text": format!("{prompt}\n\n")})),
            Value::Null => *content = json!(prompt),
            _ => return false, // leave invalid requests to upstream validation
        }
    } else {
        messages.insert(0, json!({"role": "system", "content": prompt}));
    }
    true
}

/// Read the selected preset at request time, so edits work without restarting
/// llama-server and alias groups use the actual candidate's instructions.
pub(crate) fn apply_preset_system_prompt(app: &AppRef, inst: &Instance, req: &mut Value) -> Result<bool, String> {
    let id = inst.info.read().id.clone();
    let preset = app.store.get::<crate::types::Preset>("presets", &id).map_err(|e| e.to_string())?;
    Ok(preset.map(|p| apply_system_prompt(req, &p.system_prompt, p.system_prompt_append)).unwrap_or(false))
}

/// Send a request to llama-server; usage is parsed from the (upstream, OpenAI-format) body tail
/// when the returned stream is dropped.
pub async fn forward_raw(app: &AppRef, inst: Arc<Instance>, method: Method, path: &str, headers: &HeaderMap, body: Bytes, ctx: LogCtx) -> Result<Upstream, String> {
    // Centralized here: UI, OpenAI, native instance proxy and converted Anthropic
    // messages all pass through exactly once. Raw completions/embeddings are untouched.
    let body = if method == Method::POST && matches!(path.split('?').next(), Some("/v1/chat/completions" | "/chat/completions")) {
        if let Ok(mut req) = serde_json::from_slice::<Value>(&body) {
            if apply_preset_system_prompt(app, &inst, &mut req)? {
                Bytes::from(req.to_string())
            } else {
                body
            }
        } else {
            body
        }
    } else {
        body
    };
    let url = format!("{}{}", inst.base_url(), path);
    inst.active.fetch_add(1, Ordering::Relaxed);
    inst.touch();
    inst.info.write().requests_total += 1;
    let is_stream = serde_json::from_slice::<Value>(&body).ok().and_then(|v| v.get("stream").and_then(|s| s.as_bool())).unwrap_or(false);
    let mut guard = Guard { app: app.clone(), inst: inst.clone(), started: Instant::now(), status: 0, tail: Arc::new(Mutex::new(vec![])), ctx, stream: is_stream };

    let client = reqwest::Client::new();
    let mut rb = client.request(reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::POST), &url);
    for (k, v) in headers {
        // reqwest computes the correct length after request-body rewriting.
        if !hop_by_hop(k.as_str()) && k.as_str() != "content-length" && !k.as_str().starts_with("anthropic-") {
            rb = rb.header(k.as_str(), v.as_bytes());
        }
    }
    let resp = match rb.body(body).send().await {
        Ok(r) => r,
        Err(e) => {
            guard.status = 502;
            return Err(format!("ошибка связи с llama-server: {e}"));
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
        chunk.map_err(std::io::Error::other)
    });
    Ok(Upstream { status, headers: out_headers, body: stream.boxed() })
}

pub async fn forward(app: &AppRef, inst: Arc<Instance>, method: Method, path: &str, headers: &HeaderMap, body: Bytes, ctx: LogCtx) -> Response {
    match forward_raw(app, inst, method, path, headers, body, ctx).await {
        Ok(u) => {
            let mut r = Response::new(Body::from_stream(u.body));
            *r.status_mut() = u.status;
            *r.headers_mut() = u.headers;
            r
        }
        Err(e) => oai_error(StatusCode::BAD_GATEWAY, &e, "upstream_error"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preset_prompt_legacy_defaults_and_roundtrip() {
        let mut p: crate::types::Preset = serde_json::from_value(json!({"name": "old preset"})).unwrap();
        assert!(p.system_prompt.is_empty());
        assert!(!p.system_prompt_append);
        p.system_prompt = "Ты помощник.\nОтвечай кратко.".into();
        p.system_prompt_append = true;
        let loaded: crate::types::Preset = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(loaded.system_prompt, p.system_prompt);
        assert!(loaded.system_prompt_append);
    }
    #[test]
    fn preset_prompt_default_preserves_payload() {
        let mut req = json!({"messages": [{"role": "user", "content": [{"type": "image_url", "image_url": {"url": "data:image/png;base64,abc"}}]}], "stream": true, "tools": [{"type": "function"}], "temperature": 0.2});
        let orig = req.clone();
        assert!(apply_system_prompt(&mut req, "Preset\ninstructions", false));
        assert_eq!(req["messages"][0], json!({"role": "system", "content": "Preset\ninstructions"}));
        assert_eq!(req["messages"][1], orig["messages"][0]);
        assert_eq!(req["tools"], orig["tools"]);
        assert_eq!(req["stream"], orig["stream"]);
        assert_eq!(req["temperature"], orig["temperature"]);
        assert!(apply_system_prompt(&mut json!({"messages": []}), "Preset", false));
    }
    #[test]
    fn preset_prompt_client_instructions_win_even_when_empty() {
        for role in ["system", "developer"] {
            for content in [json!("Client"), json!(""), json!([{"type": "text", "text": "Client"}]), Value::Null] {
                let mut req = json!({"messages": [{"role": role, "content": content}, {"role": "user", "content": "Hi"}]});
                let orig = req.clone();
                assert!(!apply_system_prompt(&mut req, "Preset", false));
                assert_eq!(req, orig);
            }
        }
    }
    #[test]
    fn preset_prompt_append_merges_without_extra_instruction_messages() {
        for role in ["system", "developer"] {
            let mut req = json!({"messages": [{"role": role, "content": "Client"}, {"role": "system", "content": "More"}, {"role": "user", "content": "Hi"}]});
            assert!(apply_system_prompt(&mut req, "Preset", true));
            assert_eq!(req["messages"].as_array().unwrap().len(), 3);
            assert_eq!(req["messages"][0], json!({"role": role, "content": "Preset\n\nClient"}));
            assert_eq!(req["messages"][1]["content"], "More");
        }
        let mut empty = json!({"messages": [{"role": "system", "content": ""}]});
        assert!(apply_system_prompt(&mut empty, "Preset", true));
        assert_eq!(empty["messages"][0]["content"], "Preset");
    }
    #[test]
    fn preset_prompt_append_preserves_content_blocks() {
        let original = json!({"type": "text", "text": "Client", "cache_control": {"type": "ephemeral"}});
        let mut req = json!({"messages": [{"role": "system", "content": [original.clone()]}]});
        assert!(apply_system_prompt(&mut req, "Preset", true));
        assert_eq!(req["messages"][0]["content"], json!([{"type": "text", "text": "Preset\n\n"}, original]));
    }
    #[test]
    fn preset_prompt_noop_and_request_history_unchanged() {
        for mut req in [json!({}), json!({"prompt": "Raw completion"}), json!({"messages": null})] {
            let orig = req.clone();
            assert!(!apply_system_prompt(&mut req, "Preset", true));
            assert_eq!(req, orig);
        }
        let history = json!({"messages": [{"role": "user", "content": "Hi"}]});
        for prompt in ["", " \n\t"] {
            let mut req = history.clone();
            assert!(!apply_system_prompt(&mut req, prompt, true));
            assert_eq!(req, history);
        }
        for _ in 0..3 {
            let mut req = history.clone();
            apply_system_prompt(&mut req, "Preset", true);
            assert_eq!(req["messages"].as_array().unwrap().len(), 2);
        }
        assert_eq!(history["messages"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn preset_prompt_anthropic_conversion_and_overrides() {
        let mut req = crate::anthropic::to_openai(&json!({"messages": [{"role": "user", "content": "Hi"}]}));
        assert!(apply_system_prompt(&mut req, "Preset", false));
        for system in [json!(""), json!([]), json!([{"type": "text", "text": "Client"}])] {
            let mut req = crate::anthropic::to_openai(&json!({"system": system, "messages": [{"role": "user", "content": "Hi"}]}));
            assert!(!apply_system_prompt(&mut req, "Preset", false));
            assert!(apply_system_prompt(&mut req, "Preset", true));
            assert!(req["messages"][0]["content"].as_str().unwrap().starts_with("Preset"));
        }
    }
    #[test]
    fn usage_from_stream_timings() {
        let t = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\ndata: {\"choices\":[],\"timings\":{\"cache_n\":90,\"prompt_n\":10,\"predicted_n\":7,\"predicted_per_second\":42.5}}\n\ndata: [DONE]\n\n";
        let u = extract_usage(t);
        assert_eq!(u.prompt, Some(100));
        assert_eq!(u.cached, Some(90));
        assert_eq!(u.completion, Some(7));
        assert_eq!(u.tps, Some(42.5));
    }
    #[test]
    fn usage_prefers_openai_usage() {
        let t = r#"{"choices":[{"message":{"content":"usage: hi"}}],"usage":{"prompt_tokens":120,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":100}},"timings":{"prompt_n":20,"cache_n":100,"predicted_n":3}}"#;
        let u = extract_usage(t);
        assert_eq!((u.prompt, u.cached, u.completion), (Some(120), Some(100), Some(3)));
    }
    #[test]
    fn usage_truncated_body_and_missing() {
        let t = r#"ent":"..."}}],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#;
        let u = extract_usage(t);
        assert_eq!((u.prompt, u.cached, u.completion), (Some(5), Some(0), Some(2)));
        assert_eq!(extract_usage("garbage"), Usage::default());
    }
}
