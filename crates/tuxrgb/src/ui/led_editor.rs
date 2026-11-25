//! Per-LED editor (TuxRGB 3.7): every LED of a device drawn as it sits on
//! it – keyboards as their real key layout (OpenRGB's matrix map, keys named
//! after their caps), the other zones as rows or rings of LEDs – and painted
//! with the pointer. A painted LED keeps its colour over whatever its zone
//! shows (see [`tuxrgb_core::LedPaint`]); the other LEDs keep following the
//! effect, live. Quick selections (all, a zone, letters, digits, F keys,
//! arrows, WASD, keypad, modifiers) paint many at once.

use super::motion::button;
use super::widgets::{self, icon, icon_filled, label, small, switch, title_medium};
use super::{icons, page_header};
use crate::app::{App, Message};
use crate::theme::style::{self, ButtonKind, Surface};
use crate::theme::tokens::{shape, space};
use crate::theme::{Roles, to_iced, with_alpha};
use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Event, Frame as CFrame, Geometry, Path, Stroke, Text};
use iced::widget::{Space, column, container, row};
use iced::{Alignment, Element, Length, Padding, Point, Rectangle, Renderer, Size, Theme};
use serde::{Deserialize, Serialize};
use tuxrgb_core::{DeviceInfo, DeviceKind, LedPaint, Rgb, ZoneKind};

/// What a stroke does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LedTool {
    /// Gives the LEDs the brush colour.
    #[default]
    Paint,
    /// Gives them back to the effect.
    Effect,
}

/// Letters printed on the keys drawn (OpenRGB names keys after their US
/// position, whatever the keyboard).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyLabels {
    /// AZERTY in French, QWERTY otherwise.
    #[default]
    Auto,
    Qwerty,
    Azerty,
}

impl KeyLabels {
    pub fn azerty(self) -> bool {
        match self {
            Self::Auto => tuxrgb_core::i18n::language() == tuxrgb_core::i18n::Lang::Fr,
            Self::Qwerty => false,
            Self::Azerty => true,
        }
    }
}

/// One LED as drawn by the editor, in sheet units (a key is 1 × 1).
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    /// Device LED index (OpenRGB order).
    pub led: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// A key (rounded square with its label) rather than a dot.
    pub key: bool,
    pub label: String,
}

/// Every LED of a device laid out for the editor, with the zone titles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheet {
    pub cells: Vec<Cell>,
    pub captions: Vec<(String, f32, f32)>,
    pub width: f32,
    pub height: f32,
}

/// LEDs per row of a long strip.
const ROW: usize = 24;

/// Lays out the LEDs of `info`.
pub fn sheet(info: &DeviceInfo, azerty: bool) -> Sheet {
    let mut s = Sheet::default();
    let mut y = 0.0f32;
    let mut start = 0usize;
    let titled = info.zones.iter().filter(|z| z.led_count > 0).count() > 1;
    let name = |led: usize| info.led_names.get(led).map(String::as_str).unwrap_or("");
    for zone in &info.zones {
        let n = zone.led_count;
        if n == 0 {
            continue;
        }
        if titled {
            s.captions.push((
                format!(
                    "{} · {}",
                    crate::i18n::t(&zone.name),
                    crate::i18n::trf!("{} LED", n)
                ),
                0.0,
                y,
            ));
            y += 0.8;
        }
        let mut dots: Vec<usize> = Vec::new();
        match (&zone.kind, &zone.matrix_map) {
            (ZoneKind::Matrix, Some(map)) => {
                let empty = |c: usize, r: usize| map.cells[r * map.width + c].is_none();
                for (i, pos) in map.positions(n).into_iter().enumerate() {
                    let led = start + i;
                    match pos {
                        Some((c, r)) => {
                            // The space bar takes the gaps around it, like
                            // the real one.
                            let (mut x0, mut x1) = (c, c + 1);
                            if key_of(name(led)) == Some("Space") {
                                while x0 > 0 && empty(x0 - 1, r) {
                                    x0 -= 1;
                                }
                                while x1 < map.width && empty(x1, r) {
                                    x1 += 1;
                                }
                            }
                            s.cells.push(Cell {
                                led,
                                x: x0 as f32,
                                y: y + r as f32,
                                w: (x1 - x0) as f32,
                                h: 1.0,
                                key: true,
                                label: key_label(name(led), azerty),
                            })
                        }
                        None => dots.push(led),
                    }
                }
                s.width = s.width.max(map.width as f32);
                y += map.height as f32 + 0.4;
            }
            (ZoneKind::Ring, _) if n >= 4 => {
                ring(&mut s, start, n, &mut y);
            }
            (ZoneKind::Linear, _) if info.kind == DeviceKind::Cooler && n >= 4 => {
                ring(&mut s, start, n, &mut y);
            }
            _ => dots.extend(start..start + n),
        }
        // A strip (or what the key map leaves out: logo, light bar).
        for (i, led) in dots.iter().enumerate() {
            s.cells.push(Cell {
                led: *led,
                x: (i % ROW) as f32,
                y: y + (i / ROW) as f32,
                w: 1.0,
                h: 1.0,
                key: false,
                label: String::new(),
            });
        }
        if !dots.is_empty() {
            s.width = s.width.max(dots.len().min(ROW) as f32);
            y += dots.len().div_ceil(ROW) as f32 + 0.4;
        }
        y += 0.4;
        start += n;
    }
    s.height = (y - 0.8).max(1.0);
    s.width = s.width.max(1.0);
    s
}

/// A fan ring (or a round pump): its LEDs on a circle.
fn ring(s: &mut Sheet, start: usize, n: usize, y: &mut f32) {
    let radius = (n as f32 * 1.15 / std::f32::consts::TAU).max(1.6);
    let (cx, cy) = (radius + 0.5, *y + radius + 0.5);
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        s.cells.push(Cell {
            led: start + i,
            x: cx + radius * a.cos() - 0.5,
            y: cy + radius * a.sin() - 0.5,
            w: 1.0,
            h: 1.0,
            key: false,
            label: String::new(),
        });
    }
    s.width = s.width.max(2.0 * radius + 1.0);
    *y += 2.0 * radius + 1.4;
}

/// The key of an OpenRGB key name ("Key: Left Shift" → "Left Shift").
fn key_of(name: &str) -> Option<&str> {
    name.strip_prefix("Key: ")
}

/// What a key shows: its character, or a short word.
pub fn key_label(name: &str, azerty: bool) -> String {
    let t = crate::i18n::t;
    let Some(k) = key_of(name) else {
        return String::new();
    };
    if let Some(pad) = k.strip_prefix("Number Pad ") {
        return match pad {
            "Enter" => "↵".into(),
            "Up Arrow" => "↑".into(),
            "Down Arrow" => "↓".into(),
            "Left Arrow" => "←".into(),
            "Right Arrow" => "→".into(),
            other => key_label(&format!("Key: {other}"), false),
        };
    }
    if azerty {
        let a = match k {
            "Q" => Some("A"),
            "W" => Some("Z"),
            "A" => Some("Q"),
            "Z" => Some("W"),
            "M" => Some(","),
            ";" => Some("M"),
            "," => Some(";"),
            "." => Some(":"),
            "/" => Some("!"),
            "`" => Some("²"),
            "-" => Some(")"),
            "[" => Some("^"),
            "]" => Some("$"),
            "'" => Some("ù"),
            "#" => Some("*"),
            "\\ (ISO)" => Some("<"),
            _ => None,
        };
        if let Some(a) = a {
            return a.into();
        }
    }
    let word = match k {
        "Escape" => t("Échap"),
        "Backspace" => "←",
        "Enter" => "↵",
        "Tab" => "Tab",
        "Caps Lock" => t("Verr"),
        "Left Shift" | "Right Shift" => t("Maj"),
        "Left Control" | "Right Control" => "Ctrl",
        "Left Windows" | "Right Windows" => "Win",
        "Left Alt" => "Alt",
        "Right Alt" => "AltGr",
        "Left Fn" | "Right Fn" => "Fn",
        "Menu" => "Menu",
        "Space" => t("Espace"),
        "Print Screen" => t("Impr"),
        "Scroll Lock" => t("Défil"),
        "Pause/Break" => "Pause",
        "Insert" => t("Inser"),
        "Delete" => t("Suppr"),
        "Home" => t("Début"),
        "End" => t("Fin"),
        "Page Up" => "Pg↑",
        "Page Down" => "Pg↓",
        "Num Lock" => "Num",
        "Up Arrow" => "↑",
        "Down Arrow" => "↓",
        "Left Arrow" => "←",
        "Right Arrow" => "→",
        "\\ (ISO)" | "\\ (ANSI)" => "\\",
        _ => "",
    };
    if !word.is_empty() {
        return word.to_owned();
    }
    // Characters and F keys as they are; anything longer shortened.
    if k.chars().count() <= 3 {
        return k.to_owned();
    }
    k.split_whitespace()
        .next()
        .unwrap_or(k)
        .chars()
        .take(5)
        .collect()
}

/// Quick selections of a device: label and LEDs.
pub fn groups(info: &DeviceInfo, azerty: bool) -> Vec<(String, Vec<usize>)> {
    let t = |s: &str| crate::i18n::t(s).to_owned();
    let mut out = vec![(t("Tout"), (0..info.led_count).collect::<Vec<_>>())];
    let live: Vec<_> = info.zones.iter().filter(|z| z.led_count > 0).collect();
    if live.len() > 1 {
        let mut start = 0;
        for z in &info.zones {
            if z.led_count > 0 {
                out.push((t(&z.name), (start..start + z.led_count).collect()));
            }
            start += z.led_count;
        }
    }
    let keys: Vec<(usize, &str)> = info
        .led_names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| key_of(n).map(|k| (i, k)))
        .collect();
    if keys.len() < 10 {
        return out;
    }
    let pick = |f: &dyn Fn(&str) -> bool| -> Vec<usize> {
        keys.iter().filter(|(_, k)| f(k)).map(|(i, _)| *i).collect()
    };
    let single = |k: &str, f: fn(char) -> bool| {
        let mut c = k.chars();
        matches!((c.next(), c.next()), (Some(ch), None) if f(ch))
    };
    let candidates: Vec<(String, Vec<usize>)> = vec![
        (
            t("Lettres"),
            pick(&|k| single(k, |c| c.is_ascii_alphabetic())),
        ),
        (t("Chiffres"), pick(&|k| single(k, |c| c.is_ascii_digit()))),
        (
            "F1–F12".to_owned(),
            pick(&|k| {
                k.strip_prefix('F')
                    .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            }),
        ),
        (
            t("Flèches"),
            pick(&|k| matches!(k, "Up Arrow" | "Down Arrow" | "Left Arrow" | "Right Arrow")),
        ),
        (
            if azerty { "ZQSD" } else { "WASD" }.to_owned(),
            pick(&|k| matches!(k, "W" | "A" | "S" | "D")),
        ),
        (t("Pavé numérique"), pick(&|k| k.starts_with("Number Pad"))),
        (
            t("Modificateurs"),
            pick(&|k| {
                matches!(
                    k,
                    "Left Shift"
                        | "Right Shift"
                        | "Left Control"
                        | "Right Control"
                        | "Left Alt"
                        | "Right Alt"
                        | "Left Windows"
                        | "Right Windows"
                        | "Left Fn"
                        | "Right Fn"
                        | "Caps Lock"
                        | "Tab"
                        | "Menu"
                )
            }),
        ),
        (
            t("Autres LED"),
            info.led_names
                .iter()
                .enumerate()
                .filter(|(_, n)| key_of(n).is_none())
                .map(|(i, _)| i)
                .collect(),
        ),
    ];
    out.extend(candidates.into_iter().filter(|(_, leds)| !leds.is_empty()));
    out
}

/// Editor state kept between frames: the stroke in progress, the LED under
/// the pointer.
#[derive(Default)]
pub struct EditorState {
    painting: bool,
    last: Option<usize>,
    hover: Option<usize>,
}

/// The device drawn LED by LED, paintable.
pub struct LedCanvas<'a> {
    pub sheet: Sheet,
    pub info: &'a DeviceInfo,
    /// What each LED shows right now (live frame), device LED order.
    pub live: Vec<Rgb>,
    pub paint: &'a LedPaint,
    pub roles: Roles,
}

impl LedCanvas<'_> {
    /// Size of a sheet unit in pixels and the sheet's top-left corner.
    fn fit(&self, bounds: Size) -> (f32, Point) {
        let pad = 12.0;
        let top = 26.0;
        let unit = ((bounds.width - 2.0 * pad) / self.sheet.width)
            .min((bounds.height - pad - top) / self.sheet.height)
            .clamp(4.0, 56.0);
        let w = unit * self.sheet.width;
        let h = unit * self.sheet.height;
        (
            unit,
            Point::new(
                ((bounds.width - w) * 0.5).max(pad),
                top + ((bounds.height - top - pad - h) * 0.5).max(0.0),
            ),
        )
    }

    fn rect(&self, c: &Cell, unit: f32, origin: Point) -> Rectangle {
        let gap = if c.key { 0.12 } else { 0.18 };
        Rectangle {
            x: origin.x + (c.x + gap * 0.5) * unit,
            y: origin.y + (c.y + gap * 0.5) * unit,
            width: (c.w - gap) * unit,
            height: (c.h - gap) * unit,
        }
    }

    fn hit(&self, bounds: Rectangle, cursor: Cursor) -> Option<usize> {
        let pos = cursor.position_in(bounds)?;
        let (unit, origin) = self.fit(bounds.size());
        self.sheet
            .cells
            .iter()
            .find(|c| {
                let r = self.rect(c, unit, origin);
                // Dots are small: a bit of slack around them.
                let slack = if c.key { 0.0 } else { unit * 0.08 };
                pos.x >= r.x - slack
                    && pos.x <= r.x + r.width + slack
                    && pos.y >= r.y - slack
                    && pos.y <= r.y + r.height + slack
            })
            .map(|c| c.led)
    }

    fn shown(&self, led: usize) -> (Rgb, bool) {
        match self.paint.color(led) {
            Some(c) => (c, true),
            None => (self.live.get(led).copied().unwrap_or(Rgb::BLACK), false),
        }
    }
}

impl canvas::Program<Message> for LedCanvas<'_> {
    type State = EditorState;

    fn update(
        &self,
        state: &mut EditorState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<canvas::Action<Message>> {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let led = self.hit(bounds, cursor)?;
                state.painting = true;
                state.last = Some(led);
                Some(canvas::Action::publish(Message::LedPaint(vec![led])).and_capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let led = self.hit(bounds, cursor);
                let hover_changed = led != state.hover;
                state.hover = led;
                if state.painting
                    && let Some(led) = led
                    && state.last != Some(led)
                {
                    state.last = Some(led);
                    return Some(
                        canvas::Action::publish(Message::LedPaint(vec![led])).and_capture(),
                    );
                }
                hover_changed.then(canvas::Action::request_redraw)
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.painting => {
                state.painting = false;
                state.last = None;
                Some(canvas::Action::publish(Message::LedPaintEnd).and_capture())
            }
            Event::Mouse(mouse::Event::CursorLeft) => {
                state.hover = None;
                Some(canvas::Action::request_redraw())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &EditorState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let r = &self.roles;
        let mut f = CFrame::new(renderer, bounds.size());
        let (unit, origin) = self.fit(bounds.size());
        for (title, x, y) in &self.sheet.captions {
            f.fill_text(Text {
                content: title.clone(),
                position: Point::new(origin.x + x * unit, origin.y + y * unit),
                color: r.on_surface_variant,
                size: 12.0.into(),
                ..Text::default()
            });
        }
        for c in &self.sheet.cells {
            let rect = self.rect(c, unit, origin);
            let (color, painted) = self.shown(c.led);
            let fill = to_iced(color);
            let hovered = state.hover == Some(c.led);
            if c.key {
                let path = Path::rounded_rectangle(
                    rect.position(),
                    rect.size(),
                    (rect.width.min(rect.height) * 0.2).into(),
                );
                // Keycap: the LED colour, a darker rim, the label on top.
                f.fill(&path, r.surface_container_highest);
                let inner = Path::rounded_rectangle(
                    Point::new(rect.x + 1.5, rect.y + 1.5),
                    Size::new(rect.width - 3.0, rect.height - 3.0),
                    (rect.width.min(rect.height) * 0.18).into(),
                );
                f.fill(
                    &inner,
                    with_alpha(fill, 0.25 + 0.75 * luminance_weight(color)),
                );
                if !c.label.is_empty() {
                    let len = c.label.chars().count().max(1) as f32;
                    let size = (rect.height * 0.36)
                        .min(rect.width * 1.6 / len)
                        .clamp(7.0, 15.0);
                    let ink = if color.luminance() > 0.45 {
                        iced::Color::from_rgb(0.08, 0.08, 0.1)
                    } else {
                        iced::Color::WHITE
                    };
                    f.fill_text(Text {
                        content: c.label.clone(),
                        position: rect.center(),
                        color: ink,
                        size: size.into(),
                        align_x: iced::widget::text::Alignment::Center,
                        align_y: iced::alignment::Vertical::Center,
                        ..Text::default()
                    });
                }
                if painted {
                    marker(&mut f, rect, r);
                }
                if hovered {
                    f.stroke(
                        &path,
                        Stroke::default().with_width(2.0).with_color(r.on_surface),
                    );
                }
            } else {
                let radius = rect.width.min(rect.height) * 0.5;
                let circle = Path::circle(rect.center(), radius);
                f.fill(&circle, r.surface_container_highest);
                f.fill(&Path::circle(rect.center(), (radius - 1.5).max(1.0)), fill);
                if painted {
                    marker(&mut f, rect, r);
                }
                if hovered {
                    f.stroke(
                        &circle,
                        Stroke::default().with_width(2.0).with_color(r.on_surface),
                    );
                }
            }
        }
        // The LED under the pointer, by name.
        if let Some(led) = state.hover {
            let name = self
                .info
                .led_names
                .get(led)
                .map(|n| n.strip_prefix("Key: ").unwrap_or(n).to_owned())
                .unwrap_or_default();
            let (_, painted) = self.shown(led);
            let text = if name.is_empty() {
                crate::i18n::trf!("LED {}", led + 1)
            } else {
                format!("{name} · {}", crate::i18n::trf!("LED {}", led + 1))
            };
            let text = if painted {
                format!("{text} · {}", crate::i18n::t("peinte"))
            } else {
                text
            };
            f.fill_text(Text {
                content: text,
                position: Point::new(12.0, 4.0),
                color: r.on_surface,
                size: 13.0.into(),
                ..Text::default()
            });
        }
        vec![f.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &EditorState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> mouse::Interaction {
        if state.painting || self.hit(bounds, cursor).is_some() {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
    }
}

/// Dark LEDs still read as keys: the colour shows over the cap, more
/// opaque the brighter it is.
fn luminance_weight(c: Rgb) -> f32 {
    (c.luminance() * 4.0).clamp(0.0, 1.0)
}

/// Small corner dot on a painted LED.
fn marker(f: &mut CFrame, rect: Rectangle, r: &Roles) {
    let d = (rect.width.min(rect.height) * 0.13).clamp(2.5, 5.0);
    let at = Point::new(rect.x + rect.width - d - 2.5, rect.y + d + 2.5);
    f.fill(&Path::circle(at, d * 0.5 + 1.0), with_alpha(r.surface, 0.8));
    f.fill(&Path::circle(at, d * 0.5), r.primary);
}

/// The editor page (in place of the device list and canvas).
pub fn view<'a>(app: &'a App, info: &'a DeviceInfo) -> Element<'a, Message> {
    let r = &app.roles;
    let paint = app
        .config
        .led_paint
        .get(&info.key)
        .cloned()
        .unwrap_or_default();
    let painted = paint.colors.len();
    let live = live_colors(app, info);
    let azerty = app.config.key_labels.azerty();
    let header = page_header(
        app,
        "LED par LED",
        info.display_name().to_owned(),
        row![widgets::icon_label_button(
            icons::ARROW_BACK,
            "Retour aux appareils",
            ButtonKind::Tonal,
            r,
            Some(Message::OpenLedEditor(None))
        ),]
        .into(),
    );
    let canvas_card = container(
        canvas::Canvas::new(LedCanvas {
            sheet: sheet(info, azerty),
            info,
            live,
            paint: app.config.led_paint.get(&info.key).unwrap_or(&EMPTY_PAINT),
            roles: *r,
        })
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .padding(space::M)
    .width(Length::Fill)
    .height(Length::Fill)
    .style(style::card(
        *r,
        Surface::Default,
        shape::EXTRA_LARGE_INCREASED,
    ));

    // Tools: paint with the brush colour, or give back to the effect.
    let brush = app.led_brush;
    let tools = row![
        widgets::segmented(
            &[
                (LedTool::Paint, "Peindre"),
                (LedTool::Effect, "Rendre à l'effet"),
            ],
            app.led_tool,
            r,
            Message::SetLedTool,
        ),
        button(
            row![
                container(Space::new().width(22).height(22)).style(move |_| container::Style {
                    background: Some(iced::Background::Color(to_iced(brush))),
                    border: iced::Border {
                        radius: 11.0.into(),
                        width: 1.5,
                        color: with_alpha(iced::Color::WHITE, 0.6),
                    },
                    ..Default::default()
                }),
                small(brush.to_hex()),
            ]
            .spacing(space::S)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([6.0, 12.0]))
        .style(style::chip(*r, app.led_picker))
        .on_press(Message::ToggleLedPicker),
    ]
    .spacing(space::M)
    .align_y(Alignment::Center);

    let mut quick = row![].spacing(space::XS);
    for (name, leds) in groups(info, azerty) {
        quick = quick.push(
            button(small(name))
                .padding(Padding::from([6.0, 14.0]))
                .style(style::chip(*r, false))
                .on_press(Message::LedPaint(leds)),
        );
    }

    let mut side = column![
        row![
            icon_filled(icons::FORMAT_PAINT, 22.0).color(r.primary),
            column![
                title_medium("Couleurs LED par LED"),
                small(if painted == 0 {
                    crate::i18n::t("Aucune LED peinte : tout suit l'effet.").to_owned()
                } else {
                    crate::i18n::trf!("{} LED peintes, les autres suivent l'effet.", painted)
                })
                .color(r.on_surface_variant),
            ]
            .spacing(space::XXS)
            .width(Length::Fill),
            switch(paint.enabled, r, Message::SetLedPaintEnabled),
        ]
        .spacing(space::M)
        .align_y(Alignment::Center),
        tools,
    ]
    .spacing(space::L);
    if app.led_picker || app.led_picker_shown {
        side = side.push(
            super::transition::reveal(
                app.led_picker,
                r.surface_container,
                super::effects::color_editor(app, brush),
            )
            .on_hidden(Message::LedPickerHidden),
        );
    }
    side = side.push(
        column![
            row![
                icon(icons::GRID_VIEW, 18.0).color(r.primary),
                label("Appliquer à"),
            ]
            .spacing(space::S)
            .align_y(Alignment::Center),
            small("Avec l'outil choisi : la couleur du pinceau, ou retour à l'effet.")
                .color(r.on_surface_variant),
            quick.wrap().vertical_spacing(space::XS),
        ]
        .spacing(space::S),
    );
    if info.led_names.iter().any(|n| n.starts_with("Key: ")) {
        side = side.push(
            row![
                icon(icons::KEYBOARD, 18.0).color(r.on_surface_variant),
                small("Touches")
                    .color(r.on_surface_variant)
                    .width(Length::Fill),
                widgets::segmented(
                    &[
                        (KeyLabels::Auto, "Auto"),
                        (KeyLabels::Azerty, "AZERTY"),
                        (KeyLabels::Qwerty, "QWERTY"),
                    ],
                    app.config.key_labels,
                    r,
                    Message::SetKeyLabels,
                ),
            ]
            .spacing(space::S)
            .align_y(Alignment::Center),
        );
    }
    side = side.push(Space::new().height(Length::Fill));
    side = side.push(
        column![
            small("Glissez sur les LED pour peindre. Noir éteint une LED ; « Rendre à l'effet » la laisse suivre l'effet en cours. Les couleurs suivent la luminosité globale.")
                .color(r.on_surface_variant),
            widgets::icon_label_button(
                icons::RESTORE,
                "Tout rendre à l'effet",
                ButtonKind::Outlined,
                r,
                (painted > 0).then_some(Message::ClearLedPaint),
            ),
        ]
        .spacing(space::M),
    );
    column![
        header,
        Space::new().height(space::XL),
        row![
            canvas_card,
            container(super::transition::smooth_scroll(
                iced::widget::scrollable(side.padding(Padding {
                    right: space::M,
                    ..Padding::ZERO
                })),
                r
            ))
            .padding(space::XL)
            .width(Length::Fixed(380.0))
            .height(Length::Fill)
            .style(style::card(
                *r,
                Surface::Default,
                shape::EXTRA_LARGE_INCREASED
            )),
        ]
        .spacing(space::XL)
        .height(Length::Fill),
    ]
    .into()
}

static EMPTY_PAINT: std::sync::LazyLock<LedPaint> = std::sync::LazyLock::new(LedPaint::default);

/// What each LED of `info` shows in the latest preview frame.
fn live_colors(app: &App, info: &DeviceInfo) -> Vec<Rgb> {
    let Some(frame) = app.preview.as_ref() else {
        return Vec::new();
    };
    frame
        .devices
        .iter()
        .find(|d| d.id == info.id)
        .map(|d| d.colors.iter().map(|c| frame.shown(*c)).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyboard() -> DeviceInfo {
        tuxrgb_core::device::demo_devices()
            .into_iter()
            .find(|d| d.kind == DeviceKind::Keyboard)
            .unwrap()
    }

    #[test]
    fn keys_are_labelled_for_the_layout() {
        assert_eq!(key_label("Key: Q", false), "Q");
        assert_eq!(key_label("Key: Q", true), "A");
        assert_eq!(key_label("Key: ;", true), "M");
        assert_eq!(key_label("Key: F11", true), "F11");
        assert_eq!(key_label("Key: Number Pad 7", true), "7");
        assert_eq!(key_label("Key: Up Arrow", false), "↑");
        assert_eq!(key_label("Logo", false), "");
        assert!(key_label("Key: Media Play/Pause", false).chars().count() <= 5);
    }

    #[test]
    fn the_keyboard_is_drawn_as_its_keys() {
        let kb = keyboard();
        let s = sheet(&kb, true);
        assert_eq!(s.cells.len(), kb.led_count);
        assert!(s.cells.iter().all(|c| c.key));
        assert_eq!((s.width, s.height.ceil()), (22.0, 6.0));
        let space = kb.led_names.iter().position(|n| n == "Key: Space").unwrap();
        let cell = s.cells.iter().find(|c| c.led == space).unwrap();
        assert_eq!((cell.x, cell.w), (3.0, 7.0));
        // Each LED once.
        let mut leds: Vec<usize> = s.cells.iter().map(|c| c.led).collect();
        leds.sort_unstable();
        leds.dedup();
        assert_eq!(leds.len(), kb.led_count);
    }

    #[test]
    fn the_pointer_finds_the_key_under_it() {
        let kb = keyboard();
        let paint = LedPaint::default();
        let c = LedCanvas {
            sheet: sheet(&kb, false),
            info: &kb,
            live: Vec::new(),
            paint: &paint,
            roles: crate::theme::Roles::generate(
                crate::theme::BREEZE_ACCENT,
                true,
                crate::config::SchemeVariant::Expressive,
            ),
        };
        let bounds = Rectangle::new(Point::new(100.0, 50.0), Size::new(900.0, 320.0));
        for name in [
            "Key: Escape",
            "Key: Space",
            "Key: Number Pad Enter",
            "Key: A",
        ] {
            let led = kb.led_names.iter().position(|n| n == name).unwrap();
            let cell = c.sheet.cells.iter().find(|cell| cell.led == led).unwrap();
            let (unit, origin) = c.fit(bounds.size());
            let r = c.rect(cell, unit, origin);
            let at = Point::new(bounds.x + r.center().x, bounds.y + r.center().y);
            assert_eq!(c.hit(bounds, Cursor::Available(at)), Some(led), "{name}");
        }
        // Between the main block and the arrows: nothing.
        let (unit, origin) = c.fit(bounds.size());
        let gap = Point::new(
            bounds.x + origin.x + 14.5 * unit,
            bounds.y + origin.y + 3.5 * unit,
        );
        assert_eq!(c.hit(bounds, Cursor::Available(gap)), None);
    }

    #[test]
    fn strips_and_fans_are_rows_and_rings() {
        let devices = tuxrgb_core::device::demo_devices();
        for d in &devices {
            let s = sheet(d, false);
            assert_eq!(s.cells.len(), d.led_count, "{}", d.name);
            for c in &s.cells {
                assert!(
                    c.x >= -0.01 && c.x + c.w <= s.width + 0.01,
                    "{} {c:?}",
                    d.name
                );
                assert!(
                    c.y >= -0.01 && c.y + c.h <= s.height + 1.21,
                    "{} {c:?}",
                    d.name
                );
            }
        }
    }

    #[test]
    fn quick_selections_of_a_keyboard() {
        let kb = keyboard();
        let g = groups(&kb, true);
        let find = |n: &str| {
            g.iter()
                .find(|(l, _)| l.as_str() == n)
                .map(|(_, v)| v.len())
        };
        assert_eq!(find("Tout"), Some(kb.led_count));
        assert_eq!(find("Lettres"), Some(26));
        assert_eq!(find("Chiffres"), Some(10));
        assert_eq!(find("F1–F12"), Some(12));
        assert_eq!(find("Flèches"), Some(4));
        assert_eq!(find("ZQSD"), Some(4));
        assert_eq!(find("Pavé numérique"), Some(16));
        assert!(find("Modificateurs").is_some_and(|n| n >= 8));
        assert_eq!(find("Autres LED"), None);
        // A strip: everything, nothing else.
        let strip = tuxrgb_core::device::demo_devices()
            .into_iter()
            .find(|d| d.kind == DeviceKind::LedStrip)
            .unwrap();
        assert_eq!(groups(&strip, false).len(), 1);
    }
}
