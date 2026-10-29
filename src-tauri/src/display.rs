//! Virtual monitor for Extend Display on Windows hosts, backed by the signed Virtual Display Driver
//! (IddCx). The driver is installed once with an explicit UAC prompt; afterwards the virtual monitor
//! is attached/detached from the desktop per session without elevation.

#[cfg(not(windows))]
use anyhow::Result;

pub const VDD_ADAPTER: &str = "Virtual Display Driver";

#[cfg(windows)]
mod win {
    use super::VDD_ADAPTER;
    use anyhow::{bail, Context, Result};
    use std::mem::{size_of, zeroed};
    use std::path::Path;
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows_sys::Win32::Graphics::Gdi::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn s(w: &[u16]) -> String {
        String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())])
    }

    /// `\\.\DISPLAYn` name of the virtual monitor's display source, if the driver is installed.
    pub fn vdd_display() -> Option<String> {
        unsafe {
            let mut i = 0;
            loop {
                let mut dd: DISPLAY_DEVICEW = zeroed();
                dd.cb = size_of::<DISPLAY_DEVICEW>() as u32;
                if EnumDisplayDevicesW(std::ptr::null(), i, &mut dd, 0) == 0 {
                    return None;
                }
                if s(&dd.DeviceString).contains(VDD_ADAPTER) {
                    return Some(s(&dd.DeviceName));
                }
                i += 1;
            }
        }
    }

    /// Attach (w,h > 0) or detach (w = h = 0) the virtual monitor at desktop position (x, y).
    pub fn set_mode(name: &str, width: u32, height: u32, x: i32, y: i32) -> Result<()> {
        unsafe {
            let mut dm: DEVMODEW = zeroed();
            dm.dmSize = size_of::<DEVMODEW>() as u16;
            dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_POSITION;
            dm.dmPelsWidth = width;
            dm.dmPelsHeight = height;
            dm.Anonymous1.Anonymous2.dmPosition.x = x;
            dm.Anonymous1.Anonymous2.dmPosition.y = y;
            let n = w(name);
            let r = ChangeDisplaySettingsExW(n.as_ptr(), &dm, std::ptr::null_mut(), CDS_UPDATEREGISTRY | CDS_NORESET, std::ptr::null());
            if r != DISP_CHANGE_SUCCESSFUL {
                bail!("display mode {width}x{height} rejected ({r})");
            }
            ChangeDisplaySettingsExW(std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), 0, std::ptr::null());
        }
        Ok(())
    }

    /// Runs elevated (`bekdesk --install-vdd <inf>`): creates the Root\MttVDD device and installs the driver.
    pub fn install_elevated(inf: &Path) -> Result<()> {
        let settings = inf.with_file_name("vdd_settings.xml");
        std::fs::create_dir_all(r"C:\VirtualDisplayDriver")?;
        if settings.exists() {
            std::fs::copy(&settings, r"C:\VirtualDisplayDriver\vdd_settings.xml")?;
        }
        unsafe {
            // {4d36e968-e325-11ce-bfc1-08002be10318}: Display adapters
            let guid = windows_sys::core::GUID::from_u128(0x4d36e968_e325_11ce_bfc1_08002be10318);
            let set = SetupDiCreateDeviceInfoList(&guid, std::ptr::null_mut());
            if set == INVALID_HANDLE_VALUE as HDEVINFO {
                bail!("SetupDiCreateDeviceInfoList failed");
            }
            let mut info: SP_DEVINFO_DATA = zeroed();
            info.cbSize = size_of::<SP_DEVINFO_DATA>() as u32;
            let class = w("Display");
            let hwid: Vec<u16> = "Root\\MttVDD".encode_utf16().chain([0, 0]).collect();
            let ok = SetupDiCreateDeviceInfoW(set, class.as_ptr(), &guid, std::ptr::null(), std::ptr::null_mut(), DICD_GENERATE_ID, &mut info) != 0
                && SetupDiSetDeviceRegistryPropertyW(set, &mut info, SPDRP_HARDWAREID, hwid.as_ptr() as *const u8, (hwid.len() * 2) as u32) != 0
                && SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set, &mut info) != 0;
            SetupDiDestroyDeviceInfoList(set);
            if !ok {
                bail!("could not create the virtual display device");
            }
            let infw = w(&inf.to_string_lossy());
            let mut reboot = 0;
            if UpdateDriverForPlugAndPlayDevicesW(std::ptr::null_mut(), hwid.as_ptr(), infw.as_ptr(), INSTALLFLAG_FORCE, &mut reboot) == 0 {
                bail!("driver installation failed");
            }
        }
        Ok(())
    }

    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;

    pub fn ensure_installed(inf: &Path) -> Result<()> {
        if vdd_display().is_some() {
            return Ok(());
        }
        let exe = std::env::current_exe()?;
        // Visible UAC prompt; the user can decline.
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!(
                "$p = Start-Process -FilePath '{}' -ArgumentList '--install-vdd','\"{}\"' -Verb RunAs -Wait -PassThru; exit $p.ExitCode",
                exe.display().to_string().replace('\'', "''"),
                inf.display().to_string().replace('\'', "''")
            ))
            .status()
            .context("starting driver installer")?;
        if !status.success() {
            bail!("virtual display driver was not installed (declined or failed)");
        }
        for _ in 0..20 {
            if vdd_display().is_some() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        bail!("virtual display did not appear after installing the driver")
    }
}

#[cfg(windows)]
pub use win::{ensure_installed, install_elevated, set_mode, vdd_display};

#[cfg(not(windows))]
pub fn vdd_display() -> Option<String> {
    None
}
#[cfg(not(windows))]
pub fn set_mode(_: &str, _: u32, _: u32, _: i32, _: i32) -> Result<()> {
    anyhow::bail!("Extend Display from this OS is not available yet — use Mirror")
}
#[cfg(not(windows))]
pub fn ensure_installed(_: &std::path::Path) -> Result<()> {
    anyhow::bail!("Extend Display from this OS is not available yet — use Mirror")
}
#[cfg(not(windows))]
pub fn install_elevated(_: &std::path::Path) -> Result<()> {
    anyhow::bail!("unsupported")
}

/// Parse "WxH" with sane bounds.
pub fn parse_res(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.trim().split_once(['x', 'X'])?;
    let (w, h) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    ((640..=7680).contains(&w) && (480..=4320).contains(&h)).then_some((w, h))
}

/// Primary monitor size from a "x,y,w,h;..." monitor list.
pub fn primary_size(monitors: &str) -> Option<(u32, u32)> {
    let m: Vec<i64> = monitors.split(';').next()?.split(',').filter_map(|v| v.parse().ok()).collect();
    (m.len() == 4 && m[2] > 0 && m[3] > 0).then(|| (m[2] as u32, m[3] as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses() {
        assert_eq!(parse_res("2560x1600"), Some((2560, 1600)));
        assert_eq!(parse_res("10x10"), None);
        assert_eq!(primary_size("0,0,1920,1080;1920,0,1280,1024"), Some((1920, 1080)));
        assert_eq!(primary_size(""), None);
    }
}
