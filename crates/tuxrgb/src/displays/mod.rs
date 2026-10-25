//! "Afficheurs" (TuxRGB 3.1): the screens of AIO coolers and the digital
//! displays of air coolers and cases, fed with sensor values, a text or a
//! picture.
//!
//! One background thread owns every display: it looks for them (USB HID,
//! see [`hardware_display`]), reads the latest sensor sample (the same
//! sampler as the "Moniteur matériel" effect), renders the full-colour
//! screens ([`render`]) and sends each display what it shows at its own
//! pace. The settings page gets a snapshot of every display, with a preview
//! of what it shows.

pub mod corsair;
pub mod deepcool;
pub mod hardware_display;
pub mod hid;
pub mod kraken;
pub mod render;
pub mod usb;

pub use hardware_display::{Capabilities, Content, DisplaySettings, Metric, Picture, Surface};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use hardware_display::{Frame, HardwareDisplay, Model};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tuxrgb_core::signal::HardwareSample;
use tuxrgb_core::{Rgb, SharedInputs};

/// Settings of every display, saved in the configuration.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaysConfig {
    /// Per display key ([`hid::HidInfo::key`], or `demo:…`).
    pub displays: BTreeMap<String, DisplaySettings>,
    /// Temperatures in °F.
    pub fahrenheit: bool,
}

impl DisplaysConfig {
    /// Settings of a display (its first settings when never changed).
    pub fn settings_of(&self, key: &str, caps: &Capabilities) -> DisplaySettings {
        self.displays
            .get(key)
            .cloned()
            .unwrap_or_else(|| DisplaySettings::for_display(caps))
    }
}

/// One display, as the settings page shows it.
#[derive(Clone, Debug)]
pub struct DisplayState {
    pub key: String,
    pub vendor: String,
    pub name: String,
    pub caps: Capabilities,
    /// Plugged in (or a demo display).
    pub connected: bool,
    /// A virtual display of the demo setup.
    pub demo: bool,
    /// Enabled and receiving what it shows.
    pub driving: bool,
    pub error: Option<String>,
    /// The error is a missing permission (udev rules to install).
    pub access_denied: bool,
    /// The values shown right now.
    pub shown: Vec<(Metric, Option<f32>)>,
    /// What it shows (upright), when previews are on.
    pub preview: Option<Arc<Picture>>,
}

pub enum Command {
    Configure {
        config: DisplaysConfig,
        /// Virtual displays of the demo setup (no hardware found).
        demo: bool,
        accent: Rgb,
    },
    Rescan,
    /// The settings page is open: previews and live values.
    Previews(bool),
    /// Let go of every display (quit, shutdown); `done` when finished.
    Release(Option<Sender<()>>),
    Quit,
}

/// Handle on the displays thread.
pub struct DisplayService {
    tx: Sender<Command>,
}

impl DisplayService {
    pub fn spawn(
        inputs: SharedInputs,
        events: impl Fn(Vec<DisplayState>) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (tx, rx) = crossbeam_channel::bounded(64);
        std::thread::Builder::new()
            .name("tuxrgb-displays".into())
            .spawn(move || Manager::new(inputs, Box::new(events)).run(rx))?;
        Ok(Self { tx })
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.try_send(cmd);
    }

    /// Lets go of every display (their own screens come back), waiting at
    /// most `budget` (a display that hangs never delays a shutdown).
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

impl Drop for DisplayService {
    fn drop(&mut self) {
        let _ = self.tx.try_send(Command::Quit);
    }
}

struct Slot {
    key: String,
    vendor: String,
    name: String,
    model: Option<&'static Model>,
    info: Option<hid::HidInfo>,
    caps: Capabilities,
    demo: bool,
    connected: bool,
    driver: Option<Box<dyn HardwareDisplay>>,
    error: Option<String>,
    access_denied: bool,
    next: Instant,
    since: Instant,
    images: render::ImageCache,
    shown: Vec<(Metric, Option<f32>)>,
    preview: Option<Arc<Picture>>,
    /// Failed opens in a row (retried less and less often).
    failures: u32,
    retry_at: Instant,
}

impl Slot {
    fn new(key: String, vendor: &str, name: &str, caps: Capabilities, demo: bool) -> Self {
        let now = Instant::now();
        Self {
            key,
            vendor: vendor.to_owned(),
            name: name.to_owned(),
            model: None,
            info: None,
            caps,
            demo,
            connected: demo,
            driver: None,
            error: None,
            access_denied: false,
            next: now,
            since: now,
            images: render::ImageCache::default(),
            shown: Vec::new(),
            preview: None,
            failures: 0,
            retry_at: now,
        }
    }

    fn state(&self, settings: &DisplaySettings) -> DisplayState {
        DisplayState {
            key: self.key.clone(),
            vendor: self.vendor.clone(),
            name: self.name.clone(),
            caps: self.caps.clone(),
            connected: self.connected,
            demo: self.demo,
            driving: settings.enabled && self.connected && (self.demo || self.driver.is_some()),
            error: self.error.clone(),
            access_denied: self.access_denied,
            shown: self.shown.clone(),
            preview: self.preview.clone(),
        }
    }
}

/// The values a display shows now: its choice (those it can show), by
/// pages of what fits at once, each page `cycle_s` seconds.
pub fn values_now(
    settings: &DisplaySettings,
    caps: &Capabilities,
    elapsed: Duration,
) -> Vec<Metric> {
    if caps.fixed {
        return caps.metrics.to_vec();
    }
    let mut chosen: Vec<Metric> = Vec::new();
    for m in &settings.metrics {
        if caps.metrics.contains(m) && !chosen.contains(m) {
            chosen.push(*m);
        }
    }
    if chosen.is_empty() {
        chosen = caps
            .metrics
            .iter()
            .copied()
            .take(caps.at_once.max(1))
            .collect();
    }
    let per = caps.at_once.max(1);
    if chosen.len() <= per {
        return chosen;
    }
    let pages = chosen.len().div_ceil(per);
    let page = (elapsed.as_secs() / u64::from(settings.cycle_s.max(1))) as usize % pages;
    chosen.iter().copied().skip(page * per).take(per).collect()
}

/// The virtual displays of the demo setup.
fn demo_slots() -> Vec<Slot> {
    let mut screen = Capabilities::screen(320, 320, true);
    screen.builtin = None;
    let digits = deepcool::capabilities(deepcool::Protocol::Ak);
    vec![
        Slot::new(
            "demo:aio".into(),
            "TuxRGB Demo",
            "Écran de watercooling AIO",
            screen,
            true,
        ),
        Slot::new(
            "demo:digits".into(),
            "TuxRGB Demo",
            "Afficheur numérique (ventirad)",
            digits,
            true,
        ),
    ]
}

/// The settings of the demo displays until the user changes them: they
/// show something as soon as the page opens.
fn demo_settings(key: &str, caps: &Capabilities) -> DisplaySettings {
    let mut s = DisplaySettings::for_display(caps);
    s.enabled = true;
    if key == "demo:aio" {
        s.metrics = vec![Metric::CpuTemp, Metric::CpuLoad];
    }
    s
}

/// What a demo display shows when no sensor is readable (the setup is
/// virtual too).
fn demo_sample(t: f32) -> HardwareSample {
    let wave =
        |period: f32, phase: f32| 0.5 + 0.5 * (t / period * std::f32::consts::TAU + phase).sin();
    HardwareSample {
        cpu_temp: Some(52.0 + 9.0 * wave(23.0, 0.0)),
        cpu_load: Some(0.18 + 0.30 * wave(11.0, 1.0)),
        cpu_power: Some(48.0 + 30.0 * wave(11.0, 1.0)),
        cpu_freq: Some(4200.0 + 600.0 * wave(7.0, 2.0)),
        gpu_temp: Some(47.0 + 12.0 * wave(31.0, 2.0)),
        gpu_load: Some(0.10 + 0.55 * wave(17.0, 0.5)),
        gpu_power: Some(60.0 + 150.0 * wave(17.0, 0.5)),
        gpu_freq: Some(1900.0 + 600.0 * wave(17.0, 0.5)),
        mem_used: Some(0.41),
        mem_used_gib: Some(13.1),
        mem_total_gib: Some(32.0),
        ..HardwareSample::default()
    }
}

/// `real`, its missing values taken from `stand_in`.
fn fill_in(real: &HardwareSample, stand_in: &HardwareSample) -> HardwareSample {
    HardwareSample {
        cpu_temp: real.cpu_temp.or(stand_in.cpu_temp),
        cpu_load: real.cpu_load.or(stand_in.cpu_load),
        cpu_power: real.cpu_power.or(stand_in.cpu_power),
        cpu_freq: real.cpu_freq.or(stand_in.cpu_freq),
        gpu_temp: real.gpu_temp.or(stand_in.gpu_temp),
        gpu_load: real.gpu_load.or(stand_in.gpu_load),
        gpu_power: real.gpu_power.or(stand_in.gpu_power),
        gpu_freq: real.gpu_freq.or(stand_in.gpu_freq),
        mem_used: real.mem_used.or(stand_in.mem_used),
        mem_used_gib: real.mem_used_gib.or(stand_in.mem_used_gib),
        mem_total_gib: real.mem_total_gib.or(stand_in.mem_total_gib),
        captured_at: real.captured_at,
    }
}

type Events = Box<dyn Fn(Vec<DisplayState>) + Send>;

struct Manager {
    inputs: SharedInputs,
    events: Events,
    config: DisplaysConfig,
    demo: bool,
    accent: Rgb,
    previews: bool,
    slots: Vec<Slot>,
    next_scan: Instant,
    last_event: Instant,
    dirty: bool,
    started: Instant,
}

/// Looking again for displays: often while the page is open, otherwise
/// rarely (hot-plugging is rare; a scan costs a directory read).
const SCAN_OPEN: Duration = Duration::from_secs(3);
const SCAN_IDLE: Duration = Duration::from_secs(15);
const EVENT_EVERY: Duration = Duration::from_millis(500);

impl Manager {
    fn new(inputs: SharedInputs, events: Events) -> Self {
        let now = Instant::now();
        Self {
            inputs,
            events,
            config: DisplaysConfig::default(),
            demo: false,
            accent: Rgb::new(0, 160, 255),
            previews: false,
            slots: Vec::new(),
            next_scan: now,
            last_event: now,
            dirty: true,
            started: now,
        }
    }

    fn settings(&self, slot: &Slot) -> DisplaySettings {
        match self.config.displays.get(&slot.key) {
            Some(s) => s.clone(),
            None if slot.demo => demo_settings(&slot.key, &slot.caps),
            None => DisplaySettings::for_display(&slot.caps),
        }
    }

    fn run(mut self, rx: Receiver<Command>) {
        tracing::debug!("displays thread started");
        loop {
            let now = Instant::now();
            let soonest = self
                .slots
                .iter()
                .filter(|s| s.connected)
                .map(|s| s.next)
                .chain(std::iter::once(self.next_scan))
                .min()
                .unwrap_or(now + Duration::from_secs(30));
            let wait = soonest
                .saturating_duration_since(now)
                .clamp(Duration::from_millis(10), Duration::from_secs(30));
            match rx.recv_timeout(wait) {
                Ok(cmd) => {
                    if !self.apply(cmd) {
                        break;
                    }
                    while let Ok(cmd) = rx.try_recv() {
                        if !self.apply(cmd) {
                            self.release_all();
                            tracing::debug!("displays thread stopped");
                            return;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if Instant::now() >= self.next_scan {
                self.scan();
            }
            self.refresh();
            if self.dirty || (self.previews && self.last_event.elapsed() >= EVENT_EVERY) {
                self.emit();
            }
        }
        self.release_all();
        tracing::debug!("displays thread stopped");
    }

    /// Applies a command; `false` to stop.
    fn apply(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::Configure {
                config,
                demo,
                accent,
            } => {
                let demo_changed = demo != self.demo;
                self.config = config;
                self.demo = demo;
                self.accent = accent;
                for s in &mut self.slots {
                    // New settings: shown at once.
                    s.next = Instant::now();
                }
                if demo_changed {
                    self.next_scan = Instant::now();
                }
                self.dirty = true;
            }
            Command::Rescan => {
                for s in &mut self.slots {
                    s.failures = 0;
                    s.retry_at = Instant::now();
                }
                self.next_scan = Instant::now();
            }
            Command::Previews(on) => {
                if on != self.previews {
                    self.previews = on;
                    if on {
                        self.next_scan = Instant::now();
                        for s in &mut self.slots {
                            s.next = Instant::now();
                        }
                    } else {
                        for s in &mut self.slots {
                            s.preview = None;
                        }
                    }
                    self.dirty = true;
                }
            }
            Command::Release(done) => {
                self.release_all();
                if let Some(done) = done {
                    let _ = done.send(());
                }
            }
            Command::Quit => return false,
        }
        true
    }

    fn release_all(&mut self) {
        for s in &mut self.slots {
            if let Some(mut d) = s.driver.take()
                && let Err(e) = d.release()
            {
                tracing::debug!("display {}: release: {e}", s.key);
            }
        }
    }

    /// Looks for displays (and the demo ones).
    fn scan(&mut self) {
        self.next_scan = Instant::now() + if self.previews { SCAN_OPEN } else { SCAN_IDLE };
        let mut found: BTreeMap<String, (hid::HidInfo, &'static Model)> = BTreeMap::new();
        for info in hid::enumerate() {
            let Some(model) = hardware_display::identify(info.vendor_id, info.product_id) else {
                continue;
            };
            let key = info.key();
            // Several HID interfaces of one device: the first one.
            let better = found
                .get(&key)
                .is_none_or(|(old, _)| info.interface.unwrap_or(0) < old.interface.unwrap_or(0));
            if better {
                found.insert(key, (info, model));
            }
        }
        let any_real = !found.is_empty();
        // Real displays.
        for (key, (info, model)) in &found {
            match self.slots.iter_mut().find(|s| s.key == *key) {
                Some(slot) => {
                    if !slot.connected || slot.info.as_ref().map(|i| &i.path) != Some(&info.path) {
                        slot.driver = None;
                        slot.connected = true;
                        slot.info = Some(info.clone());
                        slot.retry_at = Instant::now();
                        self.dirty = true;
                    }
                }
                None => {
                    let caps = hardware_display::capabilities(model);
                    let mut slot = Slot::new(key.clone(), model.vendor, model.name, caps, false);
                    slot.model = Some(model);
                    slot.info = Some(info.clone());
                    slot.connected = true;
                    tracing::info!("display found: {} {} ({key})", model.vendor, model.name);
                    self.slots.push(slot);
                    self.dirty = true;
                }
            }
        }
        for slot in self.slots.iter_mut().filter(|s| !s.demo) {
            if slot.connected && !found.contains_key(&slot.key) {
                tracing::info!("display gone: {}", slot.key);
                slot.connected = false;
                slot.driver = None;
                self.dirty = true;
            }
        }
        // Demo displays: only for the demo setup, while nothing real is there.
        let want_demo = self.demo && !any_real;
        let has_demo = self.slots.iter().any(|s| s.demo);
        if want_demo && !has_demo {
            self.slots.extend(demo_slots());
            self.dirty = true;
        } else if !want_demo && has_demo {
            self.slots.retain(|s| !s.demo);
            self.dirty = true;
        }
        // Unplugged displays that were never set up are forgotten.
        let config = &self.config;
        self.slots
            .retain(|s| s.connected || s.demo || config.displays.contains_key(&s.key));
    }

    /// Sends every display due what it shows now.
    fn refresh(&mut self) {
        let now = Instant::now();
        let real_sample = self.inputs.latest_hardware();
        let fahrenheit = self.config.fahrenheit;
        let look = render::Look {
            accent: self.accent,
        };
        let previews = self.previews;
        let elapsed = now.duration_since(self.started);
        for i in 0..self.slots.len() {
            if !self.slots[i].connected || now < self.slots[i].next {
                continue;
            }
            let settings = self.settings(&self.slots[i]);
            let slot = &mut self.slots[i];
            slot.next = now + settings.interval().max(slot.caps.min_interval);
            // Open / close the driver as the switch says.
            if !slot.demo {
                if settings.enabled && slot.driver.is_none() && now >= slot.retry_at {
                    if let (Some(model), Some(info)) = (slot.model, slot.info.as_ref()) {
                        match hardware_display::open(model, info) {
                            Ok(d) => {
                                // The driver knows best (no USB access for
                                // pictures, …).
                                slot.caps = d.capabilities().clone();
                                slot.driver = Some(d);
                                slot.error = None;
                                slot.access_denied = false;
                                slot.failures = 0;
                                slot.since = now;
                                self.dirty = true;
                            }
                            Err(e) => {
                                slot.access_denied =
                                    e.kind() == std::io::ErrorKind::PermissionDenied;
                                slot.error = Some(open_error(&e));
                                slot.failures += 1;
                                slot.retry_at =
                                    now + Duration::from_secs(5 * u64::from(slot.failures.min(12)));
                                self.dirty = true;
                            }
                        }
                    }
                } else if !settings.enabled && slot.driver.is_some() {
                    if let Some(mut d) = slot.driver.take() {
                        let _ = d.release();
                    }
                    self.dirty = true;
                }
            }
            let driving = settings.enabled && (slot.demo || slot.driver.is_some());
            // Nothing to send and nobody looking (a demo display only
            // exists on the page): no rendering at all.
            if (!driving || slot.demo) && !previews {
                continue;
            }
            let sample = match (&real_sample, slot.demo) {
                (Some(s), false) => s.clone(),
                // Demo displays: the real values, the missing ones made up.
                (Some(s), true) => fill_in(s, &demo_sample(elapsed.as_secs_f32())),
                (None, true) => demo_sample(elapsed.as_secs_f32()),
                (None, false) => HardwareSample::default(),
            };
            let metrics = if settings.content == Content::Metrics || !slot.caps.is_screen() {
                values_now(&settings, &slot.caps, now.duration_since(slot.since))
            } else {
                Vec::new()
            };
            let shown: Vec<(Metric, Option<f32>)> =
                metrics.iter().map(|m| (*m, m.value(&sample))).collect();
            if shown != slot.shown {
                slot.shown = shown.clone();
                self.dirty |= previews;
            }
            // Full-colour screens: render once, for the screen and its
            // preview.
            let mut upright: Option<Arc<Picture>> = None;
            if let hardware_display::Surface::Screen { width, height, .. } = slot.caps.surface {
                let wants_picture = slot.caps.image
                    && settings.content != Content::Builtin
                    && (driving || previews);
                if wants_picture {
                    let values: Vec<render::Shown> = shown
                        .iter()
                        .map(|(m, v)| render::Shown {
                            metric: *m,
                            value: *v,
                        })
                        .collect();
                    let picture = render::screen(
                        width,
                        height,
                        &settings,
                        &values,
                        fahrenheit,
                        look,
                        &mut slot.images,
                    );
                    upright = Some(Arc::new(picture));
                }
            }
            if let Some(driver) = slot.driver.as_mut().filter(|_| settings.enabled) {
                let picture = upright.as_ref().map(|p| {
                    let quarter = (settings.rotation / 90) % 4;
                    if quarter == 0 {
                        p.clone()
                    } else {
                        Arc::new(p.rotated(quarter))
                    }
                });
                let frame = Frame {
                    settings: &settings,
                    sample: &sample,
                    metrics: &metrics,
                    fahrenheit,
                    picture,
                };
                if let Err(e) = driver.show(&frame) {
                    tracing::warn!("display {}: {e}", slot.key);
                    slot.error = Some(e.to_string());
                    slot.driver = None;
                    slot.failures += 1;
                    slot.retry_at = now + Duration::from_secs(5 * u64::from(slot.failures.min(12)));
                    // It may be gone: the next scan says.
                    self.next_scan = self.next_scan.min(now + Duration::from_secs(1));
                    self.dirty = true;
                } else if slot.error.take().is_some() {
                    self.dirty = true;
                }
            }
            if previews {
                slot.preview = Some(match slot.caps.surface {
                    hardware_display::Surface::Screen { round, .. } => {
                        let mut p = match &upright {
                            Some(p) => (**p).clone(),
                            None => {
                                // Its own screen (Kraken): its name as a
                                // stand-in.
                                let text = DisplaySettings {
                                    content: Content::Text,
                                    text: crate::i18n::t(slot.caps.builtin.unwrap_or(""))
                                        .to_owned(),
                                    ..settings.clone()
                                };
                                render::screen(
                                    320,
                                    320,
                                    &text,
                                    &[],
                                    fahrenheit,
                                    look,
                                    &mut slot.images,
                                )
                            }
                        };
                        if round {
                            render::round_mask(&mut p);
                        }
                        Arc::new(p)
                    }
                    hardware_display::Surface::Matrix => {
                        let r = deepcool::Readings::of(&Frame {
                            settings: &settings,
                            sample: &sample,
                            metrics: &metrics,
                            fahrenheit,
                            picture: None,
                        });
                        let values: Vec<_> = metrics
                            .iter()
                            .take(2)
                            .map(|m| deepcool::lp_value(*m, &r))
                            .collect();
                        Arc::new(render::matrix_preview(
                            &deepcool::lp_matrix(&values, 0),
                            look,
                        ))
                    }
                    hardware_display::Surface::Digits => {
                        let values: Vec<render::Shown> = shown
                            .iter()
                            .map(|(m, v)| render::Shown {
                                metric: *m,
                                value: *v,
                            })
                            .collect();
                        Arc::new(render::digits_preview(
                            &values,
                            sample.cpu_load.map(|l| l * 100.0),
                            fahrenheit,
                            look,
                        ))
                    }
                });
            }
        }
    }

    fn emit(&mut self) {
        let states = self
            .slots
            .iter()
            .map(|s| s.state(&self.settings(s)))
            .collect();
        (self.events)(states);
        self.dirty = false;
        self.last_event = Instant::now();
    }
}

/// What the user can do about a display that cannot be opened.
fn open_error(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => {
            if cfg!(windows) {
                crate::i18n::t("Accès refusé : un autre logiciel (NZXT CAM, iCUE, DeepCool) l'utilise peut-être.")
                    .to_owned()
            } else {
                crate::i18n::t(
                    "Accès refusé : installez l'accès matériel (Réglages → Accès matériel).",
                )
                .to_owned()
            }
        }
        _ => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_alternate_by_pages() {
        let ak = deepcool::capabilities(deepcool::Protocol::Ak);
        let s = DisplaySettings {
            metrics: vec![Metric::CpuTemp, Metric::CpuLoad, Metric::GpuTemp],
            cycle_s: 5,
            ..DisplaySettings::default()
        };
        // The GPU is not on that display: temperature and load alternate.
        assert_eq!(
            values_now(&s, &ak, Duration::from_secs(1)),
            vec![Metric::CpuTemp]
        );
        assert_eq!(
            values_now(&s, &ak, Duration::from_secs(6)),
            vec![Metric::CpuLoad]
        );
        assert_eq!(
            values_now(&s, &ak, Duration::from_secs(11)),
            vec![Metric::CpuTemp]
        );
        // A screen shows four at once.
        let screen = Capabilities::screen(320, 320, true);
        let s = DisplaySettings {
            metrics: vec![Metric::CpuTemp, Metric::GpuTemp, Metric::Ram],
            ..DisplaySettings::default()
        };
        assert_eq!(values_now(&s, &screen, Duration::ZERO).len(), 3);
        // Fixed displays show what they show.
        let lq = deepcool::capabilities(deepcool::Protocol::Lq);
        assert_eq!(values_now(&s, &lq, Duration::ZERO).len(), 4);
        // Nothing chosen that it can show: its first values.
        let none = DisplaySettings {
            metrics: vec![Metric::Time],
            ..DisplaySettings::default()
        };
        assert_eq!(
            values_now(&none, &ak, Duration::ZERO),
            vec![Metric::CpuTemp]
        );
    }

    #[test]
    fn demo_displays_show_and_preview() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let service = DisplayService::spawn(SharedInputs::default(), move |s| {
            let _ = tx.send(s);
        })
        .unwrap();
        service.send(Command::Configure {
            config: DisplaysConfig::default(),
            demo: true,
            accent: Rgb::new(0, 160, 255),
        });
        service.send(Command::Previews(true));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut last = Vec::new();
        while Instant::now() < deadline {
            if let Ok(s) = rx.recv_timeout(Duration::from_millis(200)) {
                last = s;
                if last.len() == 2
                    && last
                        .iter()
                        .all(|d| d.preview.is_some() && !d.shown.is_empty())
                {
                    break;
                }
            }
        }
        assert_eq!(last.len(), 2, "two demo displays");
        for d in &last {
            assert!(d.demo && d.connected && d.driving, "{d:?}");
            assert!(d.preview.is_some(), "{} preview", d.key);
            assert!(d.shown.iter().all(|(_, v)| v.is_some()), "{:?}", d.shown);
        }
        // Switched off: no longer driven.
        let mut config = DisplaysConfig::default();
        let caps = last[1].caps.clone();
        let mut s = DisplaySettings::for_display(&caps);
        s.enabled = false;
        config.displays.insert(last[1].key.clone(), s);
        service.send(Command::Configure {
            config,
            demo: true,
            accent: Rgb::new(0, 160, 255),
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut off = false;
        while Instant::now() < deadline && !off {
            if let Ok(s) = rx.recv_timeout(Duration::from_millis(200)) {
                off = s.iter().any(|d| d.key == last[1].key && !d.driving);
            }
        }
        assert!(off);
        assert!(service.release(Duration::from_millis(500)));
    }
}
