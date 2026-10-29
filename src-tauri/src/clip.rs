//! Clipboard sync for Share Input sessions. Deskflow's own clipboard sharing is disabled so that
//! bekdesk alone owns the clipboard: text and images are pushed immediately, copied files are
//! transferred and placed on the peer's clipboard so a normal paste works there.

use crate::app::{App, Cap};
use anyhow::{anyhow, Result};
use clipboard_rs::common::{RustImage, RustImageData};
use clipboard_rs::{Clipboard, ClipboardContext, ClipboardHandler, ClipboardWatcher, ClipboardWatcherContext, ContentFormat};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

/// Largest copy that is sent automatically; bigger ones go through the drop target.
const AUTO_FILES_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

fn hash(b: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

enum Content {
    Files(Vec<PathBuf>),
    Image(Vec<u8>),
    Text(String),
}

fn read(ctx: &ClipboardContext) -> Option<(Content, u64)> {
    if ctx.has(ContentFormat::Files) {
        let files: Vec<PathBuf> = ctx.get_files().ok()?.into_iter().map(|f| PathBuf::from(f.trim_start_matches("file://"))).collect();
        let key = files.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join("\n");
        return (!files.is_empty()).then(|| (Content::Files(files), hash(key.as_bytes())));
    }
    if ctx.has(ContentFormat::Image) {
        let png = ctx.get_image().ok()?.to_png().ok()?.get_bytes().to_vec();
        let h = hash(&png);
        return Some((Content::Image(png), h));
    }
    let t = ctx.get_text().ok()?;
    let h = hash(t.as_bytes());
    (!t.is_empty()).then_some((Content::Text(t), h))
}

/// Remember what we just wrote so the watcher doesn't echo it back.
fn remember(app: &App) {
    if let Ok(ctx) = ClipboardContext::new() {
        if let Some((_, h)) = read(&ctx) {
            *app.clip_last.lock().unwrap() = h;
        }
    }
}

pub fn apply(app: &App, kind: &str, data: Vec<u8>) -> Result<()> {
    let ctx = ClipboardContext::new().map_err(|e| anyhow!("{e}"))?;
    match kind {
        "text" => ctx.set_text(String::from_utf8(data)?).map_err(|e| anyhow!("{e}"))?,
        "image" => ctx.set_image(RustImageData::from_bytes(&data).map_err(|e| anyhow!("{e}"))?).map_err(|e| anyhow!("{e}"))?,
        _ => return Ok(()),
    }
    remember(app);
    Ok(())
}

pub fn set_files(app: &App, paths: &[PathBuf]) {
    if let Ok(ctx) = ClipboardContext::new() {
        let list = paths.iter().map(|p| p.to_string_lossy().to_string()).collect();
        if ctx.set_files(list).is_ok() {
            remember(app);
        }
    }
}

struct Watch {
    app: Arc<App>,
    rt: tokio::runtime::Handle,
}

impl ClipboardHandler for Watch {
    fn on_clipboard_change(&mut self) {
        let peers: Vec<String> = self.app.input_peers().into_iter().filter(|fp| self.app.allowed(fp, Cap::Clipboard)).collect();
        if peers.is_empty() {
            return;
        }
        let Ok(ctx) = ClipboardContext::new() else { return };
        let Some((content, h)) = read(&ctx) else { return };
        {
            let mut last = self.app.clip_last.lock().unwrap();
            if *last == h {
                return;
            }
            *last = h;
        }
        let app = self.app.clone();
        self.rt.spawn(async move {
            for fp in peers {
                let r = match &content {
                    Content::Text(t) => crate::net::send_clip(&app, &fp, "text", t.as_bytes()).await,
                    Content::Image(png) => crate::net::send_clip(&app, &fp, "image", png).await,
                    Content::Files(files) => send_copied_files(&app, &fp, files.clone()).await,
                };
                if let Err(e) = r {
                    app.log(format!("clipboard → {}: {e:#}", app.peer_name(&fp)));
                }
            }
        });
    }
}

async fn send_copied_files(app: &App, fp: &str, files: Vec<PathBuf>) -> Result<()> {
    if !app.allowed(fp, Cap::Files) {
        return Ok(());
    }
    let f2 = files.clone();
    let size: u64 = tokio::task::spawn_blocking(move || crate::net::plan_files(&f2)).await??.iter().map(|(_, i)| i.size).sum();
    if size > AUTO_FILES_LIMIT {
        app.note(fp, "Copied files are over 2 GB — drop them on this device to send");
        return Ok(());
    }
    crate::net::send_files(app, fp, files, true).await
}

pub fn start(app: Arc<App>, rt: tokio::runtime::Handle) {
    std::thread::spawn(move || {
        let Ok(mut w) = ClipboardWatcherContext::new() else {
            app.log("clipboard watcher unavailable");
            return;
        };
        w.add_handler(Watch { app, rt });
        w.start_watch();
    });
}

