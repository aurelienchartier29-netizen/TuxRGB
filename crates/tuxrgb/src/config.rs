//! Persistent user settings (`~/.config/tuxrgb/config.json`).
//!
//! Everything the user sets is here: effect, colours and parameters, KDE
//! accent synchronisation, brightness, device layout, ARGB zone sizes,
//! appearance and behaviour. It is written atomically a moment after each
//! change, when the window closes, and on logout / shutdown (SIGTERM), and
//! re-applied as soon as TuxRGB starts (also minimised, at login).

use crate::paths;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use tuxrgb_core::{DeviceKey, EffectKind, EffectParams, Placement, Rgb};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccentSource {
    /// KDE Plasma / portal accent colour.
    Desktop,
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemeVariant {
    Expressive,
    TonalSpot,
    Vibrant,
    Fidelity,
    Neutral,
}

impl SchemeVariant {
    pub const ALL: [SchemeVariant; 5] = [
        SchemeVariant::Vibrant,
        SchemeVariant::TonalSpot,
        SchemeVariant::Expressive,
        SchemeVariant::Fidelity,
        SchemeVariant::Neutral,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Expressive => "Expressive",
            Self::TonalSpot => "Tonale",
            Self::Vibrant => "Vive",
            Self::Fidelity => "Fidèle",
            Self::Neutral => "Neutre",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MotionPref {
    System,
    Expressive,
    Reduced,
}

/// Interface language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LanguagePref {
    /// Follow LC_ALL / LC_MESSAGES / LANG (default).
    System,
    French,
    English,
}

impl LanguagePref {
    pub fn resolve(self) -> tuxrgb_core::i18n::Lang {
        use tuxrgb_core::i18n::Lang;
        match self {
            Self::System => tuxrgb_core::i18n::system_language(),
            Self::French => Lang::Fr,
            Self::English => Lang::En,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RendererPref {
    /// wgpu on the integrated / low-power GPU (default).
    GpuLowPower,
    GpuHighPerformance,
    /// tiny-skia CPU renderer.
    Software,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: ThemeMode,
    pub accent: AccentSource,
    pub custom_accent: Rgb,
    pub variant: SchemeVariant,
    pub motion: MotionPref,
    /// Speed of the interface animations (`1` = normal, `0.25..=3`).
    pub animation_speed: f32,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            accent: AccentSource::Desktop,
            custom_accent: Rgb::new(0x7c, 0x4d, 0xff),
            variant: SchemeVariant::Vibrant,
            motion: MotionPref::System,
            animation_speed: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Behavior {
    pub start_minimized: bool,
    pub close_to_tray: bool,
    pub autostart: bool,
    pub turn_off_on_exit: bool,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            start_minimized: false,
            close_to_tray: true,
            autostart: false,
            turn_off_on_exit: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerKind {
    /// OpenRGB embedded in TuxRGB (default, nothing to install).
    Embedded,
    /// An OpenRGB server started elsewhere.
    External,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSetting {
    pub kind: ServerKind,
    pub host: String,
    pub port: u16,
}

impl Default for ServerSetting {
    fn default() -> Self {
        Self {
            kind: ServerKind::Embedded,
            host: "127.0.0.1".into(),
            port: tuxrgb_openrgb::OPENRGB_DEFAULT_PORT,
        }
    }
}

impl ServerSetting {
    pub fn to_mode(&self) -> tuxrgb_openrgb::ServerMode {
        match self.kind {
            ServerKind::Embedded => tuxrgb_openrgb::ServerMode::Embedded,
            ServerKind::External => tuxrgb_openrgb::ServerMode::External {
                host: self.host.clone(),
                port: self.port,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub effect: EffectKind,
    pub brightness: f32,
    /// Global saturation, `0..=2` (0–200 %).
    pub saturation: f32,
    /// Crossfades between colours, effects and presets.
    pub smooth_transitions: bool,
    pub language: LanguagePref,
    pub output_enabled: bool,
    pub fps: u32,
    pub renderer: RendererPref,
    pub show_demo_devices: bool,
    pub screencast_token: Option<String>,
    pub disabled_devices: BTreeSet<DeviceKey>,
    pub appearance: Appearance,
    pub behavior: Behavior,
    pub server: ServerSetting,
    pub params: EffectParams,
    pub layout: BTreeMap<DeviceKey, Placement>,
    /// LED counts of addressable (ARGB) zones, re-applied if OpenRGB's own
    /// configuration ever loses them.
    pub zone_sizes: Vec<ZoneSize>,
    /// Information banners the user dismissed (desktop / distribution notes).
    pub dismissed_notices: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneSize {
    pub device: DeviceKey,
    pub zone: usize,
    /// Zone name at the time it was set (shown in logs, sanity check).
    pub name: String,
    pub leds: usize,
}

impl Config {
    /// Remembers the size of a zone (replaces a previous value).
    pub fn set_zone_size(&mut self, device: &DeviceKey, zone: usize, name: &str, leds: usize) {
        self.zone_sizes
            .retain(|z| !(z.device == *device && z.zone == zone));
        self.zone_sizes.push(ZoneSize {
            device: device.clone(),
            zone,
            name: name.to_owned(),
            leds,
        });
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            effect: EffectKind::RainbowWave,
            brightness: 1.0,
            saturation: 1.0,
            smooth_transitions: true,
            language: LanguagePref::System,
            output_enabled: true,
            fps: 60,
            renderer: RendererPref::GpuLowPower,
            show_demo_devices: true,
            appearance: Appearance::default(),
            behavior: Behavior::default(),
            server: ServerSetting::default(),
            screencast_token: None,
            params: EffectParams::default(),
            disabled_devices: BTreeSet::new(),
            layout: BTreeMap::new(),
            zone_sizes: Vec::new(),
            dismissed_notices: BTreeSet::new(),
        }
    }
}

/// 2: saturation became global (0–200 %), moved out of the effects.
pub const CONFIG_VERSION: u32 = 2;

impl Config {
    /// Upgrades a configuration written by an older TuxRGB.
    pub fn migrate(&mut self) {
        if self.version < 2 {
            // The per-effect "Saturation" (0–100 %) of Static, Breathing,
            // Rainbow wave and Colour cycle becomes the global setting.
            let legacy = [
                EffectKind::Static,
                EffectKind::Breathing,
                EffectKind::RainbowWave,
                EffectKind::ColorCycle,
            ];
            if legacy.contains(&self.effect)
                && let Some(tuxrgb_core::ParamValue::Number(v)) = self
                    .params
                    .0
                    .get(&self.effect)
                    .and_then(|m| m.get(tuxrgb_core::effects::SATURATION_KEY))
            {
                self.saturation = v.clamp(0.0, 1.0);
            }
            for kind in legacy {
                if let Some(m) = self.params.0.get_mut(&kind) {
                    m.remove(tuxrgb_core::effects::SATURATION_KEY);
                }
            }
            tracing::info!(
                saturation = self.saturation,
                "configuration upgraded to version 2"
            );
        }
        self.version = CONFIG_VERSION;
    }

    pub fn load() -> Self {
        let p = paths::paths();
        let mut c = match std::fs::read_to_string(&p.config_file) {
            Ok(s) => match serde_json::from_str::<Config>(&s) {
                Ok(c) => c,
                Err(e) => {
                    note(format!(
                        "invalid config ({e}), keeping a backup and using defaults"
                    ));
                    let _ = std::fs::copy(&p.config_file, p.config_file.with_extension("json.bak"));
                    Self::default()
                }
            },
            Err(_) => Self::migrate_toml().unwrap_or_default(),
        };
        c.migrate();
        c.fps = c.fps.clamp(10, 144);
        c.brightness = c.brightness.clamp(0.0, 1.0);
        c.saturation = c.saturation.clamp(0.0, 2.0);
        c
    }

    /// Loads the configuration once, at the very start of the process (the
    /// language and renderer depend on it); [`Config::initial`] then hands
    /// the same values to the interface, so both always agree.
    pub fn load_initial() -> Self {
        let c = Self::load();
        if let Ok(mut slot) = INITIAL.lock() {
            *slot = Some(c.clone());
        }
        c
    }

    /// The configuration read by [`Config::load_initial`] (read again if
    /// there was none).
    pub fn initial() -> Self {
        INITIAL
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .unwrap_or_else(Self::load)
    }

    /// What went wrong while loading, for the log (loading happens before
    /// logging starts).
    pub fn take_load_notes() -> Vec<String> {
        LOAD_NOTES
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default()
    }

    /// TuxRGB ≤ 1.1 stored `config.toml`: convert it once.
    fn migrate_toml() -> Option<Self> {
        let p = paths::paths();
        let text = std::fs::read_to_string(&p.legacy_config_file).ok()?;
        let c: Config = toml::from_str(&text).ok()?;
        // Synchronous: this runs before any thread is started (see main).
        let json = serde_json::to_string_pretty(&c).ok()? + "\n";
        if paths::write_atomic(&p.config_file, &json).is_ok() {
            let _ = std::fs::rename(
                &p.legacy_config_file,
                p.legacy_config_file.with_extension("toml.migrated"),
            );
            tracing::info!("configuration migrated to {}", p.config_file.display());
        }
        Some(c)
    }

    /// Saves in the background (the interface never waits for the disk).
    pub fn save(&self) -> Result<(), String> {
        // The screenshot tour changes effects and pages for its captures: it
        // never writes the user's settings.
        if crate::args().screenshot_tour.is_some() {
            return Ok(());
        }
        let s = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        paths::write_later(paths::paths().config_file.clone(), s + "\n");
        Ok(())
    }

    /// Saves and waits until it is on disk (quit, shutdown, sleep).
    pub fn save_now(&self) -> Result<(), String> {
        if crate::args().screenshot_tour.is_some() {
            return Ok(());
        }
        let s = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        paths::write_now(
            paths::paths().config_file.clone(),
            s + "\n",
            std::time::Duration::from_secs(3),
        )
        .map_err(|e| e.to_string())
    }
}

static INITIAL: std::sync::Mutex<Option<Config>> = std::sync::Mutex::new(None);
static LOAD_NOTES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn note(msg: String) {
    tracing::warn!("{msg}");
    if let Ok(mut v) = LOAD_NOTES.lock() {
        v.push(msg);
    }
}

/// First-run state, kept separately so "reset hardware access" can wipe it
/// without touching the user's effects and layout.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Onboarding {
    pub completed: bool,
    pub completed_with: String,
    pub hardware_configured: bool,
    pub skipped_hardware: bool,
}

impl Onboarding {
    pub fn load() -> Self {
        std::fs::read_to_string(&paths::paths().onboarding_file)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let s = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        paths::write_atomic(&paths::paths().onboarding_file, &s).map_err(|e| e.to_string())
    }

    pub fn reset() {
        let _ = std::fs::remove_file(&paths::paths().onboarding_file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tuxrgb_core::ParamValue;

    #[test]
    fn config_roundtrip() {
        let mut c = Config::default();
        c.params.set(
            EffectKind::Static,
            "color",
            ParamValue::Color(Rgb::new(9, 8, 7)),
        );
        c.layout.insert(
            DeviceKey::new("ASUS", "AURA DRAM", "I2C: /dev/i2c-1, address 0x71", ""),
            Placement::new(0.1, 0.2, 0.1, 0.2),
        );
        c.disabled_devices.insert(DeviceKey("x|y|z|".into()));
        c.set_zone_size(&DeviceKey("Gigabyte|B650|HID|".into()), 0, "ARGB_V2_1", 24);
        c.set_zone_size(&DeviceKey("Gigabyte|B650|HID|".into()), 0, "ARGB_V2_1", 30);
        assert_eq!(c.zone_sizes.len(), 1);
        let s = serde_json::to_string_pretty(&c).unwrap();
        let back: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(back, c);
        // Old TOML files still parse (migration).
        let t = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&t).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn legacy_saturation_becomes_global() {
        let mut c: Config = serde_json::from_str(
            r##"{ "version": 1, "effect": "Static",
                 "params": { "Static": { "saturation": 0.4, "color": "#ff0000" },
                             "RainbowWave": { "saturation": 0.8 } } }"##,
        )
        .unwrap();
        c.migrate();
        assert_eq!(c.version, CONFIG_VERSION);
        assert!((c.saturation - 0.4).abs() < 1e-6);
        assert!(!c.params.0[&EffectKind::Static].contains_key("saturation"));
        assert!(!c.params.0[&EffectKind::RainbowWave].contains_key("saturation"));
        assert_eq!(
            c.params.color(EffectKind::Static, "color"),
            Rgb::new(255, 0, 0)
        );
    }

    #[test]
    fn partial_config_uses_defaults() {
        let c: Config = serde_json::from_str(r#"{ "brightness": 0.5 }"#).unwrap();
        assert_eq!(c.brightness, 0.5);
        assert_eq!(c.fps, 60);
    }
}
