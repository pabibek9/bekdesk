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

    pub fn cursor_pos() -> Option<(i32, i32)> {
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut p: windows_sys::Win32::Foundation::POINT = unsafe { zeroed() };
        (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
    }

    /// Whether Scroll Lock is on (Deskflow keeps the pointer on this computer while it is).
    pub fn scroll_lock_on() -> bool {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_SCROLL};
        unsafe { GetKeyState(VK_SCROLL as i32) & 1 != 0 }
    }

    /// Press and release Scroll Lock, as if on the keyboard.
    pub fn toggle_scroll_lock() {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, VK_SCROLL};
        unsafe {
            keybd_event(VK_SCROLL as u8, 0x46, KEYEVENTF_EXTENDEDKEY, 0);
            keybd_event(VK_SCROLL as u8, 0x46, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, 0);
        }
    }

    /// Windows pointer speed, 1..=20 (10 = no scaling).
    pub fn mouse_speed() -> Option<u32> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETMOUSESPEED};
        let mut v: u32 = 0;
        (unsafe { SystemParametersInfoW(SPI_GETMOUSESPEED, 0, &mut v as *mut u32 as *mut _, 0) } != 0).then_some(v)
    }

    /// Sets the pointer speed for this sign-in only (not saved), so a crash can't leave it changed for good.
    pub fn set_mouse_speed(v: u32) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_SETMOUSESPEED};
        unsafe { SystemParametersInfoW(SPI_SETMOUSESPEED, 0, v as usize as *mut _, 0) };
    }

    /// `\\.\DISPLAYn` names and current (x, y) of every display attached to the desktop.
    fn attached() -> Vec<(String, i32, i32, bool)> {
        let mut out = vec![];
        unsafe {
            let mut i = 0;
            loop {
                let mut dd: DISPLAY_DEVICEW = zeroed();
                dd.cb = size_of::<DISPLAY_DEVICEW>() as u32;
                if EnumDisplayDevicesW(std::ptr::null(), i, &mut dd, 0) == 0 {
                    break;
                }
                i += 1;
                if dd.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 {
                    continue;
                }
                let mut dm: DEVMODEW = zeroed();
                dm.dmSize = size_of::<DEVMODEW>() as u16;
                if EnumDisplaySettingsW(dd.DeviceName.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) != 0 {
                    let p = dm.Anonymous1.Anonymous2.dmPosition;
                    out.push((s(&dd.DeviceName), p.x, p.y, dd.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0));
                }
            }
        }
        out
    }

    /// Refresh rate (Hz) of the main display.
    pub fn primary_refresh() -> Option<u32> {
        let n = w(&primary_name()?);
        unsafe {
            let mut dm: DEVMODEW = zeroed();
            dm.dmSize = size_of::<DEVMODEW>() as u16;
            (EnumDisplaySettingsW(n.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) != 0 && dm.dmDisplayFrequency > 1).then_some(dm.dmDisplayFrequency)
        }
    }

    /// Name of the main (primary) display.
    pub fn primary_name() -> Option<String> {
        attached().into_iter().find(|d| d.3).map(|d| d.0)
    }

    /// A real (not virtual) display attached to the desktop, preferring the one at the top-left.
    pub fn real_display() -> Option<String> {
        let vdd = vdd_display();
        let mut real: Vec<_> = attached().into_iter().filter(|d| Some(&d.0) != vdd.as_ref()).collect();
        real.sort_by_key(|d| (d.2, d.1));
        real.into_iter().next().map(|d| d.0)
    }

    /// Whether the virtual monitor is its own screen on the desktop (false after Win+P "PC screen only" or "Duplicate").
    pub fn vdd_attached() -> bool {
        let Some(v) = vdd_display() else { return false };
        attached().iter().any(|d| d.0 == v)
    }

    /// Move a (non-main) display to desktop position (x, y); Windows closes any gap to its neighbours.
    pub fn move_display(name: &str, x: i32, y: i32) -> Result<()> {
        if primary_name().as_deref() == Some(name) {
            bail!("the main display stays where it is; drag the other screens around it");
        }
        unsafe {
            let mut dm: DEVMODEW = zeroed();
            dm.dmSize = size_of::<DEVMODEW>() as u16;
            dm.dmFields = DM_POSITION;
            dm.Anonymous1.Anonymous2.dmPosition.x = x;
            dm.Anonymous1.Anonymous2.dmPosition.y = y;
            let n = w(name);
            let r = ChangeDisplaySettingsExW(n.as_ptr(), &dm, std::ptr::null_mut(), CDS_UPDATEREGISTRY | CDS_NORESET, std::ptr::null());
            if r != DISP_CHANGE_SUCCESSFUL {
                bail!("Windows didn't accept that arrangement ({r})");
            }
            ChangeDisplaySettingsExW(std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), 0, std::ptr::null());
        }
        Ok(())
    }

    /// How each active screen is connected and what the monitor calls itself: (`\\.\DISPLAYn`, connection, name).
    fn connections() -> Vec<(String, &'static str, String)> {
        use windows_sys::Win32::Devices::Display::*;
        unsafe {
            let (mut np, mut nm) = (0u32, 0u32);
            if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != 0 {
                return vec![];
            }
            let mut paths = vec![zeroed::<DISPLAYCONFIG_PATH_INFO>(); np as usize];
            let mut modes = vec![zeroed::<DISPLAYCONFIG_MODE_INFO>(); nm as usize];
            if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), std::ptr::null_mut()) != 0 {
                return vec![];
            }
            paths.truncate(np as usize);
            paths
                .iter()
                .map(|p| {
                    let mut src: DISPLAYCONFIG_SOURCE_DEVICE_NAME = zeroed();
                    src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
                    src.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
                    src.header.adapterId = p.sourceInfo.adapterId;
                    src.header.id = p.sourceInfo.id;
                    DisplayConfigGetDeviceInfo(&mut src.header);
                    let mut tgt: DISPLAYCONFIG_TARGET_DEVICE_NAME = zeroed();
                    tgt.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
                    tgt.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
                    tgt.header.adapterId = p.targetInfo.adapterId;
                    tgt.header.id = p.targetInfo.id;
                    DisplayConfigGetDeviceInfo(&mut tgt.header);
                    let conn = match p.targetInfo.outputTechnology {
                        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HDMI => "HDMI",
                        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EXTERNAL => "DisplayPort",
                        18 => "USB-C", // DisplayPort tunnelled over USB-C (newer constant than windows-sys 0.59)
                        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DVI => "DVI",
                        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HD15 => "VGA",
                        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_MIRACAST => "Wireless",
                        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_WIRED | DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_VIRTUAL => "Virtual",
                        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL
                        | DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED
                        | DISPLAYCONFIG_OUTPUT_TECHNOLOGY_LVDS
                        | DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED => "Built-in",
                        _ => "Other",
                    };
                    (s(&src.viewGdiDeviceName), conn, s(&tgt.monitorFriendlyDeviceName))
                })
                .collect()
        }
    }

    fn current(name: &str) -> Option<DEVMODEW> {
        unsafe {
            let mut dm: DEVMODEW = zeroed();
            dm.dmSize = size_of::<DEVMODEW>() as u16;
            let n = w(name);
            (EnumDisplaySettingsW(n.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) != 0).then_some(dm)
        }
    }

    /// This computer's active screens for Manage Screens: position, size, refresh, connection and monitor name.
    pub fn screens() -> serde_json::Value {
        let conns = connections();
        let vdd = vdd_display();
        attached()
            .into_iter()
            .filter_map(|(n, x, y, main)| {
                let dm = current(&n)?;
                let c: Vec<&(String, &str, String)> = conns.iter().filter(|c| c.0 == n).collect();
                let conn = c.first().map(|c| c.1).unwrap_or("Other");
                let label = c.iter().map(|c| c.2.as_str()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" + ");
                Some(serde_json::json!({"name": n, "x": x, "y": y, "w": dm.dmPelsWidth, "h": dm.dmPelsHeight, "hz": dm.dmDisplayFrequency,
                    "main": main, "conn": if vdd.as_ref() == Some(&n) { "bekdesk" } else { conn }, "label": label,
                    "orientation": unsafe { dm.Anonymous1.Anonymous2.dmDisplayOrientation }}))
            })
            .collect()
    }

    /// Resolutions and refresh rates a screen supports (width, height, Hz), largest first.
    pub fn modes(name: &str) -> Vec<(u32, u32, u32)> {
        let n = w(name);
        let mut out = vec![];
        unsafe {
            let mut i = 0;
            loop {
                let mut dm: DEVMODEW = zeroed();
                dm.dmSize = size_of::<DEVMODEW>() as u16;
                if EnumDisplaySettingsW(n.as_ptr(), i, &mut dm) == 0 {
                    break;
                }
                i += 1;
                if dm.dmBitsPerPel >= 24 && dm.dmDisplayFrequency > 1 {
                    out.push((dm.dmPelsWidth, dm.dmPelsHeight, dm.dmDisplayFrequency));
                }
            }
        }
        out.sort_by(|a, b| (b.0 * b.1, b.2).cmp(&(a.0 * a.1, a.2)));
        out.dedup();
        out
    }

    fn apply(name: &str, dm: &DEVMODEW) -> Result<()> {
        let n = w(name);
        let r = unsafe { ChangeDisplaySettingsExW(n.as_ptr(), dm, std::ptr::null_mut(), CDS_UPDATEREGISTRY, std::ptr::null()) };
        if r != DISP_CHANGE_SUCCESSFUL {
            bail!("Windows didn't accept that setting ({r})");
        }
        Ok(())
    }

    /// Change a screen's resolution and refresh rate.
    pub fn set_resolution(name: &str, width: u32, height: u32, hz: u32) -> Result<()> {
        let mut dm = current(name).context("screen not found")?;
        dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
        // Rotated sideways, Windows expects the size in the rotated (tall) form.
        let sideways = unsafe { dm.Anonymous1.Anonymous2.dmDisplayOrientation } % 2 == 1;
        (dm.dmPelsWidth, dm.dmPelsHeight) = if sideways { (height, width) } else { (width, height) };
        dm.dmDisplayFrequency = hz;
        apply(name, &dm)
    }

    /// Rotate a screen: 0 landscape, 1 portrait, 2 landscape (flipped), 3 portrait (flipped).
    pub fn rotate(name: &str, orientation: u32) -> Result<()> {
        let mut dm = current(name).context("screen not found")?;
        let cur = unsafe { dm.Anonymous1.Anonymous2.dmDisplayOrientation };
        if cur % 2 != orientation % 2 {
            std::mem::swap(&mut dm.dmPelsWidth, &mut dm.dmPelsHeight);
        }
        dm.Anonymous1.Anonymous2.dmDisplayOrientation = orientation;
        dm.dmFields = DM_DISPLAYORIENTATION | DM_PELSWIDTH | DM_PELSHEIGHT;
        apply(name, &dm)
    }

    /// Turn a screen off (Windows stops using it until it is turned on again in display settings or by Win+P).
    pub fn turn_off(name: &str) -> Result<()> {
        if primary_name().as_deref() == Some(name) {
            bail!("the main display can't be turned off; make another screen the main one first");
        }
        set_mode(name, 0, 0, 0, 0)
    }

    /// Every display attached to the desktop, for diagnostics.
    pub fn describe() -> String {
        let vdd = vdd_display();
        attached()
            .iter()
            .map(|(n, x, y, main)| {
                let size = unsafe {
                    let mut dm: DEVMODEW = zeroed();
                    dm.dmSize = size_of::<DEVMODEW>() as u16;
                    let nw = w(n);
                    EnumDisplaySettingsW(nw.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm);
                    format!("{}x{}@{}", dm.dmPelsWidth, dm.dmPelsHeight, dm.dmDisplayFrequency)
                };
                format!("{n} at {x},{y} {size}{}{}", if *main { " main" } else { "" }, if vdd.as_ref() == Some(n) { " (bekdesk virtual screen)" } else { "" })
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Name of the display whose top-left corner is at (x, y).
    pub fn display_at(x: i32, y: i32) -> Option<String> {
        attached().into_iter().find(|d| d.1 == x && d.2 == y).map(|d| d.0)
    }

    /// Make `name` the main display. Windows puts the main display at (0, 0), so every display is shifted
    /// by the same amount; the arrangement itself doesn't change.
    pub fn set_primary(name: &str) -> Result<()> {
        let all = attached();
        let Some(&(_, tx, ty, _)) = all.iter().find(|d| d.0 == name).map(|d| d) else { bail!("display {name} not found") };
        unsafe {
            for (n, x, y, _) in &all {
                let mut dm: DEVMODEW = zeroed();
                dm.dmSize = size_of::<DEVMODEW>() as u16;
                dm.dmFields = DM_POSITION;
                dm.Anonymous1.Anonymous2.dmPosition.x = x - tx;
                dm.Anonymous1.Anonymous2.dmPosition.y = y - ty;
                let flags = CDS_UPDATEREGISTRY | CDS_NORESET | if n == name { CDS_SET_PRIMARY } else { 0 };
                let nw = w(n);
                let r = ChangeDisplaySettingsExW(nw.as_ptr(), &dm, std::ptr::null_mut(), flags, std::ptr::null());
                if r != DISP_CHANGE_SUCCESSFUL {
                    bail!("could not move display {n} ({r})");
                }
            }
            ChangeDisplaySettingsExW(std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), 0, std::ptr::null());
        }
        Ok(())
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
pub use win::{scroll_lock_on, toggle_scroll_lock, cursor_pos, describe, modes, rotate, screens, set_resolution, turn_off, display_at, move_display, vdd_attached, real_display, primary_refresh, ensure_installed, mouse_speed, set_mouse_speed, install_elevated, primary_name, set_mode, set_primary, update_modes, vdd_display, vdd_rect};

#[cfg(not(windows))]
pub fn vdd_display() -> Option<String> {
    None
}
#[cfg(not(windows))]
pub fn move_display(_: &str, _: i32, _: i32) -> Result<()> {
    anyhow::bail!("arranging screens from bekdesk is available on Windows; use System Settings → Displays")
}
#[cfg(not(windows))]
pub fn screens() -> serde_json::Value {
    serde_json::json!([])
}
#[cfg(not(windows))]
pub fn modes(_: &str) -> Vec<(u32, u32, u32)> {
    vec![]
}
#[cfg(not(windows))]
pub fn set_resolution(_: &str, _: u32, _: u32, _: u32) -> Result<()> {
    anyhow::bail!("use System Settings → Displays")
}
#[cfg(not(windows))]
pub fn rotate(_: &str, _: u32) -> Result<()> {
    anyhow::bail!("use System Settings → Displays")
}
#[cfg(not(windows))]
pub fn turn_off(_: &str) -> Result<()> {
    anyhow::bail!("use System Settings → Displays")
}
#[cfg(not(windows))]
pub fn describe() -> String {
    String::new()
}
#[cfg(not(windows))]
pub fn primary_refresh() -> Option<u32> {
    None
}
#[cfg(not(windows))]
pub fn scroll_lock_on() -> bool {
    false
}
#[cfg(not(windows))]
pub fn toggle_scroll_lock() {}
#[cfg(not(windows))]
pub fn cursor_pos() -> Option<(i32, i32)> {
    None
}
#[cfg(not(windows))]
pub fn mouse_speed() -> Option<u32> {
    None
}
#[cfg(not(windows))]
pub fn set_mouse_speed(_: u32) {}
#[cfg(not(windows))]
pub fn primary_name() -> Option<String> {
    None
}
#[cfg(not(windows))]
pub fn vdd_attached() -> bool {
    false
}
#[cfg(not(windows))]
pub fn real_display() -> Option<String> {
    None
}
#[cfg(not(windows))]
pub fn display_at(_: i32, _: i32) -> Option<String> {
    None
}
#[cfg(not(windows))]
pub fn set_primary(_: &str) -> Result<()> {
    anyhow::bail!("changing the main display from bekdesk is available on Windows; use System Settings → Displays")
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

/// Whether this computer's lock screen is showing. Linux asks logind (GNOME, KDE and others report it);
/// macOS isn't detected yet and reports unlocked.
pub fn is_locked() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, SwitchDesktop, DESKTOP_SWITCHDESKTOP};
        // The lock screen runs on the secure desktop, which a normal program can't open or switch to.
        let h = OpenInputDesktop(0, 0, DESKTOP_SWITCHDESKTOP);
        if h.is_null() {
            return true;
        }
        let ok = SwitchDesktop(h) != 0;
        CloseDesktop(h);
        !ok
    }
    #[cfg(target_os = "linux")]
    {
        let id = std::env::var("XDG_SESSION_ID").unwrap_or_else(|_| "auto".into());
        std::process::Command::new("loginctl")
            .args(["show-session", &id, "-p", "LockedHint", "--value"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "yes")
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    false
}

/// While another computer controls this one, keep it from blanking the screen, locking from idle or sleeping
/// (its user is working on it from the other keyboard). Locking by hand still works. Call every few seconds.
pub fn keep_awake(on: bool) {
    #[cfg(windows)]
    if on {
        use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED};
        // Resets the idle timers once; repeating it is what keeps the computer awake, so stopping needs nothing.
        unsafe { SetThreadExecutionState(ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED) };
    }
    #[cfg(target_os = "linux")]
    {
        static INHIBIT: std::sync::Mutex<Option<std::process::Child>> = std::sync::Mutex::new(None);
        let mut held = INHIBIT.lock().unwrap();
        if !on {
            if let Some(mut c) = held.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        } else if held.as_mut().is_none_or(|c| !matches!(c.try_wait(), Ok(None))) {
            let why = "Another computer is controlling this one with bekdesk";
            // shortcut: if bekdesk crashes the inhibitor outlives it until logout; tie it to bekdesk's life if that bites.
            // GNOME's own idle inhibitor (stops the screen blanking and locking); logind's for other desktops.
            *held = std::process::Command::new("gnome-session-inhibit")
                .args(["--app-id", "bekdesk", "--reason", why, "--inhibit", "idle:suspend", "--inhibit-only"])
                .stdout(std::process::Stdio::null())
                .spawn()
                .or_else(|_| std::process::Command::new("systemd-inhibit").args(["--what=idle:sleep", "--who=bekdesk", &format!("--why={why}"), "sleep", "infinity"]).spawn())
                .ok();
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = on;
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
        {
            let battery = std::fs::read_dir("/sys/class/power_supply").into_iter().flatten().flatten().any(|e| e.file_name().to_string_lossy().starts_with("BAT"));
            if battery { "laptop" } else { "desktop" }
        }
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

/// Stream frame rate for a screen refreshing at `hz`: about 90% of it on fast screens, leaving the encoder,
/// network and decoder headroom so frames don't queue up (lag); 60 Hz screens keep 60, where a lower rate stutters.
pub fn stream_fps(hz: u32) -> u32 {
    if hz <= 60 { 60 } else { (hz * 9 / 10).clamp(60, 144) }
}

/// Windows pointer-speed setting (1..=20) whose multiplier is closest to `factor` times that of `base`.
/// Multipliers per setting as Windows applies them (without "Enhance pointer precision").
pub fn speed_for(base: u32, factor: f64) -> u32 {
    const MULT: [f64; 20] = [0.03125, 0.0625, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0, 3.25, 3.5];
    let want = MULT[base.clamp(1, 20) as usize - 1] * factor;
    (1..=20u32).min_by(|a, b| (MULT[*a as usize - 1] - want).abs().total_cmp(&(MULT[*b as usize - 1] - want).abs())).unwrap()
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
    fn fps_leaves_headroom() {
        assert_eq!(stream_fps(60), 60);
        assert_eq!(stream_fps(100), 90);
        assert_eq!(stream_fps(120), 108);
        assert_eq!(stream_fps(144), 129);
        assert_eq!(stream_fps(240), 144);
        assert_eq!(stream_fps(0), 60);
    }

    #[test]
    fn pointer_speed_scales() {
        assert_eq!(speed_for(10, 1.0), 10);
        assert_eq!(speed_for(10, 2.0), 14);
        assert_eq!(speed_for(10, 0.5), 6);
        assert_eq!(speed_for(14, 1.5), 18);
        assert_eq!(speed_for(20, 3.0), 20);
        assert_eq!(speed_for(1, 0.1), 1);
    }

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
