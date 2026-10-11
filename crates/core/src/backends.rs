//! Registry of llama.cpp builds: detection, version, parameter schema and devices.
use crate::help_parser::{self, ParamSchema};
use crate::paths;
use crate::proc_util::run_capture;
use crate::types::{new_id, now, Backend, BackendKind};
use anyhow::{bail, Result};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn find_server_bin(dir: &Path) -> Option<PathBuf> {
    let name = paths::exe_name("llama-server");
    if dir.is_file() {
        return Some(dir.to_path_buf());
    }
    for cand in [dir.join(&name), dir.join("bin").join(&name), dir.join("build").join("bin").join(&name), dir.join("build").join("bin").join("Release").join(&name)] {
        if cand.is_file() {
            return Some(cand);
        }
    }
    walkdir::WalkDir::new(dir)
        .max_depth(4)
        .into_iter()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(&name))
        .map(|e| e.into_path())
}

fn bin_fingerprint(p: &Path) -> String {
    let md = std::fs::metadata(p).ok();
    let mut h = Sha256::new();
    h.update(p.to_string_lossy().as_bytes());
    if let Some(m) = md {
        h.update(m.len().to_le_bytes());
        if let Ok(t) = m.modified() {
            if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                h.update(d.as_secs().to_le_bytes());
            }
        }
    }
    hex::encode(&h.finalize()[..12])
}

fn schema_cache_path(hash: &str) -> PathBuf {
    paths::data_dir().join("schemas").join(format!("{hash}.json"))
}

pub fn load_schema(b: &Backend) -> Option<ParamSchema> {
    let h = b.bin_hash.as_ref()?;
    let s = std::fs::read_to_string(schema_cache_path(h)).ok()?;
    serde_json::from_str(&s).ok()
}

pub fn parse_version(text: &str) -> (Option<String>, Option<String>) {
    let re = Regex::new(r"version:\s*(\d+)\s*\(([0-9a-fA-F]+)\)").unwrap();
    if let Some(c) = re.captures(text) {
        return (Some(c[1].to_string()), Some(c[2].to_string()));
    }
    let re2 = Regex::new(r"(?i)version[:\s]+([\w.\-]+)").unwrap();
    (re2.captures(text).map(|c| c[1].to_string()), None)
}

/// Detect / refresh a backend: version, --help schema (cached by binary fingerprint), devices.
pub async fn scan(mut b: Backend) -> Result<(Backend, ParamSchema)> {
    let Some(bin) = find_server_bin(Path::new(&b.dir)) else {
        bail!("llama-server не найден в {}", b.dir);
    };
    b.server_bin = bin.to_string_lossy().into();
    let dir = bin.parent().unwrap_or(Path::new(".")).to_path_buf();
    let bench = dir.join(paths::exe_name("llama-bench"));
    b.bench_bin = bench.is_file().then(|| bench.to_string_lossy().into());
    let hash = bin_fingerprint(&bin);

    let v = run_capture(&bin, &["--version"], Duration::from_secs(30)).await.unwrap_or_default();
    let (ver, commit) = parse_version(&v);
    b.version = ver;
    b.commit = commit;

    let schema = match (b.bin_hash.as_deref() == Some(&hash)).then(|| load_schema(&b)).flatten() {
        Some(s) => s,
        None => {
            let help = run_capture(&bin, &["--help"], Duration::from_secs(30)).await?;
            let s = help_parser::parse_help(&help);
            if s.params.len() < 5 {
                bail!("не удалось разобрать --help (найдено {} параметров)", s.params.len());
            }
            let p = schema_cache_path(&hash);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&p, serde_json::to_string(&s)?)?;
            s
        }
    };
    b.bin_hash = Some(hash);
    let devs = run_capture(&bin, &["--list-devices"], Duration::from_secs(30)).await.unwrap_or_default();
    b.devices = help_parser::parse_devices(&devs);
    if b.kind == BackendKind::Other {
        let k = b.devices.first().map(|d| BackendKind::guess(&d.id)).unwrap_or_default();
        b.kind = if k != BackendKind::Other { k } else { BackendKind::guess(&b.dir) };
    }
    b.features = features_of(&schema);
    b.scan_error = None;
    Ok((b, schema))
}

/// Features: cache types and notable flags (used for preset compatibility checks).
pub fn features_of(s: &ParamSchema) -> Vec<String> {
    let mut f: Vec<String> = s.choices("--cache-type-k").into_iter().map(|c| format!("cache-type:{c}")).collect();
    for p in &s.params {
        f.push(p.id.clone());
    }
    f.sort();
    f.dedup();
    f
}

pub fn missing_features(b: &Backend, required: &[String]) -> Vec<String> {
    required.iter().filter(|r| !b.features.contains(r)).cloned().collect()
}

pub fn new_backend(name: &str, dir: &str) -> Backend {
    Backend {
        id: new_id(),
        name: name.to_string(),
        kind: BackendKind::guess(&format!("{name} {dir}")),
        dir: dir.to_string(),
        created_at: now(),
        ..Default::default()
    }
}
