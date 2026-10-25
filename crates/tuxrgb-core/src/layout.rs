//! The setup canvas: where every device sits and where each LED is.
//!
//! Coordinates are normalised to `0.0..=1.0` on both axes of a 16:9 scene,
//! exactly like SignalRGB's layout canvas. Effects are evaluated in this
//! space, so a wave really travels across the physical setup.
//!
//! The items of the canvas are the devices and, for a header split into
//! fans (see [`crate::lighting`]), each of its fans: a fan sits in its
//! device's rectangle, where the header's LEDs were, until the user moves
//! it; from then on it has a place of its own (front fans stacked in the
//! case while the board stays put), turns and resizes on its own.

use crate::device::{DeviceInfo, DeviceKey, DeviceKind, ZoneKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::TAU;
use std::ops::Range;

/// Scene aspect ratio (width / height).
pub const SCENE_ASPECT: f32 = 16.0 / 9.0;

/// Position, size and orientation of an item on the canvas.
///
/// `x, y, w, h` is the item's rectangle *before* it is turned (scene
/// units, with the item's own proportions); it turns around its centre by
/// `rotation` degrees clockwise – any angle since TuxRGB 3.1. The box the
/// turned rectangle occupies on the canvas is [`Placement::bounds`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "PlacementRepr", into = "PlacementRepr")]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Clockwise rotation in degrees, `0.0..360.0`.
    pub rotation: f32,
    /// Mirrored horizontally (before the rotation), e.g. a strip whose
    /// first LED is on the right.
    pub mirror: bool,
}

/// Saved form of a [`Placement`]. TuxRGB 3.0 and older only knew quarter
/// turns, saved as `rotation` with `x, y, w, h` being the *turned*
/// rectangle; 3.1 saves `angle` with the rectangle before the turn.
#[derive(Serialize, Deserialize)]
struct PlacementRepr {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    rotation: u16,
    #[serde(default, skip_serializing_if = "is_zero_angle")]
    angle: f32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    mirror: bool,
}

fn is_zero(v: &u16) -> bool {
    *v == 0
}

fn is_zero_angle(v: &f32) -> bool {
    *v == 0.0
}

impl From<PlacementRepr> for Placement {
    fn from(r: PlacementRepr) -> Self {
        let mut p = Placement {
            mirror: r.mirror,
            ..Placement::new(r.x, r.y, r.w, r.h)
        };
        if r.angle != 0.0 {
            p.rotation = normalize_angle(r.angle);
        } else if !r.rotation.is_multiple_of(360) {
            // A quarter turn saved by TuxRGB ≤ 3.0: back to the rectangle
            // before the turn (same centre, physical proportions kept).
            let quarter = (r.rotation / 90) % 4;
            if quarter % 2 == 1 {
                let (cx, cy) = p.center();
                p.w = r.h / SCENE_ASPECT;
                p.h = r.w * SCENE_ASPECT;
                p.x = cx - p.w * 0.5;
                p.y = cy - p.h * 0.5;
            }
            p.rotation = f32::from(quarter) * 90.0;
        }
        p
    }
}

impl From<Placement> for PlacementRepr {
    fn from(p: Placement) -> Self {
        PlacementRepr {
            x: p.x,
            y: p.y,
            w: p.w,
            h: p.h,
            rotation: 0,
            angle: normalize_angle(p.rotation),
            mirror: p.mirror,
        }
    }
}

/// `deg` in `0.0..360.0` (a hair under 360 is 0).
pub fn normalize_angle(deg: f32) -> f32 {
    if !deg.is_finite() {
        return 0.0;
    }
    let a = deg.rem_euclid(360.0);
    if a > 359.995 { 0.0 } else { a }
}

/// An angle with the right angles magnetic (±4°): 0, 90, 180 and 270° are
/// easy to reach while dragging the rotation slider or knob.
pub fn snap_angle(deg: f32) -> f32 {
    let a = normalize_angle(deg);
    let nearest = (a / 90.0).round() * 90.0;
    if (a - nearest).abs() <= 4.0 {
        normalize_angle(nearest)
    } else {
        a
    }
}

impl Placement {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            x,
            y,
            w,
            h,
            rotation: 0.0,
            mirror: false,
        }
    }

    /// The same rectangle turned to `deg` degrees (clockwise), around its
    /// centre.
    pub fn with_rotation(self, deg: f32) -> Self {
        Self {
            rotation: normalize_angle(deg),
            ..self
        }
        .clamped()
    }

    /// `(sin, cos)` of the rotation.
    fn sin_cos(&self) -> (f32, f32) {
        self.rotation.to_radians().sin_cos()
    }

    /// Point of the rectangle (`a, b` in `0..=1` before the turn, `0, 0` =
    /// top left) → canvas. The turn happens in physical proportions (16:9
    /// canvas), so a square stays a square at any angle.
    pub fn point(&self, a: f32, b: f32) -> (f32, f32) {
        let (cx, cy) = self.center();
        let dx = (a - 0.5) * self.w * SCENE_ASPECT;
        let dy = (b - 0.5) * self.h;
        let (s, c) = self.sin_cos();
        (cx + (dx * c - dy * s) / SCENE_ASPECT, cy + dx * s + dy * c)
    }

    /// Canvas → point of the rectangle before the turn (inverse of
    /// [`Placement::point`]; inside when both are in `0..=1`).
    pub fn local(&self, x: f32, y: f32) -> (f32, f32) {
        let (cx, cy) = self.center();
        let dx = (x - cx) * SCENE_ASPECT;
        let dy = y - cy;
        let (s, c) = self.sin_cos();
        let lx = dx * c + dy * s;
        let ly = -dx * s + dy * c;
        (
            lx / (self.w * SCENE_ASPECT).max(1e-6) + 0.5,
            ly / self.h.max(1e-6) + 0.5,
        )
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (a, b) = self.local(x, y);
        (-1e-4..=1.0001).contains(&a) && (-1e-4..=1.0001).contains(&b)
    }

    /// Half width / height of the turned rectangle's box on the canvas.
    pub fn half_extents(&self) -> (f32, f32) {
        let (s, c) = self.sin_cos();
        let (s, c) = (s.abs(), c.abs());
        let hw = self.w * SCENE_ASPECT * 0.5;
        let hh = self.h * 0.5;
        ((c * hw + s * hh) / SCENE_ASPECT, s * hw + c * hh)
    }

    /// Box of the turned rectangle on the canvas: `(x0, y0, x1, y1)`.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        let (cx, cy) = self.center();
        let (hx, hy) = self.half_extents();
        (cx - hx, cy - hy, cx + hx, cy + hy)
    }

    /// Sizes kept sensible and the item kept on the canvas (a turned item
    /// larger than the canvas stays centred on it).
    pub fn clamped(mut self) -> Self {
        self.w = self.w.clamp(0.01, 1.0);
        self.h = self.h.clamp(0.01, 1.0);
        self.rotation = normalize_angle(self.rotation);
        let (hx, hy) = self.half_extents();
        let (cx, cy) = self.center();
        let fit = |c: f32, half: f32| {
            if half * 2.0 <= 1.0 {
                c.clamp(half, 1.0 - half)
            } else {
                0.5
            }
        };
        let (cx, cy) = (fit(cx, hx), fit(cy, hy));
        self.x = cx - self.w * 0.5;
        self.y = cy - self.h * 0.5;
        self
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    /// Resized from its corner handle: the top-left corner of the turned
    /// rectangle stays where it is and the bottom-right one follows the
    /// point `(x, y)` (canvas), whatever the angle.
    pub fn resized_to(&self, x: f32, y: f32) -> Self {
        let (ax, ay) = self.point(0.0, 0.0);
        let (dx, dy) = ((x - ax) * SCENE_ASPECT, y - ay);
        let (s, c) = self.sin_cos();
        // Into the rectangle's own frame.
        let lx = dx * c + dy * s;
        let ly = -dx * s + dy * c;
        let w = (lx / SCENE_ASPECT).clamp(0.02, 1.0);
        let h = ly.clamp(0.02, 1.0);
        // New centre: half the new size from the corner, turned.
        let (hx, hy) = (w * SCENE_ASPECT * 0.5, h * 0.5);
        let cx = ax + (hx * c - hy * s) / SCENE_ASPECT;
        let cy = ay + hx * s + hy * c;
        Self {
            x: cx - w * 0.5,
            y: cy - h * 0.5,
            w,
            h,
            ..*self
        }
    }

    /// Angle (degrees, clockwise, 0 = the rectangle's top pointing up)
    /// that makes the top of the rectangle face the point `(x, y)`: the
    /// rotation knob of the canvas.
    pub fn angle_towards(&self, x: f32, y: f32) -> f32 {
        let (cx, cy) = self.center();
        let (dx, dy) = ((x - cx) * SCENE_ASPECT, y - cy);
        normalize_angle(dy.atan2(dx).to_degrees() + 90.0)
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
    /// Per-zone / per-fan settings (see [`crate::lighting`]).
    pub lighting: crate::lighting::DeviceLighting,
    /// The fans of its split headers, items of their own on the canvas.
    pub segments: Vec<SceneSegment>,
}

/// One fan of a split header (a part, see [`crate::lighting`]).
#[derive(Clone, Debug, PartialEq)]
pub struct SceneSegment {
    pub zone: usize,
    pub part: usize,
    /// Its LEDs, in device LED indices.
    pub leds: Range<usize>,
    /// Key of its placement in the saved layout ([`segment_key`]).
    pub key: DeviceKey,
    pub placement: Placement,
    /// Placed by the user (saved). Otherwise it sits in its device's
    /// rectangle, where the header's LEDs were, and follows the device.
    pub pinned: bool,
}

/// An item of the canvas: a device, or one of its fans.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Target {
    /// Index in [`Scene::devices`].
    pub device: usize,
    /// Index in [`SceneDevice::segments`]; `None`: the device itself.
    pub segment: Option<usize>,
}

impl Target {
    pub const fn device(device: usize) -> Self {
        Self {
            device,
            segment: None,
        }
    }

    pub const fn segment(device: usize, segment: usize) -> Self {
        Self {
            device,
            segment: Some(segment),
        }
    }
}

/// Layout key of one fan of a split header (stored next to the devices').
pub fn segment_key(device: &DeviceKey, zone: usize, part: usize) -> DeviceKey {
    DeviceKey(format!("{}#zone{}/fan{}", device.0, zone, part))
}

/// The layout key belongs to a fan of `device` (any zone, any part).
pub fn is_segment_of(key: &DeviceKey, device: &DeviceKey) -> bool {
    key.0
        .strip_prefix(device.0.as_str())
        .is_some_and(|rest| rest.starts_with("#zone"))
}

impl SceneDevice {
    /// Recomputes the fans that sit in the device and every LED position.
    pub fn refresh(&mut self) {
        let mut geometry = led_geometry(&self.info, &self.placement);
        if !self.segments.is_empty() {
            let bands = zone_bands(&self.info, &self.placement);
            for zone in self
                .segments
                .iter()
                .map(|s| s.zone)
                .collect::<BTreeSet<_>>()
            {
                let n = self.segments.iter().filter(|s| s.zone == zone).count();
                let Some(band) = bands.get(zone).copied().flatten() else {
                    continue;
                };
                let p = self.placement;
                for seg in self
                    .segments
                    .iter_mut()
                    .filter(|s| s.zone == zone && !s.pinned)
                {
                    // Centred on where its LEDs are in the header's band:
                    // LED order kept whatever the rotation / mirror.
                    let leds = &geometry
                        [seg.leds.start.min(geometry.len())..seg.leds.end.min(geometry.len())];
                    let k = leds.len().max(1) as f32;
                    let (cx, cy) = leds
                        .iter()
                        .fold((0.0, 0.0), |(x, y), g| (x + g.x / k, y + g.y / k));
                    seg.placement = docked(band, (cx, cy), n, p.rotation, p.mirror);
                }
            }
            let total = geometry.len();
            for seg in &self.segments {
                let g = part_geometry(seg.leds.len(), &seg.placement);
                let range = seg.leds.start.min(total)..seg.leds.end.min(total);
                for (slot, g) in geometry[range].iter_mut().zip(g) {
                    *slot = g;
                }
            }
        }
        self.leds = geometry.iter().map(|g| (g.x, g.y)).collect();
        self.geometry = geometry;
    }

    /// The fan that owns LED `led`, if it belongs to a split header.
    pub fn segment_of_led(&self, led: usize) -> Option<usize> {
        self.segments.iter().position(|s| s.leds.contains(&led))
    }
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
                    lighting: Default::default(),
                    segments: Vec::new(),
                }
            })
            .collect();
        Self { devices }
    }

    /// Per-zone / per-fan settings of every device (missing = the effect),
    /// with the saved places of the fans of split headers.
    pub fn set_lighting(
        &mut self,
        map: &crate::lighting::LightingMap,
        saved: &BTreeMap<DeviceKey, Placement>,
    ) {
        for d in &mut self.devices {
            d.lighting = map.get(&d.info.key).cloned().unwrap_or_default();
            let mut segments = Vec::new();
            for (zi, zone) in d.info.zones.iter().enumerate() {
                let ranges = crate::lighting::part_ranges(zone, d.lighting.get(&zi));
                if ranges.len() < 2 {
                    continue;
                }
                let start = crate::lighting::zone_start(&d.info, zi);
                for (part, r) in ranges.into_iter().enumerate() {
                    let key = segment_key(&d.info.key, zi, part);
                    let saved = saved.get(&key).map(|p| p.clamped());
                    segments.push(SceneSegment {
                        zone: zi,
                        part,
                        leds: (start + r.start)..(start + r.end),
                        key,
                        placement: saved.unwrap_or(d.placement),
                        pinned: saved.is_some(),
                    });
                }
            }
            d.segments = segments;
            d.refresh();
        }
    }

    /// Moves / resizes one device and recomputes its LEDs (its fans that
    /// were not moved on their own follow it).
    pub fn set_placement(&mut self, index: usize, placement: Placement) {
        if let Some(dev) = self.devices.get_mut(index) {
            dev.placement = placement.clamped();
            dev.refresh();
        }
    }

    /// Where an item of the canvas is.
    pub fn placement_of(&self, t: Target) -> Option<Placement> {
        let d = self.devices.get(t.device)?;
        match t.segment {
            None => Some(d.placement),
            Some(s) => d.segments.get(s).map(|s| s.placement),
        }
    }

    /// Layout key of an item (device key, or the fan's own key).
    pub fn key_of(&self, t: Target) -> Option<&DeviceKey> {
        let d = self.devices.get(t.device)?;
        match t.segment {
            None => Some(&d.info.key),
            Some(s) => d.segments.get(s).map(|s| &s.key),
        }
    }

    /// Moves / resizes / turns one item; a fan gets a place of its own.
    pub fn set_target_placement(&mut self, t: Target, placement: Placement) {
        match t.segment {
            None => self.set_placement(t.device, placement),
            Some(s) => {
                if let Some(d) = self.devices.get_mut(t.device)
                    && let Some(seg) = d.segments.get_mut(s)
                {
                    seg.placement = placement.clamped();
                    seg.pinned = true;
                    d.refresh();
                }
            }
        }
    }

    /// The item is hidden on the canvas (its device, or itself).
    pub fn is_hidden(&self, t: Target, hidden: &BTreeSet<DeviceKey>) -> bool {
        let Some(d) = self.devices.get(t.device) else {
            return true;
        };
        hidden.contains(&d.info.key)
            || t.segment
                .and_then(|s| d.segments.get(s))
                .is_some_and(|s| hidden.contains(&s.key))
    }

    /// Items in drawing order (bottom first): device bodies, then the fans
    /// of split headers, the selected item last (on top of what overlaps
    /// it). Hidden items are left out.
    pub fn draw_order(
        &self,
        hidden: &BTreeSet<DeviceKey>,
        selected: Option<Target>,
    ) -> Vec<Target> {
        let mut out: Vec<Target> = (0..self.devices.len()).map(Target::device).collect();
        for (di, d) in self.devices.iter().enumerate() {
            out.extend((0..d.segments.len()).map(|s| Target::segment(di, s)));
        }
        out.retain(|t| !self.is_hidden(*t, hidden));
        if let Some(sel) = selected
            && let Some(i) = out.iter().position(|t| *t == sel)
        {
            let t = out.remove(i);
            out.push(t);
        }
        out
    }

    /// Flattened LED list for the engine (only enabled, controllable
    /// devices; hidden ones too: hiding is for the canvas only).
    pub fn led_points(&self) -> Vec<LedPoint> {
        self.devices
            .iter()
            .enumerate()
            .filter(|(_, d)| d.enabled && d.info.controllable && d.info.hardware_mode.is_none())
            .flat_map(|(di, d)| {
                d.geometry.iter().enumerate().map(move |(li, g)| {
                    // "Per device" effects: each fan shows the whole
                    // pattern, like a device of its own.
                    let p = d
                        .segment_of_led(li)
                        .map_or(d.placement, |s| d.segments[s].placement);
                    // Scene orientation (flames rise up the screen whatever
                    // the item's angle): the turned item's box.
                    let (x0, y0, x1, y1) = p.bounds();
                    LedPoint {
                        device: di,
                        led: li,
                        x: g.x,
                        y: g.y,
                        u: ((g.x - x0) / (x1 - x0).max(1e-4)).clamp(0.0, 1.0),
                        v: ((g.y - y0) / (y1 - y0).max(1e-4)).clamp(0.0, 1.0),
                        rx: g.rx,
                        ry: g.ry,
                    }
                })
            })
            .collect()
    }

    pub fn total_leds(&self) -> usize {
        self.devices.iter().map(|d| d.info.led_count).sum()
    }

    /// Topmost visible item under a scene coordinate (the selected one
    /// first: it is drawn on top).
    pub fn hit_test(
        &self,
        x: f32,
        y: f32,
        hidden: &BTreeSet<DeviceKey>,
        selected: Option<Target>,
    ) -> Option<Target> {
        self.draw_order(hidden, selected)
            .into_iter()
            .rev()
            .find(|t| self.placement_of(*t).is_some_and(|p| p.contains(x, y)))
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
    let local = local_frame(p);
    transform(layout_unrotated(info, &local), &local, p)
}

/// The placement's rectangle before its rotation, at the origin: the
/// frame the LEDs are laid out in.
fn local_frame(p: &Placement) -> Placement {
    Placement::new(0.0, 0.0, p.w.max(1e-4), p.h.max(1e-4))
}

/// LEDs laid out in the local frame → scene space (mirror, then rotation
/// around the centre).
fn transform(raw: Vec<LedGeom>, local: &Placement, p: &Placement) -> Vec<LedGeom> {
    let (s, c) = p.sin_cos();
    let (s, c) = (s.abs(), c.abs());
    raw.into_iter()
        .map(|g| {
            let mut a = g.x / local.w;
            if p.mirror {
                a = 1.0 - a;
            }
            let (x, y) = p.point(a, g.y / local.h);
            // The area an LED covers turns with it: its box, in physical
            // proportions.
            let rxp = g.rx * SCENE_ASPECT;
            let ryp = g.ry;
            LedGeom {
                x: x.clamp(0.0, 1.0),
                y: y.clamp(0.0, 1.0),
                rx: ((c * rxp + s * ryp) / SCENE_ASPECT).max(0.004),
                ry: (s * rxp + c * ryp).max(0.006),
            }
        })
        .collect()
}

/// The band of the device rectangle each zone's LEDs use, as a rectangle
/// turned like the device (`None` for zones without LEDs), as laid out by
/// [`led_geometry`].
fn zone_bands(info: &DeviceInfo, p: &Placement) -> Vec<Option<Placement>> {
    let local = local_frame(p);
    let lit: Vec<usize> = (0..info.zones.len())
        .filter(|&i| info.zones[i].led_count > 0)
        .collect();
    let n = lit.len().max(1) as f32;
    let vertical = local.h > local.w * SCENE_ASPECT * 1.15;
    let mut out = vec![None; info.zones.len()];
    for (bi, &zi) in lit.iter().enumerate() {
        let (a0, b0, a1, b1) = if vertical {
            (bi as f32 / n, 0.0, (bi + 1) as f32 / n, 1.0)
        } else {
            (0.0, bi as f32 / n, 1.0, (bi + 1) as f32 / n)
        };
        let (mut am, bm) = ((a0 + a1) * 0.5, (b0 + b1) * 0.5);
        if p.mirror {
            am = 1.0 - am;
        }
        let (cx, cy) = p.point(am, bm);
        let (w, h) = ((a1 - a0) * p.w, (b1 - b0) * p.h);
        out[zi] = Some(Placement {
            rotation: p.rotation,
            mirror: p.mirror,
            ..Placement::new(cx - w * 0.5, cy - h * 0.5, w, h)
        });
    }
    out
}

/// Place of a fan sitting in its header's band (`n` fans): a square cell
/// centred on its LEDs, turned like its device.
fn docked(
    band: Placement,
    (cx, cy): (f32, f32),
    n: usize,
    rotation: f32,
    mirror: bool,
) -> Placement {
    let n_f = n.max(1) as f32;
    // Screen proportions: x is SCENE_ASPECT times longer than y.
    let side = if band.w * SCENE_ASPECT >= band.h {
        (band.w / n_f * SCENE_ASPECT).min(band.h)
    } else {
        (band.w * SCENE_ASPECT).min(band.h / n_f)
    } * 0.92;
    let (w, h) = ((side / SCENE_ASPECT).max(0.01), side.max(0.01));
    Placement {
        rotation,
        mirror,
        ..Placement::new(cx - w * 0.5, cy - h * 0.5, w, h)
    }
    .clamped()
}

/// LEDs of one fan in its own rectangle: a ring (first LED at the top,
/// clockwise, before the rotation / mirror), or a line when the rectangle
/// has been stretched into a strip.
pub fn part_geometry(n: usize, p: &Placement) -> Vec<LedGeom> {
    let local = local_frame(p);
    let screen_ratio = local.w * SCENE_ASPECT / local.h.max(1e-4);
    let mut raw = Vec::with_capacity(n);
    if (0.4..=2.5).contains(&screen_ratio) && n >= 3 {
        ring(&mut raw, n, local.x, local.y, local.w, local.h);
    } else {
        line(&mut raw, n, local.x, local.y, local.w, local.h);
    }
    transform(raw, &local, p)
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
        let r = p.with_rotation(90.0);
        assert_eq!(r.rotation, 90.0);
        let (x0, y0, x1, y1) = r.bounds();
        assert!(y1 - y0 > x1 - x0, "{r:?}");
        let g = first(&r);
        assert!(g.y < y0 + (y1 - y0) * 0.2, "{g:?} {r:?}");
        // Footprints follow the orientation: a vertical strip LED is
        // short along the strip.
        assert!(g.ry < (y1 - y0) * 0.5);
        // Any angle: every LED stays on the turned strip.
        for deg in [0.0, 30.0, 45.0, 90.0, 135.0, 180.0, 222.5, 270.0, 359.0] {
            let q = p.with_rotation(deg);
            for g in led_geometry(strip, &q) {
                assert!(q.contains(g.x, g.y), "{deg}: {g:?} {q:?}");
            }
        }
        // 45°: the first LED is up and to the left of the centre.
        let q = p.with_rotation(45.0);
        let (cx, cy) = q.center();
        let g = first(&q);
        assert!(g.x < cx && g.y < cy, "{g:?} {q:?}");
    }

    #[test]
    fn turning_keeps_the_shape_and_the_centre() {
        let p = Placement::new(0.3, 0.3, 0.2, 0.1);
        for deg in [10.0f32, 33.0, 90.0, 181.0, 300.0] {
            let q = p.with_rotation(deg);
            assert_eq!((q.w, q.h), (p.w, p.h));
            let (a, b) = (q.center(), p.center());
            assert!((a.0 - b.0).abs() < 1e-5 && (a.1 - b.1).abs() < 1e-5);
            // point() and local() are inverse.
            for (u, v) in [(0.0, 0.0), (1.0, 0.0), (0.25, 0.75), (1.0, 1.0)] {
                let (x, y) = q.point(u, v);
                let (u2, v2) = q.local(x, y);
                assert!(
                    (u - u2).abs() < 1e-4 && (v - v2).abs() < 1e-4,
                    "{deg} {u} {v}"
                );
            }
            // The four corners are inside the box, on its edges.
            let (x0, y0, x1, y1) = q.bounds();
            for (u, v) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                let (x, y) = q.point(u, v);
                assert!(x >= x0 - 1e-5 && x <= x1 + 1e-5 && y >= y0 - 1e-5 && y <= y1 + 1e-5);
            }
        }
        // The right angles are magnetic, the others are kept.
        assert_eq!(snap_angle(87.0), 90.0);
        assert_eq!(snap_angle(357.5), 0.0);
        assert_eq!(snap_angle(45.0), 45.0);
        assert_eq!(snap_angle(185.0), 185.0);
        // 360° is 0°, negative angles wrap.
        assert_eq!(p.with_rotation(360.0).rotation, 0.0);
        assert_eq!(p.with_rotation(-90.0).rotation, 270.0);
    }

    #[test]
    fn the_corner_handle_and_the_knob_work_at_any_angle() {
        for deg in [0.0f32, 30.0, 90.0, 200.0] {
            let p = Placement::new(0.3, 0.3, 0.1, 0.2).with_rotation(deg);
            let corner = p.point(0.0, 0.0);
            // Dragging the handle to where it already is changes nothing.
            let (hx, hy) = p.point(1.0, 1.0);
            let same = p.resized_to(hx, hy);
            assert!(
                (same.w - p.w).abs() < 1e-4 && (same.h - p.h).abs() < 1e-4,
                "{deg}"
            );
            // Bigger: the opposite corner did not move.
            let grown = p.resized_to(hx + 0.02, hy + 0.02);
            let c2 = grown.point(0.0, 0.0);
            assert!(
                (c2.0 - corner.0).abs() < 1e-4 && (c2.1 - corner.1).abs() < 1e-4,
                "{deg}"
            );
            // The knob: pointing straight above the centre is the angle 0,
            // to the right 90°.
            let (cx, cy) = p.center();
            assert!(
                p.angle_towards(cx, cy - 0.1)
                    .min(360.0 - p.angle_towards(cx, cy - 0.1))
                    < 0.01
            );
            assert!((p.angle_towards(cx + 0.1, cy) - 90.0).abs() < 0.01);
        }
    }

    #[test]
    fn layouts_saved_by_older_versions_come_back_turned() {
        // TuxRGB 3.0 saved a quarter turn as the turned rectangle.
        let old: Placement =
            serde_json::from_str(r#"{"x":0.1,"y":0.2,"w":0.0225,"h":0.32,"rotation":90}"#).unwrap();
        assert_eq!(old.rotation, 90.0);
        // Back to the strip before the turn: wide and flat, same centre,
        // and the same box once turned.
        assert!(old.w * SCENE_ASPECT > old.h);
        let (x0, y0, x1, y1) = old.bounds();
        assert!(
            (x0 - 0.1).abs() < 1e-4 && (y0 - 0.2).abs() < 1e-4,
            "{old:?}"
        );
        assert!(
            (x1 - 0.1225).abs() < 1e-4 && (y1 - 0.52).abs() < 1e-4,
            "{old:?}"
        );
        // Half turn and no turn: same rectangle.
        let half: Placement =
            serde_json::from_str(r#"{"x":0.1,"y":0.2,"w":0.3,"h":0.1,"rotation":180}"#).unwrap();
        assert_eq!((half.w, half.h, half.rotation), (0.3, 0.1, 180.0));
        // 3.1 saves the angle and reads it back as is.
        let q = Placement::new(0.2, 0.2, 0.2, 0.1).with_rotation(37.5);
        let text = serde_json::to_string(&q).unwrap();
        assert!(
            text.contains("\"angle\":37.5") && !text.contains("rotation"),
            "{text}"
        );
        let back: Placement = serde_json::from_str(&text).unwrap();
        assert_eq!(back, q);
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
        let none = BTreeSet::new();
        assert!(scene.hit_test(cx, cy, &none, None).is_some());
        // Hidden: no longer under the pointer, still driven.
        let hidden: BTreeSet<DeviceKey> =
            scene.devices.iter().map(|d| d.info.key.clone()).collect();
        assert!(scene.hit_test(cx, cy, &hidden, None).is_none());
        assert_eq!(scene.led_points().len(), scene.total_leds());
    }

    /// The demo board with its ARGB header (30 LEDs) split into 3 fans.
    fn split_board(saved: &BTreeMap<DeviceKey, Placement>) -> (Scene, usize) {
        let devices = demo_devices();
        let mut scene = Scene::build(&devices, saved, &Default::default());
        let board = scene
            .devices
            .iter()
            .position(|d| d.info.kind == DeviceKind::Motherboard)
            .unwrap();
        let key = scene.devices[board].info.key.clone();
        let zone = scene.devices[board]
            .info
            .zones
            .iter()
            .position(|z| z.resizable())
            .unwrap();
        let mut map = crate::lighting::LightingMap::new();
        map.entry(key).or_default().insert(
            zone,
            crate::lighting::ZoneLighting {
                parts: 3,
                ..Default::default()
            },
        );
        scene.set_lighting(&map, saved);
        (scene, board)
    }

    #[test]
    fn split_header_fans_are_canvas_items() {
        let (mut scene, board) = split_board(&BTreeMap::new());
        let d = &scene.devices[board];
        assert_eq!(d.segments.len(), 3);
        assert_eq!(d.segments[0].leds, 13..23);
        assert_eq!(d.segments[2].leds, 33..43);
        // Docked: inside the board, side by side, each fan a ring of its
        // own LEDs inside its square.
        for (i, s) in d.segments.iter().enumerate() {
            assert!(!s.pinned);
            let (cx, cy) = s.placement.center();
            assert!(d.placement.contains(cx, cy), "fan {i} outside the board");
            for li in s.leds.clone() {
                let g = d.geometry[li];
                assert!(s.placement.contains(g.x, g.y) || (g.x - s.placement.x).abs() < 1e-3);
            }
        }
        assert!(d.segments[0].placement.x < d.segments[1].placement.x);
        // The board moves: docked fans follow.
        let before = d.segments[1].placement;
        let mut p = d.placement;
        p.x -= 0.1;
        scene.set_placement(board, p);
        let after = scene.devices[board].segments[1].placement;
        assert!(
            (before.x - after.x - 0.1).abs() < 1e-3,
            "{before:?} {after:?}"
        );
        // A fan moved on its own stays where it was put.
        let t = Target::segment(board, 2);
        let mine = Placement::new(0.05, 0.05, 0.06, 0.1);
        scene.set_target_placement(t, mine);
        assert!(scene.devices[board].segments[2].pinned);
        let mut p = scene.devices[board].placement;
        p.y += 0.05;
        scene.set_placement(board, p);
        assert_eq!(scene.placement_of(t), Some(mine.clamped()));
        // Its LEDs moved with it, the effect sees them there.
        let points = scene.led_points();
        let fan_led = points
            .iter()
            .find(|l| l.device == board && l.led == 35)
            .unwrap();
        assert!(mine.contains(fan_led.x, fan_led.y));
        // Hit test: the fan is found on top of whatever is under it.
        let (cx, cy) = mine.center();
        assert_eq!(scene.hit_test(cx, cy, &BTreeSet::new(), None), Some(t));
    }

    #[test]
    fn docked_fans_follow_the_led_order_in_every_orientation() {
        let (mut scene, board) = split_board(&BTreeMap::new());
        let base = scene.devices[board].placement;
        for mirror in [false, true] {
            for turns in [0.0, 1.0, 2.0, 3.0, 0.5, 1.33] {
                let p = Placement { mirror, ..base }.with_rotation(turns * 90.0);
                scene.set_placement(board, p);
                let d = &scene.devices[board];
                // Each fan sits where the header's band put its LEDs: in
                // the device, apart from the others, first to last along
                // the header (the body layout without the split).
                let body = led_geometry(&d.info, &d.placement);
                for s in &d.segments {
                    let (cx, cy) = s.placement.center();
                    assert!(
                        d.placement.contains(cx, cy),
                        "{mirror} {turns}: fan {} outside",
                        s.part
                    );
                    let mid = body[(s.leds.start + s.leds.end) / 2];
                    assert!(
                        s.placement.contains(mid.x, mid.y)
                            || ((mid.x - cx).abs() < s.placement.w
                                && (mid.y - cy).abs() < s.placement.h),
                        "{mirror} {turns}: fan {} not on its LEDs",
                        s.part
                    );
                }
                let c: Vec<(f32, f32)> = d.segments.iter().map(|s| s.placement.center()).collect();
                for i in 0..c.len() {
                    for j in i + 1..c.len() {
                        assert!((c[i].0 - c[j].0).abs() + (c[i].1 - c[j].1).abs() > 1e-3);
                    }
                }
            }
        }
    }

    #[test]
    fn saved_fan_places_come_back() {
        let (scene, board) = split_board(&BTreeMap::new());
        let key = scene.devices[board].segments[1].key.clone();
        assert!(is_segment_of(&key, &scene.devices[board].info.key));
        let mut saved = BTreeMap::new();
        let place = Placement::new(0.3, 0.6, 0.06, 0.1).with_rotation(90.0);
        saved.insert(key, place);
        let (scene, board) = split_board(&saved);
        let s = &scene.devices[board].segments[1];
        assert!(s.pinned);
        assert_eq!(s.placement, place.clamped());
        // Turned a quarter: its first LED is on the right of the ring.
        let first = scene.devices[board].geometry[s.leds.start];
        let (cx, _) = s.placement.center();
        assert!(first.x > cx, "{first:?} {:?}", s.placement);
    }

    #[test]
    fn selected_item_is_drawn_and_hit_on_top() {
        let devices = demo_devices();
        let mut saved = BTreeMap::new();
        // Two devices on top of each other.
        saved.insert(devices[0].key.clone(), Placement::new(0.2, 0.2, 0.3, 0.3));
        saved.insert(devices[1].key.clone(), Placement::new(0.25, 0.25, 0.3, 0.3));
        let scene = Scene::build(&devices, &saved, &Default::default());
        let none = BTreeSet::new();
        assert_eq!(
            scene.hit_test(0.3, 0.3, &none, None),
            Some(Target::device(1))
        );
        assert_eq!(
            scene.hit_test(0.3, 0.3, &none, Some(Target::device(0))),
            Some(Target::device(0))
        );
        assert_eq!(
            scene.draw_order(&none, Some(Target::device(0))).last(),
            Some(&Target::device(0))
        );
    }
}
