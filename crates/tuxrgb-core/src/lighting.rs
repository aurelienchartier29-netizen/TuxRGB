//! Per-zone and per-fan lighting (TuxRGB 3.0).
//!
//! By default every LED follows the active effect. Each zone of a device
//! (a motherboard header, a cooler's ring or logo, a strip) can instead be
//! given a fixed colour or be turned off. An addressable header that carries
//! several daisy-chained fans is split into parts – one per fan, like the
//! segments of OpenRGB or the "per fan per header" components of SignalRGB –
//! each with its own setting and its own place on the canvas (see
//! [`crate::layout`]). The fans of a header have the same number of LEDs by
//! default; each one can be given its own count (an 8-LED fan chained with
//! a 16-LED one). Segments defined in OpenRGB itself (external server) are
//! used as the split until TuxRGB is told otherwise.
//!
//! The settings are stored per device key (stable across sessions) and zone
//! index, and applied by the engine after the effect, before the global
//! saturation and brightness: a fixed colour dims with the brightness
//! slider like everything else, and every change crossfades.

use crate::color::Rgb;
use crate::device::{DeviceInfo, DeviceKey, ZoneInfo};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ops::Range;

/// Most fans (parts) a header can be split into.
pub const MAX_PARTS: usize = 12;

/// What one part of a zone shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum PartLighting {
    /// The active effect (default).
    #[default]
    Effect,
    /// A fixed colour.
    Color { color: Rgb },
    /// Dark.
    Off,
}

impl PartLighting {
    pub fn is_effect(self) -> bool {
        matches!(self, Self::Effect)
    }
}

/// Settings of one zone: how it is split into parts (fans), and what each
/// part shows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoneLighting {
    /// Parts (fans on a header): `0` = as OpenRGB defines the zone (its
    /// segments, or the whole zone), `1` = the whole zone.
    pub parts: usize,
    /// One entry per part; missing entries follow the effect.
    pub lighting: Vec<PartLighting>,
    /// LED count of each part when they differ; empty = equal parts.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sizes: Vec<usize>,
}

impl ZoneLighting {
    /// Setting of a part.
    pub fn part(&self, part: usize) -> PartLighting {
        self.lighting.get(part).copied().unwrap_or_default()
    }

    /// Changes one part (the list grows as needed).
    pub fn set_part(&mut self, part: usize, value: PartLighting) {
        if self.lighting.len() <= part {
            self.lighting.resize(part + 1, PartLighting::Effect);
        }
        self.lighting[part] = value;
        self.trim();
    }

    /// Nothing differs from the default.
    pub fn is_default(&self) -> bool {
        self.parts == 0 && self.sizes.is_empty() && self.lighting.iter().all(|p| p.is_effect())
    }

    fn trim(&mut self) {
        while self.lighting.last().is_some_and(|p| p.is_effect()) {
            self.lighting.pop();
        }
    }
}

/// `n` equal shares of `leds` (the rounding goes to the last ones).
fn equal_sizes(leds: usize, n: usize) -> Vec<usize> {
    (0..n).map(|p| (p + 1) * leds / n - p * leds / n).collect()
}

/// LED count of each part of a zone (they add up to the zone): TuxRGB's
/// split, else the zone's OpenRGB segments, else the whole zone. A zone
/// without LEDs (ARGB header not set up) has one empty part.
pub fn part_sizes(zone: &ZoneInfo, z: Option<&ZoneLighting>) -> Vec<usize> {
    let leds = zone.led_count;
    if leds == 0 {
        return vec![0];
    }
    let wanted = z.map_or(0, |z| z.parts);
    if wanted == 0 {
        // OpenRGB's segments, when they describe the whole zone.
        let segments = &zone.segments;
        if segments.len() >= 2
            && segments.len() <= MAX_PARTS
            && segments.iter().all(|s| *s > 0)
            && segments.iter().sum::<usize>() == leds
        {
            return segments.clone();
        }
        return vec![leds];
    }
    let n = wanted.clamp(1, MAX_PARTS).min(leds);
    if n == 1 {
        return vec![leds];
    }
    if let Some(sizes) = z.map(|z| &z.sizes)
        && sizes.len() == n
        && sizes.iter().all(|s| *s > 0)
    {
        let head: usize = sizes[..n - 1].iter().sum();
        if head < leds {
            // The last fan takes what is left (the header was resized).
            let mut out = sizes.clone();
            out[n - 1] = leds - head;
            return out;
        }
    }
    equal_sizes(leds, n)
}

/// LED ranges of the parts of a zone, inside the zone (`0..led_count`).
pub fn part_ranges(zone: &ZoneInfo, z: Option<&ZoneLighting>) -> Vec<Range<usize>> {
    let mut start = 0;
    part_sizes(zone, z)
        .into_iter()
        .map(|n| {
            let r = start..start + n;
            start += n;
            r
        })
        .collect()
}

/// Number of parts of a zone (`1`: not split).
pub fn parts_of(zone: &ZoneInfo, z: Option<&ZoneLighting>) -> usize {
    part_sizes(zone, z).len()
}

/// New sizes after giving `part` `leds` LEDs: the next fan (the previous
/// one for the last fan) takes the difference, every fan keeps one LED at
/// least. `None` when it cannot change.
pub fn resize_part(sizes: &[usize], part: usize, leds: usize) -> Option<Vec<usize>> {
    let n = sizes.len();
    if n < 2 || part >= n {
        return None;
    }
    let other = if part + 1 < n { part + 1 } else { part - 1 };
    let total = sizes[part] + sizes[other];
    let leds = leds.clamp(1, total - 1);
    if leds == sizes[part] {
        return None;
    }
    let mut out = sizes.to_vec();
    out[part] = leds;
    out[other] = total - leds;
    Some(out)
}

/// Settings of every zone of one device (zone index → settings).
pub type DeviceLighting = BTreeMap<usize, ZoneLighting>;

/// Settings of every device.
pub type LightingMap = BTreeMap<DeviceKey, DeviceLighting>;

/// Per LED of `info` (OpenRGB order: zones one after the other), what it
/// shows. Empty when everything follows the effect.
pub fn led_lighting(info: &DeviceInfo, lighting: Option<&DeviceLighting>) -> Vec<PartLighting> {
    let Some(lighting) = lighting.filter(|l| {
        l.values()
            .any(|z| z.lighting.iter().any(|p| !p.is_effect()))
    }) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(info.led_count);
    for (zi, zone) in info.zones.iter().enumerate() {
        match lighting.get(&zi) {
            Some(z) if z.lighting.iter().any(|p| !p.is_effect()) => {
                for (part, n) in part_sizes(zone, Some(z)).into_iter().enumerate() {
                    out.extend(std::iter::repeat_n(z.part(part), n));
                }
            }
            _ => out.extend(std::iter::repeat_n(PartLighting::Effect, zone.led_count)),
        }
    }
    // Zones that do not add up to the device (never seen, but harmless).
    out.resize(info.led_count, PartLighting::Effect);
    out
}

/// First LED of each zone, in device LED indices.
pub fn zone_start(info: &DeviceInfo, zone: usize) -> usize {
    info.zones.iter().take(zone).map(|z| z.led_count).sum()
}

/// LEDs of one part of one zone, in device LED indices.
pub fn part_leds(
    info: &DeviceInfo,
    lighting: Option<&DeviceLighting>,
    zone: usize,
    part: usize,
) -> Range<usize> {
    let start = zone_start(info, zone);
    let Some(z) = info.zones.get(zone) else {
        return start..start;
    };
    let ranges = part_ranges(z, lighting.and_then(|l| l.get(&zone)));
    let r = ranges
        .get(part)
        .cloned()
        .unwrap_or_else(|| ranges.last().cloned().unwrap_or(0..0));
    (start + r.start)..(start + r.end)
}

/// Number of zones / parts of a device that do not follow the effect.
pub fn custom_parts(info: &DeviceInfo, lighting: Option<&DeviceLighting>) -> usize {
    let Some(lighting) = lighting else { return 0 };
    info.zones
        .iter()
        .enumerate()
        .map(|(zi, zone)| match lighting.get(&zi) {
            Some(z) => (0..parts_of(zone, Some(z)))
                .filter(|p| !z.part(*p).is_effect())
                .count(),
            None => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{DeviceKind, ZoneKind};

    fn zone(name: &str, n: usize, max: usize) -> ZoneInfo {
        ZoneInfo {
            name: name.into(),
            kind: ZoneKind::Linear,
            led_count: n,
            matrix: None,
            leds_min: 0,
            leds_max: max,
            segments: Vec::new(),
        }
    }

    fn device() -> DeviceInfo {
        let zones = vec![
            zone("VRM", 4, 0),
            zone("ARGB 1", 24, 120),
            zone("Logo", 1, 0),
        ];
        DeviceInfo {
            id: 1,
            key: DeviceKey("mb".into()),
            name: "Board".into(),
            vendor: String::new(),
            description: String::new(),
            location: String::new(),
            serial: String::new(),
            kind: DeviceKind::Motherboard,
            led_count: zones.iter().map(|z| z.led_count).sum(),
            zones,
            controllable: true,
            single_color: false,
            mode: "Direct".into(),
            is_virtual: false,
            hardware_modes: Vec::new(),
            hardware_mode: None,
        }
    }

    fn split(parts: usize) -> ZoneLighting {
        ZoneLighting {
            parts,
            ..Default::default()
        }
    }

    #[test]
    fn untouched_devices_cost_nothing() {
        assert!(led_lighting(&device(), None).is_empty());
        let mut l = DeviceLighting::new();
        l.insert(1, split(3));
        // Split but everything still follows the effect.
        assert!(!l[&1].is_default());
        assert!(led_lighting(&device(), Some(&l)).is_empty());
    }

    #[test]
    fn a_header_is_split_into_fans() {
        let red = PartLighting::Color {
            color: Rgb::new(255, 0, 0),
        };
        let mut z = split(3);
        z.set_part(1, red);
        z.set_part(2, PartLighting::Off);
        let mut l = DeviceLighting::new();
        l.insert(1, z);
        let leds = led_lighting(&device(), Some(&l));
        assert_eq!(leds.len(), 29);
        assert!(leds[..4].iter().all(|p| p.is_effect()), "VRM untouched");
        assert!(
            leds[4..12].iter().all(|p| p.is_effect()),
            "fan 1 follows the effect"
        );
        assert!(leds[12..20].iter().all(|p| *p == red), "fan 2 red");
        assert!(
            leds[20..28].iter().all(|p| *p == PartLighting::Off),
            "fan 3 off"
        );
        assert!(leds[28].is_effect(), "logo untouched");
        assert_eq!(part_leds(&device(), Some(&l), 1, 1), 12..20);
        assert_eq!(custom_parts(&device(), Some(&l)), 2);
    }

    #[test]
    fn parts_follow_the_led_count() {
        let z = split(3);
        // 10 LEDs in 3 parts: every LED belongs to exactly one part.
        let sizes = part_sizes(&zone("h", 10, 120), Some(&z));
        assert_eq!(sizes.iter().sum::<usize>(), 10);
        assert_eq!(sizes.len(), 3);
        // More parts than LEDs: one LED each.
        assert_eq!(part_sizes(&zone("h", 2, 120), Some(&split(8))), vec![1, 1]);
        // An empty header (ARGB not configured yet).
        assert_eq!(part_sizes(&zone("h", 0, 120), Some(&z)), vec![0]);
        assert_eq!(part_ranges(&zone("h", 0, 120), Some(&z)), vec![0..0]);
    }

    #[test]
    fn fans_can_have_their_own_led_count() {
        // An 8-LED fan chained with two 16-LED ones.
        let header = zone("ARGB", 40, 120);
        let mut z = split(3);
        z.sizes = vec![8, 16, 16];
        assert_eq!(part_ranges(&header, Some(&z)), vec![0..8, 8..24, 24..40]);
        // Header resized to 48: the last fan takes the difference.
        assert_eq!(
            part_sizes(&zone("ARGB", 48, 120), Some(&z)),
            vec![8, 16, 24]
        );
        // Too small for the first fans: back to equal parts.
        assert_eq!(part_sizes(&zone("ARGB", 12, 120), Some(&z)), vec![4, 4, 4]);
        // Resizing one fan moves LEDs to / from its neighbour.
        assert_eq!(resize_part(&[8, 8, 8], 0, 10), Some(vec![10, 6, 8]));
        assert_eq!(resize_part(&[8, 8, 8], 2, 6), Some(vec![8, 10, 6]));
        assert_eq!(resize_part(&[8, 8], 0, 99), Some(vec![15, 1]));
        assert_eq!(resize_part(&[8, 8], 0, 8), None);
        assert_eq!(resize_part(&[24], 0, 8), None);
    }

    #[test]
    fn openrgb_segments_are_the_default_split() {
        let mut header = zone("ARGB", 30, 120);
        header.segments = vec![10, 20];
        assert_eq!(part_sizes(&header, None), vec![10, 20]);
        assert_eq!(
            part_sizes(&header, Some(&ZoneLighting::default())),
            vec![10, 20]
        );
        // TuxRGB's own choice wins, the whole zone included.
        assert_eq!(part_sizes(&header, Some(&split(1))), vec![30]);
        assert_eq!(part_sizes(&header, Some(&split(3))), vec![10, 10, 10]);
        // Segments that do not cover the zone are ignored.
        header.segments = vec![10, 10];
        assert_eq!(part_sizes(&header, None), vec![30]);
    }

    #[test]
    fn settings_survive_a_save() {
        let mut l = LightingMap::new();
        let mut z = ZoneLighting::default();
        z.set_part(
            0,
            PartLighting::Color {
                color: Rgb::new(0, 120, 212),
            },
        );
        l.entry(DeviceKey("mb".into())).or_default().insert(2, z);
        let mut fans = split(2);
        fans.sizes = vec![8, 16];
        l.entry(DeviceKey("mb".into())).or_default().insert(1, fans);
        let json = serde_json::to_string(&l).unwrap();
        assert!(json.contains("#0078d4"), "{json}");
        let back: LightingMap = serde_json::from_str(&json).unwrap();
        assert_eq!(back, l);
        // Resetting a part to the effect leaves nothing behind.
        let mut z = back[&DeviceKey("mb".into())][&2].clone();
        z.set_part(0, PartLighting::Effect);
        assert!(z.is_default());
    }
}
