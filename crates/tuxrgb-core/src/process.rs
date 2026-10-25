//! Running short helper programs (`nvidia-smi`, `journalctl`…) without ever
//! waiting on them forever: a tool that hangs (driver trouble, huge journal)
//! is killed after a deadline instead of freezing the thread that asked.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a command printed, if it exited successfully within `timeout`.
///
/// The standard output is read on a helper thread while the command runs,
/// so a large output can never fill the pipe and block the program (and
/// is capped at `max_bytes`). Standard input and error are closed.
pub fn output_with_timeout(
    mut cmd: Command,
    timeout: Duration,
    max_bytes: usize,
) -> Option<Vec<u8>> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let reader = std::thread::Builder::new()
        .name("tuxrgb-cmd-out".into())
        .spawn(move || {
            let mut stdout = stdout;
            let mut out = Vec::new();
            let mut buf = [0u8; 8192];
            // Past the cap the rest is read and dropped, so the command
            // never blocks on a full pipe.
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let room = max_bytes.saturating_sub(out.len());
                out.extend_from_slice(&buf[..n.min(room)]);
            }
            out
        })
        .ok();
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    match status {
        // Exited: the pipe closes (unless a grandchild kept it, which the
        // reader then outlives on its own).
        Some(s) if s.success() => reader.and_then(|r| r.join().ok()),
        // Killed or failed: the reader is left to finish by itself; a
        // process started by the command may still hold the pipe open.
        _ => None,
    }
}

// The tests drive `sh`: Unix only.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn returns_the_output_of_a_quick_command() {
        let mut c = Command::new("sh");
        c.args(["-c", "echo hello"]);
        let out = output_with_timeout(c, Duration::from_secs(5), 1024).expect("output");
        assert_eq!(out, b"hello\n");
    }

    #[test]
    fn kills_a_command_that_hangs() {
        let mut c = Command::new("sh");
        c.args(["-c", "sleep 30"]);
        let t = Instant::now();
        assert!(output_with_timeout(c, Duration::from_millis(200), 1024).is_none());
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn large_output_never_blocks_and_is_capped() {
        let mut c = Command::new("sh");
        c.args(["-c", "head -c 300000 /dev/zero"]);
        let out = output_with_timeout(c, Duration::from_secs(10), 100_000).expect("output");
        assert_eq!(out.len(), 100_000);
    }

    #[test]
    fn failing_or_missing_commands_give_nothing() {
        let mut c = Command::new("sh");
        c.args(["-c", "exit 3"]);
        assert!(output_with_timeout(c, Duration::from_secs(5), 64).is_none());
        let c = Command::new("/nonexistent/tuxrgb-test-binary");
        assert!(output_with_timeout(c, Duration::from_secs(5), 64).is_none());
    }
}
