//! Use another computer's camera here: the owner shares it, and on Windows it shows up in Zoom, Teams, browsers and
//! other camera apps as "bekdesk camera" (the bundled Softcam DirectShow filter, registered by the admin setup).
//! Frames travel as JPEG, each prefixed with its length (u32, little-endian), over the bekdesk TLS channel.

use crate::app::{App, Mode};
use crate::net::Req;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest frame accepted (a 1080p JPEG is well under this).
const MAX_FRAME: u32 = 8 * 1024 * 1024;

/// Sending side: stream this computer's camera as JPEG frames until the other side stops.
pub async fn send<W: AsyncWrite + Unpin>(out: &mut W) -> Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(3);
    capture(tx)?;
    while let Some(jpeg) = rx.recv().await {
        out.write_all(&(jpeg.len() as u32).to_le_bytes()).await?;
        out.write_all(&jpeg).await?;
    }
    bail!("the camera stopped")
}

/// Windows: read the first camera (Media Foundation) and encode each frame as JPEG.
#[cfg(windows)]
fn capture(tx: tokio::sync::mpsc::Sender<Vec<u8>>) -> Result<()> {
    use nokhwa::pixel_format::RgbFormat;
    use nokhwa::utils::{CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType};
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
    // The camera object stays on its own thread; it ends when the receiving side goes away.
    std::thread::spawn(move || {
        let opened = (|| -> Result<nokhwa::Camera> {
            let mut cam = nokhwa::Camera::new(CameraIndex::Index(0), RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 30))))
                .context("no camera found on this computer")?;
            cam.open_stream().context("the camera is busy or blocked (Windows Settings → Privacy → Camera)")?;
            Ok(cam)
        })();
        let mut cam = match opened {
            Ok(c) => {
                let _ = ready_tx.send(Ok(()));
                c
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        while !tx.is_closed() {
            let Ok(frame) = cam.frame() else { break };
            let Ok(rgb) = frame.decode_image::<RgbFormat>() else { continue };
            let (w, h) = (rgb.width() as u16, rgb.height() as u16);
            let mut jpeg = Vec::with_capacity(200_000);
            if jpeg_encoder::Encoder::new(&mut jpeg, 80).encode(rgb.as_raw(), w, h, jpeg_encoder::ColorType::Rgb).is_ok() {
                // A full queue means the network is behind: skip this frame rather than build up delay.
                let _ = tx.try_send(jpeg);
            }
        }
        let _ = cam.stop_stream();
    });
    ready_rx.recv().context("the camera didn't start")?
}

/// Linux: GStreamer (installed with GNOME) reads the camera and writes back-to-back JPEGs; split them at their
/// start/end markers (FF D8 … FF D9, which can't occur inside JPEG data).
#[cfg(not(windows))]
fn capture(tx: tokio::sync::mpsc::Sender<Vec<u8>>) -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("sharing a Mac's camera isn't available yet");
    }
    let mut child = tokio::process::Command::new("gst-launch-1.0")
        .args(["-q", "v4l2src", "!", "videoconvert", "!", "videoscale", "!", "video/x-raw,width=1280,height=720", "!", "videorate", "!", "video/x-raw,framerate=30/1", "!", "jpegenc", "quality=80", "!", "fdsink"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("couldn't start the camera (GStreamer's gst-launch-1.0 is needed: sudo dnf install gstreamer1-plugins-good)")?;
    let mut out = child.stdout.take().context("no picture from the camera")?;
    tokio::spawn(async move {
        let _child = child; // ends the camera when this task ends
        let (mut buf, mut chunk) = (Vec::<u8>::new(), vec![0u8; 64 * 1024]);
        while let Ok(n) = out.read(&mut chunk).await {
            if n == 0 || tx.is_closed() {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            while let Some((a, b)) = jpeg_bounds(&buf) {
                let _ = tx.try_send(buf[a..b].to_vec());
                buf.drain(..b);
            }
            if buf.len() > MAX_FRAME as usize {
                buf.clear();
            }
        }
    });
    Ok(())
}

/// First complete JPEG in `buf`: start of its FF D8 and just past its FF D9.
#[cfg_attr(windows, allow(dead_code))]
fn jpeg_bounds(buf: &[u8]) -> Option<(usize, usize)> {
    let a = buf.windows(2).position(|w| w == [0xFF, 0xD8])?;
    let e = buf[a + 2..].windows(2).position(|w| w == [0xFF, 0xD9])?;
    Some((a, a + 2 + e + 2))
}

/// Receiving side: show the frames as this computer's "bekdesk camera" until the connection ends.
pub async fn play<R: AsyncRead + Unpin>(input: &mut R) -> Result<()> {
    #[cfg(windows)]
    {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(2);
        let shower = std::thread::spawn(move || win::show(rx));
        loop {
            let len = input.read_u32_le().await?;
            if len > MAX_FRAME {
                bail!("bad camera frame");
            }
            let mut jpeg = vec![0u8; len as usize];
            input.read_exact(&mut jpeg).await?;
            if shower.is_finished() {
                break;
            }
            let _ = tx.try_send(jpeg);
        }
        drop(tx);
        return shower.join().map_err(|_| anyhow::anyhow!("the camera stopped"))?;
    }
    #[cfg(not(windows))]
    {
        let _ = input;
        bail!("using another computer's camera works on Windows so far")
    }
}

#[cfg(windows)]
mod win {
    use anyhow::{bail, Context, Result};
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    type Create = unsafe extern "C" fn(i32, i32, f32) -> *mut core::ffi::c_void;
    type SendFrame = unsafe extern "C" fn(*mut core::ffi::c_void, *const core::ffi::c_void);
    type Delete = unsafe extern "C" fn(*mut core::ffi::c_void);

    /// Softcam as registered by the admin setup (a protected folder, so the registered filter can't be swapped).
    pub fn dll() -> std::path::PathBuf {
        crate::service::bin_dir().join("softcam.dll")
    }

    pub fn show(rx: std::sync::mpsc::Receiver<Vec<u8>>) -> Result<()> {
        let path = dll();
        if !path.exists() {
            bail!("the bekdesk camera isn't set up on this PC yet: Settings → Lock screen, camera and admin prompts → Turn on");
        }
        unsafe {
            let w: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
            let lib = LoadLibraryW(w.as_ptr());
            if lib.is_null() {
                bail!("couldn't load the bekdesk camera");
            }
            let f = |n: &[u8]| GetProcAddress(lib, n.as_ptr()).context("the bekdesk camera is incomplete");
            let create: Create = std::mem::transmute(f(b"scCreateCamera\0")?);
            let send: SendFrame = std::mem::transmute(f(b"scSendFrame\0")?);
            let delete: Delete = std::mem::transmute(f(b"scDeleteCamera\0")?);
            let mut cam: (*mut core::ffi::c_void, u32, u32) = (std::ptr::null_mut(), 0, 0);
            let mut bgr = Vec::new();
            while let Ok(jpeg) = rx.recv() {
                let mut d = jpeg_decoder::Decoder::new(&jpeg[..]);
                let Ok(rgb) = d.decode() else { continue };
                let Some(info) = d.info() else { continue };
                // Softcam wants sizes in multiples of 4; trim the last few pixels if needed.
                let (fw, fh) = (info.width as u32, info.height as u32);
                let (w, h) = (fw & !3, fh & !3);
                if rgb.len() < (fw * fh * 3) as usize || w == 0 || h == 0 {
                    continue;
                }
                if (cam.1, cam.2) != (w, h) {
                    if !cam.0.is_null() {
                        delete(cam.0);
                    }
                    cam = (create(w as i32, h as i32, 0.0), w, h);
                    if cam.0.is_null() {
                        bail!("another app is already using the bekdesk camera");
                    }
                }
                // RGB top-down → BGR bottom-up (the DIB layout DirectShow's RGB24 uses).
                bgr.resize((w * h * 3) as usize, 0);
                for y in 0..h {
                    let src = &rgb[(y * fw * 3) as usize..][..(w * 3) as usize];
                    let dst = &mut bgr[((h - 1 - y) * w * 3) as usize..][..(w * 3) as usize];
                    for (d, s) in dst.chunks_exact_mut(3).zip(src.chunks_exact(3)) {
                        d.copy_from_slice(&[s[2], s[1], s[0]]);
                    }
                }
                send(cam.0, bgr.as_ptr() as *const _);
            }
            if !cam.0.is_null() {
                delete(cam.0);
            }
        }
        Ok(())
    }
}

/// Per computer: the running camera stream and what its card shows.
static STREAMS: Mutex<BTreeMap<String, tokio::task::JoinHandle<()>>> = Mutex::new(BTreeMap::new());
static STATUS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

pub fn status() -> BTreeMap<String, String> {
    STATUS.lock().unwrap().clone()
}

/// Keeps "Use <name>'s camera here" running for computers connected with Share Input (either way).
pub fn sync(app: &Arc<App>) {
    let want: Vec<String> = {
        let cfg = app.cfg.lock().unwrap();
        app.sessions.lock().unwrap().iter().filter(|(fp, x)| x.mode == Mode::Input && cfg.use_cam.contains(*fp)).map(|(fp, _)| fp.clone()).collect()
    };
    let mut streams = STREAMS.lock().unwrap();
    streams.retain(|fp, t| {
        let keep = want.contains(fp);
        if !keep {
            t.abort();
            STATUS.lock().unwrap().remove(fp);
        }
        keep
    });
    for fp in want {
        if streams.get(&fp).is_some_and(|t| !t.is_finished()) {
            continue;
        }
        let (app, key) = (app.clone(), fp.clone());
        streams.insert(
            fp,
            tokio::spawn(async move {
                let r = async {
                    let (mut s, _) = crate::net::request(&app, &key, &Req::Camera).await?;
                    STATUS.lock().unwrap().insert(key.clone(), "playing".into());
                    play(&mut s).await
                }
                .await;
                let msg = r.err().map_or("stopped".to_string(), |e| format!("{e:#}"));
                app.log(format!("camera from {}: {msg}", app.peer_name(&key)));
                STATUS.lock().unwrap().insert(key, msg);
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn splits_back_to_back_jpegs() {
        let buf = [0x00, 0xFF, 0xD8, 1, 2, 0xFF, 0xD9, 0xFF, 0xD8, 3];
        assert_eq!(super::jpeg_bounds(&buf), Some((1, 7)));
        assert_eq!(super::jpeg_bounds(&buf[7..]), None); // second one isn't finished yet
    }
}
