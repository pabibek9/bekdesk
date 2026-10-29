//! Upstream engines (Deskflow, Sunshine, Moonlight, Virtual Display Driver) are fetched on first use
//! from their official GitHub releases, pinned by SHA-256 in `engines.json`, and run as separate
//! processes. bekdesk never links their code.

use crate::app::App;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

const MANIFEST: &str = include_str!("../../engines.json");

#[derive(Deserialize, Clone)]
pub struct Pkg {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub exe: String,
}

pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

pub fn pkg(name: &str) -> Option<Pkg> {
    let all: HashMap<String, HashMap<String, Pkg>> = serde_json::from_str(MANIFEST).ok()?;
    all.get(name)?.get(&platform()).cloned()
}

fn dir_for(app: &App, name: &str, p: &Pkg) -> PathBuf {
    app.dir.join("engines").join(format!("{name}-{}", &p.sha256[..12]))
}

pub fn installed(app: &App) -> Value {
    let mut out = serde_json::Map::new();
    for name in ["deskflow", "sunshine", "moonlight", "vdd"] {
        let v = match pkg(name) {
            Some(p) => json!({"version": p.version, "installed": dir_for(app, name, &p).join(".ok").exists()}),
            None => json!({"version": null, "installed": false}),
        };
        out.insert(name.into(), v);
    }
    Value::Object(out)
}

fn find(dir: &Path, exe: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let ft = e.file_type().ok()?;
        if ft.is_dir() {
            if let Some(p) = find(&e.path(), exe) {
                return Some(p);
            }
        } else if ft.is_file() && e.file_name().to_string_lossy().eq_ignore_ascii_case(exe) {
            return Some(e.path());
        }
    }
    None
}

/// Returns the engine's executable (or INF for the display driver), downloading it if needed.
pub async fn ensure(app: &App, name: &str, note_key: &str) -> Result<PathBuf> {
    let p = pkg(name).with_context(|| format!("{name} is not available on {}", platform()))?;
    let dir = dir_for(app, name, &p);
    let _guard = app.engine_lock.lock().await;
    if !dir.join(".ok").exists() {
        let r = install(app, name, &p, &dir, note_key).await;
        app.note(note_key, "");
        r.with_context(|| format!("installing {name} {}", p.version))?;
    }
    find(&dir, &p.exe).with_context(|| format!("{} missing from {name} package", p.exe))
}

async fn install(app: &App, name: &str, p: &Pkg, dir: &Path, note_key: &str) -> Result<()> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    let file = dir.join(p.url.rsplit('/').next().unwrap_or("download"));
    let mut resp = reqwest::get(&p.url).await?.error_for_status()?;
    let total = resp.content_length().unwrap_or(0);
    let mut out = tokio::fs::File::create(&file).await?;
    let mut h = Sha256::new();
    let mut got = 0u64;
    let mut last_pct = 101;
    while let Some(chunk) = resp.chunk().await? {
        h.update(&chunk);
        out.write_all(&chunk).await?;
        got += chunk.len() as u64;
        let pct = if total > 0 { got * 100 / total } else { 0 };
        if pct != last_pct {
            last_pct = pct;
            app.note(note_key, format!("Downloading {name} {} — {pct}%", p.version));
        }
    }
    out.flush().await?;
    drop(out);
    if hex::encode(h.finalize()) != p.sha256 {
        let _ = std::fs::remove_dir_all(dir);
        bail!("checksum mismatch for {}", p.url);
    }
    app.note(note_key, format!("Installing {name} {}", p.version));
    let (f2, d2) = (file.clone(), dir.to_path_buf());
    tokio::task::spawn_blocking(move || extract(&f2, &d2)).await??;
    let _ = std::fs::remove_file(&file);
    std::fs::write(dir.join(".ok"), &p.version)?;
    app.log(format!("installed {name} {}", p.version));
    Ok(())
}

fn extract(file: &Path, dir: &Path) -> Result<()> {
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "zip" => zip::ZipArchive::new(std::fs::File::open(file)?)?.extract(dir)?,
        "dmg" => {
            let mnt = dir.join(".mnt");
            std::fs::create_dir_all(&mnt)?;
            let ok = std::process::Command::new("hdiutil").args(["attach", "-nobrowse", "-readonly", "-mountpoint"]).arg(&mnt).arg(file).status()?.success();
            if !ok {
                bail!("hdiutil attach failed");
            }
            let mut res = Ok(());
            for e in std::fs::read_dir(&mnt)?.flatten() {
                if e.path().extension().is_some_and(|x| x == "app") {
                    if !std::process::Command::new("ditto").arg(e.path()).arg(dir.join(e.file_name())).status()?.success() {
                        res = Err(anyhow::anyhow!("copy from dmg failed"));
                    }
                }
            }
            let _ = std::process::Command::new("hdiutil").args(["detach", "-force"]).arg(&mnt).status();
            let _ = std::fs::remove_dir(&mnt);
            res?
        }
        _ => bail!("unsupported package {}", file.display()),
    }
    Ok(())
}

/// Downloads every engine this OS can use in the background, so the first connection needs no wait.
/// Progress shows under the "components" note; offline failures are retried every minute.
pub async fn prefetch(app: std::sync::Arc<App>) {
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    loop {
        let mut failed = None;
        for name in ["deskflow", "sunshine", "moonlight", "vdd"] {
            let Some(p) = pkg(name) else { continue };
            if dir_for(&app, name, &p).join(".ok").exists() {
                continue;
            }
            if let Err(e) = ensure(&app, name, "components").await {
                app.log(format!("downloading {name}: {e:#}"));
                failed = Some(name);
            }
        }
        match failed {
            None => {
                app.note("components", "");
                return;
            }
            Some(n) => app.note("components", format!("Couldn't download {n} (no internet?). Retrying…")),
        }
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

/// Engines left running by an earlier bekdesk (crash, update, uninstall) keep serving old streams and lock
/// their files so updates fail ("used by another process"). Stop anything running from our engines folder.
pub fn kill_orphans(app: &App) {
    let dir = app.dir.join("engines");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!(
                "Get-Process | Where-Object {{ $_.Path -and $_.Path.StartsWith('{}', 'OrdinalIgnoreCase') }} | Stop-Process -Force",
                dir.display().to_string().replace('\'', "''")
            ))
            .creation_flags(0x0800_0000)
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("pkill").arg("-f").arg(&dir).status();
    }
}

/// Spawn an engine process with output captured to the bekdesk log folder.
pub fn spawn(app: &App, exe: &Path, args: &[String], log: &str) -> Result<tokio::process::Child> {
    let f = std::fs::File::create(app.dir.join("logs").join(log))?;
    let mut c = tokio::process::Command::new(exe);
    c.args(args).stdin(std::process::Stdio::null()).stdout(f.try_clone()?).stderr(f).kill_on_drop(true);
    if let Some(d) = exe.parent() {
        c.current_dir(d);
    }
    #[cfg(windows)]
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: engines are console programs
    c.spawn().with_context(|| format!("starting {}", exe.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_covers_desktop_platforms() {
        let all: HashMap<String, HashMap<String, Pkg>> = serde_json::from_str(MANIFEST).unwrap();
        for e in ["deskflow", "sunshine", "moonlight"] {
            for p in ["windows-x86_64", "macos-aarch64", "macos-x86_64"] {
                let pkg = &all[e][p];
                assert_eq!(pkg.sha256.len(), 64, "{e} {p}");
                assert!(pkg.url.starts_with("https://github.com/"), "{e} {p}");
            }
        }
    }
}
