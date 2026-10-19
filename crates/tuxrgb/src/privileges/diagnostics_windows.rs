//! Unprivileged checks of what OpenRGB needs on Windows (beta build).
//!
//! * USB / HID devices: always reachable, nothing to configure;
//! * SMBus (motherboard, RAM): the PawnIO driver and administrator rights
//!   (TuxRGB's administrator mode, see `win::elevation`).

use crate::paths::{PackageKind, package_kind};
use crate::win::elevation;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// PawnIO + administrator rights: every device class can be used.
    Ready,
    /// USB/HID devices work, motherboard/RAM (SMBus) do not.
    Partial,
    NotConfigured,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostics {
    pub package: PackageKind,
    /// PawnIO driver installed (SMBus access).
    pub pawnio: bool,
    /// This TuxRGB (and so its OpenRGB) runs with administrator rights.
    pub elevated: bool,
    /// The administrator-mode scheduled task is configured.
    pub admin_mode: bool,
    /// "Windows 11 Pro 23H2 (build 22631)".
    pub os: String,
    pub openrgb_binary: Option<PathBuf>,
    pub openrgb_bundled: bool,
    pub openrgb_origin: &'static str,
}

impl Diagnostics {
    pub fn readiness(&self) -> Readiness {
        if self.openrgb_binary.is_none() {
            Readiness::NotConfigured
        } else if self.pawnio && self.elevated {
            Readiness::Ready
        } else {
            Readiness::Partial
        }
    }
}

pub fn run() -> Diagnostics {
    let located = tuxrgb_openrgb::locate::find_openrgb();
    Diagnostics {
        package: package_kind(),
        pawnio: elevation::pawnio_installed(),
        elevated: elevation::is_elevated(),
        admin_mode: elevation::admin_mode_configured(),
        os: crate::win::os_name(),
        openrgb_bundled: located
            .as_ref()
            .map(|l| l.origin == tuxrgb_openrgb::locate::Origin::Bundled)
            .unwrap_or(false),
        openrgb_origin: match located.as_ref().map(|l| &l.origin) {
            Some(tuxrgb_openrgb::locate::Origin::Bundled) => "installé avec TuxRGB",
            Some(tuxrgb_openrgb::locate::Origin::Override) => "remplacé via TUXRGB_OPENRGB",
            Some(tuxrgb_openrgb::locate::Origin::System) => "installation OpenRGB existante",
            None => "",
        },
        openrgb_binary: located.map(|l| l.path),
    }
}
