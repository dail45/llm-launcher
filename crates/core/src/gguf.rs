//! Minimal GGUF header parser: reads metadata + tensor infos without loading weights.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufReader, Read};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum MetaValue {
    Int(i64),
    UInt(u64),
    Float(f64),
    Bool(bool),
    Str(String),
    /// Arrays of scalars are kept (if short); long arrays keep only their length.
    Array(Vec<MetaValue>),
    ArrayLen { array_len: u64, elem_type: u32 },
}

impl MetaValue {
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            MetaValue::Int(v) if *v >= 0 => Some(*v as u64),
            MetaValue::UInt(v) => Some(*v),
            MetaValue::Float(f) if *f >= 0.0 => Some(*f as u64),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            MetaValue::Int(v) => Some(*v as f64),
            MetaValue::UInt(v) => Some(*v as f64),
            MetaValue::Float(f) => Some(*f),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            MetaValue::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            MetaValue::Bool(b) => Some(*b),
            _ => self.as_u64().map(|v| v != 0),
        }
    }
    pub fn array_len(&self) -> Option<u64> {
        match self {
            MetaValue::Array(v) => Some(v.len() as u64),
            MetaValue::ArrayLen { array_len, .. } => Some(*array_len),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TensorInfo {
    pub name: String,
    pub dims: Vec<u64>,
    pub ggml_type: u32,
    pub offset: u64,
    /// Size in bytes (computed from offsets, or from type table as fallback).
    pub bytes: u64,
}

impl TensorInfo {
    pub fn n_elements(&self) -> u64 {
        self.dims.iter().product::<u64>().max(1)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GgufFile {
    pub version: u32,
    pub metadata: BTreeMap<String, MetaValue>,
    pub tensors: Vec<TensorInfo>,
    pub data_offset: u64,
}

/// Block size & bytes per block for known ggml types.
pub fn ggml_type_size(t: u32) -> Option<(u64, u64)> {
    Some(match t {
        0 => (1, 4),
        1 => (1, 2),
        2 => (32, 18),
        3 => (32, 20),
        6 => (32, 22),
        7 => (32, 24),
        8 => (32, 34),
        9 => (32, 36),
        10 => (256, 84),
        11 => (256, 110),
        12 => (256, 144),
        13 => (256, 176),
        14 => (256, 210),
        15 => (256, 292),
        16 => (256, 66),
        17 => (256, 74),
        18 => (256, 98),
        19 => (256, 50),
        20 => (32, 18),
        21 => (256, 110),
        22 => (256, 82),
        23 => (256, 136),
        24 => (1, 1),
        25 => (1, 2),
        26 => (1, 4),
        27 => (1, 8),
        28 => (1, 8),
        29 => (256, 56),
        30 => (1, 2),
        34 => (256, 54),
        35 => (256, 66),
        39 => (32, 17),
        _ => return None,
    })
}

pub fn ggml_type_name(t: u32) -> String {
    match t {
        0 => "F32", 1 => "F16", 2 => "Q4_0", 3 => "Q4_1", 6 => "Q5_0", 7 => "Q5_1", 8 => "Q8_0",
        9 => "Q8_1", 10 => "Q2_K", 11 => "Q3_K", 12 => "Q4_K", 13 => "Q5_K", 14 => "Q6_K",
        15 => "Q8_K", 16 => "IQ2_XXS", 17 => "IQ2_XS", 18 => "IQ3_XXS", 19 => "IQ1_S",
        20 => "IQ4_NL", 21 => "IQ3_S", 22 => "IQ2_S", 23 => "IQ4_XS", 24 => "I8", 25 => "I16",
        26 => "I32", 27 => "I64", 28 => "F64", 29 => "IQ1_M", 30 => "BF16", 34 => "TQ1_0",
        35 => "TQ2_0", 39 => "MXFP4",
        _ => return format!("TYPE_{t}"),
    }
    .to_string()
}

/// llama_ftype (general.file_type) → human readable name.
pub fn file_type_name(ft: u64) -> String {
    match ft {
        0 => "F32", 1 => "F16", 2 => "Q4_0", 3 => "Q4_1", 7 => "Q8_0", 8 => "Q5_0", 9 => "Q5_1",
        10 => "Q2_K", 11 => "Q3_K_S", 12 => "Q3_K_M", 13 => "Q3_K_L", 14 => "Q4_K_S",
        15 => "Q4_K_M", 16 => "Q5_K_S", 17 => "Q5_K_M", 18 => "Q6_K", 19 => "IQ2_XXS",
        20 => "IQ2_XS", 21 => "Q2_K_S", 22 => "IQ3_XS", 23 => "IQ3_XXS", 24 => "IQ1_S",
        25 => "IQ4_NL", 26 => "IQ3_S", 27 => "IQ3_M", 28 => "IQ2_S", 29 => "IQ2_M",
        30 => "IQ4_XS", 31 => "IQ1_M", 32 => "BF16", 36 => "TQ1_0", 37 => "TQ2_0", 38 => "MXFP4_MOE",
        _ => return format!("FTYPE_{ft}"),
    }
    .to_string()
}

const MAX_KEPT_ARRAY: u64 = 64;
const MAX_STRING: u64 = 1 << 24;

struct R<T: Read> {
    r: T,
    pos: u64,
}

impl<T: Read> R<T> {
    fn bytes(&mut self, n: usize) -> Result<Vec<u8>> {
        let mut b = vec![0u8; n];
        self.r.read_exact(&mut b).context("unexpected end of GGUF header")?;
        self.pos += n as u64;
        Ok(b)
    }
    fn skip(&mut self, n: u64) -> Result<()> {
        let copied = std::io::copy(&mut (&mut self.r).take(n), &mut std::io::sink())?;
        if copied != n {
            bail!("unexpected end of GGUF header");
        }
        self.pos += n;
        Ok(())
    }
    fn u8(&mut self) -> Result<u8> { Ok(self.bytes(1)?[0]) }
    fn u16(&mut self) -> Result<u16> { Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap())) }
    fn u32(&mut self) -> Result<u32> { Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap())) }
    fn u64(&mut self) -> Result<u64> { Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap())) }
    fn string(&mut self) -> Result<String> {
        let n = self.u64()?;
        if n > MAX_STRING {
            bail!("GGUF string too long ({n})");
        }
        Ok(String::from_utf8_lossy(&self.bytes(n as usize)?).into_owned())
    }
    fn scalar(&mut self, t: u32) -> Result<MetaValue> {
        Ok(match t {
            0 => MetaValue::UInt(self.u8()? as u64),
            1 => MetaValue::Int(self.u8()? as i8 as i64),
            2 => MetaValue::UInt(self.u16()? as u64),
            3 => MetaValue::Int(self.u16()? as i16 as i64),
            4 => MetaValue::UInt(self.u32()? as u64),
            5 => MetaValue::Int(self.u32()? as i32 as i64),
            6 => MetaValue::Float(f32::from_bits(self.u32()?) as f64),
            7 => MetaValue::Bool(self.u8()? != 0),
            8 => MetaValue::Str(self.string()?),
            10 => MetaValue::UInt(self.u64()?),
            11 => MetaValue::Int(self.u64()? as i64),
            12 => MetaValue::Float(f64::from_bits(self.u64()?)),
            _ => bail!("unknown GGUF value type {t}"),
        })
    }
    fn scalar_size(t: u32) -> Option<u64> {
        Some(match t {
            0 | 1 | 7 => 1,
            2 | 3 => 2,
            4 | 5 | 6 => 4,
            10 | 11 | 12 => 8,
            _ => return None,
        })
    }
    fn value(&mut self, t: u32) -> Result<MetaValue> {
        if t != 9 {
            return self.scalar(t);
        }
        let et = self.u32()?;
        let n = self.u64()?;
        if n <= MAX_KEPT_ARRAY && et != 9 {
            let mut v = Vec::with_capacity(n as usize);
            for _ in 0..n {
                v.push(self.scalar(et)?);
            }
            return Ok(MetaValue::Array(v));
        }
        if let Some(sz) = Self::scalar_size(et) {
            self.skip(sz * n)?;
        } else if et == 8 {
            for _ in 0..n {
                let l = self.u64()?;
                self.skip(l)?;
            }
        } else if et == 9 {
            for _ in 0..n {
                let _ = self.value(9)?;
            }
        } else {
            bail!("unknown GGUF array element type {et}");
        }
        Ok(MetaValue::ArrayLen { array_len: n, elem_type: et })
    }
}

/// Parse a GGUF header from a reader. `total_size` (file size) is used to size the last tensor.
pub fn parse<T: Read>(reader: T, total_size: Option<u64>) -> Result<GgufFile> {
    let mut r = R { r: reader, pos: 0 };
    let magic = r.bytes(4)?;
    if &magic != b"GGUF" {
        bail!("not a GGUF file");
    }
    let version = r.u32()?;
    if !(2..=3).contains(&version) {
        bail!("unsupported GGUF version {version}");
    }
    let n_tensors = r.u64()?;
    let n_kv = r.u64()?;
    if n_tensors > 1_000_000 || n_kv > 1_000_000 {
        bail!("GGUF header looks corrupted");
    }
    let mut metadata = BTreeMap::new();
    for _ in 0..n_kv {
        let key = r.string()?;
        let t = r.u32()?;
        let v = r.value(t)?;
        metadata.insert(key, v);
    }
    let mut tensors = Vec::with_capacity(n_tensors as usize);
    for _ in 0..n_tensors {
        let name = r.string()?;
        let nd = r.u32()?;
        if nd > 8 {
            bail!("tensor {name} has too many dims");
        }
        let mut dims = Vec::with_capacity(nd as usize);
        for _ in 0..nd {
            dims.push(r.u64()?);
        }
        let ggml_type = r.u32()?;
        let offset = r.u64()?;
        tensors.push(TensorInfo { name, dims, ggml_type, offset, bytes: 0 });
    }
    let align = metadata.get("general.alignment").and_then(|v| v.as_u64()).unwrap_or(32).max(1);
    let data_offset = r.pos.div_ceil(align) * align;

    // sizes: prefer type table, fall back to offset deltas (forks may add new types)
    let mut order: Vec<usize> = (0..tensors.len()).collect();
    order.sort_by_key(|&i| tensors[i].offset);
    for (k, &i) in order.iter().enumerate() {
        let by_type = ggml_type_size(tensors[i].ggml_type).map(|(bs, ts)| tensors[i].n_elements().div_ceil(bs) * ts);
        let by_delta = if k + 1 < order.len() {
            Some(tensors[order[k + 1]].offset.saturating_sub(tensors[i].offset))
        } else {
            total_size.map(|s| s.saturating_sub(data_offset + tensors[i].offset))
        };
        tensors[i].bytes = by_type.or(by_delta).unwrap_or(0);
    }
    Ok(GgufFile { version, metadata, tensors, data_offset })
}

pub fn parse_file(path: &Path) -> Result<GgufFile> {
    let f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let size = f.metadata()?.len();
    parse(BufReader::with_capacity(1 << 20, f), Some(size))
}

/// Summary of the model info relevant for the launcher and the memory calculator.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelInfo {
    pub arch: String,
    pub name: Option<String>,
    pub size_label: Option<String>,
    pub file_type: Option<String>,
    pub n_params: u64,
    pub ctx_max: Option<u64>,
    pub n_layers: u64,
    pub n_embd: u64,
    pub n_ff: u64,
    pub n_head: u64,
    /// per-layer KV head count (expanded to n_layers)
    pub n_head_kv: Vec<u64>,
    pub head_dim_k: u64,
    pub head_dim_v: u64,
    pub n_vocab: u64,
    pub n_expert: u64,
    pub n_expert_used: u64,
    pub sliding_window: Option<u64>,
    pub swa_pattern: Option<u64>,
    pub kv_lora_rank: Option<u64>,
    pub rope_dim: Option<u64>,
    pub ssm_d_inner: Option<u64>,
    pub ssm_d_state: Option<u64>,
    pub ssm_d_conv: Option<u64>,
    pub ssm_n_group: Option<u64>,
    pub chat_template: Option<String>,
    pub is_mmproj: bool,
    pub split_count: Option<u64>,
    pub tensor_bytes: u64,
}

impl GgufFile {
    pub fn get(&self, k: &str) -> Option<&MetaValue> {
        self.metadata.get(k)
    }

    pub fn info(&self) -> ModelInfo {
        let arch = self.get("general.architecture").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        let a = |k: &str| self.get(&format!("{arch}.{k}"));
        let au = |k: &str| a(k).and_then(|v| v.as_u64());
        let n_layers = au("block_count").unwrap_or(0);
        let n_embd = au("embedding_length").unwrap_or(0);
        let n_head = match a("attention.head_count") {
            Some(MetaValue::Array(v)) => v.iter().filter_map(|x| x.as_u64()).max().unwrap_or(0),
            Some(v) => v.as_u64().unwrap_or(0),
            None => 0,
        };
        let n_head_kv = match a("attention.head_count_kv") {
            Some(MetaValue::Array(v)) => {
                let mut out: Vec<u64> = v.iter().map(|x| x.as_u64().unwrap_or(0)).collect();
                out.resize(n_layers as usize, *out.last().unwrap_or(&n_head));
                out
            }
            Some(v) => vec![v.as_u64().unwrap_or(n_head); n_layers as usize],
            None => vec![n_head; n_layers as usize],
        };
        let head_dim = if n_head > 0 { n_embd / n_head } else { 0 };
        let n_ff = match a("feed_forward_length") {
            Some(MetaValue::Array(v)) => v.iter().filter_map(|x| x.as_u64()).max().unwrap_or(0),
            Some(v) => v.as_u64().unwrap_or(0),
            None => 0,
        };
        let n_vocab = au("vocab_size")
            .or_else(|| self.get("tokenizer.ggml.tokens").and_then(|v| v.array_len()))
            .unwrap_or(0);
        let n_params = self.tensors.iter().map(|t| t.n_elements()).sum();
        let tensor_bytes = self.tensors.iter().map(|t| t.bytes).sum();
        let is_mmproj = arch == "clip" || self.get("clip.has_vision_encoder").is_some();
        ModelInfo {
            name: self.get("general.name").and_then(|v| v.as_str()).map(String::from),
            size_label: self.get("general.size_label").and_then(|v| v.as_str()).map(String::from),
            file_type: self.get("general.file_type").and_then(|v| v.as_u64()).map(file_type_name),
            n_params,
            ctx_max: au("context_length"),
            n_layers,
            n_embd,
            n_ff,
            n_head,
            n_head_kv,
            head_dim_k: au("attention.key_length").unwrap_or(head_dim),
            head_dim_v: au("attention.value_length").unwrap_or(head_dim),
            n_vocab,
            n_expert: au("expert_count").unwrap_or(0),
            n_expert_used: au("expert_used_count").unwrap_or(0),
            sliding_window: au("attention.sliding_window"),
            swa_pattern: au("attention.sliding_window_pattern"),
            kv_lora_rank: au("attention.kv_lora_rank"),
            rope_dim: au("rope.dimension_count"),
            ssm_d_inner: au("ssm.inner_size"),
            ssm_d_state: au("ssm.state_size"),
            ssm_d_conv: au("ssm.conv_kernel"),
            ssm_n_group: au("ssm.group_count"),
            chat_template: self.get("tokenizer.chat_template").and_then(|v| v.as_str()).map(String::from),
            is_mmproj,
            split_count: self.get("split.count").and_then(|v| v.as_u64()),
            tensor_bytes,
            arch,
        }
    }
}

/// Merge several split parts into one logical file (tensors concatenated).
pub fn merge_splits(mut parts: Vec<GgufFile>) -> Option<GgufFile> {
    if parts.is_empty() {
        return None;
    }
    let mut first = parts.remove(0);
    for p in parts {
        first.tensors.extend(p.tensors);
    }
    Some(first)
}

// ---------- writer (used by tests and tools) ----------
pub mod writer {
    use super::*;
    pub enum W<'a> {
        U32(u32),
        U64(u64),
        F32(f32),
        Str(&'a str),
        Bool(bool),
        ArrU32(Vec<u32>),
        ArrStr(Vec<&'a str>),
    }
    fn s(out: &mut Vec<u8>, v: &str) {
        out.extend((v.len() as u64).to_le_bytes());
        out.extend(v.as_bytes());
    }
    /// tensors: (name, dims, type)
    pub fn build(kv: &[(&str, W)], tensors: &[(&str, Vec<u64>, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(b"GGUF");
        out.extend(3u32.to_le_bytes());
        out.extend((tensors.len() as u64).to_le_bytes());
        out.extend((kv.len() as u64).to_le_bytes());
        for (k, v) in kv {
            s(&mut out, k);
            match v {
                W::U32(x) => { out.extend(4u32.to_le_bytes()); out.extend(x.to_le_bytes()); }
                W::U64(x) => { out.extend(10u32.to_le_bytes()); out.extend(x.to_le_bytes()); }
                W::F32(x) => { out.extend(6u32.to_le_bytes()); out.extend(x.to_le_bytes()); }
                W::Bool(x) => { out.extend(7u32.to_le_bytes()); out.push(*x as u8); }
                W::Str(x) => { out.extend(8u32.to_le_bytes()); s(&mut out, x); }
                W::ArrU32(a) => {
                    out.extend(9u32.to_le_bytes()); out.extend(4u32.to_le_bytes());
                    out.extend((a.len() as u64).to_le_bytes());
                    for x in a { out.extend(x.to_le_bytes()); }
                }
                W::ArrStr(a) => {
                    out.extend(9u32.to_le_bytes()); out.extend(8u32.to_le_bytes());
                    out.extend((a.len() as u64).to_le_bytes());
                    for x in a { s(&mut out, x); }
                }
            }
        }
        let mut offset = 0u64;
        let mut sizes = vec![];
        for (name, dims, t) in tensors {
            s(&mut out, name);
            out.extend((dims.len() as u32).to_le_bytes());
            for d in dims { out.extend(d.to_le_bytes()); }
            out.extend(t.to_le_bytes());
            out.extend(offset.to_le_bytes());
            let n: u64 = dims.iter().product();
            let (bs, ts) = ggml_type_size(*t).unwrap();
            let sz = n.div_ceil(bs) * ts;
            sizes.push(sz);
            offset += sz.div_ceil(32) * 32;
        }
        while out.len() % 32 != 0 { out.push(0); }
        for sz in sizes {
            let padded = sz.div_ceil(32) * 32;
            out.extend(std::iter::repeat_n(0u8, padded as usize));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::writer::*;
    use super::*;

    #[test]
    fn parse_synthetic() {
        let toks: Vec<&str> = (0..100).map(|_| "tok").collect();
        let data = build(
            &[
                ("general.architecture", W::Str("llama")),
                ("general.name", W::Str("Test")),
                ("general.file_type", W::U32(15)),
                ("llama.block_count", W::U32(2)),
                ("llama.embedding_length", W::U32(64)),
                ("llama.attention.head_count", W::U32(8)),
                ("llama.attention.head_count_kv", W::U32(2)),
                ("llama.context_length", W::U32(4096)),
                ("tokenizer.ggml.tokens", W::ArrStr(toks)),
                ("x.bool", W::Bool(true)),
                ("x.f", W::F32(1.5)),
                ("x.arr", W::ArrU32(vec![1, 2, 3])),
            ],
            &[
                ("token_embd.weight", vec![64, 100], 1),
                ("blk.0.attn_q.weight", vec![64, 64], 8),
                ("blk.1.attn_q.weight", vec![64, 64], 12),
            ],
        );
        let g = parse(&data[..], Some(data.len() as u64)).unwrap();
        let i = g.info();
        assert_eq!(i.arch, "llama");
        assert_eq!(i.n_layers, 2);
        assert_eq!(i.n_head_kv, vec![2, 2]);
        assert_eq!(i.head_dim_k, 8);
        assert_eq!(i.n_vocab, 100);
        assert_eq!(i.file_type.as_deref(), Some("Q4_K_M"));
        assert_eq!(g.tensors[0].bytes, 64 * 100 * 2);
        assert_eq!(g.tensors[1].bytes, 64 * 64 / 32 * 34);
        assert_eq!(i.n_params, 6400 + 4096 * 2);
    }
}
