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

    /// What the colours go through on their way to the hardware.
    pub fn grade(&self) -> LedGrade {
        let c = self.sanitized();
        let (gamma, white) = if self.enabled {
            (c.gamma, c.white)
        } else {
            (1.0, [1.0; 3])
        };
        let mut table = [[0f32; 256]; 3];
        for (ch, t) in table.iter_mut().enumerate() {
            for (v, out) in t.iter_mut().enumerate() {
                *out = (v as f32 / 255.0).powf(gamma) * white[ch] * 255.0;
            }
        }
        LedGrade { table, gamma }
    }
}

/// Brightness, curve and white balance of the LED colours, in one step.
///
/// The colours arrive at full brightness with the frame's level beside them
/// ([`crate::engine::Frame::level`]). Done in 8 bits one after the other,
/// the dimming and the curve round each channel on its own: at low
/// brightness a weak channel drops to 0 at one step of the slider and comes
/// back at the next, and the colour flickers between close hues (a white
/// going magenta, a pink going red). Here the brightest channel is rounded
/// first, and the two others keep their exact share of it: a colour only
/// gets darker when the slider moves, never another hue.
#[derive(Clone, Debug, PartialEq)]
pub struct LedGrade {
    /// Light of each channel value at full brightness, `0..=255`.
    table: [[f32; 256]; 3],
    gamma: f32,
}

impl Default for LedGrade {
    fn default() -> Self {
        LedCalibration {
            enabled: false,
            ..LedCalibration::default()
        }
        .grade()
    }
}

impl LedGrade {
    /// Factor on the light for a frame `level` (the colours are dimmed
    /// before the curve: the curve raises the level to its gamma).
    pub fn scale(&self, level: f32) -> f32 {
        if level.is_finite() {
            level.clamp(0.0, 1.0).powf(self.gamma)
        } else {
            1.0
        }
    }

    /// `c` (full brightness) as sent to the LEDs at `scale`.
    #[inline]
    pub fn apply(&self, c: Rgb, scale: f32) -> Rgb {
        let f = [
            self.table[0][c.r as usize],
            self.table[1][c.g as usize],
            self.table[2][c.b as usize],
        ];
        let max = f[0].max(f[1]).max(f[2]);
        if max <= 0.0 {
            return Rgb::BLACK;
        }
        let top = (max * scale + 0.5).floor().min(255.0);
        if top <= 0.0 {
            return Rgb::BLACK;
        }
        let k = top / max;
        // Ties (a share of exactly x.5) always round up: the same colour one
        // step brighter never loses a step on another channel.
        let q = |x: f32| (x * k + 0.5 + 1e-3).floor().clamp(0.0, 255.0) as u8;
        Rgb::new(q(f[0]), q(f[1]), q(f[2]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::perceived_level;

    fn full(c: &LedCalibration, rgb: Rgb) -> Rgb {
        c.grade().apply(rgb, 1.0)
    }

    #[test]
    fn off_changes_nothing() {
        let c = LedCalibration {
            enabled: false,
            ..LedCalibration::default()
        };
        for v in [0u8, 1, 80, 128, 255] {
            assert_eq!(full(&c, Rgb::new(v, v, v)), Rgb::new(v, v, v));
        }
        assert_eq!(full(&c, Rgb::new(230, 80, 106)), Rgb::new(230, 80, 106));
        assert_eq!(c.level_exponent(), 2.0);
        // Without correction: a plain dimming.
        let g = c.grade();
        assert_eq!(
            g.apply(Rgb::new(200, 100, 0), g.scale(0.5)),
            Rgb::new(100, 50, 0)
        );
    }

    #[test]
    fn pink_of_the_interface_stays_pink() {
        // An accent pink: much red, a little green and blue.
        let pink = Rgb::new(230, 80, 106);
        let neutral = LedCalibration {
            white: [1.0; 3],
            ..LedCalibration::default()
        };
        let out = full(&neutral, pink);
        // Raw, blue was 46 % of red (purple on LEDs); as light on a
        // screen it is ~18 %: the LEDs now get that.
        let ratio = out.b as f32 / out.r as f32;
        assert!((0.15..0.21).contains(&ratio), "{out:?}");
        assert!(out.g < 25, "{out:?}");
    }

    #[test]
    fn ends_and_white_balance() {
        let c = LedCalibration::default();
        assert_eq!(full(&c, Rgb::BLACK), Rgb::BLACK);
        // Full white goes through the white balance only.
        assert_eq!(full(&c, Rgb::WHITE), Rgb::new(255, 204, 230));
        // Monotonic on every channel.
        let mut prev = Rgb::BLACK;
        for v in 0..=255u8 {
            let o = full(&c, Rgb::new(v, v, v));
            assert!(o.r >= prev.r && o.g >= prev.g && o.b >= prev.b);
            prev = o;
        }
    }

    /// Lowering the brightness only darkens a colour: no channel comes back
    /// up, and the hue stays (no white turning magenta at low light).
    #[test]
    fn lowering_the_brightness_never_changes_the_hue() {
        let c = LedCalibration::default();
        let g = c.grade();
        for colour in [
            Rgb::WHITE,
            Rgb::new(230, 80, 106),
            Rgb::new(61, 174, 233),
            Rgb::new(255, 128, 0),
        ] {
            let ideal = g.apply(colour, 1.0);
            let mut prev = ideal;
            for i in (0..=1000).rev() {
                let level = perceived_level(i as f32 / 1000.0, c.level_exponent());
                let now = g.apply(colour, g.scale(level));
                assert!(
                    now.r <= prev.r && now.g <= prev.g && now.b <= prev.b,
                    "{colour:?}: {prev:?} -> {now:?}"
                );
                prev = now;
            }
            assert_eq!(prev, Rgb::BLACK);
        }
        // White stays white down to the last step (independent rounding
        // gave (1, 0, 1), magenta).
        for i in 1..=60 {
            let level = perceived_level(i as f32 / 1000.0, c.level_exponent());
            let w = g.apply(Rgb::WHITE, g.scale(level));
            assert!(w == Rgb::BLACK || (w.g > 0 && w.b > 0), "{w:?} at {i}");
        }
    }

    #[test]
    fn brightness_keeps_its_feel() {
        // Slider at 50 %: the light must stay 25 % of full (as before).
        let c = LedCalibration {
            white: [1.0; 3],
            ..LedCalibration::default()
        };
        let g = c.grade();
        let level = perceived_level(0.5, c.level_exponent());
        let light = g.apply(Rgb::WHITE, g.scale(level)).r as f32 / 255.0;
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
