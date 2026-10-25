//! Hardware displays: the screens of AIO coolers and the digital displays
//! of air coolers and cases ("Afficheurs", TuxRGB 3.1).
//!
//! Three families of devices, driven over USB HID (no vendor software, no
//! kernel module), with protocols re-implemented from open-source projects:
//!
//! * DeepCool digital displays (AK, AG, LS, LD, LP, LQ series, AK Digital
//!   Pro, CH cases, CH510 Mesh Digital, Assassin IV VC Vision…) –
//!   [deepcool-digital-linux](https://github.com/Nortank12/deepcool-digital-linux);
//! * NZXT Kraken Z (Z53/Z63/Z73), Kraken 2023 / 2023 Elite, Kraken 2024
//!   Elite RGB / Plus LCD screens – [liquidctl](https://github.com/liquidctl/liquidctl);
//! * Corsair LCD screens (iCUE ELITE LCD caps, iCUE LINK LCD, XD5, XC7
//!   ELITE LCD, Nautilus LCD) – [OpenLinkHub](https://github.com/jurkovic-nikola/OpenLinkHub).
//!
//! Every display is a [`HardwareDisplay`]: the manager ([`super`]) samples
//! the sensors, renders full-colour screens ([`super::render`]) and hands
//! each display what it can show at the pace it was given.

use super::hid::HidInfo;
use serde::{Deserialize, Serialize};
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tuxrgb_core::signal::HardwareSample;

/// A value a display can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    CpuTemp,
    CpuLoad,
    CpuPower,
    CpuFreq,
    GpuTemp,
    GpuLoad,
    GpuPower,
    GpuFreq,
    Ram,
    /// The time of day (screens only).
    Time,
}

impl Metric {
    /// Name for the settings (French, translated by the interface).
    pub fn label(self) -> &'static str {
        match self {
            Self::CpuTemp => "Température CPU",
            Self::CpuLoad => "Charge CPU",
            Self::CpuPower => "Puissance CPU",
            Self::CpuFreq => "Fréquence CPU",
            Self::GpuTemp => "Température GPU",
            Self::GpuLoad => "Charge GPU",
            Self::GpuPower => "Puissance GPU",
            Self::GpuFreq => "Fréquence GPU",
            Self::Ram => "Mémoire (RAM)",
            Self::Time => "Heure",
        }
    }

    /// Short caption on the screens.
    pub fn caption(self) -> &'static str {
        match self {
            Self::CpuTemp | Self::CpuLoad | Self::CpuPower | Self::CpuFreq => "CPU",
            Self::GpuTemp | Self::GpuLoad | Self::GpuPower | Self::GpuFreq => "GPU",
            Self::Ram => "RAM",
            Self::Time => "",
        }
    }

    pub fn is_temperature(self) -> bool {
        matches!(self, Self::CpuTemp | Self::GpuTemp)
    }

    /// Value of the metric in `sample` (°C, %, W, MHz; RAM in %).
    pub fn value(self, sample: &HardwareSample) -> Option<f32> {
        match self {
            Self::CpuTemp => sample.cpu_temp,
            Self::CpuLoad => sample.cpu_load.map(|v| v * 100.0),
            Self::CpuPower => sample.cpu_power,
            Self::CpuFreq => sample.cpu_freq,
            Self::GpuTemp => sample.gpu_temp,
            Self::GpuLoad => sample.gpu_load.map(|v| v * 100.0),
            Self::GpuPower => sample.gpu_power,
            Self::GpuFreq => sample.gpu_freq,
            Self::Ram => sample.mem_used.map(|v| v * 100.0),
            Self::Time => None,
        }
    }

    /// Full scale of a gauge.
    pub fn range(self) -> f32 {
        match self {
            Self::CpuTemp | Self::GpuTemp => 100.0,
            Self::CpuLoad | Self::GpuLoad | Self::Ram => 100.0,
            Self::CpuPower => 250.0,
            Self::GpuPower => 450.0,
            Self::CpuFreq => 6000.0,
            Self::GpuFreq => 3200.0,
            Self::Time => 1.0,
        }
    }

    /// Value and unit as shown (`fahrenheit` for temperatures).
    pub fn format(self, value: Option<f32>, fahrenheit: bool) -> (String, &'static str) {
        let Some(v) = value else {
            return ("--".into(), self.unit(fahrenheit));
        };
        let v = if self.is_temperature() && fahrenheit {
            v * 9.0 / 5.0 + 32.0
        } else {
            v
        };
        let text = match self {
            Self::CpuFreq | Self::GpuFreq if v >= 1000.0 => format!("{:.1}", v / 1000.0),
            _ => format!("{:.0}", v.round()),
        };
        (text, self.unit_for(v, fahrenheit))
    }

    fn unit_for(self, v: f32, fahrenheit: bool) -> &'static str {
        match self {
            Self::CpuFreq | Self::GpuFreq if v >= 1000.0 => "GHz",
            _ => self.unit(fahrenheit),
        }
    }

    pub fn unit(self, fahrenheit: bool) -> &'static str {
        match self {
            Self::CpuTemp | Self::GpuTemp => {
                if fahrenheit {
                    "°F"
                } else {
                    "°C"
                }
            }
            Self::CpuLoad | Self::GpuLoad | Self::Ram => "%",
            Self::CpuPower | Self::GpuPower => "W",
            Self::CpuFreq | Self::GpuFreq => "MHz",
            Self::Time => "",
        }
    }
}

/// What a display shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Content {
    /// Sensor values.
    #[default]
    Metrics,
    /// A text of the user's (screens).
    Text,
    /// A picture (screens).
    Image,
    /// The display's own screen (NZXT Kraken: liquid temperature).
    Builtin,
}

/// Settings of one display, saved in the configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettings {
    /// Driven by TuxRGB.
    pub enabled: bool,
    pub content: Content,
    /// Values shown, in order; those that do not fit at once alternate.
    pub metrics: Vec<Metric>,
    pub text: String,
    pub image: Option<std::path::PathBuf>,
    /// Refresh period, milliseconds.
    pub interval_ms: u32,
    /// Seconds each set of values stays before the next one (more values
    /// than the display shows at once).
    pub cycle_s: u32,
    /// Screens: brightness, %.
    pub brightness: u8,
    /// Screens: orientation, 0 / 90 / 180 / 270°.
    pub rotation: u16,
    /// Digital displays: blink above 90 °C (where the display can).
    pub alarm: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            content: Content::Metrics,
            metrics: vec![Metric::CpuTemp, Metric::CpuLoad],
            text: String::new(),
            image: None,
            interval_ms: 1000,
            cycle_s: 5,
            brightness: 80,
            rotation: 0,
            alarm: false,
        }
    }
}

impl DisplaySettings {
    /// First settings of a display just found: what suits it.
    pub fn for_display(caps: &Capabilities) -> Self {
        let mut s = Self {
            // Digital displays show nothing without a program: on at once.
            // Screens have a picture of their own: the user decides.
            enabled: caps.auto_start,
            ..Self::default()
        };
        s.metrics.retain(|m| caps.metrics.contains(m));
        if s.metrics.is_empty() {
            s.metrics = caps
                .metrics
                .iter()
                .copied()
                .take(caps.at_once.max(1))
                .collect();
        }
        if caps.builtin.is_some() && !caps.image {
            s.content = Content::Builtin;
        }
        s.interval_ms = s.interval_ms.max(caps.min_interval.as_millis() as u32);
        s
    }

    pub fn interval(&self) -> Duration {
        Duration::from_millis(u64::from(self.interval_ms.clamp(250, 60_000)))
    }
}

/// The surface of a display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// Seven-segment digits with a unit and a load bar.
    Digits,
    /// 14 × 14 dot matrix (DeepCool LP).
    Matrix,
    /// Full-colour screen.
    Screen {
        width: u32,
        height: u32,
        round: bool,
    },
}

/// What a display can do.
#[derive(Clone, Debug, PartialEq)]
pub struct Capabilities {
    pub surface: Surface,
    /// Values it can show.
    pub metrics: &'static [Metric],
    /// How many values at once (more alternate).
    pub at_once: usize,
    /// Shows a fixed set of values by itself (no choice).
    pub fixed: bool,
    pub text: bool,
    pub image: bool,
    pub brightness: bool,
    pub rotation: bool,
    /// The display's own screen, if any (its name).
    pub builtin: Option<&'static str>,
    pub alarm: bool,
    pub fahrenheit: bool,
    pub min_interval: Duration,
    /// Driven as soon as found (digital displays).
    pub auto_start: bool,
    /// Why something is missing (e.g. pictures need Linux), for the user.
    pub note: Option<&'static str>,
}

/// Every value a screen can show.
pub const SCREEN_METRICS: &[Metric] = &[
    Metric::CpuTemp,
    Metric::CpuLoad,
    Metric::CpuPower,
    Metric::CpuFreq,
    Metric::GpuTemp,
    Metric::GpuLoad,
    Metric::GpuPower,
    Metric::GpuFreq,
    Metric::Ram,
    Metric::Time,
];

/// Most values a screen shows at once.
pub const SCREEN_AT_ONCE: usize = 4;

impl Capabilities {
    pub fn screen(width: u32, height: u32, round: bool) -> Self {
        Self {
            surface: Surface::Screen {
                width,
                height,
                round,
            },
            metrics: SCREEN_METRICS,
            at_once: SCREEN_AT_ONCE,
            fixed: false,
            text: true,
            image: true,
            brightness: true,
            rotation: true,
            builtin: None,
            alarm: false,
            fahrenheit: true,
            min_interval: Duration::from_millis(500),
            auto_start: false,
            note: None,
        }
    }

    pub fn is_screen(&self) -> bool {
        matches!(self.surface, Surface::Screen { .. })
    }
}

/// A rendered screen: RGBA, row-major, `width × height`.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Picture {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            rgba: vec![0; (width * height * 4) as usize],
        }
    }

    /// RGB of a pixel.
    #[cfg(test)]
    pub fn rgb(&self, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2]]
    }

    /// Turned clockwise by `quarter` quarter turns (square screens keep
    /// their size; others swap width and height).
    pub fn rotated(&self, quarter: u16) -> Picture {
        let q = quarter % 4;
        if q == 0 {
            return self.clone();
        }
        let (w, h) = (self.width, self.height);
        let (nw, nh) = if q % 2 == 1 { (h, w) } else { (w, h) };
        let mut out = Picture::new(nw, nh);
        for y in 0..h {
            for x in 0..w {
                let (nx, ny) = match q {
                    1 => (h - 1 - y, x),
                    2 => (w - 1 - x, h - 1 - y),
                    _ => (y, w - 1 - x),
                };
                let src = ((y * w + x) * 4) as usize;
                let dst = ((ny * nw + nx) * 4) as usize;
                out.rgba[dst..dst + 4].copy_from_slice(&self.rgba[src..src + 4]);
            }
        }
        out
    }

    /// Cheap fingerprint (nothing re-sent when the picture did not change).
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.width.hash(&mut h);
        self.height.hash(&mut h);
        self.rgba.hash(&mut h);
        h.finish()
    }
}

/// What a display is asked to show this time.
pub struct Frame<'a> {
    pub settings: &'a DisplaySettings,
    /// Latest sensor values.
    pub sample: &'a HardwareSample,
    /// The values to show now (the alternation already applied).
    pub metrics: &'a [Metric],
    /// Temperatures in °F.
    pub fahrenheit: bool,
    /// Screens: the rendered picture (display size, orientation applied).
    pub picture: Option<Arc<Picture>>,
}

/// A display TuxRGB drives.
pub trait HardwareDisplay: Send {
    fn capabilities(&self) -> &Capabilities;

    /// Shows a frame. An error means the display is gone (unplugged) or
    /// refuses: the manager drops it and looks for it again later.
    fn show(&mut self, frame: &Frame<'_>) -> io::Result<()>;

    /// TuxRGB lets go of the display (switched off in the settings, quit,
    /// shutdown): back to the display's own screen when it has one. Short
    /// and bounded.
    fn release(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A display model TuxRGB knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Model {
    pub vendor_id: u16,
    pub product_id: u16,
    pub vendor: &'static str,
    pub name: &'static str,
    pub family: Family,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    DeepCool(super::deepcool::Protocol),
    Kraken(super::kraken::Variant),
    Corsair(super::corsair::Variant),
}

/// Every display TuxRGB can drive.
pub fn models() -> impl Iterator<Item = &'static Model> {
    super::deepcool::MODELS
        .iter()
        .chain(super::kraken::MODELS)
        .chain(super::corsair::MODELS)
}

/// The model of a HID device, if TuxRGB drives it.
pub fn identify(vendor_id: u16, product_id: u16) -> Option<&'static Model> {
    models().find(|m| m.vendor_id == vendor_id && m.product_id == product_id)
}

/// Opens the driver of a detected display.
pub fn open(model: &Model, info: &HidInfo) -> io::Result<Box<dyn HardwareDisplay>> {
    let transport = super::hid::open(info)?;
    Ok(match model.family {
        Family::DeepCool(protocol) => Box::new(super::deepcool::DeepCool::new(
            transport,
            protocol,
            &info.product,
        )),
        Family::Kraken(variant) => Box::new(super::kraken::Kraken::open(transport, variant, info)?),
        Family::Corsair(variant) => Box::new(super::corsair::CorsairLcd::new(transport, variant)),
    })
}

/// The capabilities of a model, before it is opened (the settings page).
pub fn capabilities(model: &Model) -> Capabilities {
    match model.family {
        Family::DeepCool(p) => super::deepcool::capabilities(p),
        Family::Kraken(v) => super::kraken::capabilities(v),
        Family::Corsair(v) => super::corsair::capabilities(v),
    }
}

/// udev rules giving the logged-in user access to every supported display
/// (HID and, for the Kraken pictures, the USB device itself). Added to the
/// rules TuxRGB installs with OpenRGB's.
pub fn udev_rules() -> String {
    let mut out = String::from(
        "\n# TuxRGB 3.1 – hardware displays (AIO screens, digital displays):\n\
         # access for the logged-in user only.\n",
    );
    let mut seen = std::collections::BTreeSet::new();
    for m in models() {
        if !seen.insert((m.vendor_id, m.product_id)) {
            continue;
        }
        out.push_str(&format!(
            "SUBSYSTEMS==\"usb|hidraw\", ATTRS{{idVendor}}==\"{:04x}\", ATTRS{{idProduct}}==\"{:04x}\", TAG+=\"uaccess\", TAG+=\"TuxRGB_Display\"\n",
            m.vendor_id, m.product_id
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_are_formatted_for_the_screens() {
        assert_eq!(
            Metric::CpuTemp.format(Some(61.6), false),
            ("62".into(), "°C")
        );
        assert_eq!(
            Metric::CpuTemp.format(Some(50.0), true),
            ("122".into(), "°F")
        );
        assert_eq!(
            Metric::CpuFreq.format(Some(4875.0), false),
            ("4.9".into(), "GHz")
        );
        assert_eq!(
            Metric::GpuFreq.format(Some(810.0), false),
            ("810".into(), "MHz")
        );
        assert_eq!(Metric::GpuLoad.format(None, false), ("--".into(), "%"));
        let sample = HardwareSample {
            cpu_load: Some(0.42),
            mem_used: Some(0.5),
            ..HardwareSample::default()
        };
        assert_eq!(Metric::CpuLoad.value(&sample), Some(42.0));
        assert_eq!(Metric::Ram.value(&sample), Some(50.0));
    }

    #[test]
    fn every_model_is_unique_and_has_rules() {
        let mut ids = std::collections::BTreeSet::new();
        for m in models() {
            assert!(ids.insert((m.vendor_id, m.product_id)), "{m:?} twice");
            assert!(!capabilities(m).metrics.is_empty() || capabilities(m).builtin.is_some());
        }
        let rules = udev_rules();
        assert!(rules.contains("ATTRS{idVendor}==\"3633\", ATTRS{idProduct}==\"0002\""));
        assert!(rules.contains("ATTRS{idVendor}==\"1e71\", ATTRS{idProduct}==\"3008\""));
        assert!(rules.contains("ATTRS{idVendor}==\"1b1c\", ATTRS{idProduct}==\"0c39\""));
        assert!(rules.lines().all(|l| l.is_empty()
            || l.starts_with('#')
            || (l.contains("TAG+=\"uaccess\"") && !l.contains("MODE"))));
    }

    #[test]
    fn pictures_turn_by_quarter_turns() {
        let mut p = Picture::new(2, 1);
        p.rgba[..4].copy_from_slice(&[255, 0, 0, 255]);
        p.rgba[4..].copy_from_slice(&[0, 0, 255, 255]);
        let q = p.rotated(1);
        assert_eq!((q.width, q.height), (1, 2));
        // Clockwise: the left pixel goes to the top.
        assert_eq!(q.rgb(0, 0), [255, 0, 0]);
        assert_eq!(q.rgb(0, 1), [0, 0, 255]);
        assert_eq!(p.rotated(2).rgb(0, 0), [0, 0, 255]);
        assert_eq!(p.rotated(3).rgb(0, 0), [0, 0, 255]);
        assert_eq!(p.rotated(4), p);
    }

    #[test]
    fn first_settings_suit_the_display() {
        let digits = capabilities(identify(0x3633, 0x0002).unwrap());
        let s = DisplaySettings::for_display(&digits);
        assert!(s.enabled, "digital displays start at once");
        assert!(s.metrics.iter().all(|m| digits.metrics.contains(m)));
        let lcd = capabilities(identify(0x1b1c, 0x0c39).unwrap());
        let s = DisplaySettings::for_display(&lcd);
        assert!(!s.enabled, "screens wait for the user");
        assert_eq!(s.content, Content::Metrics);
    }
}
