//! llama-server instance lifecycle: spawn (inside a Windows Job Object), logs, health, stop.
use crate::args::LaunchSpec;
use crate::events::{Bus, Event};
use crate::paths;
use anyhow::{bail, Context, Result};
use parking_lot::{Mutex, RwLock};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstState {
    Starting,
    Loading,
    Ready,
    Stopping,
    Stopped,
    Crashed,
    Failed,
}

impl InstState {
    pub fn is_alive(&self) -> bool {
        matches!(self, Self::Starting | Self::Loading | Self::Ready)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstanceInfo {
    pub id: String,
    pub preset_id: String,
    pub preset_name: String,
    pub alias: Option<String>,
    pub model_id: String,
    pub backend_id: String,
    pub pid: Option<u32>,
    pub port: u16,
    pub state: Option<InstState>,
    pub started_at: i64,
    pub ready_at: Option<i64>,
    pub last_used: i64,
    pub active_requests: u64,
    pub requests_total: u64,
    pub tokens_generated: u64,
    pub last_tps: Option<f64>,
    pub error: Option<String>,
    pub error_hint: Option<String>,
    pub restarts: u32,
    pub load_note: Option<String>,
    pub offloaded: Option<String>,
    pub ram_bytes: Option<u64>,
    pub vram_bytes: Option<u64>,
    /// estimated bytes per device id
    pub mem_estimate: HashMap<String, u64>,
    pub devices: Vec<String>,
    pub command: String,
    pub pinned: bool,
    pub ttl: u64,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub ts: i64,
    pub stream: &'static str,
    pub line: String,
}

pub struct Instance {
    pub info: RwLock<InstanceInfo>,
    logs: Mutex<VecDeque<LogLine>>,
    child: tokio::sync::Mutex<Option<tokio::process::Child>>,
    state_tx: watch::Sender<InstState>,
    pub active: AtomicU64,
}

impl Instance {
    pub fn snapshot(&self) -> InstanceInfo {
        let mut i = self.info.read().clone();
        i.active_requests = self.active.load(Ordering::Relaxed);
        i
    }
    pub fn state(&self) -> InstState {
        *self.state_tx.borrow()
    }
    pub fn logs(&self, tail: usize) -> Vec<LogLine> {
        let l = self.logs.lock();
        l.iter().skip(l.len().saturating_sub(tail)).cloned().collect()
    }
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.info.read().port)
    }
    pub fn touch(&self) {
        self.info.write().last_used = chrono::Utc::now().timestamp();
    }
}

struct Hint {
    re: Regex,
    hint: &'static str,
}
static HINTS: LazyLock<Vec<Hint>> = LazyLock::new(|| {
    let h = |r: &str, hint: &'static str| Hint { re: Regex::new(&format!("(?i){r}")).unwrap(), hint };
    vec![
        h(r"out of memory|cudaMalloc failed|failed to allocate .*buffer|ErrorOutOfDeviceMemory|hipMalloc.*fail|unable to allocate", "Не хватило памяти GPU. Уменьшите контекст или -ngl, квантуйте KV-кэш (q8_0) или вынесите экспертов MoE на CPU."),
        h(r"unknown model architecture|unknown architecture", "Сборка не поддерживает архитектуру модели. Обновите llama.cpp или выберите подходящий форк."),
        h(r"couldn't bind|address already in use|bind failed|failed to bind", "Порт занят. Выберите другой диапазон внутренних портов в настройках."),
        h(r"error loading model|failed to load model|failed to open|no such file", "Не удалось открыть файл модели. Проверьте путь и целостность файла."),
        h(r"invalid argument|unknown argument|error: unrecognized|error while handling argument", "Сборка не поддерживает один из флагов. Проверьте параметры пресета для этой сборки."),
        h(r"no CUDA-capable device|ggml_cuda_init: failed|CUDA driver version is insufficient|cudart.*not found", "Проблема с CUDA: обновите драйвер NVIDIA или положите cudart DLL рядом с llama-server."),
        h(r"vk::|vulkan.*error|ErrorInitializationFailed", "Ошибка Vulkan: обновите драйвер видеокарты."),
        h(r"hipErrorNoBinaryForGpu|rocBLAS error|amdhip64", "Проблема с ROCm/HIP: сборка не поддерживает эту видеокарту AMD или нет HIP runtime."),
        h(r"tensor .* has wrong (shape|type)|invalid ggml type|unsupported .*type", "Модель использует формат или тип тензоров, который эта сборка не поддерживает."),
    ]
});
static RE_OFFLOAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"offloaded (\d+)/(\d+) layers").unwrap());
static RE_TPS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"eval time =.*?/\s*(\d+) tokens.*?([\d.]+) tokens per second").unwrap());

pub struct ProcessManager {
    instances: RwLock<HashMap<String, Arc<Instance>>>,
    bus: Bus,
    http: reqwest::Client,
    #[cfg(windows)]
    job: Option<win::Job>,
}

impl ProcessManager {
    pub fn new(bus: Bus) -> Self {
        Self {
            instances: RwLock::new(HashMap::new()),
            bus,
            http: reqwest::Client::builder().timeout(Duration::from_secs(3)).build().unwrap(),
            #[cfg(windows)]
            job: win::Job::new().map_err(|e| tracing::error!("job object: {e}")).ok(),
        }
    }

    pub fn get(&self, id: &str) -> Option<Arc<Instance>> {
        self.instances.read().get(id).cloned()
    }
    pub fn list(&self) -> Vec<Arc<Instance>> {
        self.instances.read().values().cloned().collect()
    }
    pub fn alive(&self) -> Vec<Arc<Instance>> {
        self.list().into_iter().filter(|i| i.state().is_alive()).collect()
    }
    pub fn used_ports(&self) -> Vec<u16> {
        self.alive().iter().map(|i| i.info.read().port).collect()
    }

    pub fn free_port(&self, start: u16, end: u16) -> Result<u16> {
        let used = self.used_ports();
        for p in start..=end {
            if used.contains(&p) {
                continue;
            }
            if std::net::TcpListener::bind(("127.0.0.1", p)).is_ok() {
                return Ok(p);
            }
        }
        bail!("нет свободных портов в диапазоне {start}-{end}")
    }

    fn emit(&self, inst: &Instance) {
        self.bus.send(Event::Instance { info: serde_json::to_value(inst.snapshot()).unwrap_or_default() });
    }

    fn set_state(&self, inst: &Instance, s: InstState) {
        inst.info.write().state = Some(s);
        let _ = inst.state_tx.send_replace(s);
        self.emit(inst);
    }

    pub fn remove(&self, id: &str) {
        if let Some(i) = self.get(id) {
            if !i.state().is_alive() {
                self.instances.write().remove(id);
                self.bus.send(Event::InstanceRemoved { id: id.into() });
            }
        }
    }

    /// Spawn an instance. `info` must have id/port set.
    pub async fn start(self: &Arc<Self>, spec: &LaunchSpec, mut info: InstanceInfo) -> Result<Arc<Instance>> {
        if let Some(existing) = self.get(&info.id) {
            if existing.state().is_alive() {
                return Ok(existing);
            }
            info.restarts = existing.info.read().restarts;
        }
        let now = chrono::Utc::now().timestamp();
        info.started_at = now;
        info.last_used = now;
        info.state = Some(InstState::Starting);
        info.command = spec.preview("cmd");
        info.error = None;
        info.error_hint = None;
        let (state_tx, _) = watch::channel(InstState::Starting);
        let inst = Arc::new(Instance {
            info: RwLock::new(info.clone()),
            logs: Mutex::new(VecDeque::with_capacity(4096)),
            child: tokio::sync::Mutex::new(None),
            state_tx,
            active: AtomicU64::new(0),
        });
        self.instances.write().insert(info.id.clone(), inst.clone());
        self.emit(&inst);

        let mut cmd = crate::proc_util::command(std::path::Path::new(&spec.program));
        #[cfg(windows)]
        {
            cmd.creation_flags(crate::proc_util::CREATE_NO_WINDOW | crate::proc_util::CREATE_NEW_PROCESS_GROUP);
        }
        if !spec.cwd.is_empty() {
            cmd.current_dir(&spec.cwd);
        }
        cmd.args(&spec.args)
            .envs(spec.env.iter().cloned())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = match cmd.spawn().with_context(|| format!("не удалось запустить {}", spec.program)) {
            Ok(c) => c,
            Err(e) => {
                inst.info.write().error = Some(e.to_string());
                self.set_state(&inst, InstState::Failed);
                return Err(e);
            }
        };
        let pid = child.id();
        #[cfg(windows)]
        if let (Some(job), Some(h)) = (&self.job, child.raw_handle()) {
            if let Err(e) = job.assign(h) {
                tracing::warn!("assign to job failed: {e}");
            }
        }
        inst.info.write().pid = pid;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        *inst.child.lock().await = Some(child);
        self.set_state(&inst, InstState::Loading);

        // log file
        let log_dir = paths::logs_dir().join("instances");
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join(format!("{}-{}.log", sanitize(&info.preset_name), chrono::Local::now().format("%Y%m%d-%H%M%S")));
        let file = Arc::new(tokio::sync::Mutex::new(tokio::fs::File::create(&log_path).await.ok()));
        rotate_logs(&log_dir, 50);
        {
            let mut f = file.lock().await;
            if let Some(f) = f.as_mut() {
                let _ = f.write_all(format!("# {}\n", spec.preview("cmd")).as_bytes()).await;
            }
        }

        for (stream, name) in [(stdout.map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), "stdout"), (stderr.map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), "stderr")] {
            let Some(stream) = stream else { continue };
            let inst = inst.clone();
            let bus = self.bus.clone();
            let file = file.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stream).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    on_line(&inst, &bus, name, &line);
                    let mut f = file.lock().await;
                    if let Some(f) = f.as_mut() {
                        let _ = f.write_all(line.as_bytes()).await;
                        let _ = f.write_all(b"\n").await;
                    }
                }
            });
        }

        // health watcher + exit watcher
        let me = self.clone();
        let inst2 = inst.clone();
        tokio::spawn(async move { me.supervise(inst2).await });
        Ok(inst)
    }

    async fn supervise(self: Arc<Self>, inst: Arc<Instance>) {
        let url = format!("{}/health", inst.base_url());
        loop {
            // process exited?
            let exit = {
                let mut g = inst.child.lock().await;
                match g.as_mut() {
                    Some(c) => c.try_wait().ok().flatten(),
                    None => {
                        if !inst.state().is_alive() {
                            return; // stopped via stop()
                        }
                        None
                    }
                }
            };
            if let Some(status) = exit {
                let was = inst.state();
                inst.info.write().exit_code = status.code();
                inst.info.write().pid = None;
                *inst.child.lock().await = None;
                if was == InstState::Stopping || was == InstState::Stopped {
                    self.set_state(&inst, InstState::Stopped);
                } else {
                    {
                        let mut i = inst.info.write();
                        if i.error.is_none() {
                            i.error = Some(format!("процесс завершился с кодом {:?}", status.code()));
                        }
                    }
                    let st = if was == InstState::Ready { InstState::Crashed } else { InstState::Failed };
                    self.set_state(&inst, st);
                    let i = inst.snapshot();
                    self.bus.toast("error", format!("{} — {}", i.preset_name, if st == InstState::Crashed { "упал" } else { "не запустился" }), i.error_hint.or(i.error).unwrap_or_default());
                }
                return;
            }
            let st = inst.state();
            if st == InstState::Loading || st == InstState::Starting {
                if let Ok(r) = self.http.get(&url).send().await {
                    if r.status().is_success() {
                        {
                            let mut w = inst.info.write();
                            let t = chrono::Utc::now().timestamp();
                            w.ready_at = Some(t);
                            w.last_used = t;
                        }
                        self.set_state(&inst, InstState::Ready);
                        let i = inst.snapshot();
                        self.bus.toast("success", format!("{} готов", i.preset_name), format!("порт {}", i.port));
                    }
                }
                tokio::time::sleep(Duration::from_millis(400)).await;
            } else {
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }
        }
    }

    /// Wait until Ready (or failure).
    pub async fn wait_ready(&self, id: &str, timeout: Duration) -> Result<Arc<Instance>> {
        let inst = self.get(id).context("instance not found")?;
        let mut rx = inst.state_tx.subscribe();
        let fut = async {
            loop {
                let s = *rx.borrow_and_update();
                match s {
                    InstState::Ready => return Ok(()),
                    InstState::Failed | InstState::Crashed | InstState::Stopped => {
                        let i = inst.snapshot();
                        bail!("{}", i.error_hint.or(i.error).unwrap_or_else(|| "инстанс остановлен".into()));
                    }
                    _ => {}
                }
                if rx.changed().await.is_err() {
                    bail!("instance dropped");
                }
            }
        };
        tokio::time::timeout(timeout, fut).await.context("таймаут загрузки модели")??;
        Ok(inst)
    }

    pub async fn stop(&self, id: &str) -> Result<()> {
        let inst = self.get(id).context("instance not found")?;
        if !inst.state().is_alive() {
            return Ok(());
        }
        self.set_state(&inst, InstState::Stopping);
        let mut g = inst.child.lock().await;
        if let Some(c) = g.as_mut() {
            let _ = c.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(15), c.wait()).await;
        }
        *g = None;
        drop(g);
        inst.info.write().pid = None;
        self.set_state(&inst, InstState::Stopped);
        Ok(())
    }

    pub async fn stop_all(&self) {
        for i in self.alive() {
            let id = i.info.read().id.clone();
            let _ = self.stop(&id).await;
        }
    }
}

fn on_line(inst: &Instance, bus: &Bus, stream: &'static str, line: &str) {
    let ts = chrono::Utc::now().timestamp_millis();
    {
        let mut l = inst.logs.lock();
        if l.len() >= 5000 {
            l.pop_front();
        }
        l.push_back(LogLine { ts, stream, line: line.to_string() });
    }
    if let Some(c) = RE_OFFLOAD.captures(line) {
        inst.info.write().offloaded = Some(format!("{}/{}", &c[1], &c[2]));
    }
    if let Some(c) = RE_TPS.captures(line) {
        if !line.contains("prompt eval") {
            let mut i = inst.info.write();
            i.last_tps = c[2].parse().ok();
            i.tokens_generated += c[1].parse::<u64>().unwrap_or(0);
        }
    }
    let state = *inst.state_tx.borrow();
    if state == InstState::Loading || state == InstState::Starting {
        let low = line.to_lowercase();
        if low.contains("load_tensors") || low.contains("llama_model_load") || low.contains("loading model") || low.contains("llama_context") || low.contains("warming up") {
            let note: String = line.chars().take(160).collect();
            inst.info.write().load_note = Some(note);
        }
    }
    let low = line.to_lowercase();
    if low.contains("error") || low.contains("failed") || low.contains("out of memory") || low.contains("unknown") || low.contains("exception") {
        for h in HINTS.iter() {
            if h.re.is_match(line) {
                let mut i = inst.info.write();
                if i.error_hint.is_none() {
                    i.error_hint = Some(h.hint.to_string());
                    i.error = Some(line.chars().take(400).collect());
                }
                break;
            }
        }
    }
    bus.send(Event::Log { instance: inst.info.read().id.clone(), stream: stream.into(), line: line.to_string(), ts });
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

fn rotate_logs(dir: &std::path::Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<_> = rd.filter_map(|e| e.ok()).filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    if files.len() <= keep {
        return;
    }
    files.sort();
    for (_, p) in files.iter().take(files.len() - keep) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(windows)]
mod win {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::*;

    pub struct Job(HANDLE);
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn new() -> anyhow::Result<Self> {
            unsafe {
                let h = CreateJobObjectW(None, None)?;
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )?;
                Ok(Self(h))
            }
        }
        pub fn assign(&self, raw: std::os::windows::io::RawHandle) -> anyhow::Result<()> {
            unsafe {
                AssignProcessToJobObject(self.0, HANDLE(raw))?;
            }
            Ok(())
        }
    }
    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}
