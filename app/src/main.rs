//! Llama Launcher desktop app: runs the core in-process, shows the UI (served by the core
//! on 127.0.0.1) in a native window and lives in the system tray.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use lll_core::app::AppRef;
use lll_core::events::Event;
use std::sync::OnceLock;
use tauri::menu::{Menu, MenuBuilder, MenuItem, PredefinedMenuItem, SubmenuBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

static CORE: OnceLock<AppRef> = OnceLock::new();
const TRAY_ID: &str = "main";

fn core() -> &'static AppRef {
    CORE.get().expect("core not started")
}

fn ui_url() -> String {
    let s = core().settings();
    // the window always talks to the local listener
    let host = if s.ui_bind == "0.0.0.0" || s.ui_bind == "::" { "127.0.0.1".to_string() } else { s.ui_bind.clone() };
    format!("http://{}:{}/", host, s.ui_port)
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let url = ui_url().parse().expect("bad ui url");
    let w = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
        .title("Llama Launcher")
        .inner_size(1360.0, 860.0)
        .min_inner_size(820.0, 560.0)
        .build();
    if let Err(e) = w {
        tracing::error!("window: {e}");
    }
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let c = core();
    let mut presets: Vec<lll_core::types::Preset> = c.store.list("presets").unwrap_or_default();
    presets.sort_by(|a, b| a.order.cmp(&b.order).then(a.name.cmp(&b.name)));
    let mut sub = SubmenuBuilder::new(app, "Пресеты");
    if presets.is_empty() {
        sub = sub.item(&MenuItem::with_id(app, "noop", "(нет пресетов)", false, None::<&str>)?);
    }
    let mut alive = 0;
    for p in &presets {
        let st = c.pm.get(&p.id).map(|i| i.state());
        let running = st.map(|s| s.is_alive()).unwrap_or(false);
        if running {
            alive += 1;
        }
        let label = match st {
            Some(lll_core::process::InstState::Ready) => format!("■ {}  (готов)", p.name),
            Some(lll_core::process::InstState::Loading | lll_core::process::InstState::Starting) => format!("■ {}  (загрузка…)", p.name),
            _ => format!("▶ {}", p.name),
        };
        sub = sub.item(&MenuItem::with_id(app, format!("preset:{}", p.id), label, true, None::<&str>)?);
    }
    let sub = sub.build()?;
    let upd = lll_core::update::snapshot();
    let mut mb = MenuBuilder::new(app);
    if upd.available && upd.asset.is_some() {
        mb = mb
            .item(&MenuItem::with_id(app, "update", format!("⬆ Обновить до v{}", upd.latest.clone().unwrap_or_default()), true, None::<&str>)?)
            .item(&PredefinedMenuItem::separator(app)?);
    }
    mb
        .item(&MenuItem::with_id(app, "open", "Открыть Llama Launcher", true, None::<&str>)?)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&sub)
        .item(&MenuItem::with_id(app, "stop_all", format!("Остановить все ({alive})"), alive > 0, None::<&str>)?)
        .item(&MenuItem::with_id(app, "open_browser", "Открыть в браузере", true, None::<&str>)?)
        .item(&MenuItem::with_id(app, "game", game_label(), true, None::<&str>)?)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&MenuItem::with_id(app, "version", format!("Версия {}", lll_core::version_string()), false, None::<&str>)?)
        .item(&MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?)
        .build()
}

fn game_label() -> String {
    let g = core().game.snapshot();
    if g.active {
        format!("🎮 Выключить игровой режим ({})", if g.reason == "manual" { "вручную".to_string() } else { g.reason })
    } else {
        "🎮 Включить игровой режим".to_string()
    }
}

fn refresh_tray(app: &AppHandle) {
    if let Some(t) = app.tray_by_id(TRAY_ID) {
        if let Ok(m) = build_menu(app) {
            let _ = t.set_menu(Some(m));
        }
        let n = core().pm.alive().len();
        let v = lll_core::VERSION;
        let g = if core().game.snapshot().active { " — 🎮 игровой режим" } else { "" };
        let _ = t.set_tooltip(Some(if n == 0 { format!("Llama Launcher v{v}{g}") } else { format!("Llama Launcher v{v} — запущено: {n}{g}") }));
    }
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "open" => show_main(app),
        "open_browser" => {
            let url = ui_url();
            #[cfg(windows)]
            let _ = std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn();
            #[cfg(not(windows))]
            let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
        }
        "game" => {
            tauri::async_runtime::spawn(async {
                let c = core();
                if c.game.snapshot().active {
                    lll_core::gamemode::deactivate(c, true).await;
                } else {
                    lll_core::gamemode::activate(c, "manual", "manual").await;
                }
            });
        }
        "update" => {
            tauri::async_runtime::spawn(async {
                let c = core().clone();
                if let Err(e) = lll_core::update::install(c.clone()) {
                    c.bus.toast("error", "Обновление", e.to_string());
                }
            });
            show_main(app);
        }
        "stop_all" => {
            tauri::async_runtime::spawn(async { core().pm.stop_all().await });
        }
        "quit" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                core().shutdown().await;
                app.exit(0);
            });
        }
        other => {
            if let Some(pid) = other.strip_prefix("preset:") {
                let pid = pid.to_string();
                tauri::async_runtime::spawn(async move {
                    let c = core();
                    let alive = c.pm.get(&pid).map(|i| i.state().is_alive()).unwrap_or(false);
                    let res = if alive { c.pm.stop(&pid).await } else { c.start_preset(&pid, false).await.map(|_| ()) };
                    if let Err(e) = res {
                        c.bus.toast("error", "Ошибка", e.to_string());
                    }
                });
            }
        }
    }
}

fn main() {
    // after a self-update: let the previous process exit first (single-instance lock, UI port)
    lll_core::update::wait_after_update();
    let minimized_arg = std::env::args().any(|a| a == "--minimized");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--data-dir" {
            if let Some(d) = args.next() {
                lll_core::paths::set_data_dir(d.into());
            }
        }
    }
    let _log = lll_core::server::init_logging();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| show_main(app)))
        .setup(move |app| {
            let handle = app.handle().clone();
            let core_app = match tauri::async_runtime::block_on(lll_core::server::boot()) {
                Ok(a) => a,
                Err(e) => {
                    let msg = format!("Не удалось запустить ядро: {e:#}\n\nВозможно, порт UI занят другой программой или уже запущен launcherd.");
                    tracing::error!("{msg}");
                    #[cfg(windows)]
                    unsafe {
                        msgbox(&msg);
                    }
                    std::process::exit(1);
                }
            };
            let _ = CORE.set(core_app.clone());
            {
                let h = handle.clone();
                lll_core::update::set_exit_hook(move || h.exit(0));
            }

            let icon = app.default_window_icon().cloned();
            let mut tb = TrayIconBuilder::with_id(TRAY_ID)
                .tooltip(format!("Llama Launcher v{}", lll_core::VERSION))
                .menu(&build_menu(&handle)?)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| on_menu(app, ev.id().as_ref()))
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                        show_main(tray.app_handle());
                    }
                });
            if let Some(i) = icon {
                tb = tb.icon(i);
            }
            tb.build(app)?;

            // keep tray menu in sync with instance state / preset list
            let h2 = handle.clone();
            let mut rx = core_app.bus.subscribe();
            tauri::async_runtime::spawn(async move {
                let mut last = std::time::Instant::now() - std::time::Duration::from_secs(10);
                loop {
                    match rx.recv().await {
                        Ok(Event::Instance { .. }) | Ok(Event::InstanceRemoved { .. }) | Ok(Event::Game { .. }) | Ok(Event::Update { .. }) => {
                            let h = h2.clone();
                            let _ = h2.run_on_main_thread(move || refresh_tray(&h));
                        }
                        Ok(Event::System { .. }) if last.elapsed().as_secs() > 15 => {
                            // periodic refresh picks up renamed/added presets
                            last = std::time::Instant::now();
                            let h = h2.clone();
                            let _ = h2.run_on_main_thread(move || refresh_tray(&h));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        _ => {}
                    }
                }
            });

            let start_hidden = minimized_arg || core_app.settings().start_minimized;
            if !start_hidden {
                show_main(&handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // closing the window hides it to the tray; quit via tray menu
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("tauri build failed")
        .run(|_app, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = &event {
                // no windows open is not a reason to exit (we live in the tray)
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}

#[cfg(windows)]
unsafe fn msgbox(text: &str) {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: *mut core::ffi::c_void, text: *const u16, caption: *const u16, utype: u32) -> i32;
    }
    let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let c: Vec<u16> = "Llama Launcher".encode_utf16().chain(std::iter::once(0)).collect();
    MessageBoxW(std::ptr::null_mut(), w.as_ptr(), c.as_ptr(), 0x10);
}
