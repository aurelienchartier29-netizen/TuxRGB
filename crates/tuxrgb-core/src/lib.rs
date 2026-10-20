//! TuxRGB core.
//!
//! This crate has no GUI, D-Bus or hardware dependency so it can be unit
//! tested anywhere. It contains:
//!
//! * [`color`]  – a compact sRGB colour type with HSV helpers,
//! * [`device`] – the hardware model exposed by the OpenRGB bridge,
//! * [`layout`] – the 2D "setup canvas" (device placement and LED positions),
//! * [`effects`] – every lighting effect, its parameters and its renderer,
//! * [`signal`] – audio spectrum / screen frame inputs for reactive effects,
//! * [`engine`] – the fixed-rate, sleep-when-idle effect engine thread,
//! * [`process`] – helper programs run with a deadline.

pub mod color;
pub mod device;
pub mod effects;
pub mod engine;
pub mod i18n;
pub mod layout;
pub mod process;
pub mod signal;

pub use color::Rgb;
pub use device::{DeviceInfo, DeviceKey, DeviceKind, ZoneInfo, ZoneKind};
pub use effects::{EffectKind, EffectParams, ParamKind, ParamSpec, ParamValue};
pub use engine::{EngineCommand, EngineHandle, Frame, FrameSink};
pub use layout::{LedPoint, Placement, Scene, SceneDevice};
pub use signal::{AudioSpectrum, ScreenFrame, SharedInputs};
