//! Parses `llama-server --help` output into a parameter schema, so the UI form
//! always matches the exact build (official or fork).
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    /// Flag without value (presence = on)
    Flag,
    /// `--x` / `--no-x` pair: tri-state (unset / on / off)
    Toggle,
    Int,
    Float,
    Text,
    Path,
    Choice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamDef {
    /// Canonical id: first long flag (e.g. `--ctx-size`)
    pub id: String,
    pub flags: Vec<String>,
    /// For toggles: the negative flag (e.g. `--no-mmap`)
    pub neg_flag: Option<String>,
    pub value_hint: Option<String>,
    pub kind: ParamKind,
    pub choices: Vec<String>,
    pub default: Option<String>,
    pub env: Option<String>,
    pub description: String,
    pub section: String,
    pub group: String,
    pub repeatable: bool,
    pub basic: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ParamSchema {
    pub params: Vec<ParamDef>,
}

impl ParamSchema {
    pub fn find(&self, flag: &str) -> Option<&ParamDef> {
        self.params
            .iter()
            .find(|p| p.id == flag || p.flags.iter().any(|f| f == flag) || p.neg_flag.as_deref() == Some(flag))
    }
    pub fn has(&self, flag: &str) -> bool {
        self.find(flag).is_some()
    }
    /// Values accepted by a choice param (e.g. cache types)
    pub fn choices(&self, flag: &str) -> Vec<String> {
        self.find(flag).map(|p| p.choices.clone()).unwrap_or_default()
    }
}

static RE_DEFAULT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(default:\s*([^)]*)\)").unwrap());
static RE_ENV: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(env:\s*([A-Z0-9_]+)\)").unwrap());
static RE_ALLOWED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"allowed values:\s*([^\n(]+)").unwrap());
static RE_SECTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-{3,}\s*(.+?)\s*-{3,}\s*$").unwrap());

/// Params shown in "basic" mode.
pub const BASIC: &[&str] = &[
    "--ctx-size", "--n-gpu-layers", "--gpu-layers", "--threads", "--batch-size", "--ubatch-size",
    "--flash-attn", "--cache-type-k", "--cache-type-v", "--parallel", "--n-cpu-moe", "--mlock",
    "--mmap", "--no-mmap", "--jinja", "--reasoning-format", "--temp", "--top-k", "--top-p", "--min-p",
    "--repeat-penalty", "--split-mode", "--tensor-split", "--main-gpu", "--cont-batching",
    "--n-predict", "--reasoning-budget", "--override-tensor", "--cpu-moe", "--kv-unified", "--swa-full",
    "--device", "--mmproj", "--model-draft", "--lora", "--chat-template", "--seed",
];

fn group_for(id: &str, section: &str) -> String {
    let g = |s: &str| s.to_string();
    let s = section.to_lowercase();
    if s.contains("sampling") {
        return g("Сэмплинг");
    }
    let id = id.trim_start_matches('-');
    let has = |k: &[&str]| k.iter().any(|x| id.contains(x));
    if has(&["draft", "spec", "lookup"]) || id.starts_with("md") {
        g("Speculative")
    } else if has(&["mmproj", "image", "vision", "audio"]) {
        g("Multimodal")
    } else if has(&["lora", "control-vector"]) {
        g("LoRA")
    } else if has(&["cache-type", "kv-", "swa", "cache-reuse", "defrag", "cache-ram", "context-shift"]) {
        g("KV-кэш")
    } else if has(&["gpu", "device", "split", "override-tensor", "cpu-moe", "n-cpu-moe", "main-gpu", "offload", "numa", "rpc"]) {
        g("GPU / offload")
    } else if has(&["threads", "batch", "flash", "mlock", "mmap", "prio", "poll", "cpu-", "cont-batching", "parallel", "direct-io", "repack"]) {
        g("Производительность")
    } else if has(&["template", "jinja", "reasoning", "chat", "think"]) {
        g("Чат-шаблон / reasoning")
    } else if has(&["ctx-size", "rope", "yarn", "model", "hf-", "alias", "n-predict", "keep", "grp-attn"]) {
        g("Модель и контекст")
    } else if has(&["host", "port", "api-key", "ssl", "timeout", "path", "metrics", "slots", "props", "webui", "embedding", "rerank", "pooling", "threads-http", "endpoint", "cors", "log", "verbose"]) {
        g("Сервер")
    } else {
        g("Прочее")
    }
}

fn split_flags_and_desc(line: &str) -> Option<(String, String)> {
    // flags part ends at first run of >=2 spaces after some non-space content
    let trimmed = line.trim_start();
    if !trimmed.starts_with('-') {
        return None;
    }
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    let mut in_br = 0i32;
    while i < bytes.len() {
        let c = bytes[i] as char;
        match c {
            '[' | '{' | '<' => in_br += 1,
            ']' | '}' | '>' => in_br -= 1,
            _ => {}
        }
        if in_br <= 0 && c == ' ' && i + 1 < bytes.len() && bytes[i + 1] == b' ' {
            // could be the alignment spaces between "-t," and "--threads"
            let rest = trimmed[i..].trim_start();
            let prev = trimmed[..i].trim_end();
            if prev.ends_with(',') && rest.starts_with('-') {
                i += 1;
                continue;
            }
            return Some((prev.to_string(), rest.to_string()));
        }
        i += 1;
    }
    Some((trimmed.trim_end().to_string(), String::new()))
}

fn parse_flag_part(part: &str) -> (Vec<String>, Option<String>) {
    // e.g. "-t,    --threads N" | "-fa,   --flash-attn [on|off|auto]" | "--mmap, --no-mmap"
    let mut flags = vec![];
    let mut hint: Option<String> = None;
    let mut chunks = vec![];
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in part.chars() {
        match c {
            '[' | '{' | '<' => depth += 1,
            ']' | '}' | '>' => depth -= 1,
            _ => {}
        }
        if c == ',' && depth <= 0 {
            chunks.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    chunks.push(cur);
    for chunk in chunks.iter() {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        let mut it = chunk.splitn(2, char::is_whitespace);
        let f = it.next().unwrap_or("").trim();
        if f.starts_with('-') {
            flags.push(f.to_string());
        } else if hint.is_none() {
            hint = Some(chunk.to_string());
            continue;
        }
        if let Some(h) = it.next() {
            let h = h.trim();
            if !h.is_empty() {
                hint = Some(h.to_string());
            }
        }
    }
    (flags, hint)
}

fn choices_from_hint(h: &str) -> Vec<String> {
    let h = h.trim();
    let inner = if (h.starts_with('[') && h.ends_with(']')) || (h.starts_with('{') && h.ends_with('}')) || (h.starts_with('<') && h.ends_with('>')) {
        &h[1..h.len() - 1]
    } else {
        return vec![];
    };
    let sep = if inner.contains('|') { '|' } else { ',' };
    if !inner.contains(sep) {
        return vec![];
    }
    inner.split(sep).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

pub fn parse_help(text: &str) -> ParamSchema {
    let mut params: Vec<ParamDef> = vec![];
    let mut section = String::from("common");
    let mut cur: Option<(Vec<String>, Option<String>, String)> = None;

    let flush = |cur: &mut Option<(Vec<String>, Option<String>, String)>, params: &mut Vec<ParamDef>, section: &str| {
        if let Some((flags, hint, desc)) = cur.take() {
            if let Some(p) = build_param(flags, hint, desc, section) {
                if !params.iter().any(|x| x.id == p.id) {
                    params.push(p);
                }
            }
        }
    };

    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(c) = RE_SECTION.captures(line.trim()) {
            flush(&mut cur, &mut params, &section);
            section = c[1].trim().to_string();
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent <= 2 && line.trim_start().starts_with('-') {
            if let Some((fp, desc)) = split_flags_and_desc(line) {
                flush(&mut cur, &mut params, &section);
                let (flags, hint) = parse_flag_part(&fp);
                if !flags.is_empty() {
                    cur = Some((flags, hint, desc));
                }
                continue;
            }
        }
        if let Some((_, _, d)) = cur.as_mut() {
            if !d.is_empty() {
                d.push('\n');
            }
            d.push_str(line.trim());
        }
    }
    flush(&mut cur, &mut params, &section);
    ParamSchema { params }
}

fn build_param(flags: Vec<String>, hint: Option<String>, desc: String, section: &str) -> Option<ParamDef> {
    let long: Vec<&String> = flags.iter().filter(|f| f.starts_with("--")).collect();
    // separate negative flag (--no-x) if a positive is present too
    let mut pos_flags = flags.clone();
    let mut neg_flag = None;
    if hint.is_none() && flags.len() >= 2 {
        if let Some(n) = flags.iter().find(|f| f.starts_with("--no-")) {
            let positive = format!("--{}", &n[5..]);
            if flags.iter().any(|f| *f == positive) || long.len() >= 2 {
                neg_flag = Some(n.clone());
                pos_flags.retain(|f| f != n);
            }
        }
    }
    let id = pos_flags
        .iter()
        .find(|f| f.starts_with("--"))
        .or(pos_flags.first())
        .cloned()
        .or_else(|| neg_flag.clone())?;
    if matches!(id.as_str(), "--help" | "--usage" | "--version" | "--completion-bash" | "--list-devices" | "--licenses") {
        return None;
    }
    let default = RE_DEFAULT.captures(&desc).map(|c| c[1].trim().trim_matches('\'').trim_matches('"').to_string());
    let env = RE_ENV.captures(&desc).map(|c| c[1].to_string());
    let mut choices: Vec<String> = vec![];
    if let Some(h) = &hint {
        choices = choices_from_hint(h);
    }
    if choices.is_empty() {
        if let Some(c) = RE_ALLOWED.captures(&desc) {
            choices = c[1].split(',').map(|s| s.trim().trim_matches('\'').to_string()).filter(|s| !s.is_empty() && !s.contains(' ')).collect();
        }
    }
    let kind = match &hint {
        None => {
            if neg_flag.is_some() {
                ParamKind::Toggle
            } else {
                ParamKind::Flag
            }
        }
        Some(_) if !choices.is_empty() => ParamKind::Choice,
        Some(h) => {
            let hu = h.to_uppercase();
            if hu.contains("FNAME") || hu.contains("PATH") || hu.contains("FILE") || hu == "DIR" {
                ParamKind::Path
            } else if matches!(hu.as_str(), "N" | "SEED" | "M" | "INDEX" | "PORT" | "N0" | "N1" | "SIZE") {
                ParamKind::Int
            } else if hu.len() <= 2 && hu.chars().all(|c| c.is_ascii_uppercase()) {
                // "P", "N", "F" ... decide by default value
                match &default {
                    Some(d) if d.parse::<i64>().is_ok() && !d.contains('.') => ParamKind::Int,
                    Some(d) if d.parse::<f64>().is_ok() => ParamKind::Float,
                    _ => ParamKind::Float,
                }
            } else {
                ParamKind::Text
            }
        }
    };
    let kind = if kind == ParamKind::Int {
        match &default {
            Some(d) if d.contains('.') && d.parse::<f64>().is_ok() => ParamKind::Float,
            _ => kind,
        }
    } else {
        kind
    };
    let dl = desc.to_lowercase();
    let repeatable = dl.contains("multiple times") || dl.contains("can be repeated") || dl.contains("multiple files");
    let group = group_for(&id, section);
    let basic = BASIC.contains(&id.as_str()) || neg_flag.as_deref().is_some_and(|n| BASIC.contains(&n));
    Some(ParamDef {
        id,
        flags: pos_flags,
        neg_flag,
        value_hint: hint,
        kind,
        choices,
        default,
        env,
        description: desc,
        section: section.to_string(),
        group,
        repeatable,
        basic,
    })
}

/// Parse `--list-devices` output: lines like `  CUDA0: NVIDIA GeForce RTX 4090 (24563 MiB, 23000 MiB free)`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackendDevice {
    pub id: String,
    pub name: String,
    pub total_mib: Option<u64>,
    pub free_mib: Option<u64>,
}

pub fn parse_devices(text: &str) -> Vec<BackendDevice> {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\s*([A-Za-z]+[0-9]+):\s*(.+?)\s*(?:\((\d+)\s*MiB(?:,\s*(\d+)\s*MiB free)?\))?\s*$").unwrap()
    });
    text.lines()
        .filter_map(|l| {
            let c = RE.captures(l)?;
            Some(BackendDevice {
                id: c[1].to_string(),
                name: c[2].to_string(),
                total_mib: c.get(3).and_then(|m| m.as_str().parse().ok()),
                free_mib: c.get(4).and_then(|m| m.as_str().parse().ok()),
            })
        })
        .collect()
}

/// Windows-ish command line tokenizer (handles "quoted args" and backslash-escaped quotes).
pub fn split_command_line(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut in_q: Option<char> = None;
    let mut has = false;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match (c, in_q) {
            ('\\', _) if i + 1 < chars.len() && chars[i + 1] == '"' => {
                cur.push('"');
                i += 1;
                has = true;
            }
            ('^', None) if i + 1 < chars.len() && (chars[i + 1] == '\n' || chars[i + 1] == '\r') => {
                // cmd line continuation
            }
            ('`', None) if i + 1 < chars.len() && (chars[i + 1] == '\n' || chars[i + 1] == '\r') => {}
            ('\\', None) if i + 1 < chars.len() && (chars[i + 1] == '\n' || chars[i + 1] == '\r') => {}
            ('"', None) | ('\'', None) => {
                in_q = Some(c);
                has = true;
            }
            (q, Some(open)) if q == open => in_q = None,
            (w, None) if w.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            _ => {
                cur.push(c);
                has = true;
            }
        }
        i += 1;
    }
    if has {
        out.push(cur);
    }
    out
}

pub fn quote_arg(a: &str) -> String {
    if a.is_empty() || a.chars().any(|c| c.is_whitespace() || c == '"' || c == '&' || c == '|') {
        format!("\"{}\"", a.replace('"', "\\\""))
    } else {
        a.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const SAMPLE: &str = r#"----- common params -----

-h,    --help, --usage                  print usage and exit
--version                               show version and build info
-t,    --threads N                      number of threads to use during generation (default: -1)
                                        (env: LLAMA_ARG_THREADS)
-c,    --ctx-size N                     size of the prompt context (default: 4096, 0 = loaded from model)
                                        (env: LLAMA_ARG_CTX_SIZE)
-fa,   --flash-attn [on|off|auto]       set Flash Attention use ('on', 'off', or 'auto', default: 'auto')
                                        (env: LLAMA_ARG_FLASH_ATTN)
--mmap, --no-mmap                       whether to memory-map model (default: enabled)
                                        (env: LLAMA_ARG_MMAP)
-ctk,  --cache-type-k TYPE              KV cache data type for K
                                        allowed values: f32, f16, bf16, q8_0, q4_0, q4_1, iq4_nl, q5_0, q5_1, turbo3
                                        (default: f16)
                                        (env: LLAMA_ARG_CACHE_TYPE_K)
--mlock                                 force system to keep model in RAM rather than swapping or compressing
-m,    --model FNAME                    model path (default: `models/$filename` with filename from `--hf-file`
                                        or `--model-url` if set, otherwise models/7B/ggml-model-f16.gguf)
-ngl,  --gpu-layers, --n-gpu-layers N   max. number of layers to store in VRAM (default: -1)
--lora FNAME                            path to LoRA adapter (can be repeated to use multiple adapters)
-sm,   --split-mode {none,layer,row}    how to split the model across multiple GPUs, one of:

----- sampling params -----

--temp N                                temperature (default: 0.8)
--top-k N                               top-k sampling (default: 40, 0 = disabled)
"#;

    #[test]
    fn parses_sample() {
        let s = parse_help(SAMPLE);
        let ids: Vec<&str> = s.params.iter().map(|p| p.id.as_str()).collect();
        assert!(!ids.contains(&"--help"));
        let t = s.find("-t").unwrap();
        assert_eq!(t.id, "--threads");
        assert_eq!(t.kind, ParamKind::Int);
        assert_eq!(t.default.as_deref(), Some("-1"));
        assert_eq!(t.env.as_deref(), Some("LLAMA_ARG_THREADS"));
        let fa = s.find("--flash-attn").unwrap();
        assert_eq!(fa.kind, ParamKind::Choice);
        assert_eq!(fa.choices, vec!["on", "off", "auto"]);
        let mm = s.find("--no-mmap").unwrap();
        assert_eq!(mm.kind, ParamKind::Toggle);
        assert_eq!(mm.id, "--mmap");
        let ctk = s.find("-ctk").unwrap();
        assert_eq!(ctk.kind, ParamKind::Choice);
        assert!(ctk.choices.contains(&"turbo3".to_string()));
        assert_eq!(ctk.default.as_deref(), Some("f16"));
        assert_eq!(s.find("--mlock").unwrap().kind, ParamKind::Flag);
        assert_eq!(s.find("-m").unwrap().kind, ParamKind::Path);
        let ngl = s.find("-ngl").unwrap();
        assert_eq!(ngl.id, "--gpu-layers");
        assert!(ngl.flags.contains(&"--n-gpu-layers".to_string()));
        assert!(s.find("--lora").unwrap().repeatable);
        assert_eq!(s.find("-sm").unwrap().choices, vec!["none", "layer", "row"]);
        let temp = s.find("--temp").unwrap();
        assert_eq!(temp.kind, ParamKind::Float);
        assert_eq!(temp.group, "Сэмплинг");
    }

    #[test]
    fn devices() {
        let d = parse_devices("Available devices:\n  CUDA0: NVIDIA GeForce RTX 4090 (24563 MiB, 23000 MiB free)\n  Vulkan1: AMD Radeon RX 7900 XTX (24560 MiB, 24000 MiB free)\n");
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].id, "CUDA0");
        assert_eq!(d[1].free_mib, Some(24000));
    }

    #[test]
    fn cmdline() {
        let t = split_command_line(r#"llama-server.exe -m "C:\models\my model.gguf" -c 8192 --jinja"#);
        assert_eq!(t, vec!["llama-server.exe", "-m", r"C:\models\my model.gguf", "-c", "8192", "--jinja"]);
    }
}
