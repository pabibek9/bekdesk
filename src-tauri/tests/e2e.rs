//! Two real bekdesk cores on loopback: TLS identity, approval flow, permissions and file transfer.

use bekdesk_core::app::{Decision, Peer};
use bekdesk_core::{net, App};
use std::sync::Arc;
use std::time::Duration;

async fn node(name: &str) -> (Arc<App>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let app = App::new(dir.path().to_path_buf(), l.local_addr().unwrap().port(), name).unwrap();
    tokio::spawn(net::serve(app.clone(), l));
    (app, dir)
}

fn see(a: &App, b: &App) {
    a.peers.lock().unwrap().insert(
        b.id.fp.clone(),
        Peer { fp: b.id.fp.clone(), name: b.name(), os: "test".into(), addrs: vec!["127.0.0.1".parse().unwrap()], port: b.port, monitors: String::new(), kind: String::new(), fullname: String::new() },
    );
}

/// Answer the first pairing prompt that shows up on `app`.
fn answer(app: Arc<App>, d: Decision) -> tokio::task::JoinHandle<String> {
    tokio::spawn(async move {
        loop {
            if let Some(r) = app.requests.lock().unwrap().first().cloned() {
                app.decide(r.id, d);
                return r.code;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
}

#[tokio::test]
async fn pairing_permissions_and_files() {
    let (a, _da) = node("Alpha").await;
    let (b, db) = node("Beta").await;
    see(&a, &b);
    see(&b, &a);

    // Unknown devices get nothing.
    let err = net::send_files(&a, &b.id.fp, vec![], false).await.unwrap_err();
    assert!(err.to_string().contains("nothing to send"));
    let f = _da.path().join("hello.txt");
    std::fs::write(&f, b"hi there").unwrap();
    assert!(net::send_files(&a, &b.id.fp, vec![f.clone()], false).await.is_err());

    // Decline, then Always Allow; both sides must show the same code.
    let h = answer(b.clone(), Decision::Decline);
    assert!(net::pair(&a, &b.id.fp).await.is_err());
    h.await.unwrap();
    assert!(!a.knows(&b.id.fp));

    let h = answer(b.clone(), Decision::Always);
    net::pair(&a, &b.id.fp).await.unwrap();
    assert_eq!(h.await.unwrap(), a.code_with(&b.id.fp));
    assert!(b.cfg.lock().unwrap().trusted.contains_key(&a.id.fp));
    assert!(a.cfg.lock().unwrap().trusted.contains_key(&b.id.fp));

    // Files and folders arrive intact.
    std::env::set_var("BEKDESK_INBOX", db.path().join("inbox"));
    let folder = _da.path().join("photos");
    std::fs::create_dir_all(folder.join("sub")).unwrap();
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(folder.join("sub").join("big.bin"), &big).unwrap();
    net::send_files(&a, &b.id.fp, vec![f, folder], false).await.unwrap();
    let got = std::fs::read_dir(db.path().join("inbox")).unwrap().next().unwrap().unwrap().path();
    assert_eq!(std::fs::read(got.join("hello.txt")).unwrap(), b"hi there");
    assert_eq!(std::fs::read(got.join("photos/sub/big.bin")).unwrap(), big);

    // Revoking a capability on the receiver is enforced.
    b.cfg.lock().unwrap().trusted.get_mut(&a.id.fp).unwrap().caps.files = false;
    let f2 = _da.path().join("hello.txt");
    assert!(net::send_files(&a, &b.id.fp, vec![f2], false).await.is_err());

    // A device advertising someone else's fingerprint is refused.
    let (c, _dc) = node("Mallory").await;
    a.peers.lock().unwrap().get_mut(&b.id.fp).unwrap().port = c.port;
    let e = net::open(&a, &b.id.fp).await.unwrap_err();
    assert!(format!("{e:#}").contains("identity mismatch"), "{e:#}");

    // Block: later requests are dropped without a prompt.
    see(&c, &b);
    let h = answer(b.clone(), Decision::Block);
    assert!(net::pair(&c, &b.id.fp).await.is_err());
    h.await.unwrap();
    assert!(b.is_blocked(&c.id.fp));
    assert!(net::pair(&c, &b.id.fp).await.is_err());
    assert!(b.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn connect_by_address_learns_the_peer() {
    let (a, _da) = node("Alpha").await;
    let (b, _db) = node("Beta").await;
    let host = format!("127.0.0.1:{}", b.port);
    let fp = net::connect_address(&a, &host).await.unwrap();
    assert_eq!(fp, b.id.fp);
    let p = a.peer(&fp).unwrap();
    assert_eq!((p.name.as_str(), p.port), ("Beta", b.port));
    // It answers "who are you" (incl. whether it's locked) once known.
    let h = net::hello(&a, &fp).await.unwrap();
    assert_eq!(h.name, "Beta");
    // Not trusted yet: adding by address grants nothing.
    assert!(!a.knows(&fp));
    assert!(net::connect_address(&a, "").await.is_err());
    assert!(net::connect_address(&a, &format!("127.0.0.1:{}", a.port)).await.unwrap_err().to_string().contains("this computer"));
}
