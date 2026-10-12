//! MCP (Model Context Protocol) client: connects to MCP servers (stdio processes or
//! Streamable HTTP) and exposes their tools to the built-in chat.
use crate::app::App;
use crate::types::{new_id, now};
use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

const PROTOCOL: &str = "2025-06-18";

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Stdio,
    Http,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct McpServerCfg {
    pub id: String,
    pub name: String,
    pub transport: Transport,
    /// stdio: executable (npx, uvx, node, python, path to exe …)
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: String,
    /// http: Streamable HTTP endpoint
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub enabled: bool,
    /// run tool calls without asking in the chat
    pub auto_approve: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub server_id: String,
    pub server: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub auto_approve: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct McpStatus {
    pub connected: bool,
    pub error: Option<String>,
    pub server_name: Option<String>,
    pub tools: usize,
    pub stderr: Vec<String>,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

enum Conn {
    Stdio { stdin: tokio::sync::Mutex<tokio::process::ChildStdin>, pending: Pending, _child: tokio::process::Child },
    Http { url: String, headers: BTreeMap<String, String>, session: Mutex<Option<String>>, http: reqwest::Client },
}

pub struct Client {
    conn: Conn,
    next: AtomicU64,
    pub tools: Mutex<Vec<McpTool>>,
    pub status: Mutex<McpStatus>,
    stderr: Arc<Mutex<Vec<String>>>,
}

impl Client {
    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let resp = match &self.conn {
            Conn::Stdio { stdin, pending, .. } => {
                let (tx, rx) = oneshot::channel();
                pending.lock().insert(id, tx);
                {
                    let mut w = stdin.lock().await;
                    w.write_all(format!("{msg}\n").as_bytes()).await.context("MCP: запись в stdin")?;
                    w.flush().await?;
                }
                match tokio::time::timeout(Duration::from_secs(120), rx).await {
                    Ok(Ok(v)) => v,
                    Ok(Err(_)) => bail!("MCP-сервер завершился"),
                    Err(_) => {
                        pending.lock().remove(&id);
                        bail!("MCP: таймаут ответа на {method}")
                    }
                }
            }
            Conn::Http { .. } => self.http_send(&msg, Some(id)).await?.context("MCP: пустой ответ")?,
        };
        if let Some(e) = resp.get("error") {
            bail!("MCP {method}: {}", e.get("message").and_then(|m| m.as_str()).unwrap_or(&e.to_string()));
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn notify(&self, method: &str) -> Result<()> {
        let msg = json!({"jsonrpc": "2.0", "method": method});
        match &self.conn {
            Conn::Stdio { stdin, .. } => {
                let mut w = stdin.lock().await;
                w.write_all(format!("{msg}\n").as_bytes()).await?;
                w.flush().await?;
            }
            Conn::Http { .. } => {
                self.http_send(&msg, None).await?;
            }
        }
        Ok(())
    }

    async fn http_send(&self, msg: &Value, id: Option<u64>) -> Result<Option<Value>> {
        let Conn::Http { url, headers, session, http } = &self.conn else { unreachable!() };
        let mut rb = http.post(url).header("accept", "application/json, text/event-stream").header("content-type", "application/json").header("mcp-protocol-version", PROTOCOL).json(msg);
        for (k, v) in headers {
            rb = rb.header(k, v);
        }
        if let Some(s) = session.lock().clone() {
            rb = rb.header("mcp-session-id", s);
        }
        let r = rb.timeout(Duration::from_secs(120)).send().await.context("MCP: HTTP-запрос")?;
        if let Some(s) = r.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            *session.lock() = Some(s.to_string());
        }
        let st = r.status();
        let ct = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        if id.is_none() {
            return Ok(None);
        }
        if !st.is_success() {
            bail!("MCP HTTP {}: {}", st.as_u16(), r.text().await.unwrap_or_default().chars().take(300).collect::<String>());
        }
        if ct.contains("text/event-stream") {
            // read SSE until the response with our id arrives
            use futures::StreamExt;
            let mut s = r.bytes_stream();
            let mut buf = String::new();
            while let Some(c) = s.next().await {
                buf.push_str(&String::from_utf8_lossy(&c?));
                while let Some(i) = buf.find('\n') {
                    let line = buf[..i].trim_end_matches('\r').to_string();
                    buf.drain(..=i);
                    if let Some(d) = line.strip_prefix("data:") {
                        if let Ok(v) = serde_json::from_str::<Value>(d.trim()) {
                            if v.get("id").and_then(|x| x.as_u64()) == id {
                                return Ok(Some(v));
                            }
                        }
                    }
                }
            }
            bail!("MCP: поток закрыт без ответа")
        }
        Ok(Some(r.json::<Value>().await.context("MCP: некорректный JSON")?))
    }
}

#[derive(Default)]
pub struct Mcp {
    clients: Mutex<HashMap<String, Arc<Client>>>,
    connecting: tokio::sync::Mutex<()>,
}

fn sanitize(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    s.trim_matches('_').to_string()
}

/// Function name exposed to the model: `<server>__<tool>` (max 64 chars).
pub fn fn_name(server: &str, tool: &str) -> String {
    let mut n = format!("{}__{}", sanitize(server), sanitize(tool));
    n.truncate(64);
    n
}

async fn connect(cfg: &McpServerCfg) -> Result<Arc<Client>> {
    let stderr = Arc::new(Mutex::new(vec![]));
    let conn = match cfg.transport {
        Transport::Stdio => {
            if cfg.command.trim().is_empty() {
                bail!("не указана команда");
            }
            let mut c = crate::proc_util::command(std::path::Path::new(resolve_cmd(&cfg.command).as_str()));
            c.args(&cfg.args).envs(&cfg.env).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
            if !cfg.cwd.trim().is_empty() {
                c.current_dir(&cfg.cwd);
            }
            let mut child = c.spawn().with_context(|| format!("не удалось запустить «{}»", cfg.command))?;
            let stdin = child.stdin.take().context("stdin")?;
            let stdout = child.stdout.take().context("stdout")?;
            let se = child.stderr.take().context("stderr")?;
            let pending: Pending = Default::default();
            let p2 = pending.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    let Ok(v) = serde_json::from_str::<Value>(&l) else { continue };
                    if let Some(id) = v.get("id").and_then(|x| x.as_u64()) {
                        if v.get("method").is_none() {
                            if let Some(tx) = p2.lock().remove(&id) {
                                let _ = tx.send(v);
                            }
                        }
                    }
                }
                p2.lock().clear(); // drop senders -> pending requests fail
            });
            let e2 = stderr.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(se).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    let mut e = e2.lock();
                    e.push(l);
                    let n = e.len();
                    if n > 50 {
                        e.drain(..n - 50);
                    }
                }
            });
            Conn::Stdio { stdin: tokio::sync::Mutex::new(stdin), pending, _child: child }
        }
        Transport::Http => {
            if cfg.url.trim().is_empty() {
                bail!("не указан URL");
            }
            Conn::Http { url: cfg.url.clone(), headers: cfg.headers.clone(), session: Mutex::new(None), http: reqwest::Client::new() }
        }
    };
    let cl = Arc::new(Client { conn, next: AtomicU64::new(1), tools: Mutex::new(vec![]), status: Mutex::new(McpStatus::default()), stderr });
    let init = cl
        .request("initialize", json!({"protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": {"name": "llama-launcher", "version": crate::VERSION}}))
        .await
        .map_err(|e| {
            let tail = cl.stderr.lock().iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join(" | ");
            if tail.is_empty() { e } else { anyhow!("{e}; stderr: {tail}") }
        })?;
    cl.notify("notifications/initialized").await?;
    let mut tools = vec![];
    let mut cursor: Option<String> = None;
    loop {
        let r = cl.request("tools/list", cursor.as_ref().map(|c| json!({"cursor": c})).unwrap_or(json!({}))).await?;
        for t in r.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default() {
            tools.push(McpTool {
                server_id: cfg.id.clone(),
                server: cfg.name.clone(),
                name: t.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                description: t.get("description").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                input_schema: t.get("inputSchema").cloned().unwrap_or(json!({"type": "object"})),
                auto_approve: cfg.auto_approve,
            });
        }
        cursor = r.get("nextCursor").and_then(|c| c.as_str()).map(String::from);
        if cursor.is_none() || tools.len() > 1000 {
            break;
        }
    }
    *cl.status.lock() = McpStatus { connected: true, error: None, server_name: init.pointer("/serverInfo/name").and_then(|n| n.as_str()).map(String::from), tools: tools.len(), stderr: vec![] };
    *cl.tools.lock() = tools;
    Ok(cl)
}

/// On Windows `npx`/`uvx` are .cmd shims; find them in PATH.
fn resolve_cmd(cmd: &str) -> String {
    #[cfg(windows)]
    {
        let p = std::path::Path::new(cmd);
        if p.extension().is_none() && !cmd.contains(['/', '\\']) {
            if let Some(path) = std::env::var_os("PATH") {
                for dir in std::env::split_paths(&path) {
                    for ext in ["exe", "cmd", "bat"] {
                        let c = dir.join(format!("{cmd}.{ext}"));
                        if c.is_file() {
                            return c.to_string_lossy().to_string();
                        }
                    }
                }
            }
        }
    }
    cmd.to_string()
}

impl Mcp {
    pub fn servers(app: &App) -> Vec<McpServerCfg> {
        let mut v: Vec<McpServerCfg> = app.store.list("mcp_servers").unwrap_or_default();
        v.sort_by_key(|s| s.created_at);
        v
    }
    pub fn status(&self, id: &str) -> McpStatus {
        self.clients.lock().get(id).map(|c| {
            let mut s = c.status.lock().clone();
            s.stderr = c.stderr.lock().iter().rev().take(20).rev().cloned().collect();
            s
        }).unwrap_or_default()
    }
    pub fn disconnect(&self, id: &str) {
        self.clients.lock().remove(id);
    }
    pub async fn reconnect(&self, app: &App, id: &str) -> Result<McpStatus> {
        self.disconnect(id);
        let cfg: McpServerCfg = app.store.get("mcp_servers", id)?.context("сервер не найден")?;
        match connect(&cfg).await {
            Ok(c) => {
                let s = c.status.lock().clone();
                self.clients.lock().insert(id.to_string(), c);
                Ok(s)
            }
            Err(e) => {
                let s = McpStatus { error: Some(e.to_string()), ..Default::default() };
                Ok(s)
            }
        }
    }
    /// Tools of all enabled servers (connects lazily).
    pub async fn tools(&self, app: &App) -> (Vec<McpTool>, Vec<(String, String)>) {
        let _g = self.connecting.lock().await;
        let mut errors = vec![];
        for cfg in Self::servers(app).into_iter().filter(|s| s.enabled) {
            if self.clients.lock().contains_key(&cfg.id) {
                continue;
            }
            match connect(&cfg).await {
                Ok(c) => {
                    self.clients.lock().insert(cfg.id.clone(), c);
                }
                Err(e) => errors.push((cfg.name.clone(), e.to_string())),
            }
        }
        let enabled: Vec<String> = Self::servers(app).into_iter().filter(|s| s.enabled).map(|s| s.id).collect();
        let cl = self.clients.lock();
        let tools = cl.iter().filter(|(id, _)| enabled.contains(id)).flat_map(|(_, c)| c.tools.lock().clone()).collect();
        (tools, errors)
    }
    pub async fn call(&self, app: &App, server_id: &str, name: &str, args: Value) -> Result<Value> {
        let c = self.clients.lock().get(server_id).cloned();
        let c = match c {
            Some(c) => c,
            None => {
                self.reconnect(app, server_id).await?;
                self.clients.lock().get(server_id).cloned().context("MCP-сервер не подключён")?
            }
        };
        let r = c.request("tools/call", json!({"name": name, "arguments": args})).await;
        if r.is_err() {
            // the process may have died: drop it so the next call reconnects
            self.disconnect(server_id);
        }
        r
    }
}

pub fn new_server(mut s: McpServerCfg) -> McpServerCfg {
    if s.id.is_empty() {
        s.id = new_id();
    }
    if s.created_at == 0 {
        s.created_at = now();
    }
    s
}

/// Convert an MCP tools/call result to text for the model (+ images as data URLs).
pub fn result_text(r: &Value) -> (String, Vec<String>) {
    let mut text = vec![];
    let mut images = vec![];
    for c in r.get("content").and_then(|c| c.as_array()).cloned().unwrap_or_default() {
        match c.get("type").and_then(|t| t.as_str()) {
            Some("text") => text.push(c.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string()),
            Some("image") => images.push(format!("data:{};base64,{}", c.get("mimeType").and_then(|m| m.as_str()).unwrap_or("image/png"), c.get("data").and_then(|d| d.as_str()).unwrap_or(""))),
            Some("resource") => text.push(c.pointer("/resource/text").and_then(|t| t.as_str()).unwrap_or("").to_string()),
            _ => {}
        }
    }
    if text.is_empty() {
        if let Some(s) = r.get("structuredContent") {
            text.push(s.to_string());
        }
    }
    let mut t = text.join("\n");
    if r.get("isError").and_then(|e| e.as_bool()).unwrap_or(false) {
        t = format!("Error: {t}");
    }
    (t, images)
}
