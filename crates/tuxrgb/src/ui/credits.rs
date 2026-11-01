//! "Crédits & licences open source".

use super::motion::button;
use super::widgets::{
    self, badge, body, icon_filled, icon_label_button, label, muted, section, small, title_medium,
};
use super::{icons, page_header};
use crate::app::{App, License, Message, SettingsView};
use crate::credits::{CREDITS, PROJECT_LICENSE, rust_crates};
use crate::theme::style::{self, ButtonKind, Surface};
use crate::theme::tokens::{shape, space};
use iced::widget::{Space, column, container, row, scrollable, text};
use iced::{Alignment, Element, Length, Padding};

fn credit_card<'a>(app: &'a App, c: &'a crate::credits::Credit) -> Element<'a, Message> {
    let r = &app.roles;
    let mut badges = row![badge(small(c.license), r, Surface::Secondary)].spacing(space::XS);
    if c.bundled {
        badges = badges.push(badge(small("Inclus dans TuxRGB"), r, Surface::Highest));
    }
    container(
        row![
            column![
                title_medium(c.name),
                small(c.role).color(r.on_surface_variant),
                small(c.authors),
                badges
            ]
            .spacing(space::XS)
            .width(Length::Fill),
            widgets::round_icon_button(
                icons::OPEN_IN_NEW,
                r,
                false,
                Some(Message::OpenUrl(c.url.to_owned()))
            ),
        ]
        .spacing(space::L)
        .align_y(Alignment::Center),
    )
    .padding(space::L)
    .width(Length::Fill)
    .style(style::card(*r, Surface::Default, shape::EXTRA_LARGE))
    .into()
}

pub fn view(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let intro = container(
        column![
            row![
                icon_filled(icons::VOLUNTEER_ACTIVISM, 28.0).color(r.on_primary_container),
                title_medium("Merci aux projets libres qui rendent TuxRGB possible"),
            ]
            .spacing(space::M)
            .align_y(Alignment::Center),
            body(trf!(
                "TuxRGB est distribué sous licence {}. Il embarque OpenRGB (version 1.0 et ajouts récents), \
                 exécuté comme serveur privé : tout le support matériel (cartes mères, RAM, GPU, \
                 périphériques USB/HID, watercooling) est le travail de la communauté OpenRGB. \
                 Les effets avancés, la carte visuelle et la synchronisation matérielle des plugins \
                 OpenRGB sont réimplémentés nativement en Rust dans TuxRGB. \
                 Le code source complet de la version d'OpenRGB embarquée est fourni avec TuxRGB (third_party/openrgb).",
                PROJECT_LICENSE
            )),
            row![
                icon_label_button(
                    icons::GAVEL,
                    "Licence GPL v2",
                    ButtonKind::Filled,
                    r,
                    Some(Message::ShowLicense(Some(License::Gpl2)))
                ),
                icon_label_button(
                    icons::DESCRIPTION,
                    "LGPL v3 (Qt)",
                    ButtonKind::Tonal,
                    r,
                    Some(Message::ShowLicense(Some(License::Lgpl3)))
                ),
                icon_label_button(
                    icons::DESCRIPTION,
                    "Apache 2.0",
                    ButtonKind::Tonal,
                    r,
                    Some(Message::ShowLicense(Some(License::Apache2)))
                ),
                icon_label_button(
                    icons::DESCRIPTION,
                    "MIT",
                    ButtonKind::Tonal,
                    r,
                    Some(Message::ShowLicense(Some(License::Mit)))
                ),
            ]
            .spacing(space::S),
        ]
        .spacing(space::L),
    )
    .padding(space::XL)
    .width(Length::Fill)
    .style(style::card(
        *r,
        Surface::Primary,
        shape::EXTRA_LARGE_INCREASED,
    ));

    let mut projects =
        column![section(icons::LIBRARY_BOOKS, "Projets intégrés", r)].spacing(space::S);
    for c in CREDITS {
        projects = projects.push(credit_card(app, c));
    }

    let crates: Vec<_> = rust_crates().collect();
    let mut table = column![].spacing(space::XXS);
    for (name, version, license, _repo) in &crates {
        table = table.push(
            row![
                text(*name).size(13.0).width(Length::FillPortion(3)),
                text(*version)
                    .size(13.0)
                    .color(r.on_surface_variant)
                    .width(Length::FillPortion(2)),
                text(*license)
                    .size(13.0)
                    .color(r.on_surface_variant)
                    .width(Length::FillPortion(4)),
            ]
            .spacing(space::M),
        );
    }
    let rust = column![
        section(icons::CODE, "Dépendances Rust", r),
        muted(
            trf!(
                "{} bibliothèques, liste générée depuis Cargo.lock.",
                crates.len()
            ),
            r
        ),
        container(table)
            .padding(space::L)
            .width(Length::Fill)
            .style(style::card(*r, Surface::Lowest, shape::EXTRA_LARGE)),
    ]
    .spacing(space::S);

    let back = button(
        row![widgets::icon(icons::ARROW_BACK, 20.0), label("Réglages")]
            .spacing(space::S)
            .align_y(Alignment::Center),
    )
    .padding(Padding::from([8.0, 16.0]))
    .style(style::button(*r, ButtonKind::Text))
    .on_press(Message::ShowSettings(SettingsView::Main));

    column![
        page_header(
            app,
            "Crédits & licences",
            "Logiciels libres utilisés par TuxRGB.",
            back.into()
        ),
        super::transition::smooth_scroll(
            scrollable(
                container(
                    column![intro, projects, rust, Space::new().height(space::XL)]
                        .spacing(space::XL)
                )
                .padding(Padding {
                    right: space::L,
                    ..Padding::ZERO
                })
            )
            .height(Length::Fill),
            r,
        ),
    ]
    .spacing(space::XL)
    .into()
}
