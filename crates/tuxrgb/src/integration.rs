//! Desktop integration of the AppImage (menu entry + icon).
//!
//! KDE shows a window in the task bar under the icon and name of the
//! `.desktop` file whose name matches the Wayland app id
//! (`io.github.tuxrgb.TuxRGB`). An AppImage started straight from the file
//! manager has none, so TuxRGB installs one in `~/.local/share` (no root):
//! the task bar shows the TuxRGB icon, the application menu gets an entry
//! that opens the maximised window. An entry made by `run-tuxrgb.sh
//! --install` is respected as long as the AppImage it points to exists.

use crate::paths::APP_ID;
use std::path::{Path, PathBuf};

/// Installed icon sizes (hicolor PNGs, same files as the packages).
const ICONS: &[(u32, &[u8])] = &[
    (
        32,
        include_bytes!("../../../data/icons/hicolor/32x32/apps/io.github.tuxrgb.TuxRGB.png"),
    ),
    (
        48,
        include_bytes!("../../../data/icons/hicolor/48x48/apps/io.github.tuxrgb.TuxRGB.png"),
    ),
    (
        64,
        include_bytes!("../../../data/icons/hicolor/64x64/apps/io.github.tuxrgb.TuxRGB.png"),
    ),
    (
        128,
        include_bytes!("../../../data/icons/hicolor/128x128/apps/io.github.tuxrgb.TuxRGB.png"),
    ),
    (
        256,
        include_bytes!("../../../data/icons/hicolor/256x256/apps/io.github.tuxrgb.TuxRGB.png"),
    ),
];

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
}

/// `Exec=` target of an existing entry, if any.
fn exec_target(desktop: &str) -> Option<PathBuf> {
    let line = desktop.lines().find(|l| l.starts_with("Exec="))?;
    let rest = line.trim_start_matches("Exec=").trim();
    let path = if let Some(q) = rest.strip_prefix('"') {
        q.split('"').next()?
    } else {
        rest.split_whitespace().next()?
    };
    Some(PathBuf::from(path))
}

fn entry(appimage: &Path) -> String {
    let exe = appimage.display();
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=TuxRGB\n\
         GenericName=Contrôle de l'éclairage RGB\n\
         Comment=Pilotez l'éclairage RGB de tout votre PC\n\
         Exec=\"{exe}\"\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=Settings;HardwareSettings;\n\
         Keywords=RGB;LED;OpenRGB;éclairage;\n\
         StartupWMClass={APP_ID}\n\
         SingleMainWindow=true\n\
         X-TuxRGB-Integration=auto\n\
         Actions=Minimized;ResetHardware;\n\
         \n\
         [Desktop Action Minimized]\n\
         Name=Démarrer dans la zone de notification\n\
         Exec=\"{exe}\" --minimized\n\
         \n\
         [Desktop Action ResetHardware]\n\
         Name=Réinitialiser l'accès matériel\n\
         Exec=\"{exe}\" --reset-hardware\n"
    )
}

/// Creates or repairs the menu entry and icon (blocking, call from a worker).
pub fn ensure() {
    let Some(appimage) = std::env::var_os("APPIMAGE").map(PathBuf::from) else {
        return; // Flatpak / native packages ship their own entry.
    };
    let Some(data) = data_home() else { return };
    let desktop = data.join("applications").join(format!("{APP_ID}.desktop"));
    let current = std::fs::read_to_string(&desktop).ok();
    let keep = current
        .as_deref()
        .and_then(exec_target)
        .is_some_and(|t| t.is_file());
    if !keep {
        match crate::paths::write_atomic(&desktop, &entry(&appimage)) {
            Ok(()) => tracing::info!("menu entry written: {}", desktop.display()),
            Err(e) => tracing::warn!("menu entry: {e}"),
        }
    }
    if install_icons(&data.join("icons/hicolor")) {
        refresh_icon_caches(&data.join("icons"));
    }
}

/// Writes every icon size whose content differs from the embedded one (a
/// new TuxRGB version brings its new icon). Returns whether anything changed.
fn install_icons(hicolor: &Path) -> bool {
    let mut changed = false;
    for (size, png) in ICONS {
        let path = hicolor
            .join(format!("{size}x{size}/apps"))
            .join(format!("{APP_ID}.png"));
        if std::fs::read(&path).is_ok_and(|cur| cur == *png) {
            continue;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match write_bytes_atomic(&path, png) {
            Ok(()) => changed = true,
            Err(e) => tracing::warn!("icon {}: {e}", path.display()),
        }
    }
    if changed {
        tracing::info!("application icon installed in {}", hicolor.display());
    }
    changed
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("png.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Icon caches keyed on folder dates (KDE's KIconLoader, GTK's
/// icon-theme.cache): touch the folders, rebuild a GTK cache if one exists.
fn refresh_icon_caches(icons: &Path) {
    let now = std::time::SystemTime::now();
    for dir in [icons.to_path_buf(), icons.join("hicolor")] {
        if let Ok(f) = std::fs::File::open(&dir) {
            let _ = f.set_modified(now);
        }
    }
    let hicolor = icons.join("hicolor");
    if hicolor.join("icon-theme.cache").exists() {
        let _ = std::process::Command::new("gtk-update-icon-cache")
            .args(["-q", "-t", "-f"])
            .arg(&hicolor)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_refreshed_only_when_they_differ() {
        let dir = std::env::temp_dir().join(format!("tuxrgb-icons-{}", std::process::id()));
        let hicolor = dir.join("hicolor");
        // An old icon from a previous version.
        let old = hicolor.join(format!("256x256/apps/{APP_ID}.png"));
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, b"old icon").unwrap();
        assert!(install_icons(&hicolor));
        assert_eq!(std::fs::read(&old).unwrap(), ICONS.last().unwrap().1);
        for (size, png) in ICONS {
            let p = hicolor.join(format!("{size}x{size}/apps/{APP_ID}.png"));
            assert_eq!(std::fs::read(p).unwrap(), *png);
        }
        // Up to date: nothing rewritten.
        assert!(!install_icons(&hicolor));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn exec_target_is_parsed() {
        let e = entry(Path::new("/home/a b/Applications/TuxRGB.AppImage"));
        assert_eq!(
            exec_target(&e),
            Some(PathBuf::from("/home/a b/Applications/TuxRGB.AppImage"))
        );
        assert_eq!(
            exec_target("Exec=/usr/bin/tuxrgb --x"),
            Some(PathBuf::from("/usr/bin/tuxrgb"))
        );
    }
}
