//! Game mode: when a game (or any other heavy GPU user) runs, unload models and pause
//! automatic loading by the router; optionally bring the models back afterwards.
use crate::app::AppRef;
use crate::events::Event;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::time::Duration;

/// Processes that use the GPU but are never treated as games by auto-detection.
pub const DEFAULT_IGNORE: &[&str] = &[
    "dwm", "explorer", "csrss", "system", "idle", "chrome", "msedge", "msedgewebview2", "firefox", "opera", "brave", "vivaldi", "yandex", "browser",
    "discord", "telegram", "slack", "teams", "ms-teams", "zoom", "obs64", "obs", "steamwebhelper", "epicwebhelper", "code", "cursor", "windsurf",
    "searchhost", "shellexperiencehost", "startmenuexperiencehost", "textinputhost", "applicationframehost", "widgets", "nvcontainer",
    "nvidia overlay", "nvidia share", "nvidia app", "radeonsoftware", "amdrsserv", "wallpaper32", "wallpaper64", "lively", "rainmeter",
    "llama-server", "llama-launcher", "launcherd", "llama-bench", "llama-perplexity", "photos", "vlc", "mpc-hc64", "potplayermini64",
];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GameStatus {
    pub enabled: bool,
    pub active: bool,
    /// process that triggered the mode, e.g. "Cyberpunk2077.exe"; "manual" when turned on by hand
    pub reason: String,
    /// "list" | "auto" | "manual"
    pub source: String,
    pub since: i64,
    /// presets that were running when the mode started (restored afterwards if enabled)
    pub unloaded: Vec<String>,
    /// detection is suppressed until this trigger disappears (after manual "resume")
    pub snoozed: Option<String>,
}

#[derive(Default)]
pub struct GameMode {
    pub status: RwLock<GameStatus>,
    /// consecutive samples a process looked like a game (auto-detect)
    hits: parking_lot::Mutex<HashMap<u32, u32>>,
    last_seen: parking_lot::Mutex<i64>,
}

fn norm(n: &str) -> String {
    let n = n.trim().to_lowercase();
    n.strip_suffix(".exe").unwrap_or(&n).to_string()
}

impl GameMode {
    pub fn snapshot(&self) -> GameStatus {
        self.status.read().clone()
    }
    /// Error text for the router when autoload is blocked.
    pub fn blocked(&self, app: &AppRef) -> Option<String> {
        let s = self.status.read();
        if s.active && app.settings().game_block_loads {
            Some(format!("игровой режим: {} — загрузка моделей приостановлена", if s.reason == "manual" { "включён вручную".to_string() } else { format!("запущена {}", s.reason) }))
        } else {
            None
        }
    }
}

fn emit(app: &AppRef) {
    let s = app.game.snapshot();
    app.bus.send(Event::Game { info: serde_json::to_value(&s).unwrap_or_default() });
}

pub async fn activate(app: &AppRef, reason: &str, source: &str) {
    let set = app.settings();
    let mut unloaded = vec![];
    for i in app.pm.alive() {
        let info = i.snapshot();
        if info.pinned && !set.game_unload_pinned {
            continue;
        }
        unloaded.push(info.id.clone());
        let _ = app.pm.stop(&info.id).await;
    }
    {
        let mut s = app.game.status.write();
        s.active = true;
        s.reason = reason.to_string();
        s.source = source.to_string();
        s.since = crate::types::now();
        s.unloaded = unloaded.clone();
        s.snoozed = None;
    }
    tracing::info!("game mode on ({reason}), unloaded {}", unloaded.len());
    let what = if reason == "manual" { "включён вручную".to_string() } else { format!("обнаружена {reason}") };
    app.bus.toast("info", "🎮 Игровой режим", format!("{what}; выгружено моделей: {}", unloaded.len()));
    emit(app);
}

pub async fn deactivate(app: &AppRef, snooze: bool) {
    let (unloaded, reason) = {
        let mut s = app.game.status.write();
        if !s.active {
            return;
        }
        s.active = false;
        s.snoozed = (snooze && s.source != "manual").then(|| s.reason.clone());
        (std::mem::take(&mut s.unloaded), s.reason.clone())
    };
    tracing::info!("game mode off ({reason})");
    let restore = app.settings().game_restore;
    app.bus.toast("info", "🎮 Игровой режим выключен", if restore && !unloaded.is_empty() { format!("возвращаю модели: {}", unloaded.len()) } else { "модели снова загружаются по запросу".into() });
    emit(app);
    if restore {
        for id in unloaded {
            let a = app.clone();
            tokio::spawn(async move {
                if let Err(e) = a.start_preset(&id, false).await {
                    tracing::warn!("game mode restore {id}: {e}");
                }
            });
        }
    }
}

/// One detection pass: returns the triggering process (name, source).
fn detect(app: &AppRef, sys: &mut sysinfo::System) -> Option<(String, String)> {
    let set = app.settings();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let list: Vec<String> = set.game_processes.iter().map(|n| norm(n)).filter(|n| !n.is_empty()).collect();
    if !list.is_empty() {
        for p in sys.processes().values() {
            let n = p.name().to_string_lossy().to_string();
            if list.contains(&norm(&n)) {
                return Some((n, "list".into()));
            }
        }
    }
    if !set.game_auto_detect {
        app.game.hits.lock().clear();
        return None;
    }
    let ours: Vec<u32> = app.pm.alive().iter().filter_map(|i| i.info.read().pid).chain([std::process::id()]).collect();
    let ignore: Vec<String> = DEFAULT_IGNORE.iter().map(|s| s.to_string()).chain(set.game_ignore.iter().map(|n| norm(n))).collect();
    let mut hits = app.game.hits.lock();
    let mut seen = vec![];
    let mut found = None;
    for gp in app.monitor.gpu_procs() {
        if ours.contains(&gp.pid) {
            continue;
        }
        let Some(proc_) = sys.process(sysinfo::Pid::from_u32(gp.pid)) else { continue };
        let name = proc_.name().to_string_lossy().to_string();
        if ignore.contains(&norm(&name)) {
            continue;
        }
        let heavy = gp.util >= set.game_gpu_util as f32 || gp.vram >= set.game_vram_mib << 20;
        if !heavy {
            continue;
        }
        seen.push(gp.pid);
        let h = hits.entry(gp.pid).or_insert(0);
        *h += 1;
        // ~3 consecutive samples so short spikes (video, alt-tab) don't trigger it
        if *h >= 3 && found.is_none() {
            found = Some((name, "auto".into()));
        }
    }
    hits.retain(|pid, _| seen.contains(pid));
    found
}

pub fn spawn(app: AppRef) {
    tokio::spawn(async move {
        let sys = std::sync::Arc::new(parking_lot::Mutex::new(sysinfo::System::new()));
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let set = app.settings();
            if !set.game_mode {
                if app.game.status.read().active {
                    deactivate(&app, false).await;
                }
                if app.game.status.read().enabled {
                    app.game.status.write().enabled = false;
                    emit(&app);
                }
                continue;
            }
            if !app.game.status.read().enabled {
                app.game.status.write().enabled = true;
                emit(&app);
            }
            let (a2, sm) = (app.clone(), sys.clone());
            let found = tokio::task::spawn_blocking(move || detect(&a2, &mut sm.lock())).await.ok().flatten();
            let s2 = app.game.snapshot();
            let now = crate::types::now();
            match found {
                Some((name, source)) => {
                    *app.game.last_seen.lock() = now;
                    if s2.snoozed.as_deref() == Some(name.as_str()) {
                        continue;
                    }
                    if !s2.active {
                        activate(&app, &name, &source).await;
                    } else if s2.source != "manual" && s2.reason != name {
                        app.game.status.write().reason = name;
                        emit(&app);
                    }
                }
                None => {
                    if s2.snoozed.is_some() {
                        app.game.status.write().snoozed = None;
                        emit(&app);
                    }
                    let last = *app.game.last_seen.lock();
                    if s2.active && s2.source != "manual" && now - last >= set.game_grace_secs as i64 {
                        deactivate(&app, false).await;
                    }
                }
            }
        }
    });
}

pub fn status_json(app: &AppRef) -> serde_json::Value {
    json!(app.game.snapshot())
}
