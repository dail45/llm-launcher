//! Hugging Face Hub: search GGUF repos, list quantizations, README, remote header estimates.
use crate::app::App;
use crate::gguf;
use crate::memory::{self, Fit};
use crate::models::{quant_from_name, split_base};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct RepoHit {
    pub id: String,
    pub author: String,
    pub downloads: u64,
    pub likes: u64,
    pub last_modified: String,
    pub tags: Vec<String>,
    pub pipeline_tag: Option<String>,
    pub gated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HfPart {
    pub path: String,
    pub size: u64,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HfFile {
    pub name: String,
    pub quant: Option<String>,
    pub size: u64,
    pub parts: Vec<HfPart>,
    pub is_mmproj: bool,
    pub fit: Fit,
    pub fit_note: String,
    pub local_model_id: Option<String>,
}

fn endpoint(app: &App) -> String {
    let e = app.settings().hf_endpoint;
    if e.is_empty() { "https://huggingface.co".into() } else { e.trim_end_matches('/').to_string() }
}

fn req(app: &App, url: &str) -> reqwest::RequestBuilder {
    let mut r = app.http.get(url);
    let t = app.settings().hf_token;
    if !t.is_empty() {
        r = r.header("Authorization", format!("Bearer {t}"));
    }
    r
}

fn check_repo(repo: &str) -> Result<()> {
    if repo.split('/').count() != 2 || repo.contains("..") || repo.contains(' ') {
        bail!("некорректный репозиторий");
    }
    Ok(())
}

pub async fn search(app: &App, q: &str, sort: &str, limit: u32, author: Option<&str>) -> Result<Vec<RepoHit>> {
    let sort = match sort {
        "likes" | "lastModified" | "trendingScore" | "downloads" | "createdAt" => sort,
        _ => "downloads",
    };
    let mut url = reqwest::Url::parse(&format!("{}/api/models", endpoint(app)))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("search", q).append_pair("filter", "gguf").append_pair("sort", sort).append_pair("direction", "-1").append_pair("limit", &limit.clamp(1, 100).to_string());
        if let Some(a) = author.filter(|a| !a.is_empty()) {
            qp.append_pair("author", a);
        }
    }
    let v: serde_json::Value = req(app, url.as_str()).send().await?.error_for_status()?.json().await?;
    Ok(v.as_array()
        .context("unexpected HF response")?
        .iter()
        .map(|m| {
            let id = m["id"].as_str().or(m["modelId"].as_str()).unwrap_or_default().to_string();
            RepoHit {
                author: id.split('/').next().unwrap_or_default().into(),
                downloads: m["downloads"].as_u64().unwrap_or(0),
                likes: m["likes"].as_u64().unwrap_or(0),
                last_modified: m["lastModified"].as_str().unwrap_or_default().into(),
                tags: m["tags"].as_array().map(|t| t.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
                pipeline_tag: m["pipeline_tag"].as_str().map(String::from),
                gated: m["gated"].as_bool().unwrap_or(false) || m["gated"].is_string(),
                id,
            }
        })
        .collect())
}

pub async fn files(app: &App, repo: &str) -> Result<Vec<HfFile>> {
    check_repo(repo)?;
    let url = format!("{}/api/models/{repo}/tree/main?recursive=true", endpoint(app));
    let v: serde_json::Value = req(app, &url).send().await?.error_for_status()?.json().await?;
    let mut singles = vec![];
    let mut splits: BTreeMap<String, Vec<(u32, HfPart)>> = BTreeMap::new();
    for e in v.as_array().context("unexpected HF response")? {
        if e["type"] != "file" {
            continue;
        }
        let path = e["path"].as_str().unwrap_or_default().to_string();
        if !path.to_lowercase().ends_with(".gguf") {
            continue;
        }
        let size = e["lfs"]["size"].as_u64().or(e["size"].as_u64()).unwrap_or(0);
        let sha = e["lfs"]["oid"].as_str().map(String::from);
        let part = HfPart { path: path.clone(), size, sha256: sha };
        let fname = path.rsplit('/').next().unwrap_or(&path).to_string();
        if let Some((base, idx, _)) = split_base(&fname) {
            let dir = path.rsplit_once('/').map(|x| x.0.to_string()).unwrap_or_default();
            splits.entry(format!("{dir}/{base}")).or_default().push((idx, part));
        } else {
            singles.push((fname, vec![part]));
        }
    }
    for (k, mut v) in splits {
        v.sort_by_key(|x| x.0);
        singles.push((k.rsplit('/').next().unwrap_or(&k).to_string(), v.into_iter().map(|x| x.1).collect()));
    }
    let snap = app.snapshot.read().clone();
    let vram: u64 = snap.gpus.iter().map(|g| g.free_bytes).sum();
    let ram = snap.ram_available;
    let local: Vec<crate::types::Model> = app.store.list("models").unwrap_or_default();
    let mut out: Vec<HfFile> = singles
        .into_iter()
        .map(|(name, parts)| {
            let size: u64 = parts.iter().map(|p| p.size).sum();
            let is_mmproj = name.to_lowercase().contains("mmproj");
            // rough: weights + ~10% for 8k ctx KV & buffers + 0.5 GiB overhead
            let need = (size as f64 * 1.1) as u64 + (512 << 20);
            let (fit, note) = if vram > 0 && need <= vram {
                (Fit::Good, "полностью в VRAM".to_string())
            } else if need <= vram + ram.saturating_sub(4 << 30) {
                (Fit::Tight, if vram > 0 { "частично на CPU (медленнее)".into() } else { "в RAM (CPU)".into() })
            } else {
                (Fit::No, "не хватит памяти".to_string())
            };
            let first = parts[0].path.rsplit('/').next().unwrap_or_default().to_string();
            let local_model_id = local.iter().find(|m| m.file_name == first && m.size == size).map(|m| m.id.clone());
            HfFile { quant: quant_from_name(&name), name, size, parts, is_mmproj, fit, fit_note: note, local_model_id }
        })
        .collect();
    out.sort_by_key(|f| (f.is_mmproj, f.size));
    Ok(out)
}

pub async fn readme(app: &App, repo: &str) -> Result<String> {
    check_repo(repo)?;
    let r = req(app, &format!("{}/{repo}/raw/main/README.md", endpoint(app))).send().await?;
    if !r.status().is_success() {
        return Ok(String::new());
    }
    Ok(r.text().await?)
}

pub fn resolve_url(app: &App, repo: &str, path: &str) -> String {
    let enc: Vec<String> = path.split('/').map(|s| urlencode(s)).collect();
    format!("{}/{repo}/resolve/main/{}", endpoint(app), enc.join("/"))
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Parse GGUF header of a remote file with HTTP Range (grows the window if needed).
pub async fn remote_header(app: &App, repo: &str, path: &str) -> Result<gguf::GgufFile> {
    let url = resolve_url(app, repo, path);
    for mb in [8u64, 48, 160] {
        let r = req(app, &url).header("Range", format!("bytes=0-{}", mb * 1024 * 1024 - 1)).send().await?;
        if !r.status().is_success() {
            bail!("HTTP {}", r.status());
        }
        let bytes = r.bytes().await?;
        match gguf::parse(&bytes[..], None) {
            Ok(g) => return Ok(g),
            Err(e) if e.to_string().contains("unexpected end") => continue,
            Err(e) => return Err(e),
        }
    }
    bail!("заголовок GGUF слишком большой")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EstimateReq {
    pub repo: String,
    pub parts: Vec<HfPart>,
    pub n_ctx: Option<u64>,
    pub backend_id: Option<String>,
    pub cache_type: Option<String>,
}

pub async fn estimate(app: &App, r: &EstimateReq) -> Result<serde_json::Value> {
    let first = r.parts.first().context("no parts")?;
    let g = remote_header(app, &r.repo, &first.path).await?;
    let info = g.info();
    let total: u64 = r.parts.iter().map(|p| p.size).sum();
    let tensors: Vec<memory::TensorLite> = if r.parts.len() == 1 { g.tensors.iter().map(|t| memory::TensorLite { n: t.name.clone(), b: t.bytes }).collect() } else { vec![] };
    let backend = match &r.backend_id {
        Some(id) => app.backend(id).ok(),
        None => app.default_backend(),
    };
    let devices = backend.as_ref().map(|b| app.device_slots(b, &[])).unwrap_or_default();
    let ct = r.cache_type.clone().unwrap_or_else(|| "f16".into());
    let mut inp = memory::MemInput { n_ctx: r.n_ctx.unwrap_or(8192), n_batch: 2048, n_ubatch: 512, cache_k: ct.clone(), cache_v: ct, flash_attn: true, parallel: 1, split_mode: "layer".into(), devices, mmap: true, compute_coef: 1.0, ..Default::default() };
    if let Some(b) = &backend {
        app.fill_mem_defaults(&mut inp, b);
    }
    let mut info2 = info.clone();
    if tensors.is_empty() {
        info2.tensor_bytes = total;
    }
    let est = memory::estimate(&info2, &tensors, total, &inp);
    let max_ngl = memory::autofit_ngl(&info2, &tensors, total, &inp);
    let max_ctx = memory::autofit_ctx(&info2, &tensors, total, &inp);
    let ncmoe = memory::autofit_ncpumoe(&info2, &tensors, total, &inp);
    Ok(serde_json::json!({ "info": info, "estimate": est, "max_ngl": max_ngl, "max_ctx": max_ctx, "n_cpu_moe": ncmoe }))
}
