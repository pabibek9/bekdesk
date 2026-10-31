//! Per-device connection modes. The device where the user picks a mode is the *origin*:
//! - Input: origin runs the Deskflow server (its keyboard/mouse), target runs the Deskflow client.
//!   Deskflow only listens on loopback; peers reach it through the bekdesk TLS tunnel.
//! - Mirror: origin runs a Sunshine host for its main display, target runs Moonlight.
//! - Extend: like Mirror, but Sunshine captures a virtual monitor the origin adds to its desktop.

use crate::app::{App, Mode, Session, SELF};
use crate::display;
use crate::engines::{ensure, moonlight_home, spawn, spawn_in};
use crate::net::{self, Req, DESKFLOW_PORT};
use anyhow::{bail, Context, Result};
use rand::Rng;
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::net::TcpListener;

/// Sunshine for Moonlight-only receivers (tablets) uses the standard port so Moonlight finds it.
const TABLET_PORT: u16 = 47989;

fn session(mode: Mode, origin: bool, ip: IpAddr, status: &str) -> Session {
    Session { mode, origin, peer_ip: ip, status: status.into(), since: SystemTime::now(), procs: vec![], tasks: vec![], respawn: None }
}

fn set_status(app: &App, fp: &str, status: impl Into<String>) {
    if let Some(s) = app.sessions.lock().unwrap().get_mut(fp) {
        s.status = status.into();
    }
}

fn fwd(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

// ---------- user actions ----------

/// Called when the user picks a mode for a device on this (origin) machine.
pub async fn set_mode(app: &Arc<App>, fp: &str, mode: Mode) -> Result<()> {
    app.note(fp, "");
    app.cfg.lock().unwrap().modes.insert(fp.to_string(), mode);
    app.save();
    // Already running in this direction: nothing to do. The same mode the other way round (that computer's
    // keyboard and mouse controlling this one) is switched over so this computer's are shared instead.
    let current = app.sessions.lock().unwrap().get(fp).map(|s| (s.mode, s.origin));
    if current == Some((mode, true)) && mode != Mode::Off {
        return Ok(());
    }
    if current.is_some() {
        let _ = net::request(app, fp, &Req::Stop).await;
        stop_local(app, fp, false);
    }
    if mode == Mode::Off {
        return Ok(());
    }
    let r = async {
        if !app.knows(fp) {
            net::pair(app, fp).await?;
        }
        start_origin(app, fp, mode).await
    }
    .await;
    if r.is_err() {
        app.cfg.lock().unwrap().modes.insert(fp.to_string(), Mode::Off);
        app.save();
    }
    r
}

/// Ends a session on this machine. `by_peer`: the other side ended it, so don't auto-restart.
pub fn stop_local(app: &Arc<App>, fp: &str, by_peer: bool) {
    let Some(s) = app.sessions.lock().unwrap().remove(fp) else { return };
    app.log(format!("{} session with {} ended", label(s.mode), app.peer_name(fp)));
    if s.origin && by_peer {
        app.cfg.lock().unwrap().modes.insert(fp.to_string(), Mode::Off);
        app.save();
    }
    if s.origin && s.mode == Mode::Extend {
        detach_virtual();
    }
    let input_origin = s.origin && s.mode == Mode::Input;
    drop(s);
    if input_origin {
        let app = app.clone();
        tokio::spawn(async move {
            if let Err(e) = restart_deskflow_server(&app).await {
                app.log(format!("deskflow server: {e:#}"));
            }
        });
    }
}

fn label(m: Mode) -> &'static str {
    match m {
        Mode::Input => "Share Input",
        Mode::Extend => "Extend Display",
        Mode::Mirror => "Mirror Display",
        Mode::Off => "Off",
    }
}

async fn start_origin(app: &Arc<App>, fp: &str, mode: Mode) -> Result<()> {
    let ip = app.peer(fp).and_then(|p| p.addrs.first().copied()).context("device is offline")?;
    match mode {
        Mode::Input => {
            ensure(app, "deskflow", fp).await?;
            if app.sessions.lock().unwrap().values().any(|s| !s.origin && s.mode == Mode::Input) {
                bail!("this computer is currently controlled by another computer's input");
            }
            app.sessions.lock().unwrap().insert(fp.into(), session(Mode::Input, true, ip, "Connecting"));
            let r = async {
                restart_deskflow_server(app).await?;
                net::request(app, fp, &Req::Start { mode, pin: String::new(), port: 0 }).await
            }
            .await;
            if let Err(e) = r {
                stop_local(app, fp, false);
                return Err(e);
            }
            set_status(app, fp, "Sharing keyboard, mouse, clipboard and files");
        }
        Mode::Mirror | Mode::Extend => {
            let slot = {
                let mut cfg = app.cfg.lock().unwrap();
                let next = cfg.slots.values().max().map_or(1, |m| m + 1);
                *cfg.slots.entry(fp.to_string()).or_insert(next)
            };
            app.save();
            let base = 48010 + (slot % 200) * 40;
            let output = if mode == Mode::Extend {
                if app.sessions.lock().unwrap().values().any(|s| s.origin && s.mode == Mode::Extend) || app.tablet_host.lock().unwrap().as_ref().is_some_and(|s| s.mode == Mode::Extend) {
                    bail!("this computer is already extending to another device (one virtual monitor for now)");
                }
                let res = app.peer(fp).and_then(|p| display::primary_size(&p.monitors)).unwrap_or((1920, 1080));
                Some(attach_virtual(app, fp, res, placement(app, fp, res)).await?)
            } else {
                None
            };
            let host = start_sunshine(app, fp, base, output.as_deref(), &app.peer_name(fp), false).await;
            let (child, args, creds) = match host {
                Ok(h) => h,
                Err(e) => {
                    if output.is_some() {
                        detach_virtual();
                    }
                    return Err(e);
                }
            };
            let pin = format!("{:04}", rand::thread_rng().gen_range(0..10000));
            let mut s = session(mode, true, ip, "Waiting for the receiver");
            s.procs.push(child);
            s.respawn = Some(args);
            s.tasks.push(tokio::spawn(post_pin(app.clone(), base, creds, pin.clone(), app.peer_name(fp), ip)));
            app.sessions.lock().unwrap().insert(fp.into(), s);
            if let Err(e) = net::request(app, fp, &Req::Start { mode, pin, port: base }).await {
                stop_local(app, fp, false);
                return Err(e);
            }
            set_status(app, fp, if mode == Mode::Extend { "Extending desktop" } else { "Mirroring main display" });
        }
        Mode::Off => {}
    }
    app.log(format!("{} → {}", label(mode), app.peer_name(fp)));
    Ok(())
}

/// Target side: the origin asked us to show its screen or accept its input.
pub async fn start_target(app: &Arc<App>, fp: &str, ip: IpAddr, mode: Mode, pin: String, port: u16) -> Result<()> {
    if mode == Mode::Off {
        stop_local(app, fp, true);
        return Ok(());
    }
    if let Some(old) = app.sessions.lock().unwrap().remove(fp) {
        drop(old);
    }
    match mode {
        Mode::Input => {
            if app.sessions.lock().unwrap().iter().any(|(_, s)| s.mode == Mode::Input) || app.deskflow_server.lock().unwrap().is_some() {
                bail!("{} is already in another Share Input session", app.name());
            }
            let exe = ensure(app, "deskflow", fp).await?;
            let local = TcpListener::bind(("127.0.0.1", 0)).await?;
            let lport = local.local_addr()?.port();
            let ini = app.dir.join("deskflow").join("client.ini");
            std::fs::create_dir_all(ini.parent().unwrap())?;
            std::fs::write(
                &ini,
                format!(
                    "[core]\ncoreMode=1\ncomputerName={}\nport={lport}\npreventSleep=false\n\n[client]\nremoteHost=127.0.0.1\n\n[security]\ntlsEnabled=false\ncheckPeerFingerprints=false\n",
                    App::screen_name(&app.id.fp)
                ),
            )?;
            let args = vec!["client".to_string(), "--new-instance".into(), "-s".into(), fwd(&ini)];
            let child = spawn(app, &exe, &args, "deskflow-client.log")?;
            let mut s = session(Mode::Input, false, ip, &format!("Controlled by {}", app.peer_name(fp)));
            s.procs.push(child);
            s.respawn = Some((exe, args, "deskflow-client.log".into()));
            let (app2, fp2) = (app.clone(), fp.to_string());
            s.tasks.push(tokio::spawn(async move {
                while let Ok((mut tcp, _)) = local.accept().await {
                    let (app3, fp3) = (app2.clone(), fp2.clone());
                    tokio::spawn(async move {
                        let _ = tcp.set_nodelay(true);
                        match net::request(&app3, &fp3, &Req::Tunnel).await {
                            Ok((mut tls, _)) => {
                                let _ = tokio::io::copy_bidirectional(&mut tcp, &mut tls).await;
                            }
                            Err(e) => app3.log(format!("input tunnel: {e:#}")),
                        }
                    });
                }
            }));
            app.sessions.lock().unwrap().insert(fp.into(), s);
        }
        Mode::Mirror | Mode::Extend => {
            let exe = ensure(app, "moonlight", fp).await?;
            let host = match ip {
                IpAddr::V6(v6) => format!("[{v6}]:{port}"),
                IpAddr::V4(v4) => format!("{v4}:{port}"),
            };
            let mut s = session(mode, false, ip, "Pairing display stream");
            let (app2, fp2) = (app.clone(), fp.to_string());
            s.tasks.push(tokio::spawn(async move {
                let pair = async {
                    let home = moonlight_home(&app2, &exe);
                    if !moonlight_paired(&exe, &home, &host).await {
                        // The pair window stays open after success, so watch for pairing and close it ourselves.
                        let mut c = spawn_in(&app2, &exe, &["pair".into(), host.clone(), "--pin".into(), pin], "moonlight-pair.log", &home)?;
                        let mut paired = false;
                        for _ in 0..40 {
                            tokio::time::sleep(Duration::from_secs(3)).await;
                            if moonlight_paired(&exe, &home, &host).await {
                                paired = true;
                                break;
                            }
                        }
                        let _ = c.start_kill();
                        if !paired {
                            bail!("display pairing did not complete");
                        }
                    }
                    // Tuned for desktop use with the least delay: stream at this screen's real size (sharp, no
                    // rescaling), V-Sync and frame pacing off (each adds up to a frame of latency), pointer maps
                    // 1:1 to the screen, and ~1.5x Moonlight's default bitrate so text stays crisp. Whether sound
                    // comes along is decided by the host (stream_audio).
                    let (w, h) = display::primary_size(&app2.monitors.lock().unwrap()).unwrap_or((1920, 1080));
                    let fps = display::stream_fps(display::primary_refresh().unwrap_or(60));
                    let args = [
                        "stream", &host, "Desktop", "--display-mode", "fullscreen", "--resolution", &format!("{w}x{h}"), "--fps", &fps.to_string(),
                        "--no-vsync", "--no-frame-pacing", "--absolute-mouse", "--keep-awake", "--bitrate", &(w * h * fps / 4000).clamp(10_000, 150_000).to_string(),
                    ]
                    .map(String::from)
                    .to_vec();
                    spawn_in(&app2, &exe, &args, "moonlight-stream.log", &home)
                }
                .await;
                match pair {
                    Ok(child) => {
                        if let Some(s) = app2.sessions.lock().unwrap().get_mut(&fp2) {
                            s.procs.push(child);
                            s.status = format!("Showing {}", app2.peer_name(&fp2));
                        }
                    }
                    Err(e) => {
                        app2.log(format!("display stream: {e:#}"));
                        set_status(&app2, &fp2, format!("Failed: {e}"));
                    }
                }
            }));
            app.sessions.lock().unwrap().insert(fp.into(), s);
        }
        Mode::Off => {}
    }
    app.log(format!("{} from {}", label(mode), app.peer_name(fp)));
    Ok(())
}

/// `Moonlight list <host>` prints the host's apps only once this client is paired with it.
async fn moonlight_paired(exe: &Path, home: &Path, host: &str) -> bool {
    let mut c = tokio::process::Command::new(exe);
    c.args(["list", host]).kill_on_drop(true).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).current_dir(home);
    matches!(tokio::time::timeout(Duration::from_secs(15), c.output()).await, Ok(Ok(o)) if String::from_utf8_lossy(&o.stdout).contains("Desktop"))
}

// ---------- Deskflow ----------

#[derive(Clone, Copy)]
struct Box2 {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

fn device_box(monitors: &str, pos: Option<crate::app::Pos>) -> Box2 {
    let (mut minx, mut miny, mut maxx, mut maxy) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for m in monitors.split(';') {
        let v: Vec<i64> = m.split(',').filter_map(|x| x.parse().ok()).collect();
        if v.len() == 4 {
            minx = minx.min(v[0]);
            miny = miny.min(v[1]);
            maxx = maxx.max(v[0] + v[2]);
            maxy = maxy.max(v[1] + v[3]);
        }
    }
    let (w, h) = if minx == i64::MAX { (192.0, 108.0) } else { ((maxx - minx) as f64 / 10.0, (maxy - miny) as f64 / 10.0) };
    let p = pos.unwrap_or_default();
    Box2 { x: p.x as f64, y: p.y as f64, w, h }
}

/// For each screen and direction, link to the nearest screen whose centre lies within that 90° cone.
pub fn links(screens: &[(String, f64, f64)]) -> Vec<(String, &'static str, String)> {
    let mut out = vec![];
    for (a, ax, ay) in screens {
        for (dir, ux, uy) in [("right", 1.0, 0.0), ("left", -1.0, 0.0), ("down", 0.0, 1.0), ("up", 0.0, -1.0)] {
            let best = screens
                .iter()
                .filter(|(b, _, _)| b != a)
                .filter_map(|(b, bx, by)| {
                    let (dx, dy) = (bx - ax, by - ay);
                    let along = dx * ux + dy * uy;
                    let across = (dx * uy - dy * ux).abs();
                    (along > 0.0 && along >= across).then(|| (b, dx * dx + dy * dy))
                })
                .min_by(|x, y| x.1.total_cmp(&y.1));
            if let Some((b, _)) = best {
                out.push((a.clone(), dir, b.clone()));
            }
        }
    }
    out
}

/// Default workspace placement: peers to the right of this computer, in discovery order.
pub fn default_positions(app: &App) -> HashMap<String, crate::app::Pos> {
    let cfg = app.cfg.lock().unwrap();
    let mut layout = cfg.layout.clone();
    let me = device_box(&app.monitors.lock().unwrap(), layout.get(SELF).copied());
    layout.entry(SELF.into()).or_default();
    let mut x = me.x + me.w + 4.0;
    let mut fps: Vec<String> = app.peers.lock().unwrap().keys().cloned().collect();
    fps.extend(cfg.trusted.keys().cloned());
    fps.sort();
    fps.dedup();
    for fp in fps {
        if !layout.contains_key(&fp) {
            let mon = app.peer(&fp).map(|p| p.monitors).unwrap_or_default();
            let b = device_box(&mon, None);
            layout.insert(fp, crate::app::Pos { x: x as i32, y: me.y as i32 });
            x += b.w + 4.0;
        }
    }
    layout
}

/// Deskflow 1.27 config: (links file, `[computer_N]` settings sections — screens live in the settings file).
fn server_config(app: &App) -> (String, String) {
    let layout = default_positions(app);
    let peers: Vec<String> = app.sessions.lock().unwrap().iter().filter(|(_, s)| s.origin && s.mode == Mode::Input).map(|(fp, _)| fp.clone()).collect();
    let me = device_box(&app.monitors.lock().unwrap(), layout.get(SELF).copied());
    let mut boxes = vec![(App::screen_name(&app.id.fp), me)];
    for fp in &peers {
        boxes.push((App::screen_name(fp), device_box(&app.peer(fp).map(|p| p.monitors).unwrap_or_default(), layout.get(fp).copied())));
    }
    let screens: Vec<(String, f64, f64)> = boxes.iter().map(|(n, b)| (n.clone(), b.x + b.w / 2.0, b.y + b.h / 2.0)).collect();
    let computers: String = screens.iter().enumerate().map(|(i, (n, _, _))| format!("[computer_{i}]\nname={n}\n\n")).collect();
    let bx = |n: &str| boxes.iter().find(|(m, _)| m == n).map(|(_, b)| *b).unwrap();
    let mut s = String::from("section: links\n");
    let l = links(&screens);
    for (n, _, _) in &screens {
        s += &format!("\t{n}:\n");
        for (_, dir, b) in l.iter().filter(|(a, _, _)| a == n) {
            // Cross only where the two screens actually overlap in the layout, so the pointer keeps its height.
            s += &match edge_range(dir, bx(n), bx(b)) {
                Some(((a0, a1), (b0, b1))) => format!("\t\t{dir}({a0},{a1}) = {b}({b0},{b1})\n"),
                None => format!("\t\t{dir} = {b}\n"),
            };
        }
    }
    s += &format!("end\n\nsection: options\n\tclipboardSharing = false\n\tkeystroke(Control+Alt+Home) = switchToScreen({})\nend\n", App::screen_name(&app.id.fp));
    (s, computers)
}

/// Percent ranges of `a`'s and `b`'s shared edge where they overlap, or None for whole edges.
fn edge_range(dir: &str, a: Box2, b: Box2) -> Option<((i32, i32), (i32, i32))> {
    let (a0, al, b0, bl) = if dir == "left" || dir == "right" { (a.y, a.h, b.y, b.h) } else { (a.x, a.w, b.x, b.w) };
    let (lo, hi) = (a0.max(b0), (a0 + al).min(b0 + bl));
    if hi <= lo || al <= 0.0 || bl <= 0.0 {
        return None;
    }
    let pct = |v: f64, start: f64, len: f64| (((v - start) / len) * 100.0).round().clamp(0.0, 100.0) as i32;
    let r = ((pct(lo, a0, al), pct(hi, a0, al)), (pct(lo, b0, bl), pct(hi, b0, bl)));
    (r != ((0, 100), (0, 100))).then_some(r)
}

pub async fn restart_deskflow_server(app: &Arc<App>) -> Result<()> {
    if let Some(mut c) = app.deskflow_server.lock().unwrap().take() {
        let _ = c.start_kill();
    }
    let any = app.sessions.lock().unwrap().values().any(|s| s.origin && s.mode == Mode::Input);
    if !any {
        return Ok(());
    }
    let exe = ensure(app, "deskflow", SELF).await?;
    let dir = app.dir.join("deskflow");
    std::fs::create_dir_all(&dir)?;
    let (conf, ini) = (dir.join("server.conf"), dir.join("server.ini"));
    let (links_conf, computers) = server_config(app);
    std::fs::write(&conf, links_conf)?;
    std::fs::write(
        &ini,
        format!(
            "[core]\ncoreMode=2\ncomputerName={}\nport={DESKFLOW_PORT}\ninterface=127.0.0.1\npreventSleep=false\n\n{computers}[server]\nenableClipboard=false\nexternalConfig=true\nexternalConfigFile={}\n\n[security]\ntlsEnabled=false\ncheckPeerFingerprints=false\n",
            App::screen_name(&app.id.fp),
            fwd(&conf)
        ),
    )?;
    tokio::time::sleep(Duration::from_millis(300)).await; // let the old server release the port
    let child = spawn(app, &exe, &["server".into(), "--new-instance".into(), "-s".into(), fwd(&ini)], "deskflow-server.log")?;
    *app.deskflow_server.lock().unwrap() = Some(child);
    Ok(())
}

// ---------- Sunshine / virtual display ----------

type Respawn = (PathBuf, Vec<String>, String);

async fn start_sunshine(app: &App, key: &str, port: u16, output: Option<&str>, receiver: &str, listen_only: bool) -> Result<(tokio::process::Child, Respawn, (String, String))> {
    let exe = ensure(app, "sunshine", key).await?;
    let dir = app.dir.join("sunshine").join(&key[..key.len().min(12)]);
    std::fs::create_dir_all(&dir)?;
    let creds_file = dir.join("bekdesk-creds.txt");
    let fresh = !creds_file.exists();
    let creds = match std::fs::read_to_string(&creds_file).ok().and_then(|s| s.split_once(':').map(|(a, b)| (a.to_string(), b.trim().to_string()))) {
        Some(c) => c,
        None => {
            let pass: String = rand::thread_rng().sample_iter(&rand::distributions::Alphanumeric).take(24).map(char::from).collect();
            let c = ("bekdesk".to_string(), pass);
            std::fs::write(&creds_file, format!("{}:{}", c.0, c.1))?;
            c
        }
    };
    let _ = std::fs::remove_file(dir.join("sunshine.log"));
    let state = fwd(&dir.join("sunshine_state.json"));
    let apps = dir.join("apps.json");
    // A listener (tablet hosting switched off) offers no apps: Moonlight devices can find it and pair, but not stream.
    let mut desktop = serde_json::json!({"name": "Desktop", "image-path": "desktop.png"});
    if output.is_some() {
        // The virtual screen exists only while a device streams it: Sunshine runs these when a stream starts/ends.
        let me = std::env::current_exe()?.display().to_string();
        let target = app.dir.join("vdd_target.txt").display().to_string();
        desktop["prep-cmd"] = serde_json::json!([{"do": format!("\"{me}\" --vdd-on \"{target}\""), "undo": format!("\"{me}\" --vdd-off"), "elevated": false}]);
    }
    let list = if listen_only { serde_json::json!([]) } else { serde_json::json!([desktop]) };
    std::fs::write(&apps, serde_json::json!({"env": {}, "apps": list}).to_string())?;
    // Sunshine's identity certificate defaults to a folder next to sunshine.exe, which an engine update or
    // reinstall wipes, and a new identity makes every Moonlight client ask for a PIN again. Keep it with our
    // data instead, carrying over one an earlier version left in the engine folder.
    let (pkey, cert) = (dir.join("cakey.pem"), dir.join("cacert.pem"));
    if !pkey.exists() {
        let old = exe.parent().map(|d| d.join("config").join("credentials")).unwrap_or_default();
        if std::fs::copy(old.join("cakey.pem"), &pkey).is_err() || std::fs::copy(old.join("cacert.pem"), &cert).is_err() {
            let _ = std::fs::remove_file(&pkey);
        }
    }
    let conf = format!(
        "sunshine_name = bekdesk {}{}\nport = {port}\nfile_state = {state}\ncredentials_file = {state}\npkey = {}\ncert = {}\nfile_apps = {}\nlog_path = {}\norigin_web_ui_allowed = pc\nupnp = disabled\n",
        app.name().replace('\n', " "),
        if receiver.is_empty() { String::new() } else { format!(" for {}", receiver.replace('\n', " ")) },
        fwd(&pkey),
        fwd(&cert),
        fwd(&apps),
        fwd(&dir.join("sunshine.log"))
    ) + if app.cfg.lock().unwrap().sound.get(key).copied().unwrap_or(false) { "" } else { "stream_audio = disabled\n" };
    let conf_path = dir.join("sunshine.conf");
    // The virtual monitor's Sunshine id is stable; remembering it lets Extend start Sunshine once instead of twice.
    let id_file = dir.join("output_id.txt");
    let cached = output.and_then(|_| std::fs::read_to_string(&id_file).ok()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    std::fs::write(&conf_path, match &cached { Some(id) => format!("{conf}output_name = {id}\n"), None => conf.clone() })?;
    if fresh {
        let mut c = spawn(app, &exe, &[fwd(&conf_path), "--creds".into(), creds.0.clone(), creds.1.clone()], "sunshine-creds.log")?;
        let _ = tokio::time::timeout(Duration::from_secs(30), c.wait()).await;
    }
    let args = vec![fwd(&conf_path)];
    let log = format!("sunshine-{}.log", &key[..key.len().min(12)]);
    let mut child = spawn(app, &exe, &args, &log)?;
    if let Some(o) = output {
        // Windows Sunshine selects a monitor by its device_id GUID, not `\\.\DISPLAYn`; an unknown name
        // silently falls back to the primary display (which looks like Mirror). Sunshine lists the GUIDs in
        // its own log at startup, so check the remembered id there and restart only if it changed.
        let sun_log = dir.join("sunshine.log");
        let mut id = None;
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            id = std::fs::read_to_string(&sun_log).ok().and_then(|t| display::device_id_from_log(&t, o));
            if id.is_some() {
                break;
            }
        }
        let id = id.context("Sunshine did not list the virtual monitor, so it can't be extended onto it (see sunshine.log)")?;
        if cached.as_deref() != Some(id.as_str()) {
            let _ = child.kill().await;
            std::fs::write(&conf_path, format!("{conf}output_name = {id}\n"))?;
            let _ = std::fs::write(&id_file, &id);
            child = spawn(app, &exe, &args, &log)?;
        }
        // Sunshine knows the screen now; hide it until a device actually connects (prep-cmd brings it back).
        detach_virtual();
    }
    Ok((child, (exe, args, log), creds))
}

/// (name of a paired Moonlight device, streaming) for the Sunshine instance run for `key`: paired once a client
/// has been accepted (state file), streaming while the last "CLIENT CONNECTED" in its log has no later "CLIENT DISCONNECTED".
pub fn stream_state(app: &App, key: &str) -> (Option<String>, bool) {
    use std::io::{Read, Seek, SeekFrom};
    let dir = app.dir.join("sunshine").join(&key[..key.len().min(12)]);
    let names: Vec<String> = moonlight_devices(app, key).into_iter().map(|d| d.1).collect();
    let paired = (!names.is_empty()).then(|| names.join(", "));
    let streaming = std::fs::File::open(dir.join("sunshine.log"))
        .ok()
        .and_then(|mut f| {
            let len = f.metadata().ok()?.len();
            f.seek(SeekFrom::Start(len.saturating_sub(64 * 1024))).ok()?;
            let mut b = vec![];
            f.read_to_end(&mut b).ok()?;
            Some(String::from_utf8_lossy(&b).into_owned())
        })
        .is_some_and(|t| streaming_in_log(&t));
    (paired, streaming)
}

/// Moonlight devices paired with the Sunshine instance run for `key`, as (uuid, name).
pub fn moonlight_devices(app: &App, key: &str) -> Vec<(String, String)> {
    paired_entries(app, key).into_iter().map(|(uuid, name, _)| (uuid, name)).collect()
}

/// (uuid, name, certificate) of every paired entry in Sunshine's state file, oldest first.
fn paired_entries(app: &App, key: &str) -> Vec<(String, String, String)> {
    let path = app.dir.join("sunshine").join(&key[..key.len().min(12)]).join("sunshine_state.json");
    let v: serde_json::Value = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let s = |d: &serde_json::Value, k: &str| d[k].as_str().unwrap_or("").to_string();
    v["root"]["named_devices"].as_array().into_iter().flatten().map(|d| (s(d, "uuid"), s(d, "name"), s(d, "cert"))).collect()
}

/// Sunshine adds a new entry every time the same device pairs again (same certificate, new uuid), so one tablet
/// shows up under several names. Returns the uuids of all but the newest entry per certificate.
fn duplicate_uuids(entries: &[(String, String, String)]) -> Vec<String> {
    entries.iter().enumerate().filter(|(i, e)| entries[i + 1..].iter().any(|later| later.2 == e.2)).map(|(_, e)| e.0.clone()).collect()
}

fn streaming_in_log(t: &str) -> bool {
    t.rfind("CLIENT CONNECTED") > t.rfind("CLIENT DISCONNECTED")
}

/// Submit a PIN for a pairing request waiting in a local Sunshine instance (2026 API: GET lists
/// pending requests, POST answers one by `pairing_id`). Prefers the request from `from`, else the only one.
async fn submit_pin(port: u16, creds: &(String, String), pin: &str, name: &str, from: Option<IpAddr>) -> Result<bool> {
    let client = reqwest::Client::builder().danger_accept_invalid_certs(true).timeout(Duration::from_secs(5)).build()?;
    let url = format!("https://127.0.0.1:{}/api/pin", port + 1);
    let list: serde_json::Value = client.get(&url).basic_auth(&creds.0, Some(&creds.1)).send().await?.json().await?;
    let pending = list["pairings"].as_array().cloned().unwrap_or_default();
    let pick = from
        .and_then(|ip| pending.iter().find(|p| p["address"].as_str().is_some_and(|a| a.contains(&ip.to_string()))))
        .or(if pending.len() == 1 { pending.first() } else { None });
    let Some(id) = pick.and_then(|p| p["id"].as_str()) else { return Ok(false) };
    // Use the device's own name when Moonlight sends one ("roth" is the protocol's placeholder).
    let asked = pick.and_then(|p| p["name"].as_str()).filter(|n| !n.is_empty() && *n != "roth");
    let name = asked.unwrap_or(name);
    let r: serde_json::Value = client.post(&url).basic_auth(&creds.0, Some(&creds.1)).json(&serde_json::json!({"pairing_id": id, "pin": pin, "name": name})).send().await?.json().await?;
    Ok(r["status"].as_bool() == Some(true) || r["status"].as_str() == Some("true"))
}

/// Answer the receiver's pairing request once it shows up.
async fn post_pin(app: Arc<App>, port: u16, creds: (String, String), pin: String, name: String, from: IpAddr) {
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        if let Ok(true) = submit_pin(port, &creds, &pin, &name, Some(from)).await {
            app.log(format!("display pairing with {name} accepted"));
            return;
        }
    }
}

/// Calls the tablet host's Sunshine web API (loopback only, with bekdesk's own credentials).
async fn tablet_api(app: &App, method: reqwest::Method, path: &str, body: Option<serde_json::Value>) -> Result<serde_json::Value> {
    let creds = std::fs::read_to_string(app.dir.join("sunshine").join("tablet").join("bekdesk-creds.txt")).context("tablets and phones aren't set up yet")?;
    let (u, p) = creds.split_once(':').context("bad credentials file")?;
    let client = reqwest::Client::builder().danger_accept_invalid_certs(true).timeout(Duration::from_secs(5)).build()?;
    let mut r = client.request(method, format!("https://127.0.0.1:{}/api/{path}", TABLET_PORT + 1)).basic_auth(u, Some(p.trim()));
    if let Some(b) = body {
        r = r.json(&b);
    }
    Ok(r.send().await?.json().await?)
}

fn api_ok(v: &serde_json::Value) -> bool {
    v["status"].as_bool() == Some(true) || v["status"].as_str() == Some("true")
}

/// Answer a Moonlight device's pairing request with the PIN it shows, under the name the user gave it.
pub async fn tablet_pin(app: &App, id: &str, pin: &str, name: &str) -> Result<()> {
    if pin.len() != 4 || !pin.chars().all(|c| c.is_ascii_digit()) {
        bail!("PIN must be 4 digits");
    }
    let name: String = name.trim().chars().filter(|c| !c.is_control()).take(40).collect();
    if name.is_empty() {
        bail!("Give the device a name, e.g. Galaxy Tab");
    }
    let r = tablet_api(app, reqwest::Method::POST, "pin", Some(serde_json::json!({"pairing_id": id, "pin": pin, "name": name}))).await?;
    app.moonlight_pending.lock().unwrap().remove(id);
    if !api_ok(&r) {
        bail!("Pairing failed: the PIN didn't match. Tap this computer in Moonlight again and type the new PIN");
    }
    app.log(format!("paired Moonlight device {name}"));
    remove_duplicates(app).await;
    Ok(())
}

/// Turn down a pairing request (also clears one Moonlight left behind, which otherwise blocks new attempts).
pub async fn tablet_decline(app: &App, id: &str) -> Result<()> {
    tablet_api(app, reqwest::Method::DELETE, "pin", Some(serde_json::json!({"pairing_id": id}))).await?;
    app.moonlight_pending.lock().unwrap().remove(id);
    Ok(())
}

/// Unpair a Moonlight device; it has to pair with a PIN again to connect.
pub async fn tablet_forget(app: &App, uuid: &str) -> Result<()> {
    if !api_ok(&tablet_api(app, reqwest::Method::POST, "clients/unpair", Some(serde_json::json!({"uuid": uuid}))).await?) {
        bail!("Sunshine didn't remove that device");
    }
    Ok(())
}

async fn remove_duplicates(app: &App) {
    for uuid in duplicate_uuids(&paired_entries(app, "tablet")) {
        if let Err(e) = tablet_forget(app, &uuid).await {
            app.log(format!("removing duplicate Moonlight entry: {e:#}"));
        }
    }
}

/// Pairing requests waiting in the tablet host, kept for the UI. Sunshine keeps a request after Moonlight has given
/// up on it and then answers every new attempt with "409 pairing session already exists", so old ones are cancelled.
async fn refresh_pending(app: &App) {
    if app.tablet_host.lock().unwrap().is_none() {
        app.moonlight_pending.lock().unwrap().clear();
        return;
    }
    let Ok(v) = tablet_api(app, reqwest::Method::GET, "pin", None).await else { return };
    let list = v["pairings"].as_array().cloned().unwrap_or_default();
    let mut stale = vec![];
    {
        let mut p = app.moonlight_pending.lock().unwrap();
        p.retain(|id, _| list.iter().any(|x| x["id"].as_str() == Some(id.as_str())));
        for x in list {
            let Some(id) = x["id"].as_str().map(String::from) else { continue };
            if p.entry(id.clone()).or_insert((x, Instant::now())).1.elapsed() > Duration::from_secs(120) {
                stale.push(id);
            }
        }
    }
    // Moonlight retrying leaves several requests from the same device; only its newest one can still be answered.
    {
        let p = app.moonlight_pending.lock().unwrap();
        for (id, (v, seen)) in p.iter() {
            let addr = v["address"].as_str().unwrap_or("");
            if !addr.is_empty() && p.iter().any(|(o, (w, t))| o != id && w["address"].as_str() == Some(addr) && t > seen) {
                stale.push(id.clone());
            }
        }
    }
    stale.dedup();
    for id in stale {
        let _ = tablet_decline(app, &id).await;
    }
    remove_duplicates(app).await;
}

/// Where the virtual monitor goes relative to this computer's primary display, from the Workspace layout.
fn placement(app: &App, fp: &str, (w, h): (u32, u32)) -> (i32, i32) {
    let layout = default_positions(app);
    let mon = app.monitors.lock().unwrap().clone();
    let (pw, ph) = display::primary_size(&mon).unwrap_or((1920, 1080));
    let me = device_box(&mon, layout.get(SELF).copied());
    let other = device_box("", layout.get(fp).copied());
    let (dx, dy) = (other.x - me.x, other.y - me.y);
    if dx.abs() >= dy.abs() {
        if dx >= 0.0 { (pw as i32, 0) } else { (-(w as i32), 0) }
    } else if dy >= 0.0 {
        (0, ph as i32)
    } else {
        (0, -(h as i32))
    }
}

async fn attach_virtual(app: &App, key: &str, (w, h): (u32, u32), (x, y): (i32, i32)) -> Result<String> {
    let inf = ensure(app, "vdd", key).await?;
    // The stock mode list is 16:9 only; ship 16:10/tablet sizes and the receiver's own size with the driver,
    // so the virtual screen matches the receiver pixel for pixel (anything else is scaled and looks blurry).
    let settings = inf.with_file_name("vdd_settings.xml");
    let mut modes = display::EXTRA_MODES.to_vec();
    modes.push((w, h));
    if let Ok(xml) = std::fs::read_to_string(&settings) {
        std::fs::write(&settings, display::with_modes(&xml, &modes))?;
    }
    // Windows sometimes makes a newly attached monitor the main one; this computer's screen must stay main.
    let main = display::primary_name();
    let name = attach_inner(app, key, &inf, settings, (w, h), (x, y)).await?;
    if let Some(m) = main.filter(|m| display::primary_name().as_ref() != Some(m)) {
        if let Err(e) = display::set_primary(&m) {
            app.log(format!("keeping the main display: {e:#}"));
        }
    }
    remember_virtual(app);
    Ok(name)
}

/// Saves the virtual screen's current "x y w h" for `bekdesk --vdd-on`, and marks it as just attached.
fn remember_virtual(app: &App) {
    if let Some((x, y, w, h)) = display::vdd_rect() {
        let _ = std::fs::write(app.dir.join("vdd_target.txt"), format!("{x} {y} {w} {h}"));
    }
    let _ = std::fs::write(app.dir.join("vdd_on.stamp"), "");
}

/// The virtual screen is shown only while a device streams it. Anything else that turns it on (Win+P "Extend",
/// plugging in an HDMI monitor, bekdesk having crashed mid-stream) would leave an invisible screen that windows
/// get lost on, so it is switched off again. Only bekdesk's own virtual screen is touched, never real ones.
fn hide_unused_virtual(app: &Arc<App>) {
    if !display::vdd_attached() {
        return;
    }
    // Just attached (a stream is starting): give the device time to connect.
    let fresh = std::fs::metadata(app.dir.join("vdd_on.stamp")).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|d| d < Duration::from_secs(25));
    if fresh {
        return;
    }
    let mut keys: Vec<String> = app.sessions.lock().unwrap().iter().filter(|(_, s)| s.origin && s.mode == Mode::Extend).map(|(fp, _)| fp.clone()).collect();
    if app.tablet_host.lock().unwrap().as_ref().is_some_and(|s| s.mode == Mode::Extend) {
        keys.push("tablet".into());
    }
    if keys.iter().any(|k| stream_state(app, k).1) {
        return;
    }
    detach_virtual();
    app.log("virtual screen was on with nothing connected; turned it off");
    // A paused stream would resume without re-running the attach command; end it so the next connect attaches again.
    for k in keys {
        let app = app.clone();
        tokio::spawn(async move {
            let _ = close_stream(&app, &k).await;
        });
    }
}

/// Ends the running stream of the Sunshine instance for `key` (its undo command then runs).
async fn close_stream(app: &App, key: &str) -> Result<()> {
    let dir = app.dir.join("sunshine").join(&key[..key.len().min(12)]);
    let conf = std::fs::read_to_string(dir.join("sunshine.conf"))?;
    let port: u16 = conf.lines().find_map(|l| l.strip_prefix("port = ")?.trim().parse().ok()).context("no port")?;
    let creds = std::fs::read_to_string(dir.join("bekdesk-creds.txt"))?;
    let (u, p) = creds.split_once(':').context("bad credentials file")?;
    let client = reqwest::Client::builder().danger_accept_invalid_certs(true).timeout(Duration::from_secs(5)).build()?;
    client.post(format!("https://127.0.0.1:{}/api/apps/close", port + 1)).basic_auth(u, Some(p.trim())).send().await?;
    Ok(())
}

async fn attach_inner(app: &App, key: &str, inf: &Path, settings: PathBuf, (w, h): (u32, u32), (x, y): (i32, i32)) -> Result<String> {
    app.note(key, "Setting up the virtual display (approve the Windows prompt)");
    let inf2 = inf.to_path_buf();
    let r = tokio::task::spawn_blocking(move || display::ensure_installed(&inf2)).await?;
    app.note(key, "");
    r?;
    let mut name = display::vdd_display().context("virtual display not found")?;
    if display::set_mode(&name, w, h, x, y).is_ok() {
        return Ok(name);
    }
    // Installed by an older bekdesk without this size: install the new list once (one Windows prompt).
    let installed = std::fs::read_to_string(r"C:\VirtualDisplayDriver\vdd_settings.xml").unwrap_or_default();
    if !display::has_mode(&installed, (w, h)) && settings.exists() {
        app.note(key, "Adding this screen size to the virtual display (approve the Windows prompt)");
        let scratch = app.dir.clone();
        let r = tokio::task::spawn_blocking(move || display::update_modes(&settings, &scratch)).await?;
        app.note(key, "");
        match r {
            Ok(()) => {
                name = display::vdd_display().context("virtual display not found")?;
                if display::set_mode(&name, w, h, x, y).is_ok() {
                    return Ok(name);
                }
            }
            Err(e) => app.log(format!("virtual display sizes: {e:#}")),
        }
    }
    app.log(format!("virtual display has no {w}x{h} mode; using 1920x1080 (the picture will be scaled)"));
    if let Err(e) = display::set_mode(&name, 1920, 1080, x, y) {
        detach_virtual();
        return Err(e);
    }
    Ok(name)
}

/// Move the virtual monitor to where the Manage Screens layout now puts the device it extends to.
pub async fn reposition_virtual(app: &Arc<App>) {
    let (key, res) = {
        let s = app.sessions.lock().unwrap();
        if let Some((fp, _)) = s.iter().find(|(_, s)| s.origin && s.mode == Mode::Extend) {
            (fp.clone(), app.peer(fp).and_then(|p| display::primary_size(&p.monitors)).unwrap_or((1920, 1080)))
        } else if app.tablet_host.lock().unwrap().as_ref().is_some_and(|s| s.mode == Mode::Extend) {
            ("tablet".to_string(), display::parse_res(&app.cfg.lock().unwrap().tablet_res).unwrap_or((1920, 1200)))
        } else {
            return;
        }
    };
    let (x, y) = placement(app, &key, res);
    let _ = tokio::task::spawn_blocking(move || {
        let name = display::vdd_display()?;
        // same fallback as attach_virtual: the driver only offers some sizes
        display::set_mode(&name, res.0, res.1, x, y).or_else(|_| display::set_mode(&name, 1920, 1080, x, y)).ok()
    })
    .await;
    remember_virtual(app);
    keep_main_display(app);
}

fn detach_virtual() {
    if let Some(name) = display::vdd_display() {
        let _ = display::set_mode(&name, 0, 0, 0, 0);
    }
}

// When the Off-mode listener last failed to start; it is retried after a minute instead of every tick.
static LISTEN_FAILED: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);

async fn ensure_tablet_host(app: &Arc<App>) {
    let want = app.cfg.lock().unwrap().tablet;
    let have = app.tablet_host.lock().unwrap().as_ref().map(|s| s.mode);
    if have == Some(want) {
        return;
    }
    // While Off, a listener runs so Moonlight devices can find this computer and ask to pair. It has no apps, so
    // nothing is streamed until the user picks Extend or Duplicate. Started only once Sunshine is downloaded.
    if want == Mode::Off
        && have.is_none()
        && (!crate::engines::installed(app)["sunshine"]["installed"].as_bool().unwrap_or(false)
            || LISTEN_FAILED.lock().unwrap().is_some_and(|t| t.elapsed() < Duration::from_secs(60)))
    {
        return;
    }
    let old = app.tablet_host.lock().unwrap().take();
    if let Some(mut old) = old {
        if old.mode == Mode::Extend {
            detach_virtual();
        }
        // Wait for the old Sunshine to exit so the new one can take the same ports.
        for c in old.procs.iter_mut() {
            let _ = c.kill().await;
        }
    }
    let r = async {
        let output = if want == Mode::Extend {
            if app.sessions.lock().unwrap().values().any(|s| s.origin && s.mode == Mode::Extend) {
                bail!("already extending to another device");
            }
            let res = display::parse_res(&app.cfg.lock().unwrap().tablet_res).unwrap_or((1920, 1200));
            Some(attach_virtual(app, "tablet", res, placement(app, "tablet", res)).await?)
        } else {
            None
        };
        let (child, respawn, _) = start_sunshine(app, "tablet", TABLET_PORT, output.as_deref(), "", want == Mode::Off).await?;
        let status = if want == Mode::Off { "Tablets and phones with Moonlight can find this computer" } else { "Ready — open Moonlight on the tablet" };
        let mut s = session(want, true, IpAddr::from([0, 0, 0, 0]), status);
        s.procs.push(child);
        s.respawn = Some(respawn);
        *app.tablet_host.lock().unwrap() = Some(s);
        Ok(())
    }
    .await;
    if let Err(e) = r {
        if want == Mode::Off {
            *LISTEN_FAILED.lock().unwrap() = Some(Instant::now());
            app.log(format!("couldn't listen for Moonlight devices: {e:#}"));
            return;
        }
        app.note("tablet", format!("{e:#}"));
        app.cfg.lock().unwrap().tablet = Mode::Off;
        app.save();
    }
}

// ---------- pointer speed ----------

// Pointer speed the user had before bekdesk changed it (0 = bekdesk hasn't changed it).
static POINTER_BASE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Windows has one pointer speed for the whole computer. A tablet or another laptop shows a different number of
/// pixels per centimetre, so the same hand movement feels slower or faster there. While the pointer is on such a
/// device (the virtual screen for Extend, or another computer for Share Input), use that device's speed setting.
pub fn pointer_watch(app: Arc<App>) {
    if display::mouse_speed().is_none() {
        return; // not Windows
    }
    let (mut vdd, mut vdd_at) = (None, Instant::now() - Duration::from_secs(5));
    let (mut remote, mut log_len, mut n) = (None::<String>, 0u64, 0u32);
    let mut applied = None;
    loop {
        std::thread::sleep(Duration::from_millis(40));
        n = n.wrapping_add(1);
        if vdd_at.elapsed() > Duration::from_secs(1) {
            vdd = display::vdd_rect();
            vdd_at = Instant::now();
        }
        if n % 4 == 0 {
            remote = deskflow_target(&app, &mut log_len, remote);
        }
        let on_vdd = || {
            let ((x, y), (vx, vy, vw, vh)) = (display::cursor_pos()?, vdd?);
            (x >= vx && y >= vy && x < vx + vw as i32 && y < vy + vh as i32).then(|| extend_key(&app)).flatten()
        };
        let key = remote.clone().or_else(on_vdd);
        let factor = key.and_then(|k| app.cfg.lock().unwrap().pointer.get(&k).copied()).unwrap_or(1.0);
        let want = ((factor - 1.0).abs() > 0.01).then_some(factor);
        if want != applied {
            match want {
                Some(f) => {
                    if POINTER_BASE.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                        POINTER_BASE.store(display::mouse_speed().unwrap_or(10), std::sync::atomic::Ordering::Relaxed);
                    }
                    display::set_mouse_speed(display::speed_for(POINTER_BASE.load(std::sync::atomic::Ordering::Relaxed), f));
                }
                None => restore_pointer(),
            }
            applied = want;
        }
    }
}

/// Put back the user's own pointer speed (when the pointer comes home, and when bekdesk quits).
pub fn restore_pointer() {
    let base = POINTER_BASE.swap(0, std::sync::atomic::Ordering::Relaxed);
    if base != 0 {
        display::set_mouse_speed(base);
    }
}

/// The device the virtual screen belongs to ("tablet" or a peer fingerprint).
fn extend_key(app: &App) -> Option<String> {
    if app.tablet_host.lock().unwrap().as_ref().is_some_and(|s| s.mode == Mode::Extend) {
        return Some("tablet".into());
    }
    app.sessions.lock().unwrap().iter().find(|(_, s)| s.origin && s.mode == Mode::Extend).map(|(fp, _)| fp.clone())
}

/// Which peer the Deskflow server says the pointer is on now (None = this computer). Re-reads the log only when it grew.
fn deskflow_target(app: &App, log_len: &mut u64, current: Option<String>) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let peers: Vec<String> = app.sessions.lock().unwrap().iter().filter(|(_, s)| s.origin && s.mode == Mode::Input).map(|(fp, _)| fp.clone()).collect();
    if peers.is_empty() {
        return None;
    }
    let mut f = std::fs::File::open(app.dir.join("logs").join("deskflow-server.log")).ok()?;
    let len = f.metadata().ok()?.len();
    if len == *log_len {
        return current;
    }
    *log_len = len;
    f.seek(SeekFrom::Start(len.saturating_sub(8 * 1024))).ok()?;
    let mut b = vec![];
    f.read_to_end(&mut b).ok()?;
    let to = switched_to(&String::from_utf8_lossy(&b))?.to_string();
    peers.into_iter().find(|fp| App::screen_name(fp) == to)
}

/// Target of the last `switch from "A" to "B" at x,y` line in a Deskflow server log.
fn switched_to(log: &str) -> Option<&str> {
    let line = &log[log.rfind("switch from \"")?..];
    let rest = &line[line.find("\" to \"")? + 6..];
    rest.get(..rest.find('"')?)
}

// ---------- supervision ----------

fn check(app: &Arc<App>, fp: &str, s: &mut Session) -> Option<bool> {
    let dead = s.procs.iter_mut().any(|p| matches!(p.try_wait(), Ok(Some(_))));
    if !dead {
        return None;
    }
    if let Some((exe, args, log)) = &s.respawn {
        app.log(format!("restarting {} for {}", exe.file_name().unwrap_or_default().to_string_lossy(), fp.get(..12).unwrap_or(fp)));
        s.procs.clear();
        if let Ok(c) = spawn(app, exe, args, log) {
            s.procs.push(c);
        }
        return Some(false);
    }
    Some(true) // viewer closed: session over
}

// This computer's own main display, remembered so it can be given back if Windows moves the role to the virtual screen.
static MAIN_DISPLAY: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Windows can make the virtual screen the main display when it's attached or moved (e.g. dragging the tablet
/// below this PC in Manage Screens), which sends the taskbar and desktop icons to the tablet. Give it back.
fn keep_main_display(app: &App) {
    let Some(primary) = display::primary_name() else { return };
    let mut main = MAIN_DISPLAY.lock().unwrap();
    if display::vdd_display().as_ref() != Some(&primary) {
        *main = Some(primary);
        return;
    }
    // Started with the virtual screen already main (e.g. after an update): use this computer's own screen.
    // After a failure `main` is cleared to "" so it isn't retried every tick.
    let target = match main.clone() {
        Some(m) if m.is_empty() => return,
        Some(m) => Some(m),
        None => display::real_display(),
    };
    if let Some(m) = target {
        match display::set_primary(&m) {
            Ok(()) => app.log("Windows made the virtual screen the main display; switched it back"),
            Err(e) => {
                app.log(format!("keeping {m} as the main display: {e:#}"));
                *main = Some(String::new());
            }
        }
    }
}

pub async fn tick(app: &Arc<App>) {
    keep_main_display(app);
    hide_unused_virtual(app);
    // Restart crashed engines; a closed viewer ends its session and tells the origin.
    let mut ended = vec![];
    {
        let mut sessions = app.sessions.lock().unwrap();
        for (fp, s) in sessions.iter_mut() {
            if check(app, fp, s) == Some(true) {
                ended.push(fp.clone());
            }
        }
    }
    if let Some(s) = app.tablet_host.lock().unwrap().as_mut() {
        check(app, "tablet", s);
    }
    for fp in ended {
        stop_local(app, &fp, false);
        let app = app.clone();
        tokio::spawn(async move {
            let _ = net::request(&app, &fp, &Req::Stop).await;
        });
    }
    let server_dead = app.deskflow_server.lock().unwrap().as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(Some(_))));
    if server_dead {
        // macOS refuses to run the input server until it's allowed under Accessibility; say so instead of looping.
        let log = std::fs::read_to_string(app.dir.join("logs").join("deskflow-server.log")).unwrap_or_default();
        if log.to_lowercase().contains("accessibility") {
            for fp in app.sessions.lock().unwrap().iter().filter(|(_, s)| s.origin && s.mode == Mode::Input).map(|(f, _)| f.clone()).collect::<Vec<_>>() {
                app.note(&fp, "macOS needs permission: System Settings → Privacy & Security → Accessibility (and Input Monitoring) → allow deskflow-core, then choose Share Input again.");
            }
        }
        let _ = restart_deskflow_server(app).await;
    }
    // Origin sessions whose peer vanished (sleep, reboot, network change) are dropped and restored below.
    let gone: Vec<String> = {
        let peers = app.peers.lock().unwrap();
        let stale = app.stale.lock().unwrap();
        app.sessions.lock().unwrap().iter().filter(|(fp, s)| s.origin && (!peers.contains_key(*fp) || stale.contains(*fp))).map(|(fp, _)| fp.clone()).collect()
    };
    for fp in gone {
        if let Some(s) = app.sessions.lock().unwrap().remove(&fp) {
            if s.mode == Mode::Extend {
                detach_virtual();
            }
            if s.mode == Mode::Input {
                let a = app.clone();
                tokio::spawn(async move { restart_deskflow_server(&a).await });
            }
        }
        app.note(&fp, "Waiting for device to come back");
    }
    // Restore saved modes for trusted devices that are reachable.
    let wanted: Vec<(String, Mode)> = app.cfg.lock().unwrap().modes.iter().filter(|(_, m)| **m != Mode::Off).map(|(f, m)| (f.clone(), *m)).collect();
    for (fp, mode) in wanted {
        let idle = !app.sessions.lock().unwrap().contains_key(&fp);
        if !idle || app.peer(&fp).is_none() || app.stale.lock().unwrap().contains(&fp) || !app.knows(&fp) {
            continue;
        }
        {
            let mut at = app.attempts.lock().unwrap();
            if at.get(&fp).is_some_and(|t| t.elapsed() < Duration::from_secs(20)) {
                continue;
            }
            at.insert(fp.clone(), Instant::now());
        }
        let app = app.clone();
        tokio::spawn(async move {
            app.note(&fp, "Reconnecting");
            match start_origin(&app, &fp, mode).await {
                Ok(()) => app.note(&fp, ""),
                Err(e) => app.note(&fp, format!("Reconnect failed: {e:#}")),
            }
        });
    }
    ensure_tablet_host(app).await;
    refresh_pending(app).await;
}

/// Mark advertised devices that no longer accept connections as stale so they disappear from the list.
/// "Scan" button / window shown: check every known device now and re-dial ones added by address.
pub async fn rescan(app: &Arc<App>) {
    let manual = app.cfg.lock().unwrap().manual.clone();
    for host in manual {
        let _ = net::connect_address(app, &host).await;
    }
    probe_peers(app).await;
}

async fn probe_peers(app: &Arc<App>) {
    let peers: Vec<(String, Vec<IpAddr>, u16)> = app.peers.lock().unwrap().values().map(|p| (p.fp.clone(), p.addrs.clone(), p.port)).collect();
    for (fp, addrs, port) in peers {
        let mut alive = false;
        for ip in addrs.iter().take(3) {
            if matches!(tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect((*ip, port))).await, Ok(Ok(_))) {
                alive = true;
                break;
            }
        }
        let mut stale = app.stale.lock().unwrap();
        if alive {
            stale.remove(&fp);
        } else {
            stale.insert(fp);
        }
    }
}

pub async fn supervise(app: Arc<App>) {
    let a = app.clone();
    let _ = tokio::task::spawn_blocking(move || crate::engines::kill_orphans(&a)).await;
    let mut n = 0u32;
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        // Background check about once a minute; the UI asks for an immediate one when the window opens.
        if n % 30 == 0 {
            probe_peers(&app).await;
        }
        // Computers added by address aren't found by mDNS; keep them connected (and re-find them after a restart).
        if n % 15 == 1 {
            let manual = app.cfg.lock().unwrap().manual.clone();
            for host in manual {
                let known = host.parse::<IpAddr>().is_ok_and(|ip| app.peers.lock().unwrap().values().any(|p| p.addrs.contains(&ip)));
                if !known {
                    let app = app.clone();
                    tokio::spawn(async move {
                        let _ = net::connect_address(&app, &host).await;
                    });
                }
            }
        }
        n = n.wrapping_add(1);
        tick(&app).await;
        if std::env::var_os("BEKDESK_DUMP").is_some() {
            let _ = std::fs::write(app.dir.join("status.json"), app.snapshot().to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_newest_entry_per_device() {
        let e = |u: &str, c: &str| (u.to_string(), String::new(), c.to_string());
        let list = [e("a", "tab"), e("b", "phone"), e("c", "tab"), e("d", "tab")];
        assert_eq!(duplicate_uuids(&list), vec!["a".to_string(), "c".to_string()]);
        assert!(duplicate_uuids(&[e("x", "1"), e("y", "2")]).is_empty());
    }

    #[test]
    fn finds_last_deskflow_switch() {
        let log = "[x] INFO: switch from \"me\" to \"bk-abc\" at 1919,500\n[y] INFO: switch from \"bk-abc\" to \"me\" at 0,500\n";
        assert_eq!(switched_to(log), Some("me"));
        assert_eq!(switched_to(&log[..log.find("[y]").unwrap()]), Some("bk-abc"));
        assert_eq!(switched_to("nothing"), None);
    }

    #[test]
    fn edges_cross_where_screens_overlap() {
        let a = Box2 { x: 0.0, y: 0.0, w: 192.0, h: 120.0 };
        let lower = Box2 { x: 192.0, y: 60.0, w: 192.0, h: 120.0 };
        assert_eq!(edge_range("right", a, lower), Some(((50, 100), (0, 50))));
        let level = Box2 { x: 192.0, y: 0.0, w: 192.0, h: 120.0 };
        assert_eq!(edge_range("right", a, level), None);
        let apart = Box2 { x: 192.0, y: 500.0, w: 192.0, h: 120.0 };
        assert_eq!(edge_range("right", a, apart), None);
        let below = Box2 { x: 96.0, y: 120.0, w: 192.0, h: 108.0 };
        assert_eq!(edge_range("down", a, below), Some(((50, 100), (0, 50))));
    }

    #[test]
    fn streaming_follows_last_event() {
        assert!(!streaming_in_log("nothing"));
        assert!(streaming_in_log("a CLIENT CONNECTED b"));
        assert!(!streaming_in_log("CLIENT CONNECTED ... CLIENT DISCONNECTED"));
        assert!(streaming_in_log("CLIENT CONNECTED CLIENT DISCONNECTED CLIENT CONNECTED"));
    }

    #[test]
    fn links_follow_layout() {
        let s = vec![("a".to_string(), 0.0, 0.0), ("b".to_string(), 200.0, 10.0), ("c".to_string(), 0.0, -150.0)];
        let l = links(&s);
        assert!(l.contains(&("a".into(), "right", "b".into())));
        assert!(l.contains(&("b".into(), "left", "a".into())));
        assert!(l.contains(&("a".into(), "up", "c".into())));
        assert!(l.contains(&("c".into(), "down", "a".into())));
        assert!(!l.iter().any(|(a, d, _)| a == "a" && *d == "left"));
    }
}
