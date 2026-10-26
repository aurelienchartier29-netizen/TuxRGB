//! "Afficheurs" page (TuxRGB 3.1): the screens of AIO coolers and the
//! digital displays of air coolers and cases. Each display found has a card
//! with a live preview of what it shows and its settings: on / off, what it
//! shows (sensor values, a text, a picture, its own screen), which values,
//! how often, brightness and orientation of the screens.

use super::motion::button;
use super::widgets::{self, badge, icon, label, small, switch, title_medium};
use super::{empty_state, icons, page_header};
use crate::app::{App, Message};
use crate::displays::{Content, DisplayState, Metric, Surface};
use crate::theme::style::{self, ButtonKind, Surface as Tone};
use crate::theme::tokens::{shape, space};
use iced::widget::{Space, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length, Padding};

/// Status chip of a display.
fn status<'a>(app: &'a App, d: &DisplayState) -> Element<'a, Message> {
    let r = &app.roles;
    let (caption, tone) = if d.demo {
        ("Démo", Tone::Secondary)
    } else if !d.connected {
        ("Débranché", Tone::Highest)
    } else if d.error.is_some() {
        ("Erreur", Tone::Tertiary)
    } else if d.driving {
        ("Actif", Tone::Primary)
    } else {
        ("Inactif", Tone::Highest)
    };
    badge(small(caption), r, tone)
}

/// The picture of the display (or its kind's icon while there is none).
fn preview<'a>(app: &'a App, d: &DisplayState) -> Element<'a, Message> {
    let r = &app.roles;
    let (w, h) = match d.caps.surface {
        Surface::Screen { .. } => (200.0, 200.0),
        Surface::Matrix => (180.0, 180.0),
        Surface::Digits => (220.0, 92.0),
    };
    let content: Element<'a, Message> = match app.display_previews.get(&d.key) {
        Some((_, handle)) => iced::widget::image(handle.clone())
            .width(Length::Fixed(w))
            .height(Length::Fixed(h))
            .filter_method(iced::widget::image::FilterMethod::Linear)
            .into(),
        None => container(icon(icons::BROWSE_ACTIVITY, 40.0).color(r.on_surface_variant))
            .center_x(Length::Fixed(w))
            .center_y(Length::Fixed(h))
            .into(),
    };
    container(content)
        .center_x(Length::Fixed(232.0))
        .center_y(Length::Fixed(h.max(150.0)))
        .into()
}

/// Values shown right now, as a line ("CPU · Temp. 63 °C  ·  …").
fn live_values(d: &DisplayState, fahrenheit: bool) -> Option<String> {
    let parts: Vec<String> = d
        .shown
        .iter()
        .filter(|(m, _)| *m != Metric::Time)
        .map(|(m, v)| {
            let (value, unit) = m.format(*v, fahrenheit);
            format!("{} {} {}", crate::i18n::t(m.label()), value, unit)
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("  ·  "))
}

fn slider_row<'a>(
    app: &'a App,
    glyph: char,
    caption: &'a str,
    value_text: String,
    slider: Element<'a, Message>,
) -> Element<'a, Message> {
    let r = &app.roles;
    column![
        row![
            icon(glyph, 18.0).color(r.on_surface_variant),
            label(caption),
            widgets::hfill(),
            text(value_text).size(13.0).color(r.on_surface_variant),
        ]
        .spacing(space::S)
        .align_y(Alignment::Center),
        slider,
    ]
    .spacing(space::XXS)
    .into()
}

fn card<'a>(app: &'a App, d: &'a DisplayState) -> Element<'a, Message> {
    let r = &app.roles;
    let caps = &d.caps;
    let settings = app.config.displays.settings_of(&d.key, caps);
    // The demo displays play their part until the user changes them.
    let enabled = if d.demo && !app.config.displays.displays.contains_key(&d.key) {
        true
    } else {
        settings.enabled
    };
    let key = d.key.clone();
    let header = row![
        column![
            title_medium(d.name.as_str()),
            small(format!(
                "{} · {}",
                d.vendor,
                match caps.surface {
                    Surface::Screen {
                        width,
                        height,
                        round,
                    } => {
                        let shape = if round {
                            crate::i18n::t("rond")
                        } else {
                            crate::i18n::t("carré")
                        };
                        trf!("Écran {} × {} {}", width, height, shape)
                    }
                    Surface::Matrix => crate::i18n::t("Matrice de points 14 × 14").to_owned(),
                    Surface::Digits => crate::i18n::t("Afficheur numérique").to_owned(),
                }
            ))
            .color(r.on_surface_variant),
        ]
        .spacing(space::XXS)
        .width(Length::Fill),
        status(app, d),
        switch(enabled, r, {
            let key = key.clone();
            move |on| Message::DisplayEnabled(key.clone(), on)
        }),
    ]
    .spacing(space::M)
    .align_y(Alignment::Center);

    let mut controls = column![header].spacing(space::M);
    if let Some(note) = caps.note {
        controls = controls.push(badge(small(note), r, Tone::Tertiary));
    }
    if let Some(e) = &d.error {
        let mut line = row![
            icon(icons::ERROR, 16.0).color(r.error),
            small(e.as_str()).color(r.error).width(Length::Fill),
        ]
        .spacing(space::XS)
        .align_y(Alignment::Center);
        // No access (udev rules from before 3.1): the setup installs it.
        if cfg!(target_os = "linux") && d.access_denied {
            line = line.push(widgets::text_button(
                "Installer l'accès",
                ButtonKind::Tonal,
                r,
                (!app.helper_busy).then_some(Message::StartHardwareSetup),
            ));
        }
        controls = controls.push(line);
    }

    // What it shows.
    let mut contents: Vec<(Content, &'static str)> = Vec::new();
    if !caps.metrics.is_empty() {
        contents.push((Content::Metrics, "Mesures"));
    }
    if caps.text {
        contents.push((Content::Text, "Texte"));
    }
    if caps.image {
        contents.push((Content::Image, "Image"));
    }
    if caps.builtin.is_some() {
        contents.push((Content::Builtin, "Écran intégré"));
    }
    let content = if contents.iter().any(|(c, _)| *c == settings.content) {
        settings.content
    } else {
        contents.first().map_or(Content::Metrics, |(c, _)| *c)
    };
    if contents.len() > 1 {
        controls = controls.push(widgets::segmented(&contents, content, r, {
            let key = key.clone();
            move |c| Message::DisplayContent(key.clone(), c)
        }));
    }
    match content {
        Content::Metrics if caps.fixed => {
            controls = controls.push(
                small(
                    "Cet afficheur montre lui-même toutes ses valeurs (température, charge, puissance du CPU…).",
                )
                .color(r.on_surface_variant),
            );
        }
        Content::Metrics => {
            let mut chips = row![].spacing(space::XS);
            for m in caps.metrics {
                let position = settings.metrics.iter().position(|x| x == m);
                let caption = crate::i18n::t(m.label()).to_owned();
                let inner: Element<'_, Message> = match position {
                    Some(i) => row![
                        container(text((i + 1).to_string()).size(11.0))
                            .center_x(Length::Fixed(16.0))
                            .center_y(Length::Fixed(16.0))
                            .style(style::card(*r, Tone::Primary, 8.0)),
                        small(caption)
                    ]
                    .spacing(space::XS)
                    .align_y(Alignment::Center)
                    .into(),
                    None => small(caption).into(),
                };
                chips = chips.push(
                    button(inner)
                        .padding(Padding::from([6.0, 12.0]))
                        .style(style::chip(*r, position.is_some()))
                        .on_press(Message::DisplayMetric(key.clone(), *m)),
                );
            }
            controls = controls.push(chips.wrap().vertical_spacing(space::XS));
            let chosen = settings
                .metrics
                .iter()
                .filter(|m| caps.metrics.contains(m))
                .count();
            let at_once = caps.at_once.max(1);
            controls = controls.push(
                small(if chosen > at_once {
                    trf!(
                        "{} à la fois : les autres alternent toutes les {} s.",
                        at_once,
                        settings.cycle_s
                    )
                } else if at_once == 1 {
                    crate::i18n::t(
                        "Une valeur à la fois : choisissez-en plusieurs pour qu'elles alternent.",
                    )
                    .to_owned()
                } else {
                    trf!("Jusqu'à {} valeurs à la fois.", at_once)
                })
                .color(r.on_surface_variant),
            );
            if chosen > at_once {
                controls = controls.push(slider_row(
                    app,
                    icons::SYNC,
                    "Alternance",
                    trf!("{} s", settings.cycle_s),
                    super::motion::slider(2.0..=30.0, settings.cycle_s as f32, {
                        let key = key.clone();
                        move |v| Message::DisplayCycle(key.clone(), v)
                    })
                    .step(1.0)
                    .colors(super::motion::SliderColors::standard(r))
                    .height(36.0)
                    .into(),
                ));
            }
        }
        Content::Text => {
            controls = controls.push(super::transition::text_field(
                text_input(crate::i18n::t("Texte à afficher"), &settings.text)
                    .on_input({
                        let key = key.clone();
                        move |t| Message::DisplayText(key.clone(), t)
                    })
                    .padding(10)
                    .size(14.0),
                r,
            ));
        }
        Content::Image => {
            let path = settings
                .image
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            controls = controls.push(
                row![
                    super::transition::text_field(
                        text_input(
                            crate::i18n::t("Chemin de l'image (PNG, JPEG, GIF, WebP)"),
                            &path
                        )
                        .on_input({
                            let key = key.clone();
                            move |t| Message::DisplayImagePath(key.clone(), t)
                        })
                        .padding(10)
                        .size(14.0),
                        r,
                    ),
                    widgets::icon_label_button(
                        icons::FOLDER_OPEN,
                        "Parcourir…",
                        ButtonKind::Tonal,
                        r,
                        Some(Message::DisplayPickImage(key.clone())),
                    ),
                ]
                .spacing(space::S)
                .align_y(Alignment::Center),
            );
        }
        Content::Builtin => {
            controls = controls.push(
                small(trf!(
                    "L'écran affiche son propre contenu : {}.",
                    crate::i18n::t(caps.builtin.unwrap_or(""))
                ))
                .color(r.on_surface_variant),
            );
        }
    }

    // How often, brightness, orientation, alarm.
    let min = caps.min_interval.as_secs_f32().max(0.25);
    let seconds = (settings.interval_ms as f32 / 1000.0).clamp(min, 10.0);
    controls = controls.push(slider_row(
        app,
        icons::TIMER,
        "Rafraîchissement",
        trf!(
            "toutes les {} s",
            format!("{seconds:.2}")
                .trim_end_matches('0')
                .trim_end_matches('.')
        ),
        super::motion::slider(min..=10.0, seconds, {
            let key = key.clone();
            move |v| Message::DisplayInterval(key.clone(), v)
        })
        .step(0.25)
        .colors(super::motion::SliderColors::standard(r))
        .height(36.0)
        .into(),
    ));
    if caps.brightness {
        controls = controls.push(slider_row(
            app,
            icons::BRIGHTNESS_6,
            "Luminosité de l'écran",
            format!("{} %", settings.brightness),
            super::motion::slider(0.0..=1.0, f32::from(settings.brightness) / 100.0, {
                let key = key.clone();
                move |v| Message::DisplayBrightness(key.clone(), v)
            })
            .step(0.01)
            .colors(super::motion::SliderColors::standard(r))
            .height(36.0)
            .into(),
        ));
    }
    if caps.rotation {
        controls = controls.push(
            row![
                icon(icons::ROTATE_RIGHT, 18.0).color(r.on_surface_variant),
                label("Orientation").width(Length::Fill),
                widgets::segmented(
                    &[(0u16, "0°"), (90, "90°"), (180, "180°"), (270, "270°")],
                    settings.rotation,
                    r,
                    {
                        let key = key.clone();
                        move |v| Message::DisplayRotation(key.clone(), v)
                    }
                ),
            ]
            .spacing(space::S)
            .align_y(Alignment::Center),
        );
    }
    if caps.alarm {
        controls = controls.push(
            row![
                icon(icons::THERMOSTAT, 18.0).color(r.on_surface_variant),
                label("Alerte au-delà de 90 °C").width(Length::Fill),
                switch(settings.alarm, r, {
                    let key = key.clone();
                    move |on| Message::DisplayAlarm(key.clone(), on)
                }),
            ]
            .spacing(space::S)
            .align_y(Alignment::Center),
        );
    }
    if let Some(values) = live_values(d, app.config.displays.fahrenheit) {
        controls = controls.push(
            row![
                icon(icons::SENSORS, 16.0).color(r.primary),
                small(values)
                    .color(r.on_surface_variant)
                    .width(Length::Fill),
            ]
            .spacing(space::XS)
            .align_y(Alignment::Center),
        );
    }

    container(
        row![
            container(preview(app, d))
                .padding(space::S)
                .style(style::card(*r, Tone::Lowest, shape::LARGE)),
            controls.width(Length::Fill),
        ]
        .spacing(space::XL),
    )
    .padding(space::XL)
    .style(style::card(*r, Tone::Low, shape::EXTRA_LARGE))
    .into()
}

pub fn view(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let real = app.displays.iter().filter(|d| !d.demo).count();
    let supporting = if app.displays.iter().any(|d| d.demo) && real == 0 {
        crate::i18n::t("Aucun afficheur détecté pour l'instant – afficheurs virtuels affichés.")
            .to_owned()
    } else {
        crate::i18n::t(
            "Écrans des watercoolings AIO et afficheurs numériques des ventirads et des boîtiers.",
        )
        .to_owned()
    };
    let header = page_header(
        app,
        "Afficheurs",
        supporting,
        row![
            widgets::segmented(
                &[(false, "°C"), (true, "°F")],
                app.config.displays.fahrenheit,
                r,
                Message::DisplaysFahrenheit
            ),
            widgets::icon_label_button(
                icons::REFRESH,
                "Détecter",
                ButtonKind::Tonal,
                r,
                Some(Message::DisplaysRescan)
            ),
        ]
        .spacing(space::S)
        .align_y(Alignment::Center)
        .into(),
    );
    let supported = small(
        "Pris en charge : DeepCool (AK, AG, LS, LD, LP, LQ, AK Digital Pro, boîtiers CH), \
         NZXT Kraken Z, 2023 et 2024 (écran LCD), Corsair iCUE ELITE LCD, iCUE LINK LCD, \
         XC7 ELITE LCD et Nautilus LCD.",
    )
    .color(r.on_surface_variant);
    let credits = small(
        "Protocoles libres repris de liquidctl, deepcool-digital-linux et OpenLinkHub (GPL-3.0). \
         Fermez NZXT CAM, iCUE ou le logiciel DeepCool : un seul programme à la fois.",
    )
    .color(r.on_surface_variant);
    let mut notes = column![supported, credits].spacing(space::S);
    if cfg!(windows) {
        notes = notes.push(
            small(
                "Sous Windows, la charge et la température du GPU viennent de Windows \
                 (comme dans le Gestionnaire des tâches) ; la température et la puissance \
                 du CPU sont lues par le pilote PawnIO en mode administrateur, ou dans \
                 LibreHardwareMonitor s'il est ouvert.",
            )
            .color(r.on_surface_variant),
        );
    }
    let body: Element<'_, Message> = if app.displays.is_empty() {
        column![
            empty_state(
                app,
                icons::BROWSE_ACTIVITY,
                "Aucun afficheur",
                "Branchez le câble USB de l'écran ou de l'afficheur sur la carte mère, puis lancez une recherche.",
            ),
            container(notes).padding(space::XL),
        ]
        .into()
    } else {
        let mut col = column![].spacing(space::L);
        for d in &app.displays {
            col = col.push(card(app, d));
        }
        col = col.push(container(notes).padding(Padding {
            left: space::S,
            right: space::S,
            ..Padding::ZERO
        }));
        col = col.push(Space::new().height(space::XL));
        super::transition::smooth_scroll(
            scrollable(container(col).padding(Padding {
                right: space::M,
                ..Padding::ZERO
            }))
            .height(Length::Fill),
            r,
        )
    };
    column![header, Space::new().height(space::XL), body]
        .height(Length::Fill)
        .into()
}
