//! Lifecycle of the private, headless OpenRGB server.

use std::fs::File;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
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
            .stderr(Stdio::from(log_err))
            // The server never needs a display.
            .env_remove("DISPLAY")
            .env("QT_QPA_PLATFORM", "offscreen");
        // * If TuxRGB dies (even with SIGKILL) the kernel stops the server
        //   too.
        // * SIGTERM / SIGHUP are blocked in the server: at logout or
        //   shutdown, systemd signals every process of the session at once,
        //   and the server must outlive TuxRGB long enough to receive the
        //   "all LEDs off" frame. TuxRGB stops it itself afterwards (SIGINT,
        //   which OpenRGB handles like SIGTERM), or the kernel does when
        //   TuxRGB exits.
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
        let child = cmd.spawn()?;
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
    /// then SIGKILL after `grace`.
    pub fn stop(&mut self, grace: Duration) {
        if !self.is_running() {
            return;
        }
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
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for EmbeddedServer {
    fn drop(&mut self) {
        self.stop(Duration::from_secs(3));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_port_returns_bindable_port() {
        let busy = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let busy_port = busy.local_addr().unwrap().port();
        let p = pick_port(busy_port);
        assert_ne!(p, busy_port);
        assert!(port_is_open(busy_port));
    }
}
