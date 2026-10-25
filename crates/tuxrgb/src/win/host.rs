//! The hidden Win32 window of TuxRGB (Windows build).
//!
//! A top-level window that is never shown (message-only windows do not
//! receive broadcasts), on its own thread, for the whole process:
//!
//! * `WM_QUERYENDSESSION` / `WM_ENDSESSION` – shutdown, restart, logoff:
//!   the LEDs go dark before Windows continues (the counterpart of the
//!   logind "delay" inhibitor), and come back if the shutdown is cancelled;
//! * `WM_POWERBROADCAST` – sleep (`PBT_APMSUSPEND`) and resume;
//! * `WM_SETTINGCHANGE` / `WM_DWMCOLORIZATIONCOLORCHANGED` – accent colour
//!   or light / dark mode changed;
//! * `WM_COPYDATA` – a second launch asks this instance to come forward;
//! * the tray icon messages (see `win/tray.rs`).

use crate::instance::ActivateRequest;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
use windows::Win32::System::Threading::SetProcessShutdownParameters;
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW,
    MSG, MSGFLT_ALLOW, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
    RegisterClassExW, RegisterWindowMessageW, TranslateMessage, WINDOW_EX_STYLE, WM_COPYDATA,
    WM_ENDSESSION, WM_POWERBROADCAST, WM_QUERYENDSESSION, WM_SETTINGCHANGE, WM_SYSCOLORCHANGE,
    WM_THEMECHANGED, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

/// Window class, also how a second launch finds the running instance.
pub const CLASS_NAME: PCWSTR = w!("TuxRGB.Host");
/// `WM_COPYDATA` tag of an activation request.
pub const COPYDATA_ACTIVATE: usize = 0x5452_4742; // "TRGB"

const WM_DWMCOLORIZATIONCOLORCHANGED: u32 = 0x0320;

/// Highest shutdown level of applications (0x100–0x3FF; default 0x280).
const SHUTDOWN_LEVEL: u32 = 0x3FF;

/// Longest time Windows waits for the LEDs at shutdown, restart, logoff
/// and sleep: the blackout's strict limit plus the messages (Windows itself
/// would allow about five seconds, and two for sleep).
const END_SESSION_BUDGET: Duration = Duration::from_millis(600);
const SUSPEND_BUDGET: Duration = Duration::from_millis(600);

/// Shutdown / sleep notifications for the service loop.
pub enum PowerSignal {
    /// Shutdown, restart or logoff starting: turn the LEDs off, then
    /// answer (the window procedure waits for it, within a budget).
    EndSession(std_mpsc::Sender<()>),
    /// The shutdown was cancelled (an application refused it).
    EndSessionCancelled,
    /// Going to sleep: turn the LEDs off, then answer.
    Suspend(std_mpsc::Sender<()>),
    Resumed,
}

#[derive(Default)]
struct Sinks {
    power: Option<UnboundedSender<PowerSignal>>,
    appearance: Option<UnboundedSender<()>>,
    activate: Option<UnboundedSender<ActivateRequest>>,
}

fn sinks() -> std::sync::MutexGuard<'static, Sinks> {
    static SINKS: OnceLock<Mutex<Sinks>> = OnceLock::new();
    SINKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Receives shutdown / sleep notifications from now on.
pub fn set_power_sink(tx: UnboundedSender<PowerSignal>) {
    sinks().power = Some(tx);
}

/// Receives "the theme changed" notifications from now on.
pub fn set_appearance_sink(tx: UnboundedSender<()>) {
    sinks().appearance = Some(tx);
}

/// Receives the activation requests of second launches.
pub fn set_activate_sink(tx: UnboundedSender<ActivateRequest>) {
    sinks().activate = Some(tx);
}

/// Window-thread state (atomics: one window, one thread).
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
/// A shutdown was announced (`WM_QUERYENDSESSION`) and not yet concluded.
static ENDING: AtomicBool = AtomicBool::new(false);
/// The LEDs were turned off for sleep.
static SUSPENDED: AtomicBool = AtomicBool::new(false);

/// The hidden window, created on first use (`None` if Win32 refused).
pub fn hwnd() -> Option<HWND> {
    static HWND_RAW: OnceLock<Option<isize>> = OnceLock::new();
    HWND_RAW
        .get_or_init(|| {
            let (tx, rx) = std_mpsc::channel::<Option<isize>>();
            let spawned = std::thread::Builder::new()
                .name("tuxrgb-win32".into())
                .spawn(move || {
                    let hwnd = create_window();
                    let _ = tx.send(hwnd.map(|h| h.0 as isize));
                    if hwnd.is_some() {
                        message_loop();
                    }
                });
            if spawned.is_err() {
                return None;
            }
            rx.recv_timeout(Duration::from_secs(5)).ok().flatten()
        })
        .map(|h| HWND(h as *mut core::ffi::c_void))
}

fn create_window() -> Option<HWND> {
    // SAFETY: standard window class registration and creation; the class
    // and window live as long as the process.
    unsafe {
        // Shutdown order: Windows ends the applications one after the other
        // (WM_QUERYENDSESSION, WM_ENDSESSION, then the process is ended),
        // highest level first. TuxRGB asks to go first, while OpenRGB (and
        // its access to the devices) is still there: otherwise OpenRGB could
        // be ended before the LEDs are turned off, and they kept their
        // colours after shutdown.
        if let Err(e) = SetProcessShutdownParameters(SHUTDOWN_LEVEL, 0) {
            tracing::warn!("cannot set the shutdown order: {e}");
        }
        let instance = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            tracing::warn!("cannot register the TuxRGB window class");
            return None;
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
            CLASS_NAME,
            w!("TuxRGB"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            instance,
            None,
        )
        .ok()?;
        let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
        TASKBAR_CREATED.store(taskbar_created, Ordering::Relaxed);
        // An elevated TuxRGB (administrator mode) must still hear from the
        // non-elevated Explorer and from a second launch.
        for msg in [WM_COPYDATA, taskbar_created] {
            let _ = ChangeWindowMessageFilterEx(hwnd, msg, MSGFLT_ALLOW, None);
        }
        tracing::info!("Windows integration window ready");
        Some(hwnd)
    }
}

fn message_loop() {
    let mut msg = MSG::default();
    // SAFETY: classic message pump of the thread that owns the window.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Sends `make(done)` to the power sink and waits for the answer.
fn power_round_trip(make: impl FnOnce(std_mpsc::Sender<()>) -> PowerSignal, budget: Duration) {
    let (tx, rx) = std_mpsc::channel();
    let sent = sinks()
        .power
        .as_ref()
        .is_some_and(|p| p.send(make(tx)).is_ok());
    if sent {
        let _ = rx.recv_timeout(budget);
    }
}

fn notify_power(signal: PowerSignal) {
    if let Some(p) = &sinks().power {
        let _ = p.send(signal);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if let Some(r) = crate::tray::handle_message(hwnd, msg, wparam, lparam) {
        return r;
    }
    let taskbar_created = TASKBAR_CREATED.load(Ordering::Relaxed);
    match msg {
        WM_QUERYENDSESSION => {
            tracing::info!(flags = lparam.0, "session ending: turning the LEDs off");
            // Only shown if Windows had to wait (it never does that long).
            let reason =
                windows::core::HSTRING::from(crate::i18n::t("Extinction de l'éclairage RGB…"));
            // SAFETY: valid window, NUL-terminated string alive for the call.
            let _ = unsafe { ShutdownBlockReasonCreate(hwnd, PCWSTR(reason.as_ptr())) };
            power_round_trip(PowerSignal::EndSession, END_SESSION_BUDGET);
            // SAFETY: valid window.
            let _ = unsafe { ShutdownBlockReasonDestroy(hwnd) };
            ENDING.store(true, Ordering::Relaxed);
            LRESULT(1)
        }
        WM_ENDSESSION => {
            let ending = ENDING.swap(false, Ordering::Relaxed);
            if wparam.0 == 0 && ending {
                tracing::info!("shutdown cancelled");
                notify_power(PowerSignal::EndSessionCancelled);
            }
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            match wparam.0 as u32 {
                PBT_APMSUSPEND => {
                    tracing::info!("going to sleep: turning the LEDs off");
                    power_round_trip(PowerSignal::Suspend, SUSPEND_BUDGET);
                    SUSPENDED.store(true, Ordering::Relaxed);
                }
                // Both arrive after a user-initiated wake: one restore.
                PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => {
                    let was = SUSPENDED.swap(false, Ordering::Relaxed);
                    if was {
                        tracing::info!("resumed from sleep");
                        notify_power(PowerSignal::Resumed);
                    }
                }
                _ => {}
            }
            LRESULT(1)
        }
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED | WM_DWMCOLORIZATIONCOLORCHANGED => {
            if let Some(a) = &sinks().appearance {
                let _ = a.send(());
            }
            // SAFETY: default processing of a valid message.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_COPYDATA => {
            // SAFETY: Windows guarantees the structure for WM_COPYDATA.
            let data = unsafe { (lparam.0 as *const COPYDATASTRUCT).as_ref() };
            let Some(data) = data.filter(|d| d.dwData == COPYDATA_ACTIVATE) else {
                return LRESULT(0);
            };
            let bytes: &[u8] = if data.lpData.is_null() || data.cbData < 2 {
                &[0, 0]
            } else {
                // SAFETY: cbData bytes are readable during the message.
                unsafe { std::slice::from_raw_parts(data.lpData as *const u8, 2) }
            };
            let request = ActivateRequest {
                reset_hardware: bytes[0] != 0,
                minimized: bytes[1] != 0,
            };
            tracing::info!(?request, "activation requested by a new launch");
            if let Some(a) = &sinks().activate {
                let _ = a.send(request);
            }
            LRESULT(1)
        }
        m if m != 0 && m == taskbar_created => {
            // Explorer restarted: the icon must be added again.
            crate::tray::taskbar_created();
            LRESULT(0)
        }
        // SAFETY: default processing of every other message.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
