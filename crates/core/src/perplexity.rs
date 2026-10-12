//! Quality comparison of quants with llama-perplexity: perplexity (PPL) and, against a
//! reference model, KL-divergence / "same top token" — how much a quant distorts the model.
use crate::app::App;
use crate::types::{new_id, now};
use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};

pub const WIKITEXT_URL: &str = "https://huggingface.co/datasets/ggml-org/ci/resolve/main/wikitext-2-raw-v1.zip";

#[derive(Debug, Clone, Deserialize)]
pub struct PplReq {
    pub model_ids: Vec<String>,
    pub backend_id: String,
    /// reference for KL-divergence (usually the biggest quant / BF16); None = PPL only
    pub base_model_id: Option<String>,
    /// "wikitext2" or a path to a UTF-8 text file
    pub dataset: Option<String>,
    pub ctx: Option<u32>,
    /// 0 = whole dataset
    pub chunks: Option<u32>,
    pub ngl: Option<i64>,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PplRun {
    pub id: String,
    pub group: String,
    pub model_id: String,
    pub model: String,
    pub quant: Option<String>,
    pub size: u64,
    pub backend_id: String,
    pub dataset: String,
    pub ctx: u32,
    pub chunks: u32,
    pub is_base: bool,
    pub ppl: Option<f64>,
    pub ppl_err: Option<f64>,
    pub kld: Option<f64>,
    pub kld_err: Option<f64>,
    pub kld_p99: Option<f64>,
    pub same_top: Option<f64>,
    pub rms_dp: Option<f64>,
    pub secs: f64,
    pub error: Option<String>,
    pub created_at: i64,
}

fn perplexity_bin(b: &crate::types::Backend) -> Option<PathBuf> {
    let dir = Path::new(&b.server_bin).parent()?.to_path_buf();
    let p = dir.join(crate::paths::exe_name("llama-perplexity"));
    p.is_file().then_some(p)
}

async fn dataset_path(app: &App, ds: &str, job: &crate::jobs::JobHandle<'_>) -> Result<(PathBuf, String)> {
    if ds != "wikitext2" {
        let p = PathBuf::from(ds);
        if !p.is_file() {
            bail!("файл с текстом не найден: {ds}");
        }
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        return Ok((p, name));
    }
    let dir = crate::paths::data_dir().join("datasets");
    let txt = dir.join("wiki.test.raw");
    if txt.is_file() {
        return Ok((txt, "wikitext-2".into()));
    }
    std::fs::create_dir_all(&dir)?;
    job.progress(None, "скачиваю wikitext-2 (~4 МБ)…");
    let zip_path = dir.join("wikitext-2-raw-v1.zip");
    crate::downloads::download_to(app, WIKITEXT_URL, &zip_path, |_, _| {}).await.context("не удалось скачать wikitext-2")?;
    let zp = zip_path.clone();
    let out = txt.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let f = std::fs::File::open(&zp)?;
        let mut z = zip::ZipArchive::new(f)?;
        for i in 0..z.len() {
            let mut e = z.by_index(i)?;
            if e.name().ends_with("wiki.test.raw") {
                let mut o = std::fs::File::create(&out)?;
                std::io::copy(&mut e, &mut o)?;
                return Ok(());
            }
        }
        bail!("в архиве нет wiki.test.raw")
    })
    .await??;
    let _ = std::fs::remove_file(&zip_path);
    Ok((txt, "wikitext-2".into()))
}

/// Parsed numbers from llama-perplexity output.
#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub ppl: Option<f64>,
    pub ppl_err: Option<f64>,
    pub kld: Option<f64>,
    pub kld_err: Option<f64>,
    pub kld_p99: Option<f64>,
    pub same_top: Option<f64>,
    pub rms_dp: Option<f64>,
    pub total_chunks: Option<u32>,
    pub done_chunks: Option<u32>,
}

pub fn parse(text: &str) -> Parsed {
    let num = r"([0-9]+(?:\.[0-9]+)?(?:[eE][-+]?[0-9]+)?)";
    let pm = r"\s*(?:±|\+/-)\s*";
    let cap = |re: &str| -> Option<(f64, Option<f64>)> {
        let r = Regex::new(re).ok()?;
        let c = r.captures_iter(text).last()?;
        Some((c.get(1)?.as_str().parse().ok()?, c.get(2).and_then(|m| m.as_str().parse().ok())))
    };
    let mut p = Parsed::default();
    if let Some((v, e)) = cap(&format!(r"Final estimate: PPL = {num}(?:{pm}{num})?")).or_else(|| cap(&format!(r"Mean PPL\(Q\)\s*:\s*{num}(?:{pm}{num})?"))) {
        p.ppl = Some(v);
        p.ppl_err = e;
    }
    if let Some((v, e)) = cap(&format!(r"Mean\s+KLD:\s*{num}(?:{pm}{num})?")) {
        p.kld = Some(v);
        p.kld_err = e;
    }
    p.kld_p99 = cap(&format!(r"99\.0%\s+KLD:\s*{num}")).map(|x| x.0);
    p.same_top = cap(&format!(r"Same top p:\s*{num}")).map(|x| x.0);
    p.rms_dp = cap(&format!(r"RMS Δp\s*:\s*{num}")).map(|x| x.0);
    p.total_chunks = cap(r"over ([0-9]+) chunks").map(|x| x.0 as u32);
    p.done_chunks = Regex::new(r"\[([0-9]+)\]").ok().and_then(|r| r.captures_iter(text).last().and_then(|c| c[1].parse().ok()));
    p
}

pub fn start(app: Arc<App>, req: PplReq) -> Result<String> {
    if req.model_ids.is_empty() {
        bail!("выберите хотя бы одну модель");
    }
    let backend = app.backend(&req.backend_id)?;
    let bin = perplexity_bin(&backend).context("в этой сборке нет llama-perplexity (он входит в стандартные релизы llama.cpp)")?;
    let mut order: Vec<String> = vec![];
    if let Some(b) = &req.base_model_id {
        order.push(b.clone());
    }
    order.extend(req.model_ids.iter().filter(|m| Some(*m) != req.base_model_id.as_ref()).cloned());
    let models = order.iter().map(|id| app.model(id)).collect::<Result<Vec<_>>>()?;
    let id = app.jobs.start(&app.bus, "perplexity", &format!("Качество квантов: {} шт.", models.len())).id;
    let jid = id.clone();
    tokio::spawn(async move {
        let job = app.jobs.handle(&app.bus, &jid);
        let ds = req.dataset.clone().unwrap_or_else(|| "wikitext2".into());
        let (data, ds_name) = match dataset_path(&app, &ds, &job).await {
            Ok(x) => x,
            Err(e) => return job.fail(e.to_string()),
        };
        let group = new_id();
        let ctx = req.ctx.unwrap_or(512);
        let chunks = req.chunks.unwrap_or(100);
        let kld_dir = crate::paths::data_dir().join("datasets");
        let _ = std::fs::create_dir_all(&kld_dir);
        let kld_file = kld_dir.join(format!("kld-{group}.bin"));
        let total = models.len();
        let mut results = vec![];
        for (k, m) in models.iter().enumerate() {
            let is_base = req.base_model_id.as_deref() == Some(m.id.as_str());
            let use_kld = req.base_model_id.is_some();
            let mut args: Vec<String> = vec!["-m".into(), m.paths[0].clone(), "-c".into(), ctx.to_string(), "-ngl".into(), req.ngl.unwrap_or(99).to_string()];
            if !use_kld || is_base {
                args.extend(["-f".into(), data.to_string_lossy().to_string()]);
            }
            if chunks > 0 {
                args.extend(["--chunks".into(), chunks.to_string()]);
            }
            if use_kld {
                args.extend(["--kl-divergence-base".into(), kld_file.to_string_lossy().to_string()]);
                if !is_base {
                    args.push("--kl-divergence".into());
                }
            }
            args.extend(req.extra_args.iter().cloned());
            let label = format!("{}/{} {}", k + 1, total, m.file_name);
            job.progress(Some(k as f64 / total as f64), label.clone());
            let t0 = std::time::Instant::now();
            let out = run(&bin, &args, |done, all| {
                if all > 0 {
                    job.progress(Some((k as f64 + done as f64 / all as f64) / total as f64), format!("{label}: {done}/{all}"));
                }
            })
            .await;
            let mut r = PplRun {
                id: new_id(), group: group.clone(), model_id: m.id.clone(), model: m.file_name.clone(), quant: crate::models::quant_from_name(&m.file_name), size: m.size,
                backend_id: req.backend_id.clone(), dataset: ds_name.clone(), ctx, chunks, is_base, secs: t0.elapsed().as_secs_f64(), created_at: now(), ..Default::default()
            };
            match out {
                Ok(text) => {
                    let p = parse(&text);
                    (r.ppl, r.ppl_err, r.kld, r.kld_err, r.kld_p99, r.same_top, r.rms_dp) = (p.ppl, p.ppl_err, p.kld, p.kld_err, p.kld_p99, p.same_top, p.rms_dp);
                    if r.ppl.is_none() {
                        r.error = Some(text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("нет результата").to_string());
                    }
                }
                Err(e) => r.error = Some(e.to_string()),
            }
            if is_base && r.error.is_some() {
                let _ = app.store.put("ppl_runs", &r.id, &r);
                results.push(r);
                break; // others need the base logits
            }
            let _ = app.store.put("ppl_runs", &r.id, &r);
            results.push(r);
        }
        let _ = std::fs::remove_file(&kld_file);
        job.done(serde_json::to_value(&results).unwrap_or_default(), "Готово");
    });
    Ok(id)
}

async fn run(bin: &Path, args: &[String], mut on_progress: impl FnMut(u32, u32)) -> Result<String> {
    let mut c = crate::proc_util::command(bin);
    c.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    c.kill_on_drop(true);
    let mut child = c.spawn().with_context(|| format!("не удалось запустить {}", bin.display()))?;
    let (so, se) = (child.stdout.take().context("stdout")?, child.stderr.take().context("stderr")?);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    for r in [Box::new(so) as Box<dyn tokio::io::AsyncRead + Unpin + Send>, Box::new(se)] {
        let tx = tx.clone();
        tokio::spawn(async move {
            // llama-perplexity prints "[1]5.12,[2]5.34," without newlines: split on ',' too
            let mut rd = BufReader::new(r);
            let mut buf = vec![];
            while let Ok(n) = rd.read_until(b',', &mut buf).await {
                if n == 0 {
                    break;
                }
                let _ = tx.send(String::from_utf8_lossy(&buf).to_string());
                buf.clear();
            }
        });
    }
    drop(tx);
    let mut text = String::new();
    let mut total = 0u32;
    while let Some(s) = rx.recv().await {
        text.push_str(&s);
        if total == 0 {
            total = parse(&text).total_chunks.unwrap_or(0);
        }
        if let Some(d) = Regex::new(r"\[([0-9]+)\][0-9.]+").ok().and_then(|r| r.captures_iter(&s).last().and_then(|c| c[1].parse().ok())) {
            on_progress(d, total);
        }
    }
    let st = child.wait().await?;
    if !st.success() && parse(&text).ppl.is_none() {
        bail!("llama-perplexity завершился с ошибкой: {}", text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(""));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_ppl() {
        let t = "perplexity: calculating perplexity over 40 chunks, n_ctx=512\n[1]4.1234,[2]5.0000,[40]6.5432,\nFinal estimate: PPL = 6.5432 +/- 0.04123\n";
        let p = parse(t);
        assert_eq!((p.ppl, p.ppl_err, p.total_chunks, p.done_chunks), (Some(6.5432), Some(0.04123), Some(40), Some(40)));
    }
    #[test]
    fn parse_kld() {
        let t = "====== Perplexity statistics ======\nMean PPL(Q)                   :   6.612345 ±   0.041000\nMean PPL(base)                :   6.543210 ±   0.040000\n====== KL divergence statistics ======\nMean    KLD:   0.012345 ±   0.000123\nMaximum KLD:   3.210000\n99.9%   KLD:   0.900000\n99.0%   KLD:   0.150000\n====== Token probability statistics ======\nRMS Δp    :   2.345 ±  0.012 %\nSame top p:  95.123 ±  0.123 %\n";
        let p = parse(t);
        assert_eq!(p.ppl, Some(6.612345));
        assert_eq!((p.kld, p.kld_err, p.kld_p99), (Some(0.012345), Some(0.000123), Some(0.15)));
        assert_eq!((p.same_top, p.rms_dp), (Some(95.123), Some(2.345)));
    }
}
