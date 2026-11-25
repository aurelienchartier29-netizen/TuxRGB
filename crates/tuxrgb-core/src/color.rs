//! sRGB colour type used everywhere in TuxRGB.

use serde::{Deserialize, Serialize};
use std::fmt;

/// An 8-bit sRGB colour.
///
/// Serialised as a `#rrggbb` string so configuration files stay readable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb::new(0, 0, 0);
    pub const WHITE: Rgb = Rgb::new(255, 255, 255);

    #[inline]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parses `#rgb`, `#rrggbb` or `rrggbb`.
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        let parse = |i: usize, len: usize| u8::from_str_radix(&s[i..i + len], 16).ok();
        match s.len() {
            6 => Some(Self::new(parse(0, 2)?, parse(2, 2)?, parse(4, 2)?)),
            3 => {
                let r = parse(0, 1)?;
                let g = parse(1, 1)?;
                let b = parse(2, 1)?;
                Some(Self::new(r * 17, g * 17, b * 17))
            }
            _ => None,
        }
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Converts HSV (all components in `0.0..=1.0`, hue wraps) to RGB.
    pub fn from_hsv(h: f32, s: f32, v: f32) -> Self {
        let h = h.rem_euclid(1.0) * 6.0;
        let s = s.clamp(0.0, 1.0);
        let v = v.clamp(0.0, 1.0);
        let i = h.floor();
        let f = h - i;
        let p = v * (1.0 - s);
        let q = v * (1.0 - s * f);
        let t = v * (1.0 - s * (1.0 - f));
        let (r, g, b) = match i as i32 {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        Self::from_f32(r, g, b)
    }

    /// Returns `(h, s, v)` with every component in `0.0..=1.0`.
    pub fn to_hsv(self) -> (f32, f32, f32) {
        let [r, g, b] = self.to_f32();
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let h = if d <= f32::EPSILON {
            0.0
        } else if max == r {
            ((g - b) / d).rem_euclid(6.0) / 6.0
        } else if max == g {
            ((b - r) / d + 2.0) / 6.0
        } else {
            ((r - g) / d + 4.0) / 6.0
        };
        let s = if max <= f32::EPSILON { 0.0 } else { d / max };
        (h, s, max)
    }

    #[inline]
    pub fn from_f32(r: f32, g: f32, b: f32) -> Self {
        #[inline]
        fn c(x: f32) -> u8 {
            (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
        }
        Self::new(c(r), c(g), c(b))
    }

    #[inline]
    pub fn to_f32(self) -> [f32; 3] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
        ]
    }

    /// Multiplies every channel by `factor` (clamped to `0..=1`).
    #[inline]
    pub fn scale(self, factor: f32) -> Self {
        let f = factor.clamp(0.0, 1.0);
        Self::new(
            (self.r as f32 * f + 0.5) as u8,
            (self.g as f32 * f + 0.5) as u8,
            (self.b as f32 * f + 0.5) as u8,
        )
    }

    /// Software dimming towards black: the HSV *value* is multiplied by
    /// `level` (`0..=1`) while hue and saturation are kept. In RGB this is
    /// exactly a uniform scale of the three channels relative to the
    /// brightest one, done in floating point and rounded once.
    #[inline]
    pub fn dim(self, level: f32) -> Self {
        let level = level.clamp(0.0, 1.0);
        let v = self.r.max(self.g).max(self.b);
        if v == 0 || level >= 1.0 {
            return if level <= 0.0 { Self::BLACK } else { self };
        }
        let new_v = v as f32 * level; // HSV value, 0..=255
        let k = new_v / v as f32;
        Self::new(
            (self.r as f32 * k).round() as u8,
            (self.g as f32 * k).round() as u8,
            (self.b as f32 * k).round() as u8,
        )
    }

    /// Linear interpolation between two colours.
    #[inline]
    pub fn lerp(a: Self, b: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t + 0.5) as u8;
        Self::new(l(a.r, b.r), l(a.g, b.g), l(a.b, b.b))
    }

    /// Additive blend with saturation.
    #[inline]
    pub fn saturating_add(self, other: Self) -> Self {
        Self::new(
            self.r.saturating_add(other.r),
            self.g.saturating_add(other.g),
            self.b.saturating_add(other.b),
        )
    }

    /// Global saturation of a colour (`amount`: `1.0` leaves it as it is,
    /// `0.0` gives white / grey of the same value, `2.0` = 200 %).
    ///
    /// Computed in Okhsv (see [`crate::oklab`]): the perceptual hue and the
    /// value never move, nothing is clipped (two different colours stay
    /// different, gradients stay gradients), and colours that are already
    /// pure – the most saturated the LEDs can show for that hue – stay as
    /// they are above 100 %. Greys, white and black never get tinted.
    pub fn saturate(self, amount: f32) -> Self {
        if (amount - 1.0).abs() < 1e-3 || (self.r == self.g && self.g == self.b) {
            return self;
        }
        let [r, g, b] = crate::oklab::saturate(self.r, self.g, self.b, amount);
        Self::new(r, g, b)
    }

    /// Floating-point interpolation, rounded once (used by crossfades).
    #[inline]
    pub fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    }

    /// Rotates the hue by `turns` (1.0 = 360°).
    pub fn rotate_hue(self, turns: f32) -> Self {
        let (h, s, v) = self.to_hsv();
        Self::from_hsv(h + turns, s, v)
    }

    /// Relative luminance (WCAG), `0.0..=1.0`.
    pub fn luminance(self) -> f32 {
        fn lin(c: f32) -> f32 {
            if c <= 0.040_45 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        let [r, g, b] = self.to_f32();
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl From<Rgb> for String {
    fn from(value: Rgb) -> Self {
        value.to_hex()
    }
}

impl TryFrom<String> for Rgb {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Rgb::from_hex(&value).ok_or_else(|| format!("invalid colour `{value}`"))
    }
}

/// How the secondary and tertiary colours of the [`Palette`] relate to the
/// desktop accent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Harmony {
    /// Neighbouring hue (±40°): calm, tone-on-tone.
    Analogous,
    /// Opposite hue (180°): maximum contrast.
    Complementary,
    /// Hue at 120°: lively, balanced.
    Triadic,
}

/// "Palette Système Harmonieuse": three coordinated colours derived from the
/// desktop accent, used by multi-colour effects.
///
/// * `primary` – the accent colour exactly as the desktop reports it;
/// * `secondary` – the same hue with the saturation shifted (a vivid accent
///   gives a softer, lighter variant; a muted accent gives a richer one), so
///   it is clearly distinct on LEDs while staying in the same family;
/// * `tertiary` – the harmonised hue (analogous, complementary or triadic);
/// * `background` – a very dark tint of the accent for "night" backgrounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub primary: Rgb,
    pub secondary: Rgb,
    pub tertiary: Rgb,
    pub background: Rgb,
}

impl Palette {
    pub fn from_accent(accent: Rgb, harmony: Harmony) -> Self {
        let (h, s, v) = accent.to_hsv();
        let secondary = if s > 0.55 {
            Rgb::from_hsv(h + 0.02, s * 0.55, v.max(0.85))
        } else {
            Rgb::from_hsv(h - 0.02, (s + 0.4).min(1.0), v.max(0.8))
        };
        let shift = match harmony {
            Harmony::Analogous => 0.11,
            Harmony::Complementary => 0.5,
            Harmony::Triadic => 1.0 / 3.0,
        };
        let tertiary = Rgb::from_hsv(h + shift, s.max(0.7), v.max(0.85));
        let background = Rgb::from_hsv(h, s.max(0.6), 0.09);
        Self {
            primary: accent,
            secondary,
            tertiary,
            background,
        }
    }

    pub fn colors(&self) -> [Rgb; 3] {
        [self.primary, self.secondary, self.tertiary]
    }

    /// Cyclic, smooth sampling: `0` = primary, `1/3` = secondary,
    /// `2/3` = tertiary, then back to primary.
    pub fn cyclic(&self, t: f32) -> Rgb {
        let c = self.colors();
        let pos = t.rem_euclid(1.0) * 3.0;
        let i = (pos.floor() as usize).min(2);
        let f = pos - i as f32;
        let f = f * f * (3.0 - 2.0 * f);
        Rgb::lerp(c[i], c[(i + 1) % 3], f)
    }

    /// Open gradient: `0` = primary → `0.5` = secondary → `1` = tertiary.
    pub fn gradient(&self, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        if t < 0.5 {
            Rgb::lerp(self.primary, self.secondary, t * 2.0)
        } else {
            Rgb::lerp(self.secondary, self.tertiary, t * 2.0 - 1.0)
        }
    }
}

/// Memo of the global saturation for one slider value: a frame repeats a
/// handful of colours (a static colour, a palette, a gradient), so each
/// distinct colour is converted once instead of once per LED. Direct-mapped,
/// 4096 slots (32 KiB), emptied when the slider moves.
pub struct SaturationCache {
    amount: f32,
    slots: Box<[(u32, [u8; 3])]>,
}

impl Default for SaturationCache {
    fn default() -> Self {
        Self {
            amount: 1.0,
            slots: vec![(0, [0; 3]); 4096].into_boxed_slice(),
        }
    }
}

impl SaturationCache {
    /// `c.saturate(amount)`, remembered.
    #[inline]
    pub fn saturate(&mut self, c: Rgb, amount: f32) -> Rgb {
        if amount.to_bits() != self.amount.to_bits() {
            self.slots.fill((0, [0; 3]));
            self.amount = amount;
        }
        let key = 1 << 24 | (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
        let slot = &mut self.slots[(key.wrapping_mul(0x9E37_79B1) >> 20) as usize];
        if slot.0 == key {
            return Rgb::new(slot.1[0], slot.1[1], slot.1[2]);
        }
        let out = c.saturate(amount);
        *slot = (key, [out.r, out.g, out.b]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturation_boost_keeps_white_and_greys() {
        for amount in [0.0, 0.5, 1.5, 2.0] {
            assert_eq!(Rgb::WHITE.saturate(amount), Rgb::WHITE);
            assert_eq!(Rgb::new(90, 90, 90).saturate(amount), Rgb::new(90, 90, 90));
            assert_eq!(Rgb::BLACK.saturate(amount), Rgb::BLACK);
        }
        // 0 %: white for bright colours, a grey of the same value otherwise.
        for c in [
            Rgb::new(255, 0, 0),
            Rgb::new(255, 128, 0),
            Rgb::new(128, 128, 255),
        ] {
            assert_eq!(c.saturate(0.0), Rgb::WHITE, "{c:?}");
        }
        let grey = Rgb::new(0, 120, 212).saturate(0.0);
        assert!(
            grey.r == grey.g && grey.g == grey.b && (190..=225).contains(&grey.r),
            "{grey:?}"
        );
        // 50 %: half-way, same hue.
        let half = Rgb::new(0, 120, 212).saturate(0.5);
        assert!(hue_drift(Rgb::new(0, 120, 212), half) < 1.0, "{half:?}");
        assert!(half.r > 60 && half.r < 160 && half.b >= 205, "{half:?}");
    }

    /// Hue difference in degrees, OKLCH.
    fn hue_drift(a: Rgb, b: Rgb) -> f32 {
        let (Some(h1), Some(h2)) = (
            crate::oklab::hue_of(a.r, a.g, a.b),
            crate::oklab::hue_of(b.r, b.g, b.b),
        ) else {
            return 0.0;
        };
        let d = (h1 - h2).abs().to_degrees() % 360.0;
        d.min(360.0 - d)
    }

    #[test]
    fn saturation_keeps_the_hue_and_the_value_never_clips() {
        // A grid of colours, through the whole slider range.
        let mut checked = 0;
        for r in (0..=255).step_by(17) {
            for g in (0..=255).step_by(17) {
                for b in (0..=255).step_by(17) {
                    let c = Rgb::new(r as u8, g as u8, b as u8);
                    let chroma = crate::oklab::chroma_of(c.r, c.g, c.b);
                    if chroma < 0.04 {
                        continue; // near greys: 8-bit rounding dominates the hue
                    }
                    let mut last = 0.0f32;
                    for amount in [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0] {
                        let out = c.saturate(amount);
                        let now = crate::oklab::chroma_of(out.r, out.g, out.b);
                        if now > 0.03 {
                            // 8-bit rounding moves a and b by ~0.002: the
                            // hue of a weak colour is only that precise.
                            let drift = hue_drift(c, out);
                            let tolerance = 1.0 + (0.003 / now).to_degrees();
                            assert!(
                                drift < tolerance,
                                "{c:?} ×{amount} → {out:?}: hue moved {drift:.2}° (> {tolerance:.2}°)"
                            );
                        }
                        // More saturation, never less colour (8-bit rounding aside).
                        assert!(
                            now + 0.006 >= last,
                            "{c:?} ×{amount} → {out:?}: chroma {now} < {last}"
                        );
                        last = now;
                        // The HSV value (LED drive) barely moves: no brightness jump.
                        let v0 = c.r.max(c.g).max(c.b) as i32;
                        let v1 = out.r.max(out.g).max(out.b) as i32;
                        assert!((v0 - v1).abs() <= 24, "{c:?} ×{amount} → {out:?}");
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked > 2000, "{checked}");
    }

    #[test]
    fn pure_colours_stay_pure_mixed_colours_get_vivid() {
        // Already the most saturated colour of their hue: unchanged above
        // 100 % (no "deepening" that turned amber into orange).
        for pure in [
            Rgb::new(255, 0, 0),
            Rgb::new(0, 255, 0),
            Rgb::new(0, 0, 255),
            Rgb::new(255, 255, 0),
            Rgb::new(0, 255, 255),
            Rgb::new(255, 0, 255),
            Rgb::new(255, 191, 0),
            Rgb::new(0, 120, 212),
        ] {
            assert_eq!(pure.saturate(2.0), pure, "{pure:?}");
            assert_eq!(pure.saturate(1.3), pure, "{pure:?}");
        }
        // Pastel and washed-out colours: clearly more vivid, same hue.
        for washed in [
            Rgb::new(255, 127, 127),
            Rgb::new(200, 150, 150),
            Rgb::new(142, 140, 216),
            Rgb::new(231, 72, 86),
            Rgb::new(120, 160, 120),
        ] {
            let c0 = crate::oklab::chroma_of(washed.r, washed.g, washed.b);
            let out = washed.saturate(2.0);
            let c1 = crate::oklab::chroma_of(out.r, out.g, out.b);
            assert!(c1 > c0 + 0.03, "{washed:?} → {out:?}: {c0} → {c1}");
            assert!(hue_drift(washed, out) < 1.5, "{washed:?} → {out:?}");
        }
    }

    #[test]
    fn saturation_cache_matches_the_direct_computation() {
        let mut cache = SaturationCache::default();
        for amount in [1.5f32, 0.5, 2.0] {
            for i in 0..20_000u32 {
                let x = i.wrapping_mul(2_654_435_761);
                let c = Rgb::new((x >> 8) as u8, (x >> 16) as u8, (x >> 24) as u8 & 0xF0);
                assert_eq!(cache.saturate(c, amount), c.saturate(amount));
                // Hits give the same answer.
                assert_eq!(cache.saturate(c, amount), c.saturate(amount));
            }
        }
    }

    #[test]
    fn boost_keeps_gradations() {
        // A pastel → pure gradient stays a gradient at 200 % (a hard clamp
        // at full saturation flattened most of it into one colour).
        let ramp: Vec<Rgb> = (0..=16)
            .map(|i| {
                let x = 200 - i * 12;
                Rgb::new(255, x as u8, x as u8)
            })
            .collect();
        let boosted: Vec<Rgb> = ramp.iter().map(|c| c.saturate(2.0)).collect();
        let distinct: std::collections::BTreeSet<(u8, u8, u8)> =
            boosted.iter().map(|c| (c.r, c.g, c.b)).collect();
        assert!(distinct.len() >= 13, "{boosted:?}");
        for w in boosted.windows(2) {
            assert!(w[1].g <= w[0].g, "{boosted:?}");
        }
    }

    #[test]
    fn palette_keeps_the_exact_accent_as_primary() {
        let accent = Rgb::new(61, 174, 233);
        for harmony in [Harmony::Analogous, Harmony::Complementary, Harmony::Triadic] {
            let p = Palette::from_accent(accent, harmony);
            assert_eq!(p.primary, accent);
            assert_ne!(p.secondary, accent);
            assert_ne!(p.tertiary, accent);
            assert!(p.background.r.max(p.background.g).max(p.background.b) < 40);
            assert_eq!(p.cyclic(0.0), accent);
            assert_eq!(p.gradient(0.0), accent);
            assert_eq!(p.gradient(1.0), p.tertiary);
        }
        let (h, _, _) = accent.to_hsv();
        let (ht, _, _) = Palette::from_accent(accent, Harmony::Complementary)
            .tertiary
            .to_hsv();
        assert!(((ht - h).rem_euclid(1.0) - 0.5).abs() < 0.02);
    }

    #[test]
    fn hex_roundtrip() {
        let c = Rgb::from_hex("#12abEF").unwrap();
        assert_eq!(c, Rgb::new(0x12, 0xab, 0xef));
        assert_eq!(c.to_hex(), "#12abef");
        assert_eq!(Rgb::from_hex("#fff"), Some(Rgb::WHITE));
        assert_eq!(Rgb::from_hex("nope"), None);
    }

    #[test]
    fn hsv_roundtrip() {
        for c in [
            Rgb::new(255, 0, 0),
            Rgb::new(10, 200, 90),
            Rgb::new(30, 30, 200),
        ] {
            let (h, s, v) = c.to_hsv();
            let back = Rgb::from_hsv(h, s, v);
            assert!((back.r as i32 - c.r as i32).abs() <= 1);
            assert!((back.g as i32 - c.g as i32).abs() <= 1);
            assert!((back.b as i32 - c.b as i32).abs() <= 1);
        }
    }

    #[test]
    fn lerp_and_scale() {
        assert_eq!(
            Rgb::lerp(Rgb::BLACK, Rgb::WHITE, 0.5),
            Rgb::new(128, 128, 128)
        );
        assert_eq!(Rgb::WHITE.scale(0.0), Rgb::BLACK);
    }
}
