#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use bekdesk_core::app::{Caps, Decision, Mode, Pos};
use bekdesk_core::{clip, discovery, display, modes, net, App};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, RunEvent, State, WebviewWindowBuilder};
use tauri_plugin_autostart::ManagerExt;

type Core<'a> = State<'a, Arc<App>>;

/// Physical monitors (x, y, w, h), main first, without bekdesk's own virtual monitor.
fn real_monitors(app: &tauri::AppHandle) -> Vec<(i32, i32, u32, u32)> {
    let virt = display::vdd_rect();
    let mut v: Vec<_> = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| (m.position().x, m.position().y, m.size().width, m.size().height))
        .filter(|m| Some(*m) != virt)
        .collect();
    if let Some(p) = app.primary_monitor().ok().flatten().map(|m| (m.position().x, m.position().y)) {
        v.sort_by_key(|m| (m.0, m.1) != p);
    }
    v
}

fn monitor_string(app: &tauri::AppHandle) -> String {
    real_monitors(app).iter().map(|(x, y, w, h)| format!("{x},{y},{w},{h}")).collect::<Vec<_>>().join(";")
}

/// Flash each real screen's number for a moment, like Windows' "Identify".
#[tauri::command]
async fn identify_screens(app: tauri::AppHandle) -> Result<(), String> {
    for (i, (x, y, mw, mh)) in real_monitors(&app).into_iter().enumerate() {
        let label = format!("identify{i}");
        if app.get_webview_window(&label).is_some() {
            continue;
        }
        let w = WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::App(format!("identify.html?n={}", i + 1).into()))
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .focused(false)
            .shadow(false)
            .inner_size(240.0, 240.0)
            .build()
            .map_err(|e| e.to_string())?;
        if let Ok(sz) = w.outer_size() {
            let _ = w.set_position(tauri::PhysicalPosition::new(x + (mw as i32 - sz.width as i32) / 2, y + (mh as i32 - sz.height as i32) / 2));
        }
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
            let _ = w.close();
        });
    }
    Ok(())
}

/// Put bekdesk in a job object that kills every engine it started when bekdesk exits or crashes,
/// so no Sunshine/Moonlight keeps running (and streaming) behind the user's back.
#[cfg(windows)]
fn engines_die_with_us() {
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of_val(&info) as u32);
        AssignProcessToJobObject(job, GetCurrentProcess());
        // The handle is deliberately never closed: Windows closes it when bekdesk exits, which ends the job.
    }
}

#[tauri::command]
fn snapshot(app: Core) -> serde_json::Value {
    let mut v = app.snapshot();
    v["layout"] = serde_json::to_value(modes::default_positions(&app)).unwrap_or_default();
    v
}

#[tauri::command]
async fn set_mode(app: Core<'_>, fp: String, mode: Mode) -> Result<(), String> {
    modes::set_mode(&app, &fp, mode).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn decide(app: Core, id: u64, decision: String) {
    let d = match decision.as_str() {
        "once" => Decision::Once,
        "always" => Decision::Always,
        "block" => Decision::Block,
        _ => Decision::Decline,
    };
    app.decide(id, d);
}

#[tauri::command]
fn set_caps(app: Core, fp: String, caps: Caps) {
    if let Some(t) = app.cfg.lock().unwrap().trusted.get_mut(&fp) {
        t.caps = caps;
    }
    app.save();
}

#[tauri::command]
fn forget(app: Core, fp: String) {
    {
        let mut c = app.cfg.lock().unwrap();
        c.trusted.remove(&fp);
        c.modes.remove(&fp);
    }
    app.once.lock().unwrap().remove(&fp);
    app.save();
    modes::stop_local(&app, &fp, false);
}

#[tauri::command]
fn unblock(app: Core, fp: String) {
    app.cfg.lock().unwrap().blocked.remove(&fp);
    app.save();
}

#[tauri::command]
fn save_layout(app: Core, layout: HashMap<String, Pos>) {
    app.cfg.lock().unwrap().layout = layout;
    app.save();
    let app = app.inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = modes::restart_deskflow_server(&app).await;
        modes::reposition_virtual(&app).await;
    });
}

#[tauri::command]
async fn send_files(app: Core<'_>, fp: String, paths: Vec<String>) -> Result<(), String> {
    let paths = paths.into_iter().map(PathBuf::from).collect();
    net::send_files(&app, &fp, paths, false).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn set_name(app: Core, handle: tauri::AppHandle, name: String) {
    let name = name.trim().chars().take(60).collect::<String>();
    if !name.is_empty() {
        app.cfg.lock().unwrap().name = name;
        app.save();
        readvertise(&handle, &app);
    }
}

/// Tell the network about a new name or monitor layout.
fn readvertise(handle: &tauri::AppHandle, app: &App) {
    if let Some(d) = handle.try_state::<mdns_sd::ServiceDaemon>() {
        let _ = discovery::advertise(app, &d);
    }
}

#[tauri::command]
fn diagnostics(app: Core) -> String {
    app.diagnostics()
}

#[tauri::command]
async fn connect_address(app: Core<'_>, host: String) -> Result<String, String> {
    let app = app.inner().clone();
    let fp = net::connect_address(&app, &host).await.map_err(|e| format!("{e:#}"))?;
    Ok(app.peer_name(&fp))
}

#[tauri::command]
fn set_tablet(app: Core, mode: Mode, res: String) -> Result<(), String> {
    if display::parse_res(&res).is_none() {
        return Err("resolution must look like 1920x1200".into());
    }
    let mut c = app.cfg.lock().unwrap();
    c.tablet = mode;
    c.tablet_res = res;
    drop(c);
    app.note("tablet", "");
    app.save();
    Ok(())
}

#[tauri::command]
async fn tablet_pin(app: Core<'_>, pin: String) -> Result<(), String> {
    modes::tablet_pin(&app, pin.trim()).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn autostart(handle: tauri::AppHandle, enable: Option<bool>) -> Result<bool, String> {
    let m = handle.autolaunch();
    match enable {
        Some(true) => m.enable().map_err(|e| e.to_string())?,
        Some(false) => m.disable().map_err(|e| e.to_string())?,
        None => {}
    }
    m.is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn open_folder(app: Core, which: String) {
    let p = if which == "logs" { app.dir.join("logs") } else { app.inbox_dir() };
    let _ = std::fs::create_dir_all(&p);
    let opener = if cfg!(windows) { "explorer" } else { "open" };
    let _ = std::process::Command::new(opener).arg(p).spawn();
}

fn show(app: &tauri::AppHandle) {
    let w = match app.get_webview_window("main") {
        Some(w) => w,
        // The window is destroyed when closed (frees the webview's memory); rebuild it from config.
        None => match app.config().app.windows.first().map(|c| WebviewWindowBuilder::from_config(app, c).and_then(|b| b.build())) {
            Some(Ok(w)) => w,
            _ => return,
        },
    };
    let _ = w.show();
    let _ = w.unminimize();
    let _ = w.set_focus();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--install-vdd" {
        // Elevated helper path, reached only through an explicit UAC prompt.
        let r = display::install_elevated(std::path::Path::new(&args[2]));
        std::process::exit(if r.is_ok() { 0 } else { 1 });
    }
    #[cfg(windows)]
    engines_die_with_us();

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--background"])))
        .setup(move |app| {
            // Overrides let two instances run side by side on one machine for testing.
            let dir = std::env::var_os("BEKDESK_DATA").map(PathBuf::from).map_or_else(|| app.path().app_data_dir(), Ok)?;
            let port = std::env::var("BEKDESK_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(net::PORT);
            let host = hostname::get().map(|h| h.to_string_lossy().to_string()).unwrap_or_else(|_| "Computer".into());
            let core = App::new(dir, port, &host)?;
            if let Some(w) = app.get_webview_window("main") {
                *core.monitors.lock().unwrap() = monitor_string(app.handle());
                if args.iter().any(|a| a == "--background") {
                    let _ = w.destroy();
                }
            }
            app.manage(core.clone());

            let rt = tauri::async_runtime::block_on(async { tokio::runtime::Handle::current() });
            let c = core.clone();
            tauri::async_runtime::spawn(async move {
                match tokio::net::TcpListener::bind(("0.0.0.0", c.port)).await {
                    Ok(l) => net::serve(c, l).await,
                    Err(e) => c.log(format!("cannot listen on port {}: {e}", c.port)),
                }
            });
            match discovery::start(core.clone()) {
                Ok(d) => {
                    app.manage(d);
                }
                Err(e) => core.log(format!("discovery unavailable: {e:#}")),
            }
            clip::start(core.clone(), rt);
            tauri::async_runtime::spawn(modes::supervise(core.clone()));
            tauri::async_runtime::spawn(bekdesk_core::engines::prefetch(core.clone()));
            // Monitors change at runtime (plug/unplug, the virtual monitor); keep the list live.
            let (handle, c) = (app.handle().clone(), core.clone());
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    let s = monitor_string(&handle);
                    let mut m = c.monitors.lock().unwrap();
                    if *m != s {
                        *m = s;
                        drop(m);
                        readvertise(&handle, &c);
                    }
                }
            });

            let open = MenuItem::with_id(app, "open", "Open bekdesk", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().ok_or("no icon")?)
                .tooltip("bekdesk")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => show(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|t, e| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, .. } = e {
                        show(t.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![snapshot, set_mode, decide, set_caps, forget, unblock, save_layout, identify_screens, connect_address, diagnostics, send_files, set_name, set_tablet, tablet_pin, autostart, open_folder])
        .build(tauri::generate_context!())
        .expect("error while building bekdesk")
        .run(|_, e| {
            // Closing the window keeps bekdesk running in the tray; only Quit (explicit exit code) ends it.
            if let RunEvent::ExitRequested { api, code: None, .. } = e {
                api.prevent_exit();
            }
        });
}
