//! Home: live setup canvas (SignalRGB-style overview), the active effect
//! with its brightness, and quick stats.

use super::canvas::SetupCanvas;
use super::widgets::{
    self, badge, body, icon, icon_filled, icon_label_button, muted, small, status_dot, title_medium,
};
use super::{icons, page_header};
use crate::app::{App, Message, Page};
use crate::theme::style::{self, ButtonKind, Surface};
use crate::theme::tokens::{shape, space, typescale};
use iced::widget::{column, container, row, text};
use iced::{Alignment, Element, Length, Padding};
use std::sync::atomic::Ordering;
use tuxrgb_openrgb::BridgeStatus;

pub fn effect_icon(kind: tuxrgb_core::EffectKind) -> char {
    use tuxrgb_core::EffectKind::*;
    match kind {
        Static => icons::CIRCLE,
        Breathing => icons::AIR,
        RainbowWave => icons::WAVES,
        ColorCycle => icons::AUTORENEW,
        GradientWave => icons::GRADIENT,
        Aurora => icons::FLARE,
        Starfield => icons::NIGHTS_STAY,
        AudioVisualizer => icons::GRAPHIC_EQ,
        ScreenSync => icons::SCREENSHOT_MONITOR,
        Plasma => icons::BLUR_ON,
        Fire => icons::LOCAL_FIRE_DEPARTMENT,
        Comet => icons::AUTO_AWESOME_MOTION,
        Scanner => icons::RADAR,
        Rain => icons::WATER_DROP,
        Bubbles => icons::BUBBLE_CHART,
        Swirl => icons::CYCLONE,
        Lightning => icons::THUNDERSTORM,
        Shader => icons::CODE,
        HardwareMonitor => icons::MONITOR_HEART,
    }
}

pub fn status_chip(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let (key, color, label_text) = match &app.bridge_status {
        BridgeStatus::Connected { .. } => (1, r.success, app.status_text()),
        BridgeStatus::Detecting { seconds } => {
            (2, r.warning, trf!("Détection du matériel… {} s", seconds))
        }
        BridgeStatus::Error(_) | BridgeStatus::NoServerBinary => (3, r.error, app.status_text()),
        _ => (4, r.warning, app.status_text()),
    };
    // Another state of the engine: dot and text fade in.
    badge(
        super::transition::swap(
            key,
            style::surface_colors(r, Surface::High).0,
            row![status_dot(color), small(label_text)]
                .spacing(space::S)
                .align_y(Alignment::Center),
        ),
        r,
        Surface::High,
    )
}

/// Global saturation, 0–200 % (every effect, every device).
fn saturation_row(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let fg = r.on_primary_container;
    let sat = app.config.saturation;
    row![
        icon(icons::COLORIZE, 22.0).color(fg),
        super::motion::slider(0.0..=2.0, sat, Message::SetSaturation)
            .step(0.01)
            .stops(&[1.0])
            .colors(super::motion::SliderColors::on_container(
                r,
                r.primary_container
            ))
            .height(28.0),
        text(format!("{:.0} %", sat * 100.0))
            .size(16.0)
            .color(fg)
            .width(Length::Fixed(56.0)),
    ]
    .spacing(space::M)
    .align_y(Alignment::Center)
    .into()
}

fn hero(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let kind = app.config.effect;
    let pct = (app.config.brightness * 100.0).round() as u32;
    let fg = r.on_primary_container;
    let key = tuxrgb_core::EffectKind::ALL
        .iter()
        .position(|k| *k == kind)
        .unwrap_or_default() as u64;
    // Another effect (from the gallery, the tray…): name and description
    // fade in instead of switching.
    let identity = super::transition::swap(
        key,
        r.primary_container,
        column![
            row![
                super::motion::shape_badge(effect_icon(kind), 68.0, 9, r.primary, r.on_primary),
                column![
                    small("EFFET ACTIF")
                        .color(fg)
                        .font(widgets::weight(iced::font::Weight::Bold)),
                    text(crate::i18n::t(kind.name()))
                        .size(typescale::HEADLINE_LARGE)
                        .font(widgets::weight(iced::font::Weight::Bold))
                        .color(fg),
                ]
                .spacing(space::XXS),
            ]
            .spacing(space::L)
            .align_y(Alignment::Center),
            body(kind.description()).color(fg),
        ]
        .spacing(space::L),
    );
    let content = column![
        identity,
        row![
            icon(icons::BRIGHTNESS_6, 22.0).color(fg),
            super::motion::slider(0.0..=1.0, app.config.brightness, Message::SetBrightness)
                .step(0.01)
                .colors(super::motion::SliderColors::on_container(
                    r,
                    r.primary_container
                ))
                .height(28.0),
            text(format!("{pct} %"))
                .size(16.0)
                .color(fg)
                .width(Length::Fixed(56.0)),
        ]
        .spacing(space::M)
        .align_y(Alignment::Center),
        saturation_row(app),
        row![
            icon_label_button(
                icons::AUTO_AWESOME,
                "Changer d'effet",
                ButtonKind::Filled,
                r,
                Some(Message::Navigate(Page::Effects))
            ),
            if app.config.output_enabled {
                icon_label_button(
                    icons::PAUSE,
                    "Éteindre",
                    ButtonKind::Tonal,
                    r,
                    Some(Message::SetOutput(false)),
                )
            } else {
                icon_label_button(
                    icons::PLAY_ARROW,
                    "Rallumer",
                    ButtonKind::Tonal,
                    r,
                    Some(Message::SetOutput(true)),
                )
            },
        ]
        .spacing(space::S)
        .wrap()
        .vertical_spacing(space::S),
    ]
    .spacing(space::L);
    container(content)
        .padding(space::XL)
        .width(Length::Fill)
        .style(style::card(*r, Surface::Primary, shape::EXTRA_EXTRA_LARGE))
        .into()
}

fn stat<'a>(app: &'a App, glyph: char, value: String, caption: String) -> Element<'a, Message> {
    let r = &app.roles;
    container(
        row![
            container(icon_filled(glyph, 22.0))
                .center_x(Length::Fixed(44.0))
                .center_y(Length::Fixed(44.0))
                .style(style::card(*r, Surface::Highest, shape::LARGE)),
            column![
                text(value)
                    .size(typescale::TITLE_LARGE)
                    .font(widgets::weight(iced::font::Weight::Bold)),
                small(caption).color(r.on_surface_variant)
            ]
            .spacing(space::XXS),
        ]
        .spacing(space::L)
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([space::M, space::L]))
    .width(Length::Fill)
    .style(style::card(*r, Surface::Default, shape::EXTRA_LARGE))
    .into()
}

fn stats(app: &App) -> Element<'_, Message> {
    let real: Vec<_> = app
        .scene
        .devices
        .iter()
        .filter(|d| !d.info.is_virtual)
        .collect();
    let leds: usize = real
        .iter()
        .filter(|d| d.enabled && d.info.controllable)
        .map(|d| d.info.led_count)
        .sum();
    let (engine_value, engine_caption) = match &app.services {
        Some(s) if s.stats.animating.load(Ordering::Relaxed) == 1 => {
            let ms = s.stats.render_ns.load(Ordering::Relaxed) as f64 / 1e6;
            (
                trf!("{} i/s", s.stats.fps_target.load(Ordering::Relaxed)),
                trf!("moteur d'effets, {} ms par image", format!("{ms:.2}")),
            )
        }
        Some(_) => (
            crate::i18n::t("En veille").to_owned(),
            crate::i18n::t("moteur d'effets (effet statique : 0 % CPU)").to_owned(),
        ),
        None => (
            crate::i18n::t("Démarrage").to_owned(),
            crate::i18n::t("moteur d'effets").to_owned(),
        ),
    };
    column![
        stat(
            app,
            icons::DEVICES,
            real.len().to_string(),
            crate::i18n::t("périphériques détectés").to_owned()
        ),
        stat(
            app,
            icons::LIGHTBULB,
            leds.to_string(),
            crate::i18n::t("LED pilotées").to_owned()
        ),
        stat(app, icons::SPEED, engine_value, engine_caption),
    ]
    .spacing(space::S)
    .into()
}

/// One problem / missing component with its direct action.
pub fn issue_banner(app: &App, issue: crate::app::Issue) -> Element<'_, Message> {
    let r = &app.roles;
    let fg = if issue.info {
        r.on_secondary_container
    } else {
        r.on_tertiary_container
    };
    let mut actions = column![].spacing(space::XS).align_x(Alignment::Center);
    if let Some((glyph, caption, msg)) = issue.action {
        actions = actions.push(icon_label_button(
            glyph,
            caption,
            ButtonKind::Filled,
            r,
            Some(msg),
        ));
    }
    if let Some((caption, msg)) = issue.secondary {
        actions = actions.push(widgets::text_button(
            caption,
            ButtonKind::Text,
            r,
            Some(msg),
        ));
    }
    container(
        row![
            icon_filled(issue.glyph, 28.0).color(fg),
            column![title_medium(issue.heading), body(issue.body)]
                .spacing(space::XXS)
                .width(Length::Fill),
            actions,
        ]
        .spacing(space::L)
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([space::L, space::XL]))
    .style(style::card(
        *r,
        if issue.info {
            Surface::Secondary
        } else {
            Surface::Tertiary
        },
        shape::EXTRA_LARGE,
    ))
    .into()
}

/// Home banners (chosen by `App::home_issues_of`): each one expands and fades
/// in when it appears, collapses and fades out when it no longer applies.
fn banners(app: &App) -> Element<'_, Message> {
    let bg = app.roles.surface;
    iced::widget::keyed_column(app.home_banners.items.iter().map(|p| {
        let banner = container(issue_banner(app, p.value.clone())).padding(Padding {
            bottom: space::XL,
            ..Padding::ZERO
        });
        (
            p.key,
            super::transition::reveal(!p.closing, bg, banner)
                .appear(p.fresh())
                .on_hidden(Message::BannerHidden(p.key))
                .into(),
        )
    }))
    .into()
}

fn canvas_card(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    container(
        column![
            row![
                icon_filled(icons::GRID_VIEW, 22.0).color(r.primary),
                title_medium("Canevas du setup"),
                widgets::hfill(),
                muted(
                    if app.using_demo {
                        trf!("Démonstration · {} LED", app.scene.total_leds())
                    } else {
                        trf!("{} LED", app.scene.total_leds())
                    },
                    r
                ),
                widgets::icon_label_button(
                    icons::OPEN_WITH,
                    "Disposer",
                    ButtonKind::Outlined,
                    r,
                    Some(Message::Navigate(Page::Devices))
                ),
            ]
            .spacing(space::M)
            .align_y(Alignment::Center),
            super::canvas::view(
                SetupCanvas {
                    scene: &app.scene,
                    frame: app.preview.as_ref(),
                    roles: *r,
                    selected: None,
                    hidden: &app.config.hidden_on_canvas,
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
        .spacing(space::M),
    )
    .padding(space::XL)
    .width(Length::FillPortion(5))
    .height(Length::Fill)
    .style(style::card(
        *r,
        Surface::Default,
        shape::EXTRA_LARGE_INCREASED,
    ))
    .into()
}

pub fn view(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let header = page_header(
        app,
        "Mon setup",
        "Votre éclairage, en direct.",
        row![
            status_chip(app),
            widgets::round_icon_button(icons::REFRESH, r, false, Some(Message::Rescan)),
        ]
        .spacing(space::S)
        .align_y(Alignment::Center)
        .into(),
    );
    let side = column![hero(app), stats(app)]
        .spacing(space::L)
        .width(Length::FillPortion(3));
    // Banners carry their own bottom margin: collapsing one also closes
    // its gap.
    column![
        header,
        iced::widget::Space::new().height(space::XL),
        banners(app),
        row![canvas_card(app), side]
            .spacing(space::L)
            .height(Length::Fill),
    ]
    .into()
}
