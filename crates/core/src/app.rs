//! Central application state and orchestration (start/stop presets, eviction, TTL, monitoring).
use crate::args::{self, LaunchCtx, LaunchSpec};
use crate::auth::Auth;
use crate::backends;
use crate::events::{Bus, Event};
use crate::gpu::{self, Monitor, SystemSnapshot};
use crate::help_parser::ParamSchema;
use crate::memory::{self, DeviceSlot, Estimate, Fit};
use crate::process::{InstState, Instance, InstanceInfo, ProcessManager};
use crate::store::Store;
use crate::types::*;
use anyhow::{anyhow, bail, Context, Result};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

pub struct App {
    pub store: Store,
    pub bus: Bus,
    pub pm: Arc<ProcessManager>,
    pub monitor: Arc<Monitor>,
    pub settings: RwLock<Settings>,
    pub auth: Auth,
    pub http: reqwest::Client,
    schemas: RwLock<HashMap<String, Arc<ParamSchema>>>,
    pub snapshot: RwLock<SystemSnapshot>,
    start_locks: parking_lot::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    pub downloads: crate::downloads::Downloads,
    pub jobs: crate::jobs::Jobs,
    restart_counts: parking_lot::Mutex<HashMap<String, (u32, i64)>>,
    pub game: crate::gamemode::GameMode,
    pub mcp: crate::mcp::Mcp,
}

pub type AppRef = Arc<App>;

#[derive(Debug, Clone, serde::Serialize)]
pub struct StartOutcome {
    pub instance: InstanceInfo,
    pub evicted: Vec<String>,
    pub warnings: Vec<String>,
}

impl App {
    pub fn open() -> Result<AppRef> {
        let dir = crate::paths::data_dir();
        std::fs::create_dir_all(&dir)?;
        let store = Store::open(&crate::paths::db_path())?;
        Self::with_store(store)
    }

    pub fn with_store(store: Store) -> Result<AppRef> {
        let settings = store.kv_get::<serde_json::Value>("settings")?.map(Settings::from_value).unwrap_or_default();
        let bus = Bus::default();
        let monitor = Arc::new(Monitor::new());
        let snap = monitor.snapshot();
        let app = Arc::new(Self {
            auth: Auth::new(store.clone())?,
            pm: Arc::new(ProcessManager::new(bus.clone())),
            downloads: crate::downloads::Downloads::new(),
            jobs: crate::jobs::Jobs::default(),
            store,
            bus,
            monitor,
            settings: RwLock::new(settings),
            http: reqwest::Client::builder().user_agent(concat!("llama-launcher/", env!("CARGO_PKG_VERSION"))).build()?,
            schemas: RwLock::new(HashMap::new()),
            snapshot: RwLock::new(snap),
            start_locks: Default::default(),
            restart_counts: Default::default(),
            game: Default::default(),
            mcp: Default::default(),
        });
        Ok(app)
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }
    pub fn save_settings(&self, s: Settings) -> Result<()> {
        self.store.kv_set("settings", &s)?;
        *self.settings.write() = s;
        Ok(())
    }

    // ---------- lookups
    pub fn backend(&self, id: &str) -> Result<Backend> {
        self.store.get("backends", id)?.ok_or_else(|| anyhow!("сборка {id} не найдена"))
    }
    pub fn model(&self, id: &str) -> Result<Model> {
        self.store.get("models", id)?.ok_or_else(|| anyhow!("модель {id} не найдена"))
    }
    pub fn preset(&self, id: &str) -> Result<Preset> {
        self.store.get("presets", id)?.ok_or_else(|| anyhow!("пресет {id} не найден"))
    }
    pub fn default_backend(&self) -> Option<Backend> {
        let all: Vec<Backend> = self.store.list("backends").ok()?;
        all.iter().find(|b| b.is_default).cloned().or_else(|| all.first().cloned())
    }

    pub fn schema(&self, backend: &Backend) -> Option<Arc<ParamSchema>> {
        let key = backend.bin_hash.clone()?;
        if let Some(s) = self.schemas.read().get(&key) {
            return Some(s.clone());
        }
        let s = Arc::new(backends::load_schema(backend)?);
        self.schemas.write().insert(key, s.clone());
        Some(s)
    }

    pub async fn rescan_backend(&self, id: &str) -> Result<Backend> {
        let b = self.backend(id)?;
        match backends::scan(b.clone()).await {
            Ok((nb, schema)) => {
                if let Some(h) = &nb.bin_hash {
                    self.schemas.write().insert(h.clone(), Arc::new(schema));
                }
                self.store.put("backends", &nb.id, &nb)?;
                Ok(nb)
            }
            Err(e) => {
                let mut b = b;
                b.scan_error = Some(e.to_string());
                self.store.put("backends", &b.id, &b)?;
                Err(e)
            }
        }
    }

    /// Primary alias for a preset (first alias marked primary, else first alias).
    pub fn primary_alias(&self, preset_id: &str) -> Option<String> {
        let al: Vec<Alias> = self.store.list("aliases").ok()?;
        let mine: Vec<&Alias> = al.iter().filter(|a| a.target_type == AliasTarget::Preset && a.target_id == preset_id).collect();
        mine.iter().find(|a| a.is_primary).or(mine.first()).map(|a| a.name.clone())
    }

    /// Resolve a model name from an API request to candidate preset ids (ordered).
    pub fn resolve(&self, name: &str) -> Option<(String, Vec<String>)> {
        if let Ok(Some(a)) = self.store.get::<Alias>("aliases", name) {
            return match a.target_type {
                AliasTarget::Preset => Some((a.name, vec![a.target_id])),
                AliasTarget::Group => {
                    let g: AliasGroup = self.store.get("alias_groups", &a.target_id).ok()??;
                    Some((a.name, g.preset_ids))
                }
            };
        }
        let presets: Vec<Preset> = self.store.list("presets").ok()?;
        presets.iter().find(|p| p.id == name || p.name == name).map(|p| (name.to_string(), vec![p.id.clone()]))
    }

    // ---------- memory
    pub fn device_slots(&self, backend: &Backend, selected: &[String]) -> Vec<DeviceSlot> {
        let snap = self.snapshot.read().clone();
        let mut taken = vec![];
        let mut out = vec![];
        for d in &backend.devices {
            if !selected.is_empty() && !selected.contains(&d.id) {
                continue;
            }
            if d.id.to_uppercase().starts_with("CPU") || d.id.to_uppercase().starts_with("BLAS") {
                continue;
            }
            let g = gpu::match_device(&d.name, d.total_mib, &snap.gpus, &taken);
            let (total, free) = match g {
                Some(g) => {
                    taken.push(g.index);
                    (g.total_bytes, g.free_bytes)
                }
                None => (d.total_mib.unwrap_or(0) << 20, d.free_mib.or(d.total_mib).unwrap_or(0) << 20),
            };
            out.push(DeviceSlot { id: d.id.clone(), name: d.name.clone(), total_bytes: total, free_bytes: free });
        }
        out
    }

    pub fn estimate_preset(&self, p: &Preset, running_self: bool) -> Result<Estimate> {
        let backend = self.backend(&p.backend_id)?;
        let model = self.model(&p.model_id)?;
        let schema = self.schema(&backend);
        let mut devices = self.device_slots(&backend, &p.devices);
        // if this preset is already running, its own usage is part of "used" — add it back
        if running_self {
            if let Some(i) = self.pm.get(&p.id) {
                let est = i.info.read().mem_estimate.clone();
                for d in devices.iter_mut() {
                    d.free_bytes += est.get(&d.id).copied().unwrap_or(0);
                }
            }
        }
        let mut inp = args::mem_input_from_preset(p, schema.as_deref(), devices);
        self.fill_mem_defaults(&mut inp, &backend);
        let mut extra = 0;
        for mid in [p.mmproj_id.clone().or(model.mmproj_id.clone()), p.draft_model_id.clone()].into_iter().flatten() {
            if let Ok(m) = self.model(&mid) {
                extra += m.size;
            }
        }
        inp.extra_gpu_bytes = extra;
        Ok(memory::estimate(&model.info, &model.tensors, model.size, &inp))
    }

    pub fn fill_mem_defaults(&self, inp: &mut memory::MemInput, backend: &Backend) {
        let s = self.settings();
        let snap = self.snapshot.read();
        inp.gpu_overhead_bytes = s.gpu_overhead_mib << 20;
        inp.margin_bytes = s.vram_margin_mib << 20;
        inp.ram_total_bytes = snap.ram_total;
        inp.ram_free_bytes = snap.ram_available;
        inp.custom_cache_bits = s.custom_cache_bits.clone();
        let cal: Option<serde_json::Value> = self.store.get("mem_calibration", &backend.id).ok().flatten();
        inp.compute_coef = cal.and_then(|v| v["compute_coef"].as_f64()).unwrap_or(1.0);
    }

    // ---------- lifecycle
    fn lock_for(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.start_locks.lock().entry(id.to_string()).or_default().clone()
    }

    pub fn launch_spec(&self, p: &Preset, port: u16) -> Result<LaunchSpec> {
        let backend = self.backend(&p.backend_id)?;
        if backend.server_bin.is_empty() {
            bail!("сборка «{}» не просканирована", backend.name);
        }
        let model = self.model(&p.model_id)?;
        let schema = self.schema(&backend);
        let mmproj_id = p.mmproj_id.clone().or(model.mmproj_id.clone()).filter(|s| !s.is_empty() && s != "none");
        let mmproj = match mmproj_id {
            Some(id) if p.mmproj_id.as_deref() != Some("none") => self.model(&id).ok(),
            _ => None,
        };
        let draft = p.draft_model_id.as_ref().and_then(|id| self.model(id).ok());
        let loras: Vec<Model> = p.lora_ids.iter().filter_map(|id| self.model(id).ok()).collect();
        let alias = self.primary_alias(&p.id).or_else(|| Some(model.alias.clone()));
        let ctx = LaunchCtx {
            preset: p,
            backend: &backend,
            schema: schema.as_deref(),
            model: &model,
            mmproj: mmproj.as_ref(),
            draft: draft.as_ref(),
            loras: loras.iter().collect(),
            host: "127.0.0.1".into(),
            port,
            alias,
        };
        Ok(args::build_launch(&ctx))
    }

    /// Start a preset (idempotent). Evicts idle instances if memory is insufficient.
    pub async fn start_preset(self: &Arc<Self>, preset_id: &str, force: bool) -> Result<StartOutcome> {
        let lock = self.lock_for(preset_id);
        let _g = lock.lock().await;
        if let Some(i) = self.pm.get(preset_id) {
            if i.state().is_alive() {
                return Ok(StartOutcome { instance: i.snapshot(), evicted: vec![], warnings: vec![] });
            }
        }
        let p = self.preset(preset_id)?;
        let backend = self.backend(&p.backend_id)?;
        let model = self.model(&p.model_id)?;
        if model.missing {
            bail!("файл модели не найден: {}", model.paths.first().cloned().unwrap_or_default());
        }
        let missing = backends::missing_features(&backend, &p.required_features);
        if !missing.is_empty() && !force {
            bail!("сборка «{}» не поддерживает: {}", backend.name, missing.join(", "));
        }
        let s = self.settings();
        let mut evicted = vec![];
        let mut warnings = vec![];

        if s.swap_mode {
            for i in self.pm.alive() {
                let (id, pinned) = {
                    let r = i.info.read();
                    (r.id.clone(), r.pinned)
                };
                if id != preset_id && !pinned {
                    self.pm.stop(&id).await.ok();
                    evicted.push(id);
                }
            }
        }
        // max instances
        loop {
            let alive = self.pm.alive();
            if alive.len() < s.max_instances.max(1) {
                break;
            }
            match self.pick_victim(&[], &evicted) {
                Some(v) => {
                    self.pm.stop(&v).await.ok();
                    evicted.push(v);
                }
                None => bail!("достигнут лимит инстансов ({}) и нечего выгрузить", s.max_instances),
            }
        }
        // memory check + eviction
        self.refresh_snapshot();
        let mut est = self.estimate_preset(&p, false)?;
        let devs: Vec<String> = est.devices.iter().map(|d| d.id.clone()).collect();
        while est.fit == Fit::No && !devs.is_empty() {
            let Some(v) = self.pick_victim(&devs, &evicted) else { break };
            tracing::info!("evicting {v} to free memory for {preset_id}");
            self.pm.stop(&v).await.ok();
            evicted.push(v);
            tokio::time::sleep(Duration::from_millis(1500)).await;
            self.refresh_snapshot();
            est = self.estimate_preset(&p, false)?;
        }
        if est.fit == Fit::No && !devs.is_empty() {
            if !force {
                let d = est.devices.iter().filter(|d| d.fit == Fit::No).map(|d| format!("{}: нужно {:.0} МиБ, свободно {:.0} МиБ", d.id, memory::mib(d.usage.total()), memory::mib(d.free))).collect::<Vec<_>>().join("; ");
                bail!("не хватает памяти ({d}). Запустите с force, если уверены.");
            }
            warnings.push("Запуск без запаса памяти (force)".into());
        }
        if !missing.is_empty() {
            warnings.push(format!("Нет фич: {}", missing.join(", ")));
        }

        let port = self.pm.free_port(s.internal_port_start, s.internal_port_end)?;
        let spec = self.launch_spec(&p, port)?;
        warnings.extend(spec.warnings.clone());
        let info = InstanceInfo {
            id: p.id.clone(),
            preset_id: p.id.clone(),
            preset_name: p.name.clone(),
            alias: self.primary_alias(&p.id),
            model_id: p.model_id.clone(),
            backend_id: p.backend_id.clone(),
            port,
            mem_estimate: est.devices.iter().map(|d| (d.id.clone(), d.usage.total())).collect(),
            devices: devs,
            pinned: p.pinned,
            ttl: p.ttl.unwrap_or(s.default_ttl),
            ..Default::default()
        };
        let inst = self.pm.start(&spec, info).await?;
        Ok(StartOutcome { instance: inst.snapshot(), evicted, warnings })
    }

    /// LRU idle unpinned instance (optionally sharing one of `devs`).
    fn pick_victim(&self, devs: &[String], exclude: &[String]) -> Option<String> {
        let mut c: Vec<(i64, String)> = self
            .pm
            .alive()
            .iter()
            .filter(|i| i.active.load(std::sync::atomic::Ordering::Relaxed) == 0)
            .map(|i| i.snapshot())
            .filter(|i| !i.pinned && !exclude.contains(&i.id))
            .filter(|i| devs.is_empty() || i.devices.is_empty() || i.devices.iter().any(|d| devs.contains(d)))
            .map(|i| (i.last_used, i.id))
            .collect();
        c.sort();
        c.first().map(|x| x.1.clone())
    }

    /// Ensure preset is running and ready (used by the router).
    pub async fn ensure_ready(self: &Arc<Self>, preset_id: &str, can_load: bool) -> Result<Arc<Instance>> {
        let alive = self.pm.get(preset_id).map(|i| i.state().is_alive()).unwrap_or(false);
        if !alive {
            if let Some(msg) = self.game.blocked(self) {
                bail!(msg);
            }
        }
        if let Some(i) = self.pm.get(preset_id) {
            match i.state() {
                InstState::Ready => {
                    i.info.write().last_used = now();
                    return Ok(i);
                }
                InstState::Starting | InstState::Loading => {}
                _ => {
                    if !can_load {
                        bail!("модель не загружена, а у ключа нет права на загрузку");
                    }
                    self.start_preset(preset_id, false).await?;
                }
            }
        } else {
            if !can_load {
                bail!("модель не загружена, а у ключа нет права на загрузку");
            }
            self.start_preset(preset_id, false).await?;
        }
        let t = Duration::from_secs(self.settings().load_timeout_secs);
        let inst = self.pm.wait_ready(preset_id, t).await?;
        inst.info.write().last_used = now();
        Ok(inst)
    }

    /// Choose a preset among alias-group candidates: running → fitting → first.
    pub fn choose_candidate(&self, cands: &[String]) -> Option<String> {
        for c in cands {
            if let Some(i) = self.pm.get(c) {
                if i.state().is_alive() {
                    return Some(c.clone());
                }
            }
        }
        for c in cands {
            if let Ok(p) = self.preset(c) {
                if let Ok(e) = self.estimate_preset(&p, false) {
                    if e.fit != Fit::No {
                        return Some(c.clone());
                    }
                }
            }
        }
        cands.first().cloned()
    }

    pub fn refresh_snapshot(&self) {
        let s = self.monitor.snapshot();
        *self.snapshot.write() = s;
    }

    // ---------- background tasks
    pub fn spawn_background(self: &Arc<Self>) {
        crate::gamemode::spawn(self.clone());
        crate::update::spawn(self.clone());
        // re-parse --help of builds whose cached schema is from an older parser
        let app = self.clone();
        tokio::spawn(async move {
            let all: Vec<Backend> = app.store.list("backends").unwrap_or_default();
            for b in all.into_iter().filter(backends::schema_outdated) {
                if let Err(e) = app.rescan_backend(&b.id).await {
                    tracing::warn!("rescan {}: {e:#}", b.name);
                }
            }
        });
        // monitor
        let app = self.clone();
        tokio::spawn(async move {
            loop {
                let ms = app.settings().poll_interval_ms.clamp(250, 10_000);
                let a2 = app.clone();
                let snap = tokio::task::spawn_blocking(move || {
                    let s = a2.monitor.snapshot();
                    // per-instance ram/vram
                    for i in a2.pm.alive() {
                        let pid = i.info.read().pid;
                        if let Some(pid) = pid {
                            let ram = a2.monitor.process_ram(pid);
                            let vram: u64 = s.gpus.iter().flat_map(|g| g.processes.iter()).filter(|(p, _)| *p == pid).map(|(_, b)| *b).sum();
                            let mut w = i.info.write();
                            w.ram_bytes = ram;
                            w.vram_bytes = (vram > 0).then_some(vram);
                        }
                    }
                    s
                })
                .await;
                if let Ok(s) = snap {
                    app.bus.send(Event::System { snapshot: serde_json::to_value(&s).unwrap_or_default() });
                    *app.snapshot.write() = s;
                }
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
        });
        // TTL reaper
        let app = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let now = now();
                for i in app.pm.alive() {
                    let s = i.snapshot();
                    if s.state != Some(InstState::Ready) || s.pinned || s.ttl == 0 || s.active_requests > 0 {
                        continue;
                    }
                    if now - s.last_used > s.ttl as i64 {
                        tracing::info!("TTL unload {}", s.preset_name);
                        let _ = app.pm.stop(&s.id).await;
                        app.bus.toast("info", format!("{} выгружен", s.preset_name), "простой дольше TTL");
                    }
                }
            }
        });
        // crash supervisor
        let app = self.clone();
        tokio::spawn(async move {
            let mut rx = app.bus.subscribe();
            loop {
                let ev = match rx.recv().await {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                };
                let Event::Instance { info } = ev else { continue };
                if info["state"] != "crashed" {
                    continue;
                }
                let s = app.settings();
                if !s.auto_restart {
                    continue;
                }
                let id = info["id"].as_str().unwrap_or_default().to_string();
                let attempt = {
                    let mut rc = app.restart_counts.lock();
                    let e = rc.entry(id.clone()).or_insert((0, now()));
                    if now() - e.1 > 600 {
                        *e = (0, now());
                    }
                    e.0 += 1;
                    e.0
                };
                if attempt > s.max_restart_attempts {
                    app.bus.toast("error", "Автоперезапуск остановлен", format!("{id}: превышено число попыток"));
                    continue;
                }
                let a = app.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
                    if let Some(i) = a.pm.get(&id) {
                        i.info.write().restarts = attempt;
                    }
                    if let Err(e) = a.start_preset(&id, false).await {
                        tracing::warn!("restart {id} failed: {e}");
                    }
                });
            }
        });
        // autostart
        let app = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let presets: Vec<Preset> = app.store.list("presets").unwrap_or_default();
            for p in presets.into_iter().filter(|p| p.autostart) {
                if let Err(e) = app.start_preset(&p.id, false).await {
                    app.bus.toast("error", format!("Автозапуск {}", p.name), e.to_string());
                }
            }
        });
        // downloads worker
        crate::downloads::spawn_worker(self.clone());
    }

    /// Store preset with version history.
    pub fn save_preset(&self, mut p: Preset) -> Result<Preset> {
        if p.id.is_empty() {
            p.id = new_id();
            p.created_at = now();
        }
        if let Some(old) = self.store.get::<Preset>("presets", &p.id)? {
            self.store.put("preset_history", &format!("{}:{}", old.id, old.version), &old)?;
            p.version = old.version + 1;
            p.created_at = old.created_at;
            // keep last 20 versions
            let hist: Vec<Preset> = self.store.list("preset_history")?;
            let mut mine: Vec<&Preset> = hist.iter().filter(|h| h.id == p.id).collect();
            mine.sort_by_key(|h| h.version);
            if mine.len() > 20 {
                for h in &mine[..mine.len() - 20] {
                    self.store.delete("preset_history", &format!("{}:{}", h.id, h.version))?;
                }
            }
        }
        p.updated_at = now();
        if p.name.trim().is_empty() {
            bail!("имя пресета пустое");
        }
        self.store.put("presets", &p.id, &p)?;
        Ok(p)
    }

    pub fn validate_alias_name(name: &str) -> Result<()> {
        if name.is_empty() || name.len() > 128 || !name.chars().all(|c| c.is_ascii_alphanumeric() || "._-:/".contains(c)) {
            bail!("алиас может содержать только a-z, 0-9 и . _ - : /");
        }
        Ok(())
    }

    pub async fn shutdown(&self) {
        self.pm.stop_all().await;
    }

    pub fn instance_logs(&self, id: &str, tail: usize) -> Result<Vec<crate::process::LogLine>> {
        Ok(self.pm.get(id).context("инстанс не найден")?.logs(tail))
    }
}
