//! Windows build: the system services that replace the Linux ones
//! (D-Bus, logind, StatusNotifierItem, XDG portals).
//!
//! * [`host`] – one hidden Win32 window for the whole process: shutdown /
//!   sleep (`WM_QUERYENDSESSION`, `WM_POWERBROADCAST`), theme changes
//!   (`WM_SETTINGCHANGE`), second launches (`WM_COPYDATA`) and the tray
//!   icon callbacks;
//! * [`elevation`] – administrator rights for the SMBus devices (RAM,
//!   motherboard), through a scheduled task created once;
//! * the tray (`win/tray.rs`), the accent colour (`theme/windows.rs`), the
//!   single instance (`win/instance.rs`) and autostart (`win/autostart.rs`)
//!   replace the Linux modules of the same name.
//!
//! The interface and the effects are the same as on Linux: this code only
//! gives them the Windows plumbing they need.

pub mod elevation;
pub mod host;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, REG_DWORD, REG_SZ, REG_VALUE_TYPE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegDeleteKeyValueW,
    RegGetValueW, RegSetKeyValueW,
};
use windows::core::{HSTRING, PCWSTR};

pub use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

/// Edition shown in the interface and the logs.
pub const EDITION: &str = "Windows";

/// A `DWORD` registry value.
pub fn reg_dword(root: HKEY, key: &str, value: &str) -> Option<u32> {
    let (key, value) = (HSTRING::from(key), HSTRING::from(value));
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: `data` holds the 4 bytes RRF_RT_REG_DWORD can return.
    let r = unsafe {
        RegGetValueW(
            root,
            &key,
            &value,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    (r == ERROR_SUCCESS).then_some(data)
}

/// A string (`REG_SZ` / `REG_EXPAND_SZ` expanded) registry value.
pub fn reg_string(root: HKEY, key: &str, value: &str) -> Option<String> {
    let (key, value) = (HSTRING::from(key), HSTRING::from(value));
    let mut size = 0u32;
    let mut kind = REG_VALUE_TYPE::default();
    // SAFETY: first call asks for the size only, the second fills a buffer
    // of that size.
    unsafe {
        let r = RegGetValueW(
            root,
            &key,
            &value,
            RRF_RT_REG_SZ,
            Some(&mut kind),
            None,
            Some(&mut size),
        );
        if r != ERROR_SUCCESS || size == 0 {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        let r = RegGetValueW(
            root,
            &key,
            &value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        );
        if r != ERROR_SUCCESS {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

/// The registry key exists.
pub fn reg_key_exists(root: HKEY, key: &str) -> bool {
    use windows::Win32::System::Registry::{KEY_READ, RegCloseKey, RegOpenKeyExW};
    let key = HSTRING::from(key);
    let mut h = HKEY::default();
    // SAFETY: the opened key is closed right away.
    unsafe {
        let ok = RegOpenKeyExW(root, &key, 0, KEY_READ, &mut h) == ERROR_SUCCESS;
        if ok {
            let _ = RegCloseKey(h);
        }
        ok
    }
}

/// Writes a `REG_SZ` value (creating the key).
pub fn reg_set_string(root: HKEY, key: &str, value: &str, data: &str) -> Result<(), String> {
    let (key, value) = (HSTRING::from(key), HSTRING::from(value));
    let wide: Vec<u16> = data.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 buffer of the given size.
    let r = unsafe {
        RegSetKeyValueW(
            root,
            &key,
            &value,
            REG_SZ.0,
            Some(wide.as_ptr() as *const _),
            (wide.len() * 2) as u32,
        )
    };
    if r == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(windows::core::Error::from(r.to_hresult()).message())
    }
}

/// Writes a `REG_DWORD` value (creating the key).
#[allow(dead_code)]
pub fn reg_set_dword(root: HKEY, key: &str, value: &str, data: u32) -> Result<(), String> {
    let (key, value) = (HSTRING::from(key), HSTRING::from(value));
    // SAFETY: 4 bytes of DWORD data.
    let r = unsafe {
        RegSetKeyValueW(
            root,
            &key,
            &value,
            REG_DWORD.0,
            Some(&data as *const u32 as *const _),
            4,
        )
    };
    if r == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(windows::core::Error::from(r.to_hresult()).message())
    }
}

/// Removes a value; a value that does not exist is not an error.
pub fn reg_delete_value(root: HKEY, key: &str, value: &str) -> Result<(), String> {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};
    let (key, value) = (HSTRING::from(key), HSTRING::from(value));
    // SAFETY: plain registry call with valid strings.
    let r = unsafe { RegDeleteKeyValueW(root, &key, &value) };
    if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND || r == ERROR_PATH_NOT_FOUND {
        Ok(())
    } else {
        Err(windows::core::Error::from(r.to_hresult()).message())
    }
}

/// NUL-terminated UTF-16 copy of `s`, truncated to fit `N` characters
/// (fixed-size fields of Win32 structures).
pub fn wide_fixed<const N: usize>(s: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (slot, c) in out.iter_mut().take(N - 1).zip(s.encode_utf16()) {
        *slot = c;
    }
    out
}

/// A `PCWSTR` for a constant identifier.
pub fn pcwstr(s: &HSTRING) -> PCWSTR {
    PCWSTR(s.as_ptr())
}

/// A command of the system (`schtasks`, …) without a console window.
pub fn hidden_command(program: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new(program);
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}

/// Windows version for the logs, the report and the diagnostics:
/// "Windows 11 Pro 23H2 (build 22631)".
pub fn os_name() -> String {
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let product = reg_string(HKEY_LOCAL_MACHINE, KEY, "ProductName").unwrap_or_default();
    let display = reg_string(HKEY_LOCAL_MACHINE, KEY, "DisplayVersion").unwrap_or_default();
    let build = reg_string(HKEY_LOCAL_MACHINE, KEY, "CurrentBuildNumber").unwrap_or_default();
    // ProductName still says "Windows 10" on Windows 11.
    let product = if build.parse::<u32>().unwrap_or(0) >= 22_000 {
        product.replacen("Windows 10", "Windows 11", 1)
    } else {
        product
    };
    let mut s = if product.is_empty() {
        "Windows".to_owned()
    } else {
        product
    };
    if !display.is_empty() {
        s.push(' ');
        s.push_str(&display);
    }
    if !build.is_empty() {
        s.push_str(&format!(" (build {build})"));
    }
    s
}

/// Interface fonts on Windows.
///
/// The text engine (cosmic-text) maps the generic "sans-serif" family to
/// "Open Sans", which Windows does not ship: every weight then falls back to
/// the first installed font of exactly that weight. For Medium (500) that
/// is Marlett, the window-buttons symbol font, so labels turned into ✓ ▾ ■
/// symbols. The generic families are pointed at the Windows fonts instead
/// (Segoe UI has no Medium: it uses the regular face, as Windows does).
pub fn configure_fonts() {
    let Ok(mut fonts) = iced::advanced::graphics::text::font_system().write() else {
        return;
    };
    let db = fonts.raw().db_mut();
    let installed = |name: &str| {
        db.faces()
            .any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)))
    };
    let pick = |candidates: &[&'static str]| candidates.iter().copied().find(|n| installed(n));
    let sans = pick(&["Segoe UI", "Arial", "Tahoma"]);
    let mono = pick(&["Cascadia Mono", "Consolas", "Courier New"]);
    let serif = pick(&["Times New Roman", "Georgia"]);
    if let Some(name) = sans {
        db.set_sans_serif_family(name);
    }
    if let Some(name) = mono {
        db.set_monospace_family(name);
    }
    if let Some(name) = serif {
        db.set_serif_family(name);
    }
    tracing::info!(?sans, ?mono, ?serif, "interface fonts");
}

/// `--help` / `--version` from a terminal: TuxRGB is a GUI program, its
/// output goes to the console that started it, if any.
pub fn attach_parent_console() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: no-op when there is no parent console.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}
