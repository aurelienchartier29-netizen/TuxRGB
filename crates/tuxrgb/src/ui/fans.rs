//! "Ventilation" page (TuxRGB 3.2): every fan of the PC with its speed, and
//! how it is driven – left to the motherboard, a fixed speed, or a curve
//! following a temperature (drawn and edited on a canvas).

use super::fan_curve::CurveEditor;
use super::motion::button;
use super::widgets::{self, badge, icon, label, small, title_medium};
use super::{icons, page_header};
use crate::app::{App, Message};
use crate::fans::{Access, FanInfo, FanKind, FanMode, FanPrefs, FanStatus, source_label};
use crate::theme::style::{self, ButtonKind, Surface as Tone};
use crate::theme::tokens::{shape, space};
use iced::widget::{Space, canvas, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length, Padding};

fn status_badge<'a>(app: &'a App, f: &FanInfo) -> Element<'a, Message> {
    let r = &app.roles;
    let (caption, tone) = match f.status {
        FanStatus::Auto if !f.controllable => ("Lecture seule", Tone::Highest),
        FanStatus::Auto => ("Carte mère", Tone::Highest),
        FanStatus::Driven => ("Piloté", Tone::Primary),
        FanStatus::Safety => ("Sécurité 100 %", Tone::Tertiary),
        FanStatus::NoSensor => ("Capteur perdu", Tone::Tertiary),
        FanStatus::Refused => ("Refusé par le pilote", Tone::Tertiary),
    };
    badge(small(caption), r, tone)
}

/// The card telling what is missing before the fans can be driven.
fn access_card(app: &App) -> Option<Element<'_, Message>> {
    let r = &app.roles;
    let (heading, body, action): (&str, String, Option<&str>) = match &app.fans.access {
        Access::Ready => return None,
        Access::Starting => (
            "Recherche des ventilateurs…",
            String::new(),
            None,
        ),
        Access::ServiceMissing => (
            "Activer le contrôle des ventilateurs",
            crate::i18n::t(
                "TuxRGB installe une fois un petit service système (mot de passe administrateur) : il applique vos réglages dès le démarrage du PC et rend les ventilateurs à la carte mère s'il s'arrête. Le pilote du capteur de la carte mère (it87, nct6775) est chargé s'il manque.",
            )
            .to_owned(),
            Some("Activer"),
        ),
        Access::ServiceOutdated(v) => (
            "Mettre à jour le service des ventilateurs",
            crate::i18n::trf!(
                "Le service installé vient de TuxRGB {} : mettez-le à jour pour cette version.",
                v
            ),
            Some("Mettre à jour"),
        ),
        Access::NeedsAdmin => (
            "Mode administrateur nécessaire",
            crate::i18n::t(
                "Sous Windows, régler les ventilateurs de la carte mère demande le pilote PawnIO et le mode administrateur de TuxRGB (Windows ne laisse aucun programme le faire autrement).",
            )
            .to_owned(),
            Some("Configurer l'accès"),
        ),
        Access::NeedsPawnIo => (
            "Pilote PawnIO nécessaire",
            crate::i18n::t(
                "Le pilote PawnIO donne accès au contrôleur des ventilateurs de la carte mère, comme pour l'éclairage de la RAM.",
            )
            .to_owned(),
            Some("Installer PawnIO"),
        ),
    };
    let mut col = column![
        row![
            icon(icons::MODE_FAN, 24.0).color(r.primary),
            title_medium(heading),
        ]
        .spacing(space::S)
        .align_y(Alignment::Center)
    ]
    .spacing(space::S);
    if !body.is_empty() {
        col = col.push(small(body).color(r.on_surface_variant));
    }
    if let Some(a) = action {
        col = col.push(widgets::text_button(
            a,
            ButtonKind::Filled,
            r,
            (!app.helper_busy).then_some(Message::FansEnable),
        ));
    }
    Some(
        container(col)
            .padding(space::XL)
            .width(Length::Fill)
            .style(style::card(*r, Tone::Primary, shape::EXTRA_LARGE))
            .into(),
    )
}

fn fan_card<'a>(app: &'a App, f: &'a FanInfo) -> Element<'a, Message> {
    let r = &app.roles;
    let prefs = app
        .config
        .fans
        .fans
        .get(&f.id)
        .cloned()
        .unwrap_or_else(|| FanPrefs::for_fan(f.kind));
    let name = prefs.name.clone().unwrap_or_else(|| f.default_name());
    let id = f.id.clone();
    // An older service (same protocol) still applies the settings.
    let can_drive =
        f.controllable && matches!(app.fans.access, Access::Ready | Access::ServiceOutdated(_));

    let title: Element<'a, Message> = match &app.fan_rename {
        Some((rid, value)) if *rid == f.id => text_input("", value)
            .on_input(Message::FanRenameInput)
            .on_submit(Message::FanRename(None))
            .padding(Padding::from([6.0, 10.0]))
            .size(16.0)
            .width(Length::Fixed(240.0))
            .into(),
        _ => row![
            title_medium(name),
            button(icon(icons::EDIT, 16.0).color(r.on_surface_variant))
                .padding(4)
                .style(style::icon_button(*r, false))
                .on_press(Message::FanRename(Some(id.clone()))),
        ]
        .spacing(space::XS)
        .align_y(Alignment::Center)
        .into(),
    };
    let place = match f.kind {
        FanKind::Gpu => crate::i18n::t("Carte graphique").to_owned(),
        FanKind::Motherboard => {
            crate::i18n::trf!("Carte mère · {} · connecteur {}", f.chip, f.index)
        }
        FanKind::Other => f.chip.clone(),
    };
    let rpm = match f.rpm {
        Some(v) => crate::i18n::trf!("{} tr/min", v),
        None => "– tr/min".to_owned(),
    };
    let duty = f.duty.map(|d| format!(" · {d:.0} %")).unwrap_or_default();
    let header = row![
        container(icon(icons::MODE_FAN, 28.0).color(r.primary))
            .center_x(Length::Fixed(44.0))
            .center_y(Length::Fixed(44.0))
            .style(style::card(*r, Tone::Primary, 22.0)),
        column![title, small(place).color(r.on_surface_variant)]
            .spacing(space::XXS)
            .width(Length::Fill),
        column![
            text(rpm).size(20.0).color(r.on_surface),
            small(duty.trim_start_matches(" · ").to_owned()).color(r.on_surface_variant),
        ]
        .align_x(Alignment::End)
        .spacing(space::XXS),
        status_badge(app, f),
    ]
    .spacing(space::M)
    .align_y(Alignment::Center);

    let mut col = column![header].spacing(space::M);
    if !can_drive {
        if !f.controllable {
            col = col.push(
                small(
                    "Ce connecteur se lit mais ne se règle pas ici : la carte mère garde la main.",
                )
                .color(r.on_surface_variant),
            );
        }
        return container(col)
            .padding(space::L)
            .width(Length::Fill)
            .style(style::card(*r, Tone::Default, shape::EXTRA_LARGE))
            .into();
    }

    col = col.push(
        row![
            widgets::segmented(
                &[
                    (FanMode::Auto, "Carte mère"),
                    (FanMode::Fixed, "Fixe"),
                    (FanMode::Curve, "Courbe"),
                ],
                prefs.mode,
                r,
                {
                    let id = id.clone();
                    move |m| Message::FanMode(id.clone(), m)
                }
            ),
            widgets::hfill(),
            widgets::icon_label_button(
                icons::RADAR,
                "Identifier",
                ButtonKind::Text,
                r,
                Some(Message::FanIdentify(id.clone())),
            ),
        ]
        .align_y(Alignment::Center),
    );
    match prefs.mode {
        FanMode::Auto => {
            col = col.push(
                small(
                    "Vitesse décidée par la carte mère (ou la carte graphique), comme sans TuxRGB.",
                )
                .color(r.on_surface_variant),
            );
        }
        FanMode::Fixed => {
            col = col.push(
                row![
                    container(
                        super::motion::slider(0.0..=100.0, f32::from(prefs.fixed), {
                            let id = id.clone();
                            move |v| Message::FanFixed(id.clone(), v.round() as u8)
                        })
                        .step(1.0)
                        .format(|v| format!("{v:.0} %"))
                        .colors(super::motion::SliderColors::standard(r))
                        .height(40.0),
                    )
                    .width(Length::Fill),
                    text(format!("{} %", prefs.fixed))
                        .size(14.0)
                        .color(r.on_surface_variant)
                        .width(Length::Fixed(52.0))
                        .align_x(iced::alignment::Horizontal::Right),
                ]
                .spacing(space::S)
                .align_y(Alignment::Center),
            );
            if prefs.fixed < 20 {
                col = col.push(
                    small("Sous 20 %, beaucoup de ventilateurs s'arrêtent : à éviter pour celui du processeur.")
                        .color(r.tertiary),
                );
            }
        }
        FanMode::Curve => {
            // Which temperature.
            let mut chips = row![].spacing(space::XS);
            for t in &app.fans.temps {
                let selected = t.id == prefs.source;
                let caption = match t.value {
                    Some(v) => format!("{} · {v:.0} °C", t.label),
                    None => format!("{} · –", t.label),
                };
                chips = chips.push(
                    button(small(caption))
                        .padding(Padding::from([6.0, 12.0]))
                        .style(style::chip(*r, selected))
                        .on_press(Message::FanSource(id.clone(), t.id.clone())),
                );
            }
            col = col.push(
                column![
                    label("Suit la température de"),
                    chips.wrap().vertical_spacing(space::XS)
                ]
                .spacing(space::XS),
            );
            let temp = app
                .fans
                .temps
                .iter()
                .find(|t| t.id == prefs.source)
                .and_then(|t| t.value);
            let editor = CurveEditor {
                fan: id.clone(),
                points: prefs.points.clone(),
                temp,
                duty: f.duty.filter(|_| f.status == FanStatus::Driven),
                roles: *r,
                editable: true,
            };
            col = col.push(
                container(
                    canvas(editor)
                        .width(Length::Fill)
                        .height(Length::Fixed(230.0)),
                )
                .padding(space::S)
                .style(style::card(*r, Tone::Lowest, shape::LARGE)),
            );
            let presets = row![
                small("Modèles :").color(r.on_surface_variant),
                widgets::text_button(
                    "Silencieux",
                    ButtonKind::Text,
                    r,
                    Some(Message::FanPreset(id.clone(), "silent"))
                ),
                widgets::text_button(
                    "Équilibré",
                    ButtonKind::Text,
                    r,
                    Some(Message::FanPreset(id.clone(), "balanced"))
                ),
                widgets::text_button(
                    "Performance",
                    ButtonKind::Text,
                    r,
                    Some(Message::FanPreset(id.clone(), "performance"))
                ),
                widgets::hfill(),
                small(match temp {
                    Some(t) => crate::i18n::trf!(
                        "{} : {} °C",
                        source_label(&prefs.source, &app.fans.temps),
                        format!("{t:.0}")
                    ),
                    None => crate::i18n::trf!(
                        "{} : température illisible",
                        source_label(&prefs.source, &app.fans.temps)
                    ),
                })
                .color(r.on_surface_variant),
            ]
            .spacing(space::XS)
            .align_y(Alignment::Center);
            col = col.push(presets);
        }
    }
    container(col)
        .padding(space::L)
        .width(Length::Fill)
        .style(style::card(*r, Tone::Default, shape::EXTRA_LARGE))
        .into()
}

/// Graphics card fans are left to their own tools.
fn gpu_note(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let body = if cfg!(windows) {
        "Les ventilateurs de la carte graphique se règlent avec le logiciel de son fabricant : AMD Software: Adrenalin Edition (Radeon) ou l'application NVIDIA (GeForce). Ici, ce sont les ventilateurs du PC branchés sur la carte mère (processeur, boîtier, pompe) qui se règlent."
    } else {
        "Les ventilateurs de la carte graphique se règlent avec LACT (Linux GPU Configuration Tool, pour les cartes AMD, NVIDIA et Intel). Ici, ce sont les ventilateurs du PC branchés sur la carte mère (processeur, boîtier, pompe) qui se règlent."
    };
    container(
        row![
            icon(icons::INFO, 22.0).color(r.primary),
            column![
                title_medium("Ventilateurs de la carte graphique"),
                small(body).color(r.on_surface_variant),
            ]
            .spacing(space::XXS)
            .width(Length::Fill),
        ]
        .spacing(space::M)
        .align_y(Alignment::Center),
    )
    .padding(space::L)
    .width(Length::Fill)
    .style(style::card(*r, Tone::Lowest, shape::EXTRA_LARGE))
    .into()
}

pub fn view(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let supporting = if app.fans.safety {
        crate::i18n::t(
            "Sécurité : une température a atteint 90 °C, les ventilateurs pilotés tournent à fond.",
        )
    } else {
        crate::i18n::t(
            "Vitesse de chaque ventilateur : réglage de la carte mère, vitesse fixe ou courbe selon une température.",
        )
    };
    let header = page_header(
        app,
        "Ventilation",
        supporting,
        widgets::icon_label_button(
            icons::REFRESH,
            "Détecter",
            ButtonKind::Tonal,
            r,
            Some(Message::FansRescan),
        )
        .into(),
    );
    let mut col = column![].spacing(space::L);
    if let Some(card) = access_card(app) {
        col = col.push(card);
    }
    col = col.push(gpu_note(app));
    if app.fans.fans.is_empty() && app.fans.access != Access::Starting {
        col = col.push(
            container(
                column![
                    icon(icons::MODE_FAN, 40.0).color(r.on_surface_variant),
                    title_medium("Aucun ventilateur trouvé"),
                    small(if cfg!(windows) {
                        "TuxRGB pilote les contrôleurs ITE et Nuvoton des cartes mères. Celui de cette carte mère n'est pas reconnu, ou l'accès n'est pas encore configuré."
                    } else {
                        "Aucun capteur de ventilateur dans /sys/class/hwmon : le pilote du contrôleur de la carte mère n'est pas chargé (« Activer » essaie it87 et nct6775)."
                    })
                    .color(r.on_surface_variant),
                ]
                .spacing(space::S)
                .align_x(Alignment::Center),
            )
            .padding(space::XL)
            .center_x(Length::Fill),
        );
    }
    for f in &app.fans.fans {
        col = col.push(fan_card(app, f));
    }
    col = col.push(
        container(
            small(
                "Sécurité : à 90 °C (processeur ou carte graphique), tous les ventilateurs pilotés passent à 100 % jusqu'à 85 °C. Si la température suivie par une courbe devient illisible, la carte mère reprend le ventilateur. Quand le service s'arrête (ou TuxRGB sous Windows), chaque ventilateur retrouve son réglage d'origine.",
            )
            .color(r.on_surface_variant),
        )
        .padding(Padding {
            left: space::S,
            right: space::S,
            ..Padding::ZERO
        }),
    );
    col = col.push(Space::new().height(space::XL));
    column![
        header,
        Space::new().height(space::XL),
        super::transition::smooth_scroll(
            scrollable(container(col).padding(Padding {
                right: space::M,
                ..Padding::ZERO
            }))
            .height(Length::Fill),
            r,
        ),
    ]
    .height(Length::Fill)
    .into()
}
