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

/// Whether this computer can send its sound (Windows and Linux; not macOS yet).
pub fn can_send() -> Result<()> {
    if cfg!(windows) {
        return Ok(());
    }
    if !cfg!(target_os = "linux") {
        bail!("sending sound from a Mac isn't available yet");
    }
    if pactl(&["info"]).is_none() {
        bail!("no sound server found (pactl from pulseaudio-utils is needed)");
    }
    Ok(())
}

/// Stream this computer's sound into `out` until the other side stops listening. With `mute_here` (Linux only:
/// the computer being controlled) its own speakers go quiet while the sound plays on the other computer.
pub async fn send<W: AsyncWrite + Unpin>(out: &mut W, mute_here: bool) -> Result<()> {
    can_send()?;
    #[cfg(windows)]
    {
        let _ = mute_here; // Windows can't silence its speakers without a virtual sound device
        return win_capture::send(out, true).await;
    }
    #[cfg(not(windows))]
    if !mute_here {
        // Keep playing here too: record what the speakers play.
        let mut rec = tokio::process::Command::new("parec")
            .args(["--device=@DEFAULT_MONITOR@", "--format=s16le", "--rate=48000", "--channels=2", "--latency-msec=30", "--raw"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("couldn't record this computer's sound (parec from pulseaudio-utils is needed)")?;
        let mut pcm = rec.stdout.take().context("no sound from parec")?;
        tokio::io::copy(&mut pcm, out).await?;
        return Ok(());
    }
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

/// Keeps the sound streams the user picked running, for every computer connected with Share Input (either way):
/// "pull" plays that computer's sound here (`hear`), "push" plays this computer's sound there (`sound`).
/// Ticking "push" on several computers plays this PC's sound on all their speakers.
/// shortcut: speakers aren't sample-synced (each adds its own small buffer); add timestamps if echo bothers people.
pub fn sync(app: &Arc<App>) {
    let want: Vec<String> = {
        let cfg = app.cfg.lock().unwrap();
        let linked = app.sessions.lock().unwrap().iter().filter(|(_, x)| x.mode == Mode::Input).map(|(fp, _)| fp.clone()).collect::<Vec<_>>();
        linked
            .iter()
            .flat_map(|fp| {
                let pull = cfg.hear.contains(fp).then(|| format!("pull:{fp}"));
                let push = (cfg.sound.get(fp) == Some(&true) && can_send().is_ok()).then(|| format!("push:{fp}"));
                let mic = cfg.use_mic.contains(fp).then(|| format!("mic:{fp}"));
                pull.into_iter().chain(push).chain(mic)
            })
            .collect()
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
        let app = app.clone();
        let (dir, key) = fp.split_once(':').map(|(d, k)| (d.to_string(), k.to_string())).unwrap_or_default();
        players.insert(
            fp.clone(),
            tokio::spawn(async move {
                let r = async {
                    if dir == "mic" {
                        let (mut s, _) = crate::net::request(&app, &key, &Req::Mic).await?;
                        STATUS.lock().unwrap().insert(fp.clone(), "playing".into());
                        play_on(&mut s, true).await
                    } else if dir == "push" {
                        let (mut s, _) = crate::net::request(&app, &key, &Req::AudioPush).await?;
                        STATUS.lock().unwrap().insert(fp.clone(), "playing".into());
                        send(&mut s, false).await
                    } else {
                        let (mut s, _) = crate::net::request(&app, &key, &Req::Audio).await?;
                        STATUS.lock().unwrap().insert(fp.clone(), "playing".into());
                        play(&mut s).await
                    }
                }
                .await;
                let msg = r.err().map_or("stopped".to_string(), |e| format!("{e:#}"));
                app.log(format!("sound {} {}: {msg}", if dir == "push" { "to" } else { "from" }, app.peer_name(&key)));
                STATUS.lock().unwrap().insert(fp, msg);
                // Wait before trying again, so a computer that can't send sound isn't asked every few seconds.
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            }),
        );
    }
}

/// Stream this computer's default microphone into `out` (only for computers the user chose to share it with).
pub async fn send_mic<W: AsyncWrite + Unpin>(out: &mut W) -> Result<()> {
    #[cfg(windows)]
    return win_capture::send(out, false).await;
    #[cfg(not(windows))]
    {
        if !cfg!(target_os = "linux") {
            bail!("sharing a Mac's microphone isn't available yet");
        }
        let mut rec = tokio::process::Command::new("parec")
            .args(["--device=@DEFAULT_SOURCE@", "--format=s16le", "--rate=48000", "--channels=2", "--latency-msec=30", "--raw"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("couldn't record the microphone (parec from pulseaudio-utils is needed)")?;
        let mut pcm = rec.stdout.take().context("no sound from the microphone")?;
        tokio::io::copy(&mut pcm, out).await?;
        Ok(())
    }
}

/// Linux: a "bekdesk microphone" input that apps can pick; whatever bekdesk plays into its sink comes out of it.
#[cfg(not(windows))]
struct VirtualMic(Vec<String>);
#[cfg(not(windows))]
impl VirtualMic {
    const SINK: &'static str = "bekdesk_mic_sink";
    fn new() -> Result<Self> {
        let sink = pactl(&["load-module", "module-null-sink", &format!("sink_name={}", Self::SINK), "sink_properties=device.description=bekdesk-mic-feed"]).context("couldn't create the bekdesk microphone")?;
        let src = pactl(&["load-module", "module-remap-source", &format!("master={}.monitor", Self::SINK), "source_name=bekdesk_mic", "source_properties=device.description=bekdesk-microphone"]);
        Ok(Self([Some(sink), src].into_iter().flatten().collect()))
    }
}
#[cfg(not(windows))]
impl Drop for VirtualMic {
    fn drop(&mut self) {
        for m in self.0.iter().rev() {
            pactl(&["unload-module", m]);
        }
    }
}

/// Listening side: play what arrives on `input` until the connection ends.
pub async fn play<R: AsyncRead + Unpin>(input: &mut R) -> Result<()> {
    play_on(input, false).await
}

/// Play `input` on the speakers, or with `mic` into a microphone apps on this computer can use
/// (Linux: bekdesk's own; Windows: the free VB-CABLE, which the user installs).
pub async fn play_on<R: AsyncRead + Unpin>(input: &mut R, mic: bool) -> Result<()> {
    #[cfg(windows)]
    {
        let dev = if mic {
            win::cable().context("to use another computer's microphone here, install the free VB-CABLE from vb-audio.com/Cable, then tick this again")?
        } else {
            windows_sys::Win32::Media::Audio::WAVE_MAPPER
        };
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(2);
        let player = std::thread::spawn(move || win::play(rx, dev));
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
        let _mic = if mic { Some(VirtualMic::new()?) } else { None };
        let device = format!("--device={}", if mic { VirtualMic::SINK } else { "@DEFAULT_SINK@" });
        let mut out = tokio::process::Command::new(if cfg!(target_os = "linux") { "pacat" } else { "false" })
            .args([device.as_str(), "--format=s16le", "--rate=48000", "--channels=2", "--latency-msec=60", "--raw"])
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
    /// The "CABLE Input" playback device of VB-CABLE, whose other end apps see as a microphone.
    pub fn cable() -> Option<u32> {
        unsafe {
            (0..waveOutGetNumDevs()).find(|&i| {
                let mut caps: WAVEOUTCAPSW = std::mem::zeroed();
                waveOutGetDevCapsW(i as usize, &mut caps, std::mem::size_of::<WAVEOUTCAPSW>() as u32) == 0 && {
                    let name = caps.szPname; // copy out of the packed struct before borrowing
                    let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
                    String::from_utf16_lossy(&name[..n]).contains("CABLE Input")
                }
            })
        }
    }

    pub fn play(rx: std::sync::mpsc::Receiver<Vec<u8>>, dev: u32) -> Result<()> {
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
            if waveOutOpen(&mut hwo, dev, &fmt, 0, 0, CALLBACK_NULL) != 0 {
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

/// Windows: record what the speakers play (WASAPI loopback through cpal) as 48 kHz 16-bit stereo.
#[cfg(windows)]
mod win_capture {
    use anyhow::{bail, Context, Result};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use tokio::io::{AsyncWrite, AsyncWriteExt};

    /// `loopback`: what the speakers play; otherwise the default microphone.
    pub async fn send<W: AsyncWrite + Unpin>(out: &mut W, loopback: bool) -> Result<()> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
        // cpal streams can't move between threads; this one lives until the receiving side goes away.
        std::thread::spawn(move || {
            let r = (|| -> Result<cpal::Stream> {
                let host = cpal::default_host();
                let (dev, cfg) = if loopback {
                    let d = host.default_output_device().context("no speakers found")?;
                    let c = d.default_output_config().context("reading the speakers' format")?;
                    (d, c)
                } else {
                    let d = host.default_input_device().context("no microphone found")?;
                    let c = d.default_input_config().context("reading the microphone's format")?;
                    (d, c)
                };
                let (rate, ch) = (cfg.sample_rate().0, cfg.channels() as usize);
                let mut conv = Convert::new(rate, ch);
                let tx2 = tx.clone();
                let err = |e| eprintln!("sound capture: {e}");
                // Opening an *input* stream on an output device makes WASAPI record what it plays (loopback).
                let stream = match cfg.sample_format() {
                    cpal::SampleFormat::F32 => dev.build_input_stream(&cfg.into(), move |d: &[f32], _: &_| {
                        let _ = tx2.try_send(conv.push(d.iter().copied()));
                    }, err, None)?,
                    cpal::SampleFormat::I16 => dev.build_input_stream(&cfg.into(), move |d: &[i16], _: &_| {
                        let _ = tx2.try_send(conv.push(d.iter().map(|&s| s as f32 / 32768.0)));
                    }, err, None)?,
                    f => bail!("unsupported speaker format {f:?}"),
                };
                stream.play()?;
                Ok(stream)
            })();
            match r {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    while !tx.is_closed() {
                        std::thread::sleep(std::time::Duration::from_millis(200));
                    }
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        });
        tokio::task::spawn_blocking(move || ready_rx.recv()).await?.context("sound capture didn't start")??;
        while let Some(chunk) = rx.recv().await {
            if !chunk.is_empty() {
                out.write_all(&chunk).await?;
            }
        }
        Ok(())
    }

    /// Any channel count and rate to 48 kHz stereo s16le (linear interpolation; plenty for speakers over a LAN).
    pub struct Convert {
        ch: usize,
        step: f64,
        pos: f64,
        last: [f32; 2],
    }
    impl Convert {
        pub fn new(rate: u32, ch: usize) -> Self {
            Self { ch: ch.max(1), step: rate as f64 / 48_000.0, pos: 0.0, last: [0.0; 2] }
        }
        pub fn push(&mut self, samples: impl Iterator<Item = f32>) -> Vec<u8> {
            let v: Vec<f32> = samples.collect();
            let frames: Vec<[f32; 2]> = v.chunks(self.ch).map(|f| [f[0], *f.get(1).unwrap_or(&f[0])]).collect();
            let mut out = Vec::with_capacity(frames.len() * 8);
            while self.pos < frames.len() as f64 {
                let i = self.pos.floor() as usize;
                let t = (self.pos - i as f64) as f32;
                let a = if i == 0 { self.last } else { frames[i - 1] };
                let b = frames[i];
                for c in 0..2 {
                    let s = (a[c] + (b[c] - a[c]) * t).clamp(-1.0, 1.0);
                    out.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes());
                }
                self.pos += self.step;
            }
            self.pos -= frames.len() as f64;
            if let Some(l) = frames.last() {
                self.last = *l;
            }
            out
        }
    }

    #[test]
    fn converts_to_48k_stereo() {
        let mut c = Convert::new(48_000, 2);
        assert_eq!(c.push([0.0f32; 960].into_iter()).len(), 480 * 4); // 10 ms stereo in, 10 ms out
        let mut c = Convert::new(96_000, 1);
        assert_eq!(c.push([0.5f32; 960].into_iter()).len(), 480 * 4); // 10 ms mono at 96 kHz → 10 ms stereo at 48 kHz
    }
}
