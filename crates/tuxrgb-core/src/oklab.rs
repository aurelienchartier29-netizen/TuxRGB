//! OKLab / Okhsv colour math for the global saturation (TuxRGB 3.0).
//!
//! The saturation slider scales the saturation of every colour in Okhsv
//! (Björn Ottosson, 2021): an HSV model whose hue is the perceptual OKLab
//! hue, whose value reaches 1 exactly on the top of the sRGB gamut (one
//! channel at 255) and whose saturation reaches 1 exactly on its edge (one
//! channel at 0). Scaling `s` at constant hue and value is therefore:
//!
//! * free of hue drift: the OKLab hue is never touched (HSV's own hue is
//!   not perceptual: a desaturated blue turns lavender, a boosted salmon
//!   becomes a different red, the 2.0.5 "deepening" turned amber into
//!   orange);
//! * free of clipping: `s` stays within `0..=1`, so every result is a real
//!   colour, and above 100 % a soft knee keeps two different colours
//!   different (a hard clamp at `s = 1` flattened gradients into one pure
//!   colour);
//! * neutral for brightness: `v` does not move, so fades, breathing,
//!   comet tails and the brightness slider behave exactly as before.
//!
//! Colours that are already pure (one channel at 0: a rainbow, a primary,
//! the Windows accent #0078D4) are the most saturated colour of their hue
//! the LEDs can show: they stay as they are above 100 %.
//!
//! The gamut functions follow Ottosson's public code (MIT licence). The
//! maximum saturation of a hue is refined by Halley steps on whichever
//! channel actually reaches zero first: his single step is off by up to 2 %
//! near the primaries, which would leave pure colours slightly below
//! `s = 1` (relative error here < 1e-6).

/// sRGB transfer function, decoding (`0..=1`).
#[inline]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB transfer function, encoding (`0..=1`).
#[inline]
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Decoded value of every 8-bit sRGB code (no `powf` per channel).
fn decode_table() -> &'static [f32; 256] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<[f32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| std::array::from_fn(|i| srgb_to_linear(i as f32 / 255.0)))
}

/// Linear sRGB of an 8-bit colour.
#[inline]
pub fn decode(r: u8, g: u8, b: u8) -> [f32; 3] {
    let t = decode_table();
    [t[r as usize], t[g as usize], t[b as usize]]
}

/// 8-bit sRGB code of a linear value (rounded, clamped).
#[inline]
pub fn encode(c: f32) -> u8 {
    (linear_to_srgb(c.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

pub fn linear_srgb_to_oklab([r, g, b]: [f32; 3]) -> Lab {
    let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l_, m_, s_) = (l.cbrt(), m.cbrt(), s.cbrt());
    Lab {
        l: 0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_,
        a: 1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_,
        b: 0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_,
    }
}

/// LMS (cubed) → linear sRGB, one row per channel.
const LMS_TO_RGB: [[f32; 3]; 3] = [
    [4.076_741_7, -3.307_711_6, 0.230_969_94],
    [-1.268_438, 2.609_757_4, -0.341_319_38],
    [-0.004_196_086_3, -0.703_418_6, 1.707_614_7],
];

#[inline]
fn dot(w: [f32; 3], v: [f32; 3]) -> f32 {
    w[0] * v[0] + w[1] * v[1] + w[2] * v[2]
}

pub fn oklab_to_linear_srgb(c: Lab) -> [f32; 3] {
    let l_ = c.l + 0.396_337_78 * c.a + 0.215_803_76 * c.b;
    let m_ = c.l - 0.105_561_346 * c.a - 0.063_854_17 * c.b;
    let s_ = c.l - 0.089_484_18 * c.a - 1.291_485_5 * c.b;
    let lms = [l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_];
    [
        dot(LMS_TO_RGB[0], lms),
        dot(LMS_TO_RGB[1], lms),
        dot(LMS_TO_RGB[2], lms),
    ]
}

/// Value of channel `i` at saturation `s` (L = 1) along the hue whose LMS
/// slopes are `k`, with its first and second derivatives.
#[inline]
fn channel_at(i: usize, s: f32, k: [f32; 3]) -> (f32, f32, f32) {
    let x = [1.0 + s * k[0], 1.0 + s * k[1], 1.0 + s * k[2]];
    let lms = [x[0] * x[0] * x[0], x[1] * x[1] * x[1], x[2] * x[2] * x[2]];
    let d1 = [
        3.0 * k[0] * x[0] * x[0],
        3.0 * k[1] * x[1] * x[1],
        3.0 * k[2] * x[2] * x[2],
    ];
    let d2 = [
        6.0 * k[0] * k[0] * x[0],
        6.0 * k[1] * k[1] * x[1],
        6.0 * k[2] * k[2] * x[2],
    ];
    let w = LMS_TO_RGB[i];
    (dot(w, lms), dot(w, d1), dot(w, d2))
}

/// Lowest channel at saturation `s` (L = 1): `(index, value)`.
#[inline]
fn lowest_channel(s: f32, k: [f32; 3]) -> (usize, f32) {
    let x = [1.0 + s * k[0], 1.0 + s * k[1], 1.0 + s * k[2]];
    let lms = [x[0] * x[0] * x[0], x[1] * x[1] * x[1], x[2] * x[2] * x[2]];
    let v = [
        dot(LMS_TO_RGB[0], lms),
        dot(LMS_TO_RGB[1], lms),
        dot(LMS_TO_RGB[2], lms),
    ];
    if v[0] <= v[1] && v[0] <= v[2] {
        (0, v[0])
    } else if v[1] <= v[2] {
        (1, v[1])
    } else {
        (2, v[2])
    }
}

/// One Halley step towards the root of channel `i`.
#[inline]
fn halley(i: usize, s: f32, k: [f32; 3]) -> f32 {
    let (f, f1, f2) = channel_at(i, s, k);
    let denom = f1 * f1 - 0.5 * f * f2;
    if denom.abs() < 1e-12 {
        return s;
    }
    s - f * f1 / denom
}

/// Largest saturation `S = C / L` inside the gamut for the normalised hue
/// direction `(a, b)` (`a² + b² = 1`).
fn max_saturation(a: f32, b: f32) -> f32 {
    // Initial guess: Ottosson's polynomial for the channel that usually
    // reaches zero first at this hue.
    let (p, first) = if -1.881_703_3 * a - 0.809_364_9 * b > 1.0 {
        (
            [
                1.190_862_8,
                1.765_767_3,
                0.596_626_4,
                0.755_152,
                0.567_712_4,
            ],
            0,
        )
    } else if 1.814_441 * a - 1.194_452_8 * b > 1.0 {
        (
            [
                0.739_565_15,
                -0.459_544_04,
                0.082_854_27,
                0.125_410_7,
                0.145_032_04,
            ],
            1,
        )
    } else {
        (
            [
                1.357_336_5,
                -0.009_157_99,
                -1.151_302_1,
                -0.505_596_06,
                0.006_921_67,
            ],
            2,
        )
    };
    let k = [
        0.396_337_78 * a + 0.215_803_76 * b,
        -0.105_561_346 * a - 0.063_854_17 * b,
        -0.089_484_18 * a - 1.291_485_5 * b,
    ];
    let mut s = p[0] + p[1] * a + p[2] * b + p[3] * a * a + p[4] * a * b;
    s = halley(first, s, k);
    // Refine on whichever channel is the lowest now (near a primary, the
    // polynomial's channel may not be the one that reaches zero first).
    for _ in 0..3 {
        let (i, v) = lowest_channel(s, k);
        if v.abs() < 1e-6 {
            break;
        }
        s = halley(i, s, k);
    }
    // Just on the cyan side of pure blue (OKLab hue 264.05°–264.21°) red
    // grazes zero – it dips a few millionths below and comes back – before
    // green reaches zero: the real edge, where pure blue itself lies, is
    // green's root, further out. Without this, blues a hair apart in hue
    // would get cusps 15 % apart.
    if b < -0.9 && (-0.106_5..=-0.098_5).contains(&a) {
        let mut g = 0.75;
        for _ in 0..6 {
            g = halley(1, g, k);
        }
        if g > s && g < 0.75 && lowest_channel(g, k).1 >= -1e-3 {
            s = g;
        }
    }
    s
}

/// Lightness and chroma of the gamut's cusp (most chromatic colour) for a
/// normalised hue direction.
pub fn find_cusp(a: f32, b: f32) -> (f32, f32) {
    let s_cusp = max_saturation(a, b);
    let rgb = oklab_to_linear_srgb(Lab {
        l: 1.0,
        a: s_cusp * a,
        b: s_cusp * b,
    });
    let l_cusp = (1.0 / rgb[0].max(rgb[1]).max(rgb[2])).cbrt();
    (l_cusp, l_cusp * s_cusp)
}

const TOE_K1: f32 = 0.206;
const TOE_K2: f32 = 0.03;
const TOE_K3: f32 = (1.0 + TOE_K1) / (1.0 + TOE_K2);

/// Okhsv's lightness estimate (closer to CIE L* in the darks).
#[inline]
fn toe(x: f32) -> f32 {
    let y = TOE_K3 * x - TOE_K1;
    0.5 * (y + (y * y + 4.0 * TOE_K2 * TOE_K3 * x).sqrt())
}

#[inline]
fn toe_inv(x: f32) -> f32 {
    (x * x + TOE_K1 * x) / (TOE_K3 * (x + TOE_K2))
}

/// A colour in Okhsv, with what its hue needs to convert it back.
#[derive(Clone, Copy, Debug)]
pub struct Okhsv {
    /// Normalised OKLab hue direction.
    pub a: f32,
    pub b: f32,
    /// Saturation, `0..=1` (1 = on the gamut edge: one channel at 0).
    pub s: f32,
    /// Value, `0..=1` (1 = on the gamut top: one channel at 255).
    pub v: f32,
    s_max: f32,
    t_max: f32,
}

/// Okhsv of an 8-bit colour; `None` for greys and black (no hue).
pub fn to_okhsv(r: u8, g: u8, b: u8) -> Option<Okhsv> {
    let lab = linear_srgb_to_oklab(decode(r, g, b));
    let c = (lab.a * lab.a + lab.b * lab.b).sqrt();
    if c < 1e-4 || lab.l <= 1e-6 {
        return None;
    }
    let (a_, b_) = (lab.a / c, lab.b / c);
    let (cusp_l, cusp_c) = find_cusp(a_, b_);
    let s_max = cusp_c / cusp_l;
    let t_max = cusp_c / (1.0 - cusp_l).max(1e-6);
    const S0: f32 = 0.5;
    let k = 1.0 - S0 / s_max;

    let t = t_max / (c + lab.l * t_max);
    let l_v = t * lab.l;
    let c_v = t * c;
    let l_vt = toe_inv(l_v);
    let c_vt = c_v * l_vt / l_v;
    let scale = oklab_to_linear_srgb(Lab {
        l: l_vt,
        a: a_ * c_vt,
        b: b_ * c_vt,
    });
    let scale_l = (1.0 / scale[0].max(scale[1]).max(scale[2]).max(1e-9)).cbrt();
    let l = lab.l / scale_l;
    let c = c / scale_l;
    let l_t = toe(l);
    let c = c * l_t / l;
    let v = l_t / l_v;
    let s = (S0 + t_max) * c_v / (t_max * S0 + t_max * k * c_v);
    let _ = c;
    Some(Okhsv {
        a: a_,
        b: b_,
        s: s.clamp(0.0, 1.0),
        v: v.clamp(0.0, 1.0),
        s_max,
        t_max,
    })
}

/// Linear sRGB of an Okhsv colour (same hue as the one it came from).
pub fn from_okhsv(c: &Okhsv) -> [f32; 3] {
    const S0: f32 = 0.5;
    let (s, v) = (c.s.clamp(0.0, 1.0), c.v.clamp(0.0, 1.0));
    let (s_max, t_max) = (c.s_max, c.t_max);
    let k = 1.0 - S0 / s_max;
    let denom = S0 + t_max - t_max * k * s;
    let l_v = 1.0 - s * S0 / denom;
    let c_v = s * t_max * S0 / denom;
    let l = v * l_v;
    if l <= 1e-7 {
        return [0.0; 3];
    }
    let mut chroma = v * c_v;
    let l_vt = toe_inv(l_v);
    let c_vt = c_v * l_vt / l_v;
    let l_new = toe_inv(l);
    chroma *= l_new / l;
    let scale = oklab_to_linear_srgb(Lab {
        l: l_vt,
        a: c.a * c_vt,
        b: c.b * c_vt,
    });
    let scale_l = (1.0 / scale[0].max(scale[1]).max(scale[2]).max(1e-9)).cbrt();
    let l = l_new * scale_l;
    chroma *= scale_l;
    oklab_to_linear_srgb(Lab {
        l,
        a: chroma * c.a,
        b: chroma * c.b,
    })
}

/// New Okhsv saturation for the slider's `amount` (`1.0` = unchanged).
///
/// * Below 100 %: `s · amount` (`0` = white / grey of the same value).
/// * Above: `a·s / (1 + (a⁴ − 1)·s⁴)^¼` – `a·s` for weak colours (×2 at
///   200 %, as before), then a soft knee that reaches 1 only for colours
///   that were already pure. Strictly increasing in `s`: two colours that
///   differ still differ (no clipping).
#[inline]
pub fn saturation_curve(s: f32, amount: f32) -> f32 {
    let s = s.clamp(0.0, 1.0);
    if amount <= 1.0 {
        return s * amount.max(0.0);
    }
    let a = amount;
    let (a2, s2) = (a * a, s * s);
    (a * s / (1.0 + (a2 * a2 - 1.0) * s2 * s2).sqrt().sqrt()).min(1.0)
}

/// Global saturation of an 8-bit colour (see the module docs). Greys and
/// black come back unchanged.
pub fn saturate(r: u8, g: u8, b: u8, amount: f32) -> [u8; 3] {
    let Some(mut hsv) = to_okhsv(r, g, b) else {
        return [r, g, b];
    };
    let s = saturation_curve(hsv.s, amount);
    if (s - hsv.s).abs() < 1e-5 {
        // Pure colour boosted, or a change too small to show.
        return [r, g, b];
    }
    hsv.s = s;
    let out = from_okhsv(&hsv);
    [encode(out[0]), encode(out[1]), encode(out[2])]
}

/// OKLCH hue angle (radians) of an 8-bit colour, `None` for greys.
pub fn hue_of(r: u8, g: u8, b: u8) -> Option<f32> {
    let lab = linear_srgb_to_oklab(decode(r, g, b));
    let c = (lab.a * lab.a + lab.b * lab.b).sqrt();
    (c > 1e-3).then(|| lab.b.atan2(lab.a))
}

/// OKLCH chroma of an 8-bit colour.
pub fn chroma_of(r: u8, g: u8, b: u8) -> f32 {
    let lab = linear_srgb_to_oklab(decode(r, g, b));
    (lab.a * lab.a + lab.b * lab.b).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklab_round_trip() {
        for &(r, g, b) in &[
            (0u8, 120u8, 212u8),
            (255, 0, 0),
            (12, 200, 40),
            (255, 255, 255),
            (3, 3, 3),
        ] {
            let lin = decode(r, g, b);
            let back = oklab_to_linear_srgb(linear_srgb_to_oklab(lin));
            for i in 0..3 {
                assert!(
                    (back[i] - lin[i]).abs() < 1e-4,
                    "{r},{g},{b}: {back:?} vs {lin:?}"
                );
            }
        }
        // White is L = 1, neutral.
        let w = linear_srgb_to_oklab([1.0, 1.0, 1.0]);
        assert!(
            (w.l - 1.0).abs() < 1e-3 && w.a.abs() < 1e-3 && w.b.abs() < 1e-3,
            "{w:?}"
        );
    }

    /// Largest `S` at L = 1 with every channel ≥ 0, by a scan from above
    /// (the in-gamut set along a ray is not always one interval near
    /// blue, so a plain bisection can stop at the wrong edge).
    fn reference_max_saturation(a: f32, b: f32) -> f32 {
        let inside = |s: f32| {
            let rgb = oklab_to_linear_srgb(Lab {
                l: 1.0,
                a: s * a,
                b: s * b,
            });
            rgb[0].min(rgb[1]).min(rgb[2]) >= 0.0
        };
        let mut s = 1.5f32;
        while s > 0.0 && !inside(s) {
            s -= 1e-4;
        }
        let (mut lo, mut hi) = (s, s + 1e-4);
        for _ in 0..30 {
            let mid = 0.5 * (lo + hi);
            if inside(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    }

    #[test]
    fn max_saturation_is_exact_for_every_hue() {
        for i in 0..720 {
            let h = i as f32 / 720.0 * std::f32::consts::TAU;
            let (a, b) = (h.cos(), h.sin());
            let s = max_saturation(a, b);
            let reference = reference_max_saturation(a, b);
            assert!(
                ((s - reference) / reference).abs() < 2e-4,
                "hue {}: {s} vs {reference}",
                i as f32 / 2.0
            );
        }
    }

    #[test]
    fn cusps_are_on_the_gamut_edge() {
        // Every hue: the cusp colour has one channel at 1 and one at 0.
        for i in 0..360 {
            let h = i as f32 / 360.0 * std::f32::consts::TAU;
            let (a, b) = (h.cos(), h.sin());
            let (l, c) = find_cusp(a, b);
            let rgb = oklab_to_linear_srgb(Lab {
                l,
                a: c * a,
                b: c * b,
            });
            let max = rgb[0].max(rgb[1]).max(rgb[2]);
            let min = rgb[0].min(rgb[1]).min(rgb[2]);
            assert!((max - 1.0).abs() < 1e-3, "hue {i}: max {max}");
            assert!(min.abs() < 1e-3, "hue {i}: min {min}");
        }
    }

    #[test]
    fn okhsv_round_trips_and_spans_the_gamut() {
        let mut n = 0;
        for r in (0..=255u32).step_by(15) {
            for g in (0..=255u32).step_by(15) {
                for b in (0..=255u32).step_by(15) {
                    let (r, g, b) = (r as u8, g as u8, b as u8);
                    let Some(hsv) = to_okhsv(r, g, b) else {
                        continue;
                    };
                    let back = from_okhsv(&hsv);
                    let back = [encode(back[0]), encode(back[1]), encode(back[2])];
                    assert_eq!(back, [r, g, b], "s {} v {}", hsv.s, hsv.v);
                    // Top of the gamut ⇔ v = 1.
                    if r.max(g).max(b) == 255 {
                        assert!(hsv.v > 0.999, "{r},{g},{b}: v {}", hsv.v);
                    }
                    n += 1;
                }
            }
        }
        assert!(n > 4000, "{n}");
        // Pure colours: s = 1 (blues with a little green excepted, where
        // the OKLab hue barely moves: they are "less saturated blues").
        for (r, g, b) in [
            (255, 0, 0),
            (0, 255, 0),
            (0, 0, 255),
            (255, 128, 0),
            (0, 120, 212),
            (180, 0, 90),
        ] {
            let hsv = to_okhsv(r, g, b).unwrap();
            assert!(hsv.s > 0.999, "{r},{g},{b}: s {}", hsv.s);
        }
    }

    #[test]
    fn curve_is_continuous_monotonic_and_bounded() {
        for amount in [0.0f32, 0.3, 0.7, 1.0, 1.2, 1.5, 2.0] {
            let mut last = -1.0f32;
            for i in 0..=1000 {
                let s = i as f32 / 1000.0;
                let out = saturation_curve(s, amount);
                assert!((0.0..=1.0).contains(&out), "{amount} {s} → {out}");
                if amount > 0.0 {
                    assert!(
                        out > last || (out == 1.0 && last == 1.0),
                        "{amount}: {s} → {out} ≤ {last}"
                    );
                }
                if amount >= 1.0 {
                    assert!(out >= s - 1e-6, "{amount}: boost lowered {s} to {out}");
                }
                last = out;
            }
            assert!((saturation_curve(1.0, amount) - amount.min(1.0)).abs() < 1e-6);
        }
        // ×2 for weak colours at 200 %.
        assert!((saturation_curve(0.1, 2.0) - 0.2).abs() < 0.002);
    }
}
