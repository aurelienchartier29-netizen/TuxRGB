//! Widget styles implementing M3 Expressive on top of iced.
//!
//! Each function returns a closure capturing the (cheap, `Copy`) colour roles
//! so it can be passed straight to `.style(...)`.

use super::tokens::shape;
use super::{Roles, mix, with_alpha};
use iced::widget::button as w_button;
use iced::widget::container;
use iced::{Background, Border, Color, Shadow, Theme, Vector, border};

/// M3 state layer opacities.
const HOVER: f32 = 0.08;
const PRESS: f32 = 0.12;

fn layer(base: Color, on: Color, status: w_button::Status) -> Color {
    match status {
        w_button::Status::Hovered => mix(base, on, HOVER),
        w_button::Status::Pressed => mix(base, on, PRESS),
        _ => base,
    }
}

fn disabled_bg(r: &Roles) -> Color {
    with_alpha(r.on_surface, 0.12)
}

fn disabled_fg(r: &Roles) -> Color {
    with_alpha(r.on_surface, 0.38)
}

pub fn elevation(r: &Roles, level: u8) -> Shadow {
    let (y, blur, a) = match level {
        0 => return Shadow::default(),
        1 => (1.0, 3.0, 0.15),
        2 => (2.0, 6.0, 0.18),
        3 => (4.0, 12.0, 0.20),
        _ => (8.0, 24.0, 0.24),
    };
    Shadow {
        color: with_alpha(r.shadow, if r.dark { a * 1.8 } else { a }),
        offset: Vector::new(0.0, y),
        blur_radius: blur,
    }
}

/// Expressive shape morph: pill at rest, squarer while pressed.
fn morph_radius(status: w_button::Status, rest: f32, pressed: f32) -> border::Radius {
    if status == w_button::Status::Pressed {
        pressed.into()
    } else {
        rest.into()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    Filled,
    Tonal,
    Outlined,
    Text,
    Danger,
}

pub fn button(r: Roles, kind: ButtonKind) -> impl Fn(&Theme, w_button::Status) -> w_button::Style {
    move |_, status| {
        let (bg, fg, border_color, shadow) = match kind {
            ButtonKind::Filled => (Some(r.primary), r.on_primary, Color::TRANSPARENT, 0),
            ButtonKind::Tonal => (
                Some(r.secondary_container),
                r.on_secondary_container,
                Color::TRANSPARENT,
                0,
            ),
            ButtonKind::Outlined => (None, r.primary, r.outline_variant, 0),
            ButtonKind::Text => (None, r.primary, Color::TRANSPARENT, 0),
            ButtonKind::Danger => (Some(r.error), r.on_error, Color::TRANSPARENT, 0),
        };
        if status == w_button::Status::Disabled {
            return w_button::Style {
                background: bg.map(|_| Background::Color(disabled_bg(&r))),
                text_color: disabled_fg(&r),
                border: Border {
                    color: if border_color == Color::TRANSPARENT {
                        border_color
                    } else {
                        disabled_bg(&r)
                    },
                    width: if kind == ButtonKind::Outlined {
                        1.0
                    } else {
                        0.0
                    },
                    radius: shape::FULL.into(),
                },
                shadow: Shadow::default(),
                snap: true,
            };
        }
        let base = bg.unwrap_or(Color::TRANSPARENT);
        let layered = match bg {
            Some(b) => layer(b, fg, status),
            None => match status {
                w_button::Status::Hovered => with_alpha(fg, HOVER),
                w_button::Status::Pressed => with_alpha(fg, PRESS),
                _ => base,
            },
        };
        let level = if status == w_button::Status::Hovered && bg.is_some() {
            shadow + 1
        } else {
            shadow
        };
        w_button::Style {
            background: Some(Background::Color(layered)),
            text_color: fg,
            border: Border {
                color: border_color,
                width: if kind == ButtonKind::Outlined {
                    1.0
                } else {
                    0.0
                },
                radius: morph_radius(status, shape::FULL, shape::MEDIUM),
            },
            shadow: if kind == ButtonKind::Filled && status == w_button::Status::Hovered {
                elevation(&r, 1)
            } else {
                elevation(&r, level)
            },
            snap: true,
        }
    }
}

/// Floating action button (M3 Expressive: 20 dp squircle, 12 dp pressed).
pub fn fab(r: Roles, on: bool) -> impl Fn(&Theme, w_button::Status) -> w_button::Style {
    move |theme, status| {
        let kind = if on {
            ButtonKind::Filled
        } else {
            ButtonKind::Tonal
        };
        let mut s = button(r, kind)(theme, status);
        s.border.radius = if status == w_button::Status::Pressed {
            shape::MEDIUM.into()
        } else {
            shape::LARGE_INCREASED.into()
        };
        s.shadow = elevation(
            &r,
            if status == w_button::Status::Hovered {
                3
            } else {
                2
            },
        );
        s
    }
}

/// Standard / toggle icon button (40 dp, round → squircle when selected).
pub fn icon_button(
    r: Roles,
    selected: bool,
) -> impl Fn(&Theme, w_button::Status) -> w_button::Style {
    move |_, status| {
        let (bg, fg) = if selected {
            (r.primary, r.on_primary)
        } else {
            (Color::TRANSPARENT, r.on_surface_variant)
        };
        let bg = if selected {
            layer(bg, fg, status)
        } else {
            match status {
                w_button::Status::Hovered => with_alpha(r.on_surface_variant, HOVER),
                w_button::Status::Pressed => with_alpha(r.on_surface_variant, PRESS),
                _ => bg,
            }
        };
        w_button::Style {
            background: Some(Background::Color(bg)),
            text_color: if status == w_button::Status::Disabled {
                disabled_fg(&r)
            } else {
                fg
            },
            border: Border {
                radius: if selected {
                    shape::MEDIUM.into()
                } else {
                    morph_radius(status, shape::FULL, shape::MEDIUM)
                },
                ..Border::default()
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Navigation rail destination; the indicator pill is drawn by the caller.
pub fn nav_item(r: Roles) -> impl Fn(&Theme, w_button::Status) -> w_button::Style {
    move |_, status| {
        // Hover / press state layer (faded in by the motion button).
        let layer = match status {
            w_button::Status::Hovered => 0.06,
            w_button::Status::Pressed => 0.1,
            _ => 0.0,
        };
        w_button::Style {
            background: (layer > 0.0).then(|| Background::Color(with_alpha(r.on_surface, layer))),
            text_color: r.on_surface_variant,
            border: Border {
                radius: shape::LARGE.into(),
                ..Border::default()
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Filter / selection chip.
pub fn chip(r: Roles, selected: bool) -> impl Fn(&Theme, w_button::Status) -> w_button::Style {
    move |_, status| {
        let (bg, fg, border_color) = if selected {
            (
                r.secondary_container,
                r.on_secondary_container,
                Color::TRANSPARENT,
            )
        } else {
            (Color::TRANSPARENT, r.on_surface_variant, r.outline_variant)
        };
        let bg = if selected {
            layer(bg, fg, status)
        } else {
            match status {
                w_button::Status::Hovered => with_alpha(r.on_surface_variant, HOVER),
                w_button::Status::Pressed => with_alpha(r.on_surface_variant, PRESS),
                _ => bg,
            }
        };
        w_button::Style {
            background: Some(Background::Color(bg)),
            text_color: fg,
            border: Border {
                color: border_color,
                width: if selected { 0.0 } else { 1.0 },
                radius: shape::SMALL.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Segment of a connected button group (M3 Expressive): outer corners are
/// full, inner corners small; the selected segment becomes a full pill.
pub fn segment(
    r: Roles,
    selected: bool,
    first: bool,
    last: bool,
) -> impl Fn(&Theme, w_button::Status) -> w_button::Style {
    move |_, status| {
        let (bg, fg) = if selected {
            (r.secondary_container, r.on_secondary_container)
        } else {
            (r.surface_container_high, r.on_surface_variant)
        };
        let inner = if status == w_button::Status::Pressed {
            shape::SMALL
        } else {
            shape::EXTRA_SMALL
        };
        let radius = if selected {
            border::Radius::new(shape::FULL)
        } else {
            border::Radius {
                top_left: if first { shape::FULL } else { inner },
                bottom_left: if first { shape::FULL } else { inner },
                top_right: if last { shape::FULL } else { inner },
                bottom_right: if last { shape::FULL } else { inner },
            }
        };
        w_button::Style {
            background: Some(Background::Color(layer(bg, fg, status))),
            text_color: fg,
            border: Border {
                radius,
                ..Border::default()
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Selectable card (effect gallery, device list).
pub fn card_button(
    r: Roles,
    selected: bool,
    morph: f32,
) -> impl Fn(&Theme, w_button::Status) -> w_button::Style {
    move |_, status| {
        let (bg, fg) = if selected {
            (r.primary_container, r.on_primary_container)
        } else {
            (r.surface_container_high, r.on_surface)
        };
        // Selected cards morph from 16 dp to 32 dp corners (spring driven).
        let radius = shape::LARGE + (shape::EXTRA_LARGE_INCREASED - shape::LARGE) * morph;
        w_button::Style {
            background: Some(Background::Color(layer(bg, fg, status))),
            text_color: fg,
            border: Border {
                color: if selected {
                    with_alpha(r.primary, 0.6)
                } else {
                    Color::TRANSPARENT
                },
                width: if selected { 2.0 } else { 0.0 },
                radius: radius.into(),
            },
            shadow: elevation(
                &r,
                if status == w_button::Status::Hovered {
                    2
                } else {
                    0
                },
            ),
            snap: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // complete M3 surface-container scale
pub enum Surface {
    Lowest,
    Low,
    Default,
    High,
    Highest,
    Primary,
    Secondary,
    Tertiary,
    Error,
    Inverse,
}

/// Rounded container on a surface tone.
pub fn card(r: Roles, surface: Surface, radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        let (bg, fg) = surface_colors(&r, surface);
        container::Style {
            text_color: Some(fg),
            background: Some(Background::Color(bg)),
            border: Border {
                radius: radius.into(),
                ..Border::default()
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

pub fn surface_colors(r: &Roles, s: Surface) -> (Color, Color) {
    match s {
        Surface::Lowest => (r.surface_container_lowest, r.on_surface),
        Surface::Low => (r.surface_container_low, r.on_surface),
        Surface::Default => (r.surface_container, r.on_surface),
        Surface::High => (r.surface_container_high, r.on_surface),
        Surface::Highest => (r.surface_container_highest, r.on_surface),
        Surface::Primary => (r.primary_container, r.on_primary_container),
        Surface::Secondary => (r.secondary_container, r.on_secondary_container),
        Surface::Tertiary => (r.tertiary_container, r.on_tertiary_container),
        Surface::Error => (r.error_container, r.on_error_container),
        Surface::Inverse => (r.inverse_surface, r.inverse_on_surface),
    }
}

/// Plain background for the whole window / pages.
pub fn page(r: Roles) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        text_color: Some(r.on_surface),
        background: Some(Background::Color(r.surface)),
        ..container::Style::default()
    }
}

/// Navigation rail indicator pill.
pub fn indicator(r: Roles, amount: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        text_color: Some(mix(
            r.on_surface_variant,
            r.on_secondary_container,
            amount.clamp(0.0, 1.0),
        )),
        background: Some(Background::Color(with_alpha(
            r.secondary_container,
            amount.clamp(0.0, 1.0),
        ))),
        border: Border {
            radius: shape::FULL.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// Coloured dot / swatch.
pub fn swatch(
    color: Color,
    ring: Option<Color>,
    radius: f32,
) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(color)),
        border: Border {
            color: ring.unwrap_or(Color::TRANSPARENT),
            width: if ring.is_some() { 3.0 } else { 0.0 },
            radius: radius.into(),
        },
        ..container::Style::default()
    }
}

/// Gradient thumbnail of an effect.
pub fn gradient_thumb(colors: Vec<Color>, radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        let mut g = iced::gradient::Linear::new(std::f32::consts::FRAC_PI_2);
        let n = colors.len().max(2) - 1;
        for (i, c) in colors.iter().enumerate().take(8) {
            g = g.add_stop(i as f32 / n as f32, *c);
        }
        container::Style {
            background: Some(Background::Gradient(g.into())),
            border: Border {
                radius: radius.into(),
                ..Border::default()
            },
            ..container::Style::default()
        }
    }
}
