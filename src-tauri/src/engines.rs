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

/// What an engine does, in words for people (the UI never names the projects bekdesk builds on).
pub fn purpose(name: &str) -> &'static str {
    match name {
        "deskflow" | "deskflow-rpm" => "keyboard & mouse sharing",
        "sunshine" => "screen sharing",
        "moonlight" => "screen viewer",
        "vdd" => "the virtual screen",
        _ => "a bekdesk component",
    }
}

/// Returns the engine's executable (or INF for the display driver), downloading it if needed.
pub async fn ensure(app: &App, name: &str, note_key: &str) -> Result<PathBuf> {
    // Linux: use a Deskflow that is already installed (package manager, or the Flatpak installed by hand); on
    // Fedora 44 install Deskflow's own small package with the usual password prompt; otherwise fall through to
    // the Flatpak bundle below (installed for this user, no password, but a large first download).
    if cfg!(target_os = "linux") && name == "deskflow" {
        let on_path = || std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|d| d.join("deskflow-core")).find(|p| p.is_file()));
        if let Some(p) = on_path() {
            return Ok(p);
        }
        if std::process::Command::new("flatpak").args(["info", "org.deskflow.deskflow"]).output().is_ok_and(|o| o.status.success()) {
            let dir = app.dir.join("engines").join("deskflow-flatpak");
            std::fs::create_dir_all(&dir)?;
            return flatpak_launcher(&dir, &app.dir);
        }
        let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        if os.lines().any(|l| l == "ID=fedora") && os.lines().any(|l| l == "VERSION_ID=44") {
            let _ = Box::pin(ensure(app, "deskflow-rpm", note_key)).await;
            if let Some(p) = on_path() {
                return Ok(p);
            }
        }
    }
    let p = pkg(name).with_context(|| match name {
        "sunshine" | "moonlight" | "vdd" if cfg!(target_os = "linux") => "showing screens isn't available on Linux yet; Share Input, clipboard and files work".to_string(),
        _ => format!("{name} is not available on {}", platform()),
    })?;
    let dir = dir_for(app, name, &p);
    let _guard = app.engine_lock.lock().await;
    if !dir.join(".ok").exists() {
        let r = install(app, name, &p, &dir, note_key).await;
        app.note(note_key, "");
        r.with_context(|| format!("setting up {}", purpose(name)))?;
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
            app.note(note_key, format!("Setting up {} (one-time download) — {pct}%", purpose(name)));
        }
    }
    out.flush().await?;
    drop(out);
    if hex::encode(h.finalize()) != p.sha256 {
        let _ = std::fs::remove_dir_all(dir);
        bail!("checksum mismatch for {}", p.url);
    }
    app.note(note_key, match file.extension().and_then(|e| e.to_str()) {
        Some("rpm") => "Setting up keyboard & mouse sharing: type your password in the window that opens".to_string(),
        Some("flatpak") => "Setting up keyboard & mouse sharing: the first time this downloads about 1 GB and can take 10 minutes or more".to_string(),
        _ => format!("Setting up {}…", purpose(name)),
    });
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
        // Linux: Deskflow's official Flatpak, installed for this user only (no root), and a small launcher that
        // runs its deskflow-core with access to bekdesk's folder (where its settings files are written).
        "flatpak" => {
            let ok = |args: &[&str]| std::process::Command::new("flatpak").args(args).status().is_ok_and(|s| s.success());
            ok(&["remote-add", "--user", "--if-not-exists", "flathub", "https://dl.flathub.org/repo/flathub.flatpakrepo"]);
            if !ok(&["install", "--user", "-y", "--noninteractive", &file.to_string_lossy()]) {
                bail!("couldn't install it (is flatpak installed?)");
            }
            flatpak_launcher(dir, dir.parent().and_then(Path::parent).context("bad engine folder")?)?;
        }
        // Fedora: Deskflow's own package, installed system-wide after the normal password prompt.
        "rpm" => {
            if !std::process::Command::new("pkexec").args(["dnf", "install", "-y"]).arg(file).status().is_ok_and(|s| s.success()) {
                bail!("it wasn't installed (password prompt cancelled or dnf failed)");
            }
        }
        // Linux viewer: Moonlight's AppImage runs as is (main() makes AppImages run without FUSE).
        "AppImage" => {
            let exe = dir.join("moonlight.AppImage");
            std::fs::copy(file, &exe)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))?;
            }
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
                failed = Some(format!("{e:#}"));
            }
        }
        match failed {
            None => {
                app.note("components", "");
                return;
            }
            Some(e) => app.note("components", format!("Couldn't set up {e}. Retrying in a minute…")),
        }
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

/// Portable Moonlight keeps its identity and paired hosts in its working directory (it looks for
/// `portable.dat` there). Run it from a folder that outlives engine updates, or every update means a new
/// identity and a PIN prompt. Carries over what an earlier version stored in the engine folder.
pub fn moonlight_home(app: &App, exe: &Path) -> PathBuf {
    let home = app.dir.join("moonlight");
    let _ = std::fs::create_dir_all(&home);
    let Some(src) = exe.parent() else { return home };
    if !home.join("portable.dat").exists() && src.join("portable.dat").exists() {
        let _ = std::fs::copy(src.join("portable.dat"), home.join("portable.dat"));
        let old = src.join("Moonlight Game Streaming Project");
        if let Ok(rd) = std::fs::read_dir(&old) {
            let _ = std::fs::create_dir_all(home.join("Moonlight Game Streaming Project"));
            for e in rd.flatten() {
                let _ = std::fs::copy(e.path(), home.join("Moonlight Game Streaming Project").join(e.file_name()));
            }
        }
    }
    home
}

/// A `deskflow-core` launcher for the Deskflow Flatpak, with access to bekdesk's folder (`data`) where its
/// settings files are written.
fn flatpak_launcher(dir: &Path, data: &Path) -> Result<PathBuf> {
    let launcher = dir.join("deskflow-core");
    std::fs::write(&launcher, format!("#!/bin/sh\nexec flatpak run --die-with-parent --filesystem='{}' --command=deskflow-core org.deskflow.deskflow \"$@\"\n", data.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(launcher)
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
    spawn_in(app, exe, args, log, exe.parent().unwrap_or(Path::new(".")))
}

pub fn spawn_in(app: &App, exe: &Path, args: &[String], log: &str, cwd: &Path) -> Result<tokio::process::Child> {
    let f = std::fs::File::create(app.dir.join("logs").join(log))?;
    let mut c = tokio::process::Command::new(exe);
    c.args(args).stdin(std::process::Stdio::null()).stdout(f.try_clone()?).stderr(f).kill_on_drop(true).current_dir(cwd);
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
