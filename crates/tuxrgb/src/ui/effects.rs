//! Effect gallery + auto-generated settings panel.

use super::canvas::SetupCanvas;
use super::color_picker::ColorPicker;
use super::dashboard::effect_icon;
use super::motion::button;
use super::widgets::{
    self, icon, icon_filled, label, muted, section, segmented, small, switch, title, title_medium,
};
use super::{icons, page_header};
use crate::app::{App, Message};
use crate::services::CaptureStatus;
use crate::theme::style::{self, ButtonKind, Surface};
use crate::theme::to_iced;
use crate::theme::tokens::{shape, space};
use iced::widget::{Space, canvas, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length, Padding};
use tuxrgb_core::color::Palette;
use tuxrgb_core::effects::{Category, KDE_ACCENT_KEY, ParamKind, SYSTEM_PALETTE_KEY};
use tuxrgb_core::{EffectKind, ParamValue, Rgb};

const PRESETS: [Rgb; 10] = [
    Rgb::new(255, 59, 92),
    Rgb::new(255, 145, 0),
    Rgb::new(255, 214, 10),
    Rgb::new(52, 220, 120),
    Rgb::new(0, 210, 190),
    Rgb::new(10, 160, 255),
    Rgb::new(90, 95, 255),
    Rgb::new(170, 80, 255),
    Rgb::new(255, 80, 200),
    Rgb::new(255, 255, 255),
];

fn effect_card(app: &App, index: usize, kind: EffectKind) -> Element<'_, Message> {
    let r = &app.roles;
    let selected = app.config.effect == kind;
    let morph = app.cards[index].value();
    let thumb_colors = kind
        .thumbnail()
        .iter()
        .map(|c| to_iced(*c))
        .collect::<Vec<_>>();
    let thumb_radius = shape::MEDIUM + (shape::EXTRA_LARGE - shape::MEDIUM) * morph.clamp(0.0, 1.2);
    let thumb = container(
        container(icon_filled(effect_icon(kind), 30.0).color(iced::Color::WHITE))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .height(Length::Fixed(88.0 + 8.0 * morph.clamp(0.0, 1.2)))
    .width(Length::Fill)
    .style(style::gradient_thumb(thumb_colors, thumb_radius));
    let header = row![
        title_medium(kind.name()),
        widgets::hfill(),
        if selected {
            Element::from(icon_filled(icons::CHECK_CIRCLE, 22.0).color(r.primary))
        } else {
            Space::new().into()
        },
    ]
    .align_y(Alignment::Center);
    button(
        column![
            thumb,
            header,
            small(kind.description()).color(r.on_surface_variant)
        ]
        .spacing(space::S),
    )
    .padding(space::M)
    .width(Length::FillPortion(1))
    .style(style::card_button(*r, selected, morph.clamp(0.0, 1.2)))
    .on_press(Message::SelectEffect(kind))
    .into()
}

fn gallery(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let mut col = column![].spacing(space::XL);
    for category in [Category::Ambient, Category::Animated, Category::Reactive] {
        let kinds: Vec<(usize, EffectKind)> = EffectKind::ALL
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, k)| k.category() == category)
            .collect();
        let glyph = match category {
            Category::Ambient => icons::LIGHTBULB,
            Category::Animated => icons::WAVES,
            Category::Reactive => icons::EQUALIZER,
        };
        let mut grid = column![].spacing(space::M);
        for chunk in kinds.chunks(3) {
            let mut line = row![].spacing(space::M);
            for (i, k) in chunk {
                line = line.push(effect_card(app, *i, *k));
            }
            for _ in chunk.len()..3 {
                line = line.push(Space::new().width(Length::FillPortion(1)));
            }
            grid = grid.push(line);
        }
        col = col.push(column![section(glyph, category.label(), r), grid].spacing(space::M));
    }
    super::transition::smooth_scroll(
        scrollable(container(col).padding(Padding {
            right: space::L,
            ..Padding::ZERO
        }))
        .height(Length::Fill),
        r,
    )
}

/// Which palette colour replaces a colour parameter.
fn palette_color(p: &Palette, key: &str) -> Rgb {
    match key {
        "color_b" => p.tertiary,
        "background" => p.background,
        "star" => p.secondary,
        _ => p.primary,
    }
}

/// A colour provided by the desktop (accent sync or system palette).
fn system_swatch<'a>(
    app: &'a App,
    title: &'static str,
    caption: String,
    c: Rgb,
) -> Element<'a, Message> {
    let r = &app.roles;
    row![
        column![label(title), small(caption).color(r.on_surface_variant)]
            .spacing(space::XXS)
            .width(Length::Fill),
        text(c.to_hex().to_uppercase())
            .size(13.0)
            .font(iced::Font::MONOSPACE)
            .color(r.on_surface_variant),
        container(Space::new().width(36).height(36)).style({
            let col = to_iced(c);
            let ring = r.primary;
            move |_| iced::widget::container::Style {
                background: Some(iced::Background::Color(col)),
                border: iced::Border {
                    color: ring,
                    width: 3.0,
                    radius: 18.0.into(),
                },
                ..Default::default()
            }
        }),
    ]
    .spacing(space::M)
    .align_y(Alignment::Center)
    .into()
}

/// The three palette colours (+ background) as a pill preview.
fn palette_preview<'a>(app: &'a App, p: Palette) -> Element<'a, Message> {
    let r = &app.roles;
    let dot = |c: Rgb, d: f32| {
        container(Space::new().width(d).height(d)).style({
            let col = to_iced(c);
            let ring = r.outline_variant;
            move |_| iced::widget::container::Style {
                background: Some(iced::Background::Color(col)),
                border: iced::Border {
                    color: ring,
                    width: 1.0,
                    radius: (d / 2.0).into(),
                },
                ..Default::default()
            }
        })
    };
    row![
        dot(p.primary, 28.0),
        dot(p.secondary, 24.0),
        dot(p.tertiary, 24.0),
        dot(p.background, 20.0),
        small(trf!(
            "Dominante {} · variante · nuance · fond",
            p.primary.to_hex().to_uppercase()
        ))
        .color(r.on_surface_variant),
    ]
    .spacing(space::S)
    .align_y(Alignment::Center)
    .into()
}

fn param_controls(app: &App, kind: EffectKind) -> Element<'_, Message> {
    let r = &app.roles;
    let params = &app.config.params;
    let mut col = column![].spacing(space::L);
    let accent = app.desktop_accent();
    let kde_sync = kind.supports_kde_accent() && params.toggle(kind, KDE_ACCENT_KEY);
    let palette = kind
        .uses_system_palette(params)
        .then(|| kind.system_palette(params, accent));
    for spec in kind.params() {
        let key = spec.key;
        let overridden = kind.overridden(params, key);
        let control: Element<'_, Message> = match spec.kind {
            // Replaced by the desktop: the accent, or a palette colour.
            ParamKind::Color if overridden => match &palette {
                Some(p) => system_swatch(
                    app,
                    spec.label,
                    crate::i18n::t("Palette Système Harmonieuse, en direct").to_owned(),
                    palette_color(p, key),
                ),
                None if kde_sync => {
                    system_swatch(app, spec.label, crate::desktop::accent_caption(), accent)
                }
                None => continue,
            },
            // Switches and choices that do not apply in this mode.
            ParamKind::Toggle | ParamKind::Choice(_) if overridden => continue,
            ParamKind::Color => {
                let c = params.color(kind, key);
                let open = app.picker == Some((kind, key));
                let mut inner = column![
                    row![
                        label(spec.label),
                        widgets::hfill(),
                        text(c.to_hex().to_uppercase())
                            .size(13.0)
                            .font(iced::Font::MONOSPACE)
                            .color(r.on_surface_variant),
                        button(Space::new().width(36).height(36))
                            .padding(0)
                            .style({
                                let col = to_iced(c);
                                let (ring, hover_ring) = if open {
                                    (r.primary, r.primary)
                                } else {
                                    (r.outline_variant, r.on_surface_variant)
                                };
                                move |_, status| iced::widget::button::Style {
                                    background: Some(iced::Background::Color(col)),
                                    border: iced::Border {
                                        color: match status {
                                            iced::widget::button::Status::Hovered
                                            | iced::widget::button::Status::Pressed => hover_ring,
                                            _ => ring,
                                        },
                                        width: 3.0,
                                        radius: 18.0.into(),
                                    },
                                    ..Default::default()
                                }
                            })
                            .on_press(Message::OpenPicker(kind, key)),
                    ]
                    .spacing(space::M)
                    .align_y(Alignment::Center)
                ]
                .spacing(space::M);
                // Inline sub-menu: expands / collapses with a fade; the one
                // closed last stays until it has collapsed.
                if open || app.picker_last == Some((kind, key)) {
                    inner = inner.push(
                        super::transition::reveal(open, r.surface_container, color_editor(app, c))
                            .on_hidden(Message::PickerHidden(kind, key)),
                    );
                }
                inner.into()
            }
            ParamKind::Range { min, max, step } => {
                let v = params.number(kind, key);
                let fmt = move |v: f32| {
                    if step >= 1.0 {
                        format!("{v:.0}")
                    } else {
                        format!("{v:.2}")
                    }
                };
                column![
                    row![
                        label(spec.label),
                        widgets::hfill(),
                        text(fmt(v)).size(13.0).color(r.on_surface_variant)
                    ],
                    super::motion::slider(min..=max, v, move |n| Message::SetParam(
                        kind,
                        key,
                        ParamValue::Number(n)
                    ))
                    .step(step)
                    .format(fmt)
                    .colors(super::motion::SliderColors::standard(r))
                    .height(40.0),
                ]
                .spacing(space::XS)
                .into()
            }
            ParamKind::Choice(options) => {
                let selected = params.choice_label(kind, key);
                let chooser: Element<'_, Message> = if options.len() <= 3 {
                    let opts: Vec<(&'static str, &'static str)> =
                        options.iter().map(|o| (*o, *o)).collect();
                    segmented(&opts, selected, r, move |o: &'static str| {
                        Message::SetParam(kind, key, ParamValue::Choice(o.to_owned()))
                    })
                } else {
                    let mut wrap = row![].spacing(space::S);
                    for o in options {
                        let is_sel = *o == selected;
                        wrap = wrap.push(
                            button(label(*o))
                                .padding(Padding::from([6.0, 14.0]))
                                .style(style::chip(*r, is_sel))
                                .on_press(Message::SetParam(
                                    kind,
                                    key,
                                    ParamValue::Choice((*o).to_owned()),
                                )),
                        );
                    }
                    wrap.wrap().vertical_spacing(space::S).into()
                };
                let mut c = column![label(spec.label), chooser].spacing(space::S);
                // How the automatic sensor picks a reading for each device.
                if kind == EffectKind::HardwareMonitor
                    && key == "sensor"
                    && selected == "Automatique (par appareil)"
                {
                    c = c.push(
                        small(
                            "Carte graphique → GPU · RAM → mémoire · appareils sans capteur \
                             (clavier, souris, tapis, casque, ventilateurs…) → température CPU, \
                             ou charge CPU si elle est illisible.",
                        )
                        .color(r.on_surface_variant),
                    );
                }
                c.into()
            }
            ParamKind::Toggle => {
                let on = params.toggle(kind, key);
                let caption: Element<'_, Message> = if key == KDE_ACCENT_KEY {
                    column![
                        label(crate::desktop::sync_label()),
                        small(crate::desktop::accent_caption()).color(r.on_surface_variant),
                    ]
                    .spacing(space::XXS)
                    .width(Length::Fill)
                    .into()
                } else if key == SYSTEM_PALETTE_KEY {
                    let mut c = column![
                        label(spec.label),
                        small(trf!(
                            "Couleurs dérivées de l'accent {} : dominante, variante et nuance harmonisée",
                            crate::desktop::session().desktop.short_name()
                        ))
                        .color(r.on_surface_variant),
                    ]
                    .spacing(space::XS)
                    .width(Length::Fill);
                    if let Some(p) = palette {
                        c = c.push(palette_preview(app, p));
                    }
                    c.into()
                } else {
                    label(spec.label).width(Length::Fill).into()
                };
                row![
                    caption,
                    switch(on, r, move |b| Message::SetParam(
                        kind,
                        key,
                        ParamValue::Toggle(b)
                    ))
                ]
                .spacing(space::M)
                .align_y(Alignment::Center)
                .into()
            }
        };
        col = col.push(control);
    }
    col.into()
}

fn color_editor(app: &App, c: Rgb) -> Element<'_, Message> {
    let r = &app.roles;
    let mut swatches = row![].spacing(space::S);
    let accent = app.system.accent.unwrap_or(crate::theme::BREEZE_ACCENT);
    for preset in std::iter::once(accent).chain(PRESETS) {
        swatches = swatches.push(
            button(Space::new().width(26).height(26))
                .padding(0)
                .style({
                    let col = to_iced(preset);
                    let ring = if preset == c {
                        Some(r.on_surface)
                    } else {
                        None
                    };
                    move |_, status| iced::widget::button::Style {
                        background: Some(iced::Background::Color(col)),
                        border: iced::Border {
                            color: ring.unwrap_or(iced::Color::TRANSPARENT),
                            width: if ring.is_some() { 2.5 } else { 0.0 },
                            radius: if status == iced::widget::button::Status::Hovered {
                                8.0.into()
                            } else {
                                13.0.into()
                            },
                        },
                        ..Default::default()
                    }
                })
                .on_press(Message::PickerChanged(preset)),
        );
    }
    container(
        column![
            canvas(ColorPicker {
                color: c,
                roles: *r
            })
            .width(Length::Fill)
            .height(Length::Fixed(170.0)),
            swatches.wrap().vertical_spacing(space::S),
            row![
                text("#").size(14.0).color(r.on_surface_variant),
                super::transition::text_field(
                    text_input("rrggbb", &app.hex_input)
                        .on_input(Message::HexInput)
                        .padding(8)
                        .size(14.0)
                        .font(iced::Font::MONOSPACE),
                    r
                ),
                widgets::text_button("OK", ButtonKind::Tonal, r, Some(Message::ClosePicker)),
            ]
            .spacing(space::S)
            .align_y(Alignment::Center),
        ]
        .spacing(space::M),
    )
    .padding(space::L)
    .style(style::card(*r, Surface::Highest, shape::EXTRA_LARGE))
    .into()
}

fn capture_hint(app: &App, kind: EffectKind) -> Option<Element<'_, Message>> {
    let r = &app.roles;
    let req = kind.requirements();
    let (status, what) = if req.audio {
        (
            &app.audio_status,
            if cfg!(windows) {
                "Capture audio (WASAPI)"
            } else {
                "Capture audio (PipeWire)"
            },
        )
    } else if req.screen {
        (
            &app.screen_status,
            if cfg!(windows) {
                "Capture d'écran (écran principal)"
            } else {
                "Partage d'écran (portail Wayland)"
            },
        )
    } else if req.shader {
        (&app.shader_status, "Rendu GPU du shader")
    } else {
        return None;
    };
    let (key, glyph, txt, color) = match status {
        CaptureStatus::Running => (1, icons::CHECK_CIRCLE, "active", r.success),
        CaptureStatus::Starting => (2, icons::SYNC, "démarrage…", r.warning),
        CaptureStatus::Off => (3, icons::INFO, "inactive", r.on_surface_variant),
        CaptureStatus::Cancelled => (4, icons::WARNING, "refusée", r.error),
        CaptureStatus::Failed(_) => (5, icons::ERROR, "erreur", r.error),
    };
    // Status changes (starting → active…) fade in.
    Some(
        super::transition::swap(
            key,
            r.surface_container,
            row![
                icon_filled(glyph, 18.0).color(color),
                small(format!(
                    "{} : {}",
                    crate::i18n::t(what),
                    crate::i18n::t(txt)
                ))
                .color(r.on_surface_variant)
            ]
            .spacing(space::S)
            .align_y(Alignment::Center),
        )
        .into(),
    )
}

fn panel(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let kind = app.config.effect;
    let mut col = column![
        row![
            super::motion::shape_badge(
                effect_icon(kind),
                56.0,
                7,
                r.secondary_container,
                r.on_secondary_container
            ),
            column![title(kind.name()), muted("Réglages de l'effet", r)].spacing(space::XXS),
            widgets::hfill(),
            widgets::round_icon_button(
                icons::SETTINGS_BACKUP_RESTORE,
                r,
                false,
                Some(Message::ResetParams(kind))
            ),
        ]
        .spacing(space::M)
        .align_y(Alignment::Center),
    ]
    .spacing(space::L);
    if let Some(h) = capture_hint(app, kind) {
        col = col.push(h);
    }
    col = col.push(param_controls(app, kind));
    col = col.push(
        column![
            row![
                icon(icons::BRIGHTNESS_6, 20.0),
                label("Luminosité globale"),
                widgets::hfill(),
                text(format!("{:.0} %", app.config.brightness * 100.0))
                    .size(13.0)
                    .color(r.on_surface_variant)
            ]
            .spacing(space::S)
            .align_y(Alignment::Center),
            super::motion::slider(0.0..=1.0, app.config.brightness, Message::SetBrightness)
                .step(0.01)
                .colors(super::motion::SliderColors::standard(r))
                .height(40.0),
            row![
                icon(icons::COLORIZE, 20.0),
                label("Saturation globale"),
                widgets::hfill(),
                text(format!("{:.0} %", app.config.saturation * 100.0))
                    .size(13.0)
                    .color(r.on_surface_variant)
            ]
            .spacing(space::S)
            .align_y(Alignment::Center),
            super::motion::slider(0.0..=2.0, app.config.saturation, Message::SetSaturation)
                .step(0.01)
                .stops(&[1.0])
                .colors(super::motion::SliderColors::standard(r))
                .height(40.0),
            small("0 % = blanc · 100 % = couleurs d'origine · 200 % = ultra-vives")
                .color(r.on_surface_variant),
        ]
        .spacing(space::XS),
    );
    // Live preview: about 45 % of the panel height, as wide as the panel
    // (16:9 canvas), with device names.
    let preview = column![
        row![
            icon_filled(icons::VISIBILITY, 18.0).color(r.primary),
            label("Aperçu en direct").color(r.primary),
        ]
        .spacing(space::S)
        .align_y(Alignment::Center),
        super::canvas::view(
            SetupCanvas {
                scene: &app.scene,
                frame: app.preview.as_ref(),
                roles: *r,
                selected: None,
                editable: false,
                show_labels: true,
                screen_hint: app.config.effect == tuxrgb_core::EffectKind::ScreenSync,
                cache: &app.canvas_cache,
                backdrop: app.backdrop.as_ref(),
                appear: super::canvas::appear(app),
            },
            Length::Fill,
            Length::Fill,
        ),
    ]
    .spacing(space::S)
    .height(Length::FillPortion(4));
    // Another effect: its settings fade in instead of switching at once.
    let key = EffectKind::ALL
        .iter()
        .position(|k| *k == kind)
        .unwrap_or_default() as u64;
    let col = super::transition::swap(key, r.surface_container, col);
    container(
        column![
            super::transition::smooth_scroll(
                scrollable(container(col).padding(Padding {
                    right: space::M,
                    ..Padding::ZERO
                }))
                .height(Length::FillPortion(5)),
                r,
            ),
            preview,
        ]
        .spacing(space::L),
    )
    .padding(space::XL)
    .width(Length::FillPortion(3))
    .height(Length::Fill)
    .style(style::card(
        *r,
        Surface::Default,
        shape::EXTRA_LARGE_INCREASED,
    ))
    .into()
}

pub fn view(app: &App) -> Element<'_, Message> {
    let header = page_header(
        app,
        "Effets",
        "Choisissez un effet : il est appliqué instantanément à tout le setup.",
        row![super::dashboard::status_chip(app)].into(),
    );
    column![
        header,
        row![
            container(gallery(app)).width(Length::FillPortion(4)),
            panel(app)
        ]
        .spacing(space::XL)
        .height(Length::Fill),
    ]
    .spacing(space::XL)
    .into()
}
