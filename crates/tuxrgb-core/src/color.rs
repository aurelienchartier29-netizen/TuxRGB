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

    /// Scales the HSV saturation by `amount` (`1.0` leaves the colour
    /// unchanged, `0.0` gives white at the same intensity, `2.0` doubles it,
    /// up to the pure hue). Greys and white have no saturation and stay as
    /// they are, so a boost never tints them.
    pub fn saturate(self, amount: f32) -> Self {
        let (h, s, v) = self.to_hsv();
        Self::from_hsv(h, (s * amount).clamp(0.0, 1.0), v)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturation_boost_keeps_white_and_greys() {
        assert_eq!(Rgb::WHITE.saturate(2.0), Rgb::WHITE);
        assert_eq!(Rgb::new(90, 90, 90).saturate(2.0), Rgb::new(90, 90, 90));
        // 50 % saturated red → 100 % at "200 %".
        let c = Rgb::new(255, 127, 127).saturate(2.0);
        assert_eq!(c, Rgb::new(255, 0, 0));
        // 25 % → 50 %.
        let (_, s, _) = Rgb::new(200, 150, 150).saturate(2.0).to_hsv();
        assert!((s - 0.5).abs() < 0.02, "{s}");
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
