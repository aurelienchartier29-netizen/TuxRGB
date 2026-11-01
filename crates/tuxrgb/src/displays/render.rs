//! Pictures of the hardware displays: what the full-colour screens show
//! (sensor gauges, a text, a picture), and the previews of the digital
//! displays and dot matrices for the settings page.
//!
//! Drawn in software (tiny-skia shapes, Roboto Flex glyphs through
//! ab_glyph): the same picture on Linux and Windows, no GPU involved.

use super::hardware_display::{Content, DisplaySettings, Metric, Picture};
use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, Paint, PathBuilder, Pixmap, Point, RadialGradient,
    Rect, SpreadMode, Stroke, Transform,
};
use tuxrgb_core::Rgb;

const BOLD: &[u8] = include_bytes!("../../assets/fonts/RobotoFlex-DisplayBold.ttf");
const TEXT: &[u8] = include_bytes!("../../assets/fonts/RobotoFlex-Text.ttf");

fn bold() -> &'static FontRef<'static> {
    static F: OnceLock<FontRef<'static>> = OnceLock::new();
    F.get_or_init(|| FontRef::try_from_slice(BOLD).expect("embedded font"))
}

fn regular() -> &'static FontRef<'static> {
    static F: OnceLock<FontRef<'static>> = OnceLock::new();
    F.get_or_init(|| FontRef::try_from_slice(TEXT).expect("embedded font"))
}

/// Colours of the screens (always dark: they glow in a dark case).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub accent: Rgb,
}

const BACKGROUND: Rgb = Rgb::new(7, 9, 13);
const LABEL: Rgb = Rgb::new(150, 160, 175);
const VALUE: Rgb = Rgb::new(245, 247, 250);
const AMBER: Rgb = Rgb::new(255, 176, 32);
const RED: Rgb = Rgb::new(255, 72, 72);

fn color(c: Rgb, a: f32) -> Color {
    Color::from_rgba8(c.r, c.g, c.b, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Rgb::new(l(a.r, b.r), l(a.g, b.g), l(a.b, b.b))
}

/// Gauge colour of a value: the accent, warming up for hot temperatures
/// and high loads.
fn value_color(metric: Metric, value: Option<f32>, accent: Rgb) -> Rgb {
    let Some(v) = value else { return accent };
    let (warm, hot) = match metric {
        Metric::CpuTemp | Metric::GpuTemp => (65.0, 85.0),
        Metric::CpuLoad | Metric::GpuLoad | Metric::Ram => (75.0, 95.0),
        _ => return accent,
    };
    if v < warm {
        accent
    } else if v < hot {
        mix(accent, AMBER, (v - warm) / (hot - warm))
    } else {
        mix(AMBER, RED, ((v - hot) / 10.0).min(1.0))
    }
}

/// Text alignment around the anchor.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Center,
    Right,
}

fn text_width(font: &FontRef<'_>, px: f32, text: &str) -> f32 {
    let scaled = font.as_scaled(PxScale::from(px));
    let mut w = 0.0;
    let mut prev = None;
    for c in text.chars() {
        let id = font.glyph_id(c);
        if let Some(p) = prev {
            w += scaled.kern(p, id);
        }
        w += scaled.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Draws `text` with its baseline at `y`.
#[allow(clippy::too_many_arguments)]
fn draw_text(
    pm: &mut Pixmap,
    font: &FontRef<'_>,
    px: f32,
    x: f32,
    y: f32,
    text: &str,
    c: Rgb,
    alpha: f32,
    align: Align,
) {
    let scaled = font.as_scaled(PxScale::from(px));
    let width = text_width(font, px, text);
    let mut pen = match align {
        Align::Left => x,
        Align::Center => x - width * 0.5,
        Align::Right => x - width,
    };
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let mut prev = None;
    for ch in text.chars() {
        let id = font.glyph_id(ch);
        if let Some(p) = prev {
            pen += scaled.kern(p, id);
        }
        let glyph = id.with_scale_and_position(PxScale::from(px), point(pen, y));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let b = outlined.px_bounds();
            outlined.draw(|gx, gy, cov| {
                let px_x = b.min.x as i32 + gx as i32;
                let px_y = b.min.y as i32 + gy as i32;
                if px_x < 0 || px_y < 0 || px_x >= w || px_y >= h {
                    return;
                }
                let a = (cov * alpha).clamp(0.0, 1.0);
                if a <= 0.0 {
                    return;
                }
                let i = ((px_y * w + px_x) * 4) as usize;
                // Premultiplied "source over".
                for (k, v) in [c.r, c.g, c.b].iter().enumerate() {
                    let dst = f32::from(data[i + k]);
                    data[i + k] = (f32::from(*v) * a + dst * (1.0 - a)).round() as u8;
                }
                let dst_a = f32::from(data[i + 3]);
                data[i + 3] = (255.0 * a + dst_a * (1.0 - a)).round() as u8;
            });
        }
        pen += scaled.h_advance(id);
        prev = Some(id);
    }
}

/// Largest size (px) at which `text` fits in `max_width`, up to `px`.
fn fit(font: &FontRef<'_>, px: f32, text: &str, max_width: f32) -> f32 {
    let w = text_width(font, px, text);
    if w <= max_width || w <= 0.0 {
        px
    } else {
        (px * max_width / w).max(6.0)
    }
}

/// Arc of a circle (degrees, clockwise from 3 o'clock), as a polyline.
fn arc(cx: f32, cy: f32, r: f32, from: f32, to: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let steps = (((to - from).abs() / 2.0).ceil() as usize).max(2);
    for i in 0..=steps {
        let a = (from + (to - from) * i as f32 / steps as f32).to_radians();
        let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.finish()
}

fn stroke(pm: &mut Pixmap, path: &tiny_skia::Path, c: Color, width: f32) {
    let mut paint = Paint::default();
    paint.set_color(c);
    paint.anti_alias = true;
    let s = Stroke {
        width,
        line_cap: LineCap::Round,
        ..Stroke::default()
    };
    pm.stroke_path(path, &paint, &s, Transform::identity(), None);
}

fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: Color) {
    if let Some(rect) = Rect::from_xywh(x, y, w.max(0.5), h.max(0.5)) {
        let mut paint = Paint::default();
        paint.set_color(c);
        paint.anti_alias = true;
        pm.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

fn round_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w * 0.5).min(h * 0.5);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

fn fill_path(pm: &mut Pixmap, path: &tiny_skia::Path, c: Color) {
    let mut paint = Paint::default();
    paint.set_color(c);
    paint.anti_alias = true;
    pm.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
}

/// Dark background with a soft glow of the accent.
fn background(pm: &mut Pixmap, accent: Rgb) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    pm.fill(color(BACKGROUND, 1.0));
    if let Some(shader) = RadialGradient::new(
        Point::from_xy(w * 0.5, h * 0.42),
        Point::from_xy(w * 0.5, h * 0.42),
        w.max(h) * 0.62,
        vec![
            GradientStop::new(0.0, color(accent, 0.20)),
            GradientStop::new(0.55, color(accent, 0.06)),
            GradientStop::new(1.0, color(accent, 0.0)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        if let Some(rect) = Rect::from_xywh(0.0, 0.0, w, h) {
            pm.fill_rect(rect, &paint, Transform::identity(), None);
        }
    }
}

/// Start and sweep (degrees) of the gauges: three quarters of a circle,
/// open at the bottom.
const GAUGE_FROM: f32 = 135.0;
const GAUGE_SWEEP: f32 = 270.0;

fn gauge(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, width: f32, fraction: Option<f32>, c: Rgb) {
    if let Some(track) = arc(cx, cy, r, GAUGE_FROM, GAUGE_FROM + GAUGE_SWEEP) {
        stroke(pm, &track, color(Rgb::WHITE, 0.10), width);
    }
    if let Some(f) = fraction
        && f > 0.003
        && let Some(value) = arc(cx, cy, r, GAUGE_FROM, GAUGE_FROM + GAUGE_SWEEP * f.min(1.0))
    {
        // Glow under the value.
        stroke(pm, &value, color(c, 0.22), width * 1.9);
        stroke(pm, &value, color(c, 1.0), width);
    }
}

/// Values shown now, with what the screen needs to draw them.
pub struct Shown {
    pub metric: Metric,
    pub value: Option<f32>,
}

fn fraction(s: &Shown) -> Option<f32> {
    s.value.map(|v| (v / s.metric.range()).clamp(0.0, 1.0))
}

/// "14:32" / its date, local time.
pub fn clock() -> (String, String) {
    let (h, m, day, month) = local_time();
    (format!("{h:02}:{m:02}"), format!("{day:02}/{month:02}"))
}

#[cfg(unix)]
fn local_time() -> (u32, u32, u32, u32) {
    // SAFETY: localtime_r with valid pointers.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        (
            tm.tm_hour as u32,
            tm.tm_min as u32,
            tm.tm_mday as u32,
            tm.tm_mon as u32 + 1,
        )
    }
}

#[cfg(windows)]
fn local_time() -> (u32, u32, u32, u32) {
    // SAFETY: plain call returning a struct.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    (
        u32::from(t.wHour),
        u32::from(t.wMinute),
        u32::from(t.wDay),
        u32::from(t.wMonth),
    )
}

#[cfg(not(any(unix, windows)))]
fn local_time() -> (u32, u32, u32, u32) {
    (0, 0, 1, 1)
}

/// Label of a value on the screens: "CPU · %"-style captions are too
/// terse, "Température" alone is ambiguous: "CPU" plus the kind.
fn caption(m: Metric) -> String {
    let kind = match m {
        Metric::CpuTemp | Metric::GpuTemp => crate::i18n::t("Temp."),
        Metric::CpuLoad | Metric::GpuLoad => crate::i18n::t("Charge"),
        Metric::CpuPower | Metric::GpuPower => crate::i18n::t("Puissance"),
        Metric::CpuFreq | Metric::GpuFreq => crate::i18n::t("Fréquence"),
        Metric::Ram => crate::i18n::t("Mémoire"),
        Metric::Time => "",
    };
    match m.caption() {
        "" => kind.to_owned(),
        c if kind.is_empty() => c.to_owned(),
        c => format!("{c} · {kind}"),
    }
}

/// One value with its gauge filling the whole screen.
fn one_value(pm: &mut Pixmap, s: &Shown, fahrenheit: bool, accent: Rgb) {
    let side = pm.width().min(pm.height()) as f32;
    let (cx, cy) = (pm.width() as f32 * 0.5, pm.height() as f32 * 0.5);
    if s.metric == Metric::Time {
        let (time, date) = clock();
        if let Some(ring) = arc(cx, cy, side * 0.41, 0.0, 360.0) {
            stroke(pm, &ring, color(accent, 0.55), side * 0.018);
        }
        let px = fit(bold(), side * 0.27, &time, side * 0.66);
        draw_text(
            pm,
            bold(),
            px,
            cx,
            cy + px * 0.36,
            &time,
            VALUE,
            1.0,
            Align::Center,
        );
        draw_text(
            pm,
            regular(),
            side * 0.075,
            cx,
            cy + side * 0.25,
            &date,
            LABEL,
            1.0,
            Align::Center,
        );
        return;
    }
    let c = value_color(s.metric, s.value, accent);
    gauge(pm, cx, cy, side * 0.40, side * 0.055, fraction(s), c);
    let (value, unit) = s.metric.format(s.value, fahrenheit);
    let px = fit(bold(), side * 0.30, &value, side * 0.52);
    draw_text(
        pm,
        bold(),
        px,
        cx,
        cy + px * 0.30,
        &value,
        VALUE,
        1.0,
        Align::Center,
    );
    draw_text(
        pm,
        regular(),
        side * 0.085,
        cx,
        cy - px * 0.48,
        &caption(s.metric),
        LABEL,
        1.0,
        Align::Center,
    );
    draw_text(
        pm,
        bold(),
        side * 0.09,
        cx,
        cy + side * 0.27,
        unit,
        c,
        1.0,
        Align::Center,
    );
}

/// Two values on concentric gauges.
fn two_values(pm: &mut Pixmap, a: &Shown, b: &Shown, fahrenheit: bool, accent: Rgb) {
    let side = pm.width().min(pm.height()) as f32;
    let (cx, cy) = (pm.width() as f32 * 0.5, pm.height() as f32 * 0.5);
    let second = mix(accent, Rgb::WHITE, 0.45);
    let ca = value_color(a.metric, a.value, accent);
    let cb = value_color(b.metric, b.value, second);
    gauge(pm, cx, cy, side * 0.42, side * 0.045, fraction(a), ca);
    gauge(pm, cx, cy, side * 0.335, side * 0.035, fraction(b), cb);
    for (s, y, px, c) in [
        (a, cy - side * 0.03, side * 0.17, ca),
        (b, cy + side * 0.21, side * 0.12, cb),
    ] {
        let (value, unit) = if s.metric == Metric::Time {
            (clock().0, "")
        } else {
            s.metric.format(s.value, fahrenheit)
        };
        let text = format!(
            "{value}{}",
            if unit.is_empty() {
                String::new()
            } else {
                format!(" {unit}")
            }
        );
        let px = fit(bold(), px, &text, side * 0.50);
        draw_text(pm, bold(), px, cx, y, &text, VALUE, 1.0, Align::Center);
        draw_text(
            pm,
            regular(),
            side * 0.058,
            cx,
            y - px * 0.92,
            &caption(s.metric),
            c,
            1.0,
            Align::Center,
        );
    }
}

/// Up to four values in tiles, inside the circle of a round screen.
fn tiles(pm: &mut Pixmap, shown: &[Shown], fahrenheit: bool, accent: Rgb) {
    let side = pm.width().min(pm.height()) as f32;
    let (cx, cy) = (pm.width() as f32 * 0.5, pm.height() as f32 * 0.5);
    let inner = side * 0.70;
    let (x0, y0) = (cx - inner * 0.5, cy - inner * 0.5);
    let gap = side * 0.03;
    let tile = (inner - gap) * 0.5;
    for (i, s) in shown.iter().take(4).enumerate() {
        let (col, row) = ((i % 2) as f32, (i / 2) as f32);
        let (x, y) = (x0 + col * (tile + gap), y0 + row * (tile + gap));
        if let Some(bg) = round_rect(x, y, tile, tile, tile * 0.18) {
            fill_path(pm, &bg, color(Rgb::WHITE, 0.06));
        }
        let c = value_color(s.metric, s.value, accent);
        let (value, unit) = if s.metric == Metric::Time {
            (clock().0, "")
        } else {
            s.metric.format(s.value, fahrenheit)
        };
        draw_text(
            pm,
            regular(),
            tile * 0.15,
            x + tile * 0.5,
            y + tile * 0.25,
            &caption(s.metric),
            LABEL,
            1.0,
            Align::Center,
        );
        let px = fit(bold(), tile * 0.36, &value, tile * 0.86);
        draw_text(
            pm,
            bold(),
            px,
            x + tile * 0.5,
            y + tile * 0.62,
            &value,
            VALUE,
            1.0,
            Align::Center,
        );
        draw_text(
            pm,
            bold(),
            tile * 0.13,
            x + tile * 0.5,
            y + tile * 0.80,
            unit,
            c,
            1.0,
            Align::Center,
        );
        // Bar at the bottom of the tile.
        if let Some(f) = fraction(s).filter(|_| s.metric != Metric::Time) {
            let (bx, by, bw) = (x + tile * 0.16, y + tile * 0.88, tile * 0.68);
            fill_rect(pm, bx, by, bw, tile * 0.035, color(Rgb::WHITE, 0.10));
            fill_rect(pm, bx, by, bw * f, tile * 0.035, color(c, 1.0));
        }
    }
}

/// Words of `text` in up to three lines that fit `width` at the largest
/// size possible.
fn layout_text(text: &str, start_px: f32, width: f32) -> (f32, Vec<String>) {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return (start_px, Vec::new());
    }
    let mut px = start_px;
    loop {
        let mut lines: Vec<String> = Vec::new();
        let mut current = String::new();
        for w in &words {
            let candidate = if current.is_empty() {
                (*w).to_owned()
            } else {
                format!("{current} {w}")
            };
            if text_width(bold(), px, &candidate) <= width || current.is_empty() {
                current = candidate;
            } else {
                lines.push(std::mem::take(&mut current));
                current = (*w).to_owned();
            }
        }
        lines.push(current);
        let fits = lines.len() <= 3 && lines.iter().all(|l| text_width(bold(), px, l) <= width);
        if fits || px < 10.0 {
            return (px, lines);
        }
        px *= 0.9;
    }
}

fn text_screen(pm: &mut Pixmap, text: &str, accent: Rgb) {
    let side = pm.width().min(pm.height()) as f32;
    let (cx, cy) = (pm.width() as f32 * 0.5, pm.height() as f32 * 0.5);
    if let Some(ring) = arc(cx, cy, side * 0.45, GAUGE_FROM, GAUGE_FROM + GAUGE_SWEEP) {
        stroke(pm, &ring, color(accent, 0.8), side * 0.016);
    }
    let text = if text.trim().is_empty() {
        "TuxRGB"
    } else {
        text
    };
    // Inside the circle of round screens, lines included.
    let (px, lines) = layout_text(text, side * 0.20, side * 0.60);
    let line_h = px * 1.12;
    let top = cy - line_h * lines.len() as f32 * 0.5 + px * 0.78;
    for (i, l) in lines.iter().enumerate() {
        draw_text(
            pm,
            bold(),
            px,
            cx,
            top + line_h * i as f32,
            l,
            VALUE,
            1.0,
            Align::Center,
        );
    }
}

/// A user's picture, decoded once.
#[derive(Default)]
pub struct ImageCache {
    source: Option<(PathBuf, Option<std::time::SystemTime>)>,
    decoded: Option<image::RgbaImage>,
    fitted: Option<((u32, u32), Picture)>,
    pub error: Option<String>,
}

impl ImageCache {
    /// The picture at `path`, covering `w × h` (centred crop).
    pub fn get(&mut self, path: &Path, w: u32, h: u32) -> Option<&Picture> {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let key = (path.to_path_buf(), mtime);
        if self.source.as_ref() != Some(&key) {
            self.source = Some(key);
            self.fitted = None;
            self.decoded = match image::open(path) {
                Ok(img) => {
                    self.error = None;
                    Some(img.to_rgba8())
                }
                Err(e) => {
                    self.error = Some(e.to_string());
                    None
                }
            };
        }
        let img = self.decoded.as_ref()?;
        if self.fitted.as_ref().map(|(size, _)| *size) != Some((w, h)) {
            let (iw, ih) = (img.width().max(1) as f32, img.height().max(1) as f32);
            let k = (w as f32 / iw).max(h as f32 / ih);
            let (sw, sh) = (
                ((iw * k).round() as u32).max(w),
                ((ih * k).round() as u32).max(h),
            );
            let scaled =
                image::imageops::resize(img, sw, sh, image::imageops::FilterType::Triangle);
            let (ox, oy) = ((sw - w) / 2, (sh - h) / 2);
            let mut p = Picture::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    let px = scaled.get_pixel(ox + x, oy + y).0;
                    // Transparent parts on the screen's black.
                    let a = u16::from(px[3]);
                    let i = ((y * w + x) * 4) as usize;
                    for (dst, src) in p.rgba[i..i + 3].iter_mut().zip(&px[..3]) {
                        *dst = ((u16::from(*src) * a + 127) / 255) as u8;
                    }
                    p.rgba[i + 3] = 255;
                }
            }
            self.fitted = Some(((w, h), p));
        }
        self.fitted.as_ref().map(|(_, p)| p)
    }
}

fn to_picture(pm: &Pixmap) -> Picture {
    let mut p = Picture::new(pm.width(), pm.height());
    // Opaque everywhere (dark background): premultiplied = straight.
    for (dst, src) in p.rgba.chunks_exact_mut(4).zip(pm.pixels()) {
        let c = src.demultiply();
        dst.copy_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    p
}

/// What a full-colour screen shows (`shown`: the values of this refresh).
pub fn screen(
    width: u32,
    height: u32,
    settings: &DisplaySettings,
    shown: &[Shown],
    fahrenheit: bool,
    look: Look,
    images: &mut ImageCache,
) -> Picture {
    if settings.content == Content::Image
        && let Some(path) = settings.image.as_deref()
        && let Some(p) = images.get(path, width, height)
    {
        return p.clone();
    }
    let Some(mut pm) = Pixmap::new(width.max(1), height.max(1)) else {
        return Picture::new(width, height);
    };
    background(&mut pm, look.accent);
    match settings.content {
        Content::Text => text_screen(&mut pm, &settings.text, look.accent),
        Content::Image => {
            // No picture (yet): say so on the screen.
            text_screen(&mut pm, crate::i18n::t("Choisissez une image"), look.accent)
        }
        _ => match shown {
            [] => text_screen(&mut pm, "TuxRGB", look.accent),
            [one] => one_value(&mut pm, one, fahrenheit, look.accent),
            [a, b] => two_values(&mut pm, a, b, fahrenheit, look.accent),
            many => tiles(&mut pm, many, fahrenheit, look.accent),
        },
    }
    to_picture(&pm)
}

/// The circle of a round screen, for the previews (outside: transparent).
pub fn round_mask(p: &mut Picture) {
    let (w, h) = (p.width as f32, p.height as f32);
    let (cx, cy, r) = (w * 0.5, h * 0.5, w.min(h) * 0.5);
    for y in 0..p.height {
        for x in 0..p.width {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let a = (r - d + 0.5).clamp(0.0, 1.0);
            if a < 1.0 {
                let i = ((y * p.width + x) * 4 + 3) as usize;
                p.rgba[i] = (f32::from(p.rgba[i]) * a) as u8;
            }
        }
    }
}

/// Preview of a seven-segment display: the values in the accent colour on
/// a dark panel, with the load bar.
pub fn digits_preview(shown: &[Shown], load: Option<f32>, fahrenheit: bool, look: Look) -> Picture {
    let (w, h) = (360u32, 150u32);
    let Some(mut pm) = Pixmap::new(w, h) else {
        return Picture::new(w, h);
    };
    pm.fill(Color::TRANSPARENT);
    if let Some(panel) = round_rect(0.0, 0.0, w as f32, h as f32, 26.0) {
        fill_path(&mut pm, &panel, color(Rgb::new(10, 12, 16), 1.0));
    }
    let lit = look.accent;
    match shown {
        [] => {}
        [one] => {
            let (value, unit) = one.metric.format(one.value, fahrenheit);
            draw_text(
                &mut pm,
                bold(),
                84.0,
                214.0,
                96.0,
                &value,
                lit,
                1.0,
                Align::Right,
            );
            draw_text(
                &mut pm,
                bold(),
                34.0,
                226.0,
                96.0,
                unit,
                lit,
                0.9,
                Align::Left,
            );
            draw_text(
                &mut pm,
                regular(),
                15.0,
                26.0,
                34.0,
                &caption(one.metric),
                LABEL,
                1.0,
                Align::Left,
            );
        }
        many => {
            // Two to four values side by side.
            let n = many.len().min(4) as f32;
            let cell = (w as f32 - 40.0) / n;
            for (i, s) in many.iter().take(4).enumerate() {
                let x = 20.0 + cell * (i as f32 + 0.5);
                let (value, unit) = s.metric.format(s.value, fahrenheit);
                let px = fit(bold(), 52.0, &value, cell * 0.9);
                draw_text(
                    &mut pm,
                    bold(),
                    px,
                    x,
                    90.0,
                    &value,
                    lit,
                    1.0,
                    Align::Center,
                );
                draw_text(
                    &mut pm,
                    bold(),
                    18.0,
                    x,
                    114.0,
                    unit,
                    lit,
                    0.85,
                    Align::Center,
                );
                // Short captions: the unit says the rest.
                draw_text(
                    &mut pm,
                    regular(),
                    13.0,
                    x,
                    36.0,
                    s.metric.caption(),
                    LABEL,
                    1.0,
                    Align::Center,
                );
            }
        }
    }
    // Ten-segment load bar.
    let segments = load.map_or(0, |l| ((l / 10.0).round() as i32).clamp(1, 10));
    for i in 0..10 {
        let x = 26.0 + i as f32 * 31.0;
        let on = i < segments;
        fill_rect(
            &mut pm,
            x,
            128.0,
            25.0,
            8.0,
            if on {
                color(lit, 1.0)
            } else {
                color(Rgb::WHITE, 0.08)
            },
        );
    }
    to_picture(&pm)
}

/// Preview of the 14 × 14 dot matrix (DeepCool LP).
pub fn matrix_preview(m: &[[bool; 14]; 14], look: Look) -> Picture {
    let side = 224u32;
    let Some(mut pm) = Pixmap::new(side, side) else {
        return Picture::new(side, side);
    };
    pm.fill(Color::TRANSPARENT);
    if let Some(panel) = round_rect(0.0, 0.0, side as f32, side as f32, 28.0) {
        fill_path(&mut pm, &panel, color(Rgb::new(10, 12, 16), 1.0));
    }
    let step = (side as f32 - 28.0) / 14.0;
    let mut pb_on = PathBuilder::new();
    let mut pb_off = PathBuilder::new();
    for (r, row) in m.iter().enumerate() {
        for (c, on) in row.iter().enumerate() {
            let (x, y) = (
                14.0 + step * (c as f32 + 0.5),
                14.0 + step * (r as f32 + 0.5),
            );
            if *on {
                pb_on.push_circle(x, y, step * 0.38);
            } else {
                pb_off.push_circle(x, y, step * 0.30);
            }
        }
    }
    if let Some(p) = pb_off.finish() {
        fill_path(&mut pm, &p, color(Rgb::WHITE, 0.07));
    }
    if let Some(p) = pb_on.finish() {
        fill_path(&mut pm, &p, color(look.accent, 1.0));
    }
    to_picture(&pm)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(metric: Metric, value: f32) -> Shown {
        Shown {
            metric,
            value: Some(value),
        }
    }

    fn look() -> Look {
        Look {
            accent: Rgb::new(0, 160, 255),
        }
    }

    /// Share of pixels clearly brighter than the background.
    fn lit_share(p: &Picture) -> f32 {
        let lit = p
            .rgba
            .chunks_exact(4)
            .filter(|px| px[0].max(px[1]).max(px[2]) > 120)
            .count();
        lit as f32 / (p.width * p.height) as f32
    }

    #[test]
    fn screens_draw_values_text_and_gauges() {
        let settings = DisplaySettings::default();
        let mut images = ImageCache::default();
        for n in 1..=4 {
            let values: Vec<Shown> = [
                shown(Metric::CpuTemp, 64.0),
                shown(Metric::CpuLoad, 37.0),
                shown(Metric::GpuTemp, 71.0),
                shown(Metric::Ram, 52.0),
            ]
            .into_iter()
            .take(n)
            .collect();
            let p = screen(320, 320, &settings, &values, false, look(), &mut images);
            assert_eq!((p.width, p.height), (320, 320));
            let share = lit_share(&p);
            assert!((0.01..0.5).contains(&share), "{n} values: {share}");
            // Opaque, dark corners.
            assert_eq!(p.rgba[3], 255);
            assert!(p.rgb(0, 0).iter().all(|c| *c < 40));
        }
        let text = DisplaySettings {
            content: Content::Text,
            text: "Bonjour TuxRGB, un texte un peu long pour l'écran".into(),
            ..DisplaySettings::default()
        };
        let p = screen(480, 480, &text, &[], false, look(), &mut images);
        assert!(lit_share(&p) > 0.01);
        // Hot CPU: the gauge turns red.
        let hot = screen(
            240,
            240,
            &settings,
            &[shown(Metric::CpuTemp, 96.0)],
            false,
            look(),
            &mut images,
        );
        let reddish = hot
            .rgba
            .chunks_exact(4)
            .filter(|px| px[0] > 200 && px[1] < 120 && px[2] < 120)
            .count();
        assert!(reddish > 100, "{reddish}");
    }

    #[test]
    fn pictures_cover_the_screen() {
        let dir = std::env::temp_dir().join(format!("tuxrgb-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wide.png");
        // 4:1 picture, left half red, right half blue.
        let mut img = image::RgbaImage::new(400, 100);
        for (x, _, px) in img.enumerate_pixels_mut() {
            *px = if x < 200 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            };
        }
        img.save(&path).unwrap();
        let settings = DisplaySettings {
            content: Content::Image,
            image: Some(path.clone()),
            ..DisplaySettings::default()
        };
        let mut images = ImageCache::default();
        let p = screen(100, 100, &settings, &[], false, look(), &mut images);
        // Centred crop: red on the left, blue on the right, full height.
        assert!(p.rgb(5, 50)[0] > 200);
        assert!(p.rgb(95, 50)[2] > 200);
        assert!(p.rgb(5, 2)[0] > 200);
        // A missing file: the screen says so.
        let missing = DisplaySettings {
            image: Some(dir.join("none.png")),
            ..settings
        };
        let p = screen(100, 100, &missing, &[], false, look(), &mut images);
        assert!(images.error.is_some());
        assert!(lit_share(&p) > 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn previews_of_digits_and_matrices() {
        let p = digits_preview(&[shown(Metric::CpuTemp, 63.0)], Some(47.0), false, look());
        assert_eq!((p.width, p.height), (360, 150));
        // Rounded corners are transparent.
        assert_eq!(p.rgba[3], 0);
        let mut m = [[false; 14]; 14];
        m[3][3] = true;
        let q = matrix_preview(&m, look());
        assert!(lit_share(&q) > 0.0);
        let mut round = Picture::new(10, 10);
        round.rgba.fill(255);
        round_mask(&mut round);
        assert_eq!(round.rgba[3], 0);
        assert_eq!(round.rgba[((5 * 10 + 5) * 4 + 3) as usize], 255);
    }

    /// `TUXRGB_RENDER_DIR=<dir> cargo test … render_gallery -- --ignored`:
    /// every layout as a PNG, to look at.
    #[test]
    #[ignore]
    fn render_gallery() {
        let dir = std::path::PathBuf::from(std::env::var("TUXRGB_RENDER_DIR").unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        let save = |name: &str, p: &Picture| {
            image::RgbaImage::from_raw(p.width, p.height, p.rgba.clone())
                .unwrap()
                .save(dir.join(name))
                .unwrap();
        };
        let mut images = ImageCache::default();
        let settings = DisplaySettings::default();
        let all = [
            shown(Metric::CpuTemp, 64.0),
            shown(Metric::GpuLoad, 37.0),
            shown(Metric::CpuFreq, 4875.0),
            shown(Metric::Ram, 52.0),
        ];
        for n in 1..=4 {
            let mut p = screen(320, 320, &settings, &all[..n], false, look(), &mut images);
            round_mask(&mut p);
            save(&format!("screen-{n}.png"), &p);
        }
        let hot = screen(
            480,
            480,
            &settings,
            &[shown(Metric::CpuTemp, 88.0)],
            false,
            look(),
            &mut images,
        );
        save("screen-hot.png", &hot);
        let time = screen(
            320,
            320,
            &settings,
            &[Shown {
                metric: Metric::Time,
                value: None,
            }],
            false,
            look(),
            &mut images,
        );
        save("screen-time.png", &time);
        let text = DisplaySettings {
            content: Content::Text,
            text: "Bienvenue sur TuxRGB".into(),
            ..DisplaySettings::default()
        };
        save(
            "screen-text.png",
            &screen(320, 320, &text, &[], false, look(), &mut images),
        );
        save(
            "digits-1.png",
            &digits_preview(&all[..1], Some(47.0), false, look()),
        );
        save(
            "digits-4.png",
            &digits_preview(&all, Some(47.0), false, look()),
        );
        let m = crate::displays::deepcool::lp_matrix(
            &[
                (63, crate::displays::deepcool::MatrixUnit::Celsius),
                (47, crate::displays::deepcool::MatrixUnit::Percent),
            ],
            0,
        );
        save("matrix.png", &matrix_preview(&m, look()));
    }

    #[test]
    fn long_texts_wrap_and_shrink() {
        let (px, lines) = layout_text(
            "Un texte assez long pour tenir sur plusieurs lignes",
            80.0,
            300.0,
        );
        assert!(lines.len() <= 3, "{lines:?}");
        assert!(px <= 80.0);
        for l in &lines {
            assert!(text_width(bold(), px, l) <= 300.0 + 1.0, "{l}");
        }
    }
}
