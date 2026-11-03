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
    /// Where each LED of a matrix zone sits (keyboards: the key layout
    /// OpenRGB reports, see [`MatrixMap`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_map: Option<MatrixMap>,
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

/// Cells of a matrix zone, row by row (OpenRGB's matrix map): the LED of
/// the zone at each place, `None` for a gap (between key blocks).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixMap {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<Option<u32>>,
}

impl MatrixMap {
    /// A map that fits its size (`width × height` cells).
    pub fn new(width: usize, height: usize, cells: Vec<Option<u32>>) -> Option<Self> {
        (width > 0 && height > 0 && cells.len() == width * height).then_some(Self {
            width,
            height,
            cells,
        })
    }

    /// `(column, row)` of each LED of a zone of `leds` LEDs (its first
    /// cell when it covers several), `None` for LEDs the map leaves out.
    pub fn positions(&self, leds: usize) -> Vec<Option<(usize, usize)>> {
        let mut out = vec![None; leds];
        for (i, cell) in self.cells.iter().enumerate() {
            if let Some(led) = cell.map(|l| l as usize)
                && led < leds
                && out[led].is_none()
            {
                out[led] = Some((i % self.width, i / self.width));
            }
        }
        out
    }
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
    /// Name of each LED, OpenRGB order ("Key: A", "Logo"…), empty when
    /// unknown (TuxRGB 3.7: the per-LED editor).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub led_names: Vec<String>,
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

/// The demo keyboard: a full-size ISO layout (22 × 6), OpenRGB key names.
fn demo_keyboard() -> (MatrixMap, Vec<String>) {
    #[rustfmt::skip]
    const ROWS: [&[(usize, &str)]; 6] = [
        &[
            (0, "Escape"), (2, "F1"), (3, "F2"), (4, "F3"), (5, "F4"), (6, "F5"), (7, "F6"),
            (8, "F7"), (9, "F8"), (10, "F9"), (11, "F10"), (12, "F11"), (13, "F12"),
            (15, "Print Screen"), (16, "Scroll Lock"), (17, "Pause/Break"),
        ],
        &[
            (0, "`"), (1, "1"), (2, "2"), (3, "3"), (4, "4"), (5, "5"), (6, "6"), (7, "7"),
            (8, "8"), (9, "9"), (10, "0"), (11, "-"), (12, "="), (13, "Backspace"),
            (15, "Insert"), (16, "Home"), (17, "Page Up"), (18, "Num Lock"),
            (19, "Number Pad /"), (20, "Number Pad *"), (21, "Number Pad -"),
        ],
        &[
            (0, "Tab"), (1, "Q"), (2, "W"), (3, "E"), (4, "R"), (5, "T"), (6, "Y"), (7, "U"),
            (8, "I"), (9, "O"), (10, "P"), (11, "["), (12, "]"), (13, "Enter"),
            (15, "Delete"), (16, "End"), (17, "Page Down"), (18, "Number Pad 7"),
            (19, "Number Pad 8"), (20, "Number Pad 9"), (21, "Number Pad +"),
        ],
        &[
            (0, "Caps Lock"), (1, "A"), (2, "S"), (3, "D"), (4, "F"), (5, "G"), (6, "H"),
            (7, "J"), (8, "K"), (9, "L"), (10, ";"), (11, "'"), (12, "#"),
            (18, "Number Pad 4"), (19, "Number Pad 5"), (20, "Number Pad 6"),
        ],
        &[
            (0, "Left Shift"), (1, "\\ (ISO)"), (2, "Z"), (3, "X"), (4, "C"), (5, "V"),
            (6, "B"), (7, "N"), (8, "M"), (9, ","), (10, "."), (11, "/"), (13, "Right Shift"),
            (16, "Up Arrow"), (18, "Number Pad 1"), (19, "Number Pad 2"), (20, "Number Pad 3"),
            (21, "Number Pad Enter"),
        ],
        &[
            (0, "Left Control"), (1, "Left Windows"), (2, "Left Alt"), (6, "Space"),
            (10, "Right Alt"), (11, "Right Fn"), (12, "Menu"), (13, "Right Control"),
            (15, "Left Arrow"), (16, "Down Arrow"), (17, "Right Arrow"), (18, "Number Pad 0"),
            (20, "Number Pad ."),
        ],
    ];
    const W: usize = 22;
    let mut cells = vec![None; W * ROWS.len()];
    let mut names = Vec::new();
    for (row, keys) in ROWS.iter().enumerate() {
        for (col, name) in keys.iter() {
            cells[row * W + col] = Some(names.len() as u32);
            names.push(format!("Key: {name}"));
        }
    }
    (
        MatrixMap::new(W, ROWS.len(), cells).expect("22 × 6 cells"),
        names,
    )
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
            led_names: Vec::new(),
        }
    }
    fn zone(name: &str, kind: ZoneKind, led_count: usize) -> ZoneInfo {
        ZoneInfo {
            name: name.to_owned(),
            kind,
            led_count,
            matrix: None,
            matrix_map: None,
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
        {
            // A full-size ISO keyboard, keys named and placed as OpenRGB
            // reports them (US names, whatever the printed layout).
            let (map, names) = demo_keyboard();
            let n = names.len();
            DeviceInfo {
                led_names: names,
                ..dev(
                    9007,
                    "Clavier mécanique",
                    DeviceKind::Keyboard,
                    vec![ZoneInfo {
                        name: "Touches".into(),
                        kind: ZoneKind::Matrix,
                        led_count: n,
                        matrix: Some((map.width, map.height)),
                        matrix_map: Some(map),
                        leds_min: n,
                        leds_max: n,
                        segments: Vec::new(),
                    }],
                )
            }
        },
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
