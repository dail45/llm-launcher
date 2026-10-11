//! llama-bench integration: matrix of backends × parameter sets.
use crate::app::App;
use crate::types::{new_id, now, BenchRun};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
pub struct BenchReq {
    pub model_id: String,
    pub backend_ids: Vec<String>,
    /// each map: llama-bench flag -> value (e.g. {"-ngl": 99, "-fa": 1, "-ctk": "q8_0"})
    pub param_sets: Vec<BTreeMap<String, Value>>,
    pub pp: Option<u64>,
    pub tg: Option<u64>,
    pub reps: Option<u64>,
}

fn v2s(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "1".into() } else { "0".into() },
        other => other.to_string(),
    }
}

pub fn start(app: Arc<App>, req: BenchReq) -> Result<String> {
    let model = app.model(&req.model_id)?;
    if req.backend_ids.is_empty() {
        bail!("выберите хотя бы одну сборку");
    }
    let id = app.jobs.start(&app.bus, "bench", &format!("Бенчмарк {}", model.alias)).id;
    let jid = id.clone();
    tokio::spawn(async move {
        let sets = if req.param_sets.is_empty() { vec![BTreeMap::new()] } else { req.param_sets.clone() };
        let total = req.backend_ids.len() * sets.len();
        let mut k = 0;
        let mut results = vec![];
        for bid in &req.backend_ids {
            for set in &sets {
                k += 1;
                app.jobs.handle(&app.bus, &jid).progress(Some((k - 1) as f64 / total as f64), format!("{k}/{total}"));
                let run = run_one(&app, &model, bid, set, &req).await;
                let r = match run {
                    Ok(r) => r,
                    Err(e) => BenchRun { id: new_id(), model_id: model.id.clone(), backend_id: bid.clone(), params: set.clone(), error: Some(e.to_string()), created_at: now(), ..Default::default() },
                };
                let _ = app.store.put("bench_runs", &r.id, &r);
                results.push(r);
            }
        }
        app.jobs.handle(&app.bus, &jid).done(serde_json::to_value(&results).unwrap_or_default(), "Готово");
    });
    Ok(id)
}

async fn run_one(app: &App, model: &crate::types::Model, bid: &str, set: &BTreeMap<String, Value>, req: &BenchReq) -> Result<BenchRun> {
    let b = app.backend(bid)?;
    let bin = b.bench_bin.clone().context("в этой сборке нет llama-bench")?;
    let pp = req.pp.unwrap_or(512);
    let tg = req.tg.unwrap_or(128);
    let mut args: Vec<String> = vec!["-m".into(), model.paths[0].clone(), "-p".into(), pp.to_string(), "-n".into(), tg.to_string(), "-r".into(), req.reps.unwrap_or(3).to_string(), "-o".into(), "json".into()];
    for (k, v) in set {
        if k.starts_with('-') {
            args.push(k.clone());
            args.push(v2s(v));
        }
    }
    let mut c = crate::proc_util::command(std::path::Path::new(&bin));
    c.args(&args).stdin(std::process::Stdio::null());
    let out = tokio::time::timeout(Duration::from_secs(3600), c.output()).await.context("таймаут бенчмарка")??;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let start = stdout.find('[').context(format!("llama-bench: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("нет вывода")))?;
    let v: Value = serde_json::from_str(&stdout[start..]).context("не удалось разобрать JSON llama-bench")?;
    let mut r = BenchRun { id: new_id(), model_id: model.id.clone(), backend_id: bid.into(), params: set.clone(), pp: Some(pp), tg: Some(tg), raw: v.clone(), created_at: now(), ..Default::default() };
    for e in v.as_array().into_iter().flatten() {
        let ts = e["avg_ts"].as_f64();
        if e["n_prompt"].as_u64().unwrap_or(0) > 0 && e["n_gen"].as_u64().unwrap_or(0) == 0 {
            r.pp_ts = ts;
        } else if e["n_gen"].as_u64().unwrap_or(0) > 0 {
            r.tg_ts = ts;
        }
    }
    Ok(r)
}
