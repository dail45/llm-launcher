//! VRAM / RAM estimator for llama.cpp model loads.
use crate::gguf::ModelInfo;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MIB: f64 = 1024.0 * 1024.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TensorLite {
    pub n: String,
    pub b: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSlot {
    /// backend device id, e.g. CUDA0
    pub id: String,
    pub name: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemInput {
    pub n_ctx: u64,
    /// None or negative = all layers
    pub n_gpu_layers: Option<i64>,
    pub n_batch: u64,
    pub n_ubatch: u64,
    pub cache_k: String,
    pub cache_v: String,
    pub flash_attn: bool,
    pub no_kv_offload: bool,
    pub n_cpu_moe: u64,
    pub cpu_moe: bool,
    /// (regex, buffer type), e.g. ("blk\\.\\d+\\.ffn_.*_exps", "CPU")
    pub override_tensor: Vec<(String, String)>,
    pub split_mode: String,
    pub tensor_split: Vec<f64>,
    pub main_gpu: usize,
    pub parallel: u64,
    pub swa_full: bool,
    pub devices: Vec<DeviceSlot>,
    pub extra_gpu_bytes: u64,
    pub gpu_overhead_bytes: u64,
    pub margin_bytes: u64,
    pub ram_total_bytes: u64,
    pub ram_free_bytes: u64,
    pub compute_coef: f64,
    pub custom_cache_bits: BTreeMap<String, f64>,
    pub mmap: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Usage {
    pub weights: u64,
    pub kv: u64,
    pub compute: u64,
    pub overhead: u64,
    pub extra: u64,
}
impl Usage {
    pub fn total(&self) -> u64 {
        self.weights + self.kv + self.compute + self.overhead + self.extra
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    Good,
    Tight,
    No,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEstimate {
    pub id: String,
    pub name: String,
    pub usage: Usage,
    pub total: u64,
    pub free: u64,
    pub layers: u64,
    pub fit: Fit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Estimate {
    pub devices: Vec<DeviceEstimate>,
    pub cpu: Usage,
    pub cpu_total: u64,
    pub ram_free: u64,
    pub cpu_fit: Fit,
    pub gpu_layers: u64,
    pub n_layers: u64,
    pub output_on_gpu: bool,
    pub kv_per_token_bytes: f64,
    pub fit: Fit,
    pub warnings: Vec<String>,
}

pub fn cache_bits(t: &str, custom: &BTreeMap<String, f64>) -> (f64, bool) {
    let t = t.to_lowercase();
    if let Some(b) = custom.get(&t) {
        return (*b, true);
    }
    let b = match t.as_str() {
        "f32" => 32.0,
        "f16" | "bf16" | "" => 16.0,
        "q8_0" => 8.5,
        "q8_1" => 9.0,
        "q5_1" => 6.0,
        "q5_0" => 5.5,
        "q4_1" => 5.0,
        "q4_0" | "iq4_nl" => 4.5,
        _ => {
            // try to infer bits from a digit in the name (e.g. "turbo3", "q3_k")
            let d = t.chars().find(|c| c.is_ascii_digit()).and_then(|c| c.to_digit(10));
            return (d.map(|d| d as f64 + 0.5).unwrap_or(4.5), false);
        }
    };
    (b, true)
}

fn layer_of(name: &str) -> Option<usize> {
    let rest = name.strip_prefix("blk.")?;
    rest.split('.').next()?.parse().ok()
}

fn is_swa_layer(info: &ModelInfo, i: usize) -> bool {
    if info.sliding_window.is_none() {
        return false;
    }
    let p = info.swa_pattern.unwrap_or(match info.arch.as_str() {
        "gemma2" | "gpt-oss" | "gptoss" => 2,
        "gemma3" | "gemma3n" => 6,
        "cohere2" | "llama4" => 4,
        _ => 0,
    });
    if p == 0 {
        return false;
    }
    (i as u64 + 1) % p != 0
}

/// Expand tensors list; if absent synthesize from the info (file-size-only estimate).
fn tensors_or_synth(info: &ModelInfo, tensors: &[TensorLite], file_size: u64) -> Vec<TensorLite> {
    if !tensors.is_empty() {
        return tensors.to_vec();
    }
    let total = if info.tensor_bytes > 0 { info.tensor_bytes } else { file_size };
    let nl = info.n_layers.max(1);
    let embd = (total as f64 * 0.04) as u64;
    let per = (total - embd * 2) / nl;
    let mut v = vec![TensorLite { n: "token_embd.weight".into(), b: embd }, TensorLite { n: "output.weight".into(), b: embd }];
    for i in 0..nl {
        if info.n_expert > 0 {
            v.push(TensorLite { n: format!("blk.{i}.attn.weight"), b: per / 10 });
            v.push(TensorLite { n: format!("blk.{i}.ffn_up_exps.weight"), b: per - per / 10 });
        } else {
            v.push(TensorLite { n: format!("blk.{i}.weight"), b: per });
        }
    }
    v
}

pub fn estimate(info: &ModelInfo, tensors: &[TensorLite], file_size: u64, inp: &MemInput) -> Estimate {
    let mut warnings = vec![];
    let tensors = tensors_or_synth(info, tensors, file_size);
    let nl = info.n_layers as usize;
    let ngl = match inp.n_gpu_layers {
        Some(v) if v >= 0 => (v as usize).min(nl + 1),
        _ => nl + 1,
    };
    let ndev = inp.devices.len();
    let ngl = if ndev == 0 { 0 } else { ngl };
    let n_ctx = if inp.n_ctx == 0 { info.ctx_max.unwrap_or(4096) } else { inp.n_ctx };
    if let Some(m) = info.ctx_max {
        if n_ctx > m {
            warnings.push(format!("Контекст {n_ctx} больше обученного ({m}) — нужен RoPE scaling"));
        }
    }
    let ubatch = inp.n_ubatch.max(1).min(inp.n_batch.max(1));

    // ---- layer → device assignment
    // device index per layer; None = CPU. Offloaded layers are the last `ngl` (output counts as one).
    let first_gpu_layer = (nl + 1).saturating_sub(ngl);
    let mut split: Vec<f64> = if inp.tensor_split.len() == ndev && inp.tensor_split.iter().any(|x| *x > 0.0) {
        inp.tensor_split.clone()
    } else {
        inp.devices.iter().map(|d| d.free_bytes.max(1) as f64).collect()
    };
    if inp.split_mode == "none" && ndev > 0 {
        let mg = inp.main_gpu.min(ndev - 1);
        split = (0..ndev).map(|i| if i == mg { 1.0 } else { 0.0 }).collect();
    }
    let ssum: f64 = split.iter().sum::<f64>().max(1e-9);
    let mut cum = vec![];
    let mut acc = 0.0;
    for s in &split {
        acc += s / ssum;
        cum.push(acc);
    }
    let dev_of_slot = |slot: usize| -> Option<usize> {
        // slot 0..=nl (nl == output)
        if ndev == 0 || slot < first_gpu_layer {
            return None;
        }
        let k = slot - first_gpu_layer;
        let frac = k as f64 / ngl.max(1) as f64;
        Some(cum.iter().position(|c| frac < *c - 1e-12).unwrap_or(ndev - 1))
    };
    let layer_dev: Vec<Option<usize>> = (0..nl).map(dev_of_slot).collect();
    let output_dev = dev_of_slot(nl);

    let ot: Vec<(Regex, String)> = inp
        .override_tensor
        .iter()
        .filter_map(|(r, t)| match Regex::new(r) {
            Ok(re) => Some((re, t.clone())),
            Err(_) => {
                warnings.push(format!("Некорректный regex в -ot: {r}"));
                None
            }
        })
        .collect();

    let mut dev = vec![Usage::default(); ndev];
    let mut layers_on = vec![0u64; ndev];
    let mut cpu = Usage::default();
    for (i, d) in layer_dev.iter().enumerate() {
        if let Some(d) = d {
            layers_on[*d] += 1;
        } else {
            let _ = i;
        }
    }

    for t in &tensors {
        let name = t.n.as_str();
        let mut target: Option<usize> = if let Some(l) = layer_of(name) {
            layer_dev.get(l).copied().flatten()
        } else if name.starts_with("token_embd") || name.starts_with("tok_embd") || name.contains("position_embd") {
            None
        } else if name.starts_with("output") || name.starts_with("lm_head") || name.starts_with("cls") {
            output_dev
        } else {
            output_dev
        };
        if name.contains("_exps") {
            if let Some(l) = layer_of(name) {
                if inp.cpu_moe || (l as u64) < inp.n_cpu_moe {
                    target = None;
                }
            }
        }
        for (re, buft) in &ot {
            if re.is_match(name) {
                let b = buft.to_uppercase();
                target = if b.starts_with("CPU") {
                    None
                } else {
                    inp.devices.iter().position(|d| d.id.eq_ignore_ascii_case(buft)).or(target)
                };
                break;
            }
        }
        match target {
            Some(d) => dev[d].weights += t.b,
            None => cpu.weights += t.b,
        }
    }

    // ---- KV cache
    let (kb, kexact) = cache_bits(&inp.cache_k, &inp.custom_cache_bits);
    let (vb, vexact) = cache_bits(&inp.cache_v, &inp.custom_cache_bits);
    if !kexact || !vexact {
        warnings.push("Неизвестный тип KV-кэша — размер оценён приблизительно (настройте в Настройках → типы кэша)".into());
    }
    if !inp.flash_attn && (inp.cache_v != "f16" && inp.cache_v != "bf16" && inp.cache_v != "f32" && !inp.cache_v.is_empty()) {
        warnings.push("Квантованный V-кэш требует Flash Attention".into());
    }
    let parallel = inp.parallel.max(1);
    let mut kv_per_token = 0.0;
    for i in 0..nl {
        let hkv = info.n_head_kv.get(i).copied().unwrap_or(info.n_head);
        let (k_el, v_el) = if let Some(r) = info.kv_lora_rank {
            let rope = info.rope_dim.unwrap_or(64);
            (r + rope, if inp.flash_attn { 0 } else { r })
        } else {
            (hkv * info.head_dim_k, hkv * info.head_dim_v)
        };
        let per_tok = (k_el as f64 * kb + v_el as f64 * vb) / 8.0;
        let cells = if is_swa_layer(info, i) && !inp.swa_full {
            (info.sliding_window.unwrap_or(n_ctx) * parallel + ubatch).min(n_ctx)
        } else {
            n_ctx
        };
        let mut bytes = (per_tok * cells as f64) as u64;
        if hkv == 0 && info.ssm_d_state.is_some() {
            // recurrent state per sequence (f32)
            let di = info.ssm_d_inner.unwrap_or(info.n_embd * 2);
            let ds = info.ssm_d_state.unwrap_or(16);
            let dc = info.ssm_d_conv.unwrap_or(4);
            let ng = info.ssm_n_group.unwrap_or(1);
            bytes = ((dc.saturating_sub(1)) * (di + 2 * ng * ds) + ds * di) * 4 * parallel;
        } else {
            kv_per_token += per_tok;
        }
        match (layer_dev[i], inp.no_kv_offload) {
            (Some(d), false) => dev[d].kv += bytes,
            _ => cpu.kv += bytes,
        }
    }

    // ---- compute buffers (heuristic, calibrated by compute_coef)
    let coef = if inp.compute_coef > 0.0 { inp.compute_coef } else { 1.0 };
    let f = 4u64;
    let logits = ubatch * info.n_vocab.max(32000) * f;
    let kq = if inp.flash_attn { ubatch * info.n_head * 256 * f } else { ubatch * n_ctx * info.n_head.max(1) * f };
    let act = ubatch * (info.n_embd * 4 + info.n_ff.max(info.n_embd * 3) * 2) * f;
    for d in 0..ndev {
        if layers_on[d] == 0 && output_dev != Some(d) && dev[d].weights == 0 {
            continue;
        }
        let mut c = act + if layers_on[d] > 0 { kq } else { 0 };
        if output_dev == Some(d) {
            c = (act + logits).max(c);
        }
        dev[d].compute = (c as f64 * coef) as u64;
        dev[d].overhead = inp.gpu_overhead_bytes;
    }
    let cpu_compute = if output_dev.is_none() { act + logits + kq.min(act * 4) } else { act / 2 + ubatch * info.n_embd * f };
    cpu.compute = cpu_compute;
    if let Some(m) = inp.devices.get(inp.main_gpu.min(ndev.saturating_sub(1))) {
        let _ = m;
        if ndev > 0 {
            let mg = inp.main_gpu.min(ndev - 1);
            dev[mg].extra += inp.extra_gpu_bytes;
        }
    } else {
        cpu.extra += inp.extra_gpu_bytes;
    }

    let fit_of = |need: u64, free: u64, margin: u64| -> Fit {
        let avail = free.saturating_sub(margin);
        if need as f64 <= avail as f64 * 0.9 {
            Fit::Good
        } else if need <= avail {
            Fit::Tight
        } else {
            Fit::No
        }
    };
    let devices: Vec<DeviceEstimate> = inp
        .devices
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let u = dev[i].clone();
            let need = u.total();
            DeviceEstimate {
                id: d.id.clone(),
                name: d.name.clone(),
                total: d.total_bytes,
                free: d.free_bytes,
                layers: layers_on[i] + if output_dev == Some(i) { 1 } else { 0 },
                fit: if need == 0 { Fit::Good } else { fit_of(need, d.free_bytes, inp.margin_bytes) },
                usage: u,
            }
        })
        .collect();
    let cpu_total = cpu.total();
    let cpu_fit = if inp.ram_free_bytes == 0 { Fit::Good } else { fit_of(cpu_total, inp.ram_free_bytes, 0) };
    if !inp.mmap && cpu.weights > 0 {
        // without mmap weights are copied into RAM and the page cache may hold another copy while loading
    }
    let worst = devices
        .iter()
        .map(|d| d.fit.clone())
        .chain(std::iter::once(cpu_fit.clone()))
        .max_by_key(|f| match f {
            Fit::Good => 0,
            Fit::Tight => 1,
            Fit::No => 2,
        })
        .unwrap_or(Fit::Good);
    Estimate {
        gpu_layers: ngl as u64,
        n_layers: nl as u64,
        output_on_gpu: output_dev.is_some(),
        kv_per_token_bytes: kv_per_token,
        devices,
        cpu,
        cpu_total,
        ram_free: inp.ram_free_bytes,
        cpu_fit,
        fit: worst,
        warnings,
    }
}

fn fits(e: &Estimate) -> bool {
    e.fit != Fit::No
}

/// Max number of GPU layers that fits.
pub fn autofit_ngl(info: &ModelInfo, t: &[TensorLite], size: u64, inp: &MemInput) -> i64 {
    let (mut lo, mut hi) = (0i64, info.n_layers as i64 + 1);
    let check = |n: i64| {
        let mut i = inp.clone();
        i.n_gpu_layers = Some(n);
        fits(&estimate(info, t, size, &i))
    };
    if check(hi) {
        return hi;
    }
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if check(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// Max context (multiple of 256) that fits with current settings.
pub fn autofit_ctx(info: &ModelInfo, t: &[TensorLite], size: u64, inp: &MemInput) -> u64 {
    let max = info.ctx_max.unwrap_or(131072).max(512);
    let check = |c: u64| {
        let mut i = inp.clone();
        i.n_ctx = c;
        fits(&estimate(info, t, size, &i))
    };
    if check(max) {
        return max;
    }
    let (mut lo, mut hi) = (0u64, max / 256);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if check(mid * 256) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo * 256
}

/// Min `--n-cpu-moe` that makes a MoE model fit with all layers on GPU.
pub fn autofit_ncpumoe(info: &ModelInfo, t: &[TensorLite], size: u64, inp: &MemInput) -> Option<u64> {
    if info.n_expert == 0 {
        return None;
    }
    let mut base = inp.clone();
    base.n_gpu_layers = None;
    base.cpu_moe = false;
    for n in 0..=info.n_layers {
        let mut i = base.clone();
        i.n_cpu_moe = n;
        if fits(&estimate(info, t, size, &i)) {
            return Some(n);
        }
    }
    None
}

/// Tensor split proportional to free memory.
pub fn autofit_split(inp: &MemInput) -> Vec<f64> {
    let s: f64 = inp.devices.iter().map(|d| d.free_bytes as f64).sum::<f64>().max(1.0);
    inp.devices.iter().map(|d| ((d.free_bytes as f64 / s) * 100.0).round() / 100.0).collect()
}

pub fn mib(b: u64) -> f64 {
    b as f64 / MIB
}

#[cfg(test)]
mod tests {
    use super::*;

    fn llama8b() -> (ModelInfo, Vec<TensorLite>) {
        let info = ModelInfo {
            arch: "llama".into(),
            n_layers: 32,
            n_embd: 4096,
            n_ff: 14336,
            n_head: 32,
            n_head_kv: vec![8; 32],
            head_dim_k: 128,
            head_dim_v: 128,
            n_vocab: 128256,
            ctx_max: Some(131072),
            ..Default::default()
        };
        let mut t = vec![TensorLite { n: "token_embd.weight".into(), b: 280 << 20 }, TensorLite { n: "output.weight".into(), b: 410 << 20 }];
        for i in 0..32 {
            t.push(TensorLite { n: format!("blk.{i}.attn_q.weight"), b: 140 << 20 });
        }
        (info, t)
    }

    fn inp(free_gib: u64) -> MemInput {
        MemInput {
            n_ctx: 8192,
            n_batch: 2048,
            n_ubatch: 512,
            cache_k: "f16".into(),
            cache_v: "f16".into(),
            flash_attn: true,
            parallel: 1,
            devices: vec![DeviceSlot { id: "CUDA0".into(), name: "GPU".into(), total_bytes: 24 << 30, free_bytes: free_gib << 30 }],
            gpu_overhead_bytes: 400 << 20,
            margin_bytes: 256 << 20,
            mmap: true,
            ..Default::default()
        }
    }

    #[test]
    fn kv_size_llama8b() {
        let (info, t) = llama8b();
        let e = estimate(&info, &t, 0, &inp(24));
        // 8192 ctx * 32 layers * 8 heads * 128 * 2 (k+v) * 2 bytes = 1 GiB
        assert_eq!(e.devices[0].usage.kv, 1 << 30);
        assert_eq!(e.devices[0].layers, 33);
        assert!(e.output_on_gpu);
        assert_eq!(e.cpu.weights, 280 << 20);
        assert_eq!(e.fit, Fit::Good);
    }

    #[test]
    fn autofit() {
        let (info, t) = llama8b();
        let small = inp(3);
        let n = autofit_ngl(&info, &t, 0, &small);
        assert!(n > 0 && n < 33, "ngl {n}");
        let c = autofit_ctx(&info, &t, 0, &inp(8));
        assert!(c >= 8192 && c < 131072, "ctx {c}");
        let mut q8 = inp(8);
        q8.cache_k = "q8_0".into();
        q8.cache_v = "q8_0".into();
        assert!(autofit_ctx(&info, &t, 0, &q8) > c);
    }

    #[test]
    fn split_two_gpus() {
        let (info, t) = llama8b();
        let mut i = inp(24);
        i.devices.push(DeviceSlot { id: "CUDA1".into(), name: "GPU2".into(), total_bytes: 12 << 30, free_bytes: 12 << 30 });
        i.tensor_split = vec![1.0, 1.0];
        let e = estimate(&info, &t, 0, &i);
        assert_eq!(e.devices[0].layers + e.devices[1].layers, 33);
        assert!(e.devices[1].usage.compute > 0);
    }

    #[test]
    fn cpu_moe_override() {
        let (mut info, mut t) = llama8b();
        info.n_expert = 8;
        for i in 0..32 {
            t.push(TensorLite { n: format!("blk.{i}.ffn_up_exps.weight"), b: 100 << 20 });
        }
        let mut i = inp(24);
        i.n_cpu_moe = 10;
        let e = estimate(&info, &t, 0, &i);
        assert_eq!(e.cpu.weights, (280 << 20) + 10 * (100 << 20));
        i.n_cpu_moe = 0;
        i.override_tensor = vec![(r"blk\.(0|1)\.ffn_.*_exps".into(), "CPU".into())];
        let e = estimate(&info, &t, 0, &i);
        assert_eq!(e.cpu.weights, (280 << 20) + 2 * (100 << 20));
    }
}
