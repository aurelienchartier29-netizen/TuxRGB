//! TuxRGB's own title bar (Linux, 4.0): in the colours of the app instead of
//! the desktop's grey bar. The window is opened without decorations; the
//! bar moves it (press), maximises it (double click), and its buttons
//! minimise, maximise / restore and close it. Thin invisible borders resize
//! the window (edges and corners), as a decorated window would.

use super::icons;
use super::widgets::{icon, small};
use crate::app::{App, Message, Page};
use crate::theme::tokens::space;
use crate::theme::with_alpha;
use iced::widget::{Space, container, mouse_area, row, stack};
use iced::{Alignment, Background, Color, Element, Length, Padding, mouse, window};

/// Height of the bar.
pub const HEIGHT: f32 = 40.0;
/// Width of the resize borders.
const GRIP: f32 = 6.0;

fn page_name(page: Page) -> &'static str {
    match page {
        Page::Home => "Accueil",
        Page::Effects => "Effets",
        Page::Devices => "Appareils",
        Page::Displays => "Afficheurs",
        Page::Fans => "Ventilation",
        Page::Settings => "Réglages",
    }
}

/// Width of one window button (the whole height of the bar, edge to edge
/// like the desktops' own: the close button reaches the corner of the
/// screen, so a click thrown into the top-right corner closes the window).
const BUTTON_W: f32 = 46.0;

/// One of the three window buttons: a flat cell, tinted on hover; the close
/// one turns red.
fn window_button<'a>(
    glyph: char,
    app: &App,
    danger: bool,
    on_press: Message,
) -> Element<'a, Message> {
    let r = app.roles;
    iced::widget::button(
        container(icon(glyph, 18.0))
            .center_x(Length::Fixed(BUTTON_W))
            .center_y(Length::Fixed(HEIGHT)),
    )
    .padding(0)
    .style(move |_, status| {
        use iced::widget::button::Status;
        let hot = matches!(status, Status::Hovered | Status::Pressed);
        let (bg, fg) = match (danger, status) {
            (true, Status::Pressed) => (with_alpha(r.error, 0.80), r.on_error),
            (true, _) if hot => (r.error, r.on_error),
            (false, Status::Pressed) => (with_alpha(r.on_surface, 0.16), r.on_surface),
            (false, _) if hot => (with_alpha(r.on_surface, 0.09), r.on_surface),
            _ => (Color::TRANSPARENT, r.on_surface_variant),
        };
        iced::widget::button::Style {
            background: Some(Background::Color(bg)),
            text_color: fg,
            ..iced::widget::button::Style::default()
        }
    })
    .on_press(on_press)
    .into()
}

/// The bar itself.
pub fn bar(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let title = match &app.wizard {
        Some(_) => crate::i18n::t("Assistant de configuration").to_owned(),
        None => crate::i18n::t(page_name(app.page)).to_owned(),
    };
    let drag = mouse_area(
        container(
            row![
                super::logo(20.0),
                small("TuxRGB")
                    .font(super::widgets::weight(iced::font::Weight::Bold))
                    .color(r.on_surface),
                small("·").color(r.on_surface_variant),
                small(title).color(r.on_surface_variant),
            ]
            .spacing(space::S)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([0.0, space::L]))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_y(Alignment::Center),
    )
    .on_press(Message::WindowDrag)
    .on_double_click(Message::WindowToggleMaximize);
    let maximize = if app.window_maximized {
        icons::FILTER_NONE
    } else {
        icons::CROP_SQUARE
    };
    let bg = r.surface_container;
    container(
        row![
            drag,
            window_button(icons::MINIMIZE, app, false, Message::WindowMinimize),
            window_button(maximize, app, false, Message::WindowToggleMaximize),
            window_button(icons::CLOSE, app, true, Message::WindowCloseButton),
        ]
        .align_y(Alignment::Center),
    )
    .height(Length::Fixed(HEIGHT))
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: Some(Background::Color(bg)),
        ..container::Style::default()
    })
    .into()
}

/// An invisible strip that resizes the window from one side or corner.
fn grip<'a>(
    w: Length,
    h: Length,
    cursor: mouse::Interaction,
    direction: window::Direction,
) -> Element<'a, Message> {
    mouse_area(
        container(Space::new().width(w).height(h)).style(|_| container::Style {
            background: Some(Background::Color(Color::TRANSPARENT)),
            ..container::Style::default()
        }),
    )
    .interaction(cursor)
    .on_press(Message::WindowDragResize(direction))
    .into()
}

/// The window's content under the bar, with the resize borders on top
/// (none while maximised) and a hairline outline.
pub fn frame<'a>(app: &'a App, content: Element<'a, Message>) -> Element<'a, Message> {
    let r = &app.roles;
    let body = iced::widget::column![bar(app), content];
    if app.window_maximized {
        return body.into();
    }
    use iced::widget::column;
    use mouse::Interaction::{ResizingDiagonallyDown as Nwse, ResizingDiagonallyUp as Nesw};
    use mouse::Interaction::{ResizingHorizontally as Ew, ResizingVertically as Ns};
    use window::Direction::*;
    let g = Length::Fixed(GRIP);
    // No resize border over the window buttons: their corner closes the
    // window, the right edge resizes from under the bar.
    let edges = column![
        row![
            grip(g, g, Nwse, NorthWest),
            grip(Length::Fill, g, Ns, North),
            Space::new().width(BUTTON_W * 3.0).height(g),
        ],
        row![
            grip(g, Length::Fill, Ew, West),
            Space::new().width(Length::Fill).height(Length::Fill),
            column![
                Space::new().width(g).height(HEIGHT - GRIP),
                grip(g, Length::Fill, Ew, East),
            ],
        ]
        .height(Length::Fill),
        row![
            grip(g, g, Nesw, SouthWest),
            grip(Length::Fill, g, Ns, South),
            grip(g, g, Nwse, SouthEast),
        ],
    ];
    let outline = with_alpha(r.outline_variant, 0.9);
    stack![
        container(body).style(move |_| container::Style {
            border: iced::Border {
                color: outline,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        }),
        edges,
    ]
    .into()
}
