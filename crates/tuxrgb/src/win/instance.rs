//! Single instance on Windows (beta build): a named mutex in the session
//! namespace, and `WM_COPYDATA` to the hidden window of the running
//! TuxRGB (see `win::host`) to bring it forward.
//!
//! Administrator mode (`win::elevation`): a normal launch hands over to
//! the elevated scheduled task; the elevated process started that way
//! waits for the previous instance to finish quitting.

use std::sync::{Mutex, OnceLock};
use tokio::sync::mpsc;
use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HANDLE, LPARAM, WAIT_ABANDONED, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, SMTO_ABORTIFHUNG,
    SendMessageTimeoutW, WM_COPYDATA,
};
use windows::core::w;

/// What a second launch asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivateRequest {
    /// `--reset-hardware`: show the hardware access wizard.
    pub reset_hardware: bool,
    /// `--minimized` (autostart): nothing to show.
    pub minimized: bool,
}

pub enum Role {
    /// This process is the instance.
    Primary,
    /// Another TuxRGB is running and has been asked to come forward.
    Secondary,
}

static ACTIVATIONS: OnceLock<Mutex<Option<mpsc::UnboundedReceiver<ActivateRequest>>>> =
    OnceLock::new();

/// Activation requests received by the primary instance (taken once by the
/// services worker).
pub fn take_activations() -> Option<mpsc::UnboundedReceiver<ActivateRequest>> {
    ACTIVATIONS.get()?.lock().ok()?.take()
}

/// Session-wide name (one TuxRGB per logged-in user).
const MUTEX_NAME: windows::core::PCWSTR = w!("Local\\io.github.tuxrgb.TuxRGB");

/// Asks the running instance to show itself.
fn forward(request: &ActivateRequest) {
    // SAFETY: plain window lookup and a bounded synchronous message; the
    // data outlives the call.
    unsafe {
        let Ok(hwnd) = FindWindowW(crate::win::host::CLASS_NAME, None) else {
            tracing::warn!("the running TuxRGB has no window to activate");
            return;
        };
        // This launch was started by the user, so it may give the running
        // instance the right to come to the foreground.
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != 0 {
            let _ = AllowSetForegroundWindow(pid);
        }
        let bytes = [request.reset_hardware as u8, request.minimized as u8];
        let data = COPYDATASTRUCT {
            dwData: crate::win::host::COPYDATA_ACTIVATE,
            cbData: bytes.len() as u32,
            lpData: bytes.as_ptr() as *mut _,
        };
        let mut result = 0usize;
        SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&data as *const _ as isize),
            SMTO_ABORTIFHUNG,
            2000,
            Some(&mut result),
        );
    }
}

/// Claims the instance, or forwards `request` to the running one.
///
/// `wait_for_previous`: started by the administrator-mode hand-over, the
/// previous (non-elevated) instance is still quitting: wait for it.
pub fn claim_with(request: ActivateRequest, wait_for_previous: bool) -> Role {
    // SAFETY: named mutex creation; the handle is kept for the whole life
    // of the process (ownership released by Windows when it exits).
    let handle: HANDLE = match unsafe { CreateMutexW(None, true, MUTEX_NAME) } {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("single instance unavailable ({e})");
            return primary();
        }
    };
    // SAFETY: read right after CreateMutexW, as documented.
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if existed {
        if !wait_for_previous {
            forward(&request);
            return Role::Secondary;
        }
        // SAFETY: waiting on our own valid handle.
        let r = unsafe { WaitForSingleObject(handle, 15_000) };
        if r != WAIT_OBJECT_0 && r != WAIT_ABANDONED {
            tracing::warn!("previous TuxRGB still running after 15 s");
            forward(&request);
            return Role::Secondary;
        }
    }
    // The handle is never closed: the mutex is held for the life of the
    // process (Windows releases it when TuxRGB exits, even after a crash).
    let _ = handle;
    primary()
}

fn primary() -> Role {
    let (tx, rx) = mpsc::unbounded_channel();
    let _ = ACTIVATIONS.set(Mutex::new(Some(rx)));
    crate::win::host::set_activate_sink(tx);
    // The window that receives second launches, shutdown and theme events.
    if crate::win::host::hwnd().is_none() {
        tracing::warn!("Windows integration window unavailable");
    }
    Role::Primary
}
