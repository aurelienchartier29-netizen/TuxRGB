//! NZXT Kraken Z (Z53 / Z63 / Z73), Kraken 2023 / 2023 Elite and Kraken
//! 2024 Elite RGB / Plus: the LCD screen on the pump.
//!
//! Commands and the picture upload follow liquidctl's `kraken3` driver
//! (GPL-3.0), re-implemented here: 64-byte HID reports for the commands
//! (brightness, orientation, screen mode, memory "buckets"), and the
//! pictures on a USB bulk endpoint (Linux; on Windows that endpoint has no
//! driver, the Kraken keeps its own screens there).

use super::hardware_display::{Capabilities, Family, Frame, HardwareDisplay, Model, Picture};
use super::hid::{HidInfo, Transport};
use super::usb::BulkOut;
use std::io;
use std::time::{Duration, Instant};

const VID: u16 = 0x1e71;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// Kraken Z53 / Z63 / Z73: 320 × 320, 512-byte bulk transfers.
    Z3,
    /// Kraken 2023 (240 × 240); firmware 2.x takes RGB565 pictures.
    K2023,
    /// Kraken 2023 Elite (640 × 640): pictures not reliable (liquidctl
    /// marks it broken), own screens only.
    K2023Elite,
    /// Kraken 2024 Elite RGB (640 × 640).
    K2024Elite,
    /// Kraken 2024 Plus (240 × 240).
    K2024Plus,
}

impl Variant {
    pub fn resolution(self) -> u32 {
        match self {
            Self::Z3 => 320,
            Self::K2023 | Self::K2024Plus => 240,
            Self::K2023Elite | Self::K2024Elite => 640,
        }
    }

    /// Bytes per bulk write.
    fn bulk_chunk(self) -> usize {
        match self {
            Self::Z3 => 512,
            _ => 2 * 1024 * 1024,
        }
    }

    /// Pictures can be sent.
    fn pictures(self) -> bool {
        self != Self::K2023Elite
    }
}

const fn model(pid: u16, name: &'static str, variant: Variant) -> Model {
    Model {
        vendor_id: VID,
        product_id: pid,
        vendor: "NZXT",
        name,
        family: Family::Kraken(variant),
    }
}

pub const MODELS: &[Model] = &[
    model(0x3008, "Kraken Z (Z53 / Z63 / Z73)", Variant::Z3),
    model(0x300c, "Kraken 2023 Elite", Variant::K2023Elite),
    model(0x300e, "Kraken 2023", Variant::K2023),
    model(0x3012, "Kraken 2024 Elite RGB", Variant::K2024Elite),
    model(0x3014, "Kraken 2024 Plus", Variant::K2024Plus),
];

pub fn capabilities(v: Variant) -> Capabilities {
    let side = v.resolution();
    let mut c = Capabilities::screen(side, side, true);
    c.builtin = Some("Température du liquide");
    // Every picture is a transfer of up to 1.6 MB: not more than this.
    c.min_interval = Duration::from_millis(2000);
    let pictures = v.pictures() && cfg!(target_os = "linux");
    if !pictures {
        c.text = false;
        c.image = false;
        c.metrics = &[];
        c.at_once = 0;
        c.note = Some(if v.pictures() {
            "Sous Windows, l'écran garde ses propres affichages (température du liquide) : envoyer des images demande un pilote USB que NZXT CAM installe."
        } else {
            "Ce modèle n'accepte pas encore d'images fiables : l'écran garde ses propres affichages."
        });
    }
    c
}

/// Magic start of every bulk upload.
const BULK_MAGIC: [u8; 12] = [
    0x12, 0xFA, 0x01, 0xE8, 0xAB, 0xCD, 0xEF, 0x98, 0x76, 0x54, 0x32, 0x10,
];
/// Screen memory, KiB.
const LCD_TOTAL_MEMORY: u32 = 24_320;
const REPORT: usize = 64;
const READ_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_READS: usize = 12;

/// The picture as the screen wants it: RGB plus a zero byte per pixel.
pub fn rgba0(p: &Picture) -> Vec<u8> {
    let mut out = Vec::with_capacity(p.rgba.len());
    for px in p.rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[0], px[1], px[2], 0]);
    }
    out
}

/// RGB565, big-endian (Kraken 2023, firmware 2.x).
pub fn rgb565(p: &Picture) -> Vec<u8> {
    let mut out = Vec::with_capacity(p.rgba.len() / 2);
    for px in p.rgba.chunks_exact(4) {
        let (r, g, b) = (px[0] >> 3, px[1] >> 2, px[2] >> 3);
        out.push((r << 3) | (g >> 3));
        out.push(((g & 0x7) << 5) | b);
    }
    out
}

/// A Kraken with a screen.
pub struct Kraken {
    hid: Box<dyn Transport>,
    bulk: Option<Box<dyn BulkOut>>,
    variant: Variant,
    caps: Capabilities,
    firmware: Option<(u8, u8, u8)>,
    /// Applied brightness / orientation (index 0–3).
    applied: Option<(u8, u8)>,
    /// What the screen shows now.
    showing: Showing,
    /// The 2.x firmware wants its first picture twice.
    fw2_primed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Showing {
    Unknown,
    Builtin,
    /// Fingerprint of the picture, and when it was sent.
    Picture(u64),
}

impl Kraken {
    pub fn open(hid: Box<dyn Transport>, variant: Variant, info: &HidInfo) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        let bulk: Option<Box<dyn BulkOut>> = if variant.pictures() {
            info.usb_sysfs.as_deref().and_then(|usb| {
                match super::usb::UsbBulk::open(usb, 0x02, 16 * 1024) {
                    Ok(b) => Some(Box::new(b) as Box<dyn BulkOut>),
                    Err(e) => {
                        tracing::warn!("Kraken: no access to the picture endpoint: {e}");
                        None
                    }
                }
            })
        } else {
            None
        };
        #[cfg(not(target_os = "linux"))]
        let bulk: Option<Box<dyn BulkOut>> = {
            let _ = info;
            None
        };
        Ok(Self::with(hid, bulk, variant))
    }

    pub fn with(hid: Box<dyn Transport>, bulk: Option<Box<dyn BulkOut>>, variant: Variant) -> Self {
        let mut caps = capabilities(variant);
        if bulk.is_none() && caps.image {
            caps.text = false;
            caps.image = false;
            caps.metrics = &[];
            caps.at_once = 0;
            caps.note = Some(
                "Pas d'accès à l'envoi d'images (droits USB) : l'écran garde ses propres affichages.",
            );
        }
        Self {
            hid,
            bulk,
            variant,
            caps,
            firmware: None,
            applied: None,
            showing: Showing::Unknown,
            fw2_primed: false,
        }
    }

    fn write(&mut self, cmd: &[u8]) -> io::Result<()> {
        let mut report = cmd.to_vec();
        report.resize(REPORT, 0);
        self.hid.write(&report)
    }

    /// Reads reports until one starts with `prefix` (others – status
    /// reports – are skipped); the first one read if none does.
    fn read_until(&mut self, prefix: &[u8]) -> io::Result<Vec<u8>> {
        let mut first: Option<Vec<u8>> = None;
        let mut buf = [0u8; REPORT];
        for _ in 0..MAX_READS {
            let n = self.hid.read(&mut buf, READ_TIMEOUT)?;
            if n == 0 {
                break;
            }
            let msg = buf[..n].to_vec();
            if msg.starts_with(prefix) {
                return Ok(msg);
            }
            first.get_or_insert(msg);
        }
        first.ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "no answer from the Kraken"))
    }

    /// A command and its answer: the reply of command `0xNN` starts with
    /// `0xNN + 1` (only the first byte is checked: the second one is not
    /// always the command's).
    fn command(&mut self, cmd: &[u8]) -> io::Result<Vec<u8>> {
        self.write(cmd)?;
        self.read_until(&[cmd[0] | 1])
    }

    fn ok(reply: &[u8]) -> bool {
        reply.get(14) == Some(&1)
    }

    fn firmware(&mut self) -> io::Result<(u8, u8, u8)> {
        if let Some(fw) = self.firmware {
            return Ok(fw);
        }
        let r = self.command(&[0x10, 0x01])?;
        let fw = (
            *r.get(0x11).unwrap_or(&0),
            *r.get(0x12).unwrap_or(&0),
            *r.get(0x13).unwrap_or(&0),
        );
        self.firmware = Some(fw);
        Ok(fw)
    }

    /// Brightness (%) and orientation (quarter turns), only when changed.
    fn apply_screen(&mut self, brightness: u8, rotation: u16) -> io::Result<()> {
        let wanted = (brightness.min(100), ((rotation / 90) % 4) as u8);
        if self.applied == Some(wanted) {
            return Ok(());
        }
        // liquidctl reads the current values first; the answer is not needed.
        self.write(&[0x30, 0x01])?;
        let _ = self.read_until(&[0x31, 0x01]);
        self.write(&[0x30, 0x02, 0x01, wanted.0, 0x0, 0x0, 0x1, wanted.1])?;
        self.applied = Some(wanted);
        Ok(())
    }

    /// The Kraken's own liquid temperature screen.
    fn builtin(&mut self) -> io::Result<()> {
        self.command(&[0x38, 0x1, 0x2, 0x0])?;
        self.showing = Showing::Builtin;
        Ok(())
    }

    fn bulk_write(&mut self, data: &[u8]) -> io::Result<()> {
        let chunk = self.variant.bulk_chunk();
        let Some(bulk) = self.bulk.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "no bulk endpoint",
            ));
        };
        for part in data.chunks(chunk) {
            bulk.write(part)?;
        }
        Ok(())
    }

    /// Sends a static picture (liquidctl's `_send_data`).
    fn send_rgba(&mut self, data: &[u8]) -> io::Result<()> {
        self.command(&[0x36, 0x03])?;
        let mut buckets = Vec::with_capacity(16);
        for b in 0..16u8 {
            buckets.push(self.command(&[0x30, 0x04, b])?);
        }
        let free = buckets
            .iter()
            .position(|r| r.iter().skip(15).all(|b| *b == 0));
        let mut index = self.prepare_bucket(free.unwrap_or(0) as u8, free.is_none())?;
        let mut header = BULK_MAGIC.to_vec();
        header.extend_from_slice(&[0x02, 0, 0, 0]);
        header.extend_from_slice(&(data.len() as u32).to_le_bytes());
        let size_kb = (header.len() + data.len()).div_ceil(1024) as u32;
        let start = match memory_offset(&buckets, index as usize, size_kb) {
            Some(start) => start,
            None => {
                self.delete_all()?;
                index = 0;
                0
            }
        };
        let mut setup = vec![0x32, 0x1, index, index + 1];
        setup.extend_from_slice(&(start as u16).to_le_bytes());
        setup.extend_from_slice(&(size_kb as u16).to_le_bytes());
        setup.push(0x1);
        if !Self::ok(&self.command(&setup)?) {
            tracing::warn!("Kraken: memory bucket setup refused");
        }
        self.command(&[0x36, 0x01, index])?;
        self.bulk_write(&header)?;
        self.bulk_write(data)?;
        self.write(&[0x36, 0x02])?;
        if !Self::ok(&self.command(&[0x38, 0x1, 0x4, index])?) {
            tracing::warn!("Kraken: switching to the new picture refused");
        }
        Ok(())
    }

    /// Deletes a bucket before writing it; the next one if it refuses.
    fn prepare_bucket(&mut self, index: u8, filled: bool) -> io::Result<u8> {
        let mut index = index;
        let mut filled = filled;
        loop {
            if index >= 16 {
                return Err(io::Error::other("no free memory bucket on the Kraken"));
            }
            let r = self.command(&[0x32, 0x2, index])?;
            if !Self::ok(&r) {
                index += 1;
                filled = true;
                continue;
            }
            if filled {
                filled = false;
                continue;
            }
            return Ok(index);
        }
    }

    fn delete_all(&mut self) -> io::Result<()> {
        self.command(&[0x38, 0x1, 0x2, 0x0])?;
        for b in 0..16u8 {
            self.command(&[0x32, 0x2, b])?;
        }
        Ok(())
    }

    /// Sends a picture to a Kraken 2023 with firmware 2.x (RGB565).
    fn send_rgb565(&mut self, data: &[u8]) -> io::Result<()> {
        let times = if self.fw2_primed { 1 } else { 2 };
        for _ in 0..times {
            self.command(&[0x36, 0x01, 0x00, 0x01, 0x06])?;
            let mut header = BULK_MAGIC.to_vec();
            header.extend_from_slice(&[0x06, 0, 0, 0]);
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            self.bulk_write(&header)?;
            self.bulk_write(data)?;
            self.command(&[0x36, 0x02])?;
        }
        self.fw2_primed = true;
        Ok(())
    }

    fn show_picture(&mut self, picture: &Picture) -> io::Result<()> {
        let fingerprint = picture.fingerprint();
        if self.showing == Showing::Picture(fingerprint) {
            return Ok(());
        }
        if self.variant == Variant::K2023 && self.firmware()?.0 == 2 {
            self.send_rgb565(&rgb565(picture))?;
        } else {
            self.send_rgba(&rgba0(picture))?;
        }
        self.showing = Showing::Picture(fingerprint);
        Ok(())
    }
}

/// Where the picture goes in the screen memory (KiB), liquidctl's
/// `_get_bucket_memory_offset`; `None`: start over from an empty memory.
pub fn memory_offset(buckets: &[Vec<u8>], index: usize, size_kb: u32) -> Option<u32> {
    let word = |r: &Vec<u8>, i: usize| -> u32 {
        u32::from(*r.get(i).unwrap_or(&0)) | (u32::from(*r.get(i + 1).unwrap_or(&0)) << 8)
    };
    let current = buckets.get(index)?;
    let offset = word(current, 17);
    let size = word(current, 19);
    if size_kb <= size {
        return Some(offset);
    }
    let mut max_used = 0;
    let mut min_used = offset;
    let mut overlap = false;
    for (bi, b) in buckets.iter().enumerate() {
        let start = word(b, 17);
        let end = start + word(b, 19);
        max_used = max_used.max(end);
        min_used = min_used.min(start);
        if (start > offset && start < offset + size_kb)
            || (start < offset && end > start)
            || (start == offset && bi != index)
        {
            overlap = true;
        }
    }
    if !overlap {
        return Some(offset);
    }
    if max_used + size_kb < LCD_TOTAL_MEMORY {
        return Some(max_used);
    }
    if size_kb < min_used {
        return Some(0);
    }
    None
}

impl HardwareDisplay for Kraken {
    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn show(&mut self, frame: &Frame<'_>) -> io::Result<()> {
        let s = frame.settings;
        self.hid.flush_input();
        self.apply_screen(s.brightness, s.rotation)?;
        let picture = frame
            .picture
            .as_deref()
            .filter(|_| self.caps.image && s.content != super::hardware_display::Content::Builtin);
        match picture {
            Some(p) => {
                let started = Instant::now();
                self.show_picture(p)?;
                tracing::debug!(
                    "Kraken picture sent in {} ms",
                    started.elapsed().as_millis()
                );
            }
            None if self.showing != Showing::Builtin => self.builtin()?,
            None => {}
        }
        Ok(())
    }

    fn release(&mut self) -> io::Result<()> {
        // Back to the liquid temperature: it stays right with no program.
        if self.showing != Showing::Builtin {
            self.hid.flush_input();
            self.builtin()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use crate::displays::hardware_display::Content;
    use crate::displays::hardware_display::DisplaySettings;
    use crate::displays::hid::mock::MockHid;
    #[cfg(target_os = "linux")]
    use crate::displays::usb::MockBulk;
    #[cfg(target_os = "linux")]
    use std::sync::{Arc, Mutex};
    use tuxrgb_core::signal::HardwareSample;

    /// Answers like liquidctl's `MockKraken` (tests/test_kraken3.py), with
    /// the reply prefixes a real Kraken uses.
    fn kraken_mock() -> (MockHid, crate::displays::hid::mock::Log) {
        MockHid::new(Box::new(|data: &[u8]| {
            let mut reply = vec![0u8; 64];
            reply[0] = data[0] | 1;
            reply[1] = data[1];
            match (data[0], data[1]) {
                (0x30, 0x01) => {
                    reply[0x18] = 50;
                    reply[0x1A] = 0;
                }
                (0x32, 0x1) | (0x32, 0x2) | (0x38, 0x1) => reply[14] = 0x1,
                (0x30, 0x04) => {
                    // Empty bucket: nothing from byte 15 on (prefix kept).
                }
                (0x36, 0x02) => return Vec::new(),
                (0x30, 0x02) => return Vec::new(),
                _ => {}
            }
            vec![reply]
        }))
    }

    fn padded(cmd: &[u8]) -> Vec<u8> {
        let mut v = cmd.to_vec();
        v.resize(64, 0);
        v
    }

    /// The Kraken Z static picture of liquidctl's test suite
    /// (tests/test_krakenz3_response.py, "static_hid" / "static_bulk"):
    /// same HID commands, same header, 800 bulk writes of 512 bytes.
    /// (Pictures are Linux-only: Windows keeps the screen's own views.)
    #[cfg(target_os = "linux")]
    #[test]
    fn static_picture_matches_liquidctl() {
        let (mock, log) = kraken_mock();
        let bulk = Arc::new(Mutex::new(Vec::new()));
        let mut k = Kraken::with(
            Box::new(mock),
            Some(Box::new(MockBulk(bulk.clone()))),
            Variant::Z3,
        );
        // liquidctl's yellow.jpg, decoded: (255, 255, 1).
        let mut picture = Picture::new(320, 320);
        for px in picture.rgba.chunks_exact_mut(4) {
            px.copy_from_slice(&[255, 255, 1, 255]);
        }
        let settings = DisplaySettings {
            content: Content::Image,
            brightness: 50,
            ..DisplaySettings::default()
        };
        let sample = HardwareSample::default();
        let frame = Frame {
            settings: &settings,
            sample: &sample,
            metrics: &[],
            fahrenheit: false,
            picture: Some(Arc::new(picture)),
        };
        k.show(&frame).unwrap();
        let sent = log.take();
        // Brightness 50 / orientation 0 are applied first (the mock says
        // so already: liquidctl skips it, the setting is simply re-sent).
        assert_eq!(sent[0], padded(&[0x30, 0x01]));
        assert_eq!(sent[1], padded(&[0x30, 0x02, 0x01, 50, 0, 0, 1, 0]));
        let mut expected: Vec<Vec<u8>> = vec![padded(&[0x36, 0x03])];
        for b in 0..16u8 {
            expected.push(padded(&[0x30, 0x04, b]));
        }
        for cmd in [
            &[50u8, 2, 0][..],
            &[50, 1, 0, 1, 0, 0, 145, 1, 1],
            &[54, 1, 0],
            &[54, 2],
            &[56, 1, 4, 0],
        ] {
            expected.push(padded(cmd));
        }
        assert_eq!(&sent[2..], &expected[..]);
        let bulk = bulk.lock().unwrap();
        assert_eq!(bulk.len(), 801);
        assert_eq!(
            bulk[0],
            vec![
                18, 250, 1, 232, 171, 205, 239, 152, 118, 84, 50, 16, 2, 0, 0, 0, 0, 64, 6, 0
            ]
        );
        let chunk: Vec<u8> = [255u8, 255, 1, 0].repeat(128);
        assert!(bulk[1..].iter().all(|c| *c == chunk));
        drop(bulk);
        // The same picture again: nothing to send.
        k.show(&frame).unwrap();
        assert!(log.take().is_empty());
        // Its own screen again (liquidctl's "liquid_hid").
        let builtin = DisplaySettings {
            content: Content::Builtin,
            ..settings.clone()
        };
        let frame = Frame {
            settings: &builtin,
            picture: None,
            ..frame
        };
        k.show(&frame).unwrap();
        assert_eq!(log.take(), vec![padded(&[56, 1, 2, 0])]);
    }

    #[test]
    fn brightness_and_orientation_like_liquidctl() {
        let (mock, log) = kraken_mock();
        let mut k = Kraken::with(Box::new(mock), None, Variant::Z3);
        k.apply_screen(60, 0).unwrap();
        assert_eq!(
            log.take(),
            vec![padded(&[48, 1]), padded(&[48, 2, 1, 60, 0, 0, 1, 0])]
        );
        k.apply_screen(60, 90).unwrap();
        assert_eq!(log.take()[1], padded(&[48, 2, 1, 60, 0, 0, 1, 1]));
        // Unchanged: nothing sent.
        k.apply_screen(60, 90).unwrap();
        assert!(log.take().is_empty());
    }

    #[test]
    fn memory_offsets_follow_liquidctl() {
        let bucket = |start: u16, size: u16| {
            let mut r = vec![0u8; 64];
            r[17..19].copy_from_slice(&start.to_le_bytes());
            r[19..21].copy_from_slice(&size.to_le_bytes());
            r
        };
        // Fits in the bucket's current space.
        let b = vec![bucket(100, 500), bucket(0, 0)];
        assert_eq!(memory_offset(&b, 0, 401), Some(100));
        // Grows over another one: after the last used kilobyte.
        let b = vec![bucket(0, 300), bucket(300, 300)];
        assert_eq!(memory_offset(&b, 0, 401), Some(600));
        // Memory full: start again from zero.
        let b = vec![bucket(0, 300), bucket(300, 24_000)];
        assert_eq!(memory_offset(&b, 0, 401), None);
    }

    #[test]
    fn rgb565_is_big_endian() {
        let mut p = Picture::new(1, 1);
        p.rgba.copy_from_slice(&[255, 0, 0, 255]);
        assert_eq!(rgb565(&p), vec![0xF8, 0x00]);
        p.rgba.copy_from_slice(&[0, 255, 0, 255]);
        assert_eq!(rgb565(&p), vec![0x07, 0xE0]);
        p.rgba.copy_from_slice(&[0, 0, 255, 255]);
        assert_eq!(rgb565(&p), vec![0x00, 0x1F]);
    }

    #[test]
    fn without_the_bulk_endpoint_the_screen_keeps_its_own() {
        let (mock, log) = kraken_mock();
        let mut k = Kraken::with(Box::new(mock), None, Variant::Z3);
        assert!(!k.capabilities().image);
        assert!(k.capabilities().note.is_some());
        let settings = DisplaySettings::default();
        let sample = HardwareSample::default();
        let frame = Frame {
            settings: &settings,
            sample: &sample,
            metrics: &[],
            fahrenheit: false,
            picture: None,
        };
        k.show(&frame).unwrap();
        assert_eq!(log.take().last().unwrap(), &padded(&[56, 1, 2, 0]));
    }
}
