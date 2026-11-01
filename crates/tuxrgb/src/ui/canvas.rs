//! The setup canvas: SignalRGB-style 2D view of the user's rig with live LED
//! colours (the native "Visual Map"). Its items are the devices and the fans
//! of split ARGB headers ("per fan per header"). In edit mode items can be
//! dragged around, resized from their corner handle and turned to any angle
//! with the knob above them (or the rotation slider of the Devices page,
//! which also has mirroring and hiding). The selected item is drawn on
//! top of whatever overlaps it, and hidden items stay connected and lit,
//! only off the canvas. The canvas is also the screen: with Screen Sync each
//! LED takes the colour of the part of the screen it covers, which the
//! canvas shows as a screen frame and, for the selected item, the sampled
//! area of every LED.
//!
//! Behind the devices, the whole canvas glows with a soft, blurred rendering
//! of the current effect ([`super::backdrop`]); devices and LEDs are drawn
//! on top with shadows, halos and rings so they stay crisp against it.

use crate::app::Message;
use crate::theme::{Roles, to_iced, with_alpha};
use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Cache, Event, Frame as CFrame, Geometry, Path, Stroke, Text};
use iced::widget::stack;
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, Vector, border};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use tuxrgb_core::engine::Frame;
use tuxrgb_core::layout::{SCENE_ASPECT, Target};
use tuxrgb_core::{DeviceKey, Placement, Scene};

pub struct SetupCanvas<'a> {
    pub scene: &'a Scene,
    pub frame: Option<&'a Arc<Frame>>,
    pub roles: Roles,
    pub selected: Option<Target>,
    /// Items hidden on the canvas (device or fan keys).
    pub hidden: &'a BTreeSet<DeviceKey>,
    pub editable: bool,
    pub show_labels: bool,
    /// Screen Sync active: draw the screen frame and sampling areas.
    pub screen_hint: bool,
    /// Static layer (grid + device bodies), cleared by the app when the
    /// scene, the selection or the theme changes.
    pub cache: &'a Cache,
    /// Blurred full-canvas rendering of the effect (preview frames).
    pub backdrop: Option<&'a iced::widget::image::Handle>,
    /// Arrival of each scene device (`0..=1`, see [`appear`]).
    pub appear: Vec<f32>,
}

/// Arrival of every device of the scene: newly detected devices fade and
/// grow into place.
pub fn appear(app: &crate::app::App) -> Vec<f32> {
    app.scene
        .devices
        .iter()
        .map(|d| super::transition::ease_out(app.device_appear(&d.info.key)))
        .collect()
}

/// Corner radius of the canvas sheet.
const SHEET_RADIUS: f32 = 28.0;

/// The canvas with its glowing sheet underneath. The sheet is a separate
/// canvas in a lower layer: images are drawn after shapes within a layer,
/// so the glow could not sit under the device bodies otherwise.
pub fn view<'a>(program: SetupCanvas<'a>, width: Length, height: Length) -> Element<'a, Message> {
    let sheet = Sheet {
        backdrop: program.backdrop,
        roles: program.roles,
    };
    stack![
        canvas::Canvas::new(sheet).width(width).height(height),
        canvas::Canvas::new(program).width(width).height(height),
    ]
    .into()
}

/// The canvas sheet: the blurred glow of the effect, or a plain surface.
struct Sheet<'a> {
    backdrop: Option<&'a iced::widget::image::Handle>,
    roles: Roles,
}

impl canvas::Program<Message> for Sheet<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let rect = scene_rect(bounds.size());
        let mut sheet = CFrame::new(renderer, bounds.size());
        sheet.fill(
            &Path::rounded_rectangle(
                Point::new(rect.x, rect.y),
                rect.size(),
                border::Radius::new(SHEET_RADIUS),
            ),
            self.roles.surface_container_lowest,
        );
        // The glow fades out before the edges (alpha baked in the image),
        // so the rounded sheet frames it on every renderer.
        if let Some(handle) = self.backdrop {
            sheet.draw_image(
                rect,
                canvas::Image::new(handle.clone())
                    .filter_method(iced::widget::image::FilterMethod::Linear),
            );
        }
        vec![sheet.into_geometry()]
    }
}

#[derive(Default)]
pub struct DragState {
    dragging: Option<(Target, Vector)>,
    /// Resizing from the bottom-right handle.
    resizing: Option<Target>,
    /// Turning with the knob above the item.
    rotating: Option<Target>,
}

/// Radius (px) of the resize handle of the selected item.
const HANDLE: f32 = 7.0;
/// Distance (px) between the top edge of the selected item and its
/// rotation knob.
const KNOB_GAP: f32 = 22.0;

/// The resize handle: bottom-right corner of the turned rectangle.
fn handle_pos(r: &Rectangle, p: &Placement) -> Point {
    let (x, y) = p.point(1.0, 1.0);
    to_screen(r, x, y)
}

/// Middle of the top edge of the turned rectangle and the rotation knob
/// further out in the same direction.
fn knob_pos(r: &Rectangle, p: &Placement) -> (Point, Point) {
    let (cx, cy) = p.center();
    let (tx, ty) = p.point(0.5, 0.0);
    let c = to_screen(r, cx, cy);
    let top = to_screen(r, tx, ty);
    let (dx, dy) = (top.x - c.x, top.y - c.y);
    let len = (dx * dx + dy * dy).sqrt().max(1e-3);
    (
        top,
        Point::new(top.x + dx / len * KNOB_GAP, top.y + dy / len * KNOB_GAP),
    )
}

/// The 16:9 drawing area centred inside the widget bounds.
fn scene_rect(bounds: Size) -> Rectangle {
    let (w, h) = if bounds.width / bounds.height > SCENE_ASPECT {
        (bounds.height * SCENE_ASPECT, bounds.height)
    } else {
        (bounds.width, bounds.width / SCENE_ASPECT)
    };
    Rectangle {
        x: (bounds.width - w) * 0.5,
        y: (bounds.height - h) * 0.5,
        width: w,
        height: h,
    }
}

fn to_screen(r: &Rectangle, x: f32, y: f32) -> Point {
    Point::new(r.x + x * r.width, r.y + y * r.height)
}

fn to_scene(r: &Rectangle, p: Point) -> (f32, f32) {
    ((p.x - r.x) / r.width, (p.y - r.y) / r.height)
}

impl SetupCanvas<'_> {
    /// The selected item, if it is shown.
    fn shown_selection(&self) -> Option<(Target, Placement)> {
        let t = self.selected?;
        if self.scene.is_hidden(t, self.hidden) {
            return None;
        }
        Some((t, self.scene.placement_of(t)?))
    }
}

impl canvas::Program<Message> for SetupCanvas<'_> {
    type State = DragState;

    fn update(
        &self,
        state: &mut DragState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<canvas::Action<Message>> {
        let rect = scene_rect(bounds.size());
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let pos = cursor.position_in(bounds)?;
                if self.editable
                    && let Some((t, p)) = self.shown_selection()
                {
                    // Knob above the selected item: turn it.
                    if knob_pos(&rect, &p).1.distance(pos) <= HANDLE * 1.8 {
                        state.rotating = Some(t);
                        return Some(canvas::Action::capture());
                    }
                    // Corner handle: resize.
                    if handle_pos(&rect, &p).distance(pos) <= HANDLE * 1.8 {
                        state.resizing = Some(t);
                        return Some(canvas::Action::capture());
                    }
                }
                let (sx, sy) = to_scene(&rect, pos);
                let hit = self.scene.hit_test(sx, sy, self.hidden, self.selected);
                if let (true, Some(t)) = (self.editable, hit)
                    && let Some(p) = self.scene.placement_of(t)
                {
                    state.dragging = Some((t, Vector::new(sx - p.x, sy - p.y)));
                }
                Some(canvas::Action::publish(Message::CanvasSelect(hit)).and_capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) if state.rotating.is_some() => {
                let t = state.rotating?;
                let pos = cursor.position_in(bounds).or_else(|| {
                    cursor
                        .position()
                        .map(|p| Point::new(p.x - bounds.x, p.y - bounds.y))
                })?;
                let (sx, sy) = to_scene(&rect, pos);
                let current = self.scene.placement_of(t)?;
                let angle = current.angle_towards(sx, sy);
                Some(canvas::Action::publish(Message::SetRotation(angle)).and_capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) if state.resizing.is_some() => {
                let t = state.resizing?;
                let pos = cursor.position_in(bounds).or_else(|| {
                    cursor
                        .position()
                        .map(|p| Point::new(p.x - bounds.x, p.y - bounds.y))
                })?;
                let (sx, sy) = to_scene(&rect, pos);
                let current = self.scene.placement_of(t)?;
                // The opposite corner stays put, whatever the angle.
                let resized = current.resized_to(sx, sy);
                Some(canvas::Action::publish(Message::CanvasMove(t, resized)).and_capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let (t, offset) = state.dragging?;
                let pos = cursor.position_in(bounds).or_else(|| {
                    cursor
                        .position()
                        .map(|p| Point::new(p.x - bounds.x, p.y - bounds.y))
                })?;
                let (sx, sy) = to_scene(&rect, pos);
                let current = self.scene.placement_of(t)?;
                let moved = Placement {
                    x: sx - offset.x,
                    y: sy - offset.y,
                    ..current
                };
                Some(canvas::Action::publish(Message::CanvasMove(t, moved)).and_capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                // The angle is saved as it changes.
                if state.rotating.take().is_some() {
                    return Some(canvas::Action::capture());
                }
                let was_resizing = state.resizing.take().is_some();
                if state.dragging.take().is_none() && !was_resizing {
                    return None;
                }
                Some(canvas::Action::publish(Message::CanvasDragEnd).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &DragState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let r = &self.roles;
        let rect = scene_rect(bounds.size());
        let unit = rect.height;
        let order = self.scene.draw_order(self.hidden, self.selected);

        let background = self.cache.draw(renderer, bounds.size(), |f: &mut CFrame| {
            // Dot grid (light enough to sit on the glow).
            let step = unit / 18.0;
            let dot = with_alpha(r.on_surface_variant, 0.16);
            let mut y = rect.y + step;
            while y < rect.y + rect.height - step * 0.5 {
                let mut x = rect.x + step;
                while x < rect.x + rect.width - step * 0.5 {
                    f.fill(&Path::circle(Point::new(x, y), 1.1), dot);
                    x += step;
                }
                y += step;
            }
            // The canvas is the screen (Screen Sync): a subtle frame.
            if self.screen_hint {
                let inset = 6.0;
                let frame_rect = Path::rounded_rectangle(
                    Point::new(rect.x + inset, rect.y + inset),
                    Size::new(rect.width - 2.0 * inset, rect.height - 2.0 * inset),
                    border::Radius::new(22.0),
                );
                f.stroke(
                    &frame_rect,
                    Stroke {
                        line_dash: canvas::LineDash {
                            segments: &[8.0, 6.0],
                            offset: 0,
                        },
                        ..Stroke::default()
                            .with_width(2.0)
                            .with_color(with_alpha(r.tertiary, 0.55))
                    },
                );
                f.fill_text(Text {
                    content: crate::i18n::t("Écran : chaque LED reprend la zone qu'elle couvre")
                        .to_owned(),
                    position: Point::new(rect.x + 20.0, rect.y + 12.0),
                    color: with_alpha(r.tertiary, 0.9),
                    size: 11.0.into(),
                    ..Text::default()
                });
            }
            // Item bodies, bottom first; the selected one last.
            for t in &order {
                let Some(d) = self.scene.devices.get(t.device) else {
                    continue;
                };
                let Some(p) = self.scene.placement_of(*t) else {
                    continue;
                };
                let a = self.appear.get(t.device).copied().unwrap_or(1.0);
                if a <= 0.001 {
                    continue;
                }
                let fan = t.segment.and_then(|s| d.segments.get(s));
                // Arrival: grows from 85 % around its centre while fading in.
                let k = 0.85 + 0.15 * a;
                // The rectangle before its turn, in pixels, drawn turned
                // around its centre (any angle).
                let size = Size::new(p.w * rect.width * k, p.h * rect.height * k);
                let (pcx, pcy) = p.center();
                let center = to_screen(&rect, pcx, pcy);
                let tl = Point::new(-size.width * 0.5, -size.height * 0.5);
                // Its box on the canvas, for the label under it.
                let (bx0, by0, bx1, by1) = p.bounds();
                let box_tl = to_screen(&rect, bx0, by0);
                let box_br = to_screen(&rect, bx1, by1);
                let box_size = Size::new((box_br.x - box_tl.x) * k, (box_br.y - box_tl.y) * k);
                let box_tl = Point::new(
                    center.x - box_size.width * 0.5,
                    center.y - box_size.height * 0.5,
                );
                // Fans are round-cornered squares, devices softer boxes.
                let radius = if fan.is_some() {
                    size.width.min(size.height) * 0.42
                } else {
                    (size.width.min(size.height) * 0.22).clamp(4.0, 20.0)
                };
                let body = Path::rounded_rectangle(tl, size, border::Radius::new(radius));
                let selected = self.selected == Some(*t);
                // A fan of the selected device: outlined like a part of it.
                let sibling =
                    fan.is_some() && !selected && self.selected.map(|s| s.device) == Some(t.device);
                let fill = if !d.enabled || !d.info.controllable {
                    with_alpha(r.surface_container_high, 0.6)
                } else if selected {
                    with_alpha(r.primary_container, 0.92)
                } else if fan.is_some() {
                    with_alpha(r.surface_container_highest, 0.95)
                } else {
                    with_alpha(r.surface_container_high, 0.95)
                };
                let outline = if selected {
                    r.primary
                } else if sibling {
                    with_alpha(r.primary, 0.6)
                } else {
                    r.outline_variant
                };
                let outline_width = if selected {
                    2.5
                } else if sibling {
                    1.5
                } else {
                    1.0
                };
                f.with_save(|f| {
                    f.translate(Vector::new(center.x, center.y));
                    if p.rotation != 0.0 {
                        f.rotate(p.rotation.to_radians());
                    }
                    // Soft two-step shadow: lifts the item off the glow.
                    for (spread, alpha) in [(4.0f32, 0.10f32), (1.5, 0.18)] {
                        f.fill(
                            &Path::rounded_rectangle(
                                Point::new(tl.x - spread, tl.y - spread + 2.0),
                                Size::new(size.width + 2.0 * spread, size.height + 2.0 * spread),
                                border::Radius::new(radius + spread),
                            ),
                            with_alpha(r.shadow, alpha * a),
                        );
                    }
                    f.fill(&body, with_alpha(fill, a));
                    f.stroke(
                        &body,
                        Stroke::default()
                            .with_width(outline_width)
                            .with_color(with_alpha(outline, a)),
                    );
                });
                // One line under the item (never wrapped, never hidden by
                // the LEDs), cut to its width with an ellipsis; very narrow
                // items (RAM sticks, small fans) only show it when selected.
                let name = match fan {
                    Some(s) => trf!("Ventilateur {}", s.part + 1),
                    None if d.info.hardware_mode.is_some() => {
                        format!(
                            "{} · {}",
                            d.info.display_name(),
                            crate::i18n::t("effet matériel")
                        )
                    }
                    None => d.info.display_name().to_owned(),
                };
                let max_chars = if selected {
                    usize::MAX
                } else {
                    (box_size.width * 1.1 / 5.8) as usize
                };
                // A fan still in its device: its number says enough (a name
                // under it would cover the device's).
                let docked = fan.is_some_and(|s| !s.pinned);
                if self.show_labels && max_chars >= 6 && !docked {
                    let content = if name.chars().count() > max_chars {
                        let cut: String = name.chars().take(max_chars.saturating_sub(1)).collect();
                        format!("{}…", cut.trim_end())
                    } else {
                        name
                    };
                    let position = Point::new(box_tl.x + 2.0, box_tl.y + box_size.height + 3.0);
                    // Halo in the canvas colour: readable on any glow.
                    let halo = with_alpha(r.surface_container_lowest, 0.75 * a);
                    for (dx, dy) in [(-0.8, 0.0), (0.8, 0.0), (0.0, -0.8), (0.0, 0.8)] {
                        f.fill_text(Text {
                            content: content.clone(),
                            position: Point::new(position.x + dx, position.y + dy),
                            color: halo,
                            size: 11.0.into(),
                            ..Text::default()
                        });
                    }
                    f.fill_text(Text {
                        content,
                        position,
                        color: with_alpha(
                            if selected {
                                r.on_primary_container
                            } else {
                                r.on_surface
                            },
                            a,
                        ),
                        size: 11.0.into(),
                        ..Text::default()
                    });
                }
                // Fan number in the middle of its ring.
                if let Some(s) = fan
                    && size.width.min(size.height) >= 22.0
                {
                    f.fill_text(Text {
                        content: (s.part + 1).to_string(),
                        position: center,
                        color: with_alpha(r.on_surface_variant, 0.8 * a),
                        size: (size.width.min(size.height) * 0.28)
                            .clamp(10.0, 18.0)
                            .into(),
                        align_x: iced::widget::text::Alignment::Center,
                        align_y: iced::alignment::Vertical::Center,
                        ..Text::default()
                    });
                }
            }
        });

        // Live LEDs (redrawn at the preview rate).
        let mut leds = CFrame::new(renderer, bounds.size());
        // Selected item in edit mode: sampled areas and the resize handle.
        if let Some((t, p)) = self.shown_selection()
            && let Some(d) = self.scene.devices.get(t.device)
        {
            if self.screen_hint {
                let range = match t.segment.and_then(|s| d.segments.get(s)) {
                    Some(s) => s.leds.clone(),
                    None => 0..d.geometry.len(),
                };
                for g in d.geometry.get(range).unwrap_or_default() {
                    let tl = to_screen(&rect, g.x - g.rx.max(0.012), g.y - g.ry.max(0.02));
                    let br = to_screen(&rect, g.x + g.rx.max(0.012), g.y + g.ry.max(0.02));
                    leds.stroke(
                        &Path::rectangle(tl, Size::new(br.x - tl.x, br.y - tl.y)),
                        Stroke::default()
                            .with_width(1.0)
                            .with_color(with_alpha(r.tertiary, 0.45)),
                    );
                }
            }
            if self.editable {
                let h = handle_pos(&rect, &p);
                leds.fill(&Path::circle(h, HANDLE + 3.0), with_alpha(r.primary, 0.25));
                leds.fill(&Path::circle(h, HANDLE), r.primary);
                leds.fill(&Path::circle(h, HANDLE - 3.0), r.on_primary);
                // Rotation knob on its stem, above the item's top edge.
                let (top, knob) = knob_pos(&rect, &p);
                leds.stroke(
                    &Path::line(top, knob),
                    Stroke::default()
                        .with_width(2.0)
                        .with_color(with_alpha(r.primary, 0.7)),
                );
                leds.fill(
                    &Path::circle(knob, HANDLE + 3.0),
                    with_alpha(r.primary, 0.25),
                );
                leds.fill(&Path::circle(knob, HANDLE), r.primary);
                leds.stroke(
                    &Path::circle(knob, HANDLE - 3.0),
                    Stroke::default().with_width(1.5).with_color(r.on_primary),
                );
            }
        }
        // Thin ring in the canvas colour around each LED: crisp dots on the
        // glow, whatever the effect.
        let ring = with_alpha(r.surface_container_lowest, 0.85);
        // The frame's colours are at full brightness (see `Frame::level`).
        let level = self.frame.map(|f| f.as_ref());
        let colors: HashMap<u32, &[tuxrgb_core::Rgb]> = self
            .frame
            .map(|f| {
                f.devices
                    .iter()
                    .map(|d| (d.id, d.colors.as_slice()))
                    .collect()
            })
            .unwrap_or_default();
        for (di, d) in self.scene.devices.iter().enumerate() {
            if self.hidden.contains(&d.info.key) {
                continue;
            }
            let a = self.appear.get(di).copied().unwrap_or(1.0);
            if a <= 0.001 {
                continue;
            }
            // LEDs of hidden fans are left out; each fan gets the LED size of
            // its own ring.
            let hidden_fans: Vec<std::ops::Range<usize>> = d
                .segments
                .iter()
                .filter(|s| self.hidden.contains(&s.key))
                .map(|s| s.leds.clone())
                .collect();
            let led_radius = |p: &Placement, n: usize| {
                // LED size adapts to density so keyboards and strips both read well.
                let area = p.w * rect.width * p.h * rect.height;
                ((area / n.max(1) as f32).sqrt() * 0.32).clamp(1.5, 7.0)
            };
            let body_r = led_radius(&d.placement, d.leds.len());
            let device_colors = colors.get(&d.info.id).copied();
            for (li, &(x, y)) in d.leds.iter().enumerate() {
                if hidden_fans.iter().any(|r| r.contains(&li)) {
                    continue;
                }
                let led_r = match d.segment_of_led(li) {
                    Some(s) => {
                        let s = &d.segments[s];
                        led_radius(&s.placement, s.leds.len()).min(5.5)
                    }
                    None => body_r,
                };
                let c = device_colors
                    .and_then(|cs| cs.get(li).copied())
                    .map(|c| to_iced(level.map_or(c, |f| f.shown(c))))
                    .unwrap_or(with_alpha(r.outline, 0.35));
                let center = to_screen(&rect, x, y);
                if d.enabled && device_colors.is_some() {
                    // Glow.
                    leds.fill(
                        &Path::circle(center, led_r * 2.3),
                        Color { a: 0.2 * a, ..c },
                    );
                }
                leds.fill(&Path::circle(center, led_r + 1.0), with_alpha(ring, a));
                leds.fill(&Path::circle(center, led_r), with_alpha(c, a));
            }
        }
        vec![background, leds.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &DragState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> mouse::Interaction {
        if state.dragging.is_some() || state.rotating.is_some() {
            return mouse::Interaction::Grabbing;
        }
        if state.resizing.is_some() {
            return mouse::Interaction::ResizingDiagonallyDown;
        }
        if self.editable
            && let Some(pos) = cursor.position_in(bounds)
        {
            let rect = scene_rect(bounds.size());
            if let Some((_, p)) = self.shown_selection() {
                if knob_pos(&rect, &p).1.distance(pos) <= HANDLE * 1.8 {
                    return mouse::Interaction::Grab;
                }
                if handle_pos(&rect, &p).distance(pos) <= HANDLE * 1.8 {
                    return mouse::Interaction::ResizingDiagonallyDown;
                }
            }
            let (sx, sy) = to_scene(&rect, pos);
            if self
                .scene
                .hit_test(sx, sy, self.hidden, self.selected)
                .is_some()
            {
                return mouse::Interaction::Grab;
            }
        }
        mouse::Interaction::default()
    }
}
