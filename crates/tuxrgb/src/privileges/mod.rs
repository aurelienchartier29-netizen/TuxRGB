//! Hardware access: diagnostics and the Polkit-elevated helper.
//!
//! Flow (Mission Center style):
//! 1. [`diagnostics::run`] checks udev rules, I2C/SMBus modules and device
//!    node permissions without any privilege.
//! 2. The onboarding wizard (or Settings → "Réinitialiser les permissions")
//!    calls [`runner::run_helper`], which starts `tuxrgb-helper` through
//!    `pkexec` (or `flatpak-spawn --host pkexec` inside Flatpak). Polkit's
//!    own dialog asks for the administrator password.

pub mod diagnostics;
pub mod report;
pub mod runner;

pub use diagnostics::{Diagnostics, Readiness};
pub use runner::{HelperError, HelperReport};
