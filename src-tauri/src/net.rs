//! Authenticated, encrypted channel between bekdesk devices.
//!
//! Every connection is mutual TLS 1.3 with self-signed device certificates. Certificates are not
//! CA-validated; instead the peer's certificate fingerprint is the device identity and is checked
//! against the trust store (and against the fingerprint advertised via mDNS when dialing).
//! Each connection carries one request line (JSON), one response line, then optional raw bytes.

use crate::app::{App, Cap, Decision, Mode, PairRequest, Transfer};
use crate::identity::{fingerprint, Identity};
use anyhow::{bail, Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring, verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_rustls::{TlsAcceptor, TlsConnector};

pub const PORT: u16 = 47290;
/// Loopback-only port of the Deskflow server; reachable from peers only through the TLS tunnel.
pub const DESKFLOW_PORT: u16 = 24860;
const LINE_LIMIT: u64 = 64 * 1024;
const CLIP_LIMIT: u64 = 64 * 1024 * 1024;
const CHUNK: usize = 256 * 1024;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Req {
    Pair { name: String, os: String },
    Start { mode: Mode, pin: String, port: u16 },
    Stop,
    /// Who are you? Same public facts mDNS advertises; lets a peer be added by address when discovery is blocked.
    Hello,
    /// Flash this computer's screen numbers (Manage Screens → Identify screens on a trusted peer).
    Identify,
    Tunnel,
    Clip { kind: String, len: u64 },
    Files { items: Vec<FileItem>, paste: bool },
    /// Send this computer's sound to the asking computer (they're connected with Share Input, either way).
    Audio,
    /// The asking computer sends its sound here to play (also on this computer's speakers).
    AudioPush,
    /// Send this computer's camera to the asking computer (only if the user shared it with that computer).
    Camera,
    /// Lock this computer's screen ("Lock all" on a computer it's connected with).
    Lock,
    /// Send this computer's microphone to the asking computer (only if the user shared it with that computer).
    Mic,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileItem {
    pub rel: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct Hello {
    pub name: String,
    pub os: String,
    pub kind: String,
    pub monitors: String,
    /// Its lock screen is showing (older bekdesk versions don't say).
    #[serde(default)]
    pub locked: bool,
    /// Battery level in percent and whether it's charging; None on desktops.
    #[serde(default)]
    pub battery: Option<u8>,
    #[serde(default)]
    pub charging: bool,
    /// Network card address, so a paired computer can wake this one later (Wake-on-LAN).
    #[serde(default)]
    pub mac: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Resp {
    pub ok: bool,
    #[serde(default)]
    pub err: String,
    #[serde(default)]
    pub trusted: bool,
    #[serde(default)]
    pub info: Option<Hello>,
}
impl Resp {
    fn ok() -> Self {
        Resp { ok: true, ..Default::default() }
    }
    fn err(e: impl ToString) -> Self {
        Resp { ok: false, err: e.to_string(), ..Default::default() }
    }
}

#[derive(Debug)]
struct AnyCert(Arc<CryptoProvider>);

impl AnyCert {
    fn schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

// Identity is enforced by fingerprint after the handshake; the handshake itself still proves key possession.
impl ServerCertVerifier for AnyCert {
    fn verify_server_cert(&self, _: &CertificateDer, _: &[CertificateDer], _: &ServerName, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes()
    }
}

impl ClientCertVerifier for AnyCert {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(&self, _: &CertificateDer, _: &[CertificateDer], _: UnixTime) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes()
    }
}

pub struct Tls {
    pub acceptor: TlsAcceptor,
    pub connector: TlsConnector,
}

impl Tls {
    pub fn new(id: &Identity) -> Result<Tls> {
        let p = Arc::new(ring::default_provider());
        let v = Arc::new(AnyCert(p.clone()));
        let server = rustls::ServerConfig::builder_with_provider(p.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_client_cert_verifier(v.clone())
            .with_single_cert(vec![id.cert.clone()], id.key())?;
        let client = rustls::ClientConfig::builder_with_provider(p)
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .dangerous()
            .with_custom_certificate_verifier(v)
            .with_client_auth_cert(vec![id.cert.clone()], id.key())?;
        Ok(Tls { acceptor: TlsAcceptor::from(Arc::new(server)), connector: TlsConnector::from(Arc::new(client)) })
    }
}

async fn read_msg<T: DeserializeOwned, S: AsyncBufReadExt + Unpin>(s: &mut S) -> Result<T> {
    let mut line = String::new();
    (&mut *s).take(LINE_LIMIT).read_line(&mut line).await?;
    if line.is_empty() {
        bail!("connection closed");
    }
    Ok(serde_json::from_str(line.trim_end())?)
}

async fn write_msg<T: Serialize, S: AsyncWrite + Unpin>(s: &mut S, m: &T) -> Result<()> {
    let mut b = serde_json::to_vec(m)?;
    b.push(b'\n');
    s.write_all(&b).await?;
    s.flush().await?;
    Ok(())
}

pub async fn serve(app: Arc<App>, listener: TcpListener) {
    loop {
        let Ok((tcp, addr)) = listener.accept().await else { continue };
        let app = app.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(app.clone(), tcp, addr).await {
                app.log(format!("incoming {addr}: {e:#}"));
            }
        });
    }
}

async fn handle(app: Arc<App>, tcp: TcpStream, addr: SocketAddr) -> Result<()> {
    tcp.set_nodelay(true)?;
    // Reachability probes and port scans just connect and leave; not worth logging.
    let Ok(Ok(tls)) = timeout(Duration::from_secs(10), app.tls.acceptor.accept(tcp)).await else { return Ok(()) };
    let fp = fingerprint(tls.get_ref().1.peer_certificates().and_then(|c| c.first()).context("no client certificate")?);
    let mut s = BufReader::new(tls);
    if app.is_blocked(&fp) {
        return Ok(());
    }
    let req: Req = timeout(Duration::from_secs(10), read_msg(&mut s)).await??;
    let ip = addr.ip();
    match req {
        Req::Pair { name, os } => {
            let r = pair_incoming(&app, &fp, &name, &os).await;
            write_msg(&mut s, &r).await
        }
        Req::Start { mode, pin, port } => {
            let ok = match mode {
                Mode::Input => app.allowed(&fp, Cap::Input),
                Mode::Extend | Mode::Mirror => app.allowed(&fp, Cap::Screen),
                Mode::Off => app.knows(&fp),
            };
            // A trusts us but we forgot it (reinstalled, or pressed Forget): ask again instead of failing.
            let ok = ok || (mode != Mode::Off && !app.knows(&fp) && {
                let (name, os) = app.peer(&fp).map(|p| (p.name, p.os)).unwrap_or_else(|| (app.peer_name(&fp), String::new()));
                pair_incoming(&app, &fp, &name, &os).await.ok && app.allowed(&fp, if mode == Mode::Input { Cap::Input } else { Cap::Screen })
            });
            let r = if !ok {
                let what = if mode == Mode::Input { "keyboard and mouse sharing" } else { "showing screens" };
                Resp::err(format!("{} didn't allow {what} (check Settings → Permissions on {})", app.name(), app.name()))
            } else {
                match crate::modes::start_target(&app, &fp, ip, mode, pin, port).await {
                    Ok(()) => Resp::ok(),
                    Err(e) => Resp::err(format!("{e:#}")),
                }
            };
            write_msg(&mut s, &r).await
        }
        Req::Identify => {
            if app.knows(&fp) {
                app.identify.notify_one();
            }
            write_msg(&mut s, &Resp::ok()).await
        }
        Req::Hello => write_msg(&mut s, &Resp { ok: true, info: Some(app.hello()), ..Default::default() }).await,
        Req::Stop => {
            if app.knows(&fp) {
                crate::modes::stop_local(&app, &fp, true);
            }
            write_msg(&mut s, &Resp::ok()).await
        }
        Req::Tunnel => {
            let active = app.sessions.lock().unwrap().get(&fp).is_some_and(|x| x.origin && x.mode == Mode::Input);
            if !active || !app.allowed(&fp, Cap::Input) {
                return write_msg(&mut s, &Resp::err("no input session")).await;
            }
            let mut local = TcpStream::connect(("127.0.0.1", DESKFLOW_PORT)).await?;
            local.set_nodelay(true)?;
            write_msg(&mut s, &Resp::ok()).await?;
            tokio::io::copy_bidirectional(&mut s, &mut local).await?;
            Ok(())
        }
        Req::Clip { kind, len } => {
            if !app.allowed(&fp, Cap::Clipboard) || len > CLIP_LIMIT {
                return write_msg(&mut s, &Resp::err("clipboard not allowed")).await;
            }
            write_msg(&mut s, &Resp::ok()).await?;
            let mut data = vec![0u8; len as usize];
            s.read_exact(&mut data).await?;
            let app2 = app.clone();
            tokio::task::spawn_blocking(move || crate::clip::apply(&app2, &kind, data)).await??;
            Ok(())
        }
        Req::Files { items, paste } => {
            if !app.allowed(&fp, Cap::Files) {
                return write_msg(&mut s, &Resp::err("file transfer not allowed")).await;
            }
            receive_files(&app, &fp, &mut s, items, paste).await
        }
        Req::Audio => {
            // Only a paired computer connected with Share Input (either way) gets this computer's sound, and only while it is.
            let linked = |app: &App| app.sessions.lock().unwrap().get(&fp).is_some_and(|x| x.mode == Mode::Input);
            if !linked(&app) || !app.knows(&fp) {
                return write_msg(&mut s, &Resp::err("Share Input isn't on")).await;
            }
            // The computer being controlled goes quiet while its sound plays on the controlling one.
            let mute_here = app.sessions.lock().unwrap().get(&fp).is_some_and(|x| !x.origin);
            if let Err(e) = crate::audio::can_send() {
                return write_msg(&mut s, &Resp::err(format!("{e:#}"))).await;
            }
            write_msg(&mut s, &Resp::ok()).await?;
            app.log(format!("sending sound to {}", app.peer_name(&fp)));
            // Stop when Share Input ends, even if the other computer vanished without closing the connection.
            let ended = async {
                while linked(&app) {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            };
            tokio::select! {
                r = crate::audio::send(&mut s, mute_here) => r,
                _ = ended => Ok(()),
            }
        }
        Req::Camera => {
            let shared = |app: &App| app.sessions.lock().unwrap().get(&fp).is_some_and(|x| x.mode == Mode::Input) && app.cfg.lock().unwrap().cam_share.contains(&fp);
            if !shared(&app) || !app.knows(&fp) {
                return write_msg(&mut s, &Resp::err(format!("{} hasn't shared its camera with you (tick it on {} first)", app.name(), app.name()))).await;
            }
            write_msg(&mut s, &Resp::ok()).await?;
            app.log(format!("sharing the camera with {}", app.peer_name(&fp)));
            let ended = async {
                while shared(&app) {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            };
            tokio::select! {
                r = crate::camera::send(&mut s) => r,
                _ = ended => Ok(()),
            }
        }
        Req::Lock => {
            let linked = app.sessions.lock().unwrap().contains_key(&fp);
            if !linked || !app.knows(&fp) {
                return write_msg(&mut s, &Resp::err("not connected")).await;
            }
            app.log(format!("locked by {}", app.peer_name(&fp)));
            crate::display::lock_screen();
            write_msg(&mut s, &Resp::ok()).await
        }
        Req::Mic => {
            let linked = |app: &App| app.sessions.lock().unwrap().get(&fp).is_some_and(|x| x.mode == Mode::Input);
            if !linked(&app) || !app.knows(&fp) || !app.cfg.lock().unwrap().mic_share.contains(&fp) {
                return write_msg(&mut s, &Resp::err(format!("{} hasn't shared its microphone with you (tick it on {} first)", app.name(), app.name()))).await;
            }
            write_msg(&mut s, &Resp::ok()).await?;
            app.log(format!("sharing the microphone with {}", app.peer_name(&fp)));
            let ended = async {
                while linked(&app) && app.cfg.lock().unwrap().mic_share.contains(&fp) {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            };
            tokio::select! {
                r = crate::audio::send_mic(&mut s) => r,
                _ = ended => Ok(()),
            }
        }
        Req::AudioPush => {
            let linked = |app: &App| app.sessions.lock().unwrap().get(&fp).is_some_and(|x| x.mode == Mode::Input);
            if !linked(&app) || !app.knows(&fp) {
                return write_msg(&mut s, &Resp::err("Share Input isn't on")).await;
            }
            write_msg(&mut s, &Resp::ok()).await?;
            app.log(format!("playing sound from {}", app.peer_name(&fp)));
            let ended = async {
                while linked(&app) {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            };
            tokio::select! {
                r = crate::audio::play(&mut s) => r,
                _ = ended => Ok(()),
            }
        }
    }
}

async fn pair_incoming(app: &Arc<App>, fp: &str, name: &str, os: &str) -> Resp {
    if app.cfg.lock().unwrap().trusted.contains_key(fp) {
        return Resp { ok: true, trusted: true, ..Default::default() };
    }
    if app.requests.lock().unwrap().iter().any(|r| r.fp == fp) {
        return Resp::err("a request is already waiting");
    }
    let id = app.next_id();
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.waiters.lock().unwrap().insert(id, tx);
    app.requests.lock().unwrap().push(PairRequest {
        id,
        fp: fp.into(),
        name: name.chars().take(64).collect(),
        os: os.chars().take(16).collect(),
        code: app.code_with(fp),
    });
    app.asked.notify_one();
    let d = timeout(Duration::from_secs(90), rx).await;
    app.requests.lock().unwrap().retain(|r| r.id != id);
    app.waiters.lock().unwrap().remove(&id);
    match d {
        Ok(Ok(Decision::Always)) => {
            app.trust(fp, name, os, true);
            app.log(format!("trusted {name}"));
            Resp { ok: true, trusted: true, ..Default::default() }
        }
        Ok(Ok(Decision::Once)) => {
            app.trust(fp, name, os, false);
            Resp::ok()
        }
        Ok(Ok(Decision::Block)) => {
            app.cfg.lock().unwrap().blocked.insert(fp.into(), name.into());
            app.save();
            Resp::err("declined")
        }
        _ => Resp::err("declined"),
    }
}

pub type Stream = BufReader<tokio_rustls::client::TlsStream<TcpStream>>;

/// Dial a discovered peer and verify it presents the expected identity.
pub async fn open(app: &App, fp: &str) -> Result<(Stream, IpAddr)> {
    let p = app.peer(fp).with_context(|| format!("{} is offline", app.peer_name(fp)))?;
    let mut last = anyhow::anyhow!("no address");
    for ip in &p.addrs {
        match timeout(Duration::from_secs(4), TcpStream::connect((*ip, p.port))).await {
            Ok(Ok(tcp)) => {
                tcp.set_nodelay(true)?;
                let tls = timeout(Duration::from_secs(10), app.tls.connector.connect(ServerName::try_from("bekdesk")?, tcp)).await??;
                let got = fingerprint(tls.get_ref().1.peer_certificates().and_then(|c| c.first()).context("no server certificate")?);
                if got != fp {
                    bail!("identity mismatch for {} — refusing to connect", p.name);
                }
                return Ok((BufReader::new(tls), *ip));
            }
            Ok(Err(e)) => last = e.into(),
            Err(_) => last = anyhow::anyhow!("timed out"),
        }
    }
    Err(last.context(format!("can't reach {}. Make sure bekdesk is open there and both are on the same Wi-Fi. {}", p.name, firewall_hint(&p.os))))
}

/// The one firewall step for a computer running `os` that lets bekdesk (port 47290) through.
pub fn firewall_hint(os: &str) -> &'static str {
    match os {
        "linux" => "If it still can't connect, run this on it: sudo firewall-cmd --permanent --add-port=47290/tcp --add-service=mdns && sudo firewall-cmd --reload",
        "macos" => "If it still can't connect: on it open System Settings → Network → Firewall → Options and allow bekdesk.",
        _ => "If it still can't connect: on it open Windows Security → Firewall & network protection → Allow an app through firewall, and tick bekdesk for Private.",
    }
}

pub async fn request(app: &App, fp: &str, req: &Req) -> Result<(Stream, IpAddr)> {
    let (mut s, ip) = open(app, fp).await?;
    write_msg(&mut s, req).await?;
    let r: Resp = timeout(Duration::from_secs(120), read_msg(&mut s)).await??;
    if !r.ok {
        bail!("{}", r.err);
    }
    Ok((s, ip))
}

/// This computer's LAN address (the interface a route to the network would use), for "your address".
/// This computer's network card address (aa:bb:cc:dd:ee:ff) on the network bekdesk uses; looked up once.
pub fn local_mac() -> String {
    static MAC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    MAC.get_or_init(|| {
        let Some(ip) = local_ip() else { return String::new() };
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let ps = format!("(Get-NetAdapter -InterfaceIndex (Get-NetIPAddress -IPAddress '{ip}').InterfaceIndex).MacAddress");
            let o = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &ps]).creation_flags(0x0800_0000).output();
            o.map(|o| String::from_utf8_lossy(&o.stdout).trim().replace('-', ":").to_lowercase()).unwrap_or_default()
        }
        #[cfg(not(windows))]
        {
            let o = std::process::Command::new("ip").args(["-o", "-4", "addr"]).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
            let ifname = o.lines().find(|l| l.contains(&format!(" {ip}/"))).and_then(|l| l.split_whitespace().nth(1)).unwrap_or("").to_string();
            std::fs::read_to_string(format!("/sys/class/net/{ifname}/address")).map(|s| s.trim().to_string()).unwrap_or_default()
        }
    })
    .clone()
}

/// Wake a sleeping or switched-off computer on this network (its network card and BIOS must allow Wake-on-LAN).
pub fn wake(mac: &str) -> Result<()> {
    let bytes: Vec<u8> = mac.split([':', '-']).filter_map(|h| u8::from_str_radix(h, 16).ok()).collect();
    if bytes.len() != 6 {
        bail!("this computer's network address isn't known yet; it's learned while both computers are on");
    }
    let mut packet = vec![0xffu8; 6];
    for _ in 0..16 {
        packet.extend_from_slice(&bytes);
    }
    let s = std::net::UdpSocket::bind("0.0.0.0:0")?;
    s.set_broadcast(true)?;
    s.send_to(&packet, "255.255.255.255:9")?;
    if let Some(IpAddr::V4(v4)) = local_ip() {
        let o = v4.octets();
        let _ = s.send_to(&packet, (std::net::Ipv4Addr::new(o[0], o[1], o[2], 255), 9));
    }
    Ok(())
}

pub fn local_ip() -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?; // no packet is sent; this only picks the outgoing interface
    s.local_addr().ok().map(|a| a.ip()).filter(|ip| !ip.is_unspecified())
}

/// Add a computer by address (IP or name, optionally `:port`) when mDNS can't see it, e.g. on a
/// Wi-Fi that isolates devices or across subnets. Returns its fingerprint; it then behaves like a discovered peer.
pub async fn connect_address(app: &Arc<App>, host: &str) -> Result<String> {
    let host = host.trim();
    if host.is_empty() || host.len() > 253 {
        bail!("type the other computer's address, e.g. 192.168.1.23");
    }
    let target = if host.parse::<SocketAddr>().is_ok() || host.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) { host.to_string() } else { format!("{host}:{PORT}") };
    let addr = tokio::net::lookup_host(&target).await.with_context(|| format!("cannot find {host}"))?.min_by_key(|a| a.is_ipv6()).with_context(|| format!("cannot find {host}"))?;
    let tcp = timeout(Duration::from_secs(5), TcpStream::connect(addr)).await.map_err(|_| anyhow::anyhow!("{host} did not answer. Is bekdesk open there, on the same network?"))??;
    tcp.set_nodelay(true)?;
    let tls = timeout(Duration::from_secs(10), app.tls.connector.connect(ServerName::try_from("bekdesk")?, tcp)).await??;
    let fp = fingerprint(tls.get_ref().1.peer_certificates().and_then(|c| c.first()).context("no server certificate")?);
    if fp == app.id.fp {
        bail!("that is this computer");
    }
    let mut s = BufReader::new(tls);
    write_msg(&mut s, &Req::Hello).await?;
    let r: Resp = timeout(Duration::from_secs(10), read_msg(&mut s)).await.map_err(|_| anyhow::anyhow!("no answer; update bekdesk on the other computer"))?.context("the other computer needs a newer bekdesk")?;
    let info = r.info.context("the other computer needs a newer bekdesk")?;
    let peer = crate::app::Peer {
        fp: fp.clone(),
        name: info.name.chars().take(60).collect(),
        os: info.os.chars().take(16).collect(),
        addrs: vec![addr.ip()],
        port: addr.port(),
        monitors: info.monitors.chars().take(200).collect(),
        kind: info.kind.chars().take(16).collect(),
        fullname: format!("manual-{fp}"),
    };
    app.stale.lock().unwrap().remove(&fp);
    if app.peers.lock().unwrap().insert(fp.clone(), peer.clone()).is_none() {
        app.log(format!("added {} by address {}", peer.name, addr));
    }
    {
        let mut c = app.cfg.lock().unwrap();
        if !c.manual.iter().any(|m| m == host) {
            c.manual.push(host.to_string());
        }
    }
    app.save();
    Ok(fp)
}

/// Ask an untrusted peer to approve us. Their answer (once/always) is mirrored locally.
/// Asks a known peer about itself (used to notice when it's locked).
pub async fn hello(app: &App, fp: &str) -> Result<Hello> {
    let (mut s, _) = open(app, fp).await?;
    write_msg(&mut s, &Req::Hello).await?;
    let r: Resp = timeout(Duration::from_secs(10), read_msg(&mut s)).await??;
    r.info.context("no answer")
}

pub async fn pair(app: &App, fp: &str) -> Result<()> {
    let p = app.peer(fp).context("device is offline")?;
    app.note(fp, format!("Confirm code {} on {}", app.code_with(fp), p.name));
    let res = async {
        let (mut s, _) = open(app, fp).await?;
        write_msg(&mut s, &Req::Pair { name: app.name(), os: App::os().into() }).await?;
        let r: Resp = timeout(Duration::from_secs(100), read_msg(&mut s)).await??;
        if !r.ok {
            bail!("{} declined", p.name);
        }
        app.trust(fp, &p.name, &p.os, r.trusted);
        Ok(())
    }
    .await;
    app.note(fp, "");
    res
}

pub async fn send_clip(app: &App, fp: &str, kind: &str, data: &[u8]) -> Result<()> {
    let (mut s, _) = request(app, fp, &Req::Clip { kind: kind.into(), len: data.len() as u64 }).await?;
    s.write_all(data).await?;
    s.flush().await?;
    Ok(())
}

/// Reject absolute paths, `..`, drive prefixes and empty names coming from a peer.
pub fn safe_rel(rel: &str) -> Option<PathBuf> {
    let p = Path::new(rel);
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(x) => out.push(x),
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty() && !rel.contains('\\') && !rel.contains(':')).then_some(out)
}

fn collect(path: &Path, rel: &str, out: &mut Vec<(PathBuf, FileItem)>) -> Result<()> {
    let md = std::fs::symlink_metadata(path)?;
    if md.is_file() {
        out.push((path.to_path_buf(), FileItem { rel: rel.into(), size: md.len() }));
    } else if md.is_dir() {
        for e in std::fs::read_dir(path)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().to_string();
            collect(&e.path(), &format!("{rel}/{name}"), out)?;
        }
    }
    Ok(())
}

pub fn plan_files(paths: &[PathBuf]) -> Result<Vec<(PathBuf, FileItem)>> {
    let mut out = vec![];
    for p in paths {
        let name = p.file_name().context("bad path")?.to_string_lossy().to_string();
        collect(p, &name, &mut out)?;
    }
    Ok(out)
}

pub async fn send_files(app: &App, fp: &str, paths: Vec<PathBuf>, paste: bool) -> Result<()> {
    let plan = tokio::task::spawn_blocking(move || plan_files(&paths)).await??;
    if plan.is_empty() {
        bail!("nothing to send");
    }
    let total: u64 = plan.iter().map(|(_, i)| i.size).sum();
    let id = app.next_id();
    let label = if plan.len() == 1 { plan[0].1.rel.clone() } else { format!("{} files", plan.len()) };
    app.transfers.lock().unwrap().push(Transfer { id, peer: app.peer_name(fp), label, outgoing: true, total, done: 0, state: "sending".into() });
    let res = async {
        let items = plan.iter().map(|(_, i)| i.clone()).collect();
        let (mut s, _) = request(app, fp, &Req::Files { items, paste }).await?;
        let mut done = 0u64;
        let mut buf = vec![0u8; CHUNK];
        for (path, item) in &plan {
            let mut f = tokio::fs::File::open(path).await?;
            let mut left = item.size;
            while left > 0 {
                let n = f.read(&mut buf[..left.min(CHUNK as u64) as usize]).await?;
                if n == 0 {
                    bail!("{} changed while sending", item.rel);
                }
                s.write_all(&buf[..n]).await?;
                left -= n as u64;
                done += n as u64;
                app.transfer_update(id, done, None);
            }
        }
        s.flush().await?;
        let r: Resp = timeout(Duration::from_secs(120), read_msg(&mut s)).await??;
        if !r.ok {
            bail!("{}", r.err);
        }
        Ok(())
    }
    .await;
    app.transfer_update(id, if res.is_ok() { total } else { 0 }, Some(if res.is_ok() { "sent" } else { "failed" }));
    res
}

pub fn inbox() -> PathBuf {
    dirs::download_dir().or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir).join("bekdesk")
}

async fn receive_files<S: AsyncRead + AsyncWrite + AsyncBufReadExt + Unpin>(app: &Arc<App>, fp: &str, s: &mut S, items: Vec<FileItem>, paste: bool) -> Result<()> {
    let mut rels = vec![];
    for i in &items {
        match safe_rel(&i.rel) {
            Some(r) => rels.push(r),
            None => return write_msg(s, &Resp::err(format!("unsafe path {}", i.rel))).await,
        }
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis();
    let base = app.inbox_dir().join(format!("{}-{stamp}", app.peer_name(fp).replace(['/', '\\', ':'], "_")));
    let total: u64 = items.iter().map(|i| i.size).sum();
    let id = app.next_id();
    let label = if items.len() == 1 { items[0].rel.clone() } else { format!("{} files", items.len()) };
    app.transfers.lock().unwrap().push(Transfer { id, peer: app.peer_name(fp), label, outgoing: false, total, done: 0, state: "receiving".into() });
    write_msg(s, &Resp::ok()).await?;
    let res: Result<()> = async {
        let mut done = 0u64;
        let mut buf = vec![0u8; CHUNK];
        for (rel, item) in rels.iter().zip(&items) {
            let path = base.join(rel);
            tokio::fs::create_dir_all(path.parent().context("bad path")?).await?;
            let mut f = tokio::fs::File::create(&path).await?;
            let mut left = item.size;
            while left > 0 {
                let n = s.read(&mut buf[..left.min(CHUNK as u64) as usize]).await?;
                if n == 0 {
                    drop(f);
                    let _ = tokio::fs::remove_file(&path).await;
                    bail!("connection lost during {}", item.rel);
                }
                f.write_all(&buf[..n]).await?;
                left -= n as u64;
                done += n as u64;
                app.transfer_update(id, done, None);
            }
            f.flush().await?;
        }
        Ok(())
    }
    .await;
    app.transfer_update(id, if res.is_ok() { total } else { 0 }, Some(if res.is_ok() { "received" } else { "failed" }));
    res?;
    write_msg(s, &Resp::ok()).await?;
    app.log(format!("received {} item(s) into {}", items.len(), base.display()));
    if paste {
        let mut tops: Vec<PathBuf> = rels.iter().filter_map(|r| r.components().next()).map(|c| base.join(c)).collect();
        tops.dedup();
        let app2 = app.clone();
        tokio::task::spawn_blocking(move || crate::clip::set_files(&app2, &tops));
    }
    Ok(())
}

impl App {
    pub fn inbox_dir(&self) -> PathBuf {
        std::env::var_os("BEKDESK_INBOX").map(PathBuf::from).unwrap_or_else(inbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_paths_are_sanitized() {
        assert!(safe_rel("a/b.txt").is_some());
        assert!(safe_rel("../x").is_none());
        assert!(safe_rel("/etc/passwd").is_none());
        assert!(safe_rel("a/../../x").is_none());
        assert!(safe_rel("C:/x").is_none());
        assert!(safe_rel("a\\..\\x").is_none());
        assert!(safe_rel("").is_none());
    }
}
