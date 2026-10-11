//! Building llama-server command lines from presets, importing command lines, and memory inputs.
use crate::help_parser::{quote_arg, split_command_line, ParamKind, ParamSchema};
use crate::memory::{DeviceSlot, MemInput};
use crate::types::{Backend, Model, Preset};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Flags managed by the launcher itself (never taken from args).
pub const MANAGED: &[&str] = &["--model", "--host", "--port", "--alias", "--mmproj", "--model-draft", "--lora", "--api-key", "--hf-repo", "--hf-file"];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LaunchSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: String,
    pub warnings: Vec<String>,
}

impl LaunchSpec {
    pub fn preview(&self, shell: &str) -> String {
        let mut s = String::new();
        for (k, v) in &self.env {
            match shell {
                "powershell" => s.push_str(&format!("$env:{k}=\"{v}\"; ")),
                _ => s.push_str(&format!("set {k}={v}&& ")),
            }
        }
        if shell == "powershell" {
            s.push_str("& ");
        }
        s.push_str(&quote_arg(&self.program));
        for a in &self.args {
            s.push(' ');
            s.push_str(&quote_arg(a));
        }
        s
    }
}

fn val_to_string(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(if *b { "1".into() } else { "0".into() }),
        _ => Some(v.to_string()),
    }
}

pub struct LaunchCtx<'a> {
    pub preset: &'a Preset,
    pub backend: &'a Backend,
    pub schema: Option<&'a ParamSchema>,
    pub model: &'a Model,
    pub mmproj: Option<&'a Model>,
    pub draft: Option<&'a Model>,
    pub loras: Vec<&'a Model>,
    pub host: String,
    pub port: u16,
    pub alias: Option<String>,
}

pub fn build_launch(c: &LaunchCtx) -> LaunchSpec {
    let mut warnings = vec![];
    let mut args: Vec<String> = vec![];
    let push = |args: &mut Vec<String>, f: &str, v: Option<String>| {
        args.push(f.to_string());
        if let Some(v) = v {
            args.push(v);
        }
    };
    push(&mut args, "--model", Some(c.model.paths.first().cloned().unwrap_or_default()));
    if let Some(m) = c.mmproj {
        push(&mut args, "--mmproj", m.paths.first().cloned());
    }
    if let Some(d) = c.draft {
        push(&mut args, "--model-draft", d.paths.first().cloned());
    }
    for l in &c.loras {
        push(&mut args, "--lora", l.paths.first().cloned());
    }
    push(&mut args, "--host", Some(c.host.clone()));
    push(&mut args, "--port", Some(c.port.to_string()));
    if let Some(a) = &c.alias {
        push(&mut args, "--alias", Some(a.clone()));
    }

    for (id, v) in &c.preset.args {
        if MANAGED.contains(&id.as_str()) {
            continue;
        }
        let def = c.schema.and_then(|s| s.find(id));
        if def.is_none() && c.schema.is_some() {
            warnings.push(format!("Флаг {id} не поддерживается сборкой «{}»", c.backend.name));
        }
        let kind = def.map(|d| d.kind.clone()).unwrap_or(match v {
            Value::Bool(_) => ParamKind::Flag,
            _ => ParamKind::Text,
        });
        let flag = def.map(|d| d.id.clone()).unwrap_or_else(|| id.clone());
        match (&kind, v) {
            (_, Value::Null) => {}
            (ParamKind::Flag, Value::Bool(b)) => {
                if *b {
                    args.push(flag);
                }
            }
            (ParamKind::Toggle, Value::Bool(b)) => {
                if *b {
                    args.push(flag);
                } else if let Some(n) = def.and_then(|d| d.neg_flag.clone()) {
                    args.push(n);
                }
            }
            (_, Value::Array(items)) => {
                for it in items {
                    if let Some(s) = val_to_string(it) {
                        push(&mut args, &flag, Some(s));
                    }
                }
            }
            (_, v) => {
                if let Some(s) = val_to_string(v) {
                    if s.is_empty() {
                        continue;
                    }
                    push(&mut args, &flag, Some(s));
                }
            }
        }
    }

    let mut env: Vec<(String, String)> = c.preset.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    if !c.preset.devices.is_empty() {
        let supports_device = c.schema.map(|s| s.has("--device")).unwrap_or(false);
        if supports_device && !c.preset.args.contains_key("--device") {
            push(&mut args, "--device", Some(c.preset.devices.join(",")));
        } else if let Some(var) = c.backend.kind.visible_devices_env() {
            if !env.iter().any(|(k, _)| k == var) {
                let idx: Vec<String> = c.preset.devices.iter().map(|d| d.trim_start_matches(|ch: char| ch.is_alphabetic()).to_string()).collect();
                env.push((var.to_string(), idx.join(",")));
            }
        }
    }
    if !c.preset.extra_args.trim().is_empty() {
        args.extend(split_command_line(&c.preset.extra_args));
    }
    LaunchSpec {
        program: c.backend.server_bin.clone(),
        cwd: std::path::Path::new(&c.backend.server_bin).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
        args,
        env,
        warnings,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImportedCommand {
    pub args: BTreeMap<String, Value>,
    pub model_path: Option<String>,
    pub mmproj_path: Option<String>,
    pub draft_path: Option<String>,
    pub lora_paths: Vec<String>,
    pub extra_args: String,
    pub env: BTreeMap<String, String>,
    pub ignored: Vec<String>,
}

/// Parse `llama-server ...` command line into preset fields using the backend schema.
pub fn import_command(cmd: &str, schema: &ParamSchema) -> ImportedCommand {
    let mut toks = split_command_line(cmd);
    let mut out = ImportedCommand::default();
    // leading env assignments: KEY=VAL or "set KEY=VAL&&"
    while let Some(t) = toks.first() {
        let t2 = t.trim_end_matches("&&");
        if t2.eq_ignore_ascii_case("set") {
            toks.remove(0);
            continue;
        }
        if let Some((k, v)) = t2.split_once('=') {
            if !k.starts_with('-') && k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
                out.env.insert(k.to_string(), v.to_string());
                toks.remove(0);
                continue;
            }
        }
        break;
    }
    if toks.first().map(|t| !t.starts_with('-')).unwrap_or(false) {
        toks.remove(0); // program
    }
    let mut extra: Vec<String> = vec![];
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i].clone();
        i += 1;
        let (flag, inline_val) = match t.split_once('=') {
            Some((f, v)) if t.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (t.clone(), None),
        };
        let Some(def) = schema.find(&flag) else {
            extra.push(t);
            // keep following value tokens with the unknown flag
            while i < toks.len() && !toks[i].starts_with('-') {
                extra.push(toks[i].clone());
                i += 1;
            }
            continue;
        };
        let takes_value = !matches!(def.kind, ParamKind::Flag | ParamKind::Toggle);
        let value = if takes_value {
            inline_val.or_else(|| {
                if i < toks.len() {
                    i += 1;
                    Some(toks[i - 1].clone())
                } else {
                    None
                }
            })
        } else {
            None
        };
        let id = def.id.clone();
        match id.as_str() {
            "--model" => out.model_path = value,
            "--mmproj" => out.mmproj_path = value,
            "--model-draft" => out.draft_path = value,
            "--lora" => out.lora_paths.extend(value),
            "--host" | "--port" | "--alias" | "--api-key" => out.ignored.push(id),
            _ => {
                let v = match def.kind {
                    ParamKind::Flag => Value::Bool(true),
                    ParamKind::Toggle => Value::Bool(def.neg_flag.as_deref() != Some(flag.as_str())),
                    ParamKind::Int => value.as_deref().and_then(|s| s.parse::<i64>().ok()).map(Value::from).unwrap_or_else(|| Value::from(value.clone().unwrap_or_default())),
                    ParamKind::Float => value.as_deref().and_then(|s| s.parse::<f64>().ok()).map(Value::from).unwrap_or_else(|| Value::from(value.clone().unwrap_or_default())),
                    _ => Value::from(value.unwrap_or_default()),
                };
                if def.repeatable {
                    let e = out.args.entry(id).or_insert(Value::Array(vec![]));
                    if let Value::Array(a) = e {
                        a.push(v);
                    }
                } else {
                    out.args.insert(id, v);
                }
            }
        }
    }
    out.extra_args = extra.iter().map(|s| quote_arg(s)).collect::<Vec<_>>().join(" ");
    out
}

fn arg<'a>(p: &'a Preset, schema: Option<&ParamSchema>, names: &[&str]) -> Option<&'a Value> {
    for n in names {
        if let Some(v) = p.args.get(*n) {
            return Some(v);
        }
        if let Some(s) = schema {
            if let Some(d) = s.find(n) {
                if let Some(v) = p.args.get(&d.id) {
                    return Some(v);
                }
            }
        }
    }
    None
}
fn as_u64(v: Option<&Value>) -> Option<u64> {
    match v? {
        Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f as u64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}
fn as_i64(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}
fn as_str(v: Option<&Value>) -> Option<String> {
    v.and_then(val_to_string)
}
fn truthy(v: Option<&Value>) -> Option<bool> {
    match v? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => Some(matches!(s.as_str(), "on" | "true" | "1" | "auto")),
        Value::Number(n) => Some(n.as_i64() != Some(0)),
        _ => None,
    }
}

/// Derive calculator input from a preset's args.
pub fn mem_input_from_preset(p: &Preset, schema: Option<&ParamSchema>, devices: Vec<DeviceSlot>) -> MemInput {
    let ot: Vec<(String, String)> = match arg(p, schema, &["--override-tensor", "-ot"]) {
        Some(Value::Array(a)) => a.iter().filter_map(val_to_string).collect::<Vec<_>>(),
        Some(v) => val_to_string(v).into_iter().collect(),
        None => vec![],
    }
    .iter()
    .flat_map(|s| s.split(',').map(|x| x.to_string()).collect::<Vec<_>>())
    .filter_map(|s| s.rsplit_once('=').map(|(a, b)| (a.to_string(), b.to_string())))
    .collect();
    let ngl = match arg(p, schema, &["--gpu-layers", "--n-gpu-layers", "-ngl"]) {
        Some(Value::String(s)) if s == "auto" || s == "all" => None,
        v => as_i64(v),
    };
    let fa = match arg(p, schema, &["--flash-attn", "-fa"]) {
        Some(Value::String(s)) => s != "off",
        Some(Value::Bool(b)) => *b,
        _ => true,
    };
    let kv_off = arg(p, schema, &["--no-kv-offload", "-nkvo"]).and_then(|v| truthy(Some(v))).unwrap_or(false)
        || matches!(arg(p, schema, &["--kv-offload"]), Some(Value::Bool(false)));
    let mmap = !matches!(arg(p, schema, &["--mmap"]), Some(Value::Bool(false))) && !matches!(arg(p, schema, &["--no-mmap"]), Some(Value::Bool(true)));
    MemInput {
        n_ctx: as_u64(arg(p, schema, &["--ctx-size", "-c"])).unwrap_or(4096),
        n_gpu_layers: ngl,
        n_batch: as_u64(arg(p, schema, &["--batch-size", "-b"])).unwrap_or(2048),
        n_ubatch: as_u64(arg(p, schema, &["--ubatch-size", "-ub"])).unwrap_or(512),
        cache_k: as_str(arg(p, schema, &["--cache-type-k", "-ctk"])).unwrap_or_else(|| "f16".into()),
        cache_v: as_str(arg(p, schema, &["--cache-type-v", "-ctv"])).unwrap_or_else(|| "f16".into()),
        flash_attn: fa,
        no_kv_offload: kv_off,
        n_cpu_moe: as_u64(arg(p, schema, &["--n-cpu-moe", "-ncmoe"])).unwrap_or(0),
        cpu_moe: truthy(arg(p, schema, &["--cpu-moe", "-cmoe"])).unwrap_or(false),
        override_tensor: ot,
        split_mode: as_str(arg(p, schema, &["--split-mode", "-sm"])).unwrap_or_else(|| "layer".into()),
        tensor_split: as_str(arg(p, schema, &["--tensor-split", "-ts"]))
            .map(|s| s.split([',', '/']).filter_map(|x| x.trim().parse().ok()).collect())
            .unwrap_or_default(),
        main_gpu: as_u64(arg(p, schema, &["--main-gpu", "-mg"])).unwrap_or(0) as usize,
        parallel: as_u64(arg(p, schema, &["--parallel", "-np"])).unwrap_or(1),
        swa_full: truthy(arg(p, schema, &["--swa-full"])).unwrap_or(false),
        devices,
        mmap,
        compute_coef: 1.0,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::help_parser::parse_help;

    const HELP: &str = "----- common -----\n-t,    --threads N                      threads (default: -1)\n-c,    --ctx-size N                     ctx (default: 4096)\n--mmap, --no-mmap                       mmap (default: enabled)\n--jinja                                 use jinja\n-ctk,  --cache-type-k TYPE              K type\n                                        allowed values: f16, q8_0, turbo3\n-m,    --model FNAME                    model path\n--port PORT                             port (default: 8080)\n--device <dev1,dev2,..>                 devices\n--lora FNAME                            lora (can be repeated to use multiple adapters)\n";

    #[test]
    fn import_and_build() {
        let s = parse_help(HELP);
        let imp = import_command(r#"CUDA_VISIBLE_DEVICES=0 llama-server.exe -m "C:\m\a b.gguf" -c 8192 --no-mmap --jinja -ctk turbo3 --port 9000 --weird 5"#, &s);
        assert_eq!(imp.model_path.as_deref(), Some(r"C:\m\a b.gguf"));
        assert_eq!(imp.args["--ctx-size"], Value::from(8192));
        assert_eq!(imp.args["--mmap"], Value::Bool(false));
        assert_eq!(imp.args["--jinja"], Value::Bool(true));
        assert_eq!(imp.args["--cache-type-k"], Value::from("turbo3"));
        assert_eq!(imp.extra_args, "--weird 5");
        assert_eq!(imp.env["CUDA_VISIBLE_DEVICES"], "0");

        let preset = Preset { args: imp.args.clone(), extra_args: imp.extra_args.clone(), devices: vec!["CUDA1".into()], ..Default::default() };
        let backend = Backend { name: "b".into(), server_bin: "/x/llama-server".into(), ..Default::default() };
        let model = Model { paths: vec!["/m/a.gguf".into()], ..Default::default() };
        let spec = build_launch(&LaunchCtx { preset: &preset, backend: &backend, schema: Some(&s), model: &model, mmproj: None, draft: None, loras: vec![], host: "127.0.0.1".into(), port: 51000, alias: Some("x".into()) });
        let a = spec.args.join(" ");
        assert!(a.contains("--model /m/a.gguf"));
        assert!(a.contains("--no-mmap"));
        assert!(a.contains("--ctx-size 8192"));
        assert!(a.contains("--device CUDA1"));
        assert!(a.ends_with("--weird 5"));
        let mi = mem_input_from_preset(&preset, Some(&s), vec![]);
        assert_eq!(mi.n_ctx, 8192);
        assert!(!mi.mmap);
        assert_eq!(mi.cache_k, "turbo3");
    }
}
