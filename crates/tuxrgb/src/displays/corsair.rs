//! Corsair LCD screens: iCUE ELITE LCD caps (H100i / H150i / H170i Elite
//! LCD and LCD XT), iCUE LINK LCD (H150i / H170i LINK), iCUE LINK XD5 LCD,
//! XC7 ELITE LCD water block, Nautilus LCD caps.
//!
//! Every one is a 480 × 480 round screen fed with JPEG pictures in 1024-byte
//! HID reports, as OpenLinkHub (GPL-3.0) does: report 0x02, command 0x05,
//! "last part" flag, part number, part length, then the JPEG bytes.

use super::hardware_display::{Capabilities, Family, Frame, HardwareDisplay, Model, Picture};
use super::hid::Transport;
use std::io;
use std::time::Duration;

const VID: u16 = 0x1b1c;
const REPORT: usize = 1024;
const HEADER: usize = 8;
const SIDE: u32 = 480;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// ELITE LCD caps, XC7, Nautilus.
    Elite,
    /// iCUE LINK screens (third header byte 0x01).
    Link,
}

const fn model(pid: u16, name: &'static str, variant: Variant) -> Model {
    Model {
        vendor_id: VID,
        product_id: pid,
        vendor: "Corsair",
        name,
        family: Family::Corsair(variant),
    }
}

pub const MODELS: &[Model] = &[
    model(0x0c39, "iCUE ELITE LCD", Variant::Elite),
    model(0x0c33, "iCUE ELITE LCD", Variant::Elite),
    model(0x0c42, "XC7 ELITE LCD", Variant::Elite),
    model(0x0c55, "NAUTILUS LCD", Variant::Elite),
    model(0x0c57, "NAUTILUS LCD", Variant::Elite),
    model(0x0c4e, "iCUE LINK LCD", Variant::Link),
    model(0x0c43, "iCUE LINK XD5 LCD", Variant::Link),
];

pub fn capabilities(_v: Variant) -> Capabilities {
    let mut c = Capabilities::screen(SIDE, SIDE, true);
    c.min_interval = Duration::from_millis(500);
    c
}

/// The JPEG of a picture, at the given brightness (%): the screen has no
/// brightness command, the picture is dimmed instead.
pub fn jpeg(picture: &Picture, brightness: u8) -> io::Result<Vec<u8>> {
    let k = u16::from(brightness.min(100));
    let mut rgb = Vec::with_capacity((picture.width * picture.height * 3) as usize);
    for px in picture.rgba.chunks_exact(4) {
        for c in &px[..3] {
            rgb.push(((u16::from(*c) * k + 50) / 100) as u8);
        }
    }
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88)
        .encode(
            &rgb,
            picture.width,
            picture.height,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(io::Error::other)?;
    Ok(out)
}

/// The reports carrying one JPEG.
pub fn reports(jpeg: &[u8], variant: Variant) -> Vec<Vec<u8>> {
    let max = REPORT - HEADER;
    let mut out = Vec::new();
    let parts: Vec<&[u8]> = jpeg.chunks(max).collect();
    for (i, part) in parts.iter().enumerate() {
        let mut r = vec![0u8; REPORT];
        r[0] = 0x02;
        r[1] = 0x05;
        r[2] = u8::from(variant == Variant::Link);
        // The last part says so: the screen then shows the picture.
        r[3] = u8::from(i + 1 == parts.len());
        r[4] = i as u8;
        r[6..8].copy_from_slice(&(part.len() as u16).to_le_bytes());
        r[HEADER..HEADER + part.len()].copy_from_slice(part);
        out.push(r);
    }
    out
}

pub struct CorsairLcd {
    transport: Box<dyn Transport>,
    variant: Variant,
    caps: Capabilities,
    last: Option<u64>,
    /// Refreshes since the picture was last sent.
    unchanged: u32,
}

impl CorsairLcd {
    pub fn new(transport: Box<dyn Transport>, variant: Variant) -> Self {
        Self {
            transport,
            variant,
            caps: capabilities(variant),
            last: None,
            unchanged: 0,
        }
    }
}

impl HardwareDisplay for CorsairLcd {
    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn show(&mut self, frame: &Frame<'_>) -> io::Result<()> {
        let Some(picture) = frame.picture.as_deref() else {
            return Ok(());
        };
        // The screen keeps the last picture: only changes are sent (plus
        // one every few seconds in case it was reset).
        let fingerprint = picture.fingerprint() ^ u64::from(frame.settings.brightness);
        if self.last == Some(fingerprint) && self.unchanged < 4 {
            self.unchanged += 1;
            return Ok(());
        }
        self.unchanged = 0;
        let data = jpeg(picture, frame.settings.brightness)?;
        if data.len() > 255 * (REPORT - HEADER) {
            return Err(io::Error::other("picture too large for the screen"));
        }
        for report in reports(&data, self.variant) {
            self.transport.write(&report)?;
        }
        self.last = Some(fingerprint);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::displays::hardware_display::DisplaySettings;
    use crate::displays::hid::mock::MockHid;
    use std::sync::Arc;
    use tuxrgb_core::signal::HardwareSample;

    #[test]
    fn jpeg_parts_carry_openlinkhub_headers() {
        let data: Vec<u8> = (0..2500u32).map(|i| i as u8).collect();
        let reports = reports(&data, Variant::Elite);
        assert_eq!(reports.len(), 3);
        assert!(reports.iter().all(|r| r.len() == 1024));
        assert_eq!(
            &reports[0][..8],
            &[0x02, 0x05, 0x00, 0x00, 0, 0, 0xF8, 0x03]
        );
        assert_eq!(
            &reports[1][..8],
            &[0x02, 0x05, 0x00, 0x00, 1, 0, 0xF8, 0x03]
        );
        // 2500 − 2 × 1016 = 468 bytes in the last part, flagged.
        assert_eq!(
            &reports[2][..8],
            &[0x02, 0x05, 0x00, 0x01, 2, 0, 0xD4, 0x01]
        );
        assert_eq!(&reports[0][8..12], &[0, 1, 2, 3]);
        assert_eq!(reports[2][8], (2032u32 % 256) as u8);
        let link = super::reports(&data, Variant::Link);
        assert_eq!(link[0][2], 0x01);
    }

    #[test]
    fn a_picture_becomes_a_jpeg_stream() {
        let (mock, log) = MockHid::silent();
        let mut lcd = CorsairLcd::new(Box::new(mock), Variant::Link);
        let mut picture = Picture::new(480, 480);
        for (i, px) in picture.rgba.chunks_exact_mut(4).enumerate() {
            px.copy_from_slice(&[(i % 255) as u8, 80, 200, 255]);
        }
        let settings = DisplaySettings::default();
        let sample = HardwareSample::default();
        let frame = Frame {
            settings: &settings,
            sample: &sample,
            metrics: &[],
            fahrenheit: false,
            picture: Some(Arc::new(picture)),
        };
        lcd.show(&frame).unwrap();
        let sent = log.take();
        assert!(!sent.is_empty());
        // Put back together, the parts are a JPEG.
        let mut jpeg = Vec::new();
        for r in &sent {
            let n = u16::from_le_bytes([r[6], r[7]]) as usize;
            jpeg.extend_from_slice(&r[8..8 + n]);
        }
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        assert_eq!(&jpeg[jpeg.len() - 2..], &[0xFF, 0xD9]);
        assert_eq!(sent.last().unwrap()[3], 1);
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (480, 480));
    }
}
