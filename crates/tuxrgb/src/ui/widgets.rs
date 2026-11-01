//! Reusable M3 Expressive building blocks.

use super::motion::button;
use crate::app::Message;
use crate::i18n::Txt;
use crate::theme::Roles;
use crate::theme::style::{self, ButtonKind, Surface};
use crate::theme::tokens::{shape, space, typescale};
use iced::font::Weight;
use iced::widget::{Space, column, container, row, text};
use iced::{Alignment, Element, Font, Length, Padding};

pub const SYMBOLS: Font = Font::with_name("TuxRGB Symbols");
pub const SYMBOLS_FILL: Font = Font::with_name("TuxRGB Symbols Fill");

pub fn weight(w: Weight) -> Font {
    Font {
        weight: w,
        ..Font::DEFAULT
    }
}

pub fn icon<'a>(glyph: char, size: f32) -> text::Text<'a> {
    text(glyph.to_string())
        .font(SYMBOLS)
        .size(size)
        .line_height(1.0)
}

pub fn icon_filled<'a>(glyph: char, size: f32) -> text::Text<'a> {
    text(glyph.to_string())
        .font(SYMBOLS_FILL)
        .size(size)
        .line_height(1.0)
}

pub fn display<'a>(s: impl Txt<'a>) -> text::Text<'a> {
    text(s.tr())
        .size(typescale::DISPLAY_SMALL)
        .font(weight(Weight::Bold))
}

/// Page title: M3 Expressive "headline large emphasized".
pub fn headline<'a>(s: impl Txt<'a>) -> text::Text<'a> {
    text(s.tr())
        .size(typescale::HEADLINE_LARGE)
        .font(weight(Weight::Bold))
}

pub fn title<'a>(s: impl Txt<'a>) -> text::Text<'a> {
    text(s.tr())
        .size(typescale::TITLE_LARGE)
        .font(weight(Weight::Semibold))
}

pub fn title_medium<'a>(s: impl Txt<'a>) -> text::Text<'a> {
    text(s.tr())
        .size(typescale::TITLE_MEDIUM)
        .font(weight(Weight::Semibold))
}

pub fn body<'a>(s: impl Txt<'a>) -> text::Text<'a> {
    text(s.tr()).size(typescale::BODY_MEDIUM)
}

pub fn label<'a>(s: impl Txt<'a>) -> text::Text<'a> {
    text(s.tr())
        .size(typescale::LABEL_LARGE)
        .font(weight(Weight::Medium))
}

pub fn small<'a>(s: impl Txt<'a>) -> text::Text<'a> {
    text(s.tr()).size(typescale::BODY_SMALL)
}

pub fn muted<'a>(s: impl Txt<'a>, r: &Roles) -> text::Text<'a> {
    let c = r.on_surface_variant;
    text(s.tr()).size(typescale::BODY_MEDIUM).color(c)
}

/// Button with a leading icon (M3 Expressive: pill, morphs when pressed).
pub fn icon_label_button<'a>(
    glyph: char,
    caption: &'a str,
    kind: ButtonKind,
    r: &Roles,
    on_press: Option<Message>,
) -> super::motion::MotionButton<'a, Message> {
    button(
        row![icon(glyph, 20.0), label(caption)]
            .spacing(space::S)
            .align_y(Alignment::Center),
    )
    .padding(Padding::from([10.0, 24.0]))
    .style(style::button(*r, kind))
    .on_press_maybe(on_press)
}

pub fn text_button<'a>(
    caption: &'a str,
    kind: ButtonKind,
    r: &Roles,
    on_press: Option<Message>,
) -> super::motion::MotionButton<'a, Message> {
    button(label(caption))
        .padding(Padding::from([10.0, 24.0]))
        .style(style::button(*r, kind))
        .on_press_maybe(on_press)
}

pub fn round_icon_button<'a>(
    glyph: char,
    r: &Roles,
    selected: bool,
    on_press: Option<Message>,
) -> super::motion::MotionButton<'a, Message> {
    button(
        container(if selected {
            icon_filled(glyph, 22.0)
        } else {
            icon(glyph, 22.0)
        })
        .center_x(Length::Fixed(24.0))
        .center_y(Length::Fixed(24.0)),
    )
    .padding(8)
    .style(style::icon_button(*r, selected))
    .on_press_maybe(on_press)
}

/// Connected button group (M3 Expressive segmented control).
pub fn segmented<'a, T: Copy + PartialEq + 'a>(
    options: &[(T, &'a str)],
    selected: T,
    r: &Roles,
    on_select: impl Fn(T) -> Message + 'a,
) -> Element<'a, Message> {
    let n = options.len();
    let mut items = row![].spacing(space::XXS);
    for (i, (value, caption)) in options.iter().enumerate() {
        let is_sel = *value == selected;
        let content: Element<'a, Message> = if is_sel {
            row![icon(crate::ui::icons::CHECK, 18.0), label(*caption)]
                .spacing(space::XS)
                .align_y(Alignment::Center)
                .into()
        } else {
            label(*caption).into()
        };
        items = items.push(
            button(content)
                .padding(Padding::from([8.0, 16.0]))
                .style(style::segment(*r, is_sel, i == 0, i + 1 == n))
                .on_press(on_select(*value)),
        );
    }
    items.into()
}

/// A settings row: title + supporting text on the left, control on the right.
pub fn setting_row<'a>(
    glyph: char,
    heading: impl Txt<'a>,
    supporting: impl Txt<'a>,
    control: impl Into<Element<'a, Message>>,
    r: &Roles,
) -> Element<'a, Message> {
    row![
        container(icon(glyph, 24.0).color(r.on_surface_variant))
            .center_x(Length::Fixed(40.0))
            .center_y(Length::Fixed(40.0)),
        column![
            title_medium(heading),
            muted(supporting, r).size(typescale::BODY_SMALL)
        ]
        .spacing(space::XXS)
        .width(Length::Fill),
        control.into(),
    ]
    .spacing(space::L)
    .align_y(Alignment::Center)
    .padding(Padding::from([space::M, 0.0]))
    .into()
}

/// M3 switch with a spring-animated thumb (see `ui::motion`).
pub fn switch<'a>(on: bool, r: &Roles, f: impl Fn(bool) -> Message + 'a) -> Element<'a, Message> {
    super::motion::switch(
        on,
        super::motion::SwitchColors {
            track_on: r.primary,
            track_off: r.surface_container_highest,
            outline_off: r.outline,
            thumb_on: r.on_primary,
            thumb_off: r.outline,
            icon_on: r.on_primary_container,
            state_layer: crate::theme::with_alpha(r.on_surface, 0.10),
        },
        f,
    )
    .into()
}

/// Section header inside a page.
pub fn section<'a>(glyph: char, heading: &'a str, r: &Roles) -> Element<'a, Message> {
    row![
        icon_filled(glyph, 22.0).color(r.primary),
        title_medium(heading).color(r.primary)
    ]
    .spacing(space::S)
    .align_y(Alignment::Center)
    .into()
}

/// Small rounded label (status chip, counters).
pub fn badge<'a>(
    content: impl Into<Element<'a, Message>>,
    r: &Roles,
    surface: Surface,
) -> Element<'a, Message> {
    container(content)
        .padding(Padding::from([4.0, 12.0]))
        .style(style::card(*r, surface, shape::FULL))
        .into()
}

pub fn status_dot<'a>(color: iced::Color) -> Element<'a, Message> {
    container(Space::new().width(10).height(10))
        .style(style::swatch(color, None, 5.0))
        .into()
}

pub fn hfill<'a>() -> Element<'a, Message> {
    Space::new().width(Length::Fill).into()
}

/// A coloured surface whose colours ease into their new values when it
/// changes (status cards): a motion button without action, whose disabled
/// look follows the style like any other button state.
pub fn animated_surface<'a, Message: Clone + 'a>(
    content: impl Into<Element<'a, Message>>,
    r: &Roles,
    surface: Surface,
    radius: f32,
    padding: impl Into<iced::Padding>,
) -> Element<'a, Message> {
    let (bg, fg) = style::surface_colors(r, surface);
    button(content)
        .width(Length::Fill)
        .padding(padding)
        .style(move |_, _| iced::widget::button::Style {
            background: Some(iced::Background::Color(bg)),
            text_color: fg,
            border: iced::Border {
                radius: radius.into(),
                ..iced::Border::default()
            },
            ..Default::default()
        })
        .into()
}
