//! Lifecycle of the private, headless OpenRGB server.

use std::fs::File;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct EmbeddedServer {
    child: Child,
    pub port: u16,
    pub binary: PathBuf,
    pub log_path: PathBuf,
    pub started_at: Instant,
}

impl EmbeddedServer {
    /// Starts `openrgb --server` bound to loopback only.
    ///
    /// `config_dir` is TuxRGB's own OpenRGB configuration directory so a
    /// separately installed OpenRGB keeps its settings untouched.
    pub fn spawn(binary: &Path, port: u16, config_dir: &Path, log_path: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(config_dir)?;
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let log = File::create(log_path)?;
        let log_err = log.try_clone()?;

        let mut cmd = Command::new(binary);
        cmd.arg("--server")
            .arg("--server-host")
            .arg("127.0.0.1")
            .arg("--server-port")
            .arg(port.to_string())
            .arg("--noautoconnect")
            .arg("--config")
            .arg(config_dir)
            // "info" lists every detected bus and controller: the log is
            // part of TuxRGB's hardware report (Réglages → Accès matériel).
            .arg("--loglevel")
            .arg("info")
            .arg("--verbose")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        // The server never needs a display.
        #[cfg(unix)]
        cmd.env_remove("DISPLAY")
            .env("QT_QPA_PLATFORM", "offscreen");
        // Nothing from the environment decides what code the server loads
        // (with administrator rights on Windows, the user's other programs
        // could otherwise point Qt to plugins of theirs), and it runs from
        // its own folder.
        for var in [
            "QT_PLUGIN_PATH",
            "QT_QPA_PLATFORM_PLUGIN_PATH",
            "QT_QPA_PLATFORMTHEME",
            "QML_IMPORT_PATH",
            "QML2_IMPORT_PATH",
            "QT_CONF_PATH",
        ] {
            cmd.env_remove(var);
        }
        if let Some(dir) = binary.parent().filter(|d| !d.as_os_str().is_empty()) {
            cmd.current_dir(dir);
        }
        #[cfg(target_os = "linux")]
        if let Some(path) = fallback_library_path(binary) {
            cmd.env("LD_LIBRARY_PATH", path);
        }
        // * If TuxRGB dies (even with SIGKILL) the kernel stops the server
        //   too.
        // * SIGTERM / SIGHUP are blocked in the server: at logout or
        //   shutdown, systemd signals every process of the session at once,
        //   and the server must outlive TuxRGB long enough to receive the
        //   "all LEDs off" frame. TuxRGB stops it itself afterwards (SIGINT,
        //   which OpenRGB handles like SIGTERM), or the kernel does when
        //   TuxRGB exits.
        #[cfg(unix)]
        unsafe {
            cmd.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGTERM);
                libc::sigaddset(&mut set, libc::SIGHUP);
                if libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        // Windows: no console window, and the server is tied
        // to TuxRGB by a job object (closed – server killed – when TuxRGB
        // exits, even if it crashes).
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let child = cmd.spawn()?;
        #[cfg(windows)]
        windows_job::adopt(&child);
        tracing::info!(pid = child.id(), port, binary = %binary.display(), "embedded OpenRGB server started");
        Ok(Self {
            child,
            port,
            binary: binary.to_owned(),
            log_path: log_path.to_owned(),
            started_at: Instant::now(),
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// `Ok(None)` while running, `Ok(Some(code))` once exited.
    pub fn exit_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    pub fn is_running(&mut self) -> bool {
        self.exit_status().is_none()
    }

    /// Last lines of the server log, for error reporting in the GUI.
    pub fn log_tail(&self, lines: usize) -> String {
        std::fs::read_to_string(&self.log_path)
            .map(|s| {
                let v: Vec<&str> = s.lines().collect();
                v[v.len().saturating_sub(lines)..].join("\n")
            })
            .unwrap_or_default()
    }

    /// Graceful stop: SIGINT (SIGTERM is blocked, see [`Self::spawn`]),
    /// then SIGKILL after `grace`. On Windows the server has no console to
    /// receive Ctrl+C: it is ended right away (TuxRGB already sent the last
    /// frame).
    pub fn stop(&mut self, grace: Duration) {
        if !self.is_running() {
            return;
        }
        #[cfg(unix)]
        {
            unsafe {
                libc::kill(self.child.id() as libc::pid_t, libc::SIGINT);
            }
            let deadline = Instant::now() + grace;
            while Instant::now() < deadline {
                if !self.is_running() {
                    tracing::info!("embedded OpenRGB server stopped");
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            tracing::warn!("OpenRGB server did not stop in time, killing it");
        }
        #[cfg(windows)]
        {
            let _ = grace;
            tracing::info!("embedded OpenRGB server stopped");
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for EmbeddedServer {
    fn drop(&mut self) {
        self.stop(Duration::from_secs(3));
    }
}

/// One job object for every server TuxRGB starts, killed with TuxRGB.
#[cfg(windows)]
mod windows_job {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    /// The job handle, kept open for the life of the process (as an
    /// integer: raw handles are not `Send`).
    fn job() -> Option<HANDLE> {
        static JOB: OnceLock<Option<isize>> = OnceLock::new();
        JOB.get_or_init(|| {
            // SAFETY: plain job object creation and configuration.
            unsafe {
                let job = CreateJobObjectW(None, windows::core::PCWSTR::null()).ok()?;
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
                .ok()?;
                Some(job.0 as isize)
            }
        })
        .map(|h| HANDLE(h as *mut core::ffi::c_void))
    }

    pub fn adopt(child: &std::process::Child) {
        let Some(job) = job() else { return };
        let process = HANDLE(child.as_raw_handle());
        // SAFETY: both handles are valid for the duration of the call.
        if let Err(e) = unsafe { AssignProcessToJobObject(job, process) } {
            tracing::warn!("OpenRGB server not tied to TuxRGB: {e}");
        }
    }
}

/// `true` if something accepts TCP connections on `127.0.0.1:port`.
pub fn port_is_open(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok()
}

/// Returns `preferred` if it can be bound on loopback, else one of the next
/// 16 ports, else any free port.
pub fn pick_port(preferred: u16) -> u16 {
    for p in preferred..preferred.saturating_add(16) {
        if TcpListener::bind((Ipv4Addr::LOCALHOST, p)).is_ok() {
            return p;
        }
    }
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(preferred)
}

/// Libraries the bundled OpenRGB takes from the host (front ends of the
/// graphics drivers, GCC's OpenMP runtime) that a desktop may lack: Ubuntu
/// 26.10 no longer installs `libopengl0`, for instance. The AppImage carries
/// a copy of each in `fallback/<name>/` next to `openrgb`, used only when the
/// host has none: the host's own copy, matched to its drivers, always wins.
#[cfg(target_os = "linux")]
pub const FALLBACK_LIBS: &[&str] = &[
    "libOpenGL.so.0",
    "libGLX.so.0",
    "libEGL.so.1",
    "libGLdispatch.so.0",
    "libgomp.so.1",
];

/// `LD_LIBRARY_PATH` for the server: the fallback copies of the libraries
/// the host is missing (see [`FALLBACK_LIBS`]), then the inherited value.
#[cfg(target_os = "linux")]
fn fallback_library_path(binary: &Path) -> Option<std::ffi::OsString> {
    let root = binary.parent()?.join("fallback");
    if !root.is_dir() {
        return None;
    }
    let mut dirs: Vec<PathBuf> = FALLBACK_LIBS
        .iter()
        .filter(|name| root.join(name).join(name).is_file() && !host_has_library(name))
        .map(|name| root.join(name))
        .collect();
    if dirs.is_empty() {
        return None;
    }
    tracing::info!(
        ?dirs,
        "libraries missing on this system: OpenRGB uses the AppImage's copies"
    );
    if let Some(old) = std::env::var_os("LD_LIBRARY_PATH") {
        dirs.extend(std::env::split_paths(&old));
    }
    std::env::join_paths(dirs).ok()
}

/// The dynamic loader finds `name` on this system (same search as for
/// OpenRGB: the host's library paths).
#[cfg(target_os = "linux")]
pub fn host_has_library(name: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(name) else {
        return false;
    };
    // SAFETY: a library opened and closed again; its constructors run, as
    // they would in OpenRGB (graphics front ends and libgomp only).
    unsafe {
        let handle = libc::dlopen(c.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL);
        if handle.is_null() {
            return false;
        }
        libc::dlclose(handle);
    }
    true
}

/// The library the dynamic loader could not find, from the server's log
/// (`error while loading shared libraries: libX.so.1: cannot open…`).
pub fn missing_library(log: &str) -> Option<String> {
    const MARK: &str = "error while loading shared libraries: ";
    let rest = &log[log.find(MARK)? + MARK.len()..];
    let name = rest.split(':').next()?.trim();
    (!name.is_empty() && !name.contains(char::is_whitespace)).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_library_is_named() {
        let log = "/tmp/.mount_TuxRGB/usr/lib/tuxrgb/openrgb/openrgb: error while loading shared \
                   libraries: libOpenGL.so.0: cannot open shared object file: No such file or directory\n";
        assert_eq!(missing_library(log).as_deref(), Some("libOpenGL.so.0"));
        assert_eq!(missing_library("[Info] Detecting I2C devices"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_host_libraries_are_found() {
        assert!(host_has_library("libc.so.6"));
        assert!(!host_has_library("libTuxRGBDoesNotExist.so.9"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fallback_copies_only_for_what_is_missing() {
        let root = std::env::temp_dir().join(format!("tuxrgb-fallback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // A library the host has, and one it lacks, both "shipped".
        for name in ["libc.so.6", "libOpenGL.so.0"] {
            std::fs::create_dir_all(root.join("fallback").join(name)).unwrap();
            std::fs::write(root.join("fallback").join(name).join(name), b"").unwrap();
        }
        let path = fallback_library_path(&root.join("openrgb"));
        if host_has_library("libOpenGL.so.0") {
            assert!(path.is_none() || !path.unwrap().to_string_lossy().contains("libOpenGL"));
        } else {
            let path = path.unwrap().to_string_lossy().into_owned();
            assert!(path.contains("fallback/libOpenGL.so.0"), "{path}");
        }
        // libc.so.6 is not in the list, and present anyway.
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pick_port_returns_bindable_port() {
        let busy = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let busy_port = busy.local_addr().unwrap().port();
        let p = pick_port(busy_port);
        assert_ne!(p, busy_port);
        assert!(port_is_open(busy_port));
    }
}
