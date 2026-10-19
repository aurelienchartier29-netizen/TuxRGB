//! The setup canvas: SignalRGB-style 2D view of the user's rig with live LED
//! colours (the native "Visual Map"). In edit mode devices can be dragged
//! around and resized from their corner handle; rotation and mirroring are
//! buttons of the Devices page. The canvas is also the screen: with Screen
//! Sync each LED takes the colour of the part of the screen it covers, which
//! the canvas shows as a screen frame and, for the selected device, the
//! sampled area of every LED.
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
use std::collections::HashMap;
use std::sync::Arc;
use tuxrgb_core::engine::Frame;
use tuxrgb_core::layout::SCENE_ASPECT;
use tuxrgb_core::{Placement, Scene};

pub struct SetupCanvas<'a> {
    pub scene: &'a Scene,
    pub frame: Option<&'a Arc<Frame>>,
    pub roles: Roles,
    pub selected: Option<usize>,
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
    dragging: Option<(usize, Vector)>,
    /// Resizing from the bottom-right handle.
    resizing: Option<usize>,
}

/// Radius (px) of the resize handle of the selected device.
const HANDLE: f32 = 7.0;

fn handle_pos(r: &Rectangle, p: &Placement) -> Point {
    to_screen(r, p.x + p.w, p.y + p.h)
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
                // Corner handle of the selected device: resize.
                if self.editable
                    && let Some(i) = self.selected
                    && let Some(d) = self.scene.devices.get(i)
                    && handle_pos(&rect, &d.placement).distance(pos) <= HANDLE * 1.8
                {
                    state.resizing = Some(i);
                    return Some(canvas::Action::capture());
                }
                let (sx, sy) = to_scene(&rect, pos);
                let hit = self.scene.hit_test(sx, sy);
                if let (true, Some(i)) = (self.editable, hit) {
                    let p = self.scene.devices[i].placement;
                    state.dragging = Some((i, Vector::new(sx - p.x, sy - p.y)));
                }
                Some(canvas::Action::publish(Message::CanvasSelect(hit)).and_capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) if state.resizing.is_some() => {
                let index = state.resizing?;
                let pos = cursor.position_in(bounds).or_else(|| {
                    cursor
                        .position()
                        .map(|p| Point::new(p.x - bounds.x, p.y - bounds.y))
                })?;
                let (sx, sy) = to_scene(&rect, pos);
                let current = self.scene.devices.get(index)?.placement;
                let resized = Placement {
                    w: (sx - current.x).clamp(0.02, 1.0 - current.x),
                    h: (sy - current.y).clamp(0.02, 1.0 - current.y),
                    ..current
                };
                Some(canvas::Action::publish(Message::CanvasMove(index, resized)).and_capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let (index, offset) = state.dragging?;
                let pos = cursor.position_in(bounds).or_else(|| {
                    cursor
                        .position()
                        .map(|p| Point::new(p.x - bounds.x, p.y - bounds.y))
                })?;
                let (sx, sy) = to_scene(&rect, pos);
                let current = self.scene.devices.get(index)?.placement;
                let moved = Placement {
                    x: sx - offset.x,
                    y: sy - offset.y,
                    ..current
                };
                Some(canvas::Action::publish(Message::CanvasMove(index, moved)).and_capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
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
            // Device bodies.
            for (i, d) in self.scene.devices.iter().enumerate() {
                let p = d.placement;
                let a = self.appear.get(i).copied().unwrap_or(1.0);
                if a <= 0.001 {
                    continue;
                }
                // Arrival: grows from 85 % around its centre while fading in.
                let k = 0.85 + 0.15 * a;
                let full = Size::new(p.w * rect.width, p.h * rect.height);
                let size = Size::new(full.width * k, full.height * k);
                let c0 = to_screen(&rect, p.x, p.y);
                let tl = Point::new(
                    c0.x + (full.width - size.width) * 0.5,
                    c0.y + (full.height - size.height) * 0.5,
                );
                let radius = (size.width.min(size.height) * 0.22).clamp(4.0, 20.0);
                let body = Path::rounded_rectangle(tl, size, border::Radius::new(radius));
                let selected = self.selected == Some(i);
                // Soft two-step shadow: lifts the device off the glow.
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
                let fill = if !d.enabled || !d.info.controllable {
                    with_alpha(r.surface_container_high, 0.6)
                } else if selected {
                    with_alpha(r.primary_container, 0.92)
                } else {
                    with_alpha(r.surface_container_high, 0.95)
                };
                f.fill(&body, with_alpha(fill, a));
                f.stroke(
                    &body,
                    Stroke::default()
                        .with_width(if selected { 2.5 } else { 1.0 })
                        .with_color(with_alpha(
                            if selected {
                                r.primary
                            } else {
                                r.outline_variant
                            },
                            a,
                        )),
                );
                // One line under the device (never wrapped, never hidden by
                // the LEDs), cut to the device width with an ellipsis; very
                // narrow devices (RAM sticks) only show it when selected.
                let name = d.info.display_name();
                let max_chars = if selected {
                    usize::MAX
                } else {
                    (size.width * 1.1 / 5.8) as usize
                };
                if self.show_labels && max_chars >= 6 {
                    let content = if name.chars().count() > max_chars {
                        let cut: String = name.chars().take(max_chars.saturating_sub(1)).collect();
                        format!("{}…", cut.trim_end())
                    } else {
                        name.to_owned()
                    };
                    let position = Point::new(tl.x + 2.0, tl.y + size.height + 3.0);
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
            }
        });

        // Live LEDs (redrawn at the preview rate).
        let mut leds = CFrame::new(renderer, bounds.size());
        // Selected device in edit mode: sampled areas and the resize handle.
        if let Some(d) = self.selected.and_then(|i| self.scene.devices.get(i)) {
            if self.screen_hint {
                for g in &d.geometry {
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
                let h = handle_pos(&rect, &d.placement);
                leds.fill(&Path::circle(h, HANDLE + 3.0), with_alpha(r.primary, 0.25));
                leds.fill(&Path::circle(h, HANDLE), r.primary);
                leds.fill(&Path::circle(h, HANDLE - 3.0), r.on_primary);
            }
        }
        // Thin ring in the canvas colour around each LED: crisp dots on the
        // glow, whatever the effect.
        let ring = with_alpha(r.surface_container_lowest, 0.85);
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
            let a = self.appear.get(di).copied().unwrap_or(1.0);
            if a <= 0.001 {
                continue;
            }
            let n = d.leds.len().max(1) as f32;
            let p = d.placement;
            // LED size adapts to density so keyboards and strips both read well.
            let area = p.w * rect.width * p.h * rect.height;
            let led_r = ((area / n).sqrt() * 0.32).clamp(1.5, 7.0);
            let device_colors = colors.get(&d.info.id).copied();
            for (li, &(x, y)) in d.leds.iter().enumerate() {
                let c = device_colors
                    .and_then(|cs| cs.get(li).copied())
                    .map(to_iced)
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
        if state.dragging.is_some() {
            return mouse::Interaction::Grabbing;
        }
        if state.resizing.is_some() {
            return mouse::Interaction::ResizingDiagonallyDown;
        }
        if self.editable
            && let Some(pos) = cursor.position_in(bounds)
        {
            let rect = scene_rect(bounds.size());
            if let Some(d) = self.selected.and_then(|i| self.scene.devices.get(i))
                && handle_pos(&rect, &d.placement).distance(pos) <= HANDLE * 1.8
            {
                return mouse::Interaction::ResizingDiagonallyDown;
            }
            let (sx, sy) = to_scene(&rect, pos);
            if self.scene.hit_test(sx, sy).is_some() {
                return mouse::Interaction::Grab;
            }
        }
        mouse::Interaction::default()
    }
}
