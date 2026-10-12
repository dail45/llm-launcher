//! Hardware monitoring: GPUs (DXGI + PDH on Windows for any vendor, NVML for NVIDIA extras), RAM, CPU.
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sysinfo::System;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GpuInfo {
    pub index: usize,
    pub name: String,
    pub vendor: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub util: Option<f32>,
    pub temp_c: Option<u32>,
    pub power_w: Option<f32>,
    pub clock_mhz: Option<u32>,
    pub luid: Option<String>,
    pub source: String,
    /// (pid, bytes) when known (NVML)
    pub processes: Vec<(u32, u64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemSnapshot {
    pub gpus: Vec<GpuInfo>,
    pub ram_total: u64,
    pub ram_available: u64,
    pub ram_used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub cpu_name: String,
    pub cpu_physical: usize,
    pub cpu_logical: usize,
    pub cpu_usage: f32,
    pub ts: i64,
}

/// GPU usage of one OS process (for game-mode detection).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GpuProc {
    pub pid: u32,
    /// dedicated VRAM, bytes
    pub vram: u64,
    /// 3D engine utilization, % (Windows only)
    pub util: f32,
}

pub struct Monitor {
    procs: Mutex<Vec<GpuProc>>,
    nvml: Option<nvml_wrapper::Nvml>,
    sys: Mutex<System>,
    #[cfg(windows)]
    pdh: Mutex<Option<win::Pdh>>,
}

#[allow(dead_code)]
fn vendor_name(id: u32) -> &'static str {
    match id {
        0x10de => "NVIDIA",
        0x1002 | 0x1022 => "AMD",
        0x8086 => "Intel",
        _ => "Other",
    }
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}

impl Monitor {
    pub fn new() -> Self {
        let nvml = nvml_wrapper::Nvml::init().ok();
        if nvml.is_none() {
            tracing::info!("NVML not available");
        }
        let mut sys = System::new();
        sys.refresh_memory();
        sys.refresh_cpu_all();
        Self {
            procs: Mutex::new(vec![]),
            nvml,
            sys: Mutex::new(sys),
            #[cfg(windows)]
            pdh: Mutex::new(win::Pdh::new().ok()),
        }
    }

    fn nvml_gpus(&self) -> Vec<GpuInfo> {
        use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
        let Some(n) = &self.nvml else { return vec![] };
        let count = n.device_count().unwrap_or(0);
        let mut out = vec![];
        for i in 0..count {
            let Ok(d) = n.device_by_index(i) else { continue };
            let mem = d.memory_info().ok();
            let processes = d
                .running_compute_processes()
                .unwrap_or_default()
                .into_iter()
                .chain(d.running_graphics_processes().unwrap_or_default())
                .filter_map(|p| match p.used_gpu_memory {
                    nvml_wrapper::enums::device::UsedGpuMemory::Used(b) => Some((p.pid, b)),
                    _ => None,
                })
                .collect();
            out.push(GpuInfo {
                index: i as usize,
                name: d.name().unwrap_or_default(),
                vendor: "NVIDIA".into(),
                total_bytes: mem.as_ref().map(|m| m.total).unwrap_or(0),
                used_bytes: mem.as_ref().map(|m| m.used).unwrap_or(0),
                free_bytes: mem.as_ref().map(|m| m.free).unwrap_or(0),
                util: d.utilization_rates().ok().map(|u| u.gpu as f32),
                temp_c: d.temperature(TemperatureSensor::Gpu).ok(),
                power_w: d.power_usage().ok().map(|mw| mw as f32 / 1000.0),
                clock_mhz: d.clock_info(Clock::Graphics).ok(),
                luid: None,
                source: "nvml".into(),
                processes,
            });
        }
        out
    }

    /// Per-process GPU usage from the last sample.
    pub fn gpu_procs(&self) -> Vec<GpuProc> {
        self.procs.lock().clone()
    }

    pub fn gpus(&self) -> Vec<GpuInfo> {
        let nv = self.nvml_gpus();
        #[cfg(windows)]
        {
            let mut list = win::dxgi_adapters();
            if let Some(p) = self.pdh.lock().as_mut() {
                let (usage, util, procs) = p.sample();
                *self.procs.lock() = procs;
                for g in list.iter_mut() {
                    if let Some(l) = &g.luid {
                        if let Some(u) = usage.get(l) {
                            g.used_bytes = *u;
                            g.free_bytes = g.total_bytes.saturating_sub(*u);
                        }
                        g.util = util.get(l).copied();
                    }
                }
            }
            // enrich NVIDIA adapters with NVML data (matched by name + memory)
            let mut used = vec![false; nv.len()];
            for g in list.iter_mut().filter(|g| g.vendor == "NVIDIA") {
                if let Some((k, n)) = nv.iter().enumerate().find(|(k, n)| {
                    !used[*k] && (n.name.contains(&g.name) || g.name.contains(&n.name)) && (n.total_bytes as i64 - g.total_bytes as i64).abs() < (1 << 30)
                }) {
                    used[k] = true;
                    g.used_bytes = n.used_bytes;
                    g.free_bytes = n.free_bytes;
                    g.total_bytes = n.total_bytes.max(g.total_bytes);
                    g.util = n.util.or(g.util);
                    g.temp_c = n.temp_c;
                    g.power_w = n.power_w;
                    g.clock_mhz = n.clock_mhz;
                    g.processes = n.processes.clone();
                    g.source = "dxgi+nvml".into();
                }
            }
            for (i, g) in list.iter_mut().enumerate() {
                g.index = i;
            }
            list
        }
        #[cfg(not(windows))]
        {
            let mut m: std::collections::HashMap<u32, u64> = Default::default();
            for (pid, b) in nv.iter().flat_map(|g| g.processes.iter()) {
                *m.entry(*pid).or_insert(0) += *b;
            }
            *self.procs.lock() = m.into_iter().map(|(pid, vram)| GpuProc { pid, vram, util: 0.0 }).collect();
            nv
        }
    }

    pub fn snapshot(&self) -> SystemSnapshot {
        let gpus = self.gpus();
        let mut s = self.sys.lock();
        s.refresh_memory();
        s.refresh_cpu_usage();
        let cpu_name = s.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default();
        SystemSnapshot {
            gpus,
            ram_total: s.total_memory(),
            ram_available: s.available_memory(),
            ram_used: s.used_memory(),
            swap_total: s.total_swap(),
            swap_used: s.used_swap(),
            cpu_name,
            cpu_physical: System::physical_core_count().unwrap_or(0),
            cpu_logical: s.cpus().len(),
            cpu_usage: s.global_cpu_usage(),
            ts: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Resident memory of a process (bytes).
    pub fn process_ram(&self, pid: u32) -> Option<u64> {
        let mut s = self.sys.lock();
        let p = sysinfo::Pid::from_u32(pid);
        s.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[p]), true);
        s.process(p).map(|p| p.memory())
    }
}

/// Match a llama.cpp backend device (e.g. "CUDA0: NVIDIA GeForce RTX 4090") to a system GPU.
pub fn match_device<'a>(dev_name: &str, dev_total_mib: Option<u64>, gpus: &'a [GpuInfo], taken: &[usize]) -> Option<&'a GpuInfo> {
    let dn = dev_name.to_lowercase();
    let mut best: Option<(i64, &GpuInfo)> = None;
    for g in gpus.iter().filter(|g| !taken.contains(&g.index)) {
        let gn = g.name.to_lowercase();
        let mut score = 0i64;
        if gn == dn {
            score += 100;
        } else if gn.contains(&dn) || dn.contains(&gn) {
            score += 60;
        } else {
            let common = dn.split_whitespace().filter(|w| w.len() > 2 && gn.contains(*w)).count() as i64;
            score += common * 10;
        }
        if let Some(t) = dev_total_mib {
            let diff = (g.total_bytes as i64 / (1 << 20) - t as i64).abs();
            if diff < 1024 {
                score += 30;
            }
        }
        if score > 0 && best.map(|(s, _)| score > s).unwrap_or(true) {
            best = Some((score, g));
        }
    }
    best.map(|(_, g)| g)
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::collections::HashMap;
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::Graphics::Dxgi::*;
    use windows::Win32::System::Performance::*;

    pub fn luid_str(high: i32, low: u32) -> String {
        format!("luid_0x{:08x}_0x{:08x}", high as u32, low)
    }

    pub fn dxgi_adapters() -> Vec<GpuInfo> {
        let mut out = vec![];
        unsafe {
            let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
            let mut i = 0u32;
            while let Ok(a) = f.EnumAdapters1(i) {
                i += 1;
                let Ok(d) = a.GetDesc1() else { continue };
                if (d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
                    continue;
                }
                let name = String::from_utf16_lossy(&d.Description).trim_end_matches('\0').trim().to_string();
                if name.contains("Microsoft Basic") {
                    continue;
                }
                let total = d.DedicatedVideoMemory as u64;
                if total < (256 << 20) {
                    continue;
                }
                let mut used = 0;
                if let Ok(a3) = a.cast::<IDXGIAdapter3>() {
                    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
                    if a3.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info).is_ok() {
                        // per-process; replaced by PDH system-wide value when available
                        used = info.CurrentUsage;
                    }
                }
                out.push(GpuInfo {
                    index: out.len(),
                    name,
                    vendor: vendor_name(d.VendorId).into(),
                    total_bytes: total,
                    used_bytes: used,
                    free_bytes: total.saturating_sub(used),
                    luid: Some(luid_str(d.AdapterLuid.HighPart, d.AdapterLuid.LowPart)),
                    source: "dxgi".into(),
                    ..Default::default()
                });
            }
        }
        out
    }

    pub struct Pdh {
        query: PDH_HQUERY,
        mem: PDH_HCOUNTER,
        util: PDH_HCOUNTER,
        pmem: PDH_HCOUNTER,
    }
    unsafe impl Send for Pdh {}

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    impl Pdh {
        pub fn new() -> anyhow::Result<Self> {
            unsafe {
                let mut q = PDH_HQUERY::default();
                if PdhOpenQueryW(PCWSTR::null(), 0, &mut q) != 0 {
                    anyhow::bail!("PdhOpenQuery failed");
                }
                let mut mem = PDH_HCOUNTER::default();
                let mut util = PDH_HCOUNTER::default();
                let p1 = wide("\\GPU Adapter Memory(*)\\Dedicated Usage");
                let p2 = wide("\\GPU Engine(*)\\Utilization Percentage");
                if PdhAddEnglishCounterW(q, PCWSTR(p1.as_ptr()), 0, &mut mem) != 0 {
                    anyhow::bail!("PDH counter unavailable");
                }
                let _ = PdhAddEnglishCounterW(q, PCWSTR(p2.as_ptr()), 0, &mut util);
                let mut pmem = PDH_HCOUNTER::default();
                let p3 = wide("\\GPU Process Memory(*)\\Dedicated Usage");
                let _ = PdhAddEnglishCounterW(q, PCWSTR(p3.as_ptr()), 0, &mut pmem);
                let _ = PdhCollectQueryData(q);
                Ok(Self { query: q, mem, util, pmem })
            }
        }

        fn array(&self, c: PDH_HCOUNTER, fmt: PDH_FMT) -> Vec<(String, f64, i64)> {
            let mut out = vec![];
            unsafe {
                let mut size = 0u32;
                let mut count = 0u32;
                let _ = PdhGetFormattedCounterArrayW(c, fmt, &mut size, &mut count, None);
                if size == 0 {
                    return out;
                }
                let mut buf = vec![0u8; size as usize + 16];
                let ptr = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
                if PdhGetFormattedCounterArrayW(c, fmt, &mut size, &mut count, Some(ptr)) != 0 {
                    return out;
                }
                for i in 0..count as usize {
                    let it = &*ptr.add(i);
                    let name = it.szName.to_string().unwrap_or_default();
                    out.push((name, it.FmtValue.Anonymous.doubleValue, it.FmtValue.Anonymous.largeValue));
                }
            }
            out
        }

        /// (dedicated usage by luid, utilization% by luid, per-process usage)
        pub fn sample(&mut self) -> (HashMap<String, u64>, HashMap<String, f32>, Vec<GpuProc>) {
            let mut usage = HashMap::new();
            let mut util: HashMap<String, f32> = HashMap::new();
            unsafe {
                if PdhCollectQueryData(self.query) != 0 {
                    return (usage, util, vec![]);
                }
            }
            // instance names look like pid_1234_luid_0x..._phys_0[_eng_0_engtype_3D]
            let pid_of = |n: &str| -> Option<u32> { n.strip_prefix("pid_")?.split('_').next()?.parse().ok() };
            let mut procs: HashMap<u32, GpuProc> = HashMap::new();
            if !self.pmem.0.is_null() {
                for (name, _, large) in self.array(self.pmem, PDH_FMT_LARGE) {
                    if let Some(pid) = pid_of(&name) {
                        procs.entry(pid).or_insert(GpuProc { pid, ..Default::default() }).vram += large.max(0) as u64;
                    }
                }
            }
            let luid_of = |n: &str| -> Option<String> {
                let i = n.find("luid_")?;
                let s = &n[i..];
                // luid_0x00000000_0x0000d1a3
                Some(s.get(..26)?.to_lowercase())
            };
            for (name, _, large) in self.array(self.mem, PDH_FMT_LARGE) {
                if let Some(l) = luid_of(&name) {
                    *usage.entry(l).or_insert(0) += large.max(0) as u64;
                }
            }
            // utilization: max over engine types of summed per-process values
            let mut per: HashMap<(String, String), f64> = HashMap::new();
            for (name, dbl, _) in self.array(self.util, PDH_FMT_DOUBLE) {
                if let (Some(l), Some(e)) = (luid_of(&name), name.split("engtype_").nth(1)) {
                    *per.entry((l, e.to_string())).or_insert(0.0) += dbl;
                    if e.eq_ignore_ascii_case("3D") {
                        if let Some(pid) = pid_of(&name) {
                            procs.entry(pid).or_insert(GpuProc { pid, ..Default::default() }).util += dbl as f32;
                        }
                    }
                }
            }
            for ((l, _), v) in per {
                let e = util.entry(l).or_insert(0.0);
                *e = e.max(v.min(100.0) as f32);
            }
            (usage, util, procs.into_values().map(|mut p| { p.util = p.util.min(100.0); p }).collect())
        }
    }
}
