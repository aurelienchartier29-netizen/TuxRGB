//! Hardware access: diagnostics and the Polkit-elevated helper.
//!
//! Flow (Mission Center style):
//! 1. [`diagnostics::run`] checks udev rules, I2C/SMBus modules and device
//!    node permissions without any privilege.
//! 2. The onboarding wizard (or Settings → "Réinitialiser les permissions")
//!    calls [`runner::run_helper`], which starts `tuxrgb-helper` through
//!    `pkexec` (or `flatpak-spawn --host pkexec` inside Flatpak). Polkit's
//!    own dialog asks for the administrator password.
//!
//! Windows build: the same interface, other files – PawnIO and
//! administrator rights instead of udev / I2C, the UAC prompt instead of
//! Polkit (see `win::elevation`).

#[cfg_attr(windows, path = "diagnostics_windows.rs")]
pub mod diagnostics;
#[cfg_attr(windows, path = "report_windows.rs")]
pub mod report;
#[cfg_attr(windows, path = "runner_windows.rs")]
pub mod runner;

pub use diagnostics::{Diagnostics, Readiness};
pub use runner::{HelperError, HelperReport};
