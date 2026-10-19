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

pub mod analyzer;
pub mod audio;
pub mod hardware;
pub mod screen;
pub mod shader;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("PipeWire : {0}")]
    PipeWire(String),
    #[error("portail ScreenCast : {0}")]
    Portal(String),
    #[error("capture annulée par l'utilisateur")]
    Cancelled,
    #[error("shader : {0}")]
    Shader(String),
}
