//! Per-device connection modes. The device where the user picks a mode is the *origin*:
//! - Input: origin runs the Deskflow server (its keyboard/mouse), target runs the Deskflow client.
//!   Deskflow only listens on loopback; peers reach it through the bekdesk TLS tunnel.
//! - Mirror: origin runs a Sunshine host for its main display, target runs Moonlight.
//! - Extend: like Mirror, but Sunshine captures a virtual monitor the origin adds to its desktop.

use crate::app::{App, Mode, Session, SELF};
use crate::display;
use crate::engines::{ensure, spawn};
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
    app.cfg.lock().unwrap().modes.insert(fp.to_string(), mode);
    app.save();
    let current = app.sessions.lock().unwrap().get(fp).map(|s| s.mode);
    if current == Some(mode) && mode != Mode::Off {
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
            let host = start_sunshine(app, fp, base, output.as_deref(), &app.peer_name(fp)).await;
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
                    if !moonlight_paired(&exe, &host).await {
                        // The pair window stays open after success, so watch for pairing and close it ourselves.
                        let mut c = spawn(&app2, &exe, &["pair".into(), host.clone(), "--pin".into(), pin], "moonlight-pair.log")?;
                        let mut paired = false;
                        for _ in 0..40 {
                            tokio::time::sleep(Duration::from_secs(3)).await;
                            if moonlight_paired(&exe, &host).await {
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
                    // 1:1 to the screen, and sound stays on the computer being shown.
                    let (w, h) = display::primary_size(&app2.monitors.lock().unwrap()).unwrap_or((1920, 1080));
                    let args = [
                        "stream", &host, "Desktop", "--display-mode", "fullscreen", "--resolution", &format!("{w}x{h}"), "--fps", "60",
                        "--no-vsync", "--no-frame-pacing", "--absolute-mouse", "--audio-on-host", "--keep-awake",
                    ]
                    .map(String::from)
                    .to_vec();
                    spawn(&app2, &exe, &args, "moonlight-stream.log")
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
async fn moonlight_paired(exe: &Path, host: &str) -> bool {
    let mut c = tokio::process::Command::new(exe);
    c.args(["list", host]).kill_on_drop(true).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    if let Some(d) = exe.parent() {
        c.current_dir(d);
    }
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
    let mut screens = vec![(App::screen_name(&app.id.fp), me.x + me.w / 2.0, me.y + me.h / 2.0)];
    for fp in &peers {
        let b = device_box(&app.peer(fp).map(|p| p.monitors).unwrap_or_default(), layout.get(fp).copied());
        screens.push((App::screen_name(fp), b.x + b.w / 2.0, b.y + b.h / 2.0));
    }
    let computers: String = screens.iter().enumerate().map(|(i, (n, _, _))| format!("[computer_{i}]\nname={n}\n\n")).collect();
    let mut s = String::from("section: links\n");
    let l = links(&screens);
    for (n, _, _) in &screens {
        s += &format!("\t{n}:\n");
        for (_, dir, b) in l.iter().filter(|(a, _, _)| a == n) {
            s += &format!("\t\t{dir} = {b}\n");
        }
    }
    s += "end\n\nsection: options\n\tclipboardSharing = false\nend\n";
    (s, computers)
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
            "[core]\ncoreMode=2\ncomputerName={}\nport={DESKFLOW_PORT}\ninterface=127.0.0.1\npreventSleep=false\n\n{computers}[server]\nexternalConfig=true\nexternalConfigFile={}\n\n[security]\ntlsEnabled=false\ncheckPeerFingerprints=false\n",
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

async fn start_sunshine(app: &App, key: &str, port: u16, output: Option<&str>, receiver: &str) -> Result<(tokio::process::Child, Respawn, (String, String))> {
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
    std::fs::write(&apps, r#"{"env":{},"apps":[{"name":"Desktop","image-path":"desktop.png"}]}"#)?;
    let conf = format!(
        "sunshine_name = bekdesk {} for {}\nport = {port}\nfile_state = {state}\ncredentials_file = {state}\nfile_apps = {}\nlog_path = {}\norigin_web_ui_allowed = pc\nupnp = disabled\n",
        app.name().replace('\n', " "),
        receiver.replace('\n', " "),
        fwd(&apps),
        fwd(&dir.join("sunshine.log"))
    );
    let conf_path = dir.join("sunshine.conf");
    std::fs::write(&conf_path, &conf)?;
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
        // its own log at startup, so read it from there and restart with `output_name` set.
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
        let _ = child.kill().await;
        std::fs::write(&conf_path, format!("{conf}output_name = {id}\n"))?;
        child = spawn(app, &exe, &args, &log)?;
    }
    Ok((child, (exe, args, log), creds))
}

/// (paired, streaming) for the Sunshine instance run for `key`: paired once a Moonlight client has been
/// accepted (state file), streaming while the last "CLIENT CONNECTED" in its log has no later "CLIENT DISCONNECTED".
pub fn stream_state(app: &App, key: &str) -> (bool, bool) {
    use std::io::{Read, Seek, SeekFrom};
    let dir = app.dir.join("sunshine").join(&key[..key.len().min(12)]);
    let paired = std::fs::read_to_string(dir.join("sunshine_state.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .is_some_and(|v| v["root"]["named_devices"].as_array().is_some_and(|a| !a.is_empty()));
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

pub async fn tablet_pin(app: &App, pin: &str) -> Result<()> {
    if pin.len() != 4 || !pin.chars().all(|c| c.is_ascii_digit()) {
        bail!("PIN must be 4 digits");
    }
    let dir = app.dir.join("sunshine").join("tablet");
    let creds = std::fs::read_to_string(dir.join("bekdesk-creds.txt")).context("turn on tablet hosting first")?;
    let (u, p) = creds.split_once(':').context("bad credentials file")?;
    if !submit_pin(TABLET_PORT, &(u.to_string(), p.trim().to_string()), pin, "Moonlight tablet", None).await? {
        bail!("No pairing request found — tap this computer in Moonlight on the tablet first, then enter the PIN it shows");
    }
    Ok(())
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
    app.note(key, "Setting up the virtual display (approve the Windows prompt)");
    let r = tokio::task::spawn_blocking(move || display::ensure_installed(&inf)).await?;
    app.note(key, "");
    r?;
    let name = display::vdd_display().context("virtual display not found")?;
    // The driver offers a fixed mode list (16:9 sizes up to 4K); fall back to 1080p if the receiver's size isn't in it.
    if display::set_mode(&name, w, h, x, y).is_err() {
        if let Err(e) = display::set_mode(&name, 1920, 1080, x, y) {
            detach_virtual();
            return Err(e);
        }
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
}

fn detach_virtual() {
    if let Some(name) = display::vdd_display() {
        let _ = display::set_mode(&name, 0, 0, 0, 0);
    }
}

async fn ensure_tablet_host(app: &Arc<App>) {
    let want = app.cfg.lock().unwrap().tablet;
    let have = app.tablet_host.lock().unwrap().as_ref().map(|s| s.mode);
    if have == Some(want) || (want == Mode::Off && have.is_none()) {
        return;
    }
    if let Some(old) = app.tablet_host.lock().unwrap().take() {
        if old.mode == Mode::Extend {
            detach_virtual();
        }
    }
    if want == Mode::Off {
        return;
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
        let (child, respawn, _) = start_sunshine(app, "tablet", TABLET_PORT, output.as_deref(), "Moonlight").await?;
        let mut s = session(want, true, IpAddr::from([0, 0, 0, 0]), "Ready — open Moonlight on the tablet");
        s.procs.push(child);
        s.respawn = Some(respawn);
        *app.tablet_host.lock().unwrap() = Some(s);
        Ok(())
    }
    .await;
    if let Err(e) = r {
        app.note("tablet", format!("{e:#}"));
        app.cfg.lock().unwrap().tablet = Mode::Off;
        app.save();
    }
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

pub async fn tick(app: &Arc<App>) {
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
}

/// Mark advertised devices that no longer accept connections as stale so they disappear from the list.
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
        if n % 8 == 0 {
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
