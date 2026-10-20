//! Devices: list with per-device control + editable setup canvas.

use super::canvas::SetupCanvas;
use super::motion::button;
use super::widgets::{self, badge, icon, icon_filled, label, muted, small, switch, title_medium};
use super::{empty_state, icons, page_header};
use crate::app::{App, Message};
use crate::theme::style::{self, ButtonKind, Surface};
use crate::theme::tokens::{shape, space};
use iced::widget::{Space, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length, Padding};
use tuxrgb_core::{DeviceKind, SceneDevice, ZoneKind};

pub fn kind_icon(kind: DeviceKind) -> char {
    match kind {
        DeviceKind::Motherboard => icons::DEVELOPER_BOARD,
        DeviceKind::Dram => icons::MEMORY_ALT,
        DeviceKind::Gpu => icons::VIDEOGAME_ASSET,
        DeviceKind::Cooler => icons::MODE_FAN,
        DeviceKind::LedStrip => icons::LINEAR_SCALE,
        DeviceKind::Keyboard | DeviceKind::Keypad => icons::KEYBOARD,
        DeviceKind::Mouse => icons::MOUSE,
        DeviceKind::MouseMat => icons::ASPECT_RATIO,
        DeviceKind::Headset | DeviceKind::HeadsetStand => icons::HEADPHONES,
        DeviceKind::Gamepad => icons::SPORTS_ESPORTS,
        DeviceKind::Light => icons::LIGHTBULB,
        DeviceKind::Speaker => icons::SPEAKER,
        DeviceKind::Storage => icons::HARD_DRIVE,
        DeviceKind::Case => icons::DEPLOYED_CODE,
        DeviceKind::Microphone => icons::MIC,
        DeviceKind::Laptop => icons::LAPTOP_CHROMEBOOK,
        DeviceKind::Monitor => icons::MONITOR,
        DeviceKind::Virtual | DeviceKind::Accessory | DeviceKind::Unknown => icons::CATEGORY,
    }
}

fn device_row<'a>(app: &'a App, index: usize, d: &'a SceneDevice) -> Element<'a, Message> {
    let r = &app.roles;
    let selected = app.selected_device == Some(index);
    let info = &d.info;
    let zones = info
        .zones
        .iter()
        .map(|z| format!("{} ({})", crate::i18n::t(&z.name), z.led_count))
        .collect::<Vec<_>>()
        .join(" · ");
    let mut badges = row![badge(
        small(trf!("{} LED", info.led_count)),
        r,
        Surface::Highest
    )]
    .spacing(space::XS);
    if info.is_virtual {
        badges = badges.push(badge(small("Démo"), r, Surface::Tertiary));
    }
    let unsized_zones = info.zones.iter().filter(|z| z.needs_size()).count();
    if unsized_zones > 0 {
        badges = badges.push(badge(
            small(if unsized_zones == 1 {
                crate::i18n::t("1 zone ARGB à configurer").to_owned()
            } else {
                trf!("{} zones ARGB à configurer", unsized_zones)
            }),
            r,
            Surface::Tertiary,
        ));
    }
    if !info.controllable && info.led_count > 0 {
        badges = badges.push(badge(small("Aucun mode pilotable"), r, Surface::Error));
    } else if info.single_color {
        badges = badges.push(badge(small("Couleur unique"), r, Surface::Highest));
    }
    let leading = container(icon_filled(kind_icon(info.kind), 26.0))
        .center_x(Length::Fixed(52.0))
        .center_y(Length::Fixed(52.0))
        .style(style::card(
            *r,
            if selected {
                Surface::Primary
            } else {
                Surface::Highest
            },
            shape::LARGE_INCREASED,
        ));
    let mut trailing = row![].spacing(space::S).align_y(Alignment::Center);
    if info.controllable {
        trailing = trailing.push(widgets::round_icon_button(
            icons::HIGHLIGHT,
            r,
            false,
            (d.enabled).then_some(Message::Identify(info.id)),
        ));
    }
    if !info.is_virtual {
        let key = info.key.clone();
        trailing = trailing.push(switch(d.enabled, r, move |on| {
            Message::ToggleDevice(key.clone(), on)
        }));
    }
    button(
        row![
            leading,
            column![
                title_medium(info.display_name()),
                small(format!(
                    "{} · {}",
                    if info.vendor.is_empty() {
                        crate::i18n::t("Fabricant inconnu")
                    } else {
                        info.vendor.as_str()
                    },
                    crate::i18n::t(info.kind.label())
                ))
                .color(r.on_surface_variant),
                badges,
                small(zones).color(r.on_surface_variant),
            ]
            .spacing(space::XS)
            .width(Length::Fill),
            trailing,
        ]
        .spacing(space::L)
        .align_y(Alignment::Center),
    )
    .padding(space::L)
    .width(Length::Fill)
    .style(style::card_button(
        *r,
        selected,
        if selected { 1.0 } else { 0.0 },
    ))
    .on_press(Message::CanvasSelect(Some(index)))
    .into()
}

/// Warning shown while addressable headers have no LED count: whatever is
/// plugged there (fans, strips) stays dark until it is set.
fn zone_kind_label(kind: ZoneKind) -> &'static str {
    match kind {
        ZoneKind::Single => "LED unique",
        ZoneKind::Linear => "Linéaire",
        ZoneKind::Ring => "Anneau",
        ZoneKind::Matrix => "Matrice",
    }
}

/// Zones of the selected device; addressable (ARGB) zones get an LED-count
/// editor – OpenRGB cannot know how many LEDs are plugged into a header.
fn zones_panel<'a>(app: &'a App, d: &'a SceneDevice) -> Element<'a, Message> {
    let r = &app.roles;
    let info = &d.info;
    let mut col = column![
        row![
            icon(icons::SETTINGS_INPUT_COMPONENT, 18.0).color(r.primary),
            label(trf!("Zones et canaux ({})", info.zones.len())),
        ]
        .spacing(space::S)
        .align_y(Alignment::Center)
    ]
    .spacing(space::S);
    let mut any_resizable = false;
    for (zi, z) in info.zones.iter().enumerate() {
        let details = if z.resizable() {
            trf!(
                "{} · {} LED · réglable de {} à {}",
                crate::i18n::t(zone_kind_label(z.kind)),
                z.led_count,
                z.leds_min,
                z.leds_max
            )
        } else {
            trf!(
                "{} · {} LED",
                crate::i18n::t(zone_kind_label(z.kind)),
                z.led_count
            )
        };
        let mut line = row![
            column![
                label(crate::i18n::t(&z.name).to_owned()).color(if z.needs_size() {
                    r.tertiary
                } else {
                    r.on_surface
                }),
                small(details).color(r.on_surface_variant),
            ]
            .spacing(space::XXS)
            .width(Length::Fill)
        ]
        .spacing(space::S)
        .align_y(Alignment::Center);
        if z.resizable() {
            any_resizable = true;
            let key = info.key.clone();
            let editable = !info.is_virtual;
            let value = app
                .zone_edits
                .get(&(key.clone(), zi))
                .cloned()
                .unwrap_or_else(|| z.led_count.to_string());
            let changed = value
                .parse::<usize>()
                .ok()
                .is_some_and(|v| v != z.led_count);
            let (k1, k2, k3, k4) = (key.clone(), key.clone(), key.clone(), key);
            let mut input = text_input("LED", &value)
                .width(Length::Fixed(64.0))
                .padding(8)
                .size(14.0);
            if editable {
                input = input
                    .on_input(move |v| Message::ZoneSizeInput(k1.clone(), zi, v))
                    .on_submit(Message::ApplyZoneSize(k4.clone(), zi));
            }
            line = line
                .push(widgets::round_icon_button(
                    icons::REMOVE,
                    r,
                    false,
                    editable.then(|| Message::ZoneSizeStep(k2, zi, -1)),
                ))
                .push(super::transition::text_field(input, r))
                .push(widgets::round_icon_button(
                    icons::ADD,
                    r,
                    false,
                    editable.then(|| Message::ZoneSizeStep(k3, zi, 1)),
                ))
                .push(widgets::text_button(
                    "Appliquer",
                    if changed {
                        ButtonKind::Filled
                    } else {
                        ButtonKind::Tonal
                    },
                    r,
                    (editable && changed).then(|| Message::ApplyZoneSize(info.key.clone(), zi)),
                ));
        }
        col = col.push(line);
    }
    if any_resizable {
        col = col.push(
            muted(
                "Connecteur ARGB (+5 V) : indiquez le nombre total de LED branchées, \
                 par exemple 3 ventilateurs de 8 LED = 24. OpenRGB mémorise la valeur.",
                r,
            )
            .size(13.0),
        );
    }
    super::transition::smooth_scroll(scrollable(col).height(Length::Shrink), r)
}

fn editor(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let selected = app.selected_device.and_then(|i| app.scene.devices.get(i));
    let footer: Element<'_, Message> = match selected {
        Some(d) => {
            let (dw, _) = d.placement.oriented_size(d.info.kind.default_size());
            let scale = (d.placement.w / dw).clamp(0.3, 3.0);
            let placement = row![
                icon(kind_icon(d.info.kind), 22.0).color(r.primary),
                column![label(d.info.display_name()), small(d.info.location.clone()).color(r.on_surface_variant)]
                    .spacing(space::XXS)
                    .width(Length::FillPortion(2)),
                widgets::round_icon_button(
                    icons::ROTATE_RIGHT,
                    r,
                    d.placement.rotation != 0,
                    Some(Message::RotateSelected)
                ),
                widgets::round_icon_button(
                    icons::FLIP,
                    r,
                    d.placement.mirror,
                    Some(Message::MirrorSelected)
                ),
                icon(icons::ZOOM_OUT_MAP, 20.0).color(r.on_surface_variant),
                container(
                    super::motion::slider(0.3..=3.0, scale, Message::ResizeSelected)
                        .step(0.05)
                        .format(|v| format!("×{v:.2}"))
                        .colors(super::motion::SliderColors::standard(r))
                        .height(40.0)
                )
                .width(Length::FillPortion(2)),
                text(format!("×{scale:.2}")).size(13.0).color(r.on_surface_variant),
            ]
            .spacing(space::M)
            .align_y(Alignment::Center);
            if d.info.zones.is_empty() {
                placement.into()
            } else {
                column![
                    container(zones_panel(app, d)).max_height(260.0),
                    placement
                ]
                .spacing(space::L)
                .into()
            }
        }
        None => row![
            icon(icons::DRAG_INDICATOR, 20.0).color(r.on_surface_variant),
            muted("Glissez les appareils pour reproduire votre setup réel : les effets suivent la disposition.", r)
        ]
        .spacing(space::S)
        .align_y(Alignment::Center)
        .into(),
    };
    container(
        column![
            super::canvas::view(
                SetupCanvas {
                    scene: &app.scene,
                    frame: app.preview.as_ref(),
                    roles: *r,
                    selected: app.selected_device,
                    editable: true,
                    show_labels: true,
                    screen_hint: app.config.effect == tuxrgb_core::EffectKind::ScreenSync,
                    cache: &app.canvas_cache,
                    backdrop: app.backdrop.as_ref(),
                    appear: super::canvas::appear(app),
                },
                Length::Fill,
                Length::Fill,
            ),
            // Another device selected: its settings fade in.
            super::transition::swap(
                app.selected_device.map_or(u64::MAX, |i| i as u64),
                r.surface_container,
                footer,
            ),
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
    let r = &app.roles;
    let real = app
        .scene
        .devices
        .iter()
        .filter(|d| !d.info.is_virtual)
        .count();
    let supporting = if app.using_demo {
        crate::i18n::t("Aucun matériel détecté pour l'instant – appareils virtuels affichés.")
            .to_owned()
    } else {
        trf!("{} périphérique(s) · {} LED", real, app.scene.total_leds())
    };
    let header = page_header(
        app,
        "Appareils",
        supporting,
        row![
            widgets::icon_label_button(
                icons::REFRESH,
                "Relancer la détection",
                ButtonKind::Tonal,
                r,
                Some(Message::Rescan)
            ),
            widgets::icon_label_button(
                icons::RESTORE,
                "Disposition par défaut",
                ButtonKind::Outlined,
                r,
                Some(Message::ResetLayout)
            ),
        ]
        .spacing(space::S)
        .into(),
    );
    let list: Element<'_, Message> = if app.scene.devices.is_empty() {
        empty_state(
            app,
            icons::USB,
            "Aucun appareil",
            "Vérifiez l'accès matériel dans les réglages puis relancez la détection.",
        )
    } else {
        // Devices detected while the page is open slide into the list.
        let col = iced::widget::keyed_column(app.scene.devices.iter().enumerate().map(|(i, d)| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            d.info.key.hash(&mut h);
            (
                h.finish(),
                super::transition::reveal(true, r.surface, device_row(app, i, d))
                    .appear(app.device_appear(&d.info.key) < 1.0)
                    .into(),
            )
        }))
        .spacing(space::S);
        super::transition::smooth_scroll(
            scrollable(container(col).padding(Padding {
                right: space::M,
                ..Padding::ZERO
            }))
            .height(Length::Fill),
            r,
        )
    };
    // The ARGB banner expands / collapses with its own bottom margin.
    let banners = iced::widget::keyed_column(app.device_banners.items.iter().map(|p| {
        let banner =
            container(super::dashboard::issue_banner(app, p.value.clone())).padding(Padding {
                bottom: space::XL,
                ..Padding::ZERO
            });
        (
            p.key,
            super::transition::reveal(!p.closing, r.surface, banner)
                .appear(p.fresh())
                .on_hidden(Message::BannerHidden(p.key))
                .into(),
        )
    }));
    column![
        header,
        Space::new().height(space::XL),
        banners,
        row![
            container(list)
                .width(Length::FillPortion(2))
                .height(Length::Fill),
            editor(app)
        ]
        .spacing(space::XL)
        .height(Length::Fill),
    ]
    .into()
}
