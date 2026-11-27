//! "Ventilation" (TuxRGB 3.2): speed of the fans of the PC, by curve,
//! fixed or left to the hardware.
//!
//! * Linux: the fans of `/sys/class/hwmon` (motherboard, graphics card) are
//!   driven by a small root service, `tuxrgb-fans.service` (see
//!   `tuxrgb-helper/src/fand.rs`), which TuxRGB installs once through
//!   Polkit and sends its settings to. The curves keep running without
//!   TuxRGB, from the start of the system.
//! * Windows: the fan controller chip of the motherboard (ITE, Nuvoton) is
//!   driven by TuxRGB itself in administrator mode, through PawnIO (see
//!   `tuxrgb-capture/src/superio.rs`); a guard process gives the fans back
//!   to the motherboard if TuxRGB ever stops without doing it.
//!
//! The curves, their safety rules and their text form are shared with the
//! Linux service ([`curve`]).

#[path = "../../../tuxrgb-helper/src/fancurve.rs"]
pub mod curve;
#[cfg(target_os = "linux")]
#[path = "../../../tuxrgb-helper/src/hwmon.rs"]
pub mod hwmon;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
pub mod windows;

use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;
use tuxrgb_core::SharedInputs;

/// How the page sets a fan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FanMode {
    /// The motherboard (or graphics card) decides.
    #[default]
    Auto,
    Fixed,
    Curve,
}

/// One fan's settings, saved in the configuration. The fixed speed and the
/// curve are kept while another mode is chosen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FanPrefs {
    /// Name given by the user.
    pub name: Option<String>,
    pub mode: FanMode,
    /// %.
    pub fixed: u8,
    /// `cpu`, `gpu` or a sensor id.
    pub source: String,
    /// (°C, %).
    pub points: Vec<(u8, u8)>,
}

impl Default for FanPrefs {
    fn default() -> Self {
        Self {
            name: None,
            mode: FanMode::Auto,
            fixed: 50,
            source: "cpu".into(),
            points: curve::preset("balanced"),
        }
    }
}

impl FanPrefs {
    /// A graphics card's own fans follow its temperature by default.
    pub fn for_fan(kind: FanKind) -> Self {
        Self {
            source: if kind == FanKind::Gpu { "gpu" } else { "cpu" }.into(),
            ..Self::default()
        }
    }

    pub fn mode(&self) -> curve::Mode {
        match self.mode {
            FanMode::Auto => curve::Mode::Auto,
            FanMode::Fixed => curve::Mode::Fixed(self.fixed.min(100)),
            FanMode::Curve => {
                let c = curve::Curve {
                    source: self.source.clone(),
                    points: self.points.clone(),
                };
                // A broken curve (hand-edited configuration) is not sent:
                // the hardware keeps the fan.
                if c.validate().is_ok() {
                    curve::Mode::Curve(c)
                } else {
                    curve::Mode::Auto
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FansConfig {
    /// By fan id.
    pub fans: BTreeMap<String, FanPrefs>,
}

impl FansConfig {
    /// What the Linux service applies: the driven fans only (a fan it is
    /// not told about goes back to the hardware), so old entries of fans
    /// long gone never add up to a refused list.
    #[cfg_attr(windows, allow(dead_code))]
    pub fn settings(&self) -> Vec<curve::Setting> {
        self.fans
            .iter()
            .filter(|(id, _)| curve::valid_id(id))
            .map(|(id, p)| curve::Setting {
                fan: id.clone(),
                mode: p.mode(),
            })
            .filter(|s| s.mode != curve::Mode::Auto)
            .collect()
    }

    /// Fans driven by TuxRGB (not left to the hardware).
    pub fn any_driven(&self) -> bool {
        self.fans.values().any(|p| p.mode != FanMode::Auto)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FanKind {
    #[default]
    Motherboard,
    Gpu,
    #[cfg_attr(windows, allow(dead_code))]
    Other,
}

/// What a fan is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FanStatus {
    #[default]
    Auto,
    Driven,
    /// Full speed: a temperature reached 90 °C.
    Safety,
    /// Its curve's temperature is unreadable: the hardware drives it.
    NoSensor,
    /// The driver refuses to hand it over.
    Refused,
}

impl FanStatus {
    fn from_curve(s: curve::Status) -> Self {
        match s {
            curve::Status::Auto => Self::Auto,
            curve::Status::Driven => Self::Driven,
            curve::Status::Safety => Self::Safety,
            curve::Status::NoSensor => Self::NoSensor,
        }
    }
}

/// One fan, as the page shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FanInfo {
    pub id: String,
    /// Chip (`it8689`, `IT8689E`, `amdgpu`…).
    pub chip: String,
    /// Header number on its chip (from 1).
    pub index: u8,
    pub kind: FanKind,
    /// Name given by the hardware, if any.
    pub label: Option<String>,
    pub rpm: Option<u32>,
    /// Current output, %.
    pub duty: Option<f32>,
    pub controllable: bool,
    pub status: FanStatus,
}

impl FanInfo {
    /// Name to show when the user gave none.
    pub fn default_name(&self) -> String {
        if let Some(l) = &self.label {
            return l.clone();
        }
        match self.kind {
            FanKind::Gpu => crate::i18n::t("Carte graphique").to_owned(),
            FanKind::Motherboard => crate::i18n::trf!("Ventilateur {}", self.index),
            FanKind::Other => format!("{} {}", self.chip, self.index),
        }
    }
}

/// A temperature a curve can follow.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TempInfo {
    pub id: String,
    pub label: String,
    pub value: Option<f32>,
}

/// Can TuxRGB drive the fans here?
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Access {
    #[default]
    Starting,
    Ready,
    /// Linux: the fan service is not installed or not running.
    #[cfg_attr(windows, allow(dead_code))]
    ServiceMissing,
    /// Linux: the service is from another TuxRGB version.
    #[cfg_attr(windows, allow(dead_code))]
    ServiceOutdated(String),
    /// Windows: TuxRGB does not run in administrator mode.
    #[cfg_attr(not(windows), allow(dead_code))]
    NeedsAdmin,
    /// Windows: the PawnIO driver is missing.
    #[cfg_attr(not(windows), allow(dead_code))]
    NeedsPawnIo,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FansState {
    pub access: Access,
    pub fans: Vec<FanInfo>,
    pub temps: Vec<TempInfo>,
    /// Safety mode: every driven fan at full speed.
    pub safety: bool,
    /// Virtual fans (screenshot tour).
    pub demo: bool,
}

pub enum Command {
    Configure(FansConfig),
    /// The page is on screen: live values every second.
    Page(bool),
    /// Full speed for a few seconds, to find a fan.
    Identify(String),
    Rescan,
    /// Gives every fan back to the hardware (quit, shutdown), then confirms.
    Release(Option<Sender<()>>),
    Quit,
}

pub struct FanService {
    tx: Sender<Command>,
}

impl FanService {
    pub fn spawn(
        inputs: SharedInputs,
        events: impl Fn(FansState) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (tx, rx) = crossbeam_channel::bounded(64);
        let demo = crate::args().screenshot_tour.is_some();
        std::thread::Builder::new()
            .name("tuxrgb-fans".into())
            .spawn(move || {
                if demo {
                    return demo_run(rx, Box::new(events));
                }
                #[cfg(target_os = "linux")]
                {
                    let _ = &inputs;
                    linux::run(rx, Box::new(events));
                }
                #[cfg(windows)]
                windows::run(inputs, rx, Box::new(events));
            })?;
        Ok(Self { tx })
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.try_send(cmd);
    }

    /// Gives the fans back, waiting at most `budget`.
    pub fn release(&self, budget: Duration) -> bool {
        let (done, wait) = crossbeam_channel::bounded(1);
        if self
            .tx
            .send_timeout(Command::Release(Some(done)), budget)
            .is_err()
        {
            return false;
        }
        wait.recv_timeout(budget).is_ok()
    }
}

impl Drop for FanService {
    fn drop(&mut self) {
        let _ = self.tx.try_send(Command::Quit);
    }
}

pub type Events = Box<dyn Fn(FansState) + Send>;

/// Default label of a temperature source.
pub fn source_label(id: &str, temps: &[TempInfo]) -> String {
    match id {
        "cpu" => crate::i18n::t("Processeur").to_owned(),
        "gpu" => crate::i18n::t("Carte graphique").to_owned(),
        _ => temps
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.label.clone())
            .unwrap_or_else(|| id.to_owned()),
    }
}

/// Virtual fans for the screenshot tour.
fn demo_run(rx: crossbeam_channel::Receiver<Command>, events: Events) {
    let mut config = FansConfig::default();
    let mut t = 0.0f32;
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Command::Configure(c)) => config = c,
            Ok(Command::Release(Some(done))) => {
                let _ = done.send(());
            }
            Ok(Command::Quit) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            _ => {}
        }
        t += 1.0;
        let cpu = 54.0 + 3.0 * (t * 0.3).sin();
        let gpu = 47.0 + 2.0 * (t * 0.2).sin();
        let temps = vec![
            TempInfo {
                id: "cpu".into(),
                label: crate::i18n::t("Processeur").into(),
                value: Some(cpu),
            },
            TempInfo {
                id: "gpu".into(),
                label: crate::i18n::t("Carte graphique").into(),
                value: Some(gpu),
            },
            TempInfo {
                id: "demo-board-temp1".into(),
                label: crate::i18n::trf!("Carte mère {}", 1),
                value: Some(36.0),
            },
        ];
        let fan = |i: u8, kind: FanKind, chip: &str, id: &str, base_rpm: f32| {
            let prefs = config
                .fans
                .get(id)
                .cloned()
                .unwrap_or_else(|| FanPrefs::for_fan(kind));
            let duty = match prefs.mode() {
                curve::Mode::Auto => 45.0,
                curve::Mode::Fixed(p) => f32::from(p),
                curve::Mode::Curve(c) => c.duty_at(if c.source == "gpu" { gpu } else { cpu }),
            };
            FanInfo {
                id: id.into(),
                chip: chip.into(),
                index: i,
                kind,
                label: None,
                rpm: Some((base_rpm * duty / 100.0) as u32),
                duty: Some(duty),
                controllable: true,
                status: if prefs.mode == FanMode::Auto {
                    FanStatus::Auto
                } else {
                    FanStatus::Driven
                },
            }
        };
        events(FansState {
            access: Access::Ready,
            fans: vec![
                fan(
                    1,
                    FanKind::Motherboard,
                    "it8689",
                    "demo-it8689-fan1",
                    1900.0,
                ),
                fan(
                    2,
                    FanKind::Motherboard,
                    "it8689",
                    "demo-it8689-fan2",
                    1500.0,
                ),
                fan(
                    3,
                    FanKind::Motherboard,
                    "it8689",
                    "demo-it8689-fan3",
                    1500.0,
                ),
            ],
            temps,
            safety: false,
            demo: true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_become_settings() {
        let mut c = FansConfig::default();
        c.fans.insert(
            "it8689-it87.2624-fan1".into(),
            FanPrefs {
                mode: FanMode::Curve,
                ..FanPrefs::default()
            },
        );
        c.fans.insert(
            "it8689-it87.2624-fan2".into(),
            FanPrefs {
                mode: FanMode::Fixed,
                fixed: 140,
                ..FanPrefs::default()
            },
        );
        // A broken curve falls back to the hardware.
        c.fans.insert(
            "it8689-it87.2624-fan3".into(),
            FanPrefs {
                mode: FanMode::Curve,
                points: vec![(50, 10)],
                ..FanPrefs::default()
            },
        );
        c.fans.insert("bad id".into(), FanPrefs::default());
        let s = c.settings();
        assert_eq!(s.len(), 2);
        assert!(matches!(s[0].mode, curve::Mode::Curve(_)));
        assert_eq!(s[1].mode, curve::Mode::Fixed(100));
        assert!(c.any_driven());
        let text = curve::format_settings(&s);
        assert_eq!(curve::parse_settings(&text), s);
    }

    #[test]
    fn config_round_trips_through_json() {
        let mut c = FansConfig::default();
        c.fans.insert(
            "amdgpu-0000:03:00.0-fan1".into(),
            FanPrefs {
                name: Some("GPU".into()),
                ..FanPrefs::for_fan(FanKind::Gpu)
            },
        );
        let json = serde_json::to_string(&c).unwrap();
        let back: FansConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.fans.values().next().unwrap().source, "gpu");
        // Older configurations have no "fans" at all.
        let empty: FansConfig = serde_json::from_str("{}").unwrap();
        assert!(empty.fans.is_empty());
    }
}
