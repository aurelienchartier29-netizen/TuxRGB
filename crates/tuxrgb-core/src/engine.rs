//! The effect engine: one dedicated thread, fixed-rate when an effect is
//! animated, fully asleep (blocked on its command channel) otherwise.
//!
//! Frames are published to [`FrameSink`]s (device I/O, GUI preview) as
//! `Arc<Frame>`, so slow consumers never block rendering: they simply pick up
//! the latest frame when they are ready.

use crate::color::{Rgb, SaturationCache};
use crate::device::DeviceKind;
use crate::effects::{self, EffectKind, EffectParams, EffectState, FrameContext};
use crate::layout::{LedPoint, Scene};
use crate::lighting::PartLighting;
use crate::signal::SharedInputs;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// LEDs of one device for one frame (OpenRGB order).
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceFrame {
    /// OpenRGB SDK controller id.
    pub id: u32,
    pub colors: Vec<Rgb>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub seq: u64,
    pub devices: Vec<DeviceFrame>,
    /// The effect over the whole canvas (GUI preview only, see
    /// [`EngineCommand::SetBackdrop`]).
    pub backdrop: Option<Arc<Backdrop>>,
}

/// Grid of the full-canvas rendering (16:9, like the canvas).
pub const BACKDROP_W: usize = 48;
pub const BACKDROP_H: usize = 27;

/// The current effect evaluated on a coarse grid covering the whole canvas,
/// with the same brightness, saturation and crossfades as the LEDs. The GUI
/// blurs it into the soft background of its previews.
#[derive(Clone, Debug, PartialEq)]
pub struct Backdrop {
    pub width: usize,
    pub height: usize,
    /// Row-major, `width × height`.
    pub colors: Vec<Rgb>,
}

/// Engine-side state of the backdrop: its own grid of virtual LEDs and its
/// own effect state (particles, smoothing), independent from the devices.
struct BackdropRender {
    points: Vec<LedPoint>,
    state: EffectState,
    buffer: Vec<Rgb>,
    displayed: Vec<[f32; 3]>,
}

impl BackdropRender {
    fn new() -> Self {
        let (w, h) = (BACKDROP_W, BACKDROP_H);
        let mut points = Vec::with_capacity(w * h);
        for j in 0..h {
            for i in 0..w {
                let x = (i as f32 + 0.5) / w as f32;
                let y = (j as f32 + 0.5) / h as f32;
                // One virtual device spanning the canvas: "per device"
                // effects (spectrum, gauges) draw their pattern across it.
                points.push(LedPoint {
                    device: 0,
                    led: j * w + i,
                    x,
                    y,
                    u: x,
                    v: y,
                    rx: 0.5 / w as f32,
                    ry: 0.5 / h as f32,
                });
            }
        }
        Self {
            buffer: vec![Rgb::BLACK; points.len()],
            displayed: vec![[0.0; 3]; points.len()],
            points,
            state: EffectState::default(),
        }
    }
}

/// Global saturation (0–200 %), then brightness, in software.
fn grade(buffer: &mut [Rgb], saturation: f32, level: f32, cache: &mut SaturationCache) {
    if (saturation - 1.0).abs() > 0.005 {
        for c in buffer.iter_mut() {
            *c = cache.saturate(*c, saturation);
        }
    }
    if level < 0.999 {
        for c in buffer.iter_mut() {
            *c = c.dim(level);
        }
    }
}

/// Blends `buffer` from `from` (fade start) by `k`, or just records it.
fn crossfade(displayed: &mut [[f32; 3]], buffer: &mut [Rgb], from: &[[f32; 3]], k: Option<f32>) {
    match k {
        Some(k) if from.len() == buffer.len() => {
            for ((d, c), from) in displayed.iter_mut().zip(buffer.iter_mut()).zip(from) {
                *d = Rgb::mix(*from, c.to_f32(), k);
                *c = Rgb::from_f32(d[0], d[1], d[2]);
            }
        }
        _ => {
            for (d, c) in displayed.iter_mut().zip(buffer.iter()) {
                *d = c.to_f32();
            }
        }
    }
}

/// Where rendered frames go.
pub trait FrameSink: Send + 'static {
    fn publish(&self, frame: &Arc<Frame>);
}

impl<F> FrameSink for F
where
    F: Fn(&Arc<Frame>) + Send + 'static,
{
    fn publish(&self, frame: &Arc<Frame>) {
        self(frame)
    }
}

/// What one LED of the scene shows (see [`crate::lighting`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum LedRole {
    /// The global effect.
    #[default]
    Effect,
    /// The effect of its own group ([`EngineScene::groups`]).
    Own(u32),
    /// A fixed colour.
    Color(Rgb),
    /// Dark.
    Off,
}

/// LEDs of one device that play the same effect of their own (a device, a
/// zone or fans with the same own effect and settings): rendered together,
/// independently from the global effect, as if they were the whole canvas –
/// a wave crosses the fan, the keyboard or the strip it was given to.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnGroup {
    /// SDK id of the device (keeps the effect's state across scene changes).
    pub device_id: u32,
    pub effect: EffectKind,
    pub params: Arc<EffectParams>,
    /// Indices into [`EngineScene::leds`].
    pub leds: Vec<u32>,
    /// The same LEDs with positions stretched over the group's own box
    /// (`0..=1`), what its effect is evaluated on.
    pub points: Vec<LedPoint>,
}

/// Engine-side view of the scene.
#[derive(Clone, Debug, Default)]
pub struct EngineScene {
    /// `(sdk id, led count)` for each scene device, same order as the scene.
    pub devices: Vec<(u32, usize)>,
    /// Kind of each scene device (same order), for per-device effects.
    pub kinds: Vec<DeviceKind>,
    /// Only LEDs of enabled + controllable devices.
    pub leds: Vec<LedPoint>,
    /// What each entry of `leds` shows; empty when every LED follows the
    /// global effect.
    pub lighting: Vec<LedRole>,
    /// Effects of their own (devices, zones, fans), see [`OwnGroup`].
    pub groups: Vec<OwnGroup>,
}

impl EngineScene {
    pub fn from_scene(scene: &Scene) -> Self {
        let leds = scene.led_points();
        let devices: Vec<(u32, usize)> = scene
            .devices
            .iter()
            .map(|d| (d.info.id, d.info.led_count))
            .collect();
        // Settings that differ from the global effect, per device.
        let ranges: Vec<Vec<(std::ops::Range<usize>, &PartLighting)>> = scene
            .devices
            .iter()
            .map(|d| crate::lighting::lit_ranges(&d.info, Some(&d.lighting)))
            .collect();
        let mut lighting = Vec::new();
        let mut groups: Vec<OwnGroup> = Vec::new();
        // Own-effect group of each group (its setting, to compare).
        let mut group_keys: Vec<(usize, &PartLighting)> = Vec::new();
        if ranges.iter().any(|r| !r.is_empty()) {
            lighting.reserve(leds.len());
            for (i, p) in leds.iter().enumerate() {
                let value = ranges
                    .get(p.device)
                    .and_then(|r| r.iter().find(|(range, _)| range.contains(&p.led)))
                    .map(|(_, v)| *v);
                let role = match value {
                    None | Some(PartLighting::Effect) => LedRole::Effect,
                    Some(PartLighting::Color { color }) => LedRole::Color(*color),
                    Some(PartLighting::Off) => LedRole::Off,
                    Some(own @ PartLighting::Own { effect, params }) => {
                        let gi = match group_keys
                            .iter()
                            .position(|(d, v)| *d == p.device && v.same_own(own))
                        {
                            Some(gi) => gi,
                            None => {
                                group_keys.push((p.device, own));
                                groups.push(OwnGroup {
                                    device_id: devices.get(p.device).map_or(0, |d| d.0),
                                    effect: *effect,
                                    params: Arc::new(params.clone()),
                                    leds: Vec::new(),
                                    points: Vec::new(),
                                });
                                groups.len() - 1
                            }
                        };
                        groups[gi].leds.push(i as u32);
                        LedRole::Own(gi as u32)
                    }
                };
                lighting.push(role);
            }
        }
        for g in &mut groups {
            g.points = stretch(g.leds.iter().map(|&i| leds[i as usize]));
        }
        Self {
            devices,
            kinds: scene.devices.iter().map(|d| d.info.kind).collect(),
            leds,
            lighting,
            groups,
        }
    }

    /// What every effect on screen needs: the global one and those of the
    /// devices, zones and fans that play their own.
    pub fn requirements(&self, global: EffectKind) -> effects::Requirements {
        let mut req = global.requirements();
        for g in &self.groups {
            let r = g.effect.requirements();
            req.animated |= r.animated;
            req.audio |= r.audio;
            req.screen |= r.screen;
            req.hardware |= r.hardware;
            req.shader |= r.shader;
        }
        req
    }
}

/// Positions of a group of LEDs stretched over their own box: the group is
/// the whole canvas of its effect.
fn stretch(points: impl Iterator<Item = LedPoint>) -> Vec<LedPoint> {
    let points: Vec<LedPoint> = points.collect();
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in &points {
        x0 = x0.min(p.x - p.rx);
        y0 = y0.min(p.y - p.ry);
        x1 = x1.max(p.x + p.rx);
        y1 = y1.max(p.y + p.ry);
    }
    let (w, h) = ((x1 - x0).max(1e-4), (y1 - y0).max(1e-4));
    points
        .into_iter()
        .map(|p| LedPoint {
            x: ((p.x - x0) / w).clamp(0.0, 1.0),
            y: ((p.y - y0) / h).clamp(0.0, 1.0),
            rx: (p.rx / w).min(0.5),
            ry: (p.ry / h).min(0.5),
            ..p
        })
        .collect()
}

/// Fixed colours and dark parts over the effects (see [`crate::lighting`]).
fn apply_lighting(buffer: &mut [Rgb], lighting: &[LedRole]) {
    for (c, l) in buffer.iter_mut().zip(lighting) {
        match l {
            LedRole::Effect | LedRole::Own(_) => {}
            LedRole::Color(color) => *c = *color,
            LedRole::Off => *c = Rgb::BLACK,
        }
    }
}

pub enum EngineCommand {
    SetScene(Arc<EngineScene>),
    SetEffect(EffectKind),
    SetParams(Arc<EffectParams>),
    /// Global brightness, `0..=1`.
    SetBrightness(f32),
    /// Global saturation, `0..=2` (`1` = colours as rendered, `2` = 200 %).
    SetSaturation(f32),
    SetFps(u32),
    /// Desktop accent colour (faded in over ~1.2 s when it changes).
    SetAccent(Rgb),
    /// Master switch: `false` outputs black and stops animating. With
    /// `fade`, the change is a soft fade; without, it is immediate
    /// (shutdown, sleep).
    SetOutputEnabled {
        on: bool,
        fade: bool,
    },
    /// Crossfade duration multiplier: `1` = default, `0` = instant changes.
    SetTransitions(f32),
    /// Flash a device in white for ~1.5 s so the user can find it.
    Identify(u32),
    /// Flash some LEDs of a device (one zone, one fan of a header).
    IdentifyLeds {
        device: u32,
        leds: std::ops::Range<usize>,
    },
    /// Also render the effect over the whole canvas ([`Frame::backdrop`]):
    /// on while a window shows previews, off in the tray (no extra cost).
    SetBackdrop(bool),
    /// Nothing uses the frames (demo devices with no window open): stop
    /// rendering entirely until resumed. The effect carries on from where
    /// it is when rendering resumes, without a fade.
    SetSuspended(bool),
    Shutdown,
}

/// Live counters, readable from any thread.
#[derive(Default)]
pub struct EngineStats {
    pub frames: AtomicU64,
    /// Average render time of the last frames, in nanoseconds.
    pub render_ns: AtomicU64,
    pub fps_target: AtomicU32,
    /// 1 when the engine is animating, 0 when asleep.
    pub animating: AtomicU32,
}

pub struct EngineHandle {
    tx: Sender<EngineCommand>,
    thread: Option<JoinHandle<()>>,
    pub stats: Arc<EngineStats>,
}

impl EngineHandle {
    pub fn send(&self, cmd: EngineCommand) {
        // The channel is bounded; commands are tiny and rare, so a full
        // channel means the engine is gone or stuck – dropping is fine.
        let _ = self.tx.try_send(cmd);
    }

    pub fn sender(&self) -> Sender<EngineCommand> {
        self.tx.clone()
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        let _ = self
            .tx
            .send_timeout(EngineCommand::Shutdown, Duration::from_millis(200));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub struct EngineConfig {
    pub effect: EffectKind,
    pub params: Arc<EffectParams>,
    pub brightness: f32,
    /// `0..=2`, see [`EngineCommand::SetSaturation`].
    pub saturation: f32,
    pub fps: u32,
    pub accent: Rgb,
}

/// Starts the engine thread.
pub fn spawn(
    config: EngineConfig,
    inputs: SharedInputs,
    sinks: Vec<Box<dyn FrameSink>>,
) -> EngineHandle {
    let (tx, rx) = bounded(256);
    let stats = Arc::new(EngineStats::default());
    stats.fps_target.store(config.fps, Ordering::Relaxed);
    let thread_stats = stats.clone();
    let thread = std::thread::Builder::new()
        .name("tuxrgb-engine".into())
        .spawn(move || Engine::new(config, inputs, sinks, thread_stats).run(rx))
        .expect("failed to spawn the effect engine thread");
    EngineHandle {
        tx,
        thread: Some(thread),
        stats,
    }
}

/// Maps the brightness slider (`0..=1`) to an LED intensity factor.
///
/// LEDs are driven linearly (PWM) but the eye is not: a gamma-2 curve makes
/// 50 % on the slider look roughly half as bright instead of barely dimmer.
#[inline]
pub fn perceived_level(slider: f32) -> f32 {
    let s = slider.clamp(0.0, 1.0);
    s * s
}

/// Keep-alive interval for non-animated effects: some controllers fall back
/// to their hardware effect after a device reset, a periodic resend fixes it.
const KEEPALIVE: Duration = Duration::from_secs(10);
const IDENTIFY_DURATION: Duration = Duration::from_millis(1500);

/// Crossfade durations (before the user's multiplier).
pub mod fades {
    use std::time::Duration;
    /// Another effect (also presets and profiles).
    pub const EFFECT: Duration = Duration::from_millis(650);
    /// Colours and settings of the current effect.
    pub const PARAMS: Duration = Duration::from_millis(350);
    /// Desktop accent colour change: slow and elegant.
    pub const ACCENT: Duration = Duration::from_millis(1200);
    /// On / off switch.
    pub const OUTPUT: Duration = Duration::from_millis(600);
    /// Devices added, moved or removed (and the very first frame).
    pub const SCENE: Duration = Duration::from_millis(450);
}

/// Brightness follows the slider through an exponential smoothing with
/// this time constant (before the user's multiplier): the first frame
/// already moves a third of the way, ~95 % is reached in 0.1 s, and a new
/// value mid-way simply retargets (no fade restarted from scratch, so
/// dragging never lags behind the finger). The saturation is applied as is,
/// on the very next frame (rendered at once, see [`Engine::urgent`]).
pub const LEVEL_TAU: f32 = 0.035;

/// Smooth start and end (sine in-out): no visible step at either end.
#[inline]
fn ease(t: f32) -> f32 {
    0.5 - 0.5 * (std::f32::consts::PI * t.clamp(0.0, 1.0)).cos()
}

struct Fade {
    /// What the LEDs showed when the fade started (blended state included,
    /// so a change in the middle of a fade continues smoothly).
    from: Vec<[f32; 3]>,
    /// Same for the backdrop (empty when it is off).
    backdrop_from: Vec<[f32; 3]>,
    start: Instant,
    duration: Duration,
}

/// Engine-side state of an [`OwnGroup`]: its effect's state and clock.
struct GroupRender {
    device_id: u32,
    effect: EffectKind,
    leds: usize,
    state: EffectState,
    buffer: Vec<Rgb>,
    started: Instant,
}

impl GroupRender {
    fn new(g: &OwnGroup) -> Self {
        Self {
            device_id: g.device_id,
            effect: g.effect,
            leds: g.points.len(),
            state: EffectState::default(),
            buffer: vec![Rgb::BLACK; g.points.len()],
            started: Instant::now(),
        }
    }
}

struct Engine {
    effect: EffectKind,
    params: Arc<EffectParams>,
    /// Applied (smoothed) brightness and the value asked for.
    brightness: f32,
    brightness_target: f32,
    /// Global saturation, applied at once.
    saturation: f32,
    /// Memo of the saturation of the colours of the last frames.
    saturation_cache: SaturationCache,
    /// A slider moved: render now instead of at the next frame tick.
    urgent: bool,
    fps: u32,
    accent: Rgb,
    output_enabled: bool,
    transitions: f32,
    scene: Arc<EngineScene>,
    /// One per [`EngineScene::groups`], same order.
    groups: Vec<GroupRender>,
    inputs: SharedInputs,
    sinks: Vec<Box<dyn FrameSink>>,
    stats: Arc<EngineStats>,
    state: EffectState,
    buffer: Vec<Rgb>,
    started: Instant,
    last_frame: Instant,
    seq: u64,
    dirty: bool,
    identify: Option<(u32, Option<std::ops::Range<usize>>, Instant)>,
    last_published: Option<Arc<Frame>>,
    /// Colours currently shown, per LED of `scene` (before the identify
    /// overlay), in floating point so fades never band.
    displayed: Vec<[f32; 3]>,
    fade: Option<Fade>,
    backdrop: Option<BackdropRender>,
    suspended: bool,
    /// Recently published frames. Once no consumer (device I/O, preview)
    /// holds one any more, its buffers are reused for a new frame: the
    /// engine does not allocate per frame.
    recycled: Vec<Arc<Frame>>,
}

/// Published frames kept for reuse (the latest is always still in use).
const RECYCLE_DEPTH: usize = 4;

impl Engine {
    fn new(
        config: EngineConfig,
        inputs: SharedInputs,
        sinks: Vec<Box<dyn FrameSink>>,
        stats: Arc<EngineStats>,
    ) -> Self {
        Self {
            effect: config.effect,
            params: config.params,
            brightness: config.brightness.clamp(0.0, 1.0),
            brightness_target: config.brightness.clamp(0.0, 1.0),
            saturation: config.saturation.clamp(0.0, 2.0),
            saturation_cache: SaturationCache::default(),
            urgent: false,
            fps: config.fps.clamp(1, 240),
            accent: config.accent,
            output_enabled: true,
            transitions: 1.0,
            scene: Arc::new(EngineScene::default()),
            groups: Vec::new(),
            inputs,
            sinks,
            stats,
            state: EffectState::default(),
            buffer: Vec::new(),
            started: Instant::now(),
            last_frame: Instant::now(),
            seq: 0,
            dirty: true,
            identify: None,
            last_published: None,
            displayed: Vec::new(),
            fade: None,
            backdrop: None,
            suspended: false,
            recycled: Vec::with_capacity(RECYCLE_DEPTH + 1),
        }
    }

    fn animating(&self) -> bool {
        !self.suspended
            && !self.scene.leds.is_empty()
            && (self.fade.is_some()
                || self.identify.is_some()
                || self.levels_moving()
                || (self.output_enabled && self.scene.requirements(self.effect).animated))
    }

    /// Keeps the state and clock of the own effects that are still there
    /// (a setting changed, a device moved), starts the new ones.
    fn sync_groups(&mut self, scene: &EngineScene) {
        let mut old = std::mem::take(&mut self.groups);
        self.groups = scene
            .groups
            .iter()
            .map(|g| {
                match old.iter().position(|o| {
                    o.device_id == g.device_id && o.effect == g.effect && o.leds == g.points.len()
                }) {
                    Some(i) => old.swap_remove(i),
                    None => GroupRender::new(g),
                }
            })
            .collect();
    }

    fn levels_moving(&self) -> bool {
        self.brightness != self.brightness_target
    }

    /// Moves the applied brightness / saturation towards the requested ones.
    fn step_levels(&mut self, dt: f32) {
        let tau = LEVEL_TAU * self.transitions.clamp(0.0, 4.0);
        let k = if tau < 1e-3 {
            1.0
        } else {
            1.0 - (-dt.max(0.0) / tau).exp()
        };
        let approach = |v: &mut f32, target: f32, eps: f32| {
            *v += (target - *v) * k;
            if (target - *v).abs() < eps {
                *v = target;
            }
        };
        approach(&mut self.brightness, self.brightness_target, 0.002);
    }

    /// Starts (or extends) a crossfade from what the LEDs show right now.
    fn begin_fade(&mut self, duration: Duration) {
        let duration = duration.mul_f32(self.transitions.clamp(0.0, 4.0));
        if duration < Duration::from_millis(20) {
            self.fade = None;
            return;
        }
        let n = self.scene.leds.len();
        self.displayed.resize(n, [0.0; 3]);
        let now = Instant::now();
        // A change in the middle of a slower fade (brightness during an
        // accent fade) keeps the slower pace.
        let remaining = self
            .fade
            .as_ref()
            .map(|f| f.duration.saturating_sub(now - f.start))
            .unwrap_or_default();
        self.fade = Some(Fade {
            from: self.displayed.clone(),
            backdrop_from: self
                .backdrop
                .as_ref()
                .map(|b| b.displayed.clone())
                .unwrap_or_default(),
            start: now,
            duration: duration.max(remaining),
        });
    }

    /// Keeps the displayed colours of LEDs that exist in both scenes, so
    /// adding, moving or removing a device fades instead of flashing.
    fn remap_displayed(&mut self, new: &EngineScene) {
        let key = |scene: &EngineScene, p: &LedPoint| {
            (
                scene.devices.get(p.device).map(|d| d.0).unwrap_or(u32::MAX),
                p.led,
            )
        };
        let old: std::collections::HashMap<(u32, usize), [f32; 3]> = self
            .scene
            .leds
            .iter()
            .zip(&self.displayed)
            .map(|(p, c)| (key(&self.scene, p), *c))
            .collect();
        self.displayed = new
            .leds
            .iter()
            .map(|p| old.get(&key(new, p)).copied().unwrap_or([0.0; 3]))
            .collect();
        if let Some(f) = &mut self.fade {
            let from: std::collections::HashMap<(u32, usize), [f32; 3]> = self
                .scene
                .leds
                .iter()
                .zip(&f.from)
                .map(|(p, c)| (key(&self.scene, p), *c))
                .collect();
            f.from = new
                .leds
                .iter()
                .map(|p| from.get(&key(new, p)).copied().unwrap_or([0.0; 3]))
                .collect();
        }
    }

    /// Applies a command; returns `false` on shutdown.
    fn apply(&mut self, cmd: EngineCommand) -> bool {
        match cmd {
            EngineCommand::SetScene(scene) => {
                // Same LEDs (a setting of a part, an own effect changed):
                // the global effect carries on undisturbed.
                let same_leds = self.scene.devices == scene.devices
                    && self.scene.leds.len() == scene.leds.len();
                self.remap_displayed(&scene);
                self.sync_groups(&scene);
                self.scene = scene;
                if !same_leds {
                    self.state.reset();
                    if let Some(b) = &mut self.backdrop {
                        b.state.reset();
                    }
                }
                self.begin_fade(fades::SCENE);
            }
            EngineCommand::SetEffect(effect) => {
                if effect != self.effect {
                    self.begin_fade(fades::EFFECT);
                    self.effect = effect;
                    self.state.reset();
                    if let Some(b) = &mut self.backdrop {
                        b.state.reset();
                    }
                    self.started = Instant::now();
                }
            }
            EngineCommand::SetParams(params) => {
                if params != self.params {
                    self.begin_fade(fades::PARAMS);
                }
                self.params = params;
            }
            // No crossfade: the level itself is smoothed (see LEVEL_TAU).
            EngineCommand::SetBrightness(b) => {
                self.brightness_target = b.clamp(0.0, 1.0);
                self.urgent = true;
            }
            // Zero latency: the frame rendered right away shows it.
            EngineCommand::SetSaturation(s) => {
                self.saturation = s.clamp(0.0, 2.0);
                self.urgent = true;
            }
            EngineCommand::SetFps(fps) => {
                self.fps = fps.clamp(1, 240);
                self.stats.fps_target.store(self.fps, Ordering::Relaxed);
                return true;
            }
            EngineCommand::SetAccent(c) => {
                if c != self.accent {
                    self.begin_fade(fades::ACCENT);
                }
                self.accent = c;
            }
            EngineCommand::SetOutputEnabled { on, fade } => {
                if fade && on != self.output_enabled {
                    self.begin_fade(fades::OUTPUT);
                } else if !fade {
                    self.fade = None;
                }
                self.output_enabled = on;
            }
            EngineCommand::SetTransitions(k) => {
                self.transitions = k.clamp(0.0, 4.0);
                return true;
            }
            EngineCommand::Identify(id) => self.identify = Some((id, None, Instant::now())),
            EngineCommand::IdentifyLeds { device, leds } => {
                self.identify = Some((device, Some(leds), Instant::now()))
            }
            EngineCommand::SetBackdrop(on) => {
                if on == self.backdrop.is_some() {
                    return true;
                }
                self.backdrop = on.then(BackdropRender::new);
            }
            EngineCommand::SetSuspended(on) => {
                if on == self.suspended {
                    return true;
                }
                self.suspended = on;
                // Resuming: render right away, from the current time.
                self.last_frame = Instant::now();
            }
            EngineCommand::Shutdown => return false,
        }
        self.dirty = true;
        true
    }

    /// Applies every command already queued; `false` on shutdown.
    fn drain(&mut self, rx: &Receiver<EngineCommand>) -> bool {
        while let Ok(cmd) = rx.try_recv() {
            if !self.apply(cmd) {
                return false;
            }
        }
        true
    }

    fn run(mut self, rx: Receiver<EngineCommand>) {
        tracing::debug!("effect engine started");
        let mut next = Instant::now();
        loop {
            if self.animating() {
                self.stats.animating.store(1, Ordering::Relaxed);
                let now = Instant::now();
                if now >= next {
                    self.render_and_publish();
                    let period = Duration::from_secs_f64(1.0 / self.fps as f64);
                    next += period;
                    // Do not try to "catch up" after a stall (suspend, debugger).
                    if next < now {
                        next = now + period;
                    }
                }
                match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
                    Ok(cmd) => {
                        // A burst (slider drag) is applied at once: the next
                        // frame shows the latest value.
                        if !self.apply(cmd) || !self.drain(&rx) {
                            break;
                        }
                        // A slider moved: show it now rather than at the
                        // next tick (at most twice the frame rate).
                        if std::mem::take(&mut self.urgent) {
                            let period = Duration::from_secs_f64(1.0 / self.fps as f64);
                            let soonest = self.last_frame + period / 2;
                            next = next.min(soonest.max(Instant::now()));
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                self.stats.animating.store(0, Ordering::Relaxed);
                if self.dirty && !self.suspended {
                    self.render_and_publish();
                }
                // Fully asleep: no wake-ups until a command or the keep-alive.
                match rx.recv_timeout(KEEPALIVE) {
                    Ok(cmd) => {
                        if !self.apply(cmd) || !self.drain(&rx) {
                            break;
                        }
                        self.urgent = false;
                        next = Instant::now();
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some(frame) = self.last_published.clone()
                            && !self.suspended
                        {
                            for sink in &self.sinks {
                                sink.publish(&frame);
                            }
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        }
        tracing::debug!("effect engine stopped");
    }

    fn render_and_publish(&mut self) {
        let started = Instant::now();
        let dt = self.last_frame.elapsed().as_secs_f32().min(0.25);
        self.last_frame = started;
        // After a sleep the elapsed time is long: smooth from one frame.
        self.step_levels(dt.min(1.5 / self.fps as f32));

        let leds = &self.scene.leds;
        self.buffer.clear();
        self.buffer.resize(leds.len(), Rgb::BLACK);

        if self.output_enabled {
            // Inputs needed by the global effect or by an own effect.
            let req = self.scene.requirements(self.effect);
            let audio = req.audio.then(|| self.inputs.latest_audio()).flatten();
            let screen = req.screen.then(|| self.inputs.latest_screen()).flatten();
            let shader = req.shader.then(|| self.inputs.latest_shader()).flatten();
            let hardware = req
                .hardware
                .then(|| self.inputs.latest_hardware())
                .flatten();
            let ctx = FrameContext {
                t: self.started.elapsed().as_secs_f32(),
                dt,
                audio: audio.as_ref(),
                screen: screen.as_deref(),
                shader: shader.as_deref(),
                hardware: hardware.as_ref(),
                accent: self.accent,
                device_kinds: &self.scene.kinds,
            };
            effects::render(
                self.effect,
                &self.params,
                &mut self.state,
                &ctx,
                leds,
                &mut self.buffer,
            );
            // Effects of their own (devices, zones, fans), each on its own
            // LEDs, with its own clock.
            for (g, render) in self.scene.groups.iter().zip(&mut self.groups) {
                render.buffer.resize(g.points.len(), Rgb::BLACK);
                let gctx = FrameContext {
                    t: render.started.elapsed().as_secs_f32(),
                    ..ctx
                };
                effects::render(
                    g.effect,
                    &g.params,
                    &mut render.state,
                    &gctx,
                    &g.points,
                    &mut render.buffer,
                );
                for (&li, c) in g.leds.iter().zip(&render.buffer) {
                    if let Some(slot) = self.buffer.get_mut(li as usize) {
                        *slot = *c;
                    }
                }
            }
            // Global saturation (0–200 %), then global brightness, both in
            // software on every LED before the frame leaves the engine:
            // OpenRGB only ever receives final colours, and the hardware
            // brightness of each controller stays at its maximum (set when
            // entering Direct mode). RAM, fan hubs, mice, keyboards and
            // motherboards all react the same way, including those without
            // a brightness setting.
            // Fixed colours / dark parts replace the effect where set.
            apply_lighting(&mut self.buffer, &self.scene.lighting);
            let level = perceived_level(self.brightness);
            grade(
                &mut self.buffer,
                self.saturation,
                level,
                &mut self.saturation_cache,
            );
            if let Some(b) = &mut self.backdrop {
                // No device kinds: per-device choices use their defaults.
                let ctx = FrameContext {
                    device_kinds: &[],
                    ..ctx
                };
                effects::render(
                    self.effect,
                    &self.params,
                    &mut b.state,
                    &ctx,
                    &b.points,
                    &mut b.buffer,
                );
                grade(
                    &mut b.buffer,
                    self.saturation,
                    level,
                    &mut self.saturation_cache,
                );
            }
        } else if let Some(b) = &mut self.backdrop {
            b.buffer.fill(Rgb::BLACK);
        }

        // Crossfade from what the LEDs showed before the last change.
        self.displayed.resize(leds.len(), [0.0; 3]);
        let progress = self.fade.as_ref().and_then(|f| {
            let t = f.start.elapsed().as_secs_f32() / f.duration.as_secs_f32().max(1e-3);
            (t < 1.0).then(|| ease(t))
        });
        let empty: &[[f32; 3]] = &[];
        crossfade(
            &mut self.displayed,
            &mut self.buffer,
            self.fade.as_ref().map_or(empty, |f| &f.from),
            progress,
        );
        // The frame to fill: a published one nobody holds any more, or a
        // new one while the pool warms up.
        let mut arc = self
            .recycled
            .iter()
            .position(|f| Arc::strong_count(f) == 1 && Arc::weak_count(f) == 0)
            .map(|i| self.recycled.swap_remove(i))
            .unwrap_or_default();
        if Arc::get_mut(&mut arc).is_none() {
            arc = Arc::default();
        }
        let Some(frame) = Arc::get_mut(&mut arc) else {
            return;
        };
        let mut old_backdrop = frame.backdrop.take();
        frame.backdrop = self.backdrop.as_mut().map(|b| {
            crossfade(
                &mut b.displayed,
                &mut b.buffer,
                self.fade.as_ref().map_or(empty, |f| &f.backdrop_from),
                progress,
            );
            match old_backdrop
                .take()
                .filter(|r| Arc::strong_count(r) == 1 && Arc::weak_count(r) == 0)
            {
                Some(mut reused) => {
                    if let Some(r) = Arc::get_mut(&mut reused) {
                        r.width = BACKDROP_W;
                        r.height = BACKDROP_H;
                        r.colors.clear();
                        r.colors.extend_from_slice(&b.buffer);
                    }
                    reused
                }
                None => Arc::new(Backdrop {
                    width: BACKDROP_W,
                    height: BACKDROP_H,
                    colors: b.buffer.clone(),
                }),
            }
        });
        if progress.is_none() {
            self.fade = None;
        }

        if self.output_enabled || self.identify.is_some() {
            // Identify overlay (never part of a fade).
            if let Some((id, range, since)) = self.identify.clone() {
                let elapsed = since.elapsed();
                if elapsed >= IDENTIFY_DURATION {
                    self.identify = None;
                } else {
                    let blink = ((elapsed.as_secs_f32() * 6.0).fract() < 0.5) as u8 as f32;
                    for (c, p) in self.buffer.iter_mut().zip(leds) {
                        if self.scene.devices.get(p.device).map(|d| d.0) == Some(id)
                            && range.as_ref().is_none_or(|r| r.contains(&p.led))
                        {
                            *c = Rgb::WHITE.scale(0.25 + 0.75 * blink);
                        }
                    }
                }
            }
        }

        // Group per device, preserving OpenRGB LED order (the vectors of the
        // recycled frame are refilled in place).
        let mut used = 0usize;
        let mut current: Option<usize> = None;
        for (c, p) in self.buffer.iter().zip(leds) {
            if current != Some(p.device) {
                let (id, count) = self
                    .scene
                    .devices
                    .get(p.device)
                    .copied()
                    .unwrap_or((u32::MAX, 0));
                if used == frame.devices.len() {
                    frame.devices.push(DeviceFrame {
                        id,
                        colors: Vec::with_capacity(count),
                    });
                }
                let d = &mut frame.devices[used];
                d.id = id;
                d.colors.clear();
                d.colors.resize(count, Rgb::BLACK);
                used += 1;
                current = Some(p.device);
            }
            if let Some(slot) = frame.devices[used - 1].colors.get_mut(p.led) {
                *slot = *c;
            }
        }
        frame.devices.truncate(used);
        self.seq += 1;
        frame.seq = self.seq;
        for sink in &self.sinks {
            sink.publish(&arc);
        }
        self.recycled.push(arc.clone());
        if self.recycled.len() > RECYCLE_DEPTH {
            self.recycled.remove(0);
        }
        self.last_published = Some(arc);
        self.dirty = false;

        let ns = started.elapsed().as_nanos() as u64;
        let avg = self.stats.render_ns.load(Ordering::Relaxed);
        self.stats.render_ns.store(
            if avg == 0 { ns } else { (avg * 15 + ns) / 16 },
            Ordering::Relaxed,
        );
        self.stats.frames.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::demo_devices;
    use std::sync::Mutex;

    fn scene() -> Arc<EngineScene> {
        let s = Scene::build(&demo_devices(), &Default::default(), &Default::default());
        Arc::new(EngineScene::from_scene(&s))
    }

    #[test]
    fn static_effect_publishes_once_then_sleeps() {
        let frames = Arc::new(Mutex::new(Vec::<Arc<Frame>>::new()));
        let sink_frames = frames.clone();
        let engine = spawn(
            EngineConfig {
                effect: EffectKind::Static,
                params: Arc::default(),
                brightness: 1.0,
                saturation: 1.0,
                fps: 60,
                accent: Rgb::WHITE,
            },
            SharedInputs::default(),
            vec![Box::new(move |f: &Arc<Frame>| {
                sink_frames.lock().unwrap().push(f.clone())
            })],
        );
        engine.send(EngineCommand::SetTransitions(0.0));
        engine.send(EngineCommand::SetScene(scene()));
        std::thread::sleep(Duration::from_millis(300));
        let n = frames.lock().unwrap().len();
        assert!(n <= 2, "static effect must not animate, got {n} frames");
        let last = frames.lock().unwrap().last().cloned().unwrap();
        let total: usize = last.devices.iter().map(|d| d.colors.len()).sum();
        assert_eq!(
            total,
            demo_devices().iter().map(|d| d.led_count).sum::<usize>()
        );
        assert_eq!(engine.stats.animating.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn brightness_dims_every_device_in_software() {
        let frames = Arc::new(Mutex::new(Vec::<Arc<Frame>>::new()));
        let sink_frames = frames.clone();
        let mut params = EffectParams::default();
        params.set(
            EffectKind::Static,
            "color",
            crate::effects::ParamValue::Color(Rgb::new(255, 128, 0)),
        );
        let engine = spawn(
            EngineConfig {
                effect: EffectKind::Static,
                params: Arc::new(params),
                brightness: 0.5,
                saturation: 1.0,
                fps: 60,
                accent: Rgb::WHITE,
            },
            SharedInputs::default(),
            vec![Box::new(move |f: &Arc<Frame>| {
                sink_frames.lock().unwrap().push(f.clone())
            })],
        );
        engine.send(EngineCommand::SetTransitions(0.0));
        engine.send(EngineCommand::SetScene(scene()));
        std::thread::sleep(Duration::from_millis(200));
        let last = frames.lock().unwrap().last().cloned().unwrap();
        // 50 % on the slider -> 25 % intensity, hue kept (2:1 ratio R:G).
        for d in &last.devices {
            for c in &d.colors {
                assert_eq!(*c, Rgb::new(64, 32, 0), "device {}", d.id);
            }
        }
        engine.send(EngineCommand::SetBrightness(0.0));
        std::thread::sleep(Duration::from_millis(200));
        let last = frames.lock().unwrap().last().cloned().unwrap();
        assert!(
            last.devices
                .iter()
                .all(|d| d.colors.iter().all(|c| *c == Rgb::BLACK))
        );
    }

    type Frames = Arc<Mutex<Vec<Arc<Frame>>>>;

    /// The brightness slider reaches the LEDs at once and smoothly: the
    /// first frame already moves, ~95 % within 0.12 s, never backwards,
    /// and the engine falls asleep again once the target is reached.
    #[test]
    fn brightness_follows_the_slider_almost_instantly() {
        let (frames, sink) = collect();
        let engine = spawn(
            EngineConfig {
                effect: EffectKind::Static,
                params: static_params(Rgb::new(255, 255, 255)),
                brightness: 1.0,
                saturation: 1.0,
                fps: 60,
                accent: Rgb::WHITE,
            },
            SharedInputs::default(),
            vec![sink],
        );
        engine.send(EngineCommand::SetScene(scene()));
        std::thread::sleep(Duration::from_millis(700));
        frames.lock().unwrap().clear();
        let sent = Instant::now();
        engine.send(EngineCommand::SetBrightness(0.0));
        std::thread::sleep(Duration::from_millis(400));
        let got = frames.lock().unwrap().clone();
        let level = |f: &Frame| f.devices[0].colors[0].r;
        assert!(!got.is_empty());
        assert!(level(&got[0]) < 255, "the first frame must already move");
        assert!(got.windows(2).all(|w| level(&w[1]) <= level(&w[0])));
        assert_eq!(level(got.last().unwrap()), 0);
        let n_at_120ms = (0.12 * 60.0) as usize;
        let near = got.get(n_at_120ms).map(|f| level(f)).unwrap_or(0);
        assert!(near < 26, "{near} after ~120 ms (expected under 10 %)");
        // Settled: no more frames (static effect, asleep until keep-alive).
        let count = got.len();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(frames.lock().unwrap().len(), count);
        assert!(sent.elapsed() < Duration::from_secs(2));
    }

    /// The backdrop covers the whole canvas with the graded effect, only
    /// while asked for.
    #[test]
    fn backdrop_renders_the_effect_over_the_canvas() {
        let (frames, sink) = collect();
        let engine = spawn(
            EngineConfig {
                effect: EffectKind::Static,
                params: static_params(Rgb::new(255, 0, 0)),
                brightness: 1.0,
                saturation: 1.0,
                fps: 60,
                accent: Rgb::WHITE,
            },
            SharedInputs::default(),
            vec![sink],
        );
        engine.send(EngineCommand::SetTransitions(0.0));
        engine.send(EngineCommand::SetScene(scene()));
        std::thread::sleep(Duration::from_millis(150));
        assert!(frames.lock().unwrap().last().unwrap().backdrop.is_none());
        engine.send(EngineCommand::SetBackdrop(true));
        std::thread::sleep(Duration::from_millis(150));
        let last = frames.lock().unwrap().last().cloned().unwrap();
        let b = last.backdrop.as_ref().expect("backdrop");
        assert_eq!((b.width, b.height), (BACKDROP_W, BACKDROP_H));
        assert_eq!(b.colors.len(), BACKDROP_W * BACKDROP_H);
        assert!(b.colors.iter().all(|c| *c == Rgb::new(255, 0, 0)));
        // Same brightness as the LEDs.
        engine.send(EngineCommand::SetBrightness(0.5));
        std::thread::sleep(Duration::from_millis(300));
        let last = frames.lock().unwrap().last().cloned().unwrap();
        let b = last.backdrop.as_ref().unwrap();
        assert_eq!(b.colors[0], last.devices[0].colors[0]);
        assert_eq!(b.colors[0], Rgb::new(64, 0, 0));
        engine.send(EngineCommand::SetBackdrop(false));
        std::thread::sleep(Duration::from_millis(150));
        assert!(frames.lock().unwrap().last().unwrap().backdrop.is_none());
    }

    fn collect() -> (Frames, Box<dyn FrameSink>) {
        let frames = Arc::new(Mutex::new(Vec::<Arc<Frame>>::new()));
        let sink = frames.clone();
        (
            frames,
            Box::new(move |f: &Arc<Frame>| sink.lock().unwrap().push(f.clone())),
        )
    }

    fn static_params(c: Rgb) -> Arc<EffectParams> {
        let mut params = EffectParams::default();
        params.set(
            EffectKind::Static,
            "color",
            crate::effects::ParamValue::Color(c),
        );
        Arc::new(params)
    }

    fn first_led(frames: &Mutex<Vec<Arc<Frame>>>) -> Rgb {
        frames.lock().unwrap().last().unwrap().devices[0].colors[0]
    }

    /// The demo scene with the whole first device playing `own`.
    fn scene_with_own(own: PartLighting) -> (Arc<EngineScene>, u32) {
        let devices = demo_devices();
        let mut s = Scene::build(&devices, &Default::default(), &Default::default());
        let first = &devices[0];
        let mut map = crate::lighting::LightingMap::new();
        let entry = map.entry(first.key.clone()).or_default();
        for zi in 0..first.zones.len() {
            entry.entry(zi).or_default().set_part(0, own.clone());
        }
        s.set_lighting(&map, &Default::default());
        (Arc::new(EngineScene::from_scene(&s)), first.id)
    }

    fn own(effect: EffectKind, color: Option<Rgb>) -> PartLighting {
        let mut params = EffectParams::default();
        if let Some(c) = color {
            params.set(effect, "color", crate::effects::ParamValue::Color(c));
        }
        PartLighting::Own { effect, params }
    }

    #[test]
    fn own_effects_play_on_their_part_only() {
        let (frames, sink) = collect();
        let engine = spawn(
            EngineConfig {
                effect: EffectKind::Static,
                params: static_params(Rgb::new(0, 0, 255)),
                brightness: 1.0,
                saturation: 1.0,
                fps: 60,
                accent: Rgb::WHITE,
            },
            SharedInputs::default(),
            vec![sink],
        );
        engine.send(EngineCommand::SetTransitions(0.0));
        let (scene, own_id) = scene_with_own(own(EffectKind::Static, Some(Rgb::new(255, 0, 0))));
        assert_eq!(scene.groups.len(), 1);
        engine.send(EngineCommand::SetScene(scene));
        std::thread::sleep(Duration::from_millis(200));
        let last = frames.lock().unwrap().last().cloned().unwrap();
        for d in &last.devices {
            let want = if d.id == own_id {
                Rgb::new(255, 0, 0)
            } else {
                Rgb::new(0, 0, 255)
            };
            assert!(d.colors.iter().all(|c| *c == want), "device {}", d.id);
        }
        // Both static: nothing animates.
        assert_eq!(engine.stats.animating.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn an_own_animated_effect_runs_under_a_static_global_one() {
        let (frames, sink) = collect();
        let engine = spawn(
            EngineConfig {
                effect: EffectKind::Static,
                params: static_params(Rgb::new(0, 0, 255)),
                brightness: 1.0,
                saturation: 1.0,
                fps: 60,
                accent: Rgb::WHITE,
            },
            SharedInputs::default(),
            vec![sink],
        );
        engine.send(EngineCommand::SetTransitions(0.0));
        let (scene, own_id) = scene_with_own(own(EffectKind::ColorCycle, None));
        // The group is its own canvas: its LEDs cover the whole box.
        let g = &scene.groups[0];
        let xs: Vec<f32> = g.points.iter().map(|p| p.x).collect();
        assert!(xs.iter().cloned().fold(1.0, f32::min) < 0.2);
        assert!(xs.iter().cloned().fold(0.0, f32::max) > 0.8);
        engine.send(EngineCommand::SetScene(scene));
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(engine.stats.animating.load(Ordering::Relaxed), 1);
        let frames: Vec<Arc<Frame>> = frames
            .lock()
            .unwrap()
            .iter()
            .filter(|f| !f.devices.is_empty())
            .cloned()
            .collect();
        assert!(frames.len() > 10, "{} frames", frames.len());
        let of = |f: &Arc<Frame>, id: u32| f.devices.iter().find(|d| d.id == id).unwrap().colors[0];
        let other = frames[0]
            .devices
            .iter()
            .find(|d| d.id != own_id)
            .unwrap()
            .id;
        let (a, b) = (&frames[frames.len() / 2], frames.last().unwrap());
        assert_ne!(of(a, own_id), of(b, own_id), "the own effect moves");
        assert_eq!(of(a, other), of(b, other), "the global one stays");
    }

    #[test]
    fn colour_changes_crossfade_then_sleep() {
        let (frames, sink) = collect();
        let engine = spawn(
            EngineConfig {
                effect: EffectKind::Static,
                params: static_params(Rgb::new(255, 0, 0)),
                brightness: 1.0,
                saturation: 1.0,
                fps: 100,
                accent: Rgb::WHITE,
            },
            SharedInputs::default(),
            vec![sink],
        );
        engine.send(EngineCommand::SetScene(scene()));
        // The first frames fade in from black.
        std::thread::sleep(Duration::from_millis(60));
        let early = first_led(&frames);
        assert!(
            early.r > 0 && early.r < 255,
            "fade-in expected, got {early:?}"
        );
        std::thread::sleep(Duration::from_millis(700));
        assert_eq!(first_led(&frames), Rgb::new(255, 0, 0));
        // Red → blue goes through a blend, never a hard switch.
        engine.send(EngineCommand::SetParams(static_params(Rgb::new(0, 0, 255))));
        std::thread::sleep(Duration::from_millis(150));
        let mid = first_led(&frames);
        assert!(mid.r > 10 && mid.b > 10, "blend expected, got {mid:?}");
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(first_led(&frames), Rgb::new(0, 0, 255));
        // Once the fade is over, a static effect sleeps again.
        let n = frames.lock().unwrap().len();
        std::thread::sleep(Duration::from_millis(300));
        assert!(frames.lock().unwrap().len() <= n + 1);
        assert_eq!(engine.stats.animating.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn accent_change_fades_slowly() {
        let (frames, sink) = collect();
        let mut params = EffectParams::default();
        params.set(
            EffectKind::Static,
            crate::effects::KDE_ACCENT_KEY,
            crate::effects::ParamValue::Toggle(true),
        );
        let engine = spawn(
            EngineConfig {
                effect: EffectKind::Static,
                params: Arc::new(params),
                brightness: 1.0,
                saturation: 1.0,
                fps: 100,
                accent: Rgb::new(0, 255, 0),
            },
            SharedInputs::default(),
            vec![sink],
        );
        engine.send(EngineCommand::SetTransitions(0.0));
        engine.send(EngineCommand::SetScene(scene()));
        std::thread::sleep(Duration::from_millis(100));
        engine.send(EngineCommand::SetTransitions(1.0));
        engine.send(EngineCommand::SetAccent(Rgb::new(255, 0, 255)));
        std::thread::sleep(Duration::from_millis(400));
        let mid = first_led(&frames);
        assert!(
            mid.g > 20 && mid.r > 20,
            "still fading after 0.4 s: {mid:?}"
        );
        std::thread::sleep(Duration::from_millis(1100));
        assert_eq!(first_led(&frames), Rgb::new(255, 0, 255));
    }

    #[test]
    fn immediate_blackout_skips_the_fade() {
        let (frames, sink) = collect();
        let engine = spawn(
            EngineConfig {
                effect: EffectKind::Static,
                params: static_params(Rgb::WHITE),
                brightness: 1.0,
                saturation: 1.0,
                fps: 100,
                accent: Rgb::WHITE,
            },
            SharedInputs::default(),
            vec![sink],
        );
        engine.send(EngineCommand::SetTransitions(0.0));
        engine.send(EngineCommand::SetScene(scene()));
        std::thread::sleep(Duration::from_millis(100));
        engine.send(EngineCommand::SetTransitions(1.0));
        engine.send(EngineCommand::SetOutputEnabled {
            on: false,
            fade: false,
        });
        std::thread::sleep(Duration::from_millis(60));
        let last = frames.lock().unwrap().last().cloned().unwrap();
        assert!(
            last.devices
                .iter()
                .all(|d| d.colors.iter().all(|c| *c == Rgb::BLACK))
        );
    }

    #[test]
    fn saturation_boost_reaches_the_frame() {
        let (frames, sink) = collect();
        let engine = spawn(
            EngineConfig {
                effect: EffectKind::Static,
                params: static_params(Rgb::new(255, 127, 127)),
                brightness: 1.0,
                saturation: 2.0,
                fps: 60,
                accent: Rgb::WHITE,
            },
            SharedInputs::default(),
            vec![sink],
        );
        engine.send(EngineCommand::SetTransitions(0.0));
        engine.send(EngineCommand::SetScene(scene()));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(first_led(&frames), Rgb::new(255, 127, 127).saturate(2.0));
        assert_ne!(first_led(&frames), Rgb::new(255, 127, 127));
    }

    /// Zero latency: the frame that follows a saturation change already
    /// has the final value (no smoothing ramp), whether the engine sleeps
    /// (static colour) or an animated effect waits for its next tick at a
    /// low frame rate.
    #[test]
    fn saturation_change_shows_on_the_next_frame() {
        for (effect, params) in [
            (EffectKind::Static, static_params(Rgb::new(255, 127, 127))),
            (EffectKind::RainbowWave, Arc::default()),
        ] {
            let (frames, sink) = collect();
            let engine = spawn(
                EngineConfig {
                    effect,
                    params,
                    brightness: 1.0,
                    saturation: 1.0,
                    fps: 8,
                    accent: Rgb::WHITE,
                },
                SharedInputs::default(),
                vec![sink],
            );
            engine.send(EngineCommand::SetTransitions(1.0));
            engine.send(EngineCommand::SetScene(scene()));
            std::thread::sleep(Duration::from_millis(800));
            let before = frames.lock().unwrap().len();
            engine.send(EngineCommand::SetSaturation(0.0));
            std::thread::sleep(Duration::from_millis(40));
            let got = frames.lock().unwrap();
            assert!(got.len() > before, "{effect:?}: no frame within 40 ms");
            for d in &got[before].devices {
                for c in &d.colors {
                    assert!(c.r == c.g && c.g == c.b, "{effect:?}: {c:?} still coloured");
                }
            }
        }
    }

    /// The grade applies the perceptual saturation, then the brightness,
    /// and leaves the colours alone at 100 %.
    #[test]
    fn grade_applies_saturation_then_brightness() {
        let mut cache = SaturationCache::default();
        let mut grade_one = |c: Rgb, s: f32, level: f32| {
            let mut b = [c];
            grade(&mut b, s, level, &mut cache);
            b[0]
        };
        let pastel = Rgb::new(200, 150, 150);
        assert_eq!(grade_one(pastel, 1.0, 1.0), pastel);
        assert_ne!(grade_one(pastel, 2.0, 1.0), pastel);
        assert_eq!(grade_one(pastel, 2.0, 1.0), pastel.saturate(2.0));
        assert_eq!(grade_one(pastel, 2.0, 0.5), pastel.saturate(2.0).dim(0.5));
        assert_eq!(grade_one(Rgb::WHITE, 2.0, 1.0), Rgb::WHITE);
    }

    /// Frames nobody holds any more are refilled in place: after warm-up the
    /// engine stops allocating, and a frame still held by a consumer is
    /// never modified.
    #[test]
    fn frames_are_recycled_without_touching_held_ones() {
        let held: Arc<Mutex<Option<Arc<Frame>>>> = Arc::default();
        let ptrs: Arc<Mutex<Vec<usize>>> = Arc::default();
        let (h, p) = (held.clone(), ptrs.clone());
        let engine = spawn(
            EngineConfig {
                effect: EffectKind::RainbowWave,
                params: Arc::default(),
                brightness: 1.0,
                saturation: 1.0,
                fps: 100,
                accent: Rgb::WHITE,
            },
            SharedInputs::default(),
            vec![Box::new(move |f: &Arc<Frame>| {
                p.lock().unwrap().push(Arc::as_ptr(f) as usize);
                let mut slot = h.lock().unwrap();
                if slot.is_none() && !f.devices.is_empty() {
                    *slot = Some(f.clone());
                }
            })],
        );
        engine.send(EngineCommand::SetBackdrop(true));
        engine.send(EngineCommand::SetScene(scene()));
        std::thread::sleep(Duration::from_millis(100));
        let snapshot = {
            let f = held.lock().unwrap().clone().unwrap();
            ((*f).clone(), f)
        };
        std::thread::sleep(Duration::from_millis(300));
        drop(engine);
        // The frame kept by the consumer is exactly what was published.
        assert_eq!(*snapshot.1, snapshot.0);
        let ptrs = ptrs.lock().unwrap();
        let distinct: std::collections::BTreeSet<_> = ptrs.iter().collect();
        assert!(ptrs.len() > 20, "{} frames", ptrs.len());
        assert!(
            distinct.len() <= RECYCLE_DEPTH + 3,
            "{} distinct frames for {} renders",
            distinct.len(),
            ptrs.len()
        );
        let total: usize = snapshot.0.devices.iter().map(|d| d.colors.len()).sum();
        assert_eq!(
            total,
            demo_devices().iter().map(|d| d.led_count).sum::<usize>()
        );
        assert!(snapshot.0.backdrop.is_some());
    }

    /// Suspended (demo devices, no window): no frame at all, then the
    /// effect resumes at once.
    #[test]
    fn suspended_engine_renders_nothing() {
        let count = Arc::new(AtomicU64::new(0));
        let c = count.clone();
        let engine = spawn(
            EngineConfig {
                effect: EffectKind::RainbowWave,
                params: Arc::default(),
                brightness: 1.0,
                saturation: 1.0,
                fps: 100,
                accent: Rgb::WHITE,
            },
            SharedInputs::default(),
            vec![Box::new(move |_: &Arc<Frame>| {
                c.fetch_add(1, Ordering::Relaxed);
            })],
        );
        engine.send(EngineCommand::SetSuspended(true));
        // (The empty start-up frame may already be out.)
        std::thread::sleep(Duration::from_millis(50));
        count.store(0, Ordering::Relaxed);
        engine.send(EngineCommand::SetScene(scene()));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(count.load(Ordering::Relaxed), 0);
        assert_eq!(engine.stats.animating.load(Ordering::Relaxed), 0);
        engine.send(EngineCommand::SetSuspended(false));
        std::thread::sleep(Duration::from_millis(200));
        assert!(count.load(Ordering::Relaxed) >= 10);
    }

    #[test]
    fn animated_effect_runs_at_fixed_rate() {
        let count = Arc::new(AtomicU64::new(0));
        let c = count.clone();
        let engine = spawn(
            EngineConfig {
                effect: EffectKind::RainbowWave,
                params: Arc::default(),
                brightness: 0.5,
                saturation: 1.0,
                fps: 50,
                accent: Rgb::WHITE,
            },
            SharedInputs::default(),
            vec![Box::new(move |_: &Arc<Frame>| {
                c.fetch_add(1, Ordering::Relaxed);
            })],
        );
        engine.send(EngineCommand::SetScene(scene()));
        std::thread::sleep(Duration::from_millis(500));
        let n = count.load(Ordering::Relaxed);
        assert!((15..=40).contains(&n), "expected ~25 frames, got {n}");
        drop(engine);
    }
}
