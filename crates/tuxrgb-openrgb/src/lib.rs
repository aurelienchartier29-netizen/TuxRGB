//! Hardware backend.
//!
//! TuxRGB ships OpenRGB (1.0 + the current development tree, C++/Qt6, GPL-2.0) inside its package and runs it
//! as a private, headless SDK server bound to `127.0.0.1` on a dedicated
//! port. This gives TuxRGB every device OpenRGB supports (motherboards, GPUs,
//! RAM over SMBus, USB/HID peripherals, AIOs…) and inherits OpenRGB's
//! hardware-safety work, while the whole application stays in Rust.
//!
//! * [`locate`] finds the bundled `openrgb` binary (AppImage, Flatpak, native).
//! * [`server`] spawns / supervises / stops that private server.
//! * [`bridge`] is the async SDK client loop that streams engine frames.
//! * [`rules`]  generates the udev rules from the bundled binary.
//! * [`network`] network lights (WLED, Philips Hue and WiZ, Govee, LIFX,
//!   Nanoleaf, Yeelight, Elgato, TP-Link Kasa), written into the private
//!   server's settings.
//! * [`detectors`] the RAM / SMBus motherboard detectors, off unless the
//!   user turns them on.

pub mod bridge;
pub mod detectors;
pub mod locate;
pub mod network;
pub mod rules;
pub mod server;

pub use bridge::{BridgeCommand, BridgeConfig, BridgeEvent, BridgeStatus, ServerMode, ZoneResize};
pub use network::{Brand, NetLight, NetworkLights, WizLight, WledLight};

/// Port used by the private embedded server (6742 is OpenRGB's default and
/// is left free for a separately installed OpenRGB).
pub const EMBEDDED_PORT: u16 = 6743;
/// Default port of a standalone OpenRGB server.
pub const OPENRGB_DEFAULT_PORT: u16 = 6742;

/// Bundled OpenRGB build (the latest release, with TuxRGB's patch on
/// Linux, see `third_party/patches`). Kept in sync with
/// `third_party/README.md`. Windows uses the official build.
#[cfg(not(windows))]
pub const OPENRGB_VERSION: &str = "1.1 (release_1.1 8e16de2, 16/10/2026, + correctif TuxRGB)";
#[cfg(windows)]
pub const OPENRGB_VERSION: &str = "1.1 (release_1.1 8e16de2, 16/10/2026)";
/// Short label used in the interface.
pub const OPENRGB_LABEL: &str = "OpenRGB 1.1";
