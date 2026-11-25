//! White balance of the LEDs, device by device: a white square of the
//! screen as the reference, the device being set lit in white (every other
//! LED off), and its red, green and blue. The RAM sticks are set together
//! unless the box is unticked. Shown in Réglages → Couleurs des LED and as a
//! step of the first-run wizard.

use super::icons;
use super::motion::button;
use super::widgets::{self, icon, small};
use crate::app::{App, Message, WhiteItem};
use crate::theme::style::{self, ButtonKind};
use crate::theme::tokens::{shape, space};
use iced::widget::{Space, checkbox, column, container, row};
use iced::{Alignment, Border, Color, Element, Length, Padding};
use tuxrgb_core::DeviceKind;

/// The items of the panel: (item, name).
fn items(app: &App) -> Vec<(WhiteItem, String)> {
    let devices = app.white_devices();
    let ram: Vec<_> = devices
        .iter()
        .filter(|d| d.kind == DeviceKind::Dram)
        .collect();
    let mut out = vec![(
        WhiteItem::All,
        crate::i18n::t("Tous les appareils").to_owned(),
    )];
    let linked = app.config.led_color.link_ram && !ram.is_empty();
    if linked {
        out.push((
            WhiteItem::Ram,
            if ram.len() > 1 {
                crate::i18n::trf!("Mémoire RAM · {} barrettes", ram.len())
            } else {
                crate::i18n::t("Mémoire RAM").to_owned()
            },
        ));
    }
    // Several sticks of the same model: numbered.
    let mut seen: std::collections::HashMap<&str, usize> = Default::default();
    for d in &devices {
        if linked && d.kind == DeviceKind::Dram {
            continue;
        }
        let same = devices.iter().filter(|o| o.name == d.name).count();
        let n = seen.entry(d.name.as_str()).or_insert(0);
        *n += 1;
        let name = if same > 1 {
            format!("{} · {}", d.display_name(), n)
        } else {
            d.display_name().to_owned()
        };
        out.push((WhiteItem::Device(d.key.clone()), name));
    }
    out
}

/// The screen's white, to compare the LEDs with.
fn reference<'a>(app: &'a App) -> Element<'a, Message> {
    let r = &app.roles;
    column![
        container(Space::new().width(132).height(132)).style({
            let outline = r.outline_variant;
            move |_| container::Style {
                background: Some(Color::WHITE.into()),
                border: Border {
                    color: outline,
                    width: 1.0,
                    radius: shape::LARGE.into(),
                },
                ..Default::default()
            }
        }),
        small("Le blanc de l'écran").color(r.on_surface_variant),
    ]
    .spacing(space::S)
    .align_x(Alignment::Center)
    .width(Length::Fixed(150.0))
    .into()
}

pub fn panel(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let items = items(app);
    let selected = app.white_item.clone();
    let ram_count = app
        .white_devices()
        .iter()
        .filter(|d| d.kind == DeviceKind::Dram)
        .count();

    let mut chips = row![].spacing(space::XS);
    for (item, name) in &items {
        let on = selected.as_ref() == Some(item);
        // A device with a white of its own shows it.
        let own = app.white_of(item).1;
        let mut content = row![small(name.clone())]
            .spacing(space::XS)
            .align_y(Alignment::Center);
        if own {
            content = content.push(icon(icons::TUNE, 14.0));
        }
        chips = chips.push(
            button(content)
                .padding(Padding::from([6.0, 12.0]))
                .style(style::chip(*r, on))
                .on_press(Message::WhiteSelect(if on {
                    None
                } else {
                    Some(item.clone())
                })),
        );
    }

    let mut col = column![
        small(
            "Choisissez un appareil : ses LED s'allument en blanc et les autres s'éteignent. \
             Réglez le rouge, le vert et le bleu jusqu'à ce que ce blanc ressemble au carré \
             de l'écran, sans reflet bleuté, verdâtre ou rosé."
        )
        .color(r.on_surface_variant),
    ]
    .spacing(space::M);
    if items.len() <= 1 {
        col = col.push(
            small(
                "Aucun appareil pour le moment : TuxRGB les cherche encore, ou l'accès au \
                 matériel n'est pas configuré. Ce réglage reste disponible dans Réglages → \
                 Couleurs des LED.",
            )
            .color(r.tertiary),
        );
    }
    col = col.push(chips.wrap().vertical_spacing(space::XS));
    if ram_count > 1 {
        col = col.push(
            checkbox(app.config.led_color.link_ram)
                .label(crate::i18n::t("Régler les barrettes de RAM ensemble"))
                .on_toggle(Message::WhiteLinkRam)
                .size(18)
                .text_size(14)
                .style({
                    let r = *r;
                    move |_, _| checkbox::Style {
                        background: if app.config.led_color.link_ram {
                            r.primary.into()
                        } else {
                            Color::TRANSPARENT.into()
                        },
                        icon_color: r.on_primary,
                        border: Border {
                            color: r.primary,
                            width: 2.0,
                            radius: 4.0.into(),
                        },
                        text_color: Some(r.on_surface),
                    }
                }),
        );
    }

    if let Some(item) = &selected {
        let (white, own) = app.white_of(item);
        let percent: fn(f32) -> String = |v| format!("{:.0} %", v * 100.0);
        let channels = [
            ("Rouge", Color::from_rgb8(0xE5, 0x39, 0x35)),
            ("Vert", Color::from_rgb8(0x43, 0xA0, 0x47)),
            ("Bleu", Color::from_rgb8(0x1E, 0x88, 0xE5)),
        ];
        let default = match item {
            WhiteItem::All => tuxrgb_core::led_color::DEFAULT_WHITE,
            _ => app.config.led_color.white,
        };
        let mut sliders = column![].spacing(space::XXS);
        for (ch, (name, tint)) in channels.into_iter().enumerate() {
            sliders = sliders.push(
                row![
                    small(name).width(Length::Fixed(64.0)),
                    super::settings::led_slider(
                        app,
                        0.0..=1.0,
                        white[ch],
                        default[ch],
                        percent,
                        Some(tint),
                        move |v| Message::WhiteChannel(ch, v),
                    ),
                ]
                .spacing(space::S)
                .align_y(Alignment::Center),
            );
        }
        let status = match (item, own) {
            (WhiteItem::All, _) => {
                "Réglage commun : tous les appareils qui n'ont pas de réglage propre le suivent."
            }
            (_, true) => "Réglage propre à cet appareil.",
            (_, false) => "Suit le réglage commun : bougez un curseur pour lui donner le sien.",
        };
        col = col.push(
            container(
                column![
                    sliders,
                    small(status).color(r.on_surface_variant),
                    row![
                        widgets::text_button(
                            "Réinitialiser",
                            ButtonKind::Text,
                            r,
                            Some(Message::WhiteReset)
                        ),
                        widgets::hfill(),
                        widgets::icon_label_button(
                            icons::CHECK,
                            "Terminé",
                            ButtonKind::Tonal,
                            r,
                            Some(Message::WhiteSelect(None)),
                        ),
                    ]
                    .spacing(space::S)
                    .align_y(Alignment::Center),
                ]
                .spacing(space::S),
            )
            .padding(space::L)
            .width(Length::Fill)
            .style(style::card(*r, style::Surface::High, shape::LARGE)),
        );
    }

    row![reference(app), col.width(Length::Fill)]
        .spacing(space::XL)
        .align_y(Alignment::Start)
        .into()
}
