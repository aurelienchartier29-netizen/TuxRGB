//! Curve of a fan, drawn on a canvas: speed (%) against temperature (°C),
//! points dragged with the mouse, the current temperature and speed shown,
//! the safety zone (90 °C and above) marked.

use crate::app::Message;
use crate::fans::curve::{Curve, SAFETY_TEMP};
use crate::theme::{Roles, with_alpha};
use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Event, Frame, Geometry, Path, Stroke, Text};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme, alignment};

/// Temperatures shown (°C).
const T_MIN: f32 = 20.0;
const T_MAX: f32 = 100.0;
const LEFT: f32 = 40.0;
const RIGHT: f32 = 14.0;
const TOP: f32 = 12.0;
const BOTTOM: f32 = 26.0;
/// Grab distance of a point, px.
const GRAB: f32 = 14.0;

pub struct CurveEditor {
    pub fan: String,
    pub points: Vec<(u8, u8)>,
    /// Current temperature of the curve's source, °C.
    pub temp: Option<f32>,
    /// Current output of the fan, %.
    pub duty: Option<f32>,
    pub roles: Roles,
    pub editable: bool,
}

#[derive(Default)]
pub struct EditorState {
    drag: Option<usize>,
    hover: Option<usize>,
}

fn plot(b: Size) -> Rectangle {
    Rectangle {
        x: LEFT,
        y: TOP,
        width: (b.width - LEFT - RIGHT).max(10.0),
        height: (b.height - TOP - BOTTOM).max(10.0),
    }
}

fn to_screen(p: Rectangle, t: f32, d: f32) -> Point {
    Point::new(
        p.x + (t.clamp(T_MIN, T_MAX) - T_MIN) / (T_MAX - T_MIN) * p.width,
        p.y + (1.0 - d.clamp(0.0, 100.0) / 100.0) * p.height,
    )
}

fn from_screen(p: Rectangle, s: Point) -> (u8, u8) {
    let t = T_MIN + ((s.x - p.x) / p.width).clamp(0.0, 1.0) * (T_MAX - T_MIN);
    let d = (1.0 - (s.y - p.y) / p.height).clamp(0.0, 1.0) * 100.0;
    (t.round() as u8, d.round() as u8)
}

impl CurveEditor {
    fn nearest(&self, area: Rectangle, at: Point) -> Option<usize> {
        self.points
            .iter()
            .enumerate()
            .map(|(i, (t, d))| {
                (
                    i,
                    to_screen(area, f32::from(*t), f32::from(*d)).distance(at),
                )
            })
            .filter(|(_, dist)| *dist <= GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }
}

impl canvas::Program<Message> for CurveEditor {
    type State = EditorState;

    fn update(
        &self,
        state: &mut EditorState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<canvas::Action<Message>> {
        if !self.editable {
            return None;
        }
        let area = plot(bounds.size());
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let p = cursor.position_in(bounds)?;
                state.drag = self.nearest(area, p);
                state.drag.map(|_| canvas::Action::capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.drag.take().map(|_| canvas::Action::capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let p = cursor
                    .position()
                    .map(|p| Point::new(p.x - bounds.x, p.y - bounds.y))?;
                match state.drag {
                    Some(i) => Some(
                        canvas::Action::publish(Message::FanPoint(
                            self.fan.clone(),
                            i,
                            from_screen(area, p),
                        ))
                        .and_capture(),
                    ),
                    None => {
                        let hover = cursor
                            .position_in(bounds)
                            .and_then(|p| self.nearest(area, p));
                        if hover != state.hover {
                            state.hover = hover;
                            Some(canvas::Action::request_redraw())
                        } else {
                            None
                        }
                    }
                }
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
        let mut f = Frame::new(renderer, bounds.size());
        let area = plot(bounds.size());
        let grid = Stroke::default()
            .with_width(1.0)
            .with_color(with_alpha(r.outline_variant, 0.6));
        let label = |f: &mut Frame, s: String, at: Point, h: alignment::Horizontal| {
            f.fill_text(Text {
                content: s,
                position: at,
                color: r.on_surface_variant,
                size: 11.0.into(),
                align_x: h.into(),
                align_y: alignment::Vertical::Center,
                ..Text::default()
            });
        };

        // Safety zone.
        let z0 = to_screen(area, SAFETY_TEMP, 100.0);
        f.fill_rectangle(
            z0,
            Size::new(area.x + area.width - z0.x, area.height),
            with_alpha(r.error, 0.10),
        );
        // Grid and axes.
        for d in [0.0, 25.0, 50.0, 75.0, 100.0] {
            let a = to_screen(area, T_MIN, d);
            let b = to_screen(area, T_MAX, d);
            f.stroke(&Path::line(a, b), grid);
            label(
                &mut f,
                format!("{d:.0} %"),
                Point::new(a.x - 6.0, a.y),
                alignment::Horizontal::Right,
            );
        }
        for t in [20.0, 40.0, 60.0, 80.0, 100.0] {
            let a = to_screen(area, t, 0.0);
            let b = to_screen(area, t, 100.0);
            f.stroke(&Path::line(a, b), grid);
            label(
                &mut f,
                format!("{t:.0} °C"),
                Point::new(a.x, a.y + 14.0),
                alignment::Horizontal::Center,
            );
        }

        // The curve, flat before the first point and after the last.
        if let (Some(first), Some(last)) = (self.points.first(), self.points.last()) {
            let mut pts = vec![to_screen(area, T_MIN, f32::from(first.1))];
            pts.extend(
                self.points
                    .iter()
                    .map(|(t, d)| to_screen(area, f32::from(*t), f32::from(*d))),
            );
            pts.push(to_screen(area, T_MAX, f32::from(last.1)));
            let line = Path::new(|b| {
                b.move_to(pts[0]);
                for p in &pts[1..] {
                    b.line_to(*p);
                }
            });
            let fill = Path::new(|b| {
                b.move_to(to_screen(area, T_MIN, 0.0));
                for p in &pts {
                    b.line_to(*p);
                }
                b.line_to(to_screen(area, T_MAX, 0.0));
                b.close();
            });
            f.fill(&fill, with_alpha(r.primary, 0.14));
            f.stroke(
                &line,
                Stroke::default()
                    .with_width(3.0)
                    .with_color(if self.editable { r.primary } else { r.outline }),
            );
            for (i, (t, d)) in self.points.iter().enumerate() {
                let c = to_screen(area, f32::from(*t), f32::from(*d));
                let active = state.drag == Some(i) || state.hover == Some(i);
                let radius = if active { 9.0 } else { 6.5 };
                f.fill(&Path::circle(c, radius), r.primary);
                f.fill(&Path::circle(c, radius - 3.0), r.on_primary);
                if active {
                    label(
                        &mut f,
                        format!("{t} °C · {d} %"),
                        Point::new(c.x, (c.y - 18.0).max(area.y + 6.0)),
                        alignment::Horizontal::Center,
                    );
                }
            }
        }

        // Where the fan is now.
        if let Some(t) = self.temp {
            let d = self.duty.unwrap_or_else(|| {
                Curve {
                    source: String::new(),
                    points: self.points.clone(),
                }
                .duty_at(t)
            });
            let top = to_screen(area, t, 100.0);
            let bottom = to_screen(area, t, 0.0);
            f.stroke(
                &Path::line(top, bottom),
                Stroke::default()
                    .with_width(1.5)
                    .with_color(with_alpha(r.tertiary, 0.8)),
            );
            let dot = to_screen(area, t, d);
            f.fill(&Path::circle(dot, 7.0), r.tertiary);
            f.fill(&Path::circle(dot, 3.0), Color::WHITE);
        }
        vec![f.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &EditorState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> mouse::Interaction {
        if !self.editable {
            return mouse::Interaction::default();
        }
        if state.drag.is_some() {
            return mouse::Interaction::Grabbing;
        }
        match cursor.position_in(bounds) {
            Some(p) if self.nearest(plot(bounds.size()), p).is_some() => mouse::Interaction::Grab,
            _ => mouse::Interaction::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_and_values_agree() {
        let area = plot(Size::new(440.0, 220.0));
        let p = to_screen(area, 60.0, 50.0);
        assert_eq!(from_screen(area, p), (60, 50));
        assert_eq!(from_screen(area, Point::new(-100.0, -100.0)), (20, 100));
        assert_eq!(from_screen(area, Point::new(9999.0, 9999.0)), (100, 0));
    }
}
