//! Finds the OpenRGB binary that ships with TuxRGB.

use std::path::{Path, PathBuf};

/// Relative location of the bundled server next to the TuxRGB binary
/// (`<prefix>/bin/tuxrgb` → `<prefix>/lib/tuxrgb/openrgb/openrgb`).
pub const BUNDLED_RELATIVE: &str = "../lib/tuxrgb/openrgb/openrgb";

/// Where the binary was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// `TUXRGB_OPENRGB` environment variable.
    Override,
    /// Shipped inside the AppImage / Flatpak / package (the normal case).
    Bundled,
    /// A separately installed system OpenRGB found in `PATH`.
    System,
}

#[derive(Clone, Debug)]
pub struct Located {
    pub path: PathBuf,
    pub origin: Origin,
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(windows)]
fn is_executable(p: &Path) -> bool {
    p.is_file()
        && p.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
}

/// Candidate paths in priority order (Windows beta build): the copy the
/// TuxRGB installer puts next to `tuxrgb.exe`, then an official OpenRGB
/// installation.
#[cfg(windows)]
pub fn candidates() -> Vec<(PathBuf, Origin)> {
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os("TUXRGB_OPENRGB") {
        out.push((PathBuf::from(p), Origin::Override));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        out.push((dir.join("openrgb").join("OpenRGB.exe"), Origin::Bundled));
    }
    for var in ["ProgramFiles", "ProgramW6432", "LOCALAPPDATA"] {
        if let Some(root) = std::env::var_os(var) {
            let root = PathBuf::from(root);
            out.push((root.join("OpenRGB").join("OpenRGB.exe"), Origin::System));
            out.push((
                root.join("Programs").join("OpenRGB").join("OpenRGB.exe"),
                Origin::System,
            ));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            out.push((dir.join("OpenRGB.exe"), Origin::System));
        }
    }
    out
}

/// Candidate paths in priority order.
#[cfg(not(windows))]
pub fn candidates() -> Vec<(PathBuf, Origin)> {
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os("TUXRGB_OPENRGB") {
        out.push((PathBuf::from(p), Origin::Override));
    }
    // AppImage: $APPDIR is the mounted image root.
    if let Some(appdir) = std::env::var_os("APPDIR") {
        out.push((
            Path::new(&appdir).join("usr/lib/tuxrgb/openrgb/openrgb"),
            Origin::Bundled,
        ));
    }
    // Relative to the running executable (AppImage, Flatpak /app, /usr, tarball).
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        out.push((dir.join(BUNDLED_RELATIVE), Origin::Bundled));
        out.push((dir.join("openrgb/openrgb"), Origin::Bundled));
    }
    for p in [
        "/app/lib/tuxrgb/openrgb/openrgb",
        "/usr/lib/tuxrgb/openrgb/openrgb",
        "/usr/lib64/tuxrgb/openrgb/openrgb",
        "/usr/libexec/tuxrgb/openrgb",
    ] {
        out.push((PathBuf::from(p), Origin::Bundled));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            out.push((dir.join("openrgb"), Origin::System));
        }
    }
    out
}

/// Returns the first usable OpenRGB binary.
pub fn find_openrgb() -> Option<Located> {
    candidates().into_iter().find_map(|(path, origin)| {
        // On Windows `canonicalize` gives `\\?\` paths: kept as found.
        #[cfg(not(windows))]
        let path = path.canonicalize().unwrap_or(path);
        is_executable(&path).then_some(Located { path, origin })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_absolute_or_override() {
        for (p, origin) in candidates() {
            if origin != Origin::Override && !cfg!(windows) {
                assert!(p.is_absolute() || p.starts_with(".."), "{p:?}");
            }
        }
    }
}
