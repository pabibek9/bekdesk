//! Windows "lock screen helper": a small service that runs Deskflow's client with system rights while another
//! computer controls this one, so the keyboard and mouse keep working on the lock screen and on admin prompts.
//!
//! Security: the service and its Deskflow copy live in `C:\ProgramData\bekdesk\bin`, which only administrators can
//! change. It takes no commands; it only reads `run\client.ini`, a folder only the user who set it up can write to,
//! and runs the client while that file keeps being refreshed by bekdesk (every few seconds during Share Input).

use anyhow::Result;
use std::path::PathBuf;

pub const NAME: &str = "bekdesk-input";
pub const ROOT: &str = r"C:\ProgramData\bekdesk";

pub fn bin_dir() -> PathBuf {
    PathBuf::from(ROOT).join("bin")
}
pub fn run_dir() -> PathBuf {
    PathBuf::from(ROOT).join("run")
}

/// Whether the helper is set up (its files exist and the service is registered). Also false after an update that
/// brings a part the last setup didn't install (the bekdesk camera), so Settings offers to run setup again.
pub fn installed() -> bool {
    let cam_shipped = std::env::current_exe().is_ok_and(|e| e.with_file_name("resources").join("softcam.dll").exists());
    cfg!(windows)
        && bin_dir().join("deskflow").join("deskflow-core.exe").exists()
        && (!cam_shipped || bin_dir().join("softcam.dll").exists())
        && std::process::Command::new("sc").args(["query", NAME]).stdout(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
}

/// A settings file the service runs the client from; refreshed by `touch` while Share Input is on.
pub fn client_ini() -> PathBuf {
    run_dir().join("client.ini")
}

/// Keeps `client_ini` fresh; the service stops the client about 15 s after this stops being called.
pub fn touch() {
    if let Ok(f) = std::fs::File::options().write(true).open(client_ini()) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

#[cfg(windows)]
pub use win::{elevated, install, run_service, uninstall, user_sid};

#[cfg(not(windows))]
pub fn install(_: &str, _: &std::path::Path) -> Result<()> {
    anyhow::bail!("only on Windows")
}
#[cfg(not(windows))]
pub fn uninstall() -> Result<()> {
    anyhow::bail!("only on Windows")
}
#[cfg(not(windows))]
pub fn run_service() {}
#[cfg(not(windows))]
pub fn elevated(_: &[String]) -> Result<()> {
    anyhow::bail!("only on Windows")
}
#[cfg(not(windows))]
pub fn user_sid() -> Result<String> {
    anyhow::bail!("only on Windows")
}

#[cfg(windows)]
mod win {
    use super::*;
    use anyhow::{bail, Context};
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Security::*;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    use windows_sys::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId};
    use windows_sys::Win32::System::Services::*;
    use windows_sys::Win32::System::Threading::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    fn run(cmd: &str, args: &[&str]) -> Result<()> {
        let st = std::process::Command::new(cmd).args(args).creation_flags(0x0800_0000).status().with_context(|| format!("running {cmd}"))?;
        if !st.success() {
            bail!("{cmd} {} failed", args.join(" "));
        }
        Ok(())
    }

    fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            let dst = to.join(e.file_name());
            if e.file_type()?.is_dir() {
                copy_dir(&e.path(), &dst)?;
            } else {
                std::fs::copy(e.path(), &dst)?;
            }
        }
        Ok(())
    }

    /// Runs elevated (`bekdesk --install-service <user SID> <deskflow folder>`), after the user accepted the UAC prompt.
    pub fn install(sid: &str, deskflow: &std::path::Path) -> Result<()> {
        if !sid.starts_with("S-1-5-") || !sid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            bail!("unexpected user id");
        }
        let _ = uninstall();
        let (root, bin, run_d) = (PathBuf::from(ROOT), bin_dir(), run_dir());
        std::fs::create_dir_all(&bin)?;
        std::fs::create_dir_all(&run_d)?;
        let r = root.to_string_lossy().to_string();
        // Only SYSTEM and administrators may change anything here; everyone may read and run.
        run("icacls", &[&r, "/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", "*S-1-5-32-545:(OI)(CI)RX"])?;
        // The run folder: the user who set this up may write the client settings; nobody else.
        let grant = format!("*{sid}:(OI)(CI)M");
        run("icacls", &[&run_d.to_string_lossy(), "/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", &grant])?;
        std::fs::copy(std::env::current_exe()?, bin.join("bekdesk-service.exe"))?;
        copy_dir(deskflow, &bin.join("deskflow"))?;
        // The "bekdesk camera" (Softcam), shipped next to bekdesk.exe; registered from the protected folder.
        let cam = std::env::current_exe()?.with_file_name("resources").join("softcam.dll");
        if cam.exists() {
            std::fs::copy(&cam, bin.join("softcam.dll"))?;
            run("regsvr32", &["/s", &bin.join("softcam.dll").to_string_lossy()])?;
        }
        let path = format!("\"{}\" --service", bin.join("bekdesk-service.exe").display());
        run("sc", &["create", NAME, "binPath=", &path, "start=", "auto", "DisplayName=", "bekdesk lock screen helper"])?;
        let _ = run("sc", &["description", NAME, "Lets a computer that controls this one with bekdesk use the keyboard and mouse on the lock screen."]);
        run("sc", &["start", NAME])
    }

    /// Runs elevated (`bekdesk --uninstall-service`).
    pub fn uninstall() -> Result<()> {
        let cam = bin_dir().join("softcam.dll");
        if cam.exists() {
            let _ = run("regsvr32", &["/s", "/u", &cam.to_string_lossy()]);
        }
        let _ = run("sc", &["stop", NAME]);
        std::thread::sleep(std::time::Duration::from_millis(800));
        let _ = run("sc", &["delete", NAME]);
        if std::path::Path::new(ROOT).exists() {
            std::fs::remove_dir_all(ROOT).context("removing the helper's files")?;
        }
        Ok(())
    }

    /// This user's security id, so the elevated setup can give only them write access to the run folder.
    pub fn user_sid() -> Result<String> {
        let o = std::process::Command::new("whoami").args(["/user", "/fo", "csv", "/nh"]).creation_flags(0x0800_0000).output()?;
        let s = String::from_utf8_lossy(&o.stdout);
        s.trim().rsplit(',').next().map(|x| x.trim_matches('"').to_string()).filter(|x| x.starts_with("S-1-5-")).context("couldn't read this user's id")
    }

    /// Runs this exe again with `args` after a visible Windows admin prompt; waits for it to finish.
    pub fn elevated(args: &[String]) -> Result<()> {
        // Each argument as a PowerShell '...' string wrapping "..." so paths with spaces stay one argument.
        let q = |s: &str| format!("'\"{}\"'", s.replace('\'', "''"));
        let list = args.iter().map(|a| q(a)).collect::<Vec<_>>().join(",");
        let exe = std::env::current_exe()?.display().to_string().replace('\'', "''");
        let st = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!("$p = Start-Process -FilePath '{exe}' -ArgumentList {list} -Verb RunAs -Wait -PassThru; exit $p.ExitCode"))
            .creation_flags(0x0800_0000)
            .status()
            .context("asking Windows for permission")?;
        if !st.success() {
            let why = std::fs::read_to_string(std::env::temp_dir().join("bekdesk-setup.log")).unwrap_or_default();
            bail!("{}", if why.trim().is_empty() { "the Windows admin prompt was declined or setup failed".to_string() } else { why.trim().to_string() });
        }
        Ok(())
    }

    static STATUS: AtomicUsize = AtomicUsize::new(0);
    static STOP: AtomicBool = AtomicBool::new(false);

    fn report(state: u32) {
        let st = SERVICE_STATUS {
            dwServiceType: SERVICE_WIN32_OWN_PROCESS,
            dwCurrentState: state,
            dwControlsAccepted: if state == SERVICE_RUNNING { SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN } else { 0 },
            dwWin32ExitCode: 0,
            dwServiceSpecificExitCode: 0,
            dwCheckPoint: 0,
            dwWaitHint: 0,
        };
        unsafe { SetServiceStatus(STATUS.load(Ordering::SeqCst) as _, &st) };
    }

    unsafe extern "system" fn control(ctrl: u32, _: u32, _: *mut core::ffi::c_void, _: *mut core::ffi::c_void) -> u32 {
        if ctrl == SERVICE_CONTROL_STOP || ctrl == SERVICE_CONTROL_SHUTDOWN {
            STOP.store(true, Ordering::SeqCst);
        }
        NO_ERROR
    }

    unsafe extern "system" fn service_main(_: u32, _: *mut windows_sys::core::PWSTR) {
        let name = w(NAME);
        let h = RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control), std::ptr::null());
        STATUS.store(h as usize, Ordering::SeqCst);
        report(SERVICE_RUNNING);
        supervise();
        report(SERVICE_STOPPED);
    }

    /// Entry point for `bekdesk-service.exe --service`, started by Windows.
    pub fn run_service() {
        let mut name = w(NAME);
        let table = [
            SERVICE_TABLE_ENTRYW { lpServiceName: name.as_mut_ptr(), lpServiceProc: Some(service_main) },
            SERVICE_TABLE_ENTRYW { lpServiceName: std::ptr::null_mut(), lpServiceProc: None },
        ];
        unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) };
    }

    /// The running client: process handle, the session it runs in and the settings it was started with.
    struct Child {
        process: HANDLE,
        session: u32,
        ini: String,
    }
    impl Drop for Child {
        fn drop(&mut self) {
            unsafe {
                TerminateProcess(self.process, 1);
                CloseHandle(self.process);
            }
        }
    }
    fn alive(c: &Child) -> bool {
        let mut code = 0u32;
        unsafe { GetExitCodeProcess(c.process, &mut code) != 0 && code == STILL_ACTIVE as u32 }
    }

    fn supervise() {
        let mut child: Option<Child> = None;
        while !STOP.load(Ordering::SeqCst) {
            let session = unsafe { WTSGetActiveConsoleSessionId() };
            let ini = std::fs::metadata(client_ini())
                .ok()
                .filter(|m| m.modified().ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() < 15))
                .and_then(|_| std::fs::read_to_string(client_ini()).ok());
            let keep = matches!((&child, &ini), (Some(c), Some(i)) if c.session == session && &c.ini == i && alive(c));
            if !keep {
                child = None; // drops: ends the old client
                if let (Some(i), true) = (ini, session != u32::MAX) {
                    child = start(session, i).ok();
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    /// The token of the session's winlogon.exe: SYSTEM rights in that session, able to reach its lock screen.
    unsafe fn winlogon_token(session: u32) -> Option<HANDLE> {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = None;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let mut sid = u32::MAX;
            if String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case("winlogon.exe") && ProcessIdToSessionId(e.th32ProcessID, &mut sid) != 0 && sid == session {
                found = Some(e.th32ProcessID);
                break;
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
        let proc = OpenProcess(PROCESS_QUERY_INFORMATION, 0, found?);
        if proc.is_null() {
            return None;
        }
        let mut tok: HANDLE = std::ptr::null_mut();
        let opened = OpenProcessToken(proc, TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY, &mut tok) != 0;
        CloseHandle(proc);
        if !opened {
            return None;
        }
        let mut dup: HANDLE = std::ptr::null_mut();
        let ok = DuplicateTokenEx(tok, TOKEN_ALL_ACCESS, std::ptr::null(), SecurityIdentification, TokenPrimary, &mut dup) != 0;
        CloseHandle(tok);
        ok.then_some(dup)
    }

    fn start(session: u32, ini: String) -> Result<Child> {
        let exe = bin_dir().join("deskflow").join("deskflow-core.exe");
        let log = std::fs::File::create(run_dir().join("deskflow-client.log"))?;
        let cmd = format!("\"{}\" client --new-instance -s \"{}\"", exe.display(), client_ini().display());
        unsafe {
            let tok = winlogon_token(session).context("no sign-in process in the active session")?;
            let lh = log.as_raw_handle() as HANDLE;
            SetHandleInformation(lh, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
            let mut desk = w(r"winsta0\default");
            let mut si: STARTUPINFOW = std::mem::zeroed();
            si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            si.lpDesktop = desk.as_mut_ptr();
            si.dwFlags = STARTF_USESTDHANDLES;
            si.hStdOutput = lh;
            si.hStdError = lh;
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let mut cmdw = w(&cmd);
            let dir = w(&exe.parent().unwrap_or(&bin_dir()).to_string_lossy());
            let ok = CreateProcessAsUserW(tok, std::ptr::null(), cmdw.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 1, CREATE_NO_WINDOW, std::ptr::null(), dir.as_ptr(), &si, &mut pi) != 0;
            CloseHandle(tok);
            if !ok {
                bail!("starting the client failed ({})", GetLastError());
            }
            CloseHandle(pi.hThread);
            Ok(Child { process: pi.hProcess, session, ini })
        }
    }
}
