//! System tray of the Windows build: a Win32 notification icon
//! (`Shell_NotifyIconW`) on the hidden window of `win::host`, with the same
//! menu as the Linux StatusNotifierItem (open, lighting on/off, effect,
//! brightness, settings, quit). Desktop notifications use its balloons
//! (shown as toasts by Windows 10/11).

use crate::i18n::t;
use crate::services::ServiceEvent;
use crate::win::host;
use std::sync::{Mutex, OnceLock};
use tokio::sync::mpsc::UnboundedSender;
use tuxrgb_core::EffectKind;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIIF_RESPECT_QUIET_TIME,
    NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_BALLOONUSERCLICK, NIN_SELECT,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIcon, CreatePopupMenu, DestroyMenu, GetCursorPos, GetSystemMetrics, HICON,
    HMENU, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING,
    MFT_RADIOCHECK, PostMessageW, SM_CXSMICON, SM_CYSMICON, SetForegroundWindow,
    SetMenuDefaultItem, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_APP,
    WM_CONTEXTMENU, WM_LBUTTONDBLCLK, WM_NULL,
};
use windows::core::{HSTRING, PCWSTR};

#[derive(Clone, Debug, PartialEq)]
pub struct TrayState {
    pub effect: EffectKind,
    pub brightness: f32,
    pub output_enabled: bool,
    pub status: String,
    /// Interface language (the menu is rebuilt when it changes).
    pub lang: tuxrgb_core::i18n::Lang,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TrayAction {
    ShowWindow,
    SetEffect(EffectKind),
    SetBrightness(f32),
    SetOutput(bool),
    OpenSettings,
    Quit,
}

/// What the icon shows; changed through [`Handle::update`].
pub struct TuxTray {
    pub state: TrayState,
}

const BRIGHTNESS_STEPS: [f32; 5] = [0.1, 0.25, 0.5, 0.75, 1.0];
/// Callback message of the icon.
const WM_TRAY: u32 = WM_APP + 1;
/// Asks the window thread to refresh the icon (tooltip changed).
const WM_TRAY_REFRESH: u32 = WM_APP + 2;
const ICON_ID: u32 = 1;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;

const CMD_OPEN: usize = 1;
const CMD_OUTPUT: usize = 2;
const CMD_SETTINGS: usize = 3;
const CMD_QUIT: usize = 4;
const CMD_EFFECT: usize = 100;
const CMD_BRIGHTNESS: usize = 200;

struct Shared {
    tray: TuxTray,
    events: UnboundedSender<ServiceEvent>,
    shown: bool,
}

fn shared() -> &'static Mutex<Option<Shared>> {
    static SHARED: OnceLock<Mutex<Option<Shared>>> = OnceLock::new();
    SHARED.get_or_init(|| Mutex::new(None))
}

fn with_shared<R>(f: impl FnOnce(&mut Shared) -> R) -> Option<R> {
    let mut guard = shared().lock().unwrap_or_else(|p| p.into_inner());
    guard.as_mut().map(f)
}

/// The application icon at the small-icon size: from the executable's
/// resources (the .ico linked at build time), else drawn from the 32 px
/// tray picture shipped for Linux.
fn icon() -> HICON {
    static ICON: OnceLock<isize> = OnceLock::new();
    let raw = *ICON.get_or_init(|| {
        // SAFETY: resource lookup in our own module; CreateIcon copies the
        // pixel buffers.
        unsafe {
            let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
            if let Ok(module) = GetModuleHandleW(None)
                && let Ok(h) = LoadImageW(
                    module,
                    // MAKEINTRESOURCEW(1): the icon linked by build.rs.
                    PCWSTR(std::ptr::without_provenance(1)),
                    IMAGE_ICON,
                    cx,
                    cy,
                    LR_DEFAULTCOLOR,
                )
            {
                return h.0 as isize;
            }
            // ARGB (big endian, as StatusNotifierItem wants) → BGRA.
            let argb: &[u8] = include_bytes!("../../assets/tray-32.argb");
            let mut bgra = Vec::with_capacity(argb.len());
            for px in argb.chunks_exact(4) {
                bgra.extend_from_slice(&[px[3], px[2], px[1], px[0]]);
            }
            let mask = [0u8; 32 * 32 / 8];
            CreateIcon(None, 32, 32, 1, 32, mask.as_ptr(), bgra.as_ptr())
                .map(|h| h.0 as isize)
                .unwrap_or(0)
        }
    });
    HICON(raw as *mut core::ffi::c_void)
}

fn tooltip(state: &TrayState) -> String {
    if state.output_enabled {
        format!(
            "TuxRGB – {} · {} %",
            t(state.effect.name()),
            (state.brightness * 100.0).round()
        )
    } else {
        format!("TuxRGB – {}", t("Éclairage éteint"))
    }
}

fn base_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        ..Default::default()
    }
}

/// Adds (or refreshes) the icon.
fn show_icon(hwnd: HWND, add: bool) -> bool {
    let Some(tip) = with_shared(|s| tooltip(&s.tray.state)) else {
        return false;
    };
    let mut data = base_data(hwnd);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = icon();
    data.szTip = crate::win::wide_fixed(&tip);
    // SAFETY: fully initialised structure for our own window.
    unsafe {
        if add {
            if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                return false;
            }
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
            true
        } else {
            Shell_NotifyIconW(NIM_MODIFY, &data).as_bool()
        }
    }
}

fn remove_icon(hwnd: HWND) {
    let data = base_data(hwnd);
    // SAFETY: removes our own icon.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

/// Handle of the running icon (the icon itself lives on the host window).
pub struct Handle {
    hwnd: isize,
}

impl Handle {
    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut core::ffi::c_void)
    }

    /// Changes what the icon shows (same shape as `ksni::Handle::update`).
    pub async fn update<F: FnOnce(&mut TuxTray)>(&self, f: F) {
        if with_shared(|s| f(&mut s.tray)).is_some() {
            // The window thread may be busy (shutdown): never wait for it.
            // SAFETY: posting to our own window.
            let _ = unsafe { PostMessageW(self.hwnd(), WM_TRAY_REFRESH, WPARAM(0), LPARAM(0)) };
        }
    }

    pub async fn shutdown(&self) {
        remove_icon(self.hwnd());
        if let Ok(mut g) = shared().lock() {
            *g = None;
        }
    }
}

pub async fn spawn(state: TrayState, tx: UnboundedSender<ServiceEvent>) -> Option<Handle> {
    let Some(hwnd) = host::hwnd() else {
        tracing::warn!("system tray unavailable: no Windows integration window");
        let _ = tx.send(ServiceEvent::TrayAvailable(false));
        return None;
    };
    if let Ok(mut g) = shared().lock() {
        *g = Some(Shared {
            tray: TuxTray { state },
            events: tx.clone(),
            shown: false,
        });
    }
    // Explorer may not be up yet (autostart): the icon is added as soon as
    // the taskbar announces itself ("TaskbarCreated").
    let shown = show_icon(hwnd, true);
    with_shared(|s| s.shown = shown);
    if shown {
        tracing::info!("system tray available");
    } else {
        tracing::info!("system tray not available (yet)");
    }
    let _ = tx.send(ServiceEvent::TrayAvailable(shown));
    Some(Handle {
        hwnd: hwnd.0 as isize,
    })
}

/// Explorer (re)started: the icon must be added again.
pub fn taskbar_created() {
    let Some(hwnd) = host::hwnd() else { return };
    if with_shared(|_| ()).is_none() {
        return;
    }
    let shown = show_icon(hwnd, true);
    if let Some(events) = with_shared(|s| {
        s.shown = shown;
        s.events.clone()
    }) {
        let _ = events.send(ServiceEvent::TrayAvailable(shown));
    }
}

/// A desktop notification, as a tray balloon (a toast on Windows 10/11).
pub fn notify(summary: &str, body: &str) {
    let Some(hwnd) = host::hwnd() else { return };
    if !with_shared(|s| s.shown).unwrap_or(false) {
        return;
    }
    let mut data = base_data(hwnd);
    data.uFlags = NIF_INFO;
    data.szInfoTitle = crate::win::wide_fixed(summary);
    data.szInfo = crate::win::wide_fixed(body);
    data.dwInfoFlags = NIIF_INFO | NIIF_RESPECT_QUIET_TIME;
    // SAFETY: our own icon.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

fn emit(action: TrayAction) {
    if let Some(tx) = with_shared(|s| s.events.clone()) {
        let _ = tx.send(ServiceEvent::Tray(action));
    }
}

fn append(menu: HMENU, flags: u32, id: usize, text: &str) {
    let text = HSTRING::from(text);
    // SAFETY: valid menu, NUL-terminated text copied by Windows.
    let _ = unsafe {
        AppendMenuW(
            menu,
            windows::Win32::UI::WindowsAndMessaging::MENU_ITEM_FLAGS(flags),
            id,
            &text,
        )
    };
}

/// Builds and shows the menu at `(x, y)`, then applies the chosen entry.
fn show_menu(hwnd: HWND, x: i32, y: i32) {
    let Some(state) = with_shared(|s| s.tray.state.clone()) else {
        return;
    };
    // SAFETY: menus created, shown and destroyed on the window thread.
    let cmd = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let Ok(effects) = CreatePopupMenu() else {
            let _ = DestroyMenu(menu);
            return;
        };
        let Ok(levels) = CreatePopupMenu() else {
            let _ = DestroyMenu(menu);
            let _ = DestroyMenu(effects);
            return;
        };
        for (i, e) in EffectKind::ALL.iter().enumerate() {
            let check = if *e == state.effect { MF_CHECKED.0 } else { 0 };
            append(
                effects,
                MF_STRING.0 | MFT_RADIOCHECK.0 | check,
                CMD_EFFECT + i,
                t(e.name()),
            );
        }
        let nearest = BRIGHTNESS_STEPS
            .iter()
            .enumerate()
            .min_by(|a, b| {
                (a.1 - state.brightness)
                    .abs()
                    .total_cmp(&(b.1 - state.brightness).abs())
            })
            .map(|(i, _)| i)
            .unwrap_or(4);
        for (i, b) in BRIGHTNESS_STEPS.iter().enumerate() {
            let check = if i == nearest { MF_CHECKED.0 } else { 0 };
            append(
                levels,
                MF_STRING.0 | MFT_RADIOCHECK.0 | check,
                CMD_BRIGHTNESS + i,
                &format!("{} %", (b * 100.0) as u32),
            );
        }
        append(menu, MF_STRING.0, CMD_OPEN, t("Ouvrir TuxRGB"));
        let _ = SetMenuDefaultItem(menu, CMD_OPEN as u32, 0);
        append(menu, MF_SEPARATOR.0, 0, "");
        let check = if state.output_enabled {
            MF_CHECKED.0
        } else {
            0
        };
        append(menu, MF_STRING.0 | check, CMD_OUTPUT, t("Éclairage allumé"));
        append(menu, MF_POPUP.0, effects.0 as usize, t("Effet"));
        append(menu, MF_POPUP.0, levels.0 as usize, t("Luminosité"));
        append(menu, MF_SEPARATOR.0, 0, "");
        append(menu, MF_STRING.0, CMD_SETTINGS, t("Paramètres…"));
        append(menu, MF_STRING.0, CMD_QUIT, t("Quitter"));
        // Without this the menu does not close when clicking elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN).0,
            x,
            y,
            hwnd,
            None,
        );
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        // Destroying the menu destroys its submenus.
        let _ = DestroyMenu(menu);
        cmd.0 as usize
    };
    match cmd {
        0 => {}
        CMD_OPEN => emit(TrayAction::ShowWindow),
        CMD_OUTPUT => {
            let on = !state.output_enabled;
            with_shared(|s| s.tray.state.output_enabled = on);
            emit(TrayAction::SetOutput(on));
        }
        CMD_SETTINGS => emit(TrayAction::OpenSettings),
        CMD_QUIT => emit(TrayAction::Quit),
        c if (CMD_EFFECT..CMD_EFFECT + EffectKind::ALL.len()).contains(&c) => {
            let e = EffectKind::ALL[c - CMD_EFFECT];
            with_shared(|s| s.tray.state.effect = e);
            emit(TrayAction::SetEffect(e));
        }
        c if (CMD_BRIGHTNESS..CMD_BRIGHTNESS + BRIGHTNESS_STEPS.len()).contains(&c) => {
            let b = BRIGHTNESS_STEPS[c - CMD_BRIGHTNESS];
            with_shared(|s| s.tray.state.brightness = b);
            emit(TrayAction::SetBrightness(b));
        }
        _ => {}
    }
}

/// Tray messages of the host window (`None`: not a tray message).
pub fn handle_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_TRAY => {
            // NOTIFYICON_VERSION_4: event in the low word of lParam, anchor
            // point in wParam.
            let event = (lparam.0 as u32) & 0xffff;
            match event {
                NIN_SELECT | NIN_KEYSELECT | WM_LBUTTONDBLCLK => emit(TrayAction::ShowWindow),
                WM_CONTEXTMENU => {
                    let (mut x, mut y) = (
                        (wparam.0 & 0xffff) as i16 as i32,
                        ((wparam.0 >> 16) & 0xffff) as i16 as i32,
                    );
                    let mut p = POINT::default();
                    // SAFETY: valid out-pointer.
                    if (x, y) == (0, 0) && unsafe { GetCursorPos(&mut p) }.is_ok() {
                        (x, y) = (p.x, p.y);
                    }
                    show_menu(hwnd, x, y);
                }
                NIN_BALLOONUSERCLICK => {
                    if let Some(tx) = with_shared(|s| s.events.clone()) {
                        let _ = tx.send(ServiceEvent::NotificationClicked(None));
                    }
                }
                _ => {}
            }
            Some(LRESULT(0))
        }
        WM_TRAY_REFRESH => {
            if with_shared(|s| s.shown).unwrap_or(false) {
                show_icon(hwnd, false);
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}
