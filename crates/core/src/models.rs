//! Model library: scanning folders for GGUF files, split grouping, metadata, mmproj linking.
use crate::gguf::{self, MetaValue};
use crate::memory::TensorLite;
use crate::store::Store;
use crate::types::{new_id, now, Model, ModelDir};
use anyhow::Result;
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static RE_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(.*)-(\d{5})-of-(\d{5})\.gguf$").unwrap());
static RE_QUANT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(UD-)?(IQ\d_[A-Z]+|IQ\d_[A-Z]_?[A-Z]*|Q\d_K_[SMLX]+|Q\d_K|Q\d_\d|TQ\d_\d|MXFP4(_MOE)?|BF16|F16|F32|Q\d_[A-Z]+)").unwrap()
});

pub fn quant_from_name(name: &str) -> Option<String> {
    RE_QUANT.find_iter(name).last().map(|m| m.as_str().to_uppercase())
}

/// base name for split files (without -0000x-of-0000y)
pub fn split_base(file: &str) -> Option<(String, u32, u32)> {
    let c = RE_SPLIT.captures(file)?;
    Some((c[1].to_string(), c[2].parse().ok()?, c[3].parse().ok()?))
}

pub fn make_alias(file_name: &str) -> String {
    let base = split_base(file_name).map(|(b, _, _)| b).unwrap_or_else(|| file_name.trim_end_matches(".gguf").trim_end_matches(".GGUF").to_string());
    let mut s: String = base
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '-' })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s.trim_matches('-').to_string()
}

fn meta_to_json(m: &BTreeMap<String, MetaValue>) -> BTreeMap<String, serde_json::Value> {
    m.iter()
        .filter(|(k, _)| !k.starts_with("tokenizer.ggml.") || k.ends_with("model") || k.ends_with("_id") || k.ends_with("pre"))
        .map(|(k, v)| {
            let j = match v {
                MetaValue::Str(s) if s.len() > 20000 => serde_json::Value::String(format!("{}… ({} символов)", &s[..s.char_indices().nth(2000).map(|x| x.0).unwrap_or(s.len())], s.len())),
                other => serde_json::to_value(other).unwrap_or_default(),
            };
            (k.clone(), j)
        })
        .collect()
}

/// Parse files of one logical model (main + split parts).
pub fn read_model(paths: &[PathBuf]) -> Result<Model> {
    let mut parts = vec![];
    let mut size = 0;
    let mut mtime = 0;
    for p in paths {
        let md = std::fs::metadata(p)?;
        size += md.len();
        if let Ok(t) = md.modified() {
            mtime = mtime.max(t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0));
        }
        parts.push(gguf::parse_file(p)?);
    }
    let meta = parts.first().map(|p| meta_to_json(&p.metadata)).unwrap_or_default();
    let merged = gguf::merge_splits(parts).ok_or_else(|| anyhow::anyhow!("empty model"))?;
    let mut info = merged.info();
    let file_name = paths[0].file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if file_name.to_lowercase().contains("mmproj") {
        info.is_mmproj = true;
    }
    if info.file_type.is_none() {
        info.file_type = quant_from_name(&file_name);
    }
    let tensors = merged.tensors.iter().map(|t| TensorLite { n: t.name.clone(), b: t.bytes }).collect();
    Ok(Model {
        id: new_id(),
        alias: make_alias(&file_name),
        paths: paths.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
        size,
        file_name,
        info,
        meta,
        tensors,
        mtime,
        added_at: now(),
        ..Default::default()
    })
}

fn collect_gguf(dir: &ModelDir) -> Vec<Vec<PathBuf>> {
    let depth = if dir.recursive { 16 } else { 1 };
    let mut singles = vec![];
    let mut splits: HashMap<(PathBuf, String), Vec<(u32, PathBuf)>> = HashMap::new();
    for e in walkdir::WalkDir::new(&dir.path).max_depth(depth).follow_links(true).into_iter().filter_map(|e| e.ok()) {
        if !e.file_type().is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if !name.to_lowercase().ends_with(".gguf") {
            continue;
        }
        let p = e.into_path();
        if let Some((base, idx, _)) = split_base(&name) {
            let parent = p.parent().unwrap_or(Path::new("")).to_path_buf();
            splits.entry((parent, base)).or_default().push((idx, p));
        } else {
            singles.push(vec![p]);
        }
    }
    for (_, mut v) in splits {
        v.sort_by_key(|x| x.0);
        singles.push(v.into_iter().map(|x| x.1).collect());
    }
    singles
}

#[derive(Debug, Default, serde::Serialize)]
pub struct ScanReport {
    pub added: usize,
    pub updated: usize,
    pub missing: usize,
    pub errors: Vec<String>,
}

pub fn unique_alias(existing: &[Model], want: &str, self_id: &str) -> String {
    let taken = |a: &str| existing.iter().any(|m| m.id != self_id && m.alias == a);
    if !taken(want) {
        return want.to_string();
    }
    (2..).map(|i| format!("{want}-{i}")).find(|a| !taken(a)).unwrap()
}

/// Rescan all enabled dirs; blocking (call via spawn_blocking).
pub fn scan_all(store: &Store) -> Result<ScanReport> {
    let dirs: Vec<ModelDir> = store.list("model_dirs")?;
    let mut models: Vec<Model> = store.list("models")?;
    let mut rep = ScanReport::default();
    let mut seen: Vec<String> = vec![];
    for d in dirs.iter().filter(|d| d.enabled) {
        for group in collect_gguf(d) {
            let main = group[0].to_string_lossy().into_owned();
            seen.push(main.clone());
            let size: u64 = group.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
            if let Some(existing) = models.iter_mut().find(|m| m.paths.first() == Some(&main)) {
                if existing.size == size && !existing.missing && !existing.tensors.is_empty() {
                    continue;
                }
                match read_model(&group) {
                    Ok(fresh) => {
                        existing.paths = fresh.paths;
                        existing.size = fresh.size;
                        existing.info = fresh.info;
                        existing.meta = fresh.meta;
                        existing.tensors = fresh.tensors;
                        existing.mtime = fresh.mtime;
                        existing.missing = false;
                        store.put("models", &existing.id, existing)?;
                        rep.updated += 1;
                    }
                    Err(e) => rep.errors.push(format!("{main}: {e}")),
                }
                continue;
            }
            match read_model(&group) {
                Ok(mut m) => {
                    m.dir_id = Some(d.id.clone());
                    m.alias = unique_alias(&models, &m.alias, &m.id);
                    store.put("models", &m.id, &m)?;
                    models.push(m);
                    rep.added += 1;
                }
                Err(e) => rep.errors.push(format!("{main}: {e}")),
            }
        }
    }
    for m in models.iter_mut() {
        let main = m.paths.first().cloned().unwrap_or_default();
        let exists = Path::new(&main).exists();
        if !exists && !m.missing {
            m.missing = true;
            store.put("models", &m.id, m)?;
            rep.missing += 1;
        } else if exists && m.missing && seen.contains(&main) {
            m.missing = false;
            store.put("models", &m.id, m)?;
        }
    }
    link_mmproj(store, &mut models)?;
    Ok(rep)
}

/// Auto-link mmproj files to models in the same folder (if unambiguous or name prefix matches).
pub fn link_mmproj(store: &Store, models: &mut [Model]) -> Result<()> {
    let projs: Vec<(String, String, String)> = models
        .iter()
        .filter(|m| m.info.is_mmproj)
        .map(|m| {
            let dir = Path::new(&m.paths[0]).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            (m.id.clone(), dir, m.file_name.to_lowercase())
        })
        .collect();
    for m in models.iter_mut().filter(|m| !m.info.is_mmproj && m.mmproj_id.is_none()) {
        let dir = Path::new(&m.paths[0]).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let same: Vec<&(String, String, String)> = projs.iter().filter(|p| p.1 == dir).collect();
        let pick = if same.len() == 1 {
            Some(same[0].0.clone())
        } else {
            let stem = m.alias.split('-').take(2).collect::<Vec<_>>().join("-");
            same.iter().find(|p| p.2.contains(&stem)).map(|p| p.0.clone())
        };
        if let Some(id) = pick {
            m.mmproj_id = Some(id);
            store.put("models", &m.id, &*m)?;
        }
    }
    Ok(())
}

/// Register a single file (e.g. after download) if not yet known.
pub fn add_file(store: &Store, path: &Path, hf_repo: Option<String>) -> Result<Model> {
    let mut models: Vec<Model> = store.list("models")?;
    let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let group: Vec<PathBuf> = if let Some((base, _, total)) = split_base(&name) {
        let dir = path.parent().unwrap_or(Path::new("."));
        (1..=total).map(|i| dir.join(format!("{base}-{i:05}-of-{total:05}.gguf"))).collect()
    } else {
        vec![path.to_path_buf()]
    };
    let main = group[0].to_string_lossy().into_owned();
    if let Some(m) = models.iter().find(|m| m.paths.first() == Some(&main)) {
        return Ok(m.clone());
    }
    let mut m = read_model(&group)?;
    m.hf_repo = hf_repo;
    m.alias = unique_alias(&models, &m.alias, &m.id);
    store.put("models", &m.id, &m)?;
    models.push(m.clone());
    link_mmproj(store, &mut models)?;
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names() {
        assert_eq!(quant_from_name("Qwen3-30B-A3B-Instruct-2507-UD-Q4_K_XL-00001-of-00002.gguf").as_deref(), Some("UD-Q4_K_XL"));
        assert_eq!(quant_from_name("gemma-3-27b-it-IQ4_XS.gguf").as_deref(), Some("IQ4_XS"));
        assert_eq!(make_alias("Qwen3-30B-A3B-Q4_K_M-00001-of-00002.gguf"), "qwen3-30b-a3b-q4_k_m");
        assert_eq!(split_base("a-00002-of-00003.gguf"), Some(("a".into(), 2, 3)));
    }
}
