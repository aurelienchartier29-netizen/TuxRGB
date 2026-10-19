//! The setup canvas: where every device sits and where each LED is.
//!
//! Coordinates are normalised to `0.0..=1.0` on both axes of a 16:9 scene,
//! exactly like SignalRGB's layout canvas. Effects are evaluated in this
//! space, so a wave really travels across the physical setup.

use crate::device::{DeviceInfo, DeviceKey, DeviceKind, ZoneKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::f32::consts::TAU;

/// Scene aspect ratio (width / height).
pub const SCENE_ASPECT: f32 = 16.0 / 9.0;

/// Position, size and orientation of a device on the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Clockwise rotation in degrees: 0, 90, 180 or 270.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: u16,
    /// Mirrored horizontally (before the rotation), e.g. a strip whose
    /// first LED is on the right.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mirror: bool,
}

fn is_zero(v: &u16) -> bool {
    *v == 0
}

impl Placement {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            x,
            y,
            w,
            h,
            rotation: 0,
            mirror: false,
        }
    }

    /// Quarter turns (0..=3).
    pub fn quarter_turns(&self) -> u16 {
        (self.rotation / 90) % 4
    }

    /// Turns the device by 90° clockwise around its centre; the rectangle
    /// keeps its physical proportions (16:9 scene units are converted).
    pub fn rotated_cw(self) -> Self {
        let (cx, cy) = self.center();
        let w = self.h / SCENE_ASPECT;
        let h = self.w * SCENE_ASPECT;
        Self {
            x: cx - w * 0.5,
            y: cy - h * 0.5,
            w,
            h,
            rotation: (self.quarter_turns() * 90 + 90) % 360,
            ..self
        }
        .clamped()
    }

    /// `(w, h)` of a device kind's default size, in this orientation.
    pub fn oriented_size(&self, (w, h): (f32, f32)) -> (f32, f32) {
        if self.quarter_turns() % 2 == 1 {
            (h / SCENE_ASPECT, w * SCENE_ASPECT)
        } else {
            (w, h)
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }

    pub fn clamped(mut self) -> Self {
        self.w = self.w.clamp(0.01, 1.0);
        self.h = self.h.clamp(0.01, 1.0);
        self.x = self.x.clamp(0.0, 1.0 - self.w);
        self.y = self.y.clamp(0.0, 1.0 - self.h);
        self
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }
}

/// A single LED in scene space.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct LedPoint {
    /// Index of the device inside [`Scene::devices`].
    pub device: usize,
    /// LED index inside the device (OpenRGB order).
    pub led: usize,
    /// Position on the canvas (`0..=1`, the canvas is the screen).
    pub x: f32,
    pub y: f32,
    /// Position inside the device rectangle (`0..=1`, scene orientation):
    /// lets "per device" effects show their whole pattern on every device.
    pub u: f32,
    pub v: f32,
    /// Half width / height of the area this LED covers on the canvas, used
    /// by Screen Sync to average exactly the matching part of the screen.
    pub rx: f32,
    pub ry: f32,
}

/// Geometry of one LED: centre and half extents, in scene units.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct LedGeom {
    pub x: f32,
    pub y: f32,
    pub rx: f32,
    pub ry: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneDevice {
    pub info: DeviceInfo,
    pub placement: Placement,
    pub enabled: bool,
    /// LED positions in scene space, OpenRGB order.
    pub leds: Vec<(f32, f32)>,
    /// Area covered by each LED (same order as `leds`).
    pub geometry: Vec<LedGeom>,
}

/// Every device on the canvas plus the flattened list of LEDs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub devices: Vec<SceneDevice>,
}

impl Scene {
    /// Builds a scene from devices and saved placements; devices without a
    /// saved placement get an automatic one.
    pub fn build(
        devices: &[DeviceInfo],
        saved: &BTreeMap<DeviceKey, Placement>,
        disabled: &std::collections::BTreeSet<DeviceKey>,
    ) -> Self {
        let mut per_kind: BTreeMap<String, usize> = BTreeMap::new();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for d in devices {
            *counts.entry(format!("{:?}", d.kind)).or_default() += 1;
        }
        let devices = devices
            .iter()
            .map(|info| {
                let kind_key = format!("{:?}", info.kind);
                let index = per_kind.entry(kind_key.clone()).or_default();
                let placement = saved
                    .get(&info.key)
                    .copied()
                    .unwrap_or_else(|| default_placement(info.kind, *index, counts[&kind_key]));
                *index += 1;
                let placement = placement.clamped();
                let geometry = led_geometry(info, &placement);
                SceneDevice {
                    leds: geometry.iter().map(|g| (g.x, g.y)).collect(),
                    geometry,
                    enabled: !disabled.contains(&info.key),
                    info: info.clone(),
                    placement,
                }
            })
            .collect();
        Self { devices }
    }

    /// Moves / resizes one device and recomputes its LEDs.
    pub fn set_placement(&mut self, index: usize, placement: Placement) {
        if let Some(dev) = self.devices.get_mut(index) {
            dev.placement = placement.clamped();
            dev.geometry = led_geometry(&dev.info, &dev.placement);
            dev.leds = dev.geometry.iter().map(|g| (g.x, g.y)).collect();
        }
    }

    /// Flattened LED list for the engine (only enabled, controllable devices).
    pub fn led_points(&self) -> Vec<LedPoint> {
        self.devices
            .iter()
            .enumerate()
            .filter(|(_, d)| d.enabled && d.info.controllable)
            .flat_map(|(di, d)| {
                let p = d.placement;
                d.geometry.iter().enumerate().map(move |(li, g)| LedPoint {
                    device: di,
                    led: li,
                    x: g.x,
                    y: g.y,
                    u: ((g.x - p.x) / p.w.max(1e-4)).clamp(0.0, 1.0),
                    v: ((g.y - p.y) / p.h.max(1e-4)).clamp(0.0, 1.0),
                    rx: g.rx,
                    ry: g.ry,
                })
            })
            .collect()
    }

    pub fn total_leds(&self) -> usize {
        self.devices.iter().map(|d| d.info.led_count).sum()
    }

    /// Topmost device under a scene coordinate.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<usize> {
        self.devices
            .iter()
            .enumerate()
            .rev()
            .find(|(_, d)| d.placement.contains(x, y))
            .map(|(i, _)| i)
    }
}

/// Sensible default placement, grouped like a real desk + PC case.
pub fn default_placement(kind: DeviceKind, index: usize, count: usize) -> Placement {
    let (w, h) = kind.default_size();
    let i = index as f32;
    let spread = |base: f32, step: f32| base + step * i;
    let (x, y) = match kind {
        // PC case on the right half of the canvas.
        DeviceKind::Motherboard => (0.62, 0.12),
        DeviceKind::Dram => (spread(0.86, 0.035), 0.12),
        DeviceKind::Gpu => (0.58, spread(0.48, 0.10)),
        DeviceKind::Cooler => {
            let col = (index % 3) as f32;
            let row = (index / 3) as f32;
            (0.88 - col * 0.10, 0.62 + row * 0.18)
        }
        DeviceKind::Case => (0.55, 0.05),
        // Desk on the left / bottom.
        DeviceKind::Keyboard => (0.06, spread(0.70, 0.16)),
        DeviceKind::Keypad => (spread(0.47, 0.11), 0.70),
        DeviceKind::Mouse => (spread(0.47, 0.06), 0.84),
        DeviceKind::MouseMat => (0.04, 0.80),
        DeviceKind::Headset | DeviceKind::HeadsetStand => (spread(0.38, 0.09), 0.52),
        DeviceKind::Gamepad => (spread(0.06, 0.11), 0.55),
        DeviceKind::Speaker => {
            if index.is_multiple_of(2) {
                (0.02, 0.35)
            } else {
                (0.47, 0.35)
            }
        }
        DeviceKind::Monitor => (0.06, spread(0.06, 0.08)),
        DeviceKind::Laptop => (0.10, spread(0.40, 0.14)),
        DeviceKind::LedStrip => (0.04, spread(0.02, 0.04).min(0.95)),
        DeviceKind::Light => (spread(0.06, 0.08), 0.20),
        _ => {
            let col = (index % 6) as f32;
            let row = (index / 6) as f32;
            (0.06 + col * 0.09, 0.30 + row * 0.10)
        }
    };
    let _ = count;
    Placement::new(x, y, w, h).clamped()
}

/// Computes where each LED of a device is, given its placement.
pub fn led_positions(info: &DeviceInfo, p: &Placement) -> Vec<(f32, f32)> {
    led_geometry(info, p).iter().map(|g| (g.x, g.y)).collect()
}

/// Position and covered area of each LED, honouring rotation and mirror.
///
/// The LEDs are first laid out in the device's own (unrotated) frame, then
/// mirrored / rotated and mapped onto the placement rectangle.
pub fn led_geometry(info: &DeviceInfo, p: &Placement) -> Vec<LedGeom> {
    let turns = p.quarter_turns();
    // Unrotated rectangle with the same physical proportions.
    let (lw, lh) = if turns % 2 == 1 {
        (p.h / SCENE_ASPECT, p.w * SCENE_ASPECT)
    } else {
        (p.w, p.h)
    };
    let local = Placement::new(0.0, 0.0, lw.max(1e-4), lh.max(1e-4));
    let raw = layout_unrotated(info, &local);
    raw.into_iter()
        .map(|g| {
            // Normalised position / half extents inside the local frame.
            let mut a = g.x / local.w;
            let b = g.y / local.h;
            let (ra, rb) = (g.rx / local.w, g.ry / local.h);
            if p.mirror {
                a = 1.0 - a;
            }
            let (u, v, ru, rv) = match turns {
                1 => (1.0 - b, a, rb, ra),
                2 => (1.0 - a, 1.0 - b, ra, rb),
                3 => (b, 1.0 - a, rb, ra),
                _ => (a, b, ra, rb),
            };
            LedGeom {
                x: (p.x + u * p.w).clamp(0.0, 1.0),
                y: (p.y + v * p.h).clamp(0.0, 1.0),
                rx: (ru * p.w).max(0.004),
                ry: (rv * p.h).max(0.006),
            }
        })
        .collect()
}

fn layout_unrotated(info: &DeviceInfo, p: &Placement) -> Vec<LedGeom> {
    let mut out = Vec::with_capacity(info.led_count);
    let zones: Vec<_> = info.zones.iter().filter(|z| z.led_count > 0).collect();
    if zones.is_empty() {
        // Unknown topology: spread LEDs on a line.
        line(&mut out, info.led_count, p.x, p.y, p.w, p.h);
        return out;
    }
    let n_zones = zones.len() as f32;
    let vertical = p.h > p.w * SCENE_ASPECT * 1.15;
    for (zi, zone) in zones.iter().enumerate() {
        // Each zone gets a band of the device rectangle.
        let (zx, zy, zw, zh) = if vertical {
            let w = p.w / n_zones;
            (p.x + w * zi as f32, p.y, w, p.h)
        } else {
            let h = p.h / n_zones;
            (p.x, p.y + h * zi as f32, p.w, h)
        };
        match zone.kind {
            ZoneKind::Single => {
                for _ in 0..zone.led_count {
                    out.push(LedGeom {
                        x: zx + zw * 0.5,
                        y: zy + zh * 0.5,
                        rx: zw * 0.5,
                        ry: zh * 0.5,
                    });
                }
            }
            ZoneKind::Matrix => {
                let (cols, rows) = zone
                    .matrix
                    .unwrap_or_else(|| auto_grid(zone.led_count, zw, zh));
                let cols = cols.max(1);
                let rows = rows.max(1);
                for i in 0..zone.led_count {
                    let c = (i % cols) as f32;
                    let r = ((i / cols) % rows) as f32;
                    out.push(LedGeom {
                        x: zx + zw * (c + 0.5) / cols as f32,
                        y: zy + zh * (r + 0.5) / rows as f32,
                        rx: zw * 0.5 / cols as f32,
                        ry: zh * 0.5 / rows as f32,
                    });
                }
            }
            ZoneKind::Ring => ring(&mut out, zone.led_count, zx, zy, zw, zh),
            ZoneKind::Linear => {
                if info.kind == DeviceKind::Cooler && zone.led_count >= 4 {
                    ring(&mut out, zone.led_count, zx, zy, zw, zh);
                } else {
                    line(&mut out, zone.led_count, zx, zy, zw, zh);
                }
            }
        }
    }
    // OpenRGB guarantees zones cover all LEDs; pad defensively otherwise.
    while out.len() < info.led_count {
        let (x, y) = p.center();
        out.push(LedGeom {
            x,
            y,
            rx: p.w * 0.5,
            ry: p.h * 0.5,
        });
    }
    out.truncate(info.led_count);
    out
}

fn line(out: &mut Vec<LedGeom>, n: usize, x: f32, y: f32, w: f32, h: f32) {
    let horizontal = w * SCENE_ASPECT >= h;
    let n_f = n.max(1) as f32;
    for i in 0..n {
        let t = (i as f32 + 0.5) / n_f;
        out.push(if horizontal {
            LedGeom {
                x: x + w * t,
                y: y + h * 0.5,
                rx: w * 0.5 / n_f,
                ry: h * 0.5,
            }
        } else {
            LedGeom {
                x: x + w * 0.5,
                y: y + h * t,
                rx: w * 0.5,
                ry: h * 0.5 / n_f,
            }
        });
    }
}

fn ring(out: &mut Vec<LedGeom>, n: usize, x: f32, y: f32, w: f32, h: f32) {
    let cx = x + w * 0.5;
    let cy = y + h * 0.5;
    // Keep the ring circular on screen despite the 16:9 scene.
    let r = (w * SCENE_ASPECT).min(h) * 0.40;
    // Each LED covers its arc of the ring and a band across it.
    let half = (std::f32::consts::PI * r / n.max(1) as f32).max(r * 0.3);
    for i in 0..n {
        let a = TAU * i as f32 / n as f32 - std::f32::consts::FRAC_PI_2;
        out.push(LedGeom {
            x: cx + a.cos() * r / SCENE_ASPECT,
            y: cy + a.sin() * r,
            rx: half / SCENE_ASPECT,
            ry: half,
        });
    }
}

fn auto_grid(n: usize, w: f32, h: f32) -> (usize, usize) {
    let aspect = (w * SCENE_ASPECT / h.max(1e-3)).max(0.1);
    let cols = ((n as f32 * aspect).sqrt().ceil() as usize).max(1);
    let rows = n.div_ceil(cols).max(1);
    (cols, rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::demo_devices;

    #[test]
    fn every_led_gets_a_position_inside_the_canvas() {
        let devices = demo_devices();
        let scene = Scene::build(&devices, &BTreeMap::new(), &Default::default());
        assert_eq!(scene.devices.len(), devices.len());
        for d in &scene.devices {
            assert_eq!(d.leds.len(), d.info.led_count);
            for &(x, y) in &d.leds {
                assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y));
            }
        }
        assert_eq!(scene.led_points().len(), scene.total_leds());
    }

    #[test]
    fn rotation_and_mirror_move_the_first_led() {
        let devices = demo_devices();
        let strip = devices
            .iter()
            .find(|d| d.kind == DeviceKind::LedStrip)
            .expect("demo strip");
        let p = Placement::new(0.1, 0.1, 0.4, 0.05);
        let first = |p: &Placement| led_geometry(strip, p)[0];
        // Unrotated horizontal strip: first LED on the left.
        assert!(first(&p).x < 0.15);
        // Mirrored: on the right.
        let m = Placement { mirror: true, ..p };
        assert!(first(&m).x > 0.45);
        // Rotated 90° clockwise: becomes vertical, first LED on top.
        let r = p.rotated_cw();
        assert_eq!(r.rotation, 90);
        assert!(r.h > r.w);
        let g = first(&r);
        assert!(g.y < r.y + r.h * 0.2, "{g:?} {r:?}");
        // Footprints follow the orientation: a vertical strip LED is wider
        // than tall in scene units only along x.
        assert!(g.ry < r.h * 0.5);
        for rot in 0..4 {
            let mut q = p;
            for _ in 0..rot {
                q = q.rotated_cw();
            }
            for g in led_geometry(strip, &q) {
                assert!(q.contains(g.x, g.y) || (g.x - q.x).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn footprints_tile_a_matrix() {
        let devices = demo_devices();
        let kb = devices
            .iter()
            .find(|d| d.kind == DeviceKind::Keyboard)
            .expect("demo keyboard");
        let p = Placement::new(0.0, 0.5, 0.5, 0.2);
        let geo = led_geometry(kb, &p);
        let area: f32 = geo.iter().map(|g| 4.0 * g.rx * g.ry).sum();
        assert!(area <= p.w * p.h * 1.05, "{area}");
        assert!(geo.iter().all(|g| g.rx > 0.0 && g.ry > 0.0));
    }

    #[test]
    fn hit_test_finds_device() {
        let devices = demo_devices();
        let scene = Scene::build(&devices, &BTreeMap::new(), &Default::default());
        let (cx, cy) = scene.devices[0].placement.center();
        assert!(scene.hit_test(cx, cy).is_some());
    }
}
