//! Use another computer's camera here: the owner shares it, and on Windows it shows up in Zoom, Teams, browsers and
//! other camera apps as "bekdesk camera" (the bundled Softcam DirectShow filter, registered by the admin setup);
//! on Linux as "bekdesk-camera" (the v4l2loopback kernel module, which the user installs).
//! Frames travel as JPEG, each prefixed with its length (u32, little-endian), over the bekdesk TLS channel.

use crate::app::{App, Mode};
use crate::net::Req;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
#[cfg(not(windows))]
use tokio::io::AsyncRead;
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest frame accepted (a 1080p JPEG is well under this).
const MAX_FRAME: u32 = 8 * 1024 * 1024;

/// Sending side: switch this computer's camera on. Done before telling the other computer "yes", so a camera
/// that can't start is reported to it in words instead of as a dropped connection.
pub fn open() -> Result<tokio::sync::mpsc::Receiver<Vec<u8>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(3);
    capture(tx)?;
    Ok(rx)
}

/// Sending side: stream the opened camera's JPEG frames until the other side stops.
pub async fn send<W: AsyncWrite + Unpin>(mut rx: tokio::sync::mpsc::Receiver<Vec<u8>>, out: &mut W) -> Result<()> {
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
    use nokhwa::utils::{CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution};
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
    // The camera object stays on its own thread; it ends when the receiving side goes away.
    std::thread::spawn(move || {
        let opened = (|| -> Result<nokhwa::Camera> {
            // Cameras differ in what they offer (many laptop cameras have no compressed 1280×720 mode, and asking
            // for exactly that fails), so try the preferred mode first and then whatever the camera does have.
            let wanted = [
                RequestedFormatType::Closest(CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 30)),
                RequestedFormatType::HighestResolution(Resolution::new(1280, 720)),
                RequestedFormatType::HighestResolution(Resolution::new(640, 480)),
                RequestedFormatType::HighestFrameRate(30),
                RequestedFormatType::AbsoluteHighestResolution,
                RequestedFormatType::None,
            ];
            let mut last = anyhow::anyhow!("no camera found on this computer");
            for want in wanted {
                match nokhwa::Camera::new(CameraIndex::Index(0), RequestedFormat::new::<RgbFormat>(want)) {
                    Ok(mut cam) => match cam.open_stream() {
                        Ok(()) => return Ok(cam),
                        Err(e) => last = anyhow::anyhow!("this computer's camera is busy or blocked (close other camera apps; Windows Settings → Privacy & security → Camera): {e}"),
                    },
                    Err(e) => last = anyhow::anyhow!("this computer's camera couldn't start: {e}"),
                }
            }
            Err(last)
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

/// Fits an RGB picture into a `w`×`h` BGR one (nearest pixel; black bars where the shapes differ).
#[cfg_attr(not(windows), allow(dead_code))]
fn fit_bgr(rgb: &[u8], (fw, fh): (usize, usize), out: &mut Vec<u8>, (w, h): (usize, usize)) {
    out.clear();
    out.resize(w * h * 3, 0);
    let (pw, ph) = if fw * h > fh * w { (w, (fh * w / fw).max(1)) } else { ((fw * h / fh).max(1), h) };
    let (ox, oy) = ((w - pw) / 2, (h - ph) / 2);
    for y in 0..ph {
        let row = (y * fh / ph) * fw;
        for x in 0..pw {
            let (s, d) = ((row + x * fw / pw) * 3, ((y + oy) * w + x + ox) * 3);
            out[d..d + 3].copy_from_slice(&[rgb[s + 2], rgb[s + 1], rgb[s]]);
        }
    }
}

/// First complete JPEG in `buf`: start of its FF D8 and just past its FF D9.
#[cfg_attr(windows, allow(dead_code))]
fn jpeg_bounds(buf: &[u8]) -> Option<(usize, usize)> {
    let a = buf.windows(2).position(|w| w == [0xFF, 0xD8])?;
    let e = buf[a + 2..].windows(2).position(|w| w == [0xFF, 0xD9])?;
    Some((a, a + 2 + e + 2))
}

/// Windows receiving side: "bekdesk camera" stays available to camera apps while this runs, but the other
/// computer's real camera is only switched on while an app is actually using it.
#[cfg(windows)]
async fn watch(app: &Arc<App>, key: &str) -> Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(2);
    let used = Arc::new(AtomicBool::new(false));
    let shower = {
        let used = used.clone();
        std::thread::spawn(move || win::show(rx, used))
    };
    let stopped = |shower: std::thread::JoinHandle<Result<()>>| -> Result<()> {
        shower.join().map_err(|_| anyhow::anyhow!("the bekdesk camera stopped"))??;
        bail!("the bekdesk camera stopped")
    };
    loop {
        // Allowed? Asking also puts an Allow button on the owner's screen if not.
        crate::net::request(app, key, &Req::Camera { probe: true }).await?;
        STATUS.lock().unwrap().insert(key.to_string(), "ready".into());
        while !used.load(Ordering::Relaxed) {
            if shower.is_finished() {
                return stopped(shower);
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
        let (mut s, _) = crate::net::request(app, key, &Req::Camera { probe: false }).await?;
        STATUS.lock().unwrap().insert(key.to_string(), "playing".into());
        while used.load(Ordering::Relaxed) {
            let len = s.read_u32_le().await.map_err(|_| anyhow::anyhow!("{}'s camera stopped sending. Check no other app is using the camera there, then it starts again by itself", app.peer_name(key)))?;
            if len > MAX_FRAME {
                bail!("bad camera frame");
            }
            let mut jpeg = vec![0u8; len as usize];
            s.read_exact(&mut jpeg).await?;
            if shower.is_finished() {
                return stopped(shower);
            }
            let _ = tx.try_send(jpeg);
        }
        // Dropping the connection here switches the other computer's camera off until an app wants it again.
    }
}

/// Linux receiving side: show the frames as this computer's "bekdesk-camera" until the connection ends.
#[cfg(not(windows))]
pub async fn play<R: AsyncRead + Unpin>(input: &mut R) -> Result<()> {
    {
        let dev = virtual_camera()?;
        // GStreamer decodes the JPEGs into the virtual camera, which camera apps then open like a real one.
        let mut gst = tokio::process::Command::new("gst-launch-1.0")
            .args(["-q", "fdsrc", "!", "image/jpeg,framerate=30/1", "!", "jpegdec", "!", "videoconvert", "!", "video/x-raw,format=YUY2", "!", "v4l2sink", &format!("device={dev}"), "sync=false"])
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("couldn't start the camera (GStreamer's gst-launch-1.0 is needed)")?;
        let mut pipe = gst.stdin.take().context("no camera input")?;
        loop {
            let len = input.read_u32_le().await?;
            if len > MAX_FRAME {
                bail!("bad camera frame");
            }
            let mut jpeg = vec![0u8; len as usize];
            input.read_exact(&mut jpeg).await?;
            pipe.write_all(&jpeg).await.context("the bekdesk camera stopped")?;
        }
    }
}

/// The "bekdesk camera" device among the names under `/sys/devices/virtual/video4linux/<videoN>/name`.
#[cfg_attr(windows, allow(dead_code))]
fn pick_camera(named: &[(String, String)]) -> Option<String> {
    named.iter().find(|(_, name)| name.trim().starts_with("bekdesk")).map(|(dev, _)| format!("/dev/{dev}"))
}

/// Linux: the virtual camera (the v4l2loopback kernel module), switched on the first time through the system's
/// own password prompt.
#[cfg(not(windows))]
fn virtual_camera() -> Result<String> {
    if !cfg!(target_os = "linux") {
        bail!("using another computer's camera on a Mac isn't available yet");
    }
    let find = || {
        let named: Vec<(String, String)> = std::fs::read_dir("/sys/devices/virtual/video4linux").into_iter().flatten().flatten().filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), std::fs::read_to_string(e.path().join("name")).ok()?))).collect();
        pick_camera(&named)
    };
    if let Some(dev) = find() {
        return Ok(dev);
    }
    // Ask once per run: a declined password prompt shouldn't come back every time the stream retries.
    static ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !ASKED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        let _ = std::process::Command::new("pkexec").args(["modprobe", "v4l2loopback", "video_nr=42", "card_label=bekdesk-camera", "exclusive_caps=1"]).status();
    }
    find().context("the virtual camera isn't installed. Install v4l2loopback (Kali/Ubuntu/Debian: sudo apt install v4l2loopback-dkms; Fedora: akmod-v4l2loopback from RPM Fusion), restart bekdesk, and allow the password prompt")
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

    type Connected = unsafe extern "C" fn(*mut core::ffi::c_void) -> bool;

    /// Shows frames from `rx` as the bekdesk camera until `rx` closes; `used` says whether a camera app has it open.
    pub fn show(rx: std::sync::mpsc::Receiver<Vec<u8>>, used: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Result<()> {
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
            let connected: Option<Connected> = GetProcAddress(lib, b"scIsConnected\0".as_ptr()).map(|p| std::mem::transmute(p));
            // Created up front so camera apps list it and can open it before any picture arrives.
            const W: usize = 1280;
            const H: usize = 720;
            // A camera this app just let go of (switching it off and on) can take a moment to disappear.
            let mut cam = (create(W as i32, H as i32, 0.0),);
            for _ in 0..15 {
                if !cam.0.is_null() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
                cam.0 = create(W as i32, H as i32, 0.0);
            }
            if cam.0.is_null() {
                bail!("another app is already using the bekdesk camera");
            }
            // The camera part answers "has an app asked for a picture since I last asked?"; an app that is showing
            // the camera asks many times a second, so two quiet seconds mean it stopped.
            let mut asked = std::time::Instant::now() - std::time::Duration::from_secs(60);
            let mut bgr = Vec::new();
            loop {
                if connected.is_none_or(|f| f(cam.0)) {
                    asked = std::time::Instant::now();
                }
                used.store(asked.elapsed() < std::time::Duration::from_secs(2), std::sync::atomic::Ordering::Relaxed);
                let jpeg = match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                    Ok(j) => j,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                let mut d = jpeg_decoder::Decoder::new(&jpeg[..]);
                let Ok(rgb) = d.decode() else { continue };
                let Some(info) = d.info() else { continue };
                let (fw, fh) = (info.width as usize, info.height as usize);
                if fw == 0 || fh == 0 || rgb.len() < fw * fh * 3 {
                    continue;
                }
                // The camera keeps one size for its whole life: recreating it for a different picture size would
                // cut off the app that has it open (a black picture), so every frame is fitted into it instead.
                super::fit_bgr(&rgb, (fw, fh), &mut bgr, (W, H));
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
                #[cfg(windows)]
                let r = watch(&app, &key).await;
                // shortcut: on Linux the other computer's camera stays on while this is switched on (the virtual
                // camera can't say whether an app is watching); stream on demand if that bothers people.
                #[cfg(not(windows))]
                let r = async {
                    let (mut s, _) = crate::net::request(&app, &key, &Req::Camera { probe: false }).await?;
                    STATUS.lock().unwrap().insert(key.clone(), "playing".into());
                    play(&mut s).await
                }
                .await;
                let msg = r.err().map_or("stopped".to_string(), |e| format!("{e:#}"));
                app.log(format!("camera from {}: {msg}", app.peer_name(&key)));
                STATUS.lock().unwrap().insert(key, msg);
                // Short, so the picture starts soon after the owner presses Allow.
                tokio::time::sleep(std::time::Duration::from_secs(6)).await;
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_the_bekdesk_camera() {
        let named = [("video0".to_string(), "Integrated Camera\n".to_string()), ("video42".to_string(), "bekdesk-camera\n".to_string())];
        assert_eq!(super::pick_camera(&named).as_deref(), Some("/dev/video42"));
        assert_eq!(super::pick_camera(&named[..1]), None);
    }

    #[test]
    fn fits_a_picture_into_the_camera_size() {
        // A 2×1 red|blue picture into 4×4: doubled across, centred, black bars above and below, colours as BGR.
        let rgb = [255, 0, 0, 0, 0, 255];
        let mut out = vec![];
        super::fit_bgr(&rgb, (2, 1), &mut out, (4, 4));
        let px = |x: usize, y: usize| out[(y * 4 + x) * 3..][..3].to_vec();
        assert_eq!(px(0, 0), [0, 0, 0]);
        assert_eq!((px(0, 1), px(1, 1), px(2, 1), px(3, 2)), (vec![0, 0, 255], vec![0, 0, 255], vec![255, 0, 0], vec![255, 0, 0]));
        assert_eq!(px(3, 3), [0, 0, 0]);
        // Same size: copied through.
        super::fit_bgr(&rgb, (2, 1), &mut out, (2, 1));
        assert_eq!(out, [0, 0, 255, 255, 0, 0]);
    }

    #[test]
    fn splits_back_to_back_jpegs() {
        let buf = [0x00, 0xFF, 0xD8, 1, 2, 0xFF, 0xD9, 0xFF, 0xD8, 3];
        assert_eq!(super::jpeg_bounds(&buf), Some((1, 7)));
        assert_eq!(super::jpeg_bounds(&buf[7..]), None); // second one isn't finished yet
    }
}
