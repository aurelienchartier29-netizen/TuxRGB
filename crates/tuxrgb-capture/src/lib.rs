//! Reactive inputs for TuxRGB, all native Wayland / PipeWire:
//!
//! * [`audio`]    – captures the default output's monitor through PipeWire
//!   (what you hear), no PulseAudio emulation and no X11.
//! * [`analyzer`] – real FFT (realfft), Hann window, log-spaced bands, AGC
//!   and beat detection.
//! * [`screen`]   – asks the xdg-desktop-portal ScreenCast interface for a
//!   monitor stream (KDE/GNOME dialog), then reads it with PipeWire and keeps
//!   a tiny downscaled frame for the Screen Sync effect.
//! * [`hardware`] – CPU / GPU / memory sensors (Hardware Sync).
//! * [`shader`]   – GLSL shaders rendered off-screen by the GPU (wgpu).
//!
//! Capture threads only run while an effect needs them.
//!
//! The Windows build swaps the platform parts behind the same API:
//! WASAPI loopback for [`audio`], GDI for [`screen`], Windows counters for
//! [`hardware`]; the analysis, the sampling and the shaders are shared.

pub mod analyzer;
#[cfg_attr(windows, path = "audio_windows.rs")]
pub mod audio;
pub mod hardware;
#[cfg(windows)]
pub mod pawnio;
mod sampling;
#[cfg_attr(windows, path = "screen_windows.rs")]
pub mod screen;
pub mod shader;
#[cfg(windows)]
pub mod superio;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// Audio capture (PipeWire on Linux, WASAPI on Windows).
    #[cfg_attr(not(windows), error("PipeWire : {0}"))]
    #[cfg_attr(windows, error("WASAPI : {0}"))]
    PipeWire(String),
    /// Screen capture (ScreenCast portal on Linux, GDI on Windows).
    #[cfg_attr(not(windows), error("portail ScreenCast : {0}"))]
    #[cfg_attr(windows, error("capture d'écran : {0}"))]
    Portal(String),
    #[error("capture annulée par l'utilisateur")]
    Cancelled,
    #[error("shader : {0}")]
    Shader(String),
}
