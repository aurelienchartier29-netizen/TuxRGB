//! Devices: list with per-device control + editable setup canvas.

use super::canvas::SetupCanvas;
use super::motion::button;
use super::widgets::{self, badge, icon, icon_filled, label, muted, small, switch, title_medium};
use super::{empty_state, icons, page_header};
use crate::app::{App, Message};
use crate::theme::style::{self, ButtonKind, Surface};
use crate::theme::to_iced;
use crate::theme::tokens::{shape, space};
use iced::widget::{Space, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length, Padding};
use tuxrgb_core::layout::Target;
use tuxrgb_core::lighting::{self, MAX_PARTS};
use tuxrgb_core::{DeviceKey, DeviceKind, EffectKind, PartLighting, Rgb, SceneDevice, ZoneKind};

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
    let custom = lighting::custom_parts(info, Some(&d.lighting));
    if custom > 0 {
        badges = badges.push(badge(
            small(if custom == 1 {
                crate::i18n::t("1 zone personnalisée").to_owned()
            } else {
                trf!("{} zones personnalisées", custom)
            }),
            r,
            Surface::Secondary,
        ));
    }
    if let Some(mode) = &info.hardware_mode {
        badges = badges.push(badge(
            small(trf!("Effet matériel : {}", mode)),
            r,
            Surface::Tertiary,
        ));
    }
    let hidden = app.config.hidden_on_canvas.contains(&info.key);
    if hidden {
        badges = badges.push(badge(small("Masqué sur le canvas"), r, Surface::Highest));
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
    // Shown / hidden on the canvas (it stays connected and lit).
    trailing = trailing.push(widgets::round_icon_button(
        if hidden {
            icons::VISIBILITY_OFF
        } else {
            icons::VISIBILITY
        },
        r,
        hidden,
        Some(Message::ToggleHidden(info.key.clone())),
    ));
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
                badges.wrap().vertical_spacing(space::XS),
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
    .on_press(Message::CanvasSelect(Some(Target::device(index))))
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

/// Choice shown by the lighting chips of a part (or of a whole device,
/// where the parts may differ).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Light {
    /// Follows the global effect.
    Global,
    /// An effect of its own.
    Own,
    Color,
    Off,
    Mixed,
}

fn light_of(p: &PartLighting) -> Light {
    match p {
        PartLighting::Effect => Light::Global,
        PartLighting::Own { .. } => Light::Own,
        PartLighting::Color { .. } => Light::Color,
        PartLighting::Off => Light::Off,
    }
}

/// Round colour swatch (opens the picker), as on the Effects page.
fn swatch<'a>(app: &'a App, c: Rgb, open: bool, on_press: Message) -> Element<'a, Message> {
    let r = &app.roles;
    let col = to_iced(c);
    let (ring, hover_ring) = if open {
        (r.primary, r.primary)
    } else {
        (r.outline_variant, r.on_surface_variant)
    };
    button(Space::new().width(30).height(30))
        .padding(0)
        .style(move |_, status| iced::widget::button::Style {
            background: Some(iced::Background::Color(col)),
            border: iced::Border {
                color: match status {
                    iced::widget::button::Status::Hovered
                    | iced::widget::button::Status::Pressed => hover_ring,
                    _ => ring,
                },
                width: 3.0,
                radius: 15.0.into(),
            },
            ..Default::default()
        })
        .on_press(on_press)
        .into()
}

/// What a part of a zone shows: global effect / own effect / colour / off
/// chips, the colour swatch or the own effect's button, and a button that
/// flashes the part (`zone == usize::MAX`: the whole device).
fn part_controls<'a>(
    app: &'a App,
    key: &DeviceKey,
    zone: usize,
    part: usize,
    value: Option<&PartLighting>,
    device_id: u32,
) -> Element<'a, Message> {
    let r = &app.roles;
    let whole = zone == usize::MAX;
    let k = key.clone();
    let current = value.cloned();
    // A new own effect starts from the global one (minus the GPU shader,
    // which only exists once).
    let first_effect = if app.config.effect == EffectKind::Shader {
        EffectKind::RainbowWave
    } else {
        app.config.effect
    };
    let chips = widgets::segmented(
        &[
            (Light::Global, "Global"),
            (Light::Own, "Effet"),
            (Light::Color, "Couleur"),
            (Light::Off, "Éteint"),
        ],
        value.map_or(Light::Mixed, light_of),
        r,
        move |l| {
            let k = k.clone();
            match (l, whole) {
                (Light::Global, true) => Message::SetDeviceLighting(k, PartLighting::Effect),
                (Light::Off, true) => Message::SetDeviceLighting(k, PartLighting::Off),
                (Light::Global, false) => {
                    Message::SetPartLighting(k, zone, part, PartLighting::Effect)
                }
                (Light::Off, false) => Message::SetPartLighting(k, zone, part, PartLighting::Off),
                (Light::Own, _) => match &current {
                    Some(PartLighting::Own { .. }) => Message::OpenOwnEditor(k, zone, part),
                    _ => Message::SetOwnEffect(k, zone, part, first_effect),
                },
                (Light::Color, _) => Message::OpenPartPicker(k, zone, part),
                (Light::Mixed, _) => Message::Noop,
            }
        },
    );
    let mut line = row![chips].spacing(space::S).align_y(Alignment::Center);
    // The swatch / effect button keeps its place when hidden: the chips
    // stay aligned.
    line = line.push(match value {
        Some(PartLighting::Color { color }) => {
            let open = app.part_picker == Some((key.clone(), zone, part));
            swatch(
                app,
                *color,
                open,
                Message::OpenPartPicker(key.clone(), zone, part),
            )
        }
        Some(PartLighting::Own { effect, .. }) => {
            let open = app.own_editor == Some((key.clone(), zone, part));
            button(
                container(icon(super::dashboard::effect_icon(*effect), 18.0))
                    .center_x(Length::Fixed(30.0))
                    .center_y(Length::Fixed(30.0)),
            )
            .padding(0)
            .style(style::icon_button(app.roles, open))
            .on_press(Message::OpenOwnEditor(key.clone(), zone, part))
            .into()
        }
        _ => Space::new().width(30).height(30).into(),
    });
    line = line.push(widgets::round_icon_button(
        icons::HIGHLIGHT,
        r,
        false,
        Some(if whole {
            Message::Identify(device_id)
        } else {
            Message::IdentifyPart(key.clone(), zone, part)
        }),
    ));
    line.into()
}

/// What expands under a part's row: its colour picker.
fn part_picker<'a>(
    app: &'a App,
    key: &DeviceKey,
    zone: usize,
    part: usize,
    value: Option<&PartLighting>,
) -> Option<Element<'a, Message>> {
    let target = Some((key.clone(), zone, part));
    let mut col = column![].spacing(space::S);
    let mut any = false;
    // Colour picker.
    let open = app.part_picker == target;
    if open || app.part_picker_last == target {
        let color = match value {
            Some(PartLighting::Color { color }) => *color,
            _ => Rgb::from_hex(&app.hex_input).unwrap_or(Rgb::WHITE),
        };
        col = col.push(
            super::transition::reveal(
                open,
                r_surface(app),
                super::effects::color_editor(app, color),
            )
            .on_hidden(Message::PartPickerHidden),
        );
        any = true;
    }
    any.then(|| col.into())
}

/// Name of a part for titles: the device, a zone, or a fan of a header.
fn part_name(app: &App, key: &DeviceKey, zone: usize, part: usize) -> (String, String) {
    let Some(d) = app.scene.devices.iter().find(|d| d.info.key == *key) else {
        return (String::new(), String::new());
    };
    let device = d.info.display_name().to_owned();
    if zone == usize::MAX {
        return (device, crate::i18n::t("Tout l'appareil").to_owned());
    }
    let zone_name = d
        .info
        .zones
        .get(zone)
        .map(|z| crate::i18n::t(&z.name).to_owned())
        .unwrap_or_default();
    let split = d.segments.iter().any(|s| s.zone == zone);
    if split {
        (
            trf!("Ventilateur {}", part + 1),
            format!("{device} · {zone_name}"),
        )
    } else {
        (zone_name, device)
    }
}

/// The effect a part plays on its own (every effect but the GPU shader,
/// which only exists once) and its settings, as on the Effects page. Shown
/// under the canvas, which keeps showing the result live.
fn own_panel<'a>(
    app: &'a App,
    key: &DeviceKey,
    zone: usize,
    part: usize,
    effect: EffectKind,
) -> Element<'a, Message> {
    let r = &app.roles;
    let (name, context) = part_name(app, key, zone, part);
    let mut chips = row![].spacing(space::XS);
    for kind in EffectKind::ALL {
        if kind == EffectKind::Shader {
            continue;
        }
        let selected = kind == effect;
        let content = row![
            icon(super::dashboard::effect_icon(kind), 16.0),
            small(crate::i18n::t(kind.name()).to_owned())
        ]
        .spacing(space::XS)
        .align_y(Alignment::Center);
        chips = chips.push(
            button(content)
                .padding(Padding::from([6.0, 12.0]))
                .style(style::chip(*r, selected))
                .on_press(Message::SetOwnEffect(key.clone(), zone, part, kind)),
        );
    }
    let header = row![
        widgets::round_icon_button(
            icons::ARROW_BACK,
            r,
            false,
            Some(Message::OpenOwnEditor(key.clone(), zone, part)),
        ),
        column![
            label(trf!("Effet propre · {}", name)),
            small(context).color(r.on_surface_variant),
        ]
        .spacing(space::XXS)
        .width(Length::Fill),
        widgets::text_button(
            "Suivre l'effet global",
            ButtonKind::Tonal,
            r,
            Some(Message::SetPartLighting(
                key.clone(),
                zone,
                part,
                PartLighting::Effect
            )),
        ),
    ]
    .spacing(space::M)
    .align_y(Alignment::Center);
    let body = column![
        small(
            "Indépendant de l'effet global : il couvre ce composant comme s'il était tout le setup."
        )
        .color(r.on_surface_variant),
        chips.wrap().vertical_spacing(space::XS),
        super::effects::param_controls(
            app,
            effect,
            super::effects::ParamScope::Part(key.clone(), zone, part),
        ),
    ]
    .spacing(space::M);
    column![
        header,
        container(super::transition::smooth_scroll(
            scrollable(container(body).padding(Padding {
                right: space::M,
                ..Padding::ZERO
            }))
            .height(Length::Shrink),
            r
        ))
        .max_height(380.0),
    ]
    .spacing(space::M)
    .into()
}

fn r_surface(app: &App) -> iced::Color {
    app.roles.surface_container
}

/// M3 filter chip (one of a set of choices).
fn chip<'a>(
    app: &'a App,
    caption: &'a str,
    selected: bool,
    on_press: Message,
) -> Element<'a, Message> {
    let content: Element<'a, Message> = if selected {
        row![icon(icons::CHECK, 16.0), small(caption)]
            .spacing(space::XS)
            .align_y(Alignment::Center)
            .into()
    } else {
        small(caption).into()
    };
    button(content)
        .padding(Padding::from([6.0, 14.0]))
        .style(style::chip(app.roles, selected))
        .on_press(on_press)
        .into()
}

/// Compact − / + of a fan's LED count.
fn small_step<'a>(app: &'a App, glyph: char, on_press: Option<Message>) -> Element<'a, Message> {
    button(
        container(icon(glyph, 14.0))
            .center_x(Length::Fixed(18.0))
            .center_y(Length::Fixed(18.0)),
    )
    .padding(2)
    .style(style::icon_button(app.roles, false))
    .on_press_maybe(on_press)
    .into()
}

/// Lighting of a whole device: the value shared by every part, or `None`
/// when they differ.
fn device_value(d: &SceneDevice) -> Option<PartLighting> {
    let mut seen: Option<&PartLighting> = None;
    let none = lighting::ZoneLighting::default();
    for (zi, zone) in d.info.zones.iter().enumerate() {
        let z = d.lighting.get(&zi).unwrap_or(&none);
        for part in 0..lighting::parts_of(zone, Some(z)) {
            let v = z.part_ref(part);
            match seen {
                None => seen = Some(v),
                Some(s) if s != v => return None,
                _ => {}
            }
        }
    }
    Some(seen.cloned().unwrap_or_default())
}

/// Zones of the selected device: LED counts of addressable (ARGB) headers –
/// OpenRGB cannot know how many LEDs are plugged in –, fans per header, and
/// what each zone or fan shows (the effect, a fixed colour, or nothing).
fn zones_panel<'a>(app: &'a App, index: usize, d: &'a SceneDevice) -> Element<'a, Message> {
    let r = &app.roles;
    let info = &d.info;
    let mut col = column![].spacing(space::S);

    // The device's own effects (keyboards, mice…): TuxRGB lets go of it.
    let peripheral = matches!(
        info.kind,
        DeviceKind::Keyboard
            | DeviceKind::Keypad
            | DeviceKind::Mouse
            | DeviceKind::MouseMat
            | DeviceKind::Headset
            | DeviceKind::HeadsetStand
            | DeviceKind::Gamepad
            | DeviceKind::Microphone
            | DeviceKind::Speaker
            // Network bulbs (Philips WiZ scenes).
            | DeviceKind::Light
    );
    if !info.hardware_modes.is_empty() && (peripheral || info.hardware_mode.is_some()) {
        let current = info.hardware_mode.clone();
        let wanted = app.config.hardware_modes.get(&info.key).cloned();
        let shown = wanted.clone().or(current.clone());
        let mut chips = row![chip(
            app,
            "Effets TuxRGB",
            shown.is_none(),
            Message::SetHardwareMode(info.key.clone(), None)
        )]
        .spacing(space::XS);
        for m in &info.hardware_modes {
            chips = chips.push(chip(
                app,
                m.as_str(),
                shown.as_deref() == Some(m.as_str()),
                Message::SetHardwareMode(info.key.clone(), Some(m.clone())),
            ));
        }
        col = col.push(
            column![
                row![
                    icon(icons::AUTO_AWESOME, 18.0).color(r.primary),
                    label("Effet de l'appareil"),
                ]
                .spacing(space::S)
                .align_y(Alignment::Center),
                small(if shown.is_some() {
                    "L'appareil joue son propre effet (réactions aux touches…) : TuxRGB n'y envoie plus de couleurs, mais l'éteint toujours à l'arrêt du PC."
                } else {
                    "Suit les effets de TuxRGB. Choisissez un effet intégré pour laisser l'appareil le jouer lui-même."
                })
                .color(r.on_surface_variant),
                chips.wrap(),
            ]
            .spacing(space::XS),
        );
    }
    // Playing its own effect: TuxRGB's zone settings do not apply.
    if info.hardware_mode.is_some() {
        return super::transition::smooth_scroll(scrollable(col).height(Length::Shrink), r);
    }
    col = col.push(
        row![
            icon(icons::SETTINGS_INPUT_COMPONENT, 18.0).color(r.primary),
            label(trf!("Zones et canaux ({})", info.zones.len())),
        ]
        .spacing(space::S)
        .align_y(Alignment::Center),
    );

    // The whole device at once (the only choice for single-colour ones).
    let whole = device_value(d);
    if info.controllable && (info.zones.len() > 1 || info.single_color) {
        col = col.push(
            row![
                column![
                    label("Tout l'appareil"),
                    small(if info.single_color {
                        "Une seule couleur pour tout l'appareil (limite du contrôleur)."
                    } else {
                        "Même réglage pour toutes les zones et tous les ventilateurs."
                    })
                    .color(r.on_surface_variant),
                ]
                .spacing(space::XXS)
                .width(Length::Fill),
                part_controls(app, &info.key, usize::MAX, 0, whole.as_ref(), info.id),
            ]
            .spacing(space::S)
            .align_y(Alignment::Center),
        );
        if let Some(p) = part_picker(app, &info.key, usize::MAX, 0, whole.as_ref()) {
            col = col.push(p);
        }
    }

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
        let zl = d.lighting.get(&zi).cloned().unwrap_or_default();
        let sizes = lighting::part_sizes(z, Some(&zl));
        let parts = sizes.len();
        let per_zone = info.controllable && !info.single_color && z.led_count > 0;
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
        } else if per_zone {
            // One part: the zone's lighting on its own line.
            line = line.push(part_controls(
                app,
                &info.key,
                zi,
                0,
                Some(zl.part_ref(0)),
                info.id,
            ));
        }
        col = col.push(line);
        if !z.resizable() {
            if per_zone && let Some(p) = part_picker(app, &info.key, zi, 0, Some(zl.part_ref(0))) {
                col = col.push(p);
            }
            continue;
        }
        if !per_zone {
            continue;
        }
        // Addressable header: how many fans, then one line per fan.
        let (k1, k2) = (info.key.clone(), info.key.clone());
        let mut fans =
            row![
                container(icon(icons::MODE_FAN, 18.0).color(r.on_surface_variant))
                    .center_x(Length::Fixed(28.0)),
                column![
                label("Ventilateurs sur ce connecteur"),
                small("Chaque ventilateur peut suivre l'effet global, jouer son propre effet, garder sa couleur ou rester éteint.")
                    .color(r.on_surface_variant),
            ]
                .spacing(space::XXS)
                .width(Length::Fill),
                widgets::round_icon_button(
                    icons::REMOVE,
                    r,
                    false,
                    (parts > 1).then(|| Message::SetZoneParts(k1, zi, parts - 1)),
                ),
                text(parts.to_string())
                    .size(15.0)
                    .width(Length::Fixed(28.0))
                    .center(),
                widgets::round_icon_button(
                    icons::ADD,
                    r,
                    false,
                    (parts < MAX_PARTS.min(z.led_count)).then(|| Message::SetZoneParts(
                        k2,
                        zi,
                        parts + 1
                    )),
                ),
            ]
            .spacing(space::S)
            .align_y(Alignment::Center);
        if parts == 1 {
            fans = fans.push(part_controls(
                app,
                &info.key,
                zi,
                0,
                Some(zl.part_ref(0)),
                info.id,
            ));
        }
        col = col.push(container(fans).padding(Padding {
            left: space::L,
            ..Padding::ZERO
        }));
        if parts == 1 {
            if let Some(p) = part_picker(app, &info.key, zi, 0, Some(zl.part_ref(0))) {
                col = col.push(p);
            }
            continue;
        }
        let ranges = lighting::part_ranges(z, Some(&zl));
        for part in 0..parts {
            let range = ranges[part].clone();
            // The fan on the canvas: its number selects it there.
            let segment = d
                .segments
                .iter()
                .position(|s| s.zone == zi && s.part == part);
            let on_canvas = segment.is_some_and(|s| app.selected_segment == Some(s))
                && app.selected_device == Some(index);
            let number = button(
                container(text((part + 1).to_string()).size(13.0))
                    .center_x(Length::Fixed(28.0))
                    .center_y(Length::Fixed(28.0)),
            )
            .padding(0)
            .style(style::chip(*r, on_canvas))
            .on_press_maybe(
                segment.map(|s| Message::CanvasSelect(Some(Target::segment(index, s)))),
            );
            let n = sizes[part];
            let (k1, k2) = (info.key.clone(), info.key.clone());
            let stepper = row![
                small_step(
                    app,
                    icons::REMOVE,
                    (n > 1).then(|| Message::SetPartSize(k1, zi, part, n - 1))
                ),
                small(trf!("{} LED", n)).color(r.on_surface_variant),
                small_step(
                    app,
                    icons::ADD,
                    Some(Message::SetPartSize(k2, zi, part, n + 1))
                ),
            ]
            .spacing(space::XXS)
            .align_y(Alignment::Center);
            let fan = row![
                number,
                column![
                    label(trf!("Ventilateur {}", part + 1)),
                    row![
                        small(trf!("LED {} à {}", range.start + 1, range.end))
                            .color(r.on_surface_variant),
                        stepper,
                    ]
                    .spacing(space::S)
                    .align_y(Alignment::Center),
                ]
                .spacing(space::XXS)
                .width(Length::Fill),
                part_controls(app, &info.key, zi, part, Some(zl.part_ref(part)), info.id),
            ]
            .spacing(space::S)
            .align_y(Alignment::Center);
            col = col.push(container(fan).padding(Padding {
                left: space::L,
                ..Padding::ZERO
            }));
            if let Some(p) = part_picker(app, &info.key, zi, part, Some(zl.part_ref(part))) {
                col = col.push(p);
            }
        }
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
    let target = app.selected_target();
    let selected = target.and_then(|t| app.scene.devices.get(t.device).map(|d| (t, d)));
    // The settings of an own effect, when open, take the place of the zones.
    let own = app.own_editor.as_ref().and_then(|(key, zone, part)| {
        let d = app.scene.devices.iter().find(|d| d.info.key == *key)?;
        let value = if *zone == usize::MAX {
            device_value(d)?
        } else {
            d.lighting
                .get(zone)
                .map(|z| z.part(*part))
                .unwrap_or_default()
        };
        value.own_effect().map(|e| (key, *zone, *part, e))
    });
    let footer: Element<'_, Message> = match (own, selected) {
        (Some((key, zone, part, effect)), _) => own_panel(app, key, zone, part, effect),
        (None, selected) => match selected {
        Some((t, d)) => {
            let fan = t.segment.and_then(|s| d.segments.get(s));
            let place = app.scene.placement_of(t).unwrap_or(d.placement);
            let base = if fan.is_some() {
                DeviceKind::Cooler.default_size()
            } else {
                d.info.kind.default_size()
            };
            // Size relative to the kind's own shape (before the turn).
            let scale = (place.w / base.0).clamp(0.3, 3.0);
            let key = app.scene.key_of(t).cloned().unwrap_or_else(|| d.info.key.clone());
            let hidden = app.scene.is_hidden(t, &app.config.hidden_on_canvas);
            let (title, subtitle) = match fan {
                Some(s) => (
                    trf!("Ventilateur {}", s.part + 1),
                    format!(
                        "{} · {}",
                        d.info.display_name(),
                        crate::i18n::t(&d.info.zones[s.zone].name)
                    ),
                ),
                None => (d.info.display_name().to_owned(), d.info.location.clone()),
            };
            // Continuous rotation (TuxRGB 3.1): any angle, the right angles
            // magnetic.
            let degrees = place.rotation;
            let rotation = super::motion::slider(0.0..=359.0, degrees, Message::SetRotation)
                .step(1.0)
                .format(|v| format!("{v:.0}°"))
                .colors(super::motion::SliderColors::standard(r))
                .height(40.0);
            let placement = column![
                row![
                    icon(if fan.is_some() { icons::MODE_FAN } else { kind_icon(d.info.kind) }, 22.0)
                        .color(r.primary),
                    column![label(title), small(subtitle).color(r.on_surface_variant)]
                        .spacing(space::XXS)
                        .width(Length::Fill),
                    widgets::round_icon_button(
                        if hidden { icons::VISIBILITY_OFF } else { icons::VISIBILITY },
                        r,
                        hidden,
                        Some(Message::ToggleHidden(key)),
                    ),
                ]
                .spacing(space::M)
                .align_y(Alignment::Center),
                row![
                    icon(icons::ROTATE_RIGHT, 20.0).color(r.on_surface_variant),
                    container(rotation).width(Length::Fill),
                    container(
                        text(format!("{degrees:.0}°"))
                            .size(13.0)
                            .color(r.on_surface_variant)
                    )
                    .width(40.0)
                    .align_x(Alignment::End),
                    widgets::round_icon_button(
                        icons::FLIP,
                        r,
                        place.mirror,
                        Some(Message::MirrorSelected)
                    ),
                ]
                .spacing(space::M)
                .align_y(Alignment::Center),
                row![
                    icon(icons::ZOOM_OUT_MAP, 20.0).color(r.on_surface_variant),
                    container(
                        super::motion::slider(0.3..=3.0, scale, Message::ResizeSelected)
                            .step(0.05)
                            .format(|v| format!("×{v:.2}"))
                            .colors(super::motion::SliderColors::standard(r))
                            .height(40.0)
                    )
                    .width(Length::Fill),
                    container(text(format!("×{scale:.2}")).size(13.0).color(r.on_surface_variant))
                        .width(40.0)
                        .align_x(Alignment::End),
                    // Same width as the mirror button above: both sliders
                    // line up.
                    Space::new().width(40.0),
                ]
                .spacing(space::M)
                .align_y(Alignment::Center),
            ]
            .spacing(space::S);
            if d.info.zones.is_empty() && d.info.hardware_modes.is_empty() {
                placement.into()
            } else {
                column![
                    container(zones_panel(app, t.device, d)).max_height(320.0),
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
        },
    };
    let swap_key = if own.is_some() {
        u64::MAX - 1
    } else {
        target.map_or(u64::MAX, |t| {
            (t.device as u64) << 8 | t.segment.map_or(0, |s| s as u64 + 1)
        })
    };
    container(
        column![
            super::canvas::view(
                SetupCanvas {
                    scene: &app.scene,
                    frame: app.preview.as_ref(),
                    roles: *r,
                    selected: target,
                    hidden: &app.config.hidden_on_canvas,
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
            super::transition::swap(swap_key, r.surface_container, footer),
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
