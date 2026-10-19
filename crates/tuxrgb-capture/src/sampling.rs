//! Shared by the screen capture of every platform: the downscaled frame
//! the Screen Sync effect reads.

use tuxrgb_core::Rgb;

/// Width/height of the analysed frame: fine enough for every LED to get
/// its own part of the screen (each cell is a box-filtered average).
pub const SAMPLE_W: usize = 128;
pub const SAMPLE_H: usize = 72;
/// Pixels averaged per cell and axis (4 × 4 = 16 samples per cell).
pub(crate) const SUB: usize = 4;
/// Upper bound of analysed frames per second.
pub const MAX_FPS: u32 = 30;

/// Box-filtered downscale of a 4-bytes-per-pixel frame to
/// `SAMPLE_W × SAMPLE_H`: each cell averages `SUB × SUB` pixels spread over
/// the area it covers, so thin bright details are not missed or flickering.
pub fn downsample(
    bytes: &[u8],
    w: usize,
    h: usize,
    stride: usize,
    order: (usize, usize, usize),
) -> Vec<Rgb> {
    let mut pixels = Vec::with_capacity(SAMPLE_W * SAMPLE_H);
    downsample_into(&mut pixels, bytes, w, h, stride, order);
    pixels
}

/// [`downsample`] appending to an existing buffer (reused between frames).
pub fn downsample_into(
    pixels: &mut Vec<Rgb>,
    bytes: &[u8],
    w: usize,
    h: usize,
    stride: usize,
    (ri, gi, bi): (usize, usize, usize),
) {
    pixels.reserve(SAMPLE_W * SAMPLE_H);
    for sy in 0..SAMPLE_H {
        for sx in 0..SAMPLE_W {
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for jy in 0..SUB {
                let y = ((sy * SUB + jy) * h + h / (2 * SAMPLE_H * SUB)) / (SAMPLE_H * SUB);
                for jx in 0..SUB {
                    let x = ((sx * SUB + jx) * w + w / (2 * SAMPLE_W * SUB)) / (SAMPLE_W * SUB);
                    let i = y.min(h - 1) * stride + x.min(w - 1) * 4;
                    if i + 3 < bytes.len() {
                        r += bytes[i + ri] as u32;
                        g += bytes[i + gi] as u32;
                        b += bytes[i + bi] as u32;
                        n += 1;
                    }
                }
            }
            let n = n.max(1);
            pixels.push(Rgb::new((r / n) as u8, (g / n) as u8, (b / n) as u8));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsample_keeps_regions_where_they_are() {
        // 256×144 BGRx frame: left half red, right half blue, a white line
        // one pixel high in the middle of the top-left quarter.
        let (w, h) = (256usize, 144usize);
        let mut bytes = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let (r, g, b) = if y == 36 && x < 128 {
                    (255, 255, 255)
                } else if x < w / 2 {
                    (255, 0, 0)
                } else {
                    (0, 0, 255)
                };
                bytes[i] = b;
                bytes[i + 1] = g;
                bytes[i + 2] = r;
            }
        }
        let px = downsample(&bytes, w, h, w * 4, (2, 1, 0));
        assert_eq!(px.len(), SAMPLE_W * SAMPLE_H);
        assert_eq!(px[10 * SAMPLE_W + 5], Rgb::new(255, 0, 0));
        assert_eq!(px[60 * SAMPLE_W + 120], Rgb::new(0, 0, 255));
        // The thin line is averaged in, never lost.
        let row = 36 * SAMPLE_H / h;
        assert!(px[row * SAMPLE_W + 10].g > 0);
    }
}
