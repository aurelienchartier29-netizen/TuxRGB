//! Developer tool: `tuxrgb --screenshot-tour <dir>` visits every page and
//! saves a PNG of each (README/Flathub screenshots, visual regression checks).

use std::io::Write;
use std::path::Path;

/// Pages visited, in order (`TUXRGB_TOUR_MOTION=1`: [`MOTION_STOPS`]).
pub fn stops() -> &'static [&'static str] {
    if std::env::var_os("TUXRGB_TOUR_MOTION").is_some() {
        MOTION_STOPS
    } else {
        STOPS
    }
}

/// Animation checks: `m-…` stops trigger a change and are captured after
/// `motion_delay` milliseconds, in the middle of the animation.
pub const MOTION_STOPS: &[&str] = &[
    "home",
    "m-page-out",
    "m-page-in",
    "m-picker-open",
    "m-effect-swap",
    "m-dialog-in",
    "m-dialog-open",
    "m-dialog-out",
    "m-snackbar",
    "m-wizard",
    "m-wizard-late",
    "m-page-out2",
    "m-page-in2",
];

/// Delay before the capture of a motion stop.
pub fn motion_delay(stop: &str) -> u64 {
    match stop {
        "m-page-out" => 40,
        "m-page-in" => 150,
        "m-picker-open" => 110,
        "m-effect-swap" => 60,
        "m-dialog-in" => 60,
        "m-dialog-open" => 900,
        "m-dialog-out" => 150,
        "m-snackbar" => 120,
        "m-wizard" => 30,
        "m-wizard-late" => 250,
        "m-page-out2" => 30,
        "m-page-in2" => 250,
        _ => 0,
    }
}

pub const STOPS: &[&str] = &[
    "onboarding",
    "home",
    "effects",
    "effects-picker",
    "effects-palette",
    "effects-monitor",
    "devices",
    "devices-zones",
    "devices-fans",
    "devices-own",
    "devices-hardware",
    "displays",
    "settings",
    "settings-led",
    "settings-network",
    "settings-updates",
    "credits",
];

/// `TUXRGB_DEV_MINIMIZE_AFTER=<seconds>`: minimises the window that long
/// after it opened (developer check: CPU use of a minimised window).
pub fn dev_minimize_after() -> Option<u64> {
    std::env::var("TUXRGB_DEV_MINIMIZE_AFTER")
        .ok()?
        .parse()
        .ok()
}

/// `TUXRGB_DEV_CLOSE_AFTER=<seconds>`: closes the window that long after it
/// opened, like its close button (developer check: memory once in the tray).
pub fn dev_close_after() -> Option<u64> {
    std::env::var("TUXRGB_DEV_CLOSE_AFTER").ok()?.parse().ok()
}

/// Minimal PNG encoder (stored deflate blocks) – no image crate needed.
pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    fn crc32(data: &[u8]) -> u32 {
        let mut table = [0u32; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *entry = c;
        }
        let mut crc = 0xFFFF_FFFFu32;
        for b in data {
            crc = table[((crc ^ *b as u32) & 0xFF) as usize] ^ (crc >> 8);
        }
        crc ^ 0xFFFF_FFFF
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    // Raw scanlines with filter byte 0.
    let stride = width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * stride..(y + 1) * stride]);
    }
    // zlib stream made of stored blocks.
    let mut z = vec![0x78, 0x01];
    for (i, block) in raw.chunks(65_535).enumerate() {
        let last = (i + 1) * 65_535 >= raw.len();
        z.push(last as u8);
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + *byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    std::fs::File::create(path)?.write_all(&png)
}
