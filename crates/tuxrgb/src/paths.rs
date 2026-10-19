//! Locations and packaging environment.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

#[cfg_attr(windows, allow(dead_code))]
pub const APP_ID: &str = "io.github.tuxrgb.TuxRGB";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(windows, allow(dead_code))]
pub enum PackageKind {
    AppImage,
    Flatpak,
    /// Installed by a distribution package (`/usr`).
    Native,
    /// Running from a source checkout (`cargo run`).
    Development,
    /// Windows beta build installed by its setup program.
    #[cfg(windows)]
    WindowsInstaller,
}

impl PackageKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::AppImage => "AppImage",
            Self::Flatpak => "Flatpak",
            Self::Native => "Paquet natif",
            Self::Development => "Développement",
            #[cfg(windows)]
            Self::WindowsInstaller => "Installateur Windows (bêta)",
        }
    }
}

#[cfg(windows)]
pub fn package_kind() -> PackageKind {
    static KIND: OnceLock<PackageKind> = OnceLock::new();
    *KIND.get_or_init(|| {
        // The installer puts its uninstaller next to tuxrgb.exe.
        let installed = std::env::current_exe()
            .ok()
            .and_then(|p| Some(p.parent()?.join("uninstall.exe").exists()))
            .unwrap_or(false);
        if installed {
            PackageKind::WindowsInstaller
        } else {
            PackageKind::Development
        }
    })
}

#[cfg(not(windows))]
pub fn package_kind() -> PackageKind {
    static KIND: OnceLock<PackageKind> = OnceLock::new();
    *KIND.get_or_init(|| {
        if std::env::var_os("APPIMAGE").is_some() {
            PackageKind::AppImage
        } else if Path::new("/.flatpak-info").exists() {
            PackageKind::Flatpak
        } else if std::env::current_exe()
            .map(|p| p.starts_with("/usr") || p.starts_with("/opt"))
            .unwrap_or(false)
        {
            PackageKind::Native
        } else {
            PackageKind::Development
        }
    })
}

pub struct Paths {
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    /// TOML configuration of TuxRGB ≤ 1.1, migrated to JSON on first start.
    pub legacy_config_file: PathBuf,
    pub onboarding_file: PathBuf,
    pub openrgb_config_dir: PathBuf,
    pub openrgb_log: PathBuf,
    /// TuxRGB's own log (previous run kept as `tuxrgb.log.old`).
    pub app_log: PathBuf,
    /// Present while a window is being created with the GPU renderer: if
    /// TuxRGB dies there (driver crash), the next start uses the software
    /// renderer automatically.
    pub gpu_marker: PathBuf,
}

pub fn paths() -> &'static Paths {
    static PATHS: OnceLock<Paths> = OnceLock::new();
    PATHS.get_or_init(|| {
        let base = directories::BaseDirs::new();
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let config_root = base
            .as_ref()
            .map(|b| b.config_dir().to_path_buf())
            .unwrap_or_else(|| home.join(".config"));
        // Windows has no "state" directory: logs go to %LOCALAPPDATA%.
        #[cfg(windows)]
        let state_root = base
            .as_ref()
            .map(|b| b.data_local_dir().to_path_buf())
            .unwrap_or_else(std::env::temp_dir);
        #[cfg(not(windows))]
        let state_root = base
            .as_ref()
            .and_then(|b| b.state_dir().map(Path::to_path_buf))
            .unwrap_or_else(|| home.join(".local/state"));
        let config_dir = config_root.join("tuxrgb");
        let state_dir = state_root.join("tuxrgb");
        Paths {
            config_file: config_dir.join("config.json"),
            legacy_config_file: config_dir.join("config.toml"),
            onboarding_file: config_dir.join("onboarding.toml"),
            openrgb_config_dir: config_dir.join("openrgb"),
            openrgb_log: state_dir.join("openrgb-server.log"),
            app_log: state_dir.join("tuxrgb.log"),
            gpu_marker: state_dir.join("gpu-window.marker"),
            config_dir,
        }
    })
}

/// Writes through a temporary file, flushed to disk, then renames: after a
/// crash or a power cut the file is either the old or the new version,
/// never empty or torn.
pub fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    // The rename itself reaches the disk with the directory (best effort).
    if let Some(dir) = path.parent()
        && let Ok(d) = std::fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

/// A file write handed to the [`writer`] thread.
struct WriteJob {
    path: PathBuf,
    content: String,
    done: Option<std::sync::mpsc::Sender<std::io::Result<()>>>,
}

/// One thread writes the files queued by [`write_later`] and [`write_now`],
/// strictly in order: the interface never waits for the disk (the flush to
/// disk can take a while on a busy or slow drive), and an older version can
/// never overwrite a newer one.
fn writer() -> Option<std::sync::mpsc::Sender<WriteJob>> {
    use std::sync::Mutex;
    static WRITER: OnceLock<Option<Mutex<std::sync::mpsc::Sender<WriteJob>>>> = OnceLock::new();
    WRITER
        .get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel::<WriteJob>();
            std::thread::Builder::new()
                .name("tuxrgb-writer".into())
                .spawn(move || {
                    for job in rx {
                        let result = write_atomic(&job.path, &job.content);
                        if let Err(e) = &result {
                            tracing::warn!("cannot write {}: {e}", job.path.display());
                        }
                        if let Some(done) = job.done {
                            let _ = done.send(result);
                        }
                    }
                })
                .ok()
                .map(|_| Mutex::new(tx))
        })
        .as_ref()
        .and_then(|m| m.lock().ok().map(|tx| tx.clone()))
}

/// Queues `content` for `path` and returns at once.
pub fn write_later(path: PathBuf, content: String) {
    match writer() {
        Some(tx) => {
            if let Err(e) = tx.send(WriteJob {
                path,
                content,
                done: None,
            }) {
                let job = e.0;
                let _ = write_atomic(&job.path, &job.content);
            }
        }
        None => {
            let _ = write_atomic(&path, &content);
        }
    }
}

/// Writes `content` to `path` after everything queued before it, and waits
/// for the result (at most `timeout`): quitting, shutdown.
pub fn write_now(path: PathBuf, content: String, timeout: Duration) -> std::io::Result<()> {
    let Some(tx) = writer() else {
        return write_atomic(&path, &content);
    };
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    if let Err(e) = tx.send(WriteJob {
        path,
        content,
        done: Some(done_tx),
    }) {
        let job = e.0;
        return write_atomic(&job.path, &job.content);
    }
    done_rx
        .recv_timeout(timeout)
        .unwrap_or_else(|_| Err(std::io::Error::other("disk too slow, write still pending")))
}

/// Returns the allocator's free memory to the system (glibc keeps freed
/// blocks for reuse, so a closed window's renderer would otherwise stay in
/// the resident memory of a process living in the tray).
pub fn trim_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: malloc_trim only releases unused heap pages.
        let released = unsafe { libc::malloc_trim(0) };
        tracing::debug!(released, "memory trimmed");
    }
}

/// Opens a URL with the desktop's handler (the default browser on Windows).
#[cfg(windows)]
pub fn open_url(url: &str) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, w};
    let url = HSTRING::from(url);
    // SAFETY: strings valid for the call.
    unsafe {
        ShellExecuteW(None, w!("open"), &url, None, None, SW_SHOWNORMAL);
    }
}

/// Opens a URL with the desktop's handler (portal-aware inside Flatpak).
#[cfg(not(windows))]
pub fn open_url(url: &str) {
    let _ = std::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_writes_land_in_order_and_flush_on_demand() {
        let dir = std::env::temp_dir().join(format!("tuxrgb-writer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("config.json");
        for i in 0..50 {
            write_later(file.clone(), format!("version {i}"));
        }
        write_now(file.clone(), "final".into(), Duration::from_secs(5)).expect("written");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "final");
        assert!(!file.with_extension("tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
