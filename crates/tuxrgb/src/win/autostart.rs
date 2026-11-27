//! "Launch at login" on Windows: the per-user `Run` key.
//!
//! In administrator mode the entry still starts the normal `tuxrgb.exe`,
//! which hands over to the elevated scheduled task without any prompt
//! (`win::elevation`): Windows never starts elevated `Run` entries itself.

use crate::win::{HKEY_CURRENT_USER, reg_delete_value, reg_set_string};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "TuxRGB";

/// Command line that relaunches this exact build, in the tray.
fn launch_command() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "tuxrgb.exe".into());
    format!("\"{exe}\" --minimized")
}

pub async fn set_enabled(enabled: bool) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        if enabled {
            reg_set_string(HKEY_CURRENT_USER, RUN_KEY, VALUE, &launch_command())
        } else {
            reg_delete_value(HKEY_CURRENT_USER, RUN_KEY, VALUE)
        }
    })
    .await
    .map_err(|e| e.to_string())?
}
