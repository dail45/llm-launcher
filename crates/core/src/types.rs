//! Persistent domain types.
use crate::gguf::ModelInfo;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}
pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Cuda,
    Rocm,
    Vulkan,
    Cpu,
    Sycl,
    Metal,
    #[default]
    Other,
}

impl BackendKind {
    pub fn guess(s: &str) -> Self {
        let s = s.to_lowercase();
        if s.contains("cuda") || s.contains("cu12") || s.contains("cu11") || s.contains("cu13") {
            Self::Cuda
        } else if s.contains("hip") || s.contains("rocm") || s.contains("radeon") {
            Self::Rocm
        } else if s.contains("vulkan") || s.contains("vk") {
            Self::Vulkan
        } else if s.contains("sycl") {
            Self::Sycl
        } else if s.contains("metal") || s.contains("macos") {
            Self::Metal
        } else if s.contains("cpu") || s.contains("avx") || s.contains("noavx") {
            Self::Cpu
        } else {
            Self::Other
        }
    }
    /// Env var used to restrict visible devices.
    pub fn visible_devices_env(&self) -> Option<&'static str> {
        match self {
            Self::Cuda => Some("CUDA_VISIBLE_DEVICES"),
            Self::Rocm => Some("HIP_VISIBLE_DEVICES"),
            Self::Vulkan => Some("GGML_VK_VISIBLE_DEVICES"),
            Self::Sycl => Some("ONEAPI_DEVICE_SELECTOR"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Backend {
    pub id: String,
    pub name: String,
    pub kind: BackendKind,
    /// Folder containing llama-server(.exe)
    pub dir: String,
    pub server_bin: String,
    pub bench_bin: Option<String>,
    pub source_repo: Option<String>,
    pub release_tag: Option<String>,
    pub version: Option<String>,
    pub commit: Option<String>,
    pub bin_hash: Option<String>,
    pub features: Vec<String>,
    pub devices: Vec<crate::help_parser::BackendDevice>,
    pub is_default: bool,
    pub color: Option<String>,
    pub notes: String,
    pub managed: bool,
    pub created_at: i64,
    pub scan_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ModelDir {
    pub id: String,
    pub path: String,
    pub recursive: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Model {
    pub id: String,
    pub alias: String,
    /// All parts (for split models), first = main file
    pub paths: Vec<String>,
    pub size: u64,
    pub file_name: String,
    pub dir_id: Option<String>,
    pub info: ModelInfo,
    pub mmproj_id: Option<String>,
    pub tags: Vec<String>,
    pub favorite: bool,
    pub notes: String,
    pub mtime: i64,
    pub added_at: i64,
    pub missing: bool,
    pub hf_repo: Option<String>,
    pub sha256: Option<String>,
    /// Raw metadata (without huge arrays) for the details view
    #[serde(default)]
    pub meta: BTreeMap<String, Value>,
    #[serde(default)]
    pub tensors: Vec<crate::memory::TensorLite>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub model_id: String,
    pub backend_id: String,
    /// param id (e.g. "--ctx-size") -> value (bool/number/string/array)
    pub args: BTreeMap<String, Value>,
    pub extra_args: String,
    pub env: BTreeMap<String, String>,
    /// backend device ids (e.g. ["CUDA0","CUDA1"]); empty = all
    pub devices: Vec<String>,
    pub mmproj_id: Option<String>,
    pub draft_model_id: Option<String>,
    pub lora_ids: Vec<String>,
    /// idle TTL seconds (None = global default, 0 = never)
    pub ttl: Option<u64>,
    pub pinned: bool,
    pub autostart: bool,
    pub required_features: Vec<String>,
    pub group: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub hotkey: Option<String>,
    pub order: i64,
    pub version: u64,
    pub notes: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AliasTarget {
    Preset,
    Group,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alias {
    /// alias name is the id
    pub name: String,
    pub target_type: AliasTarget,
    pub target_id: String,
    #[serde(default)]
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AliasGroup {
    pub id: String,
    pub name: String,
    pub preset_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    pub prefix: String,
    pub hash: String,
    /// allowed alias names, "*" = all
    pub scopes: Vec<String>,
    pub can_load: bool,
    pub can_manage: bool,
    pub expires_at: Option<i64>,
    pub last_used: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSession {
    pub id: String,
    pub refresh_hash: String,
    pub device: String,
    pub ip: String,
    pub created_at: i64,
    pub last_seen: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BenchRun {
    pub id: String,
    pub model_id: String,
    pub backend_id: String,
    pub params: BTreeMap<String, Value>,
    pub pp: Option<u64>,
    pub tg: Option<u64>,
    pub pp_ts: Option<f64>,
    pub tg_ts: Option<f64>,
    pub raw: Value,
    pub created_at: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GhSource {
    pub id: String,
    /// owner/repo
    pub repo: String,
    pub name: String,
    /// default asset name pattern (substring or regex)
    pub asset_pattern: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub ui_bind: String,
    pub ui_port: u16,
    pub router_bind: String,
    pub router_port: u16,
    pub internal_port_start: u16,
    pub internal_port_end: u16,
    pub default_ttl: u64,
    pub max_instances: usize,
    pub swap_mode: bool,
    pub load_timeout_secs: u64,
    pub vram_margin_mib: u64,
    pub gpu_overhead_mib: u64,
    pub poll_interval_ms: u64,
    pub download_dir: String,
    pub backends_dir: String,
    pub hf_token: String,
    pub hf_endpoint: String,
    pub github_token: String,
    pub max_parallel_downloads: usize,
    pub download_speed_limit_kbps: u64,
    pub local_no_auth: bool,
    pub router_require_key: bool,
    pub allowed_subnets: Vec<String>,
    pub trusted_proxies: Vec<String>,
    pub cors_origins: Vec<String>,
    pub log_request_bodies: bool,
    pub auto_restart: bool,
    pub max_restart_attempts: u32,
    pub language: String,
    pub theme: String,
    pub access_ttl_secs: i64,
    pub refresh_ttl_days: i64,
    /// custom KV cache type sizes, bits per element (for fork types like turbo3)
    pub custom_cache_bits: BTreeMap<String, f64>,
    pub check_updates: bool,
    pub start_minimized: bool,
    /// how long per-request token usage is kept for analytics (0 = forever)
    pub usage_retention_days: u32,
    /// game mode: unload models while a game / heavy GPU app runs
    pub game_mode: bool,
    /// detect games automatically by GPU load of other processes
    pub game_auto_detect: bool,
    /// explicit game executables (e.g. "eldenring.exe")
    pub game_processes: Vec<String>,
    /// never treat these as games (in addition to the built-in list)
    pub game_ignore: Vec<String>,
    /// auto-detect thresholds
    pub game_gpu_util: u32,
    pub game_vram_mib: u64,
    pub game_unload_pinned: bool,
    /// the router does not load models while the mode is on
    pub game_block_loads: bool,
    /// start the unloaded presets again when the game exits
    pub game_restore: bool,
    /// seconds without a game before the mode turns off
    pub game_grace_secs: u64,
    /// optional HTTPS listener for the UI/API (needed for installing the PWA on phones)
    pub https_enabled: bool,
    pub https_port: u16,
    /// PEM certificate chain and private key
    pub https_cert: String,
    pub https_key: String,
}

impl Default for Settings {
    fn default() -> Self {
        let base = crate::paths::data_dir();
        Self {
            ui_bind: "127.0.0.1".into(),
            ui_port: 8090,
            router_bind: "127.0.0.1".into(),
            router_port: 8080,
            internal_port_start: 51000,
            internal_port_end: 51999,
            default_ttl: 600,
            max_instances: 4,
            swap_mode: false,
            load_timeout_secs: 600,
            vram_margin_mib: 512,
            gpu_overhead_mib: 400,
            poll_interval_ms: 1000,
            download_dir: base.join("models").to_string_lossy().into(),
            backends_dir: base.join("backends").to_string_lossy().into(),
            hf_token: String::new(),
            hf_endpoint: "https://huggingface.co".into(),
            github_token: String::new(),
            max_parallel_downloads: 2,
            download_speed_limit_kbps: 0,
            local_no_auth: true,
            router_require_key: true,
            allowed_subnets: vec![],
            trusted_proxies: vec![],
            cors_origins: vec![],
            log_request_bodies: false,
            auto_restart: false,
            max_restart_attempts: 3,
            language: "ru".into(),
            theme: "dark".into(),
            access_ttl_secs: 15 * 60,
            refresh_ttl_days: 30,
            custom_cache_bits: BTreeMap::from([("turbo3".to_string(), 3.5)]),
            check_updates: true,
            start_minimized: false,
            usage_retention_days: 365,
            game_mode: true,
            game_auto_detect: true,
            game_processes: vec![],
            game_ignore: vec![],
            game_gpu_util: 60,
            game_vram_mib: 3072,
            game_unload_pinned: true,
            game_block_loads: true,
            game_restore: false,
            game_grace_secs: 20,
            https_enabled: false,
            https_port: 8443,
            https_cert: String::new(),
            https_key: String::new(),
        }
    }
}

// serde(default) for forward compatibility with older stored settings
impl Settings {
    pub fn from_value(v: Value) -> Self {
        let mut base = serde_json::to_value(Settings::default()).unwrap();
        if let (Some(b), Some(o)) = (base.as_object_mut(), v.as_object()) {
            for (k, val) in o {
                b.insert(k.clone(), val.clone());
            }
        }
        serde_json::from_value(base).unwrap_or_default()
    }
}
