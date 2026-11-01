//! Hardware model shared by the OpenRGB bridge, the engine and the GUI.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable identity of a device across restarts.
///
/// OpenRGB SDK ids are only stable for the lifetime of one server, so the
/// layout and per-device settings are keyed by vendor, name, location and
/// serial instead.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceKey(pub String);

impl DeviceKey {
    pub fn new(vendor: &str, name: &str, location: &str, serial: &str) -> Self {
        Self(format!(
            "{}|{}|{}|{}",
            vendor.trim(),
            name.trim(),
            location.trim(),
            serial.trim()
        ))
    }
}

impl fmt::Display for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Device categories, mirroring OpenRGB's `device_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeviceKind {
    Motherboard,
    Dram,
    Gpu,
    Cooler,
    LedStrip,
    Keyboard,
    Mouse,
    MouseMat,
    Headset,
    HeadsetStand,
    Gamepad,
    Light,
    Speaker,
    Virtual,
    Storage,
    Case,
    Microphone,
    Accessory,
    Keypad,
    Laptop,
    Monitor,
    Unknown,
}

impl DeviceKind {
    /// French, user-facing label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Motherboard => "Carte mère",
            Self::Dram => "Mémoire RAM",
            Self::Gpu => "Carte graphique",
            Self::Cooler => "Refroidissement",
            Self::LedStrip => "Bande LED",
            Self::Keyboard => "Clavier",
            Self::Mouse => "Souris",
            Self::MouseMat => "Tapis de souris",
            Self::Headset => "Casque",
            Self::HeadsetStand => "Support de casque",
            Self::Gamepad => "Manette",
            Self::Light => "Éclairage",
            Self::Speaker => "Enceinte",
            Self::Virtual => "Virtuel",
            Self::Storage => "Stockage",
            Self::Case => "Boîtier",
            Self::Microphone => "Microphone",
            Self::Accessory => "Accessoire",
            Self::Keypad => "Pavé",
            Self::Laptop => "Portable",
            Self::Monitor => "Écran",
            Self::Unknown => "Périphérique",
        }
    }

    /// Default aspect ratio (width / height) of the device on the canvas.
    pub fn default_size(self) -> (f32, f32) {
        match self {
            Self::Motherboard => (0.22, 0.30),
            Self::Dram => (0.025, 0.20),
            Self::Gpu => (0.30, 0.08),
            Self::Cooler => (0.09, 0.16),
            Self::LedStrip => (0.40, 0.025),
            Self::Keyboard => (0.40, 0.14),
            Self::Keypad => (0.10, 0.12),
            Self::Mouse => (0.05, 0.09),
            Self::MouseMat => (0.26, 0.15),
            Self::Headset | Self::HeadsetStand => (0.08, 0.12),
            Self::Gamepad => (0.10, 0.07),
            Self::Light => (0.06, 0.10),
            Self::Speaker => (0.06, 0.14),
            Self::Case => (0.30, 0.55),
            Self::Monitor => (0.45, 0.06),
            Self::Laptop => (0.30, 0.12),
            _ => (0.08, 0.08),
        }
    }
}

/// How the LEDs of a zone are arranged physically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ZoneKind {
    Single,
    Linear,
    /// A closed loop of LEDs (fan rings, round AIO pumps).
    Ring,
    Matrix,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZoneInfo {
    pub name: String,
    pub kind: ZoneKind,
    pub led_count: usize,
    /// Matrix width/height when known (keyboards); `None` → auto grid.
    pub matrix: Option<(usize, usize)>,
    /// Size limits reported by OpenRGB. Addressable headers (ARGB +5 V) have
    /// `leds_min < leds_max`: the user tells how many LEDs are plugged in.
    #[serde(default)]
    pub leds_min: usize,
    #[serde(default)]
    pub leds_max: usize,
    /// LED count of each segment defined in OpenRGB (its own "per fan"
    /// split of a header), empty when there is none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<usize>,
}

impl ZoneInfo {
    /// The zone length can be changed (addressable ARGB header / channel).
    pub fn resizable(&self) -> bool {
        self.leds_max > self.leds_min
    }

    /// Resizable zone still at its empty default: nothing is driven on it
    /// until the user enters the number of LEDs.
    pub fn needs_size(&self) -> bool {
        self.resizable() && self.led_count == 0
    }
}

/// One RGB controller as reported by the OpenRGB SDK.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// SDK id for the current server session (used for LED updates).
    pub id: u32,
    pub key: DeviceKey,
    pub name: String,
    pub vendor: String,
    pub description: String,
    pub location: String,
    pub serial: String,
    pub kind: DeviceKind,
    pub zones: Vec<ZoneInfo>,
    pub led_count: usize,
    /// `true` when TuxRGB can drive the controller in software (Direct,
    /// Custom or Static mode, like OpenRGB's own effects).
    pub controllable: bool,
    /// The controller only takes one colour for the whole device (its mode
    /// colour): TuxRGB sends the average colour of its LEDs.
    #[serde(default)]
    pub single_color: bool,
    /// Name of the OpenRGB mode TuxRGB uses ("Direct", "Custom", "Static").
    #[serde(default)]
    pub mode: String,
    /// Demo device shown when no hardware is available.
    pub is_virtual: bool,
    /// The controller's own effects (OpenRGB modes other than the ones
    /// TuxRGB drives: "Rainbow wave", "Reactive"…), by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hardware_modes: Vec<String>,
    /// The hardware effect it runs instead of TuxRGB's, when the user chose
    /// one (keyboards, mice: typing reactions…). TuxRGB then leaves it
    /// alone until it is turned back to "TuxRGB effects".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware_mode: Option<String>,
}

impl DeviceInfo {
    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() {
            self.kind.label()
        } else if self.is_virtual {
            // Demo rig: French source names, translated at display time so
            // a language switch applies at once. Real hardware names are
            // shown exactly as OpenRGB reports them.
            crate::i18n::tr(&self.name)
        } else {
            &self.name
        }
    }
}

/// A plausible demo setup so the canvas and effects can be previewed without
/// hardware (or before hardware access has been configured).
pub fn demo_devices() -> Vec<DeviceInfo> {
    fn dev(id: u32, name: &str, kind: DeviceKind, zones: Vec<ZoneInfo>) -> DeviceInfo {
        let led_count = zones.iter().map(|z| z.led_count).sum();
        DeviceInfo {
            id,
            key: DeviceKey::new("TuxRGB Demo", name, &format!("demo:{id}"), ""),
            name: name.to_owned(),
            vendor: "TuxRGB Demo".to_owned(),
            description: "Périphérique virtuel de démonstration".to_owned(),
            location: format!("demo:{id}"),
            serial: String::new(),
            kind,
            zones,
            led_count,
            controllable: true,
            single_color: false,
            mode: "Direct".to_owned(),
            is_virtual: true,
            hardware_modes: match kind {
                DeviceKind::Keyboard => {
                    vec!["Rainbow Wave".into(), "Reactive".into(), "Breathing".into()]
                }
                DeviceKind::Mouse => vec!["Spectrum Cycle".into(), "Breathing".into()],
                _ => Vec::new(),
            },
            hardware_mode: None,
        }
    }
    fn zone(name: &str, kind: ZoneKind, led_count: usize) -> ZoneInfo {
        ZoneInfo {
            name: name.to_owned(),
            kind,
            led_count,
            matrix: None,
            leds_min: led_count,
            leds_max: led_count,
            segments: Vec::new(),
        }
    }
    vec![
        dev(
            9001,
            "Carte mère ARGB",
            DeviceKind::Motherboard,
            vec![
                zone("Dissipateur VRM", ZoneKind::Linear, 12),
                zone("Chipset", ZoneKind::Single, 1),
                ZoneInfo {
                    leds_min: 0,
                    leds_max: 120,
                    ..zone("En-tête ARGB 1", ZoneKind::Linear, 30)
                },
            ],
        ),
        dev(
            9002,
            "DDR5 RGB #1",
            DeviceKind::Dram,
            vec![zone("Barrette", ZoneKind::Linear, 10)],
        ),
        dev(
            9003,
            "DDR5 RGB #2",
            DeviceKind::Dram,
            vec![zone("Barrette", ZoneKind::Linear, 10)],
        ),
        dev(
            9004,
            "Carte graphique",
            DeviceKind::Gpu,
            vec![zone("Logo", ZoneKind::Linear, 22)],
        ),
        dev(
            9005,
            "Ventilateur avant",
            DeviceKind::Cooler,
            vec![zone("Anneau", ZoneKind::Linear, 16)],
        ),
        dev(
            9006,
            "Ventilateur arrière",
            DeviceKind::Cooler,
            vec![zone("Anneau", ZoneKind::Linear, 16)],
        ),
        dev(
            9007,
            "Clavier mécanique",
            DeviceKind::Keyboard,
            vec![ZoneInfo {
                name: "Touches".into(),
                kind: ZoneKind::Matrix,
                led_count: 22 * 6,
                matrix: Some((22, 6)),
                leds_min: 22 * 6,
                leds_max: 22 * 6,
                segments: Vec::new(),
            }],
        ),
        dev(
            9008,
            "Souris",
            DeviceKind::Mouse,
            vec![zone("Logo", ZoneKind::Linear, 3)],
        ),
        dev(
            9009,
            "Bande LED bureau",
            DeviceKind::LedStrip,
            vec![zone("Bande", ZoneKind::Linear, 60)],
        ),
    ]
}
