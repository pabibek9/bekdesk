//! Sound from a computer this one controls with Share Input, played here instead of there.
//! The controlled computer (Linux) moves its sound to a silent bekdesk output while streaming, so its own
//! speakers stay quiet, and sends that output's raw 48 kHz 16-bit stereo over the bekdesk TLS channel.
//! Uses the PulseAudio tools (`pactl`, `parec`, `pacat`), which also drive PipeWire on current distributions.

use crate::app::{App, Mode};
use crate::net::Req;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncWrite};
#[cfg(windows)]
use tokio::io::AsyncReadExt;
#[cfg(not(windows))]
use tokio::io::AsyncWriteExt;

#[cfg_attr(not(windows), allow(dead_code))]
const RATE: u32 = 48_000;
#[cfg_attr(not(windows), allow(dead_code))]
const CHANNELS: u16 = 2;
/// 50 ms of sound.
#[cfg_attr(not(windows), allow(dead_code))]
const CHUNK: usize = RATE as usize * CHANNELS as usize * 2 / 20;
const SINK: &str = "bekdesk_remote";

fn pactl(args: &[&str]) -> Option<String> {
    let o = std::process::Command::new("pactl").args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Ids in the first column of `pactl list short ...` output.
fn ids(short: &str) -> Vec<&str> {
    short.lines().filter_map(|l| l.split_whitespace().next()).filter(|id| id.chars().all(|c| c.is_ascii_digit())).collect()
}

/// Puts sound back on the speakers when streaming ends, however it ends.
struct Restore {
    default: String,
    module: String,
}
impl Drop for Restore {
    fn drop(&mut self) {
        pactl(&["set-default-sink", &self.default]);
        for id in pactl(&["list", "short", "sink-inputs"]).unwrap_or_default().lines().filter_map(|l| ids(l).first().map(|s| s.to_string())) {
            pactl(&["move-sink-input", &id, &self.default]);
        }
        pactl(&["unload-module", &self.module]);
    }
}

/// Controlled side: whether this computer can send its sound.
pub fn can_send() -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("sending a computer's sound is only available from Linux so far");
    }
    if pactl(&["info"]).is_none() {
        bail!("no sound server found (pactl from pulseaudio-utils is needed)");
    }
    Ok(())
}

/// Controlled side: stream this computer's sound into `out` until the other side stops listening.
pub async fn send<W: AsyncWrite + Unpin>(out: &mut W) -> Result<()> {
    can_send()?;
    let default = pactl(&["get-default-sink"]).context("no sound output here")?;
    // shortcut: a bekdesk output left behind by a crash makes this fail until it's removed or the PC restarts.
    let module = pactl(&["load-module", "module-null-sink", &format!("sink_name={SINK}"), "sink_properties=device.description=bekdesk"]).context("couldn't create bekdesk's sound output")?;
    let _restore = Restore { default, module };
    pactl(&["set-default-sink", SINK]);
    for id in ids(&pactl(&["list", "short", "sink-inputs"]).unwrap_or_default()) {
        pactl(&["move-sink-input", id, SINK]);
    }
    let mut rec = tokio::process::Command::new("parec")
        .args([&format!("--device={SINK}.monitor"), "--format=s16le", "--rate=48000", "--channels=2", "--latency-msec=30", "--raw"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("couldn't record this computer's sound (parec from pulseaudio-utils is needed)")?;
    let mut pcm = rec.stdout.take().context("no sound from parec")?;
    tokio::io::copy(&mut pcm, out).await?;
    Ok(())
}

/// Listening side, per controlled computer: the running player, and what to show on its card.
static PLAYERS: Mutex<BTreeMap<String, tokio::task::JoinHandle<()>>> = Mutex::new(BTreeMap::new());
static STATUS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// "playing", or why sound from that computer isn't playing; keyed by fingerprint.
pub fn status() -> BTreeMap<String, String> {
    STATUS.lock().unwrap().clone()
}

/// Plays the sound of each computer this one controls (Share Input) and the user chose to hear here; stops the rest.
pub fn sync(app: &Arc<App>) {
    let want: Vec<String> = {
        let hear = app.cfg.lock().unwrap().hear.clone();
        app.sessions.lock().unwrap().iter().filter(|(fp, x)| x.origin && x.mode == Mode::Input && hear.contains(*fp)).map(|(fp, _)| fp.clone()).collect()
    };
    let mut players = PLAYERS.lock().unwrap();
    players.retain(|fp, t| {
        let keep = want.contains(fp);
        if !keep {
            t.abort();
            STATUS.lock().unwrap().remove(fp);
        }
        keep
    });
    for fp in want {
        if players.get(&fp).is_some_and(|t| !t.is_finished()) {
            continue;
        }
        let (app, key) = (app.clone(), fp.clone());
        players.insert(
            fp,
            tokio::spawn(async move {
                let r = async {
                    let (mut s, _) = crate::net::request(&app, &key, &Req::Audio).await?;
                    STATUS.lock().unwrap().insert(key.clone(), "playing".into());
                    play(&mut s).await
                }
                .await;
                let msg = r.err().map_or("stopped".to_string(), |e| format!("{e:#}"));
                app.log(format!("sound from {}: {msg}", app.peer_name(&key)));
                STATUS.lock().unwrap().insert(key, msg);
                // Wait before trying again, so a computer that can't send sound isn't asked every few seconds.
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            }),
        );
    }
}

/// Listening side: play what arrives on `input` until the connection ends.
pub async fn play<R: AsyncRead + Unpin>(input: &mut R) -> Result<()> {
    #[cfg(windows)]
    {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(2);
        let player = std::thread::spawn(move || win::play(rx));
        let mut buf = vec![0u8; CHUNK];
        let mut pending: Vec<u8> = Vec::with_capacity(CHUNK * 2);
        loop {
            let n = input.read(&mut buf).await?;
            if n == 0 || player.is_finished() {
                break;
            }
            pending.extend_from_slice(&buf[..n]);
            while pending.len() >= CHUNK {
                // A full queue means playback fell behind: drop sound rather than let the delay grow.
                let _ = tx.try_send(pending.drain(..CHUNK).collect());
            }
        }
        drop(tx);
        return player.join().map_err(|_| anyhow::anyhow!("sound player stopped"))?;
    }
    #[cfg(not(windows))]
    {
        let mut out = tokio::process::Command::new(if cfg!(target_os = "linux") { "pacat" } else { "false" })
            .args(["--format=s16le", "--rate=48000", "--channels=2", "--latency-msec=60", "--raw"])
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("couldn't play sound here (Linux needs pacat from pulseaudio-utils; macOS isn't supported yet)")?;
        let mut stdin = out.stdin.take().context("no player input")?;
        tokio::io::copy(input, &mut stdin).await?;
        stdin.shutdown().await?;
        Ok(())
    }
}

#[cfg(windows)]
mod win {
    use super::{CHANNELS, CHUNK, RATE};
    use anyhow::{bail, Result};
    use std::ptr::{addr_of, addr_of_mut};
    use windows_sys::Win32::Media::Audio::*;

    /// Plays 16-bit PCM chunks through the default output with the classic waveOut API, 5 buffers (250 ms) deep, so the delay stays short.
    pub fn play(rx: std::sync::mpsc::Receiver<Vec<u8>>) -> Result<()> {
        unsafe {
            let fmt = WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_PCM as u16,
                nChannels: CHANNELS,
                nSamplesPerSec: RATE,
                nAvgBytesPerSec: RATE * CHANNELS as u32 * 2,
                nBlockAlign: CHANNELS * 2,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            let mut hwo: HWAVEOUT = std::ptr::null_mut();
            if waveOutOpen(&mut hwo, WAVE_MAPPER, &fmt, 0, 0, CALLBACK_NULL) != 0 {
                bail!("couldn't open this computer's speakers");
            }
            let size = std::mem::size_of::<WAVEHDR>() as u32;
            let mut slots: Vec<(Box<WAVEHDR>, Vec<u8>, bool)> = (0..5).map(|_| (Box::new(std::mem::zeroed()), vec![0u8; CHUNK], false)).collect();
            let mut next = 0;
            while let Ok(chunk) = rx.recv() {
                let (hdr, data, used) = &mut slots[next];
                let h: *mut WAVEHDR = &mut **hdr;
                // Wait for this buffer to finish playing before reusing it.
                while *used && addr_of!((*h).dwFlags).read_unaligned() & WHDR_DONE == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                if *used {
                    waveOutUnprepareHeader(hwo, h, size);
                }
                data[..chunk.len()].copy_from_slice(&chunk);
                *h = std::mem::zeroed();
                addr_of_mut!((*h).lpData).write_unaligned(data.as_mut_ptr());
                addr_of_mut!((*h).dwBufferLength).write_unaligned(chunk.len() as u32);
                waveOutPrepareHeader(hwo, h, size);
                waveOutWrite(hwo, h, size);
                *used = true;
                next = (next + 1) % slots.len();
            }
            waveOutReset(hwo);
            for (hdr, _, used) in slots.iter_mut() {
                if *used {
                    waveOutUnprepareHeader(hwo, &mut **hdr, size);
                }
            }
            waveOutClose(hwo);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_pactl_ids() {
        let out = "71\t56\t70\tPipeWire\ts16le 2ch 48000Hz\n93\t56\t92\tPipeWire\tfloat32le 2ch 48000Hz\n";
        assert_eq!(super::ids(out), ["71", "93"]);
        assert!(super::ids("").is_empty());
        assert!(super::ids("Failure: No such entity").is_empty());
    }

    #[test]
    fn chunk_is_whole_frames() {
        // 50 ms of 48 kHz stereo 16-bit: 2400 frames of 4 bytes.
        assert_eq!(super::CHUNK, 9600);
        assert_eq!(super::CHUNK % 4, 0);
    }
}
