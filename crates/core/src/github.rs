//! Installing llama.cpp builds (official and forks) from GitHub Releases.
use crate::app::App;
use crate::types::{now, Backend, BackendKind, GhSource};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub url: String,
    pub windows: bool,
    pub kind: BackendKind,
    pub is_cudart: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Release {
    pub tag: String,
    pub name: String,
    pub published_at: String,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

pub fn default_sources() -> Vec<GhSource> {
    vec![GhSource { id: "ggml-org".into(), repo: "ggml-org/llama.cpp".into(), name: "llama.cpp (official)".into(), asset_pattern: "win".into() }]
}

pub async fn releases(app: &App, repo: &str) -> Result<Vec<Release>> {
    if !repo.contains('/') || repo.contains("..") {
        bail!("репозиторий должен быть в формате owner/repo");
    }
    let mut req = app.http.get(format!("https://api.github.com/repos/{repo}/releases?per_page=15")).header("Accept", "application/vnd.github+json");
    let tok = app.settings().github_token;
    if !tok.is_empty() {
        req = req.header("Authorization", format!("Bearer {tok}"));
    }
    let v: serde_json::Value = req.send().await?.error_for_status()?.json().await?;
    let arr = v.as_array().context("unexpected GitHub response")?;
    Ok(arr
        .iter()
        .map(|r| Release {
            tag: r["tag_name"].as_str().unwrap_or_default().into(),
            name: r["name"].as_str().unwrap_or_default().into(),
            published_at: r["published_at"].as_str().unwrap_or_default().into(),
            prerelease: r["prerelease"].as_bool().unwrap_or(false),
            assets: r["assets"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| {
                            let name = x["name"].as_str()?.to_string();
                            if !name.to_lowercase().ends_with(".zip") {
                                return None;
                            }
                            let l = name.to_lowercase();
                            Some(Asset {
                                windows: l.contains("win"),
                                kind: BackendKind::guess(&l),
                                is_cudart: l.starts_with("cudart"),
                                size: x["size"].as_u64().unwrap_or(0),
                                url: x["browser_download_url"].as_str()?.to_string(),
                                name,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect())
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect()
}

pub fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    let f = std::fs::File::open(zip_path)?;
    let mut z = zip::ZipArchive::new(f)?;
    for i in 0..z.len() {
        let mut e = z.by_index(i)?;
        let Some(rel) = e.enclosed_name() else { continue };
        let out = dest.join(rel);
        if e.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut o = std::fs::File::create(&out)?;
        std::io::copy(&mut e, &mut o)?;
    }
    Ok(())
}

pub struct InstallReq {
    pub repo: String,
    pub tag: String,
    pub asset_name: String,
    pub asset_url: String,
    pub extra_urls: Vec<String>,
    pub name: Option<String>,
}

/// Install as a background job. Returns job id.
pub fn install(app: Arc<App>, req: InstallReq) -> String {
    let title = format!("Установка {} {}", req.repo, req.tag);
    let id = app.jobs.start(&app.bus, "backend_install", &title).id;
    let app2 = app.clone();
    let jid = id.clone();
    tokio::spawn(async move {
        let app = app2;
        let res: Result<Backend> = async {
            let base = PathBuf::from(app.settings().backends_dir);
            let stem = req.asset_name.trim_end_matches(".zip");
            let dir = base.join(sanitize(&req.repo.replace('/', "_"))).join(format!("{}-{}", sanitize(&req.tag), sanitize(stem)));
            std::fs::create_dir_all(&dir)?;
            let tmp = std::env::temp_dir().join(format!("lll-{}.zip", crate::types::new_id()));
            let mut urls = vec![req.asset_url.clone()];
            urls.extend(req.extra_urls.clone());
            for (k, u) in urls.iter().enumerate() {
                let total_n = urls.len();
                let jobs = &app.jobs;
                let _ = jobs;
                download_with_job(&app, &jid, u, &tmp, k, total_n).await?;
                let (t, d) = (tmp.clone(), dir.clone());
                tokio::task::spawn_blocking(move || extract_zip(&t, &d)).await??;
                let _ = std::fs::remove_file(&tmp);
            }
            let mut b = crate::backends::new_backend(req.name.as_deref().unwrap_or(&format!("{} {}", req.repo.split('/').next_back().unwrap_or(""), req.tag)), &dir.to_string_lossy());
            b.kind = BackendKind::guess(&req.asset_name);
            b.source_repo = Some(req.repo.clone());
            b.release_tag = Some(req.tag.clone());
            b.managed = true;
            b.created_at = now();
            app.store.put("backends", &b.id, &b)?;
            let b = app.rescan_backend(&b.id).await?;
            Ok(b)
        }
        .await;
        let jobs_ok = app.jobs.get(&jid).is_some();
        let _ = jobs_ok;
        match res {
            Ok(b) => {
                set_job(&app, &jid, |h| h.done(serde_json::json!({"backend_id": b.id}), format!("Установлено: {}", b.name)));
                app.bus.toast("success", "Сборка установлена", b.name);
            }
            Err(e) => {
                set_job(&app, &jid, |h| h.fail(e.to_string()));
                app.bus.toast("error", "Установка не удалась", e.to_string());
            }
        }
    });
    id
}

/// Re-attach to an existing job id to report progress.
fn set_job(app: &App, id: &str, f: impl FnOnce(&crate::jobs::JobHandle)) {
    let h = app.jobs.handle(&app.bus, id);
    f(&h);
}

async fn download_with_job(app: &App, jid: &str, url: &str, dest: &Path, k: usize, n: usize) -> Result<()> {
    crate::downloads::download_to(app, url, dest, |done, total| {
        let frac = if total > 0 { done as f64 / total as f64 } else { 0.0 };
        let p = (k as f64 + frac) / n as f64;
        set_job(app, jid, |h| h.progress(Some(p * 0.95), format!("Скачивание {}/{}: {:.1} МБ", k + 1, n, done as f64 / 1e6)));
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub backend_id: String,
    pub backend_name: String,
    pub repo: String,
    pub current: String,
    pub latest: String,
}

pub async fn check_updates(app: &App) -> Result<Vec<UpdateInfo>> {
    let backends: Vec<Backend> = app.store.list("backends")?;
    let mut out = vec![];
    let mut cache: std::collections::HashMap<String, String> = Default::default();
    for b in backends {
        let (Some(repo), Some(tag)) = (b.source_repo.clone(), b.release_tag.clone()) else { continue };
        let latest = match cache.get(&repo) {
            Some(t) => t.clone(),
            None => {
                let Ok(r) = releases(app, &repo).await else { continue };
                let Some(t) = r.iter().find(|r| !r.prerelease).or(r.first()).map(|r| r.tag.clone()) else { continue };
                cache.insert(repo.clone(), t.clone());
                t
            }
        };
        if latest != tag {
            out.push(UpdateInfo { backend_id: b.id, backend_name: b.name, repo, current: tag, latest });
        }
    }
    Ok(out)
}
