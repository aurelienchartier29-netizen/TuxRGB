//! "Launch at login" – XDG autostart entry, or the Background portal when
//! sandboxed (Flatpak).

use crate::paths::{APP_ID, PackageKind, package_kind};
use std::path::PathBuf;

fn autostart_file() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("autostart").join(format!("{APP_ID}.desktop"))
}

/// Command line that relaunches this exact build.
fn launch_command() -> String {
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return format!("\"{}\" --minimized", appimage.to_string_lossy());
    }
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "tuxrgb".into());
    format!("\"{exe}\" --minimized")
}

pub async fn set_enabled(enabled: bool) -> Result<(), String> {
    if package_kind() == PackageKind::Flatpak {
        let request = ashpd::desktop::background::Background::request()
            .reason("Démarrer TuxRGB à l'ouverture de session pour appliquer vos effets")
            .auto_start(enabled)
            .command(["tuxrgb", "--minimized"])
            .dbus_activatable(false);
        let response = request
            .send()
            .await
            .map_err(|e| e.to_string())?
            .response()
            .map_err(|e| e.to_string())?;
        if enabled && !response.auto_start() {
            return Err("le démarrage automatique a été refusé".into());
        }
        return Ok(());
    }
    let path = autostart_file();
    if enabled {
        let content = format!(
            "[Desktop Entry]\nType=Application\nName=TuxRGB\nComment=Éclairage RGB\nExec={}\nIcon={APP_ID}\nTerminal=false\nX-GNOME-Autostart-enabled=true\nX-KDE-autostart-after=panel\n",
            launch_command()
        );
        crate::paths::write_atomic(&path, &content).map_err(|e| e.to_string())
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}
