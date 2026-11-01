//! HSV colour picker drawn on a canvas: saturation/value pad + hue rail.

use crate::app::Message;
use crate::theme::{Roles, to_iced, with_alpha};
use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Event, Frame, Geometry, Path, Stroke, gradient};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme, border};
use tuxrgb_core::Rgb;

pub struct ColorPicker {
    pub color: Rgb,
    pub roles: Roles,
}

#[derive(Default)]
pub struct PickerState {
    drag: Option<Area>,
    /// Hue kept separately so it survives fully desaturated colours.
    hue: Option<f32>,
}

#[derive(Clone, Copy, PartialEq)]
enum Area {
    Pad,
    Hue,
}

const RAIL_W: f32 = 28.0;
const GAP: f32 = 16.0;

fn layout(b: Size) -> (Rectangle, Rectangle) {
    let pad = Rectangle {
        x: 0.0,
        y: 0.0,
        width: (b.width - RAIL_W - GAP).max(10.0),
        height: b.height,
    };
    let rail = Rectangle {
        x: pad.width + GAP,
        y: 0.0,
        width: RAIL_W,
        height: b.height,
    };
    (pad, rail)
}

impl ColorPicker {
    fn hsv(&self, state: &PickerState) -> (f32, f32, f32) {
        let (h, s, v) = self.color.to_hsv();
        (
            state.hue.filter(|_| s < 0.02 || v < 0.02).unwrap_or(h),
            s,
            v,
        )
    }
}

impl canvas::Program<Message> for ColorPicker {
    type State = PickerState;

    fn update(
        &self,
        state: &mut PickerState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<canvas::Action<Message>> {
        let (pad, rail) = layout(bounds.size());
        let pos = || {
            cursor
                .position()
                .map(|p| Point::new(p.x - bounds.x, p.y - bounds.y))
        };
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let p = cursor.position_in(bounds)?;
                state.drag = if pad.contains(p) {
                    Some(Area::Pad)
                } else if rail.contains(p) {
                    Some(Area::Hue)
                } else {
                    None
                };
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.drag = None;
                return None;
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) if state.drag.is_some() => {}
            _ => return None,
        }
        let p = pos()?;
        let (h, s, v) = self.hsv(state);
        let new = match state.drag? {
            Area::Pad => {
                let s = ((p.x - pad.x) / pad.width).clamp(0.0, 1.0);
                let v = 1.0 - ((p.y - pad.y) / pad.height).clamp(0.0, 1.0);
                Rgb::from_hsv(h, s, v)
            }
            Area::Hue => {
                let h = ((p.y - rail.y) / rail.height).clamp(0.0, 0.999);
                state.hue = Some(h);
                Rgb::from_hsv(h, s.max(0.05), v.max(0.05))
            }
        };
        Some(canvas::Action::publish(Message::PickerChanged(new)).and_capture())
    }

    fn draw(
        &self,
        state: &PickerState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut f = Frame::new(renderer, bounds.size());
        let (pad, rail) = layout(bounds.size());
        let (h, s, v) = self.hsv(state);
        let radius = border::Radius::new(16.0);

        // Saturation (left→right) over the pure hue, then value (top→bottom).
        let pad_path = Path::rounded_rectangle(pad.position(), pad.size(), radius);
        let hue_color = to_iced(Rgb::from_hsv(h, 1.0, 1.0));
        f.fill(
            &pad_path,
            gradient::Linear::new(Point::new(pad.x, 0.0), Point::new(pad.x + pad.width, 0.0))
                .add_stop(0.0, Color::WHITE)
                .add_stop(1.0, hue_color),
        );
        f.fill(
            &pad_path,
            gradient::Linear::new(Point::new(0.0, pad.y), Point::new(0.0, pad.y + pad.height))
                .add_stop(0.0, Color::TRANSPARENT)
                .add_stop(1.0, Color::BLACK),
        );
        // Hue rail.
        let mut g = gradient::Linear::new(
            Point::new(0.0, rail.y),
            Point::new(0.0, rail.y + rail.height),
        );
        for i in 0..7 {
            let t = i as f32 / 6.0;
            g = g.add_stop(t, to_iced(Rgb::from_hsv(t.min(0.999), 1.0, 1.0)));
        }
        f.fill(
            &Path::rounded_rectangle(
                rail.position(),
                rail.size(),
                border::Radius::new(RAIL_W / 2.0),
            ),
            g,
        );
        // Handles.
        let pad_handle = Point::new(pad.x + s * pad.width, pad.y + (1.0 - v) * pad.height);
        f.fill(&Path::circle(pad_handle, 11.0), Color::WHITE);
        f.fill(&Path::circle(pad_handle, 8.0), to_iced(self.color));
        f.stroke(
            &Path::circle(pad_handle, 11.0),
            Stroke::default()
                .with_width(1.0)
                .with_color(with_alpha(Color::BLACK, 0.3)),
        );
        let hy = rail.y + h * rail.height;
        let knob = Path::rounded_rectangle(
            Point::new(rail.x - 3.0, hy - 5.0),
            Size::new(RAIL_W + 6.0, 10.0),
            border::Radius::new(5.0),
        );
        f.fill(&knob, self.roles.surface_container_lowest);
        f.stroke(
            &knob,
            Stroke::default()
                .with_width(2.0)
                .with_color(self.roles.on_surface),
        );
        vec![f.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &PickerState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> mouse::Interaction {
        if state.drag.is_some() || cursor.is_over(bounds) {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
    }
}
