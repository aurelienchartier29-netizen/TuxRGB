//! Administrator rights on Windows.
//!
//! USB / HID devices (keyboards, mice, USB lighting controllers) work with
//! normal rights. The motherboard and the RAM are reached over SMBus, which
//! OpenRGB only opens with the PawnIO driver installed *and* administrator
//! rights. Asking for them at every start (UAC prompt) would break the
//! login autostart, so TuxRGB does what most Windows RGB tools do:
//!
//! 1. once, with the UAC prompt, `tuxrgb.exe --windows-setup install`
//!    (elevated) installs PawnIO if the installer left its setup next to
//!    TuxRGB, and registers an on-demand scheduled task "TuxRGB (user)"
//!    that runs `tuxrgb.exe --elevated-handoff` with the highest rights of
//!    the logged-in user;
//! 2. from then on, a normal launch (Start menu, login, tray) writes what
//!    it was asked for (show the window, start in the tray…) to a small
//!    hand-over file, runs the task (no prompt) and exits; the elevated
//!    instance reads the file and starts – OpenRGB, its child, inherits the
//!    rights.
//!
//! `--windows-setup reset` removes the task (Réglages → Réinitialiser).

use crate::instance::ActivateRequest;
use crate::win::{HKEY_LOCAL_MACHINE, hidden_command, reg_key_exists};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Exit codes of the elevated setup (same meaning as `tuxrgb-helper`'s).
pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;

/// The Program Files folders, as Windows names them (known folders, not
/// the environment, which the user's other programs can change). Only
/// administrators can write there.
pub fn protected_dirs() -> Vec<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    };
    let mut out = Vec::new();
    for id in [FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86] {
        // SAFETY: the returned string is copied, then freed as documented.
        unsafe {
            if let Ok(p) = SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None) {
                if let Ok(s) = p.to_string() {
                    let path = PathBuf::from(s);
                    if !out.contains(&path) {
                        out.push(path);
                    }
                }
                CoTaskMemFree(Some(p.0 as *const _));
            }
        }
    }
    out
}

/// This `tuxrgb.exe` lives in a folder only administrators can change (an
/// installed copy, not a portable one in Downloads).
pub fn exe_is_protected() -> bool {
    std::env::current_exe()
        .is_ok_and(|exe| tuxrgb_openrgb::locate::is_inside(&exe, &protected_dirs()))
}

/// Writes `bytes` to a new file, then keeps it open for reading only,
/// shared for reading only: until the returned handle is dropped nobody can
/// change, replace or delete it. The content read back is checked, so a
/// swap between the write and the lock is caught.
pub fn write_locked(path: &Path, bytes: &[u8]) -> Result<std::fs::File, String> {
    use std::io::{Read, Write};
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 1;
    let _ = std::fs::remove_file(path);
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        f.write_all(bytes).map_err(|e| e.to_string())?;
    }
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .map_err(|e| e.to_string())?;
    let mut back = Vec::with_capacity(bytes.len());
    f.read_to_end(&mut back).map_err(|e| e.to_string())?;
    if back != bytes {
        return Err("fichier modifié pendant l'écriture".into());
    }
    Ok(f)
}

/// TuxRGB runs with administrator rights (asked once, then remembered).
pub fn is_elevated() -> bool {
    static ELEVATED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ELEVATED.get_or_init(query_elevated)
}

fn query_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    // SAFETY: token of our own process, closed after the query.
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// The PawnIO driver (SMBus access for OpenRGB on Windows) is installed.
pub fn pawnio_installed() -> bool {
    reg_key_exists(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Services\PawnIO",
    )
}

fn user_name() -> String {
    std::env::var("USERNAME").unwrap_or_else(|_| "user".into())
}

/// Scheduled task of this user (one per account on the machine).
pub fn task_name() -> String {
    format!(r"\TuxRGB\TuxRGB ({})", user_name())
}

/// Written by the setup: administrator mode is configured for this user
/// (avoids asking the Task Scheduler at every launch).
fn marker() -> PathBuf {
    crate::paths::paths().config_dir.join("windows-admin-mode")
}

/// Administrator mode is configured.
pub fn admin_mode_configured() -> bool {
    marker().exists()
}

fn handoff_file() -> PathBuf {
    crate::paths::paths().config_dir.join("handoff")
}

/// Saves what this launch was asked for, for the elevated instance.
fn write_handoff(req: &ActivateRequest) -> bool {
    let text = format!(
        "reset_hardware={}\nminimized={}\n",
        req.reset_hardware as u8, req.minimized as u8
    );
    crate::paths::write_atomic(&handoff_file(), &text).is_ok()
}

/// The request of the launch that handed over (elevated instance), read
/// once and deleted.
pub fn take_handoff() -> Option<ActivateRequest> {
    let path = handoff_file();
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let flag = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .is_some_and(|v| v.trim() == "1")
    };
    Some(ActivateRequest {
        reset_hardware: flag("reset_hardware"),
        minimized: flag("minimized"),
    })
}

/// Starts the elevated instance through the scheduled task.
fn run_task() -> bool {
    match hidden_command("schtasks.exe")
        .args(["/run", "/tn", &task_name()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(s) if s.success() => true,
        Ok(s) => {
            tracing::warn!("administrator task did not start ({s})");
            false
        }
        Err(e) => {
            tracing::warn!("schtasks unavailable: {e}");
            false
        }
    }
}

/// Normal launch with administrator mode configured: hands over to the
/// elevated instance. `false`: carry on with normal rights (the task is
/// gone: the marker is removed so the next launch does not try again).
pub fn hand_over(req: &ActivateRequest) -> bool {
    if is_elevated() || !admin_mode_configured() {
        return false;
    }
    if !write_handoff(req) {
        return false;
    }
    // This launch comes from the user: it may let the elevated instance
    // (started by the Task Scheduler) bring its window to the front.
    {
        use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
        // SAFETY: plain call, no pointers.
        let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    }
    if run_task() {
        tracing::info!("handed over to the administrator instance");
        true
    } else {
        let _ = std::fs::remove_file(handoff_file());
        let _ = std::fs::remove_file(marker());
        false
    }
}

static HANDOVER_PENDING: AtomicBool = AtomicBool::new(false);

/// The setup just enabled administrator mode while this instance runs with
/// normal rights: it must hand over (now, or when the wizard closes).
pub fn mark_handover_pending() {
    if !is_elevated() && admin_mode_configured() {
        HANDOVER_PENDING.store(true, Ordering::Relaxed);
    }
}

/// Starts the elevated instance if a hand-over is pending; `true` when this
/// instance must now quit (leaving the LEDs as they are).
pub fn take_pending_handover(window_open: bool) -> bool {
    if !HANDOVER_PENDING.swap(false, Ordering::Relaxed) {
        return false;
    }
    hand_over(&ActivateRequest {
        reset_hardware: false,
        minimized: !window_open,
    })
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Task definition: no trigger (started on demand only), highest rights of
/// the interactive user, no time limit, normal priority (the Task
/// Scheduler default, 7, would slow the effects down).
fn task_xml(exe: &Path, user: &str) -> String {
    let dir = exe.parent().unwrap_or(Path::new("."));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>TuxRGB avec les droits administrateur (carte mère, RAM via SMBus), lancé par TuxRGB sans invite UAC.</Description>
  </RegistrationInfo>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>--elevated-handoff</Arguments>
      <WorkingDirectory>{dir}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#,
        user = xml_escape(user),
        exe = xml_escape(&exe.to_string_lossy()),
        dir = xml_escape(&dir.to_string_lossy()),
    )
}

fn schtasks(args: &[&str]) -> Result<(), String> {
    let out = hidden_command("schtasks.exe")
        .args(args)
        .output()
        .map_err(|e| format!("schtasks : {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(if msg.is_empty() {
            format!("schtasks : code {}", out.status.code().unwrap_or(-1))
        } else {
            msg
        })
    }
}

/// PawnIO setup left next to TuxRGB by the installer (optional).
fn bundled_pawnio_setup() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.join("pawnio").join("PawnIO_setup.exe");
    p.is_file().then_some(p)
}

fn install(steps: &mut Vec<String>) -> Result<(), String> {
    // The task starts this file with administrator rights at every launch,
    // without asking: a copy that other programs can rewrite (Downloads,
    // a portable folder) would give them those rights.
    if !exe_is_protected() {
        return Err(crate::i18n::t(
            "le mode administrateur demande TuxRGB installé dans Program Files (installateur)",
        )
        .into());
    }
    if !pawnio_installed() {
        match bundled_pawnio_setup() {
            Some(setup) => {
                let status = hidden_command(&setup.to_string_lossy())
                    .args(["-install", "-silent"])
                    .status()
                    .map_err(|e| format!("PawnIO : {e}"))?;
                if status.success() && pawnio_installed() {
                    steps.push("pawnio-installed".into());
                } else {
                    steps.push("pawnio-failed".into());
                }
            }
            None => steps.push("pawnio-missing".into()),
        }
    } else {
        steps.push("pawnio-present".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let user = match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => format!(r"{d}\{}", user_name()),
        _ => user_name(),
    };
    // schtasks wants the definition as UTF-16 with a byte order mark.
    let xml = task_xml(&exe, &user);
    let mut bytes = vec![0xFF, 0xFE];
    for unit in xml.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    // Locked while schtasks reads it: nobody can swap the definition.
    let file = std::env::temp_dir().join(format!("tuxrgb-task-{}.xml", std::process::id()));
    let lock = write_locked(&file, &bytes)?;
    let created = schtasks(&[
        "/create",
        "/tn",
        &task_name(),
        "/xml",
        &file.to_string_lossy(),
        "/f",
    ]);
    drop(lock);
    let _ = std::fs::remove_file(&file);
    created?;
    steps.push("elevated-task".into());
    crate::paths::write_atomic(&marker(), "1\n").map_err(|e| e.to_string())?;
    Ok(())
}

fn reset(steps: &mut Vec<String>) -> Result<(), String> {
    // A task that is already gone is fine.
    if schtasks(&["/delete", "/tn", &task_name(), "/f"]).is_ok() {
        steps.push("elevated-task-removed".into());
    }
    let _ = std::fs::remove_file(marker());
    Ok(())
}

/// Runs a setup action in this (elevated) process.
pub fn run_action(action: &str) -> Result<Vec<String>, String> {
    let mut steps = Vec::new();
    match action {
        "install" => install(&mut steps)?,
        "reset" => reset(&mut steps)?,
        // Nothing to unlock in the firmware on Windows: PawnIO does it.
        "enable-smbus" => steps.push("smbus-already-functional".into()),
        "status" => {}
        other => return Err(format!("action inconnue : {other}")),
    }
    Ok(steps)
}

/// `tuxrgb.exe --windows-setup <action> <report-file>`, started elevated by
/// [`run_elevated`] (or by the installer): writes `STEP …` lines and a
/// `RESULT …` line, like the Linux helper does on its standard output.
pub fn setup_main(action: &str, report: Option<&Path>) -> i32 {
    let (text, code) = match run_action(action) {
        Ok(steps) => {
            let mut t: String = steps.iter().map(|s| format!("STEP {s}\n")).collect();
            t.push_str("RESULT ok\n");
            (t, EXIT_OK)
        }
        Err(e) => (format!("RESULT error {e}\n"), EXIT_FAILED),
    };
    match report {
        Some(p) => {
            // A new file only: never through a link or over an existing
            // file planted at that name.
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(p)
            {
                let _ = f.write_all(text.as_bytes());
            }
        }
        None => print!("{text}"),
    }
    code
}

/// What happened to an elevated run.
pub enum Elevated {
    /// Exit code and report of the elevated setup.
    Done(i32, String),
    /// The user said no to the UAC prompt.
    Cancelled,
    Failed(String),
}

/// Runs `tuxrgb.exe --windows-setup <action>` with the UAC prompt and
/// waits for it (blocking: call from a worker thread).
pub fn run_elevated(action: &str) -> Elevated {
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED};
    use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
    use windows::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    use windows::core::{HSTRING, w};

    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return Elevated::Failed(e.to_string()),
    };
    let report = std::env::temp_dir().join(format!(
        "tuxrgb-setup-{}-{}.txt",
        std::process::id(),
        action
    ));
    let _ = std::fs::remove_file(&report);
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(format!("--windows-setup {action} \"{}\"", report.display()));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: crate::win::pcwstr(&file),
        lpParameters: crate::win::pcwstr(&params),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: the strings outlive the call; the process handle is closed.
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            return if e.code() == ERROR_CANCELLED.to_hresult() {
                Elevated::Cancelled
            } else {
                Elevated::Failed(e.message())
            };
        }
        if info.hProcess.is_invalid() {
            return Elevated::Failed("processus introuvable".into());
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        let text = std::fs::read_to_string(&report).unwrap_or_default();
        let _ = std::fs::remove_file(&report);
        Elevated::Done(code as i32, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_files_is_protected_and_downloads_is_not() {
        let dirs = protected_dirs();
        assert!(!dirs.is_empty());
        let inside = dirs[0].join("TuxRGB").join("tuxrgb.exe");
        assert!(tuxrgb_openrgb::locate::is_inside(&inside, &dirs));
        let outside = std::env::temp_dir().join("tuxrgb.exe");
        assert!(!tuxrgb_openrgb::locate::is_inside(&outside, &dirs));
    }

    #[test]
    fn a_locked_file_cannot_be_swapped() {
        let path = std::env::temp_dir().join(format!("tuxrgb-lock-{}.xml", std::process::id()));
        let lock = write_locked(&path, b"<Task/>").unwrap();
        // Nobody can rewrite it while it is held.
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::remove_file(&path).is_err() || !path.exists());
        drop(lock);
        let _ = std::fs::remove_file(&path);
    }
}
