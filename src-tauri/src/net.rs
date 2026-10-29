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
    Tunnel,
    Clip { kind: String, len: u64 },
    Files { items: Vec<FileItem>, paste: bool },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileItem {
    pub rel: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Resp {
    pub ok: bool,
    #[serde(default)]
    pub err: String,
    #[serde(default)]
    pub trusted: bool,
}
impl Resp {
    fn ok() -> Self {
        Resp { ok: true, ..Default::default() }
    }
    fn err(e: impl ToString) -> Self {
        Resp { ok: false, err: e.to_string(), trusted: false }
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
            let r = if !ok {
                Resp::err(format!("{} has not allowed this on {}", app.name(), app.name()))
            } else {
                match crate::modes::start_target(&app, &fp, ip, mode, pin, port).await {
                    Ok(()) => Resp::ok(),
                    Err(e) => Resp::err(format!("{e:#}")),
                }
            };
            write_msg(&mut s, &r).await
        }
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
    }
}

async fn pair_incoming(app: &Arc<App>, fp: &str, name: &str, os: &str) -> Resp {
    if app.cfg.lock().unwrap().trusted.contains_key(fp) {
        return Resp { ok: true, trusted: true, err: String::new() };
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
    let d = timeout(Duration::from_secs(90), rx).await;
    app.requests.lock().unwrap().retain(|r| r.id != id);
    app.waiters.lock().unwrap().remove(&id);
    match d {
        Ok(Ok(Decision::Always)) => {
            app.trust(fp, name, os, true);
            app.log(format!("trusted {name}"));
            Resp { ok: true, trusted: true, err: String::new() }
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
    Err(last.context(format!("cannot reach {}", p.name)))
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

/// Ask an untrusted peer to approve us. Their answer (once/always) is mirrored locally.
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
