#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use bekdesk_core::app::{Caps, Decision, Mode, Pos};
use bekdesk_core::{audio, clip, discovery, display, modes, net, App};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, RunEvent, State, WebviewWindowBuilder};
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

/// Flash each screen's number for a moment, like Windows' "Identify": this computer's screens, the
/// virtual screen a tablet or phone extends to (shown with its name), and every connected computer's screens.
#[tauri::command]
async fn identify_screens(app: tauri::AppHandle, core: Core<'_>) -> Result<(), String> {
    let core = core.inner().clone();
    let peers: Vec<String> = core.peers.lock().unwrap().keys().filter(|fp| core.knows(fp)).cloned().collect();
    for fp in peers {
        let c = core.clone();
        tauri::async_runtime::spawn(async move {
            let _ = net::request(&c, &fp, &net::Req::Identify).await;
        });
    }
    flash_numbers(&app)
}

fn flash_numbers(app: &tauri::AppHandle) -> Result<(), String> {
    let mut screens: Vec<((i32, i32, u32, u32), String)> = real_monitors(app).into_iter().enumerate().map(|(i, m)| (m, (i + 1).to_string())).collect();
    // The extended tablet or laptop screen is numbered after this computer's own screens, as in Manage Screens.
    if let Some(v) = display::vdd_rect() {
        screens.push((v, (screens.len() + 1).to_string()));
    }
    for (i, ((x, y, mw, mh), text)) in screens.into_iter().enumerate() {
        let label = format!("identify{i}");
        if app.get_webview_window(&label).is_some() {
            continue;
        }
        let q: String = text.chars().filter(|c| c.is_alphanumeric() || *c == ' ').map(|c| if c == ' ' { '+' } else { c }).collect();
        let w = WebviewWindowBuilder::new(app, &label, tauri::WebviewUrl::App(format!("identify.html?n={q}").into()))
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .focused(false)
            .shadow(false)
            .inner_size(if text.len() > 2 { 420.0 } else { 240.0 }, 240.0)
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

/// Manage Screens: one of this computer's screens was dragged to (nx, ny) in desktop pixels.
#[tauri::command]
fn move_display(app: tauri::AppHandle, core: Core, x: i32, y: i32, nx: i32, ny: i32) -> Result<(), String> {
    let name = display::display_at(x, y).ok_or("that screen isn't connected any more")?;
    display::move_display(&name, nx, ny).map_err(|e| format!("{e:#}"))?;
    *core.monitors.lock().unwrap() = monitor_string(&app);
    Ok(())
}

/// Manage Screens right-click menu: what the screen at (x, y) supports and is set to now.
#[tauri::command]
fn display_modes(x: i32, y: i32) -> Result<serde_json::Value, String> {
    let name = display::display_at(x, y).ok_or("that screen isn't connected any more")?;
    Ok(serde_json::json!({"modes": display::modes(&name)}))
}

/// Resolution/refresh (`w`, `h`, `hz`), rotation (`orientation` 0-3) or turn off (`off`) for the screen at (x, y).
#[tauri::command]
fn change_display(app: tauri::AppHandle, core: Core, x: i32, y: i32, w: Option<u32>, h: Option<u32>, hz: Option<u32>, orientation: Option<u32>, off: Option<bool>) -> Result<(), String> {
    let name = display::display_at(x, y).ok_or("that screen isn't connected any more")?;
    let r = match (w, h, hz, orientation, off) {
        (_, _, _, _, Some(true)) => display::turn_off(&name),
        (_, _, _, Some(o), _) => display::rotate(&name, o.min(3)),
        (Some(w), Some(h), Some(hz), _, _) => display::set_resolution(&name, w, h, hz),
        _ => return Err("nothing to change".into()),
    };
    r.map_err(|e| format!("{e:#}"))?;
    *core.monitors.lock().unwrap() = monitor_string(&app);
    Ok(())
}

/// Right-click → "Make this my main display" in Manage Screens (this computer's screens only).
#[tauri::command]
fn make_primary(x: i32, y: i32) -> Result<(), String> {
    let name = display::display_at(x, y).ok_or("that screen isn't connected any more")?;
    display::set_primary(&name).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn open_display_settings() {
    let (cmd, arg) = if cfg!(windows) { ("explorer", "ms-settings:display") } else { ("open", "x-apple.systempreferences:com.apple.Displays-Settings.extension") };
    let _ = std::process::Command::new(cmd).arg(arg).spawn();
}

/// Pointer speed multiplier while the mouse is on `key` (a peer or "tablet"); applies immediately.
#[tauri::command]
fn set_pointer(app: Core, key: String, factor: f64) {
    app.cfg.lock().unwrap().pointer.insert(key, factor.clamp(0.25, 4.0));
    app.save();
}

/// Whether `key` (a peer or "tablet") also plays this computer's sound. Takes effect on the next connect.
#[tauri::command]
// async: runs on the async runtime, which `audio::sync` needs to start its streams.
async fn set_sound(app: Core<'_>, key: String, on: bool) -> Result<(), String> {
    app.cfg.lock().unwrap().sound.insert(key, on);
    app.save();
    audio::sync(app.inner());
    Ok(())
}

/// Whether the sound of `fp` (a computer this one controls with Share Input) plays here, muted there.
#[tauri::command]
async fn set_hear(app: Core<'_>, fp: String, on: bool) -> Result<(), String> {
    let core = app.inner();
    let mut cfg = core.cfg.lock().unwrap();
    if on { cfg.hear.insert(fp.clone()) } else { cfg.hear.remove(&fp) };
    drop(cfg);
    core.save();
    audio::sync(core);
    Ok(())
}

/// Microphone sharing: `share` lets `fp` use this computer's microphone; otherwise `fp`'s microphone is used here.
#[tauri::command]
async fn set_mic(app: Core<'_>, fp: String, share: bool, on: bool) -> Result<(), String> {
    let core = app.inner();
    {
        let mut cfg = core.cfg.lock().unwrap();
        let set = if share { &mut cfg.mic_share } else { &mut cfg.use_mic };
        if on { set.insert(fp) } else { set.remove(&fp) };
    }
    core.save();
    audio::sync(core);
    Ok(())
}

/// Whether this computer stays awake (no idle lock or sleep) while `fp` controls it with Share Input.
#[tauri::command]
fn set_stay_awake(app: Core, fp: String, on: bool) {
    let mut cfg = app.cfg.lock().unwrap();
    if on { cfg.let_sleep.remove(&fp) } else { cfg.let_sleep.insert(fp) };
    drop(cfg);
    app.save();
}

/// Whether the Windows lock screen helper is set up; checked once, then after each change (the check runs `sc`).
static LOCK_SCREEN: std::sync::OnceLock<std::sync::Mutex<bool>> = std::sync::OnceLock::new();
fn lock_screen_on() -> bool {
    *LOCK_SCREEN.get_or_init(|| std::sync::Mutex::new(bekdesk_core::service::installed())).lock().unwrap()
}

/// Welcome screen / Settings: set up (or remove) the lock screen helper, plus the extra-screen driver, behind one
/// Windows admin prompt. Waits for both parts to download first.
#[tauri::command]
async fn admin_setup(app: Core<'_>, enable: bool) -> Result<bool, String> {
    use bekdesk_core::{engines, service};
    let r: anyhow::Result<()> = async {
        let args = if enable {
            let deskflow = engines::ensure(&app, "deskflow", "components").await?;
            let inf = engines::ensure(&app, "vdd", "components").await.ok();
            let folder = deskflow.parent().map(|p| p.display().to_string()).unwrap_or_default();
            vec!["--admin-setup".into(), service::user_sid()?, folder, inf.map_or("-".into(), |p| p.display().to_string())]
        } else {
            vec!["--uninstall-service".to_string()]
        };
        tokio::task::spawn_blocking(move || service::elevated(&args)).await?
    }
    .await;
    let now = service::installed();
    *LOCK_SCREEN.get_or_init(|| std::sync::Mutex::new(now)).lock().unwrap() = now;
    r.map(|()| now).map_err(|e| format!("{e:#}"))
}

/// Settings: scrolling while another computer controls this one; used from the next Share Input session.
#[tauri::command]
fn set_scroll(app: Core, speed: f64, invert: bool) {
    let mut cfg = app.cfg.lock().unwrap();
    cfg.scroll_speed = speed.clamp(0.1, 10.0);
    cfg.invert_scroll = invert;
    drop(cfg);
    app.save();
}

#[tauri::command]
async fn rescan(app: Core<'_>) -> Result<(), String> {
    modes::rescan(app.inner()).await;
    Ok(())
}

#[tauri::command]
async fn pair_device(app: Core<'_>, fp: String) -> Result<(), String> {
    net::pair(&app, &fp).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn block_device(app: Core, fp: String) {
    let name = app.peer_name(&fp);
    {
        let mut c = app.cfg.lock().unwrap();
        c.trusted.remove(&fp);
        c.modes.remove(&fp);
        c.blocked.insert(fp.clone(), name);
    }
    app.save();
    modes::stop_local(&app, &fp, false);
}

/// While this computer shares its keyboard and mouse with another, a small drop zone sits on the screen edge
/// facing it (as placed in Manage Screens): drag files or images onto it and they go to that computer, ready to paste.
#[allow(dead_code)]
fn sync_drop_zones(app: &tauri::AppHandle, core: &App) {
    let want: Vec<(String, String, &str)> = core
        .sessions
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, s)| s.origin && s.mode == Mode::Input)
        .filter_map(|(fp, _)| Some((format!("dropzone-{}", &fp[..fp.len().min(12)]), fp.clone(), modes::peer_side(core, fp)?)))
        .collect();
    for (label, w) in app.webview_windows() {
        if label.starts_with("dropzone-") && !want.iter().any(|(l, _, _)| *l == label) {
            let _ = w.close();
        }
    }
    let mons = real_monitors(app);
    let Some(x0) = mons.iter().map(|m| m.0).min() else { return };
    let y0 = mons.iter().map(|m| m.1).min().unwrap_or(0);
    let x1 = mons.iter().map(|m| m.0 + m.2 as i32).max().unwrap_or(0);
    let y1 = mons.iter().map(|m| m.1 + m.3 as i32).max().unwrap_or(0);
    for (label, fp, side) in want {
        let w = match app.get_webview_window(&label) {
            Some(w) => w,
            None => {
                let q: String = format!("fp={fp}&side={side}&name={}", core.peer_name(&fp)).chars().map(|c| if c == ' ' { '+' } else { c }).filter(|c| c.is_ascii_alphanumeric() || "=&+-_.".contains(*c)).collect();
                let b = WebviewWindowBuilder::new(app, &label, tauri::WebviewUrl::App(format!("dropzone.html?{q}").into()))
                    .title("bekdesk drop zone")
                    .decorations(false)
                    .always_on_top(true)
                    .skip_taskbar(true)
                    .resizable(false)
                    .focused(false)
                    .shadow(false)
                    .inner_size(if matches!(side, "left" | "right") { 64.0 } else { 180.0 }, if matches!(side, "left" | "right") { 180.0 } else { 64.0 });
                #[cfg(not(target_os = "macos"))]
                let b = b.transparent(true);
                match b.build() {
                    Ok(w) => w,
                    Err(e) => return core.log(format!("drop zone: {e}")),
                }
            }
        };
        // Centred on the edge of the whole desktop that faces the other computer, a little in from the very edge:
        // touching the edge itself would move the pointer (and the drag) over to that computer.
        if let Ok(sz) = w.outer_size() {
            let (w_, h_, gap) = (sz.width as i32, sz.height as i32, 16);
            let (cx, cy) = ((x0 + x1 - w_) / 2, (y0 + y1 - h_) / 2);
            let (x, y) = match side {
                "left" => (x0 + gap, cy),
                "right" => (x1 - w_ - gap, cy),
                "up" => (cx, y0 + gap),
                _ => (cx, y1 - h_ - gap),
            };
            let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
        }
    }
}

/// Drop zone → send the dropped files to `fp` and put them on its clipboard, so they can be pasted there.
#[tauri::command]
async fn drop_files(app: Core<'_>, fp: String, paths: Vec<String>) -> Result<(), String> {
    net::send_files(&app, &fp, paths.into_iter().map(PathBuf::from).collect(), true).await.map_err(|e| format!("{e:#}"))
}

/// Sidebar → "Check for updates".
#[tauri::command]
async fn check_update() -> Result<Option<String>, String> {
    bekdesk_core::update::check().await
}

/// Sidebar → "Update to …": download, check and start installing the new version, then close so it can replace us.
#[tauri::command]
async fn install_update(app: tauri::AppHandle, core: Core<'_>) -> Result<(), String> {
    bekdesk_core::update::install(&core.dir).await.map_err(|e| format!("{e:#}"))?;
    modes::restore_pointer();
    app.exit(0);
    Ok(())
}

/// Device card → Details: what the logs say about the connection with `key` (a peer or "tablet").
#[tauri::command]
fn device_details(app: Core, key: String) -> String {
    modes::connection_details(&app, &key)
}

/// Device card → "Turn off Scroll Lock" (Windows).
#[tauri::command]
fn toggle_scroll_lock() {
    display::toggle_scroll_lock();
}

/// Sidebar → Quit: stop sharing and close bekdesk completely (there's no tray icon to quit from on some Linux desktops).
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    modes::restore_pointer();
    app.exit(0);
}

/// Opens one of the project's web pages in the browser (https only).
#[tauri::command]
fn open_url(url: String) {
    if url.starts_with("https://") && !url.contains(['"', ' ', '\'', '\n']) {
        let _ = if cfg!(windows) { std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", &url]).spawn() } else { std::process::Command::new("open").arg(&url).spawn() };
    }
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
        // BREAKAWAY_OK: Sunshine starts its stream start/stop commands (`bekdesk --vdd-on/--vdd-off`) with
        // CREATE_BREAKAWAY_FROM_JOB, which fails with "access denied" unless the job allows it. Engines bekdesk
        // starts itself don't ask to break away, so they still close with bekdesk.
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of_val(&info) as u32);
        AssignProcessToJobObject(job, GetCurrentProcess());
        // The handle is deliberately never closed: Windows closes it when bekdesk exits, which ends the job.
    }
}

#[tauri::command]
fn snapshot(app: Core) -> serde_json::Value {
    let mut v = app.snapshot();
    v["me"]["screens"] = display::screens();
    v["me"]["scroll_lock"] = modes::scroll_locked(&app).into();
    v["version"] = env!("CARGO_PKG_VERSION").into();
    v["lock_screen"] = if cfg!(windows) { lock_screen_on().into() } else { serde_json::Value::Null };
    v["update"] = match &*bekdesk_core::update::STATE.lock().unwrap() {
        Some(Ok(Some(latest))) => serde_json::json!({"available": latest}),
        Some(Err(e)) => serde_json::json!({"error": e}),
        _ => serde_json::json!({}),
    };
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
async fn tablet_pin(app: Core<'_>, id: String, pin: String, name: String) -> Result<(), String> {
    modes::tablet_pin(&app, &id, pin.trim(), &name).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn tablet_decline(app: Core<'_>, id: String) -> Result<(), String> {
    modes::tablet_decline(&app, &id).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn tablet_forget(app: Core<'_>, uuid: String) -> Result<(), String> {
    modes::tablet_forget(&app, &uuid).await.map_err(|e| format!("{e:#}"))
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
    let _ = std::process::Command::new(OPENER).arg(p).spawn();
}

const OPENER: &str = if cfg!(windows) { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };

/// Connections → Shared files: everything received from other computers, newest first, with small image previews.
#[tauri::command]
async fn shared_files(app: Core<'_>) -> Result<Vec<serde_json::Value>, String> {
    let root = app.inbox_dir();
    tokio::task::spawn_blocking(move || shared_list(&root)).await.map_err(|e| e.to_string())
}

fn shared_list(root: &std::path::Path) -> Vec<serde_json::Value> {
    let mut out = vec![];
    // Received files land in <inbox>/<sender name>-<time>/...
    for batch in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let folder = batch.file_name().to_string_lossy().to_string();
        let from = folder.rsplit_once('-').map_or(folder.as_str(), |(n, _)| n).to_string();
        let mut stack = vec![batch.path()];
        while let Some(p) = stack.pop() {
            if p.is_dir() {
                stack.extend(std::fs::read_dir(&p).into_iter().flatten().flatten().map(|e| e.path()));
                continue;
            }
            let Ok(m) = p.metadata() else { continue };
            let when = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            out.push((when, p, m.len(), from.clone()));
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter()
        .take(120)
        .map(|(when, p, size, from)| {
            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let mime = match ext.as_str() { "png" => "image/png", "jpg" | "jpeg" => "image/jpeg", "gif" => "image/gif", "webp" => "image/webp", _ => "" };
            // Previews only for small images; bigger ones show an icon.
            let thumb = (!mime.is_empty() && size < 1_500_000).then(|| std::fs::read(&p).ok()).flatten().map(|b| format!("data:{mime};base64,{}", base64(&b)));
            serde_json::json!({"name": p.file_name().map(|n| n.to_string_lossy().to_string()), "path": p, "size": size, "from": from, "when": when, "thumb": thumb})
        })
        .collect()
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

/// Shared files: open one, or show it in its folder. Only files inside the received-files folder.
#[tauri::command]
fn open_shared(app: Core, path: String, reveal: bool) -> Result<(), String> {
    let p = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
    let root = std::fs::canonicalize(app.inbox_dir()).map_err(|e| e.to_string())?;
    if !p.starts_with(&root) {
        return Err("not a received file".into());
    }
    let mut c = std::process::Command::new(OPENER);
    if reveal && cfg!(windows) {
        c.arg(format!("/select,{}", p.display()));
    } else {
        c.arg(if reveal { p.parent().unwrap_or(&root).to_path_buf() } else { p });
    }
    c.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Files given with `bekdesk --send <files>` (right-click → Send with bekdesk), waiting for the user to pick computers.
static PENDING_SEND: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
fn queue_send(args: &[String]) -> bool {
    let Some(i) = args.iter().position(|a| a == "--send") else { return false };
    PENDING_SEND.lock().unwrap().extend(args[i + 1..].iter().cloned());
    true
}

#[tauri::command]
fn take_pending_send() -> Vec<String> {
    std::mem::take(&mut *PENDING_SEND.lock().unwrap())
}

/// Adds "Send with bekdesk" to the file manager's right-click menu (Windows: Send to; GNOME Files: Scripts).
fn add_send_menu() {
    let Ok(exe) = std::env::current_exe() else { return };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let Some(dir) = dirs::data_dir().map(|d| d.join(r"Microsoft\Windows\SendTo")) else { return };
        let lnk = dir.join("bekdesk (send to another PC).lnk");
        if lnk.exists() {
            return;
        }
        let q = |p: &std::path::Path| p.display().to_string().replace('\'', "''");
        let ps = format!("$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}'); $s.TargetPath='{}'; $s.Arguments='--send'; $s.IconLocation='{},0'; $s.Save()", q(&lnk), q(&exe), q(&exe));
        let _ = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &ps]).creation_flags(0x0800_0000).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        let Some(dir) = dirs::data_dir().map(|d| d.join("nautilus/scripts")) else { return };
        let f = dir.join("Send with bekdesk");
        let _ = std::fs::create_dir_all(&dir);
        if std::fs::write(&f, format!("#!/bin/sh\nexec \"{}\" --send \"$@\"\n", exe.display())).is_ok() {
            let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755));
        }
    }
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

/// Run by Sunshine when a stream starts (`--vdd-on <target file>`) or ends (`--vdd-off`), so bekdesk's virtual
/// screen exists only while a device is showing it. The target file holds "x y w h" saved by bekdesk.
fn vdd_command(args: &[String]) -> Option<i32> {
    match args.get(1).map(String::as_str)? {
        "--vdd-off" => {
            if let Some(n) = display::vdd_display() {
                let _ = display::set_mode(&n, 0, 0, 0, 0);
            }
            Some(0)
        }
        "--vdd-on" => {
            let target = PathBuf::from(args.get(2)?);
            let _ = std::fs::write(target.with_file_name("vdd_on.stamp"), "");
            let v: Vec<i64> = std::fs::read_to_string(&target).unwrap_or_default().split_whitespace().filter_map(|t| t.parse().ok()).collect();
            let (x, y, w, h) = if v.len() == 4 { (v[0] as i32, v[1] as i32, v[2] as u32, v[3] as u32) } else { (1920, 0, 1920, 1080) };
            // Use the size the device asked for when it can be shown pixel for pixel; keep the edge that touches this PC.
            let env = |k: &str| std::env::var(k).ok().and_then(|t| t.parse::<u32>().ok()).filter(|n| (480..=7680).contains(n));
            let (cw, ch) = (env("SUNSHINE_CLIENT_WIDTH").unwrap_or(w), env("SUNSHINE_CLIENT_HEIGHT").unwrap_or(h));
            let cx = if x < 0 { x + w as i32 - cw as i32 } else { x };
            let cy = if y < 0 { y + h as i32 - ch as i32 } else { y };
            let name = display::vdd_display()?;
            let ok = display::set_mode(&name, cw, ch, cx, cy).or_else(|_| display::set_mode(&name, w, h, x, y)).or_else(|_| display::set_mode(&name, 1920, 1080, x, y)).is_ok();
            Some(if ok { 0 } else { 1 })
        }
        _ => None,
    }
}

/// Writes any panic to logs/crash.log in the data folder, so a crash leaves something to read.
fn crash_log() {
    let base = std::env::var_os("BEKDESK_DATA").map(PathBuf::from).or_else(|| {
        let home = if cfg!(windows) { std::env::var_os("APPDATA").map(PathBuf::from) } else { std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")) };
        home.map(|h| h.join("io.github.pabibek9.bekdesk"))
    });
    std::panic::set_hook(Box::new(move |info| {
        if let Some(dir) = base.as_ref().map(|b| b.join("logs")) {
            let _ = std::fs::create_dir_all(&dir);
            let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let line = format!("{ts} v{} panic: {info}
", env!("CARGO_PKG_VERSION"));
            let _ = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crash.log")).and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
        }
    }));
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--install-vdd" {
        // Elevated helper path, reached only through an explicit UAC prompt.
        let r = display::install_elevated(std::path::Path::new(&args[2]));
        std::process::exit(if r.is_ok() { 0 } else { 1 });
    }
    // Started by Windows as the lock screen helper service.
    if args.get(1).map(String::as_str) == Some("--service") {
        bekdesk_core::service::run_service();
        return;
    }
    // Elevated (one UAC prompt): `--admin-setup <user SID> <deskflow folder> <driver .inf or ->` or `--uninstall-service`.
    if args.get(1).map(String::as_str) == Some("--admin-setup") && args.len() == 5 {
        let r = bekdesk_core::service::install(&args[2], std::path::Path::new(&args[3])).and_then(|()| {
            if args[4] != "-" && display::vdd_display().is_none() {
                display::install_elevated(std::path::Path::new(&args[4]))?;
            }
            Ok(())
        });
        let _ = std::fs::write(std::env::temp_dir().join("bekdesk-setup.log"), r.as_ref().err().map(|e| format!("{e:#}")).unwrap_or_default());
        std::process::exit(if r.is_ok() { 0 } else { 1 });
    }
    if args.get(1).map(String::as_str) == Some("--uninstall-service") {
        let r = bekdesk_core::service::uninstall();
        let _ = std::fs::write(std::env::temp_dir().join("bekdesk-setup.log"), r.as_ref().err().map(|e| format!("{e:#}")).unwrap_or_default());
        std::process::exit(if r.is_ok() { 0 } else { 1 });
    }
    // AppImages (Moonlight on Linux) normally need FUSE 2, which Fedora doesn't install by default; this runs them without.
    #[cfg(target_os = "linux")]
    std::env::set_var("APPIMAGE_EXTRACT_AND_RUN", "1");
    if let Some(code) = vdd_command(&args) {
        std::process::exit(code);
    }
    #[cfg(windows)]
    engines_die_with_us();
    crash_log();

    let mut builder = tauri::Builder::default();
    // Opening bekdesk again (e.g. from the app menu on Linux, where GNOME shows no tray icon) shows the running one.
    // A test copy with its own data folder (BEKDESK_DATA) may run alongside.
    if std::env::var_os("BEKDESK_DATA").is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if queue_send(&args) {
                let _ = app.emit("send-pending", ());
            }
            show(app)
        }));
    }
    builder
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--background"])))
        .setup(move |app| {
            queue_send(&args);
            add_send_menu();
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
            // Look for a new version a minute after starting, then every six hours.
            tauri::async_runtime::spawn(async {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                loop {
                    let _ = bekdesk_core::update::check().await;
                    tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
                }
            });
            let c = core.clone();
            std::thread::spawn(move || modes::pointer_watch(c));
            let (handle, c) = (app.handle().clone(), core.clone());
            tauri::async_runtime::spawn(async move {
                loop {
                    c.identify.notified().await;
                    let _ = flash_numbers(&handle);
                }
            });
            let (handle, c) = (app.handle().clone(), core.clone());
            tauri::async_runtime::spawn(async move {
                loop {
                    c.asked.notified().await;
                    let h = handle.clone();
                    let _ = handle.run_on_main_thread(move || {
                        show(&h);
                        if let Some(w) = h.get_webview_window("main") {
                            let _ = w.request_user_attention(Some(tauri::UserAttentionType::Critical));
                        }
                    });
                }
            });
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
                    // The edge drop zone window is off until it is checked on real computers (see docs/RELEASE_NOTES.md).
                    audio::sync(&c);
                    let awake = {
                        let let_sleep = c.cfg.lock().unwrap().let_sleep.clone();
                        c.sessions.lock().unwrap().iter().any(|(fp, x)| !x.origin && x.mode == Mode::Input && !let_sleep.contains(fp))
                    };
                    display::keep_awake(awake);
                }
            });

            let open = MenuItem::with_id(app, "open", "Open bekdesk", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            // No tray on desktops without one (GNOME without the AppIndicator extension): run without it.
            if let Err(e) = TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().ok_or("no icon")?)
                .tooltip("bekdesk")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => show(app),
                    "quit" => {
                        modes::restore_pointer();
                        app.exit(0)
                    }
                    _ => {}
                })
                .on_tray_icon_event(|t, e| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, .. } = e {
                        show(t.app_handle());
                    }
                })
                .build(app)
            {
                core.log(format!("no tray icon: {e}"));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![snapshot, set_mode, decide, set_caps, forget, unblock, save_layout, identify_screens, connect_address, diagnostics, make_primary, move_display, display_modes, change_display, open_display_settings, set_sound, set_hear, set_stay_awake, set_mic, shared_files, open_shared, take_pending_send, set_scroll, admin_setup, set_pointer, rescan, pair_device, block_device, open_url, quit_app, toggle_scroll_lock, device_details, check_update, install_update, drop_files, send_files, set_name, set_tablet, tablet_pin, tablet_decline, tablet_forget, autostart, open_folder])
        .build(tauri::generate_context!())
        .expect("error while building bekdesk")
        .run(|_, e| {
            // Closing the window keeps bekdesk running in the tray; only Quit (explicit exit code) ends it.
            if let RunEvent::ExitRequested { api, code: None, .. } = e {
                api.prevent_exit();
            }
        });
}
