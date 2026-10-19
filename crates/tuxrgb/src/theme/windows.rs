//! Desktop appearance on Windows (beta build): light / dark mode, accent
//! colour, animations – the same interface as the Linux module (portal,
//! kdeglobals…), fed by Windows itself:
//!
//! * accent – WinRT `UISettings.GetColorValue(UIColorType.Accent)`, else
//!   the registry (`HKCU\Software\Microsoft\Windows\DWM\AccentColor`);
//! * dark mode – `AppsUseLightTheme` (Personalize key);
//! * reduced motion – "Animation effects" off (`SPI_GETCLIENTAREAANIMATION`).
//!
//! Changes arrive as `WM_SETTINGCHANGE` / `WM_DWMCOLORIZATIONCOLORCHANGED`
//! on the hidden window of `win::host`.

use crate::win::{HKEY_CURRENT_USER, reg_dword};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tuxrgb_core::Rgb;

#[derive(Clone, Debug, PartialEq)]
pub struct SystemAppearance {
    /// `None` = the desktop has no preference.
    pub dark: Option<bool>,
    pub accent: Option<Rgb>,
    pub reduced_motion: bool,
    pub source: &'static str,
}

impl Default for SystemAppearance {
    fn default() -> Self {
        Self {
            dark: None,
            accent: None,
            reduced_motion: false,
            source: "défaut",
        }
    }
}

const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const DWM: &str = r"Software\Microsoft\Windows\DWM";

fn dark_mode() -> Option<bool> {
    reg_dword(HKEY_CURRENT_USER, PERSONALIZE, "AppsUseLightTheme").map(|light| light == 0)
}

/// Accent through WinRT (Windows 10 1809+).
fn winrt_accent() -> Option<Rgb> {
    use windows::UI::ViewManagement::{UIColorType, UISettings};
    let settings = UISettings::new().ok()?;
    let c = settings.GetColorValue(UIColorType::Accent).ok()?;
    Some(Rgb::new(c.R, c.G, c.B))
}

/// Accent from the registry (`0xAABBGGRR`, then `ColorizationColor`,
/// `0xAARRGGBB`).
fn registry_accent() -> Option<Rgb> {
    if let Some(v) = reg_dword(HKEY_CURRENT_USER, DWM, "AccentColor") {
        return Some(Rgb::new(
            (v & 0xff) as u8,
            ((v >> 8) & 0xff) as u8,
            ((v >> 16) & 0xff) as u8,
        ));
    }
    reg_dword(HKEY_CURRENT_USER, DWM, "ColorizationColor").map(|v| {
        Rgb::new(
            ((v >> 16) & 0xff) as u8,
            ((v >> 8) & 0xff) as u8,
            (v & 0xff) as u8,
        )
    })
}

fn reduced_motion() -> bool {
    use windows::Win32::Foundation::BOOL;
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut enabled = BOOL(1);
    // SAFETY: the action writes one BOOL.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut enabled as *mut BOOL as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok();
    ok && !enabled.as_bool()
}

fn read_now() -> SystemAppearance {
    let (accent, source) = match winrt_accent() {
        Some(c) => (Some(c), "WinRT UISettings"),
        None => (registry_accent(), "registre Windows"),
    };
    SystemAppearance {
        dark: dark_mode(),
        accent,
        reduced_motion: reduced_motion(),
        source: if accent.is_some() { source } else { "défaut" },
    }
}

/// First paint, synchronously (named after the Linux fast path): dark mode
/// and accent from the registry.
pub fn read_kdeglobals() -> (Option<bool>, Option<Rgb>) {
    (dark_mode(), registry_accent())
}

pub async fn read() -> SystemAppearance {
    tokio::task::spawn_blocking(read_now)
        .await
        .unwrap_or_default()
}

/// Pushes a fresh [`SystemAppearance`] every time Windows reports a change
/// (a burst of messages is folded into one read; only real changes are
/// sent).
pub async fn watch(tx: UnboundedSender<SystemAppearance>) {
    let (dirty_tx, mut dirty_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    crate::win::host::set_appearance_sink(dirty_tx);
    // The window must exist to receive the broadcasts.
    let _ = tokio::task::spawn_blocking(|| crate::win::host::hwnd().is_some()).await;
    let mut last = read().await;
    let _ = tx.send(last.clone());
    while dirty_rx.recv().await.is_some() {
        tokio::time::sleep(Duration::from_millis(150)).await;
        while dirty_rx.try_recv().is_ok() {}
        let now = read().await;
        if now != last {
            last = now.clone();
            let _ = tx.send(now);
        }
    }
}
