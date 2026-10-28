//! Fan curves: what speed a fan gets for a temperature, and the safety rules
//! around it. Shared, as plain code without any dependency, by the Linux
//! fan service (`tuxrgb-helper fan-daemon`, running as root) and by TuxRGB
//! itself (the curve editor, and the Windows controller).
//!
//! Safety, whatever the settings:
//! * at [`SAFETY_TEMP`] (CPU or graphics card) every controlled fan runs at
//!   100 % until the temperature is back under [`SAFETY_RELEASE`];
//! * a curve whose temperature cannot be read for [`SENSOR_GRACE`] seconds
//!   gives its fan back to the motherboard's own automatic control;
//! * a fan is never stopped by a curve unless the user put a 0 % point.
//!
//! The text format of the settings (one line per fan) is also defined here:
//! it is what the GUI sends to the service and what the service keeps in
//! `/var/lib/tuxrgb/fans.conf`.

#![allow(dead_code)]

/// Above this (°C, CPU or GPU), every controlled fan runs at full speed.
pub const SAFETY_TEMP: f32 = 90.0;
/// Safety mode ends below this.
pub const SAFETY_RELEASE: f32 = 85.0;
/// Seconds without a reading of its temperature before a curve gives up.
pub const SENSOR_GRACE: u32 = 3;
/// Temperature fall needed before the speed goes down (°C).
pub const HYSTERESIS: f32 = 2.0;
/// Fastest change per second, up and down (% of full speed).
pub const RAMP_UP: f32 = 20.0;
pub const RAMP_DOWN: f32 = 4.0;
pub const MAX_POINTS: usize = 8;
/// Temperatures of a curve, °C.
pub const TEMP_MIN: u8 = 0;
pub const TEMP_MAX: u8 = 110;

/// How a fan is driven.
#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    /// The motherboard (or graphics card) decides, as without TuxRGB.
    Auto,
    /// A constant speed, % of full speed.
    Fixed(u8),
    Curve(Curve),
}

/// Speed as a function of one temperature.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    /// `cpu`, `gpu` or the id of a temperature sensor.
    pub source: String,
    /// (°C, %), temperatures strictly increasing.
    pub points: Vec<(u8, u8)>,
}

impl Curve {
    /// Speed at `temp`, %: flat before the first point and after the last,
    /// straight lines between them.
    pub fn duty_at(&self, temp: f32) -> f32 {
        let pts = &self.points;
        let Some(first) = pts.first() else {
            return 100.0;
        };
        if temp <= f32::from(first.0) {
            return f32::from(first.1);
        }
        for w in pts.windows(2) {
            let (t0, d0) = (f32::from(w[0].0), f32::from(w[0].1));
            let (t1, d1) = (f32::from(w[1].0), f32::from(w[1].1));
            if temp <= t1 {
                let k = if t1 > t0 {
                    (temp - t0) / (t1 - t0)
                } else {
                    1.0
                };
                return d0 + (d1 - d0) * k;
            }
        }
        f32::from(pts[pts.len() - 1].1)
    }

    /// Checks a curve received from outside.
    pub fn validate(&self) -> Result<(), String> {
        if !valid_id(&self.source) {
            return Err("source de température invalide".into());
        }
        if self.points.len() < 2 || self.points.len() > MAX_POINTS {
            return Err("une courbe a de 2 à 8 points".into());
        }
        for w in self.points.windows(2) {
            if w[1].0 <= w[0].0 {
                return Err("températures de la courbe non croissantes".into());
            }
        }
        if self
            .points
            .iter()
            .any(|&(t, d)| !(TEMP_MIN..=TEMP_MAX).contains(&t) || d > 100)
        {
            return Err("point de courbe hors limites".into());
        }
        Ok(())
    }
}

/// Ready-made curves (CPU temperatures).
pub fn preset(name: &str) -> Vec<(u8, u8)> {
    match name {
        "silent" => vec![(40, 20), (55, 30), (70, 50), (80, 75), (88, 100)],
        "performance" => vec![(30, 40), (45, 55), (60, 75), (72, 90), (80, 100)],
        // "balanced"
        _ => vec![(35, 25), (50, 35), (65, 55), (75, 80), (85, 100)],
    }
}

/// Ids of fans and sensors: short, without spaces or separators of the
/// text format.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 96
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

/// One fan's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    pub fan: String,
    pub mode: Mode,
}

impl Setting {
    /// `FAN <id> auto` / `FAN <id> fixed <pct>` /
    /// `FAN <id> curve <source> <t>:<pct>,<t>:<pct>…`
    pub fn to_line(&self) -> String {
        match &self.mode {
            Mode::Auto => format!("FAN {} auto", self.fan),
            Mode::Fixed(p) => format!("FAN {} fixed {p}", self.fan),
            Mode::Curve(c) => {
                let pts: Vec<String> = c.points.iter().map(|(t, d)| format!("{t}:{d}")).collect();
                format!("FAN {} curve {} {}", self.fan, c.source, pts.join(","))
            }
        }
    }

    pub fn parse(line: &str) -> Result<Self, String> {
        let mut w = line.split_ascii_whitespace();
        if w.next() != Some("FAN") {
            return Err("ligne inattendue".into());
        }
        let fan = w
            .next()
            .filter(|f| valid_id(f))
            .ok_or("ventilateur invalide")?;
        let mode = match w.next() {
            Some("auto") => Mode::Auto,
            Some("fixed") => {
                let p: u8 = w
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|p| *p <= 100)
                    .ok_or("vitesse invalide")?;
                Mode::Fixed(p)
            }
            Some("curve") => {
                let source = w.next().ok_or("source manquante")?.to_owned();
                let points = w
                    .next()
                    .ok_or("points manquants")?
                    .split(',')
                    .map(|p| {
                        let (t, d) = p.split_once(':')?;
                        Some((t.parse().ok()?, d.parse().ok()?))
                    })
                    .collect::<Option<Vec<(u8, u8)>>>()
                    .ok_or("points invalides")?;
                let c = Curve { source, points };
                c.validate()?;
                Mode::Curve(c)
            }
            _ => return Err("mode inconnu".into()),
        };
        if w.next().is_some() {
            return Err("ligne trop longue".into());
        }
        Ok(Self {
            fan: fan.to_owned(),
            mode,
        })
    }
}

/// Settings of every fan, one per line (unknown or broken lines skipped:
/// a file written by another version never stops the service).
pub fn parse_settings(text: &str) -> Vec<Setting> {
    let mut out: Vec<Setting> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Ok(s) = Setting::parse(line)
            && !out.iter().any(|o| o.fan == s.fan)
            && out.len() < 64
        {
            out.push(s);
        }
    }
    out
}

pub fn format_settings(settings: &[Setting]) -> String {
    let mut s = String::new();
    for x in settings {
        s.push_str(&x.to_line());
        s.push('\n');
    }
    s
}

/// What a curve-driven fan is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Left to the hardware.
    Auto,
    /// Driven as set.
    Driven,
    /// Full speed: a temperature reached [`SAFETY_TEMP`].
    Safety,
    /// Its temperature is unreadable: back to the hardware's control.
    NoSensor,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Driven => "driven",
            Self::Safety => "safety",
            Self::NoSensor => "nosensor",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "auto" => Self::Auto,
            "driven" => Self::Driven,
            "safety" => Self::Safety,
            "nosensor" => Self::NoSensor,
            _ => return None,
        })
    }
}

/// Runtime state of one driven fan: temperature hysteresis and ramp.
#[derive(Clone, Debug, Default)]
pub struct Driver {
    /// Temperature the curve is evaluated at (follows rises at once,
    /// falls with [`HYSTERESIS`] of delay).
    effective: Option<f32>,
    /// Speed being output, %.
    current: Option<f32>,
    /// Consecutive seconds without a temperature.
    missing: u32,
}

/// What to do with a fan this second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Output {
    /// Give it back to the hardware.
    Auto,
    /// This speed, %.
    Duty(f32),
}

impl Driver {
    /// One step (`dt` seconds since the previous). `temp`: the curve's
    /// temperature if readable; `safety`: see [`safety_state`].
    pub fn step(
        &mut self,
        mode: &Mode,
        temp: Option<f32>,
        safety: bool,
        dt: f32,
    ) -> (Output, Status) {
        let target = match mode {
            // The hardware's own control has its own safety.
            Mode::Auto => {
                self.reset();
                return (Output::Auto, Status::Auto);
            }
            Mode::Fixed(p) => f32::from(*p),
            Mode::Curve(c) => match temp.filter(|t| t.is_finite()) {
                Some(t) => {
                    self.missing = 0;
                    let eff = match self.effective {
                        Some(e) if t < e && e - t <= HYSTERESIS => e,
                        Some(e) if t < e => t + HYSTERESIS,
                        _ => t,
                    };
                    self.effective = Some(eff);
                    c.duty_at(eff)
                }
                None => {
                    self.missing = self.missing.saturating_add(1);
                    // Never driven yet (sensor missing from the start), or
                    // gone for good: the hardware's control. `missing`
                    // stays counted, so the fan does not come back to the
                    // last speed on the next second.
                    if (self.current.is_none() || self.missing >= SENSOR_GRACE) && !safety {
                        self.effective = None;
                        self.current = None;
                        return (Output::Auto, Status::NoSensor);
                    }
                    // Short gap: keep the last speed.
                    self.current.unwrap_or(100.0)
                }
            },
        };
        if safety {
            self.current = Some(100.0);
            return (Output::Duty(100.0), Status::Safety);
        }
        let target = target.clamp(0.0, 100.0);
        let next = match self.current {
            None => target,
            Some(c) if target > c => (c + RAMP_UP * dt).min(target),
            Some(c) => (c - RAMP_DOWN * dt).max(target),
        };
        self.current = Some(next);
        (Output::Duty(next), Status::Driven)
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Safety mode with its own hysteresis (one per controller, not per fan).
pub fn safety_state(active: bool, hottest: Option<f32>) -> bool {
    match hottest {
        Some(t) if t >= SAFETY_TEMP => true,
        Some(t) if t < SAFETY_RELEASE => false,
        Some(_) => active,
        None => active,
    }
}

/// `pct` (0–100) as an 8-bit PWM value.
pub fn pwm_of(pct: f32) -> u8 {
    (pct.clamp(0.0, 100.0) * 2.55 + 0.5) as u8
}

/// 8-bit PWM value as %.
pub fn pct_of(pwm: u8) -> f32 {
    f32::from(pwm) / 2.55
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(points: &[(u8, u8)]) -> Curve {
        Curve {
            source: "cpu".into(),
            points: points.to_vec(),
        }
    }

    #[test]
    fn interpolation() {
        let c = curve(&[(40, 20), (60, 40), (80, 100)]);
        assert_eq!(c.duty_at(20.0), 20.0);
        assert_eq!(c.duty_at(50.0), 30.0);
        assert_eq!(c.duty_at(70.0), 70.0);
        assert_eq!(c.duty_at(95.0), 100.0);
        assert!(c.validate().is_ok());
        assert!(curve(&[(40, 20)]).validate().is_err());
        assert!(curve(&[(40, 20), (40, 30)]).validate().is_err());
        assert!(curve(&[(40, 20), (50, 130)]).validate().is_err());
        for p in ["silent", "balanced", "performance"] {
            assert!(curve(&preset(p)).validate().is_ok(), "{p}");
        }
    }

    #[test]
    fn lines_round_trip() {
        let settings = vec![
            Setting {
                fan: "it8689-it87.2624-pwm1".into(),
                mode: Mode::Curve(curve(&[(30, 25), (70, 80), (85, 100)])),
            },
            Setting {
                fan: "it8689-it87.2624-pwm2".into(),
                mode: Mode::Fixed(45),
            },
            Setting {
                fan: "amdgpu-0000:03:00.0-pwm1".into(),
                mode: Mode::Auto,
            },
        ];
        let text = format_settings(&settings);
        assert_eq!(parse_settings(&text), settings);
        // Hostile or broken input is refused line by line.
        for bad in [
            "FAN ../../etc/passwd fixed 10",
            "FAN a fixed 101",
            "FAN a curve cpu 10:20",
            "FAN a curve cpu 10:20,5:30",
            "FAN a fixed 10 extra",
            "FAN a turbo",
            "RUN rm",
        ] {
            assert!(Setting::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(
            parse_settings("# c\nFAN a fixed 10\nFAN a fixed 20\n").len(),
            1
        );
    }

    #[test]
    fn ramp_and_hysteresis() {
        let mode = Mode::Curve(curve(&[(40, 20), (80, 100)]));
        let mut d = Driver::default();
        // First reading: straight to the curve.
        assert_eq!(d.step(&mode, Some(60.0), false, 1.0).0, Output::Duty(60.0));
        // A rise: at most RAMP_UP per second.
        assert_eq!(d.step(&mode, Some(80.0), false, 1.0).0, Output::Duty(80.0));
        assert_eq!(d.step(&mode, Some(80.0), false, 1.0).0, Output::Duty(100.0));
        // A small fall (within the hysteresis): no change.
        assert_eq!(d.step(&mode, Some(78.5), false, 1.0).0, Output::Duty(100.0));
        // A real fall: slowly down.
        assert_eq!(d.step(&mode, Some(50.0), false, 1.0).0, Output::Duty(96.0));
    }

    #[test]
    fn safety_and_missing_sensor() {
        let mode = Mode::Curve(curve(&[(40, 20), (80, 60)]));
        let mut d = Driver::default();
        assert_eq!(d.step(&mode, Some(40.0), false, 1.0).0, Output::Duty(20.0));
        // Safety: full speed at once, even for a fixed speed.
        assert_eq!(
            d.step(&mode, Some(40.0), true, 1.0),
            (Output::Duty(100.0), Status::Safety)
        );
        let mut f = Driver::default();
        assert_eq!(
            f.step(&Mode::Fixed(0), None, true, 1.0).0,
            Output::Duty(100.0)
        );
        // Sensor lost: the last speed for a moment, then the hardware.
        let mut d = Driver::default();
        d.step(&mode, Some(60.0), false, 1.0);
        assert_eq!(d.step(&mode, None, false, 1.0).0, Output::Duty(40.0));
        assert_eq!(d.step(&mode, None, false, 1.0).0, Output::Duty(40.0));
        assert_eq!(
            d.step(&mode, None, false, 1.0),
            (Output::Auto, Status::NoSensor)
        );
        // …and stays with the hardware while the sensor is missing.
        for _ in 0..5 {
            assert_eq!(
                d.step(&mode, None, false, 1.0),
                (Output::Auto, Status::NoSensor)
            );
        }
        // The sensor is back: the curve again, straight to its speed.
        assert_eq!(d.step(&mode, Some(60.0), false, 1.0).0, Output::Duty(40.0));
        // Missing from the start: the hardware at once, not full speed.
        let mut d = Driver::default();
        assert_eq!(
            d.step(&mode, None, false, 1.0),
            (Output::Auto, Status::NoSensor)
        );
        // Hysteresis of the safety mode.
        assert!(safety_state(false, Some(90.0)));
        assert!(safety_state(true, Some(87.0)));
        assert!(!safety_state(true, Some(84.0)));
        assert!(!safety_state(false, Some(87.0)));
        assert_eq!(pwm_of(100.0), 255);
        assert_eq!(pwm_of(50.0), 128);
        assert_eq!(pwm_of(-3.0), 0);
    }
}
