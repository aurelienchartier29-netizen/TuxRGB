//! Is a game running? (TuxRGB 4.0.1, "mode jeu")
//!
//! Controllers on an I2C / SMBus bus (RGB RAM, SMBus motherboards, graphics
//! cards lit through their I2C bus) are slow to write: each frame of an
//! animation is many small bus transactions, on a bus the memory's SPD
//! chips, the firmware and the graphics driver also use (on Windows the
//! driver even waits for each one with the processor). While a game runs,
//! the bridge leaves them on their colour and writes the other devices at
//! most 30 times a second (see `bridge`): nothing of TuxRGB stands between
//! the game and its frames.
//!
//! No extra polling: the bridge asks at its usual check (every 5 seconds),
//! where it already lists the processes for [`crate::conflicts`].
//!
//! * Linux: a Steam game (Steam's `reaper … SteamLaunch`, native or Proton),
//!   a Wine / Proton game outside Steam (Lutris, Heroic, Bottles:
//!   `wineserver`), or a game in gamescope.
//! * Windows: an application in full screen (exclusive or borderless), as
//!   the system itself sees it to hold its notifications.

use std::sync::atomic::{AtomicU8, Ordering};

/// Answer forced by the tests (0: none, 1: no game, 2: a game).
static FORCED: AtomicU8 = AtomicU8::new(0);

/// Tests only: makes [`playing`] answer `game` (`None`: look again).
#[doc(hidden)]
pub fn force_for_tests(game: Option<bool>) {
    FORCED.store(
        match game {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        },
        Ordering::Relaxed,
    );
}

/// A game runs now.
pub fn playing() -> bool {
    match FORCED.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => detect(),
    }
}

/// The name of a process that is (part of) a game. `cmdline` is read only
/// when the name needs it.
#[cfg_attr(windows, allow(dead_code))]
fn is_game_process(name: &str, cmdline: impl FnOnce() -> Option<String>) -> bool {
    match name {
        "wineserver" => true,
        n if n.starts_with("gamescope") => true,
        // Steam starts every game under `reaper` (and only games).
        "reaper" => cmdline().is_some_and(|c| c.contains("SteamLaunch")),
        _ => false,
    }
}

#[cfg(not(windows))]
fn detect() -> bool {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return false;
    };
    dir.flatten().any(|e| {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            return false;
        };
        let Ok(comm) = std::fs::read_to_string(e.path().join("comm")) else {
            return false;
        };
        is_game_process(comm.trim_end(), || {
            std::fs::read(format!("/proc/{pid}/cmdline"))
                .ok()
                .map(|b| String::from_utf8_lossy(&b).replace('\0', " "))
        })
    })
}

/// An application is in full screen.
#[cfg(windows)]
fn detect() -> bool {
    use windows::Win32::UI::Shell::{
        QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
    };
    // SAFETY: a plain query of the shell's state, no pointers kept.
    match unsafe { SHQueryUserNotificationState() } {
        Ok(state) => state == QUNS_BUSY || state == QUNS_RUNNING_D3D_FULL_SCREEN,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn games_are_recognised_by_their_processes() {
        let steam = || Some("/home/a/.steam/ubuntu12_32/reaper SteamLaunch AppId=570 -- …".into());
        assert!(is_game_process("reaper", steam));
        assert!(is_game_process("wineserver", || None));
        assert!(is_game_process("gamescope-wl", || None));
        // Not games: another reaper, Steam itself, a browser.
        assert!(!is_game_process("reaper", || Some("reaper --other".into())));
        assert!(!is_game_process("steam", || None));
        assert!(!is_game_process("firefox", || None));
        // The command line is read only for `reaper`.
        assert!(!is_game_process("kwin_wayland", || panic!("not read")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn no_game_in_the_test_run() {
        // Cheap enough for the 5-second check: one pass over /proc.
        let t = std::time::Instant::now();
        let _ = detect();
        assert!(t.elapsed() < std::time::Duration::from_millis(500));
    }
}
