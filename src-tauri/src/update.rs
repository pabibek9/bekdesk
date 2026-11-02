//! Updates from the project's GitHub releases: check for a newer version, then download this platform's
//! installer, check it against the release's SHA256SUMS.txt and install it.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Where releases are published. They must be readable without signing in (a public repository, or a public
/// repository used only for releases) for installed copies to see them.
pub const REPO: &str = "pabibek9/bekdesk";

/// The last check's result, for the UI: newest version available, or why checking failed.
pub static STATE: Mutex<Option<Result<Option<String>, String>>> = Mutex::new(None);

/// "0.1.18" or "v0.1.18" as numbers, for comparing.
fn parse(v: &str) -> Vec<u64> {
    v.trim().trim_start_matches('v').split(['.', '-']).map_while(|p| p.parse().ok()).collect()
}

/// Whether `latest` is a newer version than `current`.
pub fn newer(current: &str, latest: &str) -> bool {
    parse(latest) > parse(current)
}

/// The release file meant for this computer.
fn asset_name() -> Option<&'static str> {
    if cfg!(windows) {
        Some("bekdesk-Windows-x64-Setup.exe")
    } else if cfg!(target_os = "macos") {
        Some("bekdesk-macOS-universal.dmg")
    } else if std::env::var_os("APPIMAGE").is_some() {
        Some("bekdesk-Linux-x86_64.AppImage")
    } else if Path::new("/usr/bin/dnf").exists() || Path::new("/usr/bin/rpm").exists() && !Path::new("/usr/bin/apt").exists() {
        Some("bekdesk-Linux-x86_64.rpm")
    } else if Path::new("/usr/bin/apt").exists() {
        Some("bekdesk-Linux-amd64.deb")
    } else {
        None
    }
}

async fn latest_release() -> Result<Value> {
    let client = reqwest::Client::builder().user_agent("bekdesk-updater").timeout(std::time::Duration::from_secs(20)).build()?;
    // bekdesk publishes preview releases, which /releases/latest leaves out: take the newest published one.
    let r = client.get(format!("https://api.github.com/repos/{REPO}/releases?per_page=10")).send().await?;
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        bail!("no public release found");
    }
    let all: Vec<Value> = r.error_for_status()?.json().await?;
    all.into_iter().find(|r| r["draft"] != true).context("no release published yet")
}

/// Asks GitHub for the newest release; remembers and returns its version if it's newer than this one.
pub async fn check() -> Result<Option<String>, String> {
    let r = async {
        let rel = latest_release().await?;
        let tag = rel["tag_name"].as_str().context("release without a version")?.trim_start_matches('v').to_string();
        Ok::<_, anyhow::Error>(newer(env!("CARGO_PKG_VERSION"), &tag).then_some(tag))
    }
    .await
    .map_err(|e| format!("{e:#}"));
    *STATE.lock().unwrap() = Some(r.clone());
    r
}

/// SHA-256 listed for `name` in a SHA256SUMS.txt ("<hash>  <file>" per line).
fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, file) = l.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| hash.to_lowercase())
    })
}

/// Downloads the newest installer for this computer, checks it, and starts installing it. Returns once the
/// installer runs; the caller then quits so files can be replaced.
pub async fn install(dir: &Path) -> Result<()> {
    let name = asset_name().context("no update package for this kind of computer; download it from the releases page")?;
    let rel = latest_release().await?;
    let url = |n: &str| rel["assets"].as_array().into_iter().flatten().find(|a| a["name"] == n).and_then(|a| a["browser_download_url"].as_str()).map(String::from);
    let (file_url, sums_url) = (url(name).with_context(|| format!("{name} is missing from the release"))?, url("SHA256SUMS.txt").context("the release has no SHA256SUMS.txt")?);
    let sums = reqwest::get(&sums_url).await?.error_for_status()?.text().await?;
    let want = sum_for(&sums, name).with_context(|| format!("SHA256SUMS.txt doesn't list {name}"))?;
    let bytes = reqwest::get(&file_url).await?.error_for_status()?.bytes().await?;
    if hex::encode(Sha256::digest(&bytes)) != want {
        bail!("the download doesn't match the release's checksum; not installing it");
    }
    let target: PathBuf = dir.join("update").join(name);
    std::fs::create_dir_all(target.parent().unwrap())?;
    std::fs::write(&target, &bytes)?;
    run(&target)
}

/// Starts installing `file` in a process that outlives bekdesk, and reopens bekdesk afterwards where possible.
fn run(file: &Path) -> Result<()> {
    let me = std::env::current_exe()?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Silent per-user install, then start the new version. Breakaway: bekdesk's job closes its children on exit.
        // raw_arg: cmd has its own quoting rules, which the normal argument escaping would break.
        std::process::Command::new("cmd")
            .arg("/c")
            .raw_arg(format!("start \"\" /wait \"{}\" /S & start \"\" \"{}\"", file.display(), me.display()))
            .creation_flags(0x0800_0000 | 0x0100_0000)
            .spawn()
            .context("starting the installer")?;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = &me;
        // The disk image opens; drag bekdesk to Applications to replace the old one.
        std::process::Command::new("open").arg(file).spawn().context("opening the update")?;
    }
    #[cfg(target_os = "linux")]
    {
        let f = file.display().to_string().replace('\'', "'\\''");
        let script = match std::env::var("APPIMAGE") {
            Ok(img) => format!("sleep 1; cp '{f}' '{0}' && chmod +x '{0}' && '{0}'", img.replace('\'', "'\\''")),
            Err(_) if f.ends_with(".rpm") => format!("pkexec dnf install -y '{f}' && '{}'", me.display()),
            Err(_) => format!("pkexec apt-get install -y '{f}' && '{}'", me.display()),
        };
        std::process::Command::new("sh").args(["-c", &format!("({script}) >/dev/null 2>&1 &")]).spawn().context("starting the update")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(newer("0.1.17", "v0.1.18"));
        assert!(newer("0.1.9", "0.1.10"));
        assert!(newer("0.1.17", "0.2.0"));
        assert!(!newer("0.1.17", "0.1.17"));
        assert!(!newer("0.1.18", "v0.1.17"));
    }

    #[test]
    fn finds_checksum() {
        let sums = "abc123  bekdesk-Windows-x64-Setup.exe\nDEF456 *bekdesk-Linux-x86_64.rpm\n";
        assert_eq!(sum_for(sums, "bekdesk-Windows-x64-Setup.exe").as_deref(), Some("abc123"));
        assert_eq!(sum_for(sums, "bekdesk-Linux-x86_64.rpm").as_deref(), Some("def456"));
        assert_eq!(sum_for(sums, "bekdesk-macOS-universal.dmg"), None);
    }
}
