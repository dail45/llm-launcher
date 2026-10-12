//! Resumable download manager (models from Hugging Face, generic files).
use crate::app::App;
use crate::events::Event;
use crate::types::{new_id, now, ModelDir};
use anyhow::{bail, Context, Result};
use futures::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DlStatus {
    Queued,
    Running,
    Verifying,
    Paused,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub id: String,
    pub url: String,
    pub repo: Option<String>,
    pub file: String,
    pub dest: String,
    pub total: u64,
    pub done: u64,
    pub status: DlStatus,
    pub error: Option<String>,
    pub sha256: Option<String>,
    pub speed_bps: f64,
    pub created_at: i64,
    pub register_model: bool,
}

pub struct Downloads {
    tokens: Mutex<HashMap<String, CancellationToken>>,
    pub notify: tokio::sync::Notify,
}

impl Default for Downloads {
    fn default() -> Self {
        Self::new()
    }
}

impl Downloads {
    pub fn new() -> Self {
        Self { tokens: Mutex::new(HashMap::new()), notify: tokio::sync::Notify::new() }
    }
}

fn emit(app: &App, d: &Download) {
    app.bus.send(Event::Download { info: serde_json::to_value(d).unwrap_or_default() });
}

pub fn enqueue(app: &App, url: String, repo: Option<String>, file: String, dest: PathBuf, total: u64, sha256: Option<String>) -> Result<Download> {
    let all: Vec<Download> = app.store.list("downloads")?;
    let dest_s = dest.to_string_lossy().to_string();
    if let Some(d) = all.iter().find(|d| d.dest == dest_s && d.status != DlStatus::Done && d.status != DlStatus::Failed) {
        return Ok(d.clone());
    }
    let d = Download { id: new_id(), url, repo, file, dest: dest_s, total, done: 0, status: DlStatus::Queued, error: None, sha256, speed_bps: 0.0, created_at: now(), register_model: true };
    app.store.put("downloads", &d.id, &d)?;
    emit(app, &d);
    app.downloads.notify.notify_one();
    Ok(d)
}

pub fn pause(app: &App, id: &str) -> Result<()> {
    if let Some(t) = app.downloads.tokens.lock().remove(id) {
        t.cancel();
    }
    if let Some(mut d) = app.store.get::<Download>("downloads", id)? {
        if d.status == DlStatus::Queued || d.status == DlStatus::Running {
            d.status = DlStatus::Paused;
            app.store.put("downloads", id, &d)?;
            emit(app, &d);
        }
    }
    Ok(())
}

pub fn resume(app: &App, id: &str) -> Result<()> {
    let mut d: Download = app.store.get("downloads", id)?.context("not found")?;
    if matches!(d.status, DlStatus::Paused | DlStatus::Failed) {
        d.status = DlStatus::Queued;
        d.error = None;
        app.store.put("downloads", id, &d)?;
        emit(app, &d);
        app.downloads.notify.notify_one();
    }
    Ok(())
}

pub fn remove(app: &App, id: &str, delete_partial: bool) -> Result<()> {
    if let Some(t) = app.downloads.tokens.lock().remove(id) {
        t.cancel();
    }
    if let Some(d) = app.store.get::<Download>("downloads", id)? {
        if delete_partial && d.status != DlStatus::Done {
            let _ = std::fs::remove_file(format!("{}.part", d.dest));
        }
    }
    app.store.delete("downloads", id)?;
    Ok(())
}

pub fn spawn_worker(app: Arc<App>) {
    // mark interrupted downloads as queued again
    if let Ok(list) = app.store.list::<Download>("downloads") {
        for mut d in list.into_iter().filter(|d| d.status == DlStatus::Running || d.status == DlStatus::Verifying) {
            d.status = DlStatus::Queued;
            let _ = app.store.put("downloads", &d.id, &d);
        }
    }
    tokio::spawn(async move {
        loop {
            let max = app.settings().max_parallel_downloads.max(1);
            let list: Vec<Download> = app.store.list("downloads").unwrap_or_default();
            let running = app.downloads.tokens.lock().len();
            if running < max {
                let mut q: Vec<&Download> = list.iter().filter(|d| d.status == DlStatus::Queued && !app.downloads.tokens.lock().contains_key(&d.id)).collect();
                q.sort_by_key(|d| d.created_at);
                for d in q.into_iter().take(max - running) {
                    let tok = CancellationToken::new();
                    app.downloads.tokens.lock().insert(d.id.clone(), tok.clone());
                    let a = app.clone();
                    let d = d.clone();
                    tokio::spawn(async move {
                        let id = d.id.clone();
                        let res = run_one(&a, d, tok.clone()).await;
                        a.downloads.tokens.lock().remove(&id);
                        if let Err(e) = res {
                            if !tok.is_cancelled() {
                                if let Ok(Some(mut d)) = a.store.get::<Download>("downloads", &id) {
                                    d.status = DlStatus::Failed;
                                    d.error = Some(e.to_string());
                                    let _ = a.store.put("downloads", &id, &d);
                                    emit(&a, &d);
                                    a.bus.toast("error", format!("Загрузка {} не удалась", d.file), e.to_string());
                                }
                            }
                        }
                        a.downloads.notify.notify_one();
                    });
                }
            }
            tokio::select! {
                _ = app.downloads.notify.notified() => {},
                _ = tokio::time::sleep(Duration::from_secs(2)) => {},
            }
        }
    });
}

pub fn auth_header(app: &App, url: &str) -> Option<String> {
    let s = app.settings();
    let host_hf = url.contains("huggingface.co") || (!s.hf_endpoint.is_empty() && url.starts_with(&s.hf_endpoint));
    if host_hf && !s.hf_token.is_empty() {
        return Some(format!("Bearer {}", s.hf_token));
    }
    if url.contains("github.com") && !s.github_token.is_empty() {
        return Some(format!("Bearer {}", s.github_token));
    }
    None
}

async fn run_one(app: &App, mut d: Download, tok: CancellationToken) -> Result<()> {
    let dest = PathBuf::from(&d.dest);
    if let Some(p) = dest.parent() {
        tokio::fs::create_dir_all(p).await?;
    }
    let part = PathBuf::from(format!("{}.part", d.dest));
    let have = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
    d.status = DlStatus::Running;
    d.done = have;
    app.store.put("downloads", &d.id, &d)?;
    emit(app, &d);

    if d.total == 0 || have < d.total {
        let mut req = app.http.get(&d.url);
        if let Some(h) = auth_header(app, &d.url) {
            req = req.header("Authorization", h);
        }
        if have > 0 {
            req = req.header("Range", format!("bytes={have}-"));
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            bail!("HTTP {status}{}", if status.as_u16() == 401 || status.as_u16() == 403 { " — нужен HF-токен или принятие лицензии модели" } else { "" });
        }
        let resumed = status.as_u16() == 206;
        if !resumed && have > 0 {
            d.done = 0;
        }
        if let Some(len) = resp.content_length() {
            d.total = len + if resumed { have } else { 0 };
        }
        let mut f = tokio::fs::OpenOptions::new().create(true).write(true).append(resumed).truncate(!resumed).open(&part).await?;
        let mut stream = resp.bytes_stream();
        let mut last = Instant::now();
        let mut last_bytes = d.done;
        let started = Instant::now();
        let start_done = d.done;
        loop {
            let chunk = tokio::select! {
                _ = tok.cancelled() => { f.flush().await?; return Ok(()); }
                c = stream.next() => c,
            };
            let Some(chunk) = chunk else { break };
            let chunk = chunk?;
            f.write_all(&chunk).await?;
            d.done += chunk.len() as u64;
            let limit = app.settings().download_speed_limit_kbps;
            if limit > 0 {
                let expected = (d.done - start_done) as f64 / (limit as f64 * 1024.0);
                let el = started.elapsed().as_secs_f64();
                if expected > el {
                    tokio::time::sleep(Duration::from_secs_f64((expected - el).min(2.0))).await;
                }
            }
            if last.elapsed() > Duration::from_millis(700) {
                d.speed_bps = (d.done - last_bytes) as f64 / last.elapsed().as_secs_f64();
                last = Instant::now();
                last_bytes = d.done;
                emit(app, &d);
                if let Ok(Some(cur)) = app.store.get::<Download>("downloads", &d.id) {
                    if cur.status == DlStatus::Paused {
                        return Ok(());
                    }
                }
                app.store.put("downloads", &d.id, &d)?;
            }
        }
        f.flush().await?;
        drop(f);
        if d.total > 0 && d.done < d.total {
            bail!("соединение оборвалось ({} из {} байт) — можно продолжить", d.done, d.total);
        }
    }
    if let Some(expected) = d.sha256.clone().filter(|s| s.len() == 64) {
        d.status = DlStatus::Verifying;
        emit(app, &d);
        app.store.put("downloads", &d.id, &d)?;
        let p = part.clone();
        let got = tokio::task::spawn_blocking(move || sha256_file(&p)).await??;
        if !got.eq_ignore_ascii_case(&expected) {
            let _ = tokio::fs::remove_file(&part).await;
            bail!("контрольная сумма не совпала (файл удалён, перезапустите загрузку)");
        }
    }
    tokio::fs::rename(&part, &dest).await?;
    d.status = DlStatus::Done;
    d.speed_bps = 0.0;
    app.store.put("downloads", &d.id, &d)?;
    emit(app, &d);
    if d.register_model && d.dest.to_lowercase().ends_with(".gguf") {
        ensure_download_dir(app)?;
        // for split models register once all parts exist
        let st = app.store.clone();
        let destp = dest.clone();
        let repo = d.repo.clone();
        let res = tokio::task::spawn_blocking(move || {
            let name = destp.file_name().unwrap().to_string_lossy().to_string();
            if let Some((base, _, total)) = crate::models::split_base(&name) {
                let dir = destp.parent().unwrap();
                let all = (1..=total).all(|i| dir.join(format!("{base}-{i:05}-of-{total:05}.gguf")).exists());
                if !all {
                    return Ok(None);
                }
            }
            crate::models::add_file(&st, &destp, repo).map(Some)
        })
        .await?;
        match res {
            Ok(Some(m)) => app.bus.toast("success", "Модель загружена", m.alias),
            Ok(None) => {}
            Err(e) => app.bus.toast("error", "Не удалось прочитать модель", e.to_string()),
        }
    }
    Ok(())
}

pub fn ensure_download_dir(app: &App) -> Result<()> {
    let dir = app.settings().download_dir;
    let dirs: Vec<ModelDir> = app.store.list("model_dirs")?;
    if !dirs.iter().any(|d| Path::new(&d.path) == Path::new(&dir)) {
        let d = ModelDir { id: new_id(), path: dir, recursive: true, enabled: true };
        app.store.put("model_dirs", &d.id, &d)?;
    }
    Ok(())
}

pub fn sha256_file(p: &Path) -> Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 4 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// Simple one-shot download with progress callback (used for backend archives).
pub async fn download_to(app: &App, url: &str, dest: &Path, mut on_progress: impl FnMut(u64, u64)) -> Result<()> {
    let mut req = app.http.get(url);
    if url.starts_with("https://api.github.com/") {
        // release asset via the API (works for private repositories with a token)
        req = req.header("Accept", "application/octet-stream");
    }
    if let Some(h) = auth_header(app, url) {
        req = req.header("Authorization", h);
    }
    let resp = req.send().await?.error_for_status()?;
    let total = resp.content_length().unwrap_or(0);
    let mut f = tokio::fs::File::create(dest).await?;
    let mut s = resp.bytes_stream();
    let mut done = 0u64;
    let mut last = Instant::now();
    while let Some(c) = s.next().await {
        let c = c?;
        f.write_all(&c).await?;
        done += c.len() as u64;
        if last.elapsed() > Duration::from_millis(500) {
            on_progress(done, total);
            last = Instant::now();
        }
    }
    f.flush().await?;
    on_progress(done, total);
    Ok(())
}
