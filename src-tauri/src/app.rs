use crate::identity::{pairing_code, Identity};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Off,
    Input,
    Extend,
    Mirror,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Caps {
    pub input: bool,
    pub clipboard: bool,
    pub files: bool,
    pub screen: bool,
}
impl Default for Caps {
    fn default() -> Self {
        Caps { input: true, clipboard: true, files: true, screen: true }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Cap {
    Input,
    Clipboard,
    Files,
    Screen,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Trusted {
    pub name: String,
    pub os: String,
    #[serde(default)]
    pub caps: Caps,
}

/// Position of a device in the Workspace view, in workspace units (1 unit = 10 physical px).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
}

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
pub struct Config {
    pub name: String,
    pub trusted: HashMap<String, Trusted>,
    pub blocked: HashMap<String, String>,
    /// Mode this device runs as originator towards each peer; restored on reconnect.
    pub modes: HashMap<String, Mode>,
    /// "self" or a peer fingerprint.
    pub layout: HashMap<String, Pos>,
    pub slots: HashMap<String, u16>,
    /// Host mode offered to Moonlight receivers that don't run bekdesk (Android tablets).
    pub tablet: Mode,
    pub tablet_res: String,
    /// Addresses added by hand ("Connect by address"); re-dialled when the device isn't found by discovery.
    pub manual: Vec<String>,
    /// Devices (peer fingerprint or "tablet") that also play this computer's sound; off by default.
    pub sound: HashMap<String, bool>,
    /// Pointer speed multiplier while the mouse is on a device (peer fingerprint or "tablet"); 1.0 when unset.
    pub pointer: HashMap<String, f64>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Peer {
    pub fp: String,
    pub name: String,
    pub os: String,
    pub addrs: Vec<IpAddr>,
    pub port: u16,
    /// "x,y,w,h;..." physical pixels, primary first.
    pub monitors: String,
    /// "laptop" or "desktop" (empty if the peer didn't say).
    pub kind: String,
    #[serde(skip)]
    pub fullname: String,
}

pub struct Session {
    pub mode: Mode,
    pub origin: bool,
    pub peer_ip: IpAddr,
    pub status: String,
    pub since: SystemTime,
    pub procs: Vec<tokio::process::Child>,
    pub tasks: Vec<tokio::task::JoinHandle<()>>,
    /// How to restart procs[0] if it crashes; None means its exit ends the session.
    pub respawn: Option<(PathBuf, Vec<String>, String)>,
}
impl Drop for Session {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
        // procs are kill_on_drop
    }
}

#[derive(Serialize, Clone)]
pub struct Transfer {
    pub id: u64,
    pub peer: String,
    pub label: String,
    pub outgoing: bool,
    pub total: u64,
    pub done: u64,
    pub state: String,
}

#[derive(Serialize, Clone)]
pub struct PairRequest {
    pub id: u64,
    pub fp: String,
    pub name: String,
    pub os: String,
    pub code: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision {
    Once,
    Always,
    Decline,
    Block,
}

pub struct App {
    pub dir: PathBuf,
    pub id: Identity,
    pub port: u16,
    pub monitors: Mutex<String>,
    pub cfg: Mutex<Config>,
    pub peers: Mutex<HashMap<String, Peer>>,
    pub sessions: Mutex<HashMap<String, Session>>,
    pub once: Mutex<HashSet<String>>,
    pub requests: Mutex<Vec<PairRequest>>,
    pub waiters: Mutex<HashMap<u64, oneshot::Sender<Decision>>>,
    pub transfers: Mutex<Vec<Transfer>>,
    pub notes: Mutex<HashMap<String, String>>,
    /// Advertised peers that stopped answering (e.g. crashed without an mDNS goodbye).
    pub stale: Mutex<HashSet<String>>,
    pub log: Mutex<VecDeque<String>>,
    pub clip_last: Mutex<u64>,
    pub attempts: Mutex<HashMap<String, Instant>>,
    pub deskflow_server: Mutex<Option<tokio::process::Child>>,
    pub tablet_host: Mutex<Option<Session>>,
    pub engine_lock: tokio::sync::Mutex<()>,
    /// Woken when a trusted peer asks this computer to flash its screen numbers.
    pub identify: tokio::sync::Notify,
    /// Moonlight pairing requests waiting for a PIN, by pairing id, with when bekdesk first saw them.
    pub moonlight_pending: Mutex<HashMap<String, (Value, Instant)>>,
    pub tls: crate::net::Tls,
    seq: AtomicU64,
}

pub const SELF: &str = "self";

impl App {
    pub fn new(dir: PathBuf, port: u16, default_name: &str) -> Result<Arc<App>> {
        std::fs::create_dir_all(dir.join("logs"))?;
        let id = Identity::load_or_create(&dir)?;
        let mut cfg: Config = std::fs::read(dir.join("config.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if cfg.name.trim().is_empty() {
            cfg.name = default_name.to_string();
        }
        if cfg.tablet_res.is_empty() {
            cfg.tablet_res = "1920x1200".into();
        }
        let tls = crate::net::Tls::new(&id)?;
        Ok(Arc::new(App {
            dir,
            id,
            port,
            monitors: Mutex::new(String::new()),
            cfg: Mutex::new(cfg),
            peers: Default::default(),
            sessions: Default::default(),
            once: Default::default(),
            requests: Default::default(),
            waiters: Default::default(),
            transfers: Default::default(),
            notes: Default::default(),
            stale: Default::default(),
            log: Default::default(),
            clip_last: Default::default(),
            attempts: Default::default(),
            deskflow_server: Default::default(),
            tablet_host: Default::default(),
            engine_lock: Default::default(),
            identify: Default::default(),
            moonlight_pending: Default::default(),
            tls,
            seq: AtomicU64::new(1),
        }))
    }

    pub fn next_id(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    pub fn os() -> &'static str {
        std::env::consts::OS
    }

    pub fn name(&self) -> String {
        self.cfg.lock().unwrap().name.clone()
    }

    /// Deskflow screen name; must be identical on both ends of an input session.
    pub fn screen_name(fp: &str) -> String {
        format!("bk-{}", &fp[..12])
    }

    pub fn save(&self) {
        let body = serde_json::to_vec_pretty(&*self.cfg.lock().unwrap()).unwrap_or_default();
        let tmp = self.dir.join("config.json.tmp");
        if std::fs::write(&tmp, body).is_ok() {
            let _ = std::fs::rename(&tmp, self.dir.join("config.json"));
        }
    }

    pub fn log(&self, msg: impl Into<String>) {
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut l = self.log.lock().unwrap();
        l.push_back(format!("{ts} {}", msg.into()));
        while l.len() > 300 {
            l.pop_front();
        }
    }

    pub fn note(&self, fp: &str, msg: impl Into<String>) {
        let msg = msg.into();
        if msg.is_empty() {
            self.notes.lock().unwrap().remove(fp);
        } else {
            self.notes.lock().unwrap().insert(fp.to_string(), msg);
        }
    }

    pub fn peer(&self, fp: &str) -> Option<Peer> {
        self.peers.lock().unwrap().get(fp).cloned()
    }

    pub fn peer_name(&self, fp: &str) -> String {
        self.peer(fp)
            .map(|p| p.name)
            .or_else(|| self.cfg.lock().unwrap().trusted.get(fp).map(|t| t.name.clone()))
            .unwrap_or_else(|| fp[..12.min(fp.len())].to_string())
    }

    pub fn is_blocked(&self, fp: &str) -> bool {
        self.cfg.lock().unwrap().blocked.contains_key(fp)
    }

    /// Whether `fp` may use `cap` on this device. "Allow once" grants everything until restart.
    pub fn allowed(&self, fp: &str, cap: Cap) -> bool {
        if self.once.lock().unwrap().contains(fp) {
            return true;
        }
        let cfg = self.cfg.lock().unwrap();
        cfg.trusted.get(fp).is_some_and(|t| match cap {
            Cap::Input => t.caps.input,
            Cap::Clipboard => t.caps.clipboard,
            Cap::Files => t.caps.files,
            Cap::Screen => t.caps.screen,
        })
    }

    pub fn knows(&self, fp: &str) -> bool {
        self.once.lock().unwrap().contains(fp) || self.cfg.lock().unwrap().trusted.contains_key(fp)
    }

    pub fn trust(&self, fp: &str, name: &str, os: &str, always: bool) {
        if always {
            self.cfg.lock().unwrap().trusted.insert(
                fp.to_string(),
                Trusted { name: name.into(), os: os.into(), caps: Caps::default() },
            );
            self.save();
        } else {
            self.once.lock().unwrap().insert(fp.to_string());
        }
    }

    pub fn decide(&self, id: u64, d: Decision) {
        if let Some(tx) = self.waiters.lock().unwrap().remove(&id) {
            let _ = tx.send(d);
        }
    }

    pub fn code_with(&self, fp: &str) -> String {
        pairing_code(&self.id.fp, fp)
    }

    pub fn input_peers(&self) -> Vec<String> {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, s)| s.mode == Mode::Input)
            .map(|(fp, _)| fp.clone())
            .collect()
    }

    pub fn transfer_update(&self, id: u64, done: u64, state: Option<&str>) {
        if let Some(t) = self.transfers.lock().unwrap().iter_mut().find(|t| t.id == id) {
            t.done = done;
            if let Some(s) = state {
                t.state = s.to_string();
            }
        }
    }

    /// Plain-text summary for "Copy diagnostics": state plus the tail of every engine log.
    pub fn diagnostics(&self) -> String {
        fn tail(p: &std::path::Path, n: usize) -> String {
            let t = std::fs::read_to_string(p).unwrap_or_default();
            let lines: Vec<&str> = t.lines().collect();
            lines[lines.len().saturating_sub(n)..].join("\n")
        }
        let mut out = format!("bekdesk {} on {} ({})\n\n", env!("CARGO_PKG_VERSION"), Self::os(), std::env::consts::ARCH);
        let mut snap = self.snapshot();
        if let Some(o) = snap.as_object_mut() {
            o.remove("log");
        }
        out += &serde_json::to_string_pretty(&snap).unwrap_or_default();
        out += "\n\n== bekdesk log ==\n";
        out += &self.log.lock().unwrap().iter().rev().take(80).rev().cloned().collect::<Vec<_>>().join("\n");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(self.dir.join("logs")).into_iter().flatten().flatten().map(|e| e.path()).collect();
        files.extend(std::fs::read_dir(self.dir.join("sunshine")).into_iter().flatten().flatten().map(|e| e.path().join("sunshine.log")));
        files.sort();
        for f in files.iter().filter(|f| f.exists()) {
            out += &format!("\n\n== {} ==\n{}", f.strip_prefix(&self.dir).unwrap_or(f).display(), tail(f, 40));
        }
        out
    }

    pub fn hello(&self) -> crate::net::Hello {
        crate::net::Hello { name: self.name(), os: Self::os().into(), kind: crate::display::device_kind().into(), monitors: self.monitors.lock().unwrap().chars().take(200).collect() }
    }

    /// Short, easy-to-read number for this computer, derived from its identity (like a remote-desktop ID).
    pub fn friendly_id(fp: &str) -> String {
        let n = u64::from_str_radix(fp.get(..15).unwrap_or("0"), 16).unwrap_or(0) % 1_000_000_000;
        format!("{:03} {:03} {:03}", n / 1_000_000, n / 1_000 % 1_000, n % 1_000)
    }

    pub fn snapshot(&self) -> Value {
        let cfg = self.cfg.lock().unwrap().clone();
        let peers = self.peers.lock().unwrap().clone();
        let notes = self.notes.lock().unwrap().clone();
        let once = self.once.lock().unwrap().clone();
        let stale = self.stale.lock().unwrap().clone();
        let sessions: HashMap<String, Value> = self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .map(|(fp, s)| {
                let up = s.since.elapsed().map(|d| d.as_secs()).unwrap_or(0);
                let (paired, streaming) = if s.origin && matches!(s.mode, Mode::Mirror | Mode::Extend) { crate::modes::stream_state(self, fp) } else { (None, false) };
                let paired = paired.is_some();
                (fp.clone(), json!({"mode": s.mode, "origin": s.origin, "status": s.status, "ip": s.peer_ip, "uptime": up, "paired": paired, "streaming": streaming}))
            })
            .collect();
        // Offline trusted devices stay listed so their saved mode is visible.
        let peers: HashMap<String, Peer> = peers.into_iter().filter(|(fp, _)| !stale.contains(fp)).collect();
        let mut ids: Vec<String> = peers.keys().cloned().collect();
        for fp in cfg.trusted.keys() {
            if !ids.contains(fp) {
                ids.push(fp.clone());
            }
        }
        let devices: Vec<Value> = ids
            .iter()
            .filter(|fp| !cfg.blocked.contains_key(*fp))
            .map(|fp| {
                let p = peers.get(fp);
                let t = cfg.trusted.get(fp);
                json!({
                    "fp": fp,
                    "name": p.map(|p| p.name.clone()).or(t.map(|t| t.name.clone())),
                    "os": p.map(|p| p.os.clone()).or(t.map(|t| t.os.clone())),
                    "online": p.is_some(),
                    "addrs": p.map(|p| p.addrs.clone()).unwrap_or_default(),
                    "monitors": p.map(|p| p.monitors.clone()).unwrap_or_default(),
                    "kind": p.map(|p| p.kind.clone()).unwrap_or_default(),
                    "trusted": t.is_some(),
                    "once": once.contains(fp),
                    "caps": t.map(|t| t.caps),
                    "mode": cfg.modes.get(fp).copied().unwrap_or_default(),
                    "session": sessions.get(fp),
                    "note": notes.get(fp),
                    "code": self.code_with(fp),
                    "pos": cfg.layout.get(fp),
                })
            })
            .collect();
        let tablet_state = crate::modes::stream_state(self, "tablet");
        json!({
            "me": {"fp": self.id.fp, "name": cfg.name, "os": Self::os(), "kind": crate::display::device_kind(), "monitors": *self.monitors.lock().unwrap(), "pos": cfg.layout.get(SELF), "port": self.port, "ip": crate::net::local_ip().map(|i| i.to_string()), "id": Self::friendly_id(&self.id.fp)},
            "devices": devices,
            "requests": *self.requests.lock().unwrap(),
            "transfers": *self.transfers.lock().unwrap(),
            "blocked": cfg.blocked,
            "tablet": cfg.tablet,
            "tablet_res": cfg.tablet_res,
            "tablet_status": self.tablet_host.lock().unwrap().as_ref().map(|s| s.status.clone()),
            "tablet_note": notes.get("tablet"),
            "tablet_state": {"running": self.tablet_host.lock().unwrap().is_some(), "paired": tablet_state.0.is_some(), "name": tablet_state.0, "streaming": tablet_state.1,
                "pending": self.moonlight_pending.lock().unwrap().values().map(|(v, _)| v.clone()).collect::<Vec<_>>(),
                "devices": crate::modes::moonlight_devices(self, "tablet").into_iter().map(|(uuid, name)| json!({"uuid": uuid, "name": name})).collect::<Vec<_>>()},
            "sound": cfg.sound,
            "pointer": cfg.pointer,
            "components": notes.get("components"),
            "log": self.log.lock().unwrap().iter().rev().take(80).cloned().collect::<Vec<_>>(),
            "engines": crate::engines::installed(self),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_id_is_nine_digits() {
        let id = App::friendly_id(&"ab".repeat(32));
        assert_eq!(id.len(), 11);
        assert!(id.chars().all(|c| c.is_ascii_digit() || c == ' '));
        assert_eq!(id, App::friendly_id(&"ab".repeat(32)));
    }
}
