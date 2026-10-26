//! Colours of the LEDs as on the screen.
//!
//! A screen and an RGB LED do not turn the same three numbers into the same
//! light:
//!
//! * a screen follows the sRGB curve: a value of 50 % gives about a fifth of
//!   the light. An LED is driven by PWM, linearly: 50 % gives half of the
//!   light. Every mixed colour then gets far too much of its weaker
//!   channels on the LEDs – a pink of the interface (much red, a little blue)
//!   comes out purple, a pastel comes out washed out;
//! * the three LED dies are not balanced like a screen's white: green and
//!   blue are much more efficient than red, so the "white" of most ARGB
//!   hardware is cold, and every colour leans that way.
//!
//! [`LedCalibration`] undoes both before the colours leave TuxRGB for the
//! hardware: each channel goes through the screen's curve (the gamma) and is
//! then weighted by the white balance. The interface keeps showing the
//! colours as they are meant; the LEDs now show the same ones.
//!
//! The brightness slider keeps its feel (see
//! [`LedCalibration::level_exponent`]).

use crate::Rgb;
use serde::{Deserialize, Serialize};

/// Gamma of the screens (sRGB is close to a 2.2 power).
pub const DEFAULT_GAMMA: f32 = 2.2;
/// White balance of typical ARGB LEDs: green and blue toned down (between
/// neutral and FastLED's `TypicalLEDStrip` correction).
pub const DEFAULT_WHITE: [f32; 3] = [1.0, 0.80, 0.90];
pub const GAMMA_RANGE: std::ops::RangeInclusive<f32> = 1.0..=3.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LedCalibration {
    /// Correct the colours. Off: the values go to the LEDs as they are
    /// (TuxRGB 3.1.2 and earlier).
    pub enabled: bool,
    /// Curve applied to each channel, [`GAMMA_RANGE`].
    pub gamma: f32,
    /// Weight of red, green and blue, `0..=1` each.
    pub white: [f32; 3],
}

impl Default for LedCalibration {
    fn default() -> Self {
        Self {
            enabled: true,
            gamma: DEFAULT_GAMMA,
            white: DEFAULT_WHITE,
        }
    }
}

impl LedCalibration {
    /// Values brought back into range (a hand-edited configuration).
    pub fn sanitized(self) -> Self {
        let clean =
            |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        Self {
            enabled: self.enabled,
            gamma: clean(
                self.gamma,
                *GAMMA_RANGE.start(),
                *GAMMA_RANGE.end(),
                DEFAULT_GAMMA,
            ),
            white: [
                clean(self.white[0], 0.0, 1.0, 1.0),
                clean(self.white[1], 0.0, 1.0, 1.0),
                clean(self.white[2], 0.0, 1.0, 1.0),
            ],
        }
    }

    /// Gamma in force (1 when the correction is off).
    pub fn effective_gamma(&self) -> f32 {
        if self.enabled {
            self.sanitized().gamma
        } else {
            1.0
        }
    }

    /// Exponent of the brightness slider so that the light of the LEDs
    /// stays the square of the slider (see `engine::perceived_level`)
    /// whatever the gamma: the colours are scaled before the curve, which
    /// raises the scale to the gamma.
    pub fn level_exponent(&self) -> f32 {
        2.0 / self.effective_gamma()
    }

    /// The table the colours go through on their way to the hardware.
    pub fn lut(&self) -> ColorLut {
        if !self.enabled {
            return ColorLut::IDENTITY;
        }
        let c = self.sanitized();
        let mut t = [[0u8; 256]; 3];
        for (ch, table) in t.iter_mut().enumerate() {
            for (v, out) in table.iter_mut().enumerate() {
                let x = (v as f32 / 255.0).powf(c.gamma) * c.white[ch];
                *out = (x * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
        ColorLut(t)
    }
}

/// Per-channel 8-bit table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorLut([[u8; 256]; 3]);

impl ColorLut {
    pub const IDENTITY: Self = {
        let mut t = [[0u8; 256]; 3];
        let mut v = 0;
        while v < 256 {
            t[0][v] = v as u8;
            t[1][v] = v as u8;
            t[2][v] = v as u8;
            v += 1;
        }
        Self(t)
    };

    #[inline]
    pub fn apply(&self, c: Rgb) -> Rgb {
        Rgb::new(
            self.0[0][c.r as usize],
            self.0[1][c.g as usize],
            self.0[2][c.b as usize],
        )
    }
}

impl Default for ColorLut {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_changes_nothing() {
        let c = LedCalibration {
            enabled: false,
            ..LedCalibration::default()
        };
        let lut = c.lut();
        for v in [0u8, 1, 80, 128, 255] {
            assert_eq!(lut.apply(Rgb::new(v, v, v)), Rgb::new(v, v, v));
        }
        assert_eq!(c.level_exponent(), 2.0);
    }

    #[test]
    fn pink_of_the_interface_stays_pink() {
        // An accent pink: much red, a little green and blue.
        let pink = Rgb::new(230, 80, 106);
        let neutral = LedCalibration {
            white: [1.0; 3],
            ..LedCalibration::default()
        };
        let out = neutral.lut().apply(pink);
        // Raw, blue was 46 % of red (purple on LEDs); as light on a
        // screen it is ~18 %: the LEDs now get that.
        let ratio = out.b as f32 / out.r as f32;
        assert!((0.15..0.21).contains(&ratio), "{out:?}");
        assert!(out.g < 25, "{out:?}");
    }

    #[test]
    fn ends_and_white_balance() {
        let lut = LedCalibration::default().lut();
        assert_eq!(lut.apply(Rgb::BLACK), Rgb::BLACK);
        // Full white goes through the white balance only.
        assert_eq!(lut.apply(Rgb::WHITE), Rgb::new(255, 204, 230));
        // Monotonic on every channel.
        let mut prev = Rgb::BLACK;
        for v in 0..=255u8 {
            let c = lut.apply(Rgb::new(v, v, v));
            assert!(c.r >= prev.r && c.g >= prev.g && c.b >= prev.b);
            prev = c;
        }
    }

    #[test]
    fn brightness_keeps_its_feel() {
        // Slider at 50 %: the light must stay 25 % of full (as before).
        let c = LedCalibration {
            white: [1.0; 3],
            ..LedCalibration::default()
        };
        let level = 0.5f32.powf(c.level_exponent());
        let v = (255.0 * level).round() as u8;
        let light = c.lut().apply(Rgb::new(v, v, v)).r as f32 / 255.0;
        assert!((light - 0.25).abs() < 0.01, "{light}");
    }

    #[test]
    fn nonsense_is_cleaned() {
        let c = LedCalibration {
            enabled: true,
            gamma: f32::NAN,
            white: [2.0, -1.0, 0.5],
        }
        .sanitized();
        assert_eq!(c.gamma, DEFAULT_GAMMA);
        assert_eq!(c.white, [1.0, 0.0, 0.5]);
    }
}
