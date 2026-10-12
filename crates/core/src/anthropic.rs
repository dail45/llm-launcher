//! Anthropic Messages API (`/v1/messages`) on top of llama-server's OpenAI chat completions.
//! Lets Claude Code / Anthropic SDK clients use local models: text, images, tools, thinking, streaming.
use crate::app::AppRef;
use crate::http_util::resolve_principal;
use crate::router::{forward_raw, pick_instance, LogCtx};
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::StreamExt;
use serde_json::{json, Map, Value};

fn err(status: StatusCode, kind: &str, msg: &str) -> Response {
    (status, Json(json!({"type": "error", "error": {"type": kind, "message": msg}}))).into_response()
}

fn err_kind(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        413 => "request_too_large",
        429 => "rate_limit_error",
        503 | 529 => "overloaded_error",
        _ => "api_error",
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|b| match b.get("type").and_then(|t| t.as_str()) {
                Some("text") => b.get("text").and_then(|t| t.as_str()).map(String::from),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn image_part(b: &Value) -> Option<Value> {
    let src = b.get("source")?;
    let url = match src.get("type").and_then(|t| t.as_str()) {
        Some("base64") => format!("data:{};base64,{}", src.get("media_type").and_then(|m| m.as_str()).unwrap_or("image/png"), src.get("data").and_then(|d| d.as_str()).unwrap_or("")),
        Some("url") => src.get("url").and_then(|u| u.as_str())?.to_string(),
        _ => return None,
    };
    Some(json!({"type": "image_url", "image_url": {"url": url}}))
}

/// Convert an Anthropic Messages request into an OpenAI chat-completions request.
pub fn to_openai(req: &Value) -> Value {
    let mut msgs: Vec<Value> = vec![];
    if let Some(sys) = req.get("system") {
        let t = text_of(sys);
        if !t.is_empty() {
            msgs.push(json!({"role": "system", "content": t}));
        }
    }
    for m in req.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default() {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user").to_string();
        let content = m.get("content").cloned().unwrap_or(Value::Null);
        let blocks = match content {
            Value::String(s) => {
                msgs.push(json!({"role": role, "content": s}));
                continue;
            }
            Value::Array(a) => a,
            _ => continue,
        };
        let mut parts: Vec<Value> = vec![];
        let mut tool_calls: Vec<Value> = vec![];
        let mut tool_results: Vec<Value> = vec![];
        let mut has_image = false;
        for b in &blocks {
            match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "text" => parts.push(json!({"type": "text", "text": b.get("text").and_then(|t| t.as_str()).unwrap_or("")})),
                "image" => {
                    if let Some(p) = image_part(b) {
                        has_image = true;
                        parts.push(p);
                    }
                }
                "document" => {
                    if let Some(t) = b.pointer("/source/data").and_then(|d| d.as_str()).filter(|_| b.pointer("/source/type").and_then(|t| t.as_str()) == Some("text")) {
                        parts.push(json!({"type": "text", "text": t}));
                    }
                }
                "tool_use" => tool_calls.push(json!({
                    "id": b.get("id").cloned().unwrap_or(json!("")),
                    "type": "function",
                    "function": {"name": b.get("name").cloned().unwrap_or(json!("")), "arguments": serde_json::to_string(b.get("input").unwrap_or(&json!({}))).unwrap_or_default()},
                })),
                "tool_result" => {
                    let c = b.get("content").cloned().unwrap_or(Value::Null);
                    let mut t = text_of(&c);
                    if b.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false) {
                        t = format!("Error: {t}");
                    }
                    tool_results.push(json!({"role": "tool", "tool_call_id": b.get("tool_use_id").cloned().unwrap_or(json!("")), "content": t}));
                    // images returned by tools go to a follow-up user message
                    if let Value::Array(a) = &c {
                        for ib in a.iter().filter(|x| x.get("type").and_then(|t| t.as_str()) == Some("image")) {
                            if let Some(p) = image_part(ib) {
                                has_image = true;
                                parts.push(p);
                            }
                        }
                    }
                }
                _ => {} // thinking / redacted_thinking / server tool blocks are not replayed
            }
        }
        msgs.extend(tool_results);
        let simple = !has_image;
        let content: Value = if parts.is_empty() {
            Value::Null
        } else if simple {
            json!(parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"))
        } else {
            json!(parts)
        };
        if role == "assistant" {
            let mut o = json!({"role": "assistant", "content": if content.is_null() { json!("") } else { content }});
            if !tool_calls.is_empty() {
                o["tool_calls"] = json!(tool_calls);
            }
            msgs.push(o);
        } else if !content.is_null() {
            msgs.push(json!({"role": role, "content": content}));
        }
    }
    let mut out = Map::new();
    out.insert("model".into(), req.get("model").cloned().unwrap_or(json!("")));
    out.insert("messages".into(), json!(msgs));
    for (a, o) in [("max_tokens", "max_tokens"), ("temperature", "temperature"), ("top_p", "top_p"), ("top_k", "top_k"), ("stop_sequences", "stop")] {
        if let Some(v) = req.get(a).filter(|v| !v.is_null()) {
            out.insert(o.into(), v.clone());
        }
    }
    let stream = req.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    out.insert("stream".into(), json!(stream));
    if stream {
        out.insert("stream_options".into(), json!({"include_usage": true}));
    }
    if let Some(tools) = req.get("tools").and_then(|t| t.as_array()) {
        let t: Vec<Value> = tools
            .iter()
            .filter(|t| t.get("input_schema").is_some())
            .map(|t| json!({"type": "function", "function": {"name": t.get("name"), "description": t.get("description").cloned().unwrap_or(json!("")), "parameters": t.get("input_schema")}}))
            .collect();
        if !t.is_empty() {
            out.insert("tools".into(), json!(t));
        }
    }
    if let Some(tc) = req.get("tool_choice") {
        let v = match tc.get("type").and_then(|t| t.as_str()) {
            Some("any") => json!("required"),
            Some("none") => json!("none"),
            Some("tool") => json!({"type": "function", "function": {"name": tc.get("name")}}),
            _ => json!("auto"),
        };
        out.insert("tool_choice".into(), v);
        if tc.get("disable_parallel_tool_use").and_then(|d| d.as_bool()).unwrap_or(false) {
            out.insert("parallel_tool_calls".into(), json!(false));
        }
    }
    match req.pointer("/thinking/type").and_then(|t| t.as_str()) {
        Some("enabled") => {
            out.insert("chat_template_kwargs".into(), json!({"enable_thinking": true}));
        }
        Some("disabled") => {
            out.insert("chat_template_kwargs".into(), json!({"enable_thinking": false}));
        }
        _ => {}
    }
    Value::Object(out)
}

fn stop_reason(finish: Option<&str>) -> &'static str {
    match finish {
        Some("length") => "max_tokens",
        Some("tool_calls") | Some("function_call") => "tool_use",
        _ => "end_turn",
    }
}

fn usage_of(v: &Value) -> (u64, u64, u64) {
    let u = crate::router::extract_usage(&v.to_string());
    let prompt = u.prompt.unwrap_or(0);
    let cached = u.cached.unwrap_or(0).min(prompt);
    (prompt - cached, cached, u.completion.unwrap_or(0))
}

fn msg_id() -> String {
    format!("msg_{}", uuid::Uuid::new_v4().simple())
}

/// Convert a non-streaming OpenAI chat completion into an Anthropic message.
pub fn from_openai(v: &Value, model: &str) -> Value {
    let m = v.pointer("/choices/0/message").cloned().unwrap_or(json!({}));
    let mut content = vec![];
    if let Some(r) = m.get("reasoning_content").and_then(|r| r.as_str()).filter(|r| !r.is_empty()) {
        content.push(json!({"type": "thinking", "thinking": r, "signature": ""}));
    }
    if let Some(t) = m.get("content").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
        content.push(json!({"type": "text", "text": t}));
    }
    for tc in m.get("tool_calls").and_then(|t| t.as_array()).cloned().unwrap_or_default() {
        let args = tc.pointer("/function/arguments").and_then(|a| a.as_str()).unwrap_or("{}");
        content.push(json!({
            "type": "tool_use",
            "id": tc.get("id").and_then(|i| i.as_str()).filter(|i| !i.is_empty()).map(String::from).unwrap_or_else(|| format!("toolu_{}", uuid::Uuid::new_v4().simple())),
            "name": tc.pointer("/function/name").cloned().unwrap_or(json!("")),
            "input": serde_json::from_str::<Value>(args).unwrap_or(json!({})),
        }));
    }
    let (inp, cached, out) = usage_of(v);
    json!({
        "id": msg_id(), "type": "message", "role": "assistant", "model": model, "content": content,
        "stop_reason": stop_reason(v.pointer("/choices/0/finish_reason").and_then(|f| f.as_str())), "stop_sequence": null,
        "usage": {"input_tokens": inp, "output_tokens": out, "cache_read_input_tokens": cached, "cache_creation_input_tokens": 0},
    })
}

#[derive(PartialEq, Clone, Copy)]
enum Block {
    None,
    Thinking,
    Text,
    Tool,
}

/// Stateful OpenAI SSE → Anthropic SSE translator.
pub struct StreamConv {
    buf: String,
    model: String,
    started: bool,
    block: Block,
    index: i64,
    tool_idx: Option<i64>,
    finish: Option<String>,
    usage: (u64, u64, u64),
    done: bool,
}

fn ev(name: &str, data: Value) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}

impl StreamConv {
    pub fn new(model: &str) -> Self {
        Self { buf: String::new(), model: model.into(), started: false, block: Block::None, index: -1, tool_idx: None, finish: None, usage: (0, 0, 0), done: false }
    }
    fn start(&mut self, out: &mut String) {
        if !self.started {
            self.started = true;
            out.push_str(&ev("message_start", json!({"type": "message_start", "message": {"id": msg_id(), "type": "message", "role": "assistant", "model": self.model, "content": [], "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 0, "output_tokens": 0}}})));
            out.push_str(&ev("ping", json!({"type": "ping"})));
        }
    }
    fn close(&mut self, out: &mut String) {
        if self.block != Block::None {
            out.push_str(&ev("content_block_stop", json!({"type": "content_block_stop", "index": self.index})));
            self.block = Block::None;
        }
    }
    fn open(&mut self, out: &mut String, b: Block, cb: Value) {
        self.close(out);
        self.index += 1;
        self.block = b;
        out.push_str(&ev("content_block_start", json!({"type": "content_block_start", "index": self.index, "content_block": cb})));
    }
    fn delta(&self, out: &mut String, d: Value) {
        out.push_str(&ev("content_block_delta", json!({"type": "content_block_delta", "index": self.index, "delta": d})));
    }
    fn chunk(&mut self, v: &Value, out: &mut String) {
        self.start(out);
        if v.get("usage").map(|u| !u.is_null()).unwrap_or(false) || v.get("timings").is_some() {
            let u = usage_of(v);
            if u != (0, 0, 0) {
                self.usage = u;
            }
        }
        let Some(c) = v.pointer("/choices/0") else { return };
        let d = c.get("delta").cloned().unwrap_or(json!({}));
        if let Some(r) = d.get("reasoning_content").and_then(|r| r.as_str()).filter(|r| !r.is_empty()) {
            if self.block != Block::Thinking {
                self.open(out, Block::Thinking, json!({"type": "thinking", "thinking": "", "signature": ""}));
            }
            self.delta(out, json!({"type": "thinking_delta", "thinking": r}));
        }
        if let Some(t) = d.get("content").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
            if self.block != Block::Text {
                self.open(out, Block::Text, json!({"type": "text", "text": ""}));
            }
            self.delta(out, json!({"type": "text_delta", "text": t}));
        }
        for tc in d.get("tool_calls").and_then(|t| t.as_array()).cloned().unwrap_or_default() {
            let idx = tc.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
            if self.block != Block::Tool || self.tool_idx != Some(idx) {
                let id = tc.get("id").and_then(|i| i.as_str()).filter(|i| !i.is_empty()).map(String::from).unwrap_or_else(|| format!("toolu_{}", uuid::Uuid::new_v4().simple()));
                let name = tc.pointer("/function/name").and_then(|n| n.as_str()).unwrap_or("");
                self.open(out, Block::Tool, json!({"type": "tool_use", "id": id, "name": name, "input": {}}));
                self.tool_idx = Some(idx);
            }
            if let Some(a) = tc.pointer("/function/arguments").and_then(|a| a.as_str()).filter(|a| !a.is_empty()) {
                self.delta(out, json!({"type": "input_json_delta", "partial_json": a}));
            }
        }
        if let Some(f) = c.get("finish_reason").and_then(|f| f.as_str()) {
            self.finish = Some(f.to_string());
        }
    }
    /// Feed upstream bytes, get Anthropic SSE text to send.
    pub fn feed(&mut self, bytes: &[u8]) -> String {
        self.buf.push_str(&String::from_utf8_lossy(bytes));
        let mut out = String::new();
        while let Some(i) = self.buf.find('\n') {
            let line = self.buf[..i].trim_end_matches('\r').to_string();
            self.buf.drain(..=i);
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                out.push_str(&self.finish());
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                if let Some(e) = v.get("error") {
                    self.start(&mut out);
                    self.close(&mut out);
                    let msg = e.get("message").and_then(|m| m.as_str()).unwrap_or("upstream error");
                    out.push_str(&ev("error", json!({"type": "error", "error": {"type": "api_error", "message": msg}})));
                    self.done = true;
                    continue;
                }
                self.chunk(&v, &mut out);
            }
        }
        out
    }
    pub fn finish(&mut self) -> String {
        let mut out = String::new();
        if self.done {
            return out;
        }
        self.done = true;
        self.start(&mut out);
        self.close(&mut out);
        let (inp, cached, o) = self.usage;
        out.push_str(&ev("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": stop_reason(self.finish.as_deref()), "stop_sequence": null}, "usage": {"input_tokens": inp, "output_tokens": o, "cache_read_input_tokens": cached}})));
        out.push_str(&ev("message_stop", json!({"type": "message_stop"})));
        out
    }
}

async fn read_req(app: &AppRef, req: Request) -> Result<(crate::auth::Principal, String, HeaderMap, Value), Response> {
    let (parts, body) = req.into_parts();
    let (p, meta) = resolve_principal(app, &parts, true).map_err(|e| err(e.status, "authentication_error", &e.message))?;
    let bytes = axum::body::to_bytes(body, 512 * 1024 * 1024).await.map_err(|_| err(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large", "тело запроса слишком большое"))?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| err(StatusCode::BAD_REQUEST, "invalid_request_error", &format!("некорректный JSON: {e}")))?;
    Ok((p, meta.ip.to_string(), parts.headers, v))
}

pub async fn messages(State(app): State<AppRef>, req: Request) -> Response {
    let (p, ip, headers, v) = match read_req(&app, req).await {
        Ok(x) => x,
        Err(r) => return r,
    };
    let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let (alias, inst) = match pick_instance(&app, &p, Some(&model)).await {
        Ok(x) => x,
        Err((st, msg, _)) => return err(st, err_kind(st), &msg),
    };
    let oai = to_openai(&v);
    let stream = oai["stream"].as_bool().unwrap_or(false);
    let (user_id, user) = p.user();
    let ctx = LogCtx { alias, principal: p.label(), user_id, user, ip, path: "/v1/messages".into() };
    let mut h = HeaderMap::new();
    h.insert("content-type", HeaderValue::from_static("application/json"));
    let _ = headers;
    let up = match forward_raw(&app, inst, Method::POST, "/v1/chat/completions", &h, Bytes::from(oai.to_string()), ctx).await {
        Ok(u) => u,
        Err(e) => return err(StatusCode::BAD_GATEWAY, "api_error", &e),
    };
    if !up.status.is_success() {
        let body: Vec<u8> = up.body.filter_map(|c| async move { c.ok() }).collect::<Vec<_>>().await.concat();
        let msg = serde_json::from_slice::<Value>(&body).ok().and_then(|v| v.pointer("/error/message").and_then(|m| m.as_str()).map(String::from)).unwrap_or_else(|| String::from_utf8_lossy(&body).to_string());
        return err(up.status, err_kind(up.status), &msg);
    }
    if !stream {
        let body: Vec<u8> = up.body.filter_map(|c| async move { c.ok() }).collect::<Vec<_>>().await.concat();
        let v: Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => return err(StatusCode::BAD_GATEWAY, "api_error", &format!("некорректный ответ llama-server: {e}")),
        };
        return Json(from_openai(&v, &model)).into_response();
    }
    let conv = std::sync::Arc::new(parking_lot::Mutex::new(StreamConv::new(&model)));
    let c2 = conv.clone();
    let body = up
        .body
        .map(move |chunk| chunk.map(|b| Bytes::from(conv.lock().feed(&b))))
        .chain(futures::stream::once(async move { Ok::<_, std::io::Error>(Bytes::from(c2.lock().finish())) }));
    let mut r = Response::new(Body::from_stream(body));
    let hs = r.headers_mut();
    hs.insert("content-type", HeaderValue::from_static("text/event-stream"));
    hs.insert("cache-control", HeaderValue::from_static("no-cache"));
    hs.insert("x-accel-buffering", HeaderValue::from_static("no"));
    r
}

/// `/v1/messages/count_tokens`: apply the model's chat template and tokenize via llama-server.
pub async fn count_tokens(State(app): State<AppRef>, req: Request) -> Response {
    let (p, _ip, _h, v) = match read_req(&app, req).await {
        Ok(x) => x,
        Err(r) => return r,
    };
    let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let oai = to_openai(&v);
    let fallback = || Json(json!({"input_tokens": (oai.to_string().len() / 4) as u64})).into_response();
    let (_alias, inst) = match pick_instance(&app, &p, Some(&model)).await {
        Ok(x) => x,
        Err(_) => return fallback(),
    };
    let client = reqwest::Client::new();
    let base = inst.base_url();
    let tmpl = client.post(format!("{base}/apply-template")).json(&json!({"messages": oai["messages"], "tools": oai.get("tools")})).send().await;
    let Ok(Ok(t)) = (match tmpl {
        Ok(r) => Ok(r.json::<Value>().await),
        Err(e) => Err(e),
    }) else {
        return fallback();
    };
    let Some(prompt) = t.get("prompt").and_then(|p| p.as_str()) else { return fallback() };
    match client.post(format!("{base}/tokenize")).json(&json!({"content": prompt})).send().await {
        Ok(r) => match r.json::<Value>().await {
            Ok(tv) => Json(json!({"input_tokens": tv.get("tokens").and_then(|t| t.as_array()).map(|a| a.len()).unwrap_or(0)})).into_response(),
            Err(_) => fallback(),
        },
        Err(_) => fallback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_conversion() {
        let r = json!({
            "model": "coder", "max_tokens": 100, "system": [{"type": "text", "text": "be brief"}], "stream": true,
            "tools": [{"name": "get_weather", "description": "w", "input_schema": {"type": "object"}}, {"type": "web_search_20250305", "name": "web_search"}],
            "tool_choice": {"type": "any"},
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": [{"type": "thinking", "thinking": "hmm"}, {"type": "text", "text": "calling"}, {"type": "tool_use", "id": "t1", "name": "get_weather", "input": {"city": "Moscow"}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "text", "text": "+5"}]}, {"type": "text", "text": "and?"}]},
                {"role": "user", "content": [{"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "AAA"}}, {"type": "text", "text": "what"}]}
            ]
        });
        let o = to_openai(&r);
        let m = o["messages"].as_array().unwrap();
        assert_eq!(m[0], json!({"role": "system", "content": "be brief"}));
        assert_eq!(m[2]["tool_calls"][0]["function"]["arguments"], "{\"city\":\"Moscow\"}");
        assert_eq!(m[2]["content"], "calling");
        assert_eq!(m[3], json!({"role": "tool", "tool_call_id": "t1", "content": "+5"}));
        assert_eq!(m[4], json!({"role": "user", "content": "and?"}));
        assert_eq!(m[5]["content"][0]["image_url"]["url"], "data:image/jpeg;base64,AAA");
        assert_eq!(o["tools"].as_array().unwrap().len(), 1);
        assert_eq!(o["tool_choice"], "required");
        assert_eq!(o["stream_options"]["include_usage"], true);
    }
    #[test]
    fn stream_conversion() {
        let mut c = StreamConv::new("coder");
        let mut s = c.feed(b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\ndata: {\"choices\":[{\"del");
        s += &c.feed(b"ta\":{\"content\":\"lo\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"f\",\"arguments\":\"{\\\"a\\\":\"}}]}}]}\n\n");
        s += &c.feed(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"1}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":30,\"completion_tokens\":9,\"prompt_tokens_details\":{\"cached_tokens\":20}}}\n\ndata: [DONE]\n\n");
        s += &c.finish();
        let evs: Vec<&str> = s.lines().filter_map(|l| l.strip_prefix("event: ")).collect();
        assert_eq!(evs, ["message_start", "ping", "content_block_start", "content_block_delta", "content_block_stop", "content_block_start", "content_block_delta", "content_block_delta", "content_block_stop", "content_block_start", "content_block_delta", "content_block_delta", "content_block_stop", "message_delta", "message_stop"]);
        assert!(s.contains("\"thinking_delta\""));
        assert!(s.contains("\"partial_json\":\"1}\""));
        assert!(s.contains("\"stop_reason\":\"tool_use\""));
        assert!(s.contains("\"input_tokens\":10") && s.contains("\"cache_read_input_tokens\":20") && s.contains("\"output_tokens\":9"));
    }
    #[test]
    fn response_conversion() {
        let v = json!({"choices": [{"message": {"content": "ok", "reasoning_content": "r", "tool_calls": [{"id": "x", "function": {"name": "f", "arguments": "{\"q\":1}"}}]}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 5, "completion_tokens": 2}});
        let m = from_openai(&v, "coder");
        assert_eq!(m["content"][0]["type"], "thinking");
        assert_eq!(m["content"][1]["text"], "ok");
        assert_eq!(m["content"][2]["input"]["q"], 1);
        assert_eq!(m["stop_reason"], "tool_use");
        assert_eq!(m["usage"]["input_tokens"], 5);
    }
}
