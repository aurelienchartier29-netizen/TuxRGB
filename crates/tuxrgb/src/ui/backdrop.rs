//! Soft background of the setup canvas: the current effect rendered over the
//! whole canvas by the engine ([`Backdrop`], 48 × 27 cells), turned here into
//! a blurred glow that sits behind the devices.
//!
//! Built once per preview frame (≤ 30 per second, only while a window is
//! open) by the preview feed, off the interface thread, never in `view`
//! (working buffers reused, position-only factors computed once): upscale
//! ×4 (bilinear), three separable box blurs
//! (close to a Gaussian), then an alpha made of the glow strength, a gentle
//! vignette and a feathered rounded-rectangle edge. Drawn over the plain
//! canvas sheet, the glow stays calm, fades out before the rounded corners
//! (whatever the canvas size and renderer) and the devices drawn on top keep
//! a crisp contrast. About 0.3 ms per frame.

use iced::widget::image::Handle;
use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use tuxrgb_core::engine::Backdrop;

/// Size of the image handed to the GPU (stretched with linear filtering).
pub const OUT_W: usize = 192;
pub const OUT_H: usize = 108;
const SCALE: usize = 4;
/// Box blur radius, in output pixels (three passes ≈ Gaussian σ ≈ 6.5 px,
/// about 1.6 engine cells).
const BLUR: usize = 6;
const BLUR_PASSES: usize = 3;
/// Edge of the glow, in output pixels: fully transparent along the border,
/// fully visible `FEATHER` pixels further in, with rounded corners.
const INSET: f32 = 2.0;
const FEATHER: f32 = 9.0;
const CORNER: f32 = 12.0;

/// How much of the effect shows through the canvas colour (glow strength):
/// enough to read the effect, not enough to compete with the LEDs.
pub fn strength(dark: bool) -> f32 {
    if dark { 0.52 } else { 0.42 }
}

/// Darkness of the theme on screen, for glows built off the interface
/// thread (the preview feed); kept current by the app after each update.
static PREVIEW_DARK: AtomicBool = AtomicBool::new(true);

pub fn set_preview_dark(dark: bool) {
    PREVIEW_DARK.store(dark, Ordering::Relaxed);
}

pub fn preview_dark() -> bool {
    PREVIEW_DARK.load(Ordering::Relaxed)
}

/// Builds the blurred RGBA image of `b` (straight alpha), to draw over the
/// canvas sheet.
pub fn soften(b: &Backdrop, strength: f32) -> Handle {
    Handle::from_rgba(OUT_W as u32, OUT_H as u32, soften_rgba(b, strength))
}

/// Working buffers of [`soften_rgba`], kept per thread: no 0.5 MB of
/// allocations for each preview frame.
#[derive(Default)]
struct Scratch {
    src: Vec<[f32; 3]>,
    img: Vec<[f32; 3]>,
    tmp: Vec<[f32; 3]>,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::default();
}

/// Vignette and rounded-edge factors of every output pixel: they only
/// depend on the position, so they are computed once.
struct Mask {
    vignette: Vec<f32>,
    edge: Vec<f32>,
}

fn mask() -> &'static Mask {
    static MASK: OnceLock<Mask> = OnceLock::new();
    MASK.get_or_init(|| {
        let (ow, oh) = (OUT_W, OUT_H);
        let (hw, hh) = (ow as f32 * 0.5, oh as f32 * 0.5);
        let mut vignette = Vec::with_capacity(ow * oh);
        let mut edge = Vec::with_capacity(ow * oh);
        for y in 0..oh {
            let ny = (y as f32 + 0.5) / oh as f32 * 2.0 - 1.0;
            for x in 0..ow {
                let nx = (x as f32 + 0.5) / ow as f32 * 2.0 - 1.0;
                // 1 in the middle, ~0.7 towards the corners.
                vignette.push(1.0 - 0.3 * smoothstep(0.35, 1.45, (nx * nx + ny * ny).sqrt()));
                // Signed distance to the rounded rectangle inset by INSET.
                let qx = (x as f32 + 0.5 - hw).abs() - (hw - INSET - CORNER);
                let qy = (y as f32 + 0.5 - hh).abs() - (hh - INSET - CORNER);
                let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                let d = outside + qx.max(qy).min(0.0) - CORNER;
                edge.push(smoothstep(0.0, FEATHER, -d));
            }
        }
        Mask { vignette, edge }
    })
}

pub fn soften_rgba(b: &Backdrop, strength: f32) -> Vec<u8> {
    SCRATCH.with(|scratch| {
        let mut scratch = scratch.borrow_mut();
        let Scratch { src, img, tmp } = &mut *scratch;
        soften_into(b, strength, src, img, tmp)
    })
}

fn soften_into(
    b: &Backdrop,
    strength: f32,
    src: &mut Vec<[f32; 3]>,
    img: &mut Vec<[f32; 3]>,
    tmp: &mut Vec<[f32; 3]>,
) -> Vec<u8> {
    let (w, h) = (b.width.max(1), b.height.max(1));
    src.clear();
    if b.colors.len() == w * h {
        src.extend(
            b.colors
                .iter()
                .map(|c| [c.r as f32, c.g as f32, c.b as f32]),
        );
    } else {
        src.resize(w * h, [0.0; 3]);
    }

    // Bilinear upscale onto the output grid (cell centres aligned).
    let (ow, oh) = (OUT_W, OUT_H);
    img.clear();
    img.resize(ow * oh, [0.0f32; 3]);
    for y in 0..oh {
        let fy = ((y as f32 + 0.5) * h as f32 / oh as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(h - 1);
        for x in 0..ow {
            let fx = ((x as f32 + 0.5) * w as f32 / ow as f32 - 0.5).clamp(0.0, (w - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(w - 1);
            let (a, bb, c, d) = (
                src[y0 * w + x0],
                src[y0 * w + x1],
                src[y1 * w + x0],
                src[y1 * w + x1],
            );
            let px = &mut img[y * ow + x];
            for k in 0..3 {
                let top = a[k] + (bb[k] - a[k]) * tx;
                let bot = c[k] + (d[k] - c[k]) * tx;
                px[k] = top + (bot - top) * ty;
            }
        }
    }
    debug_assert_eq!(ow, w * SCALE);

    // Separable box blurs (running sums, clamped edges).
    tmp.clear();
    tmp.resize(ow * oh, [0.0f32; 3]);
    for _ in 0..BLUR_PASSES {
        box_pass(img, tmp, ow, oh, BLUR, true);
        box_pass(tmp, img, ow, oh, BLUR, false);
    }

    // Colour = the blurred effect; alpha = strength × vignette × edge.
    let mask = mask();
    let mut out = Vec::with_capacity(ow * oh * 4);
    for (i, p) in img.iter().enumerate() {
        let a = strength * mask.vignette[i] * mask.edge[i];
        for c in *p {
            out.push(c.round().clamp(0.0, 255.0) as u8);
        }
        out.push((a * 255.0).round().clamp(0.0, 255.0) as u8);
    }
    out
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One box blur pass along rows (`horizontal`) or columns.
fn box_pass(
    src: &[[f32; 3]],
    dst: &mut [[f32; 3]],
    w: usize,
    h: usize,
    r: usize,
    horizontal: bool,
) {
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let at = |line: usize, i: usize| {
        if horizontal {
            line * w + i
        } else {
            i * w + line
        }
    };
    let n = (2 * r + 1) as f32;
    for line in 0..lines {
        let get = |i: isize| src[at(line, i.clamp(0, len as isize - 1) as usize)];
        let mut sum = [0.0f32; 3];
        for i in -(r as isize)..=(r as isize) {
            let p = get(i);
            for k in 0..3 {
                sum[k] += p[k];
            }
        }
        for i in 0..len {
            dst[at(line, i)] = [sum[0] / n, sum[1] / n, sum[2] / n];
            let add = get(i as isize + r as isize + 1);
            let sub = get(i as isize - r as isize);
            for k in 0..3 {
                sum[k] += add[k] - sub[k];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tuxrgb_core::Rgb;
    use tuxrgb_core::engine::{BACKDROP_H, BACKDROP_W};

    fn backdrop(f: impl Fn(usize, usize) -> Rgb) -> Backdrop {
        let mut colors = Vec::new();
        for y in 0..BACKDROP_H {
            for x in 0..BACKDROP_W {
                colors.push(f(x, y));
            }
        }
        Backdrop {
            width: BACKDROP_W,
            height: BACKDROP_H,
            colors,
        }
    }

    #[test]
    fn uniform_colour_becomes_a_calm_glow_with_soft_edges() {
        let b = backdrop(|_, _| Rgb::new(255, 0, 0));
        let px = soften_rgba(&b, 0.6);
        assert_eq!(px.len(), OUT_W * OUT_H * 4);
        let alpha = |x: usize, y: usize| px[(y * OUT_W + x) * 4 + 3];
        let centre = ((OUT_H / 2) * OUT_W + OUT_W / 2) * 4;
        // The effect colour, at 60 % in the middle.
        assert_eq!(&px[centre..centre + 3], &[255, 0, 0]);
        assert!((150..=155).contains(&px[centre + 3]), "{}", px[centre + 3]);
        // Transparent on the border and in the corners (the rounded sheet
        // shows there), dimmer towards the edges (vignette).
        assert_eq!(alpha(0, OUT_H / 2), 0);
        assert_eq!(alpha(OUT_W / 2, 0), 0);
        assert_eq!(alpha(4, 4), 0);
        assert!(alpha(30, 30) > 0 && alpha(30, 30) < alpha(OUT_W / 2, OUT_H / 2));
    }

    /// The original, straightforward version (allocating, mask computed
    /// per pixel): the optimised one must give exactly the same bytes.
    fn reference(b: &Backdrop, strength: f32) -> Vec<u8> {
        let (w, h) = (b.width.max(1), b.height.max(1));
        let src: Vec<[f32; 3]> = b
            .colors
            .iter()
            .map(|c| [c.r as f32, c.g as f32, c.b as f32])
            .collect();
        let (ow, oh) = (OUT_W, OUT_H);
        let mut img = vec![[0.0f32; 3]; ow * oh];
        for y in 0..oh {
            let fy = ((y as f32 + 0.5) * h as f32 / oh as f32 - 0.5).clamp(0.0, (h - 1) as f32);
            let (y0, ty) = (fy.floor() as usize, fy.fract());
            let y1 = (y0 + 1).min(h - 1);
            for x in 0..ow {
                let fx = ((x as f32 + 0.5) * w as f32 / ow as f32 - 0.5).clamp(0.0, (w - 1) as f32);
                let (x0, tx) = (fx.floor() as usize, fx.fract());
                let x1 = (x0 + 1).min(w - 1);
                let (a, bb, c, d) = (
                    src[y0 * w + x0],
                    src[y0 * w + x1],
                    src[y1 * w + x0],
                    src[y1 * w + x1],
                );
                let px = &mut img[y * ow + x];
                for k in 0..3 {
                    let top = a[k] + (bb[k] - a[k]) * tx;
                    let bot = c[k] + (d[k] - c[k]) * tx;
                    px[k] = top + (bot - top) * ty;
                }
            }
        }
        let mut tmp = vec![[0.0f32; 3]; ow * oh];
        for _ in 0..BLUR_PASSES {
            box_pass(&img, &mut tmp, ow, oh, BLUR, true);
            box_pass(&tmp, &mut img, ow, oh, BLUR, false);
        }
        let (hw, hh) = (ow as f32 * 0.5, oh as f32 * 0.5);
        let mut out = Vec::with_capacity(ow * oh * 4);
        for y in 0..oh {
            let ny = (y as f32 + 0.5) / oh as f32 * 2.0 - 1.0;
            for x in 0..ow {
                let nx = (x as f32 + 0.5) / ow as f32 * 2.0 - 1.0;
                let v = 1.0 - 0.3 * smoothstep(0.35, 1.45, (nx * nx + ny * ny).sqrt());
                let qx = (x as f32 + 0.5 - hw).abs() - (hw - INSET - CORNER);
                let qy = (y as f32 + 0.5 - hh).abs() - (hh - INSET - CORNER);
                let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                let d = outside + qx.max(qy).min(0.0) - CORNER;
                let edge = smoothstep(0.0, FEATHER, -d);
                let a = strength * v * edge;
                let p = img[y * ow + x];
                for c in p {
                    out.push(c.round().clamp(0.0, 255.0) as u8);
                }
                out.push((a * 255.0).round().clamp(0.0, 255.0) as u8);
            }
        }
        out
    }

    #[test]
    fn optimised_glow_is_byte_identical() {
        let frames = [
            backdrop(|x, y| Rgb::new((x * 5) as u8, (y * 9) as u8, ((x * y) % 256) as u8)),
            backdrop(|_, _| Rgb::new(255, 0, 0)),
            backdrop(|x, _| if x % 7 < 3 { Rgb::WHITE } else { Rgb::BLACK }),
        ];
        // Several frames in a row on the same thread (reused buffers).
        for strength in [strength(true), strength(false), 1.0] {
            for b in &frames {
                assert_eq!(soften_rgba(b, strength), reference(b, strength));
            }
        }
    }

    #[test]
    fn sharp_edges_are_blurred() {
        // Left half white, right half black.
        let b = backdrop(|x, _| {
            if x < BACKDROP_W / 2 {
                Rgb::WHITE
            } else {
                Rgb::BLACK
            }
        });
        let px = soften_rgba(&b, 1.0);
        let row = OUT_H / 2;
        let at = |x: usize| px[(row * OUT_W + x) * 4];
        let mid = OUT_W / 2;
        // A gradient across the border instead of a step.
        assert!(at(mid - 8) > at(mid) && at(mid) > at(mid + 8));
        assert!(at(mid - 8) < 255 && at(mid + 8) > 0);
    }
}

#[cfg(test)]
mod dump {
    use super::*;
    use tuxrgb_core::Rgb;
    use tuxrgb_core::engine::{BACKDROP_H, BACKDROP_W};

    /// `TUXRGB_DUMP=/path.rgba cargo test -p tuxrgb dump_gradient -- --ignored`
    #[test]
    #[ignore]
    fn dump_gradient() {
        let mut colors = Vec::new();
        for _y in 0..BACKDROP_H {
            for x in 0..BACKDROP_W {
                let t = (x as f32 / BACKDROP_W as f32 * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                colors.push(Rgb::new((255.0 * t) as u8, 0, (200.0 * (1.0 - t)) as u8));
            }
        }
        let b = Backdrop {
            width: BACKDROP_W,
            height: BACKDROP_H,
            colors,
        };
        let px = soften_rgba(&b, 0.62);
        std::fs::write(std::env::var("TUXRGB_DUMP").unwrap(), px).unwrap();
    }
}
