//! Effect renderers. Pure functions of time, parameters and LED positions;
//! the only state kept between frames is smoothing for reactive effects.

use super::{DIRECTIONS, EffectKind, EffectParams, SCOPE_KEY};
use crate::color::{Palette, Rgb};
use crate::device::DeviceKind;
use crate::layout::{LedPoint, SCENE_ASPECT};
use crate::signal::{AudioSpectrum, HardwareSample, ScreenFrame};
use std::f32::consts::{PI, TAU};

/// Per-frame inputs.
pub struct FrameContext<'a> {
    /// Seconds since the effect started.
    pub t: f32,
    /// Seconds since the previous frame.
    pub dt: f32,
    pub audio: Option<&'a AudioSpectrum>,
    pub screen: Option<&'a ScreenFrame>,
    /// Latest frame of the GLSL shader renderer.
    pub shader: Option<&'a ScreenFrame>,
    pub hardware: Option<&'a HardwareSample>,
    /// Accent colour of the desktop (KDE, GNOME, COSMIC…).
    pub accent: Rgb,
    /// Kind of each scene device (index = [`LedPoint::device`]).
    pub device_kinds: &'a [DeviceKind],
}

impl<'a> FrameContext<'a> {
    /// A context without any reactive input (tests, previews).
    pub fn basic(t: f32, dt: f32, accent: Rgb) -> Self {
        Self {
            t,
            dt,
            audio: None,
            screen: None,
            shader: None,
            hardware: None,
            accent,
            device_kinds: &[],
        }
    }
}

/// Mutable state carried across frames (smoothing buffers).
#[derive(Default)]
pub struct EffectState {
    smoothed: Vec<[f32; 3]>,
    audio_env: Vec<f32>,
    pulse: f32,
    flash: f32,
    flash_index: usize,
    /// Smoothed sensor values (see [`SENSOR_COUNT`]).
    sensors: [Option<f32>; SENSOR_COUNT],
}

impl EffectState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Renders `kind` into `out` (one colour per LED in `leds`).
pub fn render(
    kind: EffectKind,
    params: &EffectParams,
    state: &mut EffectState,
    ctx: &FrameContext<'_>,
    leds: &[LedPoint],
    out: &mut [Rgb],
) {
    debug_assert_eq!(leds.len(), out.len());
    let palette = kind
        .uses_system_palette(params)
        .then(|| kind.system_palette(params, ctx.accent));
    let t = ctx.t;
    match kind {
        EffectKind::Static => {
            out.fill(kind.main_color(params, ctx.accent));
        }
        EffectKind::Breathing => {
            let bpm = params.number(kind, "speed");
            let floor = params.number(kind, "floor");
            // Smooth "sine squared" curve with a short hold at the bottom,
            // which reads more natural than a raw sine.
            let cycles = t * bpm / 60.0;
            let phase = cycles.fract();
            let s = (phase * PI).sin();
            let k = floor + (1.0 - floor) * s * s;
            let c = match &palette {
                // Multi-tint: each breath takes the next palette colour,
                // changed while the LEDs are at their darkest.
                Some(p) => {
                    let cols = p.colors();
                    let i = cycles.floor().max(0.0) as usize % 3;
                    let prev = cols[(i + 2) % 3];
                    Rgb::lerp(prev, cols[i], smoothstep(phase / 0.12))
                }
                None => kind.main_color(params, ctx.accent),
            };
            out.fill(c.scale(k));
        }
        EffectKind::RainbowWave => {
            let speed = params.number(kind, "speed");
            let scale = params.number(kind, "scale");
            let dir = params.choice(kind, "direction");
            for (o, p) in out.iter_mut().zip(leds) {
                let d = directional(dir, p.x, p.y);
                *o = Rgb::from_hsv(d / scale - t * speed * 0.25, 1.0, 1.0);
            }
        }
        EffectKind::ColorCycle => {
            let speed = params.number(kind, "speed");
            let pos = t * speed * 0.08;
            out.fill(match &palette {
                Some(p) => p.cyclic(pos),
                None => Rgb::from_hsv(pos, 1.0, 1.0),
            });
        }
        EffectKind::GradientWave => {
            let a = params.color(kind, "color_a");
            let b = params.color(kind, "color_b");
            let speed = params.number(kind, "speed");
            let scale = params.number(kind, "scale");
            let dir = params.choice(kind, "direction");
            for (o, p) in out.iter_mut().zip(leds) {
                let d = directional(dir, p.x, p.y);
                let pos = d / scale - t * speed * 0.25;
                *o = match &palette {
                    Some(pal) => pal.cyclic(pos),
                    None => Rgb::lerp(a, b, 0.5 + 0.5 * (TAU * pos).sin()),
                };
            }
        }
        EffectKind::Aurora => {
            let (a, b, bg) = match &palette {
                Some(p) => (p.primary, p.tertiary, p.background),
                None => (
                    params.color(kind, "color_a"),
                    params.color(kind, "color_b"),
                    params.color(kind, "background"),
                ),
            };
            let t = t * params.number(kind, "speed");
            for (o, p) in out.iter_mut().zip(leds) {
                // Two slowly drifting curtains built from layered sines.
                let c1 =
                    0.5 + 0.5 * (p.x * 5.1 + t * 0.9 + (p.y * 3.0 + t * 0.4).sin() * 1.3).sin();
                let c2 =
                    0.5 + 0.5 * (p.x * 3.3 - t * 0.6 + (p.y * 4.0 - t * 0.3).cos() * 1.7).sin();
                let curtain = (c1 * c1 * c1).clamp(0.0, 1.0);
                let veil = (c2 * c2).clamp(0.0, 1.0) * 0.8;
                let col = Rgb::lerp(bg, a, curtain);
                *o = Rgb::lerp(col, b, veil * (1.0 - curtain * 0.6));
            }
        }
        EffectKind::Starfield => {
            let bg = palette
                .map(|p| p.background)
                .unwrap_or_else(|| params.color(kind, "background"));
            let star = params.color(kind, "star");
            let density = params.number(kind, "density");
            let speed = params.number(kind, "speed");
            let rainbow = params.toggle(kind, "rainbow");
            for (i, (o, p)) in out.iter_mut().zip(leds).enumerate() {
                let seed = led_seed(p, i);
                let r1 = unit(seed);
                let r2 = unit(hash(seed ^ 0x9e37_79b9));
                if r1 > density {
                    *o = bg;
                    continue;
                }
                let f = 0.25 + r2 * 0.9;
                let phase = r2 * TAU;
                let s = ((t * speed * f * TAU + phase).sin() * 0.5 + 0.5).powf(6.0);
                let c = match &palette {
                    Some(pal) => Rgb::lerp(pal.colors()[(seed % 3) as usize], Rgb::WHITE, 0.25),
                    None if rainbow => Rgb::from_hsv(unit(hash(seed ^ 0x51ed_270b)), 0.6, 1.0),
                    None => star,
                };
                *o = Rgb::lerp(bg, c, s);
            }
        }
        EffectKind::Plasma => {
            let speed = params.number(kind, "speed");
            let k = 6.0 / params.number(kind, "scale");
            let tt = t * speed;
            let cx = 0.5 + 0.35 * (tt * 0.31).sin();
            let cy = 0.5 + 0.35 * (tt * 0.23).cos();
            for (o, p) in out.iter_mut().zip(leds) {
                let x = p.x * SCENE_ASPECT;
                let y = p.y;
                let dx = (p.x - cx) * SCENE_ASPECT;
                let dy = p.y - cy;
                let v = (x * k * 0.6 + tt).sin()
                    + (y * k + tt * 0.7).sin()
                    + ((x + y) * k * 0.45 + tt * 1.3).sin()
                    + ((dx * dx + dy * dy).sqrt() * k * 1.1 - tt).sin();
                let pos = v / 8.0 + 0.5 + tt * 0.04;
                *o = match &palette {
                    Some(pal) => pal.cyclic(pos),
                    None => Rgb::from_hsv(pos, 1.0, 1.0),
                };
            }
        }
        EffectKind::Fire => {
            let speed = params.number(kind, "speed");
            let height = params.number(kind, "height");
            let per_device = params.choice(kind, SCOPE_KEY) == 0;
            for (o, p) in out.iter_mut().zip(leds) {
                // Height above the bottom: of the device, or of the setup.
                let (h, across) = if per_device {
                    (1.0 - p.v, p.u * 2.0 + p.device as f32 * 3.7)
                } else {
                    (1.0 - p.y, p.x * SCENE_ASPECT * 2.5)
                };
                let n = fbm(across * 2.2, h * 2.4 - t * speed * 1.7);
                let flicker = noise2(across * 5.0 + 11.0, t * speed * 6.0);
                let heat = ((1.0 - h / height.max(0.05)) * 0.9
                    + (n - 0.45) * 1.1
                    + (flicker - 0.5) * 0.25)
                    .clamp(0.0, 1.0);
                *o = fire_ramp(heat, palette.as_ref());
            }
        }
        EffectKind::Comet => {
            let speed = params.number(kind, "speed");
            let tail = params.number(kind, "tail");
            let dir = params.choice(kind, "direction");
            let base = params.color(kind, "color");
            let bg = palette
                .map(|p| p.background)
                .unwrap_or_else(|| params.color(kind, "background"));
            let rainbow = params.toggle(kind, "rainbow");
            let travel = 1.0 + tail + 0.1;
            let head = (t * speed * 0.35).fract() * travel - 0.05;
            for (o, p) in out.iter_mut().zip(leds) {
                let d = directional(dir, p.x, p.y);
                let behind = head - d;
                let k = if behind < 0.0 {
                    (1.0 + behind / 0.03).max(0.0).powi(2)
                } else {
                    (1.0 - behind / tail).max(0.0).powf(2.2)
                };
                let f = (behind / tail).clamp(0.0, 1.0);
                let col = match &palette {
                    Some(pal) => {
                        Rgb::lerp(Rgb::lerp(pal.primary, Rgb::WHITE, 0.3), pal.tertiary, f)
                    }
                    None if rainbow => Rgb::from_hsv(t * 0.1 + d * 0.3, 1.0, 1.0),
                    None => Rgb::lerp(Rgb::lerp(base, Rgb::WHITE, 0.35), base, f.sqrt()),
                };
                *o = Rgb::lerp(bg, col, k);
            }
        }
        EffectKind::Scanner => {
            let speed = params.number(kind, "speed");
            let width = params.number(kind, "width");
            let dir = params.choice(kind, "direction");
            let (core, edge, bg) = match &palette {
                Some(p) => (p.primary, p.tertiary, p.background),
                None => {
                    let c = params.color(kind, "color");
                    (c, c, params.color(kind, "background"))
                }
            };
            let pos = 0.5 - 0.5 * (t * speed * PI * 0.8).cos();
            for (o, p) in out.iter_mut().zip(leds) {
                let d = directional(dir, p.x, p.y).clamp(0.0, 1.0);
                let x = (d - pos) / width.max(0.01);
                let k = (-x * x * 2.0).exp();
                *o = Rgb::lerp(bg, Rgb::lerp(edge, core, k), k);
            }
        }
        EffectKind::Rain => {
            let speed = params.number(kind, "speed");
            let density = params.number(kind, "density");
            let base = params.color(kind, "color");
            let bg = palette
                .map(|p| p.background)
                .unwrap_or_else(|| params.color(kind, "background"));
            let columns = 10.0 + density * 36.0;
            for (o, p) in out.iter_mut().zip(leds) {
                let col = (p.x * columns).floor() as u32;
                let mut k = 0.0f32;
                let mut colour_index = 0usize;
                for drop in 0..2u32 {
                    let seed = hash(col.wrapping_mul(2654) ^ drop.wrapping_mul(0x68e3_1da4));
                    let v = 0.5 + 0.8 * unit(seed);
                    let pos = t * speed * 0.55 * v + unit(hash(seed ^ 0x1b56_c4e9));
                    let cycle = pos.floor() as u32;
                    if unit(hash(seed ^ cycle.wrapping_mul(0x2c1b_3c6d))) > density {
                        continue;
                    }
                    let yd = pos.fract() * 1.5 - 0.25;
                    let behind = yd - p.y;
                    let trail = 0.22;
                    if (0.0..trail).contains(&behind) {
                        let kk = (1.0 - behind / trail).powi(2);
                        if kk > k {
                            k = kk;
                            colour_index = (col + cycle) as usize % 3;
                        }
                    } else if (-0.02..0.0).contains(&behind) {
                        k = k.max(1.0 + behind / 0.02);
                    }
                }
                let c = match &palette {
                    Some(pal) => pal.colors()[colour_index],
                    None => base,
                };
                let c = Rgb::lerp(c, Rgb::WHITE, k.powi(4) * 0.4);
                *o = Rgb::lerp(bg, c, k);
            }
        }
        EffectKind::Bubbles => {
            let speed = params.number(kind, "speed").max(0.05);
            let count = params.number(kind, "count").round().max(1.0) as u32;
            let width = params.number(kind, "width");
            let base = params.color(kind, "color");
            let rainbow = params.toggle(kind, "rainbow");
            let bg = palette
                .map(|p| p.background)
                .unwrap_or_else(|| params.color(kind, "background"));
            let bubbles: Vec<(f32, f32, f32, f32, Rgb)> = (0..count)
                .map(|i| {
                    let seed = hash(i.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b);
                    let period = (2.2 + unit(seed) * 2.0) / speed;
                    let ti = t + unit(hash(seed ^ 1)) * period;
                    let cycle = (ti / period).floor() as u32;
                    let age = (ti / period).fract();
                    let s2 = hash(seed ^ cycle.wrapping_mul(0x27d4_eb2d));
                    let cx = unit(s2) * SCENE_ASPECT;
                    let cy = unit(hash(s2 ^ 0x1656_67b1));
                    let col = match &palette {
                        Some(pal) => pal.colors()[(i + cycle) as usize % 3],
                        None if rainbow => Rgb::from_hsv(unit(hash(s2 ^ 7)), 0.9, 1.0),
                        None => base,
                    };
                    (cx, cy, age * 0.9, (1.0 - age).powf(1.5), col)
                })
                .collect();
            for (o, p) in out.iter_mut().zip(leds) {
                let x = p.x * SCENE_ASPECT;
                let mut acc = bg.to_f32();
                for &(cx, cy, radius, fade, col) in &bubbles {
                    let dist = ((x - cx).powi(2) + (p.y - cy).powi(2)).sqrt();
                    let r = (dist - radius) / width.max(0.01);
                    let k = (-r * r).exp() * fade;
                    let c = col.to_f32();
                    for ch in 0..3 {
                        acc[ch] += c[ch] * k;
                    }
                }
                *o = Rgb::from_f32(acc[0], acc[1], acc[2]);
            }
        }
        EffectKind::Swirl => {
            let speed = params.number(kind, "speed");
            let twist = params.number(kind, "twist");
            for (o, p) in out.iter_mut().zip(leds) {
                let dx = (p.x - 0.5) * SCENE_ASPECT;
                let dy = p.y - 0.5;
                let angle = dy.atan2(dx) / TAU;
                let dist = (dx * dx + dy * dy).sqrt();
                let pos = angle + dist * twist - t * speed * 0.2;
                *o = match &palette {
                    Some(pal) => pal.cyclic(pos),
                    None => Rgb::from_hsv(pos, 1.0, 1.0),
                };
            }
        }
        EffectKind::Lightning => {
            let rate = params.number(kind, "rate").max(0.05);
            let (flash, bg) = match &palette {
                Some(p) => (Rgb::lerp(p.primary, Rgb::WHITE, 0.55), p.background),
                None => (
                    params.color(kind, "color"),
                    params.color(kind, "background"),
                ),
            };
            let slot_len = 1.0 / (rate * 2.0);
            let slot = (t / slot_len).floor() as i64;
            // Strikes of this slot and the previous one (they can overlap).
            let strikes: Vec<(f32, f32, f32)> = [slot - 1, slot]
                .iter()
                .filter_map(|&s| {
                    let seed = hash(s as u32 ^ 0x5bd1_e995);
                    if unit(seed) > 0.5 {
                        return None;
                    }
                    let start = s as f32 * slot_len + unit(hash(seed ^ 3)) * slot_len * 0.5;
                    let tau = t - start;
                    if tau < 0.0 {
                        return None;
                    }
                    // Double flash then a fast decay, like a real stroke.
                    let env = pulse(tau, 0.0, 0.05)
                        + 0.75 * pulse(tau, 0.11, 0.07)
                        + 0.4 * pulse(tau, 0.22, 0.12);
                    let cx = unit(hash(seed ^ 5)) * SCENE_ASPECT;
                    let w = 0.25 + unit(hash(seed ^ 9)) * 0.6;
                    (env > 0.003).then_some((cx, w, env.min(1.0)))
                })
                .collect();
            for (o, p) in out.iter_mut().zip(leds) {
                let x = p.x * SCENE_ASPECT;
                let mut k = 0.0f32;
                for &(cx, w, env) in &strikes {
                    let d = (x - cx) / w;
                    k = k.max(env * (0.12 + 0.88 * (-d * d).exp()));
                }
                *o = Rgb::lerp(bg, flash, k);
            }
        }
        EffectKind::AudioVisualizer => render_audio(kind, params, state, ctx, leds, out),
        EffectKind::ScreenSync => render_screen(kind, params, state, ctx, leds, out),
        EffectKind::Shader => render_shader(kind, params, state, ctx, leds, out),
        EffectKind::HardwareMonitor => render_hardware(kind, params, state, ctx, leds, out),
    }
}

/// Distance along the selected direction, roughly `0..=1`.
fn directional(dir: usize, x: f32, y: f32) -> f32 {
    match DIRECTIONS.get(dir).copied().unwrap_or("") {
        "Droite → gauche" => 1.0 - x,
        "Haut → bas" => y,
        "Bas → haut" => 1.0 - y,
        "Diagonale" => (x + y) * 0.5,
        "Depuis le centre" => {
            let dx = (x - 0.5) * SCENE_ASPECT;
            let dy = y - 0.5;
            (dx * dx + dy * dy).sqrt()
        }
        _ => x,
    }
}

/// Black → red → orange → yellow → white, or the system palette.
fn fire_ramp(heat: f32, palette: Option<&Palette>) -> Rgb {
    let stops: [Rgb; 5] = match palette {
        Some(p) => [
            Rgb::BLACK,
            p.primary.scale(0.55),
            p.primary,
            p.tertiary,
            Rgb::lerp(p.tertiary, Rgb::WHITE, 0.6),
        ],
        None => [
            Rgb::BLACK,
            Rgb::new(170, 16, 0),
            Rgb::new(255, 96, 0),
            Rgb::new(255, 196, 48),
            Rgb::new(255, 240, 190),
        ],
    };
    const POS: [f32; 5] = [0.0, 0.32, 0.58, 0.84, 1.0];
    let h = heat.clamp(0.0, 1.0);
    for i in 1..5 {
        if h <= POS[i] {
            let f = (h - POS[i - 1]) / (POS[i] - POS[i - 1]);
            return Rgb::lerp(stops[i - 1], stops[i], f);
        }
    }
    stops[4]
}

/// Bounds of each device's LEDs in device-local coordinates.
#[derive(Clone, Copy)]
struct Bounds {
    u0: f32,
    u1: f32,
    v0: f32,
    v1: f32,
}

impl Bounds {
    const EMPTY: Bounds = Bounds {
        u0: f32::MAX,
        u1: f32::MIN,
        v0: f32::MAX,
        v1: f32::MIN,
    };
    fn du(&self) -> f32 {
        (self.u1 - self.u0).max(0.0)
    }
    fn dv(&self) -> f32 {
        (self.v1 - self.v0).max(0.0)
    }
    fn nu(&self, u: f32) -> f32 {
        if self.du() < 1e-3 {
            0.5
        } else {
            (u - self.u0) / self.du()
        }
    }
    fn nv(&self, v: f32) -> f32 {
        if self.dv() < 1e-3 {
            0.5
        } else {
            (v - self.v0) / self.dv()
        }
    }
}

fn device_bounds(leds: &[LedPoint]) -> Vec<Bounds> {
    let n = leds.iter().map(|p| p.device + 1).max().unwrap_or(0);
    let mut b = vec![Bounds::EMPTY; n];
    for p in leds {
        let d = &mut b[p.device];
        d.u0 = d.u0.min(p.u);
        d.u1 = d.u1.max(p.u);
        d.v0 = d.v0.min(p.v);
        d.v1 = d.v1.max(p.v);
    }
    b
}

/// How one device shows a level (per-device spectrum, gauges).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Shape {
    /// Rows and columns: bars along `u`, height along `v`.
    Grid,
    /// A single row: bars along `u`, the level is the brightness.
    Row,
    /// A single column (RAM stick, vertical strip): a VU meter.
    Column,
    /// One LED / one point.
    Dot,
}

fn shape_of(b: &Bounds) -> Shape {
    match (b.du() > 0.05, b.dv() > 0.05) {
        (true, true) if b.dv() > b.du() * 1.6 => Shape::Column,
        (true, true) => Shape::Grid,
        (true, false) => Shape::Row,
        (false, true) => Shape::Column,
        (false, false) => Shape::Dot,
    }
}

fn render_audio(
    kind: EffectKind,
    params: &EffectParams,
    state: &mut EffectState,
    ctx: &FrameContext<'_>,
    leds: &[LedPoint],
    out: &mut [Rgb],
) {
    let palette = kind
        .uses_system_palette(params)
        .then(|| kind.system_palette(params, ctx.accent));
    let bg = palette
        .map(|p| p.background)
        .unwrap_or_else(|| params.color(kind, "background"));
    let lo = params.color(kind, "color_a");
    let hi = params.color(kind, "color_b");
    let sensitivity = params.number(kind, "sensitivity");
    let mode = params.choice(kind, "mode");
    let colours = params.choice(kind, "palette");
    let empty = AudioSpectrum::default();
    let audio = ctx.audio.unwrap_or(&empty);
    // Stale data (capture stopped): fade to the background.
    let stale = ctx.audio.is_none() || audio.captured_at.elapsed().as_secs_f32() > 0.5;

    if state.audio_env.len() != leds.len() {
        state.audio_env = vec![0.0; leds.len()];
    }
    let colour_at = |x: f32| -> Rgb {
        match (&palette, colours) {
            (Some(p), _) => p.gradient(x),
            (None, 0) => Rgb::from_hsv(0.66 - x * 0.66, 1.0, 1.0),
            (None, 1) => Rgb::lerp(lo, hi, x),
            (None, _) => ctx.accent,
        }
    };
    // Attack fast, release slow (per LED envelope), frame-rate independent.
    let attack = 1.0 - (-ctx.dt * 45.0).exp();
    let release = 1.0 - (-ctx.dt * 7.0).exp();
    let target_pulse = if stale {
        0.0
    } else {
        (audio.bass * sensitivity).clamp(0.0, 1.0)
    };
    let k = if target_pulse > state.pulse {
        attack
    } else {
        release
    };
    state.pulse += (target_pulse - state.pulse) * k;
    // Beat flash.
    if !stale && audio.beat && state.flash < 0.6 {
        state.flash = 1.0;
        state.flash_index = state.flash_index.wrapping_add(1);
    }
    state.flash *= (-ctx.dt * 7.0).exp();

    let bounds = if mode == 0 {
        device_bounds(leds)
    } else {
        Vec::new()
    };
    let level_of = |band: f32| (audio.sample(band) * sensitivity).clamp(0.0, 1.0);
    // Soft edge of a bar: fully lit below the level, glow just above.
    let bar = |height: f32, level: f32| {
        if height <= level {
            1.0
        } else {
            ((level - height) * 6.0 + 1.0).clamp(0.0, 1.0) * 0.35
        }
    };

    for (i, (o, p)) in out.iter_mut().zip(leds).enumerate() {
        let (target, col) = if stale {
            (0.0, colour_at(p.x))
        } else {
            match mode {
                // Every device shows the whole spectrum, whatever its place
                // on the canvas: nothing is ever left dark.
                0 => {
                    let b = bounds.get(p.device).copied().unwrap_or(Bounds::EMPTY);
                    match shape_of(&b) {
                        Shape::Grid => {
                            let band = b.nu(p.u);
                            let height = 1.0 - b.nv(p.v);
                            (bar(height, level_of(band)), colour_at(band))
                        }
                        Shape::Row => {
                            let band = b.nu(p.u);
                            (level_of(band), colour_at(band))
                        }
                        Shape::Column => {
                            // Side-by-side sticks take neighbouring bands,
                            // like an equaliser.
                            let band = p.x.clamp(0.0, 1.0);
                            let height = 1.0 - b.nv(p.v);
                            let level = level_of(band).max(audio.level * sensitivity * 0.6);
                            (bar(height, level.min(1.0)), colour_at(height))
                        }
                        Shape::Dot => (state.pulse, colour_at(audio.mid.clamp(0.0, 1.0))),
                    }
                }
                // Spectrum across the canvas: x picks the band, y the height.
                1 => (bar(1.0 - p.y, level_of(p.x)), colour_at(p.x)),
                // Pulse: the whole setup breathes with the bass.
                2 => (state.pulse, colour_at(audio.mid.clamp(0.0, 1.0))),
                // Centre wave: louder → wider from the centre.
                3 => {
                    let dx = (p.x - 0.5).abs() * 2.0;
                    let level = (audio.level * sensitivity * 1.4).clamp(0.0, 1.0);
                    let v = if dx <= level {
                        1.0 - dx / level.max(1e-3) * 0.5
                    } else {
                        0.0
                    };
                    (v, colour_at(dx))
                }
                // Flash on every bass hit, next colour each time.
                _ => {
                    let c = match &palette {
                        Some(pal) => pal.colors()[state.flash_index % 3],
                        None if colours == 0 => {
                            Rgb::from_hsv(state.flash_index as f32 * 0.17, 1.0, 1.0)
                        }
                        None => colour_at((state.flash_index % 2) as f32),
                    };
                    (state.flash.max(state.pulse * 0.25), c)
                }
            }
        };
        let env = &mut state.audio_env[i];
        let k = if target > *env { attack } else { release };
        *env += (target - *env) * k;
        *o = Rgb::lerp(bg, col, *env);
    }
}

fn render_screen(
    kind: EffectKind,
    params: &EffectParams,
    state: &mut EffectState,
    ctx: &FrameContext<'_>,
    leds: &[LedPoint],
    out: &mut [Rgb],
) {
    let mode = params.choice(kind, "mode");
    let sat = params.number(kind, "saturation");
    let smoothing = params.number(kind, "smoothing");
    let black = params.number(kind, "black_level");
    if state.smoothed.len() != leds.len() {
        state.smoothed = vec![[0.0; 3]; leds.len()];
    }
    let Some(frame) = ctx.screen else {
        // No frame yet (portal dialog still open): gentle idle glow.
        let k = 0.5 + 0.5 * (ctx.t * 2.0).sin();
        out.fill(Rgb::new(20, 20, 40).scale(0.4 + 0.6 * k));
        return;
    };
    let average = if mode == 2 {
        Some(frame.average(0.0, 0.0, 1.0, 1.0))
    } else {
        None
    };
    let alpha = smoothing_alpha(smoothing, ctx.dt);
    for (i, (o, p)) in out.iter_mut().zip(leds).enumerate() {
        let target = match (mode, average) {
            (_, Some(avg)) => avg,
            (1, _) => {
                // Project the LED to the closest screen edge and average a strip.
                let (ex, ey, horizontal) = nearest_edge(p.x, p.y);
                if horizontal {
                    frame.average_area(ex, ey, p.rx.max(0.02), 0.04)
                } else {
                    frame.average_area(ex, ey, 0.025, p.ry.max(0.035))
                }
            }
            // Exactly the part of the screen this LED covers on the canvas.
            _ => frame.average_area(p.x, p.y, p.rx.max(0.012), p.ry.max(0.02)),
        };
        let target = crush_blacks(target, black).saturate(sat).to_f32();
        let s = &mut state.smoothed[i];
        for c in 0..3 {
            s[c] += (target[c] - s[c]) * alpha;
        }
        *o = Rgb::from_f32(s[0], s[1], s[2]);
    }
}

fn render_shader(
    kind: EffectKind,
    params: &EffectParams,
    state: &mut EffectState,
    ctx: &FrameContext<'_>,
    leds: &[LedPoint],
    out: &mut [Rgb],
) {
    let smoothing = params.number(kind, "smoothing");
    if state.smoothed.len() != leds.len() {
        state.smoothed = vec![[0.0; 3]; leds.len()];
    }
    let Some(frame) = ctx.shader else {
        // Shader still compiling / no GPU: soft breathing placeholder.
        let k = 0.5 + 0.5 * (ctx.t * 1.5).sin();
        out.fill(Rgb::new(10, 30, 40).scale(0.5 + 0.5 * k));
        return;
    };
    let alpha = smoothing_alpha(smoothing, ctx.dt);
    for (i, (o, p)) in out.iter_mut().zip(leds).enumerate() {
        let target = frame
            .average_area(p.x, p.y, p.rx.min(0.05), p.ry.min(0.08))
            .to_f32();
        let s = &mut state.smoothed[i];
        for c in 0..3 {
            s[c] += (target[c] - s[c]) * alpha;
        }
        *o = Rgb::from_f32(s[0], s[1], s[2]);
    }
}

/// Number of sensors tracked by [`render_hardware`].
const SENSOR_COUNT: usize = 5;
const CPU_TEMP: usize = 0;
const CPU_LOAD: usize = 1;
const GPU_TEMP: usize = 2;
const GPU_LOAD: usize = 3;
const MEM_USED: usize = 4;

fn render_hardware(
    kind: EffectKind,
    params: &EffectParams,
    state: &mut EffectState,
    ctx: &FrameContext<'_>,
    leds: &[LedPoint],
    out: &mut [Rgb],
) {
    let low = params.number(kind, "temp_low");
    let high = params.number(kind, "temp_high").max(low + 1.0);
    let cold = params.color(kind, "cold");
    let warm = params.color(kind, "warm");
    let hot = params.color(kind, "hot");
    let gauge = params.choice(kind, "display") == 1;
    let sensor = params.choice(kind, "sensor");

    // Normalised, smoothed readings (colours glide instead of jumping).
    let temp = |c: Option<f32>| c.map(|c| ((c - low) / (high - low)).clamp(0.0, 1.0));
    let raw: [Option<f32>; SENSOR_COUNT] = match ctx.hardware {
        Some(h) => [
            temp(h.cpu_temp),
            h.cpu_load,
            temp(h.gpu_temp),
            h.gpu_load,
            h.mem_used,
        ],
        None => [None; SENSOR_COUNT],
    };
    let k = 1.0 - (-ctx.dt * 2.5).exp();
    for (s, r) in state.sensors.iter_mut().zip(raw) {
        *s = match (*s, r) {
            (Some(old), Some(new)) => Some(old + (new.clamp(0.0, 1.0) - old) * k),
            (None, Some(new)) => Some(new.clamp(0.0, 1.0)),
            (old, None) => old,
        };
    }
    // First sensor of the chain that has a reading.
    let read =
        |chain: &[usize]| -> f32 { chain.iter().find_map(|&j| state.sensors[j]).unwrap_or(0.0) };
    // A sensor chosen by hand: the closest one that exists if it is missing.
    let chosen = |i: usize| -> &'static [usize] {
        match i {
            CPU_TEMP => &[CPU_TEMP, CPU_LOAD],
            CPU_LOAD => &[CPU_LOAD, CPU_TEMP],
            GPU_TEMP => &[GPU_TEMP, GPU_LOAD, CPU_TEMP, CPU_LOAD],
            GPU_LOAD => &[GPU_LOAD, GPU_TEMP, CPU_LOAD, CPU_TEMP],
            _ => &[MEM_USED, CPU_LOAD, CPU_TEMP],
        }
    };
    let auto = |kind: Option<&DeviceKind>| -> &'static [usize] { auto_sensors(kind) };
    let gradient = |v: f32| {
        if v < 0.5 {
            Rgb::lerp(cold, warm, v * 2.0)
        } else {
            Rgb::lerp(warm, hot, v * 2.0 - 1.0)
        }
    };
    let bounds = if gauge {
        device_bounds(leds)
    } else {
        Vec::new()
    };
    for (o, p) in out.iter_mut().zip(leds) {
        let chain = match sensor {
            0 => auto(ctx.device_kinds.get(p.device)),
            n => chosen((n - 1).min(SENSOR_COUNT - 1)),
        };
        let v = read(chain);
        if gauge {
            let b = bounds.get(p.device).copied().unwrap_or(Bounds::EMPTY);
            let pos = match shape_of(&b) {
                Shape::Column => 1.0 - b.nv(p.v),
                Shape::Dot => 0.0,
                _ => b.nu(p.u),
            };
            let lit = ((v - pos) * 12.0 + 0.5).clamp(0.0, 1.0);
            *o = Rgb::lerp(gradient(pos).scale(0.06), gradient(pos.min(v)), lit);
        } else {
            *o = gradient(v);
        }
    }
}

/// "Automatique (par appareil)": the device's own sensor when it has one
/// (graphics card, memory), then the CPU temperature, then the CPU load.
/// Devices without a sensor of their own – keyboard, mouse, mouse mat,
/// headset, fans, strips, motherboard… – follow the CPU temperature (the CPU
/// load when no temperature can be read).
fn auto_sensors(kind: Option<&DeviceKind>) -> &'static [usize] {
    match kind {
        Some(DeviceKind::Gpu) => &[GPU_TEMP, GPU_LOAD, CPU_TEMP, CPU_LOAD],
        Some(DeviceKind::Dram) => &[MEM_USED, CPU_TEMP, CPU_LOAD],
        _ => &[CPU_TEMP, CPU_LOAD],
    }
}

/// Exponential smoothing independent from the frame rate.
fn smoothing_alpha(smoothing: f32, dt: f32) -> f32 {
    if smoothing <= 0.0 {
        1.0
    } else {
        1.0 - smoothing.powf(dt * 30.0)
    }
}

/// Maps `0..black` to black and rescales the rest (dark scenes stay dark
/// instead of glowing grey).
fn crush_blacks(c: Rgb, black: f32) -> Rgb {
    if black <= 0.0 {
        return c;
    }
    let [r, g, b] = c.to_f32();
    let f = |x: f32| ((x - black) / (1.0 - black)).max(0.0);
    Rgb::from_f32(f(r), f(g), f(b))
}

/// Closest screen edge: `(x, y, horizontal_edge)`.
fn nearest_edge(x: f32, y: f32) -> (f32, f32, bool) {
    let d = [
        (x * SCENE_ASPECT, 0),
        ((1.0 - x) * SCENE_ASPECT, 1),
        (y, 2),
        (1.0 - y, 3),
    ];
    let (_, side) = d
        .iter()
        .copied()
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or((0.0, 0));
    match side {
        0 => (0.02, y, false),
        1 => (0.98, y, false),
        2 => (x, 0.03, true),
        _ => (x, 0.97, true),
    }
}

#[inline]
fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// A smooth bump starting at `at`, lasting `len` seconds.
#[inline]
fn pulse(t: f32, at: f32, len: f32) -> f32 {
    let x = (t - at) / len;
    if !(0.0..=4.0).contains(&x) {
        return 0.0;
    }
    // Fast rise, exponential decay.
    (x * 4.0).min(1.0) * (-(x - 0.25).max(0.0) * 1.8).exp()
}

#[inline]
fn led_seed(p: &LedPoint, i: usize) -> u32 {
    hash(p.device as u32 * 7919 + p.led as u32 * 104_729 + i as u32)
}

#[inline]
fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

#[inline]
fn unit(x: u32) -> f32 {
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// Smooth value noise, `0..=1`.
fn noise2(x: f32, y: f32) -> f32 {
    let (xi, yi) = (x.floor(), y.floor());
    let (xf, yf) = (x - xi, y - yi);
    let (i, j) = (xi as i32, yi as i32);
    let h = |a: i32, b: i32| {
        unit(hash(
            (a as u32).wrapping_mul(0x8da6_b343) ^ (b as u32).wrapping_mul(0xd816_3841),
        ))
    };
    let sx = xf * xf * (3.0 - 2.0 * xf);
    let sy = yf * yf * (3.0 - 2.0 * yf);
    let top = h(i, j) + (h(i + 1, j) - h(i, j)) * sx;
    let bottom = h(i, j + 1) + (h(i + 1, j + 1) - h(i, j + 1)) * sx;
    top + (bottom - top) * sy
}

/// Three octaves of [`noise2`].
fn fbm(x: f32, y: f32) -> f32 {
    0.5 * noise2(x, y)
        + 0.3 * noise2(x * 2.03 + 5.2, y * 2.03 + 1.3)
        + 0.2 * noise2(x * 4.01 + 1.7, y * 4.01 + 9.2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::demo_devices;
    use crate::effects::{KDE_ACCENT_KEY, ParamValue, SYSTEM_PALETTE_KEY};
    use crate::layout::Scene;

    fn scene_leds() -> (Vec<LedPoint>, Vec<DeviceKind>) {
        let scene = Scene::build(&demo_devices(), &Default::default(), &Default::default());
        (
            scene.led_points(),
            scene.devices.iter().map(|d| d.info.kind).collect(),
        )
    }

    /// Automatic sensor: peripherals without a sensor follow the CPU
    /// temperature, or the CPU load when the temperature is unknown; GPU and
    /// RAM use their own sensor, and the CPU when it is missing.
    #[test]
    fn hardware_monitor_auto_falls_back_to_the_cpu() {
        let kind = EffectKind::HardwareMonitor;
        let params = EffectParams::default();
        let kinds = [
            DeviceKind::Keyboard,
            DeviceKind::Mouse,
            DeviceKind::MouseMat,
            DeviceKind::Headset,
            DeviceKind::Gpu,
            DeviceKind::Dram,
            DeviceKind::Motherboard,
        ];
        let leds: Vec<LedPoint> = (0..kinds.len())
            .map(|d| LedPoint {
                device: d,
                led: 0,
                x: 0.5,
                y: 0.5,
                u: 0.5,
                v: 0.5,
                rx: 0.01,
                ry: 0.01,
            })
            .collect();
        let low = params.number(kind, "temp_low");
        let high = params.number(kind, "temp_high");
        let run = |hw: HardwareSample| {
            let mut state = EffectState::default();
            let ctx = FrameContext {
                hardware: Some(&hw),
                device_kinds: &kinds,
                ..FrameContext::basic(0.0, 1.0 / 60.0, Rgb::WHITE)
            };
            let mut out = vec![Rgb::BLACK; leds.len()];
            render(kind, &params, &mut state, &ctx, &leds, &mut out);
            out
        };
        let sample = |cpu_temp, cpu_load, gpu_temp, gpu_load, mem_used| HardwareSample {
            cpu_temp,
            cpu_load,
            gpu_temp,
            gpu_load,
            mem_used,
            captured_at: std::time::Instant::now(),
        };
        let cold = params.color(kind, "cold");
        let hot = params.color(kind, "hot");
        // Hot CPU, idle CPU load, cold GPU, little memory used.
        let out = run(sample(
            Some(high),
            Some(0.0),
            Some(low),
            Some(0.0),
            Some(0.0),
        ));
        for (d, c) in out.iter().enumerate() {
            let expected = match kinds[d] {
                DeviceKind::Gpu | DeviceKind::Dram => cold,
                _ => hot,
            };
            assert_eq!(*c, expected, "{:?}", kinds[d]);
        }
        // No CPU temperature: the CPU load (here at 100 %) takes over.
        let out = run(sample(None, Some(1.0), Some(low), Some(0.0), Some(0.0)));
        for (d, c) in out.iter().enumerate().take(4) {
            assert_eq!(*c, hot, "{:?}", kinds[d]);
        }
        // A GPU and RAM without readings follow the CPU temperature too.
        let out = run(sample(Some(high), Some(0.0), None, None, None));
        assert!(out.iter().all(|c| *c == hot), "{out:?}");
    }

    #[test]
    fn every_effect_renders_without_panicking() {
        let (leds, kinds) = scene_leds();
        let audio = AudioSpectrum {
            bands: [0.6; crate::signal::AUDIO_BANDS],
            bass: 0.8,
            mid: 0.4,
            treble: 0.2,
            level: 0.5,
            beat: true,
            captured_at: std::time::Instant::now(),
        };
        let screen = ScreenFrame {
            width: 4,
            height: 2,
            pixels: vec![Rgb::new(200, 10, 10); 8],
            captured_at: std::time::Instant::now(),
        };
        let hw = HardwareSample {
            cpu_temp: Some(70.0),
            cpu_load: Some(0.5),
            gpu_temp: None,
            gpu_load: Some(0.9),
            mem_used: Some(0.4),
            captured_at: std::time::Instant::now(),
        };
        for palette in [false, true] {
            let mut params = EffectParams::default();
            for kind in EffectKind::ALL {
                if kind.supports_system_palette() {
                    params.set(kind, SYSTEM_PALETTE_KEY, ParamValue::Toggle(palette));
                }
            }
            for kind in EffectKind::ALL {
                let mut state = EffectState::default();
                let mut out = vec![Rgb::BLACK; leds.len()];
                for frame in 0..30 {
                    let ctx = FrameContext {
                        t: frame as f32 / 7.0,
                        dt: 1.0 / 60.0,
                        audio: Some(&audio),
                        screen: Some(&screen),
                        shader: Some(&screen),
                        hardware: Some(&hw),
                        accent: Rgb::new(61, 174, 233),
                        device_kinds: &kinds,
                    };
                    render(kind, &params, &mut state, &ctx, &leds, &mut out);
                }
            }
        }
    }

    #[test]
    fn audio_reaches_every_device() {
        let (leds, kinds) = scene_leds();
        let audio = AudioSpectrum {
            bands: [0.7; crate::signal::AUDIO_BANDS],
            bass: 0.7,
            mid: 0.7,
            treble: 0.7,
            level: 0.7,
            beat: false,
            captured_at: std::time::Instant::now(),
        };
        let params = EffectParams::default();
        let mut state = EffectState::default();
        let mut out = vec![Rgb::BLACK; leds.len()];
        for frame in 0..60 {
            let ctx = FrameContext {
                audio: Some(&audio),
                device_kinds: &kinds,
                ..FrameContext::basic(frame as f32 / 60.0, 1.0 / 60.0, Rgb::WHITE)
            };
            render(
                EffectKind::AudioVisualizer,
                &params,
                &mut state,
                &ctx,
                &leds,
                &mut out,
            );
        }
        let bg = params.color(EffectKind::AudioVisualizer, "background");
        let n_devices = leds.iter().map(|p| p.device).max().unwrap() + 1;
        for d in 0..n_devices {
            let lit = leds
                .iter()
                .zip(&out)
                .filter(|(p, c)| p.device == d && **c != bg)
                .count();
            assert!(lit > 0, "device {d} stays dark with music playing");
        }
    }

    #[test]
    fn screen_sync_samples_the_led_area() {
        let (leds, kinds) = scene_leds();
        // Left half red, right half blue.
        let (w, h) = (32, 18);
        let pixels = (0..w * h)
            .map(|i| {
                if (i % w) < w / 2 {
                    Rgb::new(255, 0, 0)
                } else {
                    Rgb::new(0, 0, 255)
                }
            })
            .collect();
        let screen = ScreenFrame {
            width: w,
            height: h,
            pixels,
            captured_at: std::time::Instant::now(),
        };
        let mut params = EffectParams::default();
        params.set(EffectKind::ScreenSync, "smoothing", ParamValue::Number(0.0));
        let mut out = vec![Rgb::BLACK; leds.len()];
        let ctx = FrameContext {
            screen: Some(&screen),
            device_kinds: &kinds,
            ..FrameContext::basic(0.0, 1.0 / 60.0, Rgb::WHITE)
        };
        render(
            EffectKind::ScreenSync,
            &params,
            &mut EffectState::default(),
            &ctx,
            &leds,
            &mut out,
        );
        for (p, c) in leds.iter().zip(&out) {
            if p.x + p.rx.max(0.012) < 0.48 {
                assert!(c.r > 200 && c.b < 30, "{p:?} {c:?}");
            } else if p.x - p.rx.max(0.012) > 0.52 {
                assert!(c.b > 200 && c.r < 30, "{p:?} {c:?}");
            }
        }
    }

    #[test]
    fn static_is_uniform() {
        let (leds, _) = scene_leds();
        let mut out = vec![Rgb::BLACK; leds.len()];
        let ctx = FrameContext::basic(0.0, 0.016, Rgb::WHITE);
        render(
            EffectKind::Static,
            &EffectParams::default(),
            &mut EffectState::default(),
            &ctx,
            &leds,
            &mut out,
        );
        assert!(out.iter().all(|c| *c == out[0]));
    }

    #[test]
    fn static_follows_kde_accent_when_synchronised() {
        let (leds, _) = scene_leds();
        let mut out = vec![Rgb::BLACK; leds.len()];
        let accent = Rgb::new(61, 174, 233);
        let ctx = FrameContext::basic(0.0, 0.016, accent);
        let mut params = EffectParams::default();
        params.set(EffectKind::Static, KDE_ACCENT_KEY, ParamValue::Toggle(true));
        render(
            EffectKind::Static,
            &params,
            &mut EffectState::default(),
            &ctx,
            &leds,
            &mut out,
        );
        assert!(out.iter().all(|c| *c == accent));
    }

    #[test]
    fn system_palette_uses_the_accent_as_dominant_colour() {
        let (leds, _) = scene_leds();
        let accent = Rgb::new(233, 84, 32);
        let mut params = EffectParams::default();
        params.set(
            EffectKind::ColorCycle,
            SYSTEM_PALETTE_KEY,
            ParamValue::Toggle(true),
        );
        let mut out = vec![Rgb::BLACK; leds.len()];
        render(
            EffectKind::ColorCycle,
            &params,
            &mut EffectState::default(),
            &FrameContext::basic(0.0, 0.016, accent),
            &leds,
            &mut out,
        );
        assert!(out.iter().all(|c| *c == accent));
    }

    #[test]
    fn noise_is_bounded() {
        for i in 0..500 {
            let x = i as f32 * 0.37 - 40.0;
            let n = fbm(x, x * 0.7 + 3.0);
            assert!((0.0..=1.0).contains(&n));
        }
    }
}
