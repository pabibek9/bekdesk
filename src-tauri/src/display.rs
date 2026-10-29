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
    use windows_sys::Win32::Devices::Display::{SetDisplayConfig, SDC_APPLY, SDC_TOPOLOGY_EXTEND};
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

    /// Current desktop rectangle (x, y, w, h) of the virtual monitor, if it is attached.
    pub fn vdd_rect() -> Option<(i32, i32, u32, u32)> {
        let name = vdd_display()?;
        unsafe {
            let mut dm: DEVMODEW = zeroed();
            dm.dmSize = size_of::<DEVMODEW>() as u16;
            let n = w(&name);
            if EnumDisplaySettingsW(n.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) == 0 || dm.dmPelsWidth == 0 {
                return None;
            }
            let p = dm.Anonymous1.Anonymous2.dmPosition;
            Some((p.x, p.y, dm.dmPelsWidth, dm.dmPelsHeight))
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
            let mut r = ChangeDisplaySettingsExW(n.as_ptr(), &dm, std::ptr::null_mut(), CDS_UPDATEREGISTRY | CDS_NORESET, std::ptr::null());
            if r != DISP_CHANGE_SUCCESSFUL && width > 0 {
                // An inactive IddCx monitor can't be attached with the legacy call (-1); switch the
                // desktop to "extend" so it becomes active, then set its mode and position.
                SetDisplayConfig(0, std::ptr::null(), 0, std::ptr::null(), SDC_APPLY | SDC_TOPOLOGY_EXTEND);
                std::thread::sleep(std::time::Duration::from_millis(800));
                r = ChangeDisplaySettingsExW(n.as_ptr(), &dm, std::ptr::null_mut(), CDS_UPDATEREGISTRY | CDS_NORESET, std::ptr::null());
            }
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

    /// Installs new settings for an already-installed driver and restarts its device so the new modes load
    /// (the driver reads its mode list only at start). One visible UAC prompt; the user can decline.
    pub fn update_modes(settings: &Path, scratch: &Path) -> Result<()> {
        let ps1 = scratch.join("vdd-modes.ps1");
        std::fs::write(
            &ps1,
            format!(
                "Copy-Item -LiteralPath '{}' -Destination 'C:\\VirtualDisplayDriver\\vdd_settings.xml' -Force\r\n\
                 $d = Get-PnpDevice | Where-Object {{ $_.HardwareID -contains 'Root\\MttVDD' }}\r\n\
                 $d | Disable-PnpDevice -Confirm:$false\r\nStart-Sleep -Seconds 2\r\n$d | Enable-PnpDevice -Confirm:$false\r\n",
                settings.display().to_string().replace('\'', "''")
            ),
        )?;
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!(
                "$p = Start-Process -FilePath powershell -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','\"{}\"' -Verb RunAs -Wait -PassThru -WindowStyle Hidden; exit $p.ExitCode",
                ps1.display().to_string().replace('\'', "''")
            ))
            .status()
            .context("starting display settings update")?;
        if !status.success() {
            bail!("the virtual display's screen sizes were not updated (declined or failed)");
        }
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if vdd_display().is_some() {
                return Ok(());
            }
        }
        bail!("virtual display did not come back after updating its screen sizes")
    }

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
pub use win::{ensure_installed, install_elevated, set_mode, update_modes, vdd_display, vdd_rect};

#[cfg(not(windows))]
pub fn vdd_display() -> Option<String> {
    None
}
#[cfg(not(windows))]
pub fn vdd_rect() -> Option<(i32, i32, u32, u32)> {
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
pub fn update_modes(_: &std::path::Path, _: &std::path::Path) -> Result<()> {
    anyhow::bail!("unsupported")
}
#[cfg(not(windows))]
pub fn install_elevated(_: &std::path::Path) -> Result<()> {
    anyhow::bail!("unsupported")
}

/// "laptop" if the machine has a built-in battery, else "desktop"; drives the device frame in Manage Screens.
pub fn device_kind() -> &'static str {
    static KIND: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    KIND.get_or_init(|| {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
            let mut s: SYSTEM_POWER_STATUS = std::mem::zeroed();
            // BatteryFlag 128 = no system battery, 255 = unknown
            if GetSystemPowerStatus(&mut s) != 0 && s.BatteryFlag != 128 && s.BatteryFlag != 255 {
                return "laptop";
            }
            "desktop"
        }
        #[cfg(target_os = "macos")]
        {
            let out = std::process::Command::new("pmset").args(["-g", "batt"]).output();
            if out.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("InternalBattery")) {
                return "laptop";
            }
            "desktop"
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        "desktop"
    })
}

/// Screen sizes the virtual monitor should offer beyond the driver's stock 16:9 list: common 16:10 laptops
/// and tablets, so they get a pixel-exact screen instead of a blurry 1920x1080.
pub const EXTRA_MODES: [(u32, u32); 8] = [(1280, 800), (1440, 900), (1680, 1050), (1920, 1200), (2000, 1200), (2560, 1600), (2880, 1800), (2048, 1536)];

/// `vdd_settings.xml` with every mode in `modes` present in its `<resolutions>` list.
pub fn with_modes(xml: &str, modes: &[(u32, u32)]) -> String {
    let add: String = modes
        .iter()
        .filter(|m| !has_mode(xml, **m))
        .map(|(w, h)| format!("        <resolution>\r\n            <width>{w}</width>\r\n            <height>{h}</height>\r\n            <refresh_rate>60</refresh_rate>\r\n        </resolution>\r\n"))
        .collect();
    match xml.find("</resolutions>") {
        Some(i) if !add.is_empty() => format!("{}{add}    {}", xml[..i].trim_end_matches([' ', '\t']), &xml[i..]),
        _ => xml.to_string(),
    }
}

pub fn has_mode(xml: &str, (w, h): (u32, u32)) -> bool {
    let compact: String = xml.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains(&format!("<width>{w}</width><height>{h}</height>"))
}

/// Sunshine (Windows) logs "Currently available display devices:" followed by a JSON array at startup.
/// Returns the `device_id` of the entry whose `display_name` is `display_name` (e.g. `\\.\DISPLAY3`), which is
/// what its `output_name` option needs. Uses the last listing in the log.
pub fn device_id_from_log(log: &str, display_name: &str) -> Option<String> {
    let rest = &log[log.rfind("Currently available display devices:")?..];
    let start = rest.find('[')?;
    let mut depth = 0;
    let end = rest[start..].char_indices().find_map(|(i, c)| {
        depth += (c == '[') as i32 - (c == ']') as i32;
        (depth == 0).then_some(start + i + 1)
    })?;
    let list: Vec<serde_json::Value> = serde_json::from_str(&rest[start..end]).ok()?;
    list.iter().find(|d| d["display_name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(display_name)))?["device_id"].as_str().map(String::from)
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
    fn adds_missing_modes_once() {
        let xml = "<resolutions>\r\n        <resolution>\r\n            <width>1920</width>\r\n            <height>1080</height>\r\n        </resolution>\r\n    </resolutions>";
        let out = with_modes(xml, &[(1920, 1080), (1920, 1200)]);
        assert!(has_mode(&out, (1920, 1200)) && has_mode(&out, (1920, 1080)));
        assert_eq!(out.matches("<width>1920</width>").count(), 2);
        assert_eq!(with_modes(&out, &[(1920, 1200)]), out);
        assert!(out.ends_with("</resolutions>"));
    }

    #[test]
    fn finds_device_id() {
        let log = r#"[x]: Info: Currently available display devices:
[
  {"device_id": "{aaa}", "display_name": "\\\\.\\DISPLAY1", "info": {"a": [1]}},
  {"device_id": "{bbb}", "display_name": "\\\\.\\DISPLAY3"}
]
[x]: Info: more"#;
        assert_eq!(device_id_from_log(log, r"\\.\DISPLAY3").as_deref(), Some("{bbb}"));
        assert_eq!(device_id_from_log(log, r"\\.\DISPLAY9"), None);
        assert_eq!(device_id_from_log("nothing", "x"), None);
    }

    #[test]
    fn parses() {
        assert_eq!(parse_res("2560x1600"), Some((2560, 1600)));
        assert_eq!(parse_res("10x10"), None);
        assert_eq!(primary_size("0,0,1920,1080;1920,0,1280,1024"), Some((1920, 1080)));
        assert_eq!(primary_size(""), None);
    }
}
