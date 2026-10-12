//! Self-update of the launcher from GitHub Releases.
//!
//! The kind of installation is detected from the running executable:
//! * `nsis`     — installed with the setup.exe (an `uninstall.exe` lies next to the exe);
//! * `msi`      — installed with the MSI (exe under Program Files);
//! * `portable` — the single portable exe;
//! * `headless` — `launcherd.exe`.
//!
//! The matching asset is downloaded, checked against `SHA256SUMS.txt`, then a small hidden
//! PowerShell helper waits for this process to exit, replaces the exe (or runs the installer)
//! and starts the launcher again.
use crate::app::App;
use crate::events::Event;
use anyhow::{bail, Context, Result};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

pub const REPO: &str = match option_env!("LLL_UPDATE_REPO") {
    Some(r) => r,
    None => "dail45/llm-launcher",
};

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Nsis,
    Msi,
    Portable,
    Headless,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: Option<String>,
    pub available: bool,
    pub notes: String,
    pub published_at: String,
    pub url: String,
    pub asset: Option<String>,
    pub asset_size: u64,
    pub kind: Option<Kind>,
    pub checked_at: Option<i64>,
    pub error: Option<String>,
}

static STATE: OnceLock<Mutex<UpdateInfo>> = OnceLock::new();
static EXIT_HOOK: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn state() -> &'static Mutex<UpdateInfo> {
    STATE.get_or_init(|| Mutex::new(UpdateInfo { current: crate::VERSION.into(), kind: Some(kind()), ..Default::default() }))
}

/// The desktop app registers a graceful exit (closes the tray icon); headless falls back to process::exit.
pub fn set_exit_hook(f: impl Fn() + Send + Sync + 'static) {
    let _ = EXIT_HOOK.set(Box::new(f));
}

pub fn snapshot() -> UpdateInfo {
    state().lock().clone()
}

pub fn kind() -> Kind {
    // for testing the download/verification path on any OS
    match std::env::var("LLL_UPDATE_KIND").as_deref() {
        Ok("nsis") => return Kind::Nsis,
        Ok("msi") => return Kind::Msi,
        Ok("portable") => return Kind::Portable,
        Ok("headless") => return Kind::Headless,
        _ => {}
    }
    if !cfg!(windows) {
        return Kind::Unsupported;
    }
    let Ok(exe) = std::env::current_exe() else { return Kind::Unsupported };
    kind_for(&exe, std::env::var("ProgramFiles").ok().as_deref(), std::env::var("ProgramW6432").ok().as_deref())
}

pub fn kind_for(exe: &Path, pf: Option<&str>, pf64: Option<&str>) -> Kind {
    let name = exe.file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name.starts_with("launcherd") {
        return Kind::Headless;
    }
    let dir = exe.parent().unwrap_or(Path::new("."));
    if dir.join("uninstall.exe").is_file() {
        return Kind::Nsis;
    }
    let d = dir.to_string_lossy().to_lowercase();
    for p in [pf, pf64].into_iter().flatten() {
        if !p.is_empty() && d.starts_with(&p.to_lowercase()) {
            return Kind::Msi;
        }
    }
    Kind::Portable
}

fn asset_matches(k: Kind, name: &str) -> bool {
    let n = name.to_lowercase();
    match k {
        Kind::Headless => n.starts_with("launcherd") && n.ends_with(".exe"),
        Kind::Portable => n.ends_with("-portable.exe"),
        Kind::Nsis => n.ends_with("-setup.exe"),
        Kind::Msi => n.ends_with(".msi"),
        Kind::Unsupported => false,
    }
}

/// "v1.2.3-rc.1" → ([1,2,3], Some("rc.1"))
fn parse_ver(v: &str) -> Option<(Vec<u64>, Option<String>)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let (core, pre) = match v.split_once(['-', '+']) {
        Some((c, p)) => (c, Some(p.to_string())),
        None => (v, None),
    };
    let nums: Option<Vec<u64>> = core.split('.').map(|x| x.parse().ok()).collect();
    let mut nums = nums?;
    while nums.len() < 3 {
        nums.push(0);
    }
    Some((nums, pre))
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    let (Some((a, ap)), Some((b, bp))) = (parse_ver(latest), parse_ver(current)) else { return false };
    if a != b {
        return a > b;
    }
    match (ap, bp) {
        (None, Some(_)) => true,
        (Some(x), Some(y)) => x > y,
        _ => false,
    }
}

async fn gh_json(app: &App, url: &str) -> Result<Value> {
    let mut req = app.http.get(url).header("Accept", "application/vnd.github+json");
    let tok = app.settings().github_token;
    if !tok.is_empty() {
        req = req.header("Authorization", format!("Bearer {tok}"));
    }
    Ok(req.send().await?.error_for_status()?.json().await?)
}

/// Asset download URL: through the API when a token is set (private repositories), else the public link.
fn asset_url(app: &App, a: &Value) -> Option<String> {
    let api = a["url"].as_str().filter(|_| !app.settings().github_token.is_empty());
    api.or(a["browser_download_url"].as_str()).map(String::from)
}

async fn latest_release(app: &App) -> Result<Value> {
    latest_release_raw(app).await.map_err(|e| {
        if e.downcast_ref::<reqwest::Error>().and_then(|r| r.status()).map(|s| s.as_u16() == 404).unwrap_or(false) {
            anyhow::anyhow!("релизы {REPO} не найдены: репозиторий приватный или релизов ещё нет (для приватного укажите GitHub-токен в Настройки → Загрузки)")
        } else {
            e
        }
    })
}

async fn latest_release_raw(app: &App) -> Result<Value> {
    if app.settings().app_update_prerelease {
        let v = gh_json(app, &format!("https://api.github.com/repos/{REPO}/releases?per_page=10")).await?;
        v.as_array().and_then(|a| a.iter().find(|r| !r["draft"].as_bool().unwrap_or(false)).cloned()).context("релизов нет")
    } else {
        gh_json(app, &format!("https://api.github.com/repos/{REPO}/releases/latest")).await
    }
}

pub async fn check(app: &App) -> UpdateInfo {
    let k = kind();
    let mut info = UpdateInfo { current: crate::VERSION.into(), kind: Some(k), checked_at: Some(crate::types::now()), ..Default::default() };
    match latest_release(app).await {
        Ok(r) => {
            let tag = r["tag_name"].as_str().unwrap_or_default().to_string();
            info.available = is_newer(&tag, crate::VERSION);
            info.latest = Some(tag.trim_start_matches('v').to_string());
            info.notes = r["body"].as_str().unwrap_or_default().chars().take(4000).collect();
            info.published_at = r["published_at"].as_str().unwrap_or_default().into();
            info.url = r["html_url"].as_str().unwrap_or_default().into();
            if let Some(a) = r["assets"].as_array().and_then(|a| a.iter().find(|x| asset_matches(k, x["name"].as_str().unwrap_or_default()))) {
                info.asset = a["name"].as_str().map(String::from);
                info.asset_size = a["size"].as_u64().unwrap_or(0);
            }
        }
        Err(e) => info.error = Some(format!("{e:#}")),
    }
    let prev = std::mem::replace(&mut *state().lock(), info.clone());
    app.bus.send(Event::Update { info: serde_json::to_value(&info).unwrap_or_default() });
    if info.available && prev.latest != info.latest {
        app.bus.toast("info", &format!("Доступна новая версия {}", info.latest.clone().unwrap_or_default()), "Настройки → Обновление");
    }
    info
}

/// Periodic check (on start and every 12 h) when enabled in the settings.
pub fn spawn(app: Arc<App>) {
    cleanup();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        loop {
            if app.settings().app_update_check {
                check(&app).await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(12 * 3600)).await;
        }
    });
}

fn updates_dir() -> PathBuf {
    crate::paths::data_dir().join("updates")
}

/// Remove downloaded installers and the previous exe left after an update.
fn cleanup() {
    let _ = std::fs::remove_dir_all(updates_dir());
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("exe.old"));
    }
}

async fn sha256_of(path: &Path) -> Result<String> {
    let p = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<String> {
        use sha2::Digest;
        use std::io::Read;
        let mut f = std::fs::File::open(&p)?;
        let mut h = sha2::Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        Ok(hex::encode(h.finalize()))
    })
    .await?
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// PowerShell helper run after this process exits.
pub fn helper_script(k: Kind, pid: u32, exe: &Path, file: &Path, args: &[String]) -> String {
    let exe_q = ps_quote(&exe.to_string_lossy());
    let file_q = ps_quote(&file.to_string_lossy());
    let log_q = ps_quote(&file.with_file_name("update-helper.log").to_string_lossy());
    let mut s = format!(
        "$ErrorActionPreference='SilentlyContinue'\nStart-Transcript -LiteralPath {log_q} -Force | Out-Null\n$t=0; while ((Get-Process -Id {pid}) -and $t -lt 120) {{ Start-Sleep -Milliseconds 250; $t++ }}\nStop-Process -Id {pid} -Force\nStart-Sleep -Milliseconds 300\n"
    );
    match k {
        Kind::Portable | Kind::Headless => {} // handled by swap_and_restart
        Kind::Nsis => s.push_str(&format!("Start-Process -FilePath {file_q} -ArgumentList '/S' -Wait\n")),
        Kind::Msi => s.push_str(&format!("Start-Process -FilePath 'msiexec.exe' -ArgumentList @('/i', ('\"'+{file_q}+'\"'), '/passive', '/norestart') -Wait\n")),
        Kind::Unsupported => {}
    }
    let a: Vec<String> = args.iter().map(|x| ps_quote(&format!("\"{}\"", x.replace('"', "\\\"")))).collect();
    if a.is_empty() {
        s.push_str(&format!("Start-Process -FilePath {exe_q}\n"));
    } else {
        s.push_str(&format!("Start-Process -FilePath {exe_q} -ArgumentList @({})\n", a.join(",")));
    }
    s
}

pub fn install(app: Arc<App>) -> Result<String> {
    let k = kind();
    if k == Kind::Unsupported {
        bail!("автообновление доступно только в Windows-сборке (в Docker обновите образ: docker pull)");
    }
    if BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        bail!("обновление уже выполняется");
    }
    let job = app.jobs.start(&app.bus, "app_update", "Обновление Llama Launcher");
    let jid = job.id.clone();
    drop(job);
    let a = app.clone();
    let id = jid.clone();
    tokio::spawn(async move {
        let r = run_install(&a, &id, k).await;
        let h = a.jobs.handle(&a.bus, &id);
        match r {
            Ok(()) => {}
            Err(e) => {
                BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
                h.fail(format!("{e:#}"));
            }
        }
    });
    Ok(jid)
}

async fn run_install(app: &Arc<App>, jid: &str, k: Kind) -> Result<()> {
    let h = || app.jobs.handle(&app.bus, jid);
    h().progress(None, "Проверка последней версии…");
    let r = latest_release(app).await?;
    let tag = r["tag_name"].as_str().unwrap_or_default().to_string();
    if !is_newer(&tag, crate::VERSION) {
        bail!("уже установлена последняя версия ({})", crate::VERSION);
    }
    let assets = r["assets"].as_array().cloned().unwrap_or_default();
    let asset = assets.iter().find(|x| asset_matches(k, x["name"].as_str().unwrap_or_default())).with_context(|| format!("в релизе {tag} нет файла для типа установки «{k:?}»"))?;
    let name = asset["name"].as_str().unwrap_or_default().to_string();
    let url = asset_url(app, asset).context("нет ссылки на файл")?;
    let dir = updates_dir();
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(&name);
    let part = dir.join(format!("{name}.part"));
    crate::downloads::download_to(app, &url, &part, |done, total| {
        let p = if total > 0 { done as f64 / total as f64 } else { 0.0 };
        h().progress(Some(p * 0.9), format!("Скачивание {name}: {:.1} / {:.1} МБ", done as f64 / 1e6, total as f64 / 1e6));
    })
    .await?;
    std::fs::rename(&part, &file)?;

    h().progress(Some(0.92), "Проверка контрольной суммы…");
    if let Some(sums) = assets.iter().find(|x| x["name"].as_str().map(|n| n.eq_ignore_ascii_case("SHA256SUMS.txt")).unwrap_or(false)) {
        let tmp = dir.join("SHA256SUMS.txt");
        crate::downloads::download_to(app, &asset_url(app, sums).unwrap_or_default(), &tmp, |_, _| {}).await?;
        let txt = std::fs::read_to_string(&tmp)?.replace('\u{feff}', "");
        // asset names in the release have spaces replaced by dots
        let norm = |s: &str| s.trim().replace(' ', ".").to_lowercase();
        let want = txt.lines().find_map(|l| {
            let (hash, n) = l.trim().split_once(char::is_whitespace)?;
            (norm(n) == norm(&name)).then(|| hash.trim().to_lowercase())
        });
        let want = want.with_context(|| format!("{name} нет в SHA256SUMS.txt"))?;
        let got = sha256_of(&file).await?;
        if got != want {
            let _ = std::fs::remove_file(&file);
            bail!("контрольная сумма не совпадает — файл повреждён");
        }
    } else {
        tracing::warn!("update: SHA256SUMS.txt not found in release {tag}");
    }

    let exe = std::env::current_exe()?;
    let args = restart_args(std::env::args().skip(1));
    match k {
        // Windows allows renaming a running exe: swap it in place and start the new
        // version directly (it waits for this process to exit) — no PowerShell needed.
        Kind::Portable | Kind::Headless => swap_and_restart(&exe, &file, &args)?,
        _ => spawn_helper(&helper_script(k, std::process::id(), &exe, &file, &args))?,
    }
    h().done(json!({"version": tag}), format!("Установка {tag}, перезапуск…"));
    app.bus.toast("info", "Обновление", &format!("Устанавливается {tag}, лаунчер перезапустится"));
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    app.shutdown().await;
    match EXIT_HOOK.get() {
        Some(f) => f(),
        None => std::process::exit(0),
    }
    Ok(())
}

/// Arguments for the restarted process: drop `--minimized` (show the window) and stale `--after-update <pid>`.
pub fn restart_args(it: impl Iterator<Item = String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = it.peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--minimized" => {}
            "--after-update" => {
                it.next();
            }
            _ => out.push(a),
        }
    }
    out
}

/// Replace `exe` with `file` (old one is kept as `.exe.old` until the next start) and launch it.
pub fn swap_and_restart(exe: &Path, file: &Path, args: &[String]) -> Result<()> {
    let old = exe.with_extension("exe.old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).with_context(|| format!("не удалось переименовать {} — нет прав на запись в папку программы?", exe.display()))?;
    let rollback = || {
        let _ = std::fs::remove_file(exe);
        let _ = std::fs::rename(&old, exe);
    };
    if std::fs::rename(file, exe).is_err() {
        // different volume (data dir elsewhere) — copy instead
        if let Err(e) = std::fs::copy(file, exe) {
            rollback();
            return Err(e).context("не удалось записать новую версию");
        }
        let _ = std::fs::remove_file(file);
    }
    let mut args = args.to_vec();
    args.push("--after-update".into());
    args.push(std::process::id().to_string());
    if let Err(e) = spawn_detached(exe, &args) {
        rollback();
        return Err(e).context("не удалось запустить новую версию");
    }
    Ok(())
}

#[cfg(windows)]
const BREAKAWAY: u32 = 0x0100_0000; // CREATE_BREAKAWAY_FROM_JOB

fn spawn_detached(exe: &Path, args: &[String]) -> Result<()> {
    let mk = |_flags: u32| {
        let mut c = std::process::Command::new(exe);
        c.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        if let Some(d) = exe.parent() {
            c.current_dir(d);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(_flags);
        }
        c
    };
    #[cfg(windows)]
    {
        let base = crate::proc_util::CREATE_NO_WINDOW | crate::proc_util::CREATE_NEW_PROCESS_GROUP;
        // break away from a job object (e.g. when started from a terminal/IDE) so we are not killed with the parent
        if mk(base | BREAKAWAY).spawn().is_ok() {
            return Ok(());
        }
        mk(base).spawn()?;
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        mk(0).spawn()?;
        Ok(())
    }
}

/// Called first thing in `main`: after a self-update wait until the previous process exits.
pub fn wait_after_update() {
    let mut it = std::env::args();
    while let Some(a) = it.next() {
        if a == "--after-update" {
            if let Some(pid) = it.next().and_then(|p| p.parse::<u32>().ok()) {
                wait_pid(pid, 30_000);
            }
        }
    }
}

#[cfg(windows)]
fn wait_pid(pid: u32, ms: u32) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(h, ms);
            let _ = CloseHandle(h);
        }
    }
}

#[cfg(not(windows))]
fn wait_pid(pid: u32, ms: u32) {
    let t = std::time::Instant::now();
    while Path::new(&format!("/proc/{pid}")).exists() && t.elapsed().as_millis() < ms as u128 {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[cfg(windows)]
fn spawn_helper(script: &str) -> Result<()> {
    use base64::Engine;
    use std::os::windows::process::CommandExt;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let enc = base64::engine::general_purpose::STANDARD.encode(utf16);
    let mk = |flags: u32| {
        let mut c = std::process::Command::new("powershell.exe");
        c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &enc])
            .creation_flags(flags)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        c
    };
    let base = crate::proc_util::CREATE_NO_WINDOW | crate::proc_util::CREATE_NEW_PROCESS_GROUP;
    if mk(base | BREAKAWAY).spawn().is_err() {
        mk(base).spawn().context("не удалось запустить PowerShell")?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn spawn_helper(_script: &str) -> Result<()> {
    bail!("автообновление доступно только в Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("v0.2.1", "0.2.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("v0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(is_newer("0.3.0", "0.3.0-rc.1"));
        assert!(!is_newer("0.3.0-rc.1", "0.3.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn kinds_and_assets() {
        let t = tempfile::tempdir().unwrap();
        let inst = t.path().join("inst");
        std::fs::create_dir_all(&inst).unwrap();
        std::fs::write(inst.join("uninstall.exe"), b"").unwrap();
        assert_eq!(kind_for(&inst.join("llama-launcher.exe"), None, None), Kind::Nsis);
        assert_eq!(kind_for(&t.path().join("LlamaLauncher-0.2.0-portable.exe"), None, None), Kind::Portable);
        assert_eq!(kind_for(&t.path().join("launcherd.exe"), None, None), Kind::Headless);
        let pf = t.path().join("Program Files");
        assert_eq!(kind_for(&pf.join("Llama Launcher").join("llama-launcher.exe"), Some(&pf.to_string_lossy()), None), Kind::Msi);
        let names = ["launcherd-0.2.0.exe", "Llama.Launcher_0.2.0_x64-setup.exe", "Llama.Launcher_0.2.0_x64_en-US.msi", "LlamaLauncher-0.2.0-portable.exe", "SHA256SUMS.txt"];
        let pick = |k| names.iter().filter(|n| asset_matches(k, n)).copied().collect::<Vec<_>>();
        assert_eq!(pick(Kind::Headless), vec!["launcherd-0.2.0.exe"]);
        assert_eq!(pick(Kind::Nsis), vec!["Llama.Launcher_0.2.0_x64-setup.exe"]);
        assert_eq!(pick(Kind::Msi), vec!["Llama.Launcher_0.2.0_x64_en-US.msi"]);
        assert_eq!(pick(Kind::Portable), vec!["LlamaLauncher-0.2.0-portable.exe"]);
    }

    #[test]
    fn script() {
        let s = helper_script(Kind::Msi, 42, Path::new(r"C:\Users\Пётр\LL\LlamaLauncher.exe"), Path::new(r"C:\data\updates\new.exe"), &["--data-dir".into(), r"D:\it's".into()]);
        assert!(s.contains("Get-Process -Id 42"));
        assert!(s.contains(r"'C:\Users\Пётр\LL\LlamaLauncher.exe'"));
        assert!(s.contains("it''s"));
        let n = helper_script(Kind::Nsis, 1, Path::new("a.exe"), Path::new("s.exe"), &[]);
        assert!(n.contains("-ArgumentList '/S' -Wait") && n.ends_with("Start-Process -FilePath 'a.exe'\n"));
    }

    #[test]
    fn args_for_restart() {
        let a = restart_args(["--minimized", "--data-dir", "D:\\x", "--after-update", "12"].map(String::from).into_iter());
        assert_eq!(a, vec!["--data-dir".to_string(), "D:\\x".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn swap_in_place() {
        let t = tempfile::tempdir().unwrap();
        let exe = t.path().join("LL.exe");
        std::fs::write(&exe, "#!/bin/sh\necho old\n").unwrap();
        let upd = t.path().join("updates");
        std::fs::create_dir_all(&upd).unwrap();
        let new = upd.join("new.exe");
        std::fs::write(&new, "#!/bin/sh\necho \"$@\" > \"$(dirname \"$0\")/ran.txt\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).unwrap();
        swap_and_restart(&exe, &new, &["--data-dir".into(), "X".into()]).unwrap();
        assert!(!new.exists());
        assert_eq!(std::fs::read_to_string(exe.with_extension("exe.old")).unwrap(), "#!/bin/sh\necho old\n");
        let ran = t.path().join("ran.txt");
        for _ in 0..50 {
            if ran.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let txt = std::fs::read_to_string(&ran).unwrap();
        assert!(txt.starts_with("--data-dir X --after-update "), "{txt}");
    }
}
