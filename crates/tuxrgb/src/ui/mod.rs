//! Views. Layout: M3 Expressive navigation rail + page, with overlays
//! (dialogs, snackbar). The onboarding wizard replaces the whole shell.

pub mod backdrop;
pub mod canvas;
pub mod color_picker;
mod credits;
mod dashboard;
mod devices;
mod displays;
mod effects;
#[rustfmt::skip]
pub mod icons;
pub mod motion;
mod onboarding;
pub mod settings;
pub mod transition;
pub mod widgets;

use self::motion::button;
use self::transition::{ease_in, ease_out, veil};
use crate::app::{App, License, Message, Modal, Page, SettingsView, VIEW_FADE_OUT, ViewSnap};
use crate::theme::Roles;
use crate::theme::style::{self, ButtonKind, Surface};
use crate::theme::tokens::{RAIL_WIDTH, shape, space};
use crate::theme::with_alpha;
use iced::widget::{
    Space, center, column, container, mouse_area, opaque, row, scrollable, stack, text,
};
use iced::{Alignment, Background, Element, Length, Padding};
use widgets::{body, icon, icon_filled, muted, small, title};

pub fn view(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let shell = |app| row![nav_rail(app), page(app)].into();
    let base: Element<'_, Message> = match (&app.transition, &app.wizard) {
        // Wizard steps, wizard ↔ app: the whole window fades through.
        (Some(t), wizard) if t.whole => {
            let current = match wizard {
                Some(w) => onboarding::view(app, w),
                None => shell(app),
            };
            let from = match &t.from {
                ViewSnap::Wizard(w) => onboarding::view(app, w),
                ViewSnap::Main(..) => shell(app),
            };
            fade_through(app, from, current)
        }
        (_, Some(w)) => onboarding::view(app, w),
        (_, None) => shell(app),
    };
    let mut layers = stack![
        container(base)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(style::page(*r))
    ];
    if let Some(m) = app.modal {
        // Fades and grows in (expressive spring), fades and shrinks out.
        let t = app.modal_t.value();
        let roles = app.roles.faded(t);
        let (dialog, dismiss) = match m {
            Modal::ResetAccess => (reset_dialog(app, roles), Message::AskResetAccess(false)),
            Modal::License(l) => (license_dialog(roles, l), Message::ShowLicense(None)),
        };
        layers = layers.push(dialog_layer(app, dialog, dismiss, t));
    }
    if let Some((_, msg)) = &app.snackbar {
        layers = layers.push(snackbar(app, msg));
    }
    layers.into()
}

/// M3 "fade through": the outgoing view fades into the background colour,
/// then the incoming one fades in while growing from 95 % (transformation,
/// no re-layout). Input is ignored for the ~0.3 s it lasts.
fn fade_through<'a>(
    app: &'a App,
    from: Element<'a, Message>,
    current: Element<'a, Message>,
) -> Element<'a, Message> {
    let p = app.transition_t;
    let bg = app.roles.surface;
    if p < VIEW_FADE_OUT {
        let k = ease_in(p / VIEW_FADE_OUT);
        veil(from).color(with_alpha(bg, k)).block(true).into()
    } else {
        let k = ease_out((p - VIEW_FADE_OUT) / (1.0 - VIEW_FADE_OUT));
        veil(current)
            .color(with_alpha(bg, 1.0 - k))
            .scale(0.95 + 0.05 * k)
            .block(true)
            .into()
    }
}

/// Scrim + dialog for the dialog animation value `t`: the scrim darkens,
/// the dialog (colours already faded by `t`) grows from 90 % with an
/// expressive spring; on the way out it fades while shrinking slightly.
fn dialog_layer<'a>(
    app: &'a App,
    dialog: Element<'a, Message>,
    on_dismiss: Message,
    t: f32,
) -> Element<'a, Message> {
    let opening = app.confirm_reset || app.license.is_some();
    let a = t.clamp(0.0, 1.0);
    let scale = if opening {
        0.9 + 0.1 * t
    } else {
        0.94 + 0.06 * a
    };
    let scrim_color = with_alpha(app.roles.scrim, 0.45 * a);
    let mut scrim = mouse_area(
        container(Space::new().width(Length::Fill).height(Length::Fill)).style(move |_| {
            container::Style {
                background: Some(Background::Color(scrim_color)),
                ..container::Style::default()
            }
        }),
    );
    if opening {
        scrim = scrim.on_press(on_dismiss);
    }
    // Default text colour of the dialog fades too.
    let theme = app.roles.faded(a).iced_theme();
    let dialog = veil(
        iced::widget::themer(Some(theme), dialog).text_color(|t: &iced::Theme| t.palette().text),
    )
    .scale(scale)
    .block(!opening || t < 0.9);
    stack![scrim, dialog].into()
}

/// The TuxRGB penguin (embedded RGBA, no image decoder needed).
pub fn logo<'a>(size: f32) -> Element<'a, Message> {
    static HANDLE: std::sync::OnceLock<iced::widget::image::Handle> = std::sync::OnceLock::new();
    let handle = HANDLE
        .get_or_init(|| {
            // 256 px: sharp up to 112 px on a 2x screen.
            iced::widget::image::Handle::from_rgba(
                256,
                256,
                include_bytes!("../../assets/logo-256.rgba").as_slice(),
            )
        })
        .clone();
    iced::widget::image(handle)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into()
}

fn nav_rail(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let items = [
        (Page::Home, icons::SPACE_DASHBOARD, "Accueil"),
        (Page::Effects, icons::AUTO_AWESOME, "Effets"),
        (Page::Devices, icons::DEVICES, "Appareils"),
        (Page::Displays, icons::BROWSE_ACTIVITY, "Afficheurs"),
        (Page::Settings, icons::SETTINGS, "Réglages"),
    ];
    let mut col = column![
        container(logo(56.0)).padding(Padding::from([space::L, 0.0])),
        Space::new().height(space::L),
    ]
    .align_x(Alignment::Center)
    .spacing(space::M)
    .width(Length::Fixed(RAIL_WIDTH));

    for (page, glyph, caption) in items {
        let amount = app.nav[page.index()].value();
        let a = amount.clamp(0.0, 1.0);
        // Indicator pill expands horizontally from the centre (spring).
        let pill_w = 32.0 + 24.0 * amount.clamp(0.0, 1.2);
        // The icon fills and the label turns bold half-way through the
        // indicator's spring, while their colours ease continuously.
        let ink = crate::theme::mix(r.on_surface_variant, r.on_secondary_container, a);
        let glyph_el = if a >= 0.5 {
            icon_filled(glyph, 24.0).color(ink)
        } else {
            icon(glyph, 24.0).color(ink)
        };
        let indicator = container(glyph_el)
            .center_x(Length::Fixed(pill_w))
            .center_y(Length::Fixed(32.0))
            .style(style::indicator(*r, amount));
        let caption = text(crate::i18n::t(caption))
            .size(12.0)
            .font(widgets::weight(if a >= 0.5 {
                iced::font::Weight::Bold
            } else {
                iced::font::Weight::Medium
            }))
            .color(crate::theme::mix(r.on_surface_variant, r.on_surface, a));
        col = col.push(
            button(
                column![
                    container(indicator).center_x(Length::Fixed(64.0)),
                    container(caption).center_x(Length::Fixed(80.0))
                ]
                .spacing(space::XS)
                .align_x(Alignment::Center),
            )
            .width(Length::Fixed(80.0))
            .padding(Padding::from([space::XS, 0.0]))
            .style(style::nav_item(*r))
            .on_press(Message::Navigate(page)),
        );
    }
    col = col.push(Space::new().height(Length::Fill));
    // Master switch at the bottom of the rail.
    let on = app.config.output_enabled;
    col = col.push(
        container(
            button(
                container(icon_filled(icons::POWER_SETTINGS_NEW, 24.0))
                    .center_x(Length::Fixed(24.0))
                    .center_y(Length::Fixed(24.0)),
            )
            .padding(16)
            .style(style::fab(*r, on))
            .on_press(Message::SetOutput(!on)),
        )
        .padding(Padding {
            bottom: space::XL,
            ..Padding::ZERO
        }),
    );
    // M3: the navigation rail sits directly on the surface.
    container(col).height(Length::Fill).into()
}

fn page(app: &App) -> Element<'_, Message> {
    let current = page_body(app, app.page, app.settings_view);
    match &app.transition {
        Some(t) if !t.whole => {
            let from = match &t.from {
                ViewSnap::Main(p, v) => page_body(app, *p, *v),
                ViewSnap::Wizard(_) => page_body(app, app.page, app.settings_view),
            };
            fade_through(app, from, current)
        }
        _ => current,
    }
}

/// A page with its margins.
fn page_body(app: &App, page: Page, settings_view: SettingsView) -> Element<'_, Message> {
    let content = match page {
        Page::Home => dashboard::view(app),
        Page::Effects => effects::view(app),
        Page::Devices => devices::view(app),
        Page::Displays => displays::view(app),
        Page::Settings => match settings_view {
            SettingsView::Main => settings::view(app),
            SettingsView::Credits => credits::view(app),
        },
    };
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(Padding {
            top: space::XL,
            right: space::XXL,
            bottom: space::XL,
            left: space::XXL,
        })
        .into()
}

/// Centred dialog surface.
fn dialog_card<'a>(r: Roles, content: Element<'a, Message>, width: f32) -> Element<'a, Message> {
    center(opaque(
        container(content)
            .width(Length::Fixed(width))
            .padding(space::XL)
            .style(style::card(r, Surface::High, shape::EXTRA_LARGE_INCREASED)),
    ))
    .into()
}

fn reset_dialog(app: &App, roles: Roles) -> Element<'_, Message> {
    let r = &roles;
    let content = column![
        icon_filled(icons::RESTART_ALT, 28.0).color(r.secondary),
        title("Réinitialiser l'accès matériel ?"),
        body(if cfg!(windows) {
            "TuxRGB va retirer le mode administrateur qu'il a configuré (Windows demandera une \
             confirmation), effacer l'état du premier lancement puis relancer l'assistant d'accès \
             matériel.\n\nUtile après un changement de carte mère, de RAM ou de périphérique, ou si \
             un appareil n'est plus détecté."
        } else {
            "TuxRGB va retirer les règles udev et la configuration I2C qu'il a installées \
             (votre mot de passe administrateur sera demandé), effacer l'état du premier \
             lancement puis relancer l'assistant d'accès matériel.\n\nUtile après un changement \
             de carte mère, de RAM ou de périphérique, ou si un appareil n'est plus détecté."
        })
        .color(r.on_surface_variant),
        row![
            widgets::hfill(),
            widgets::text_button(
                "Annuler",
                ButtonKind::Text,
                r,
                Some(Message::AskResetAccess(false))
            ),
            widgets::icon_label_button(
                icons::RESTART_ALT,
                "Réinitialiser",
                ButtonKind::Danger,
                r,
                (!app.helper_busy).then_some(Message::ConfirmResetAccess)
            ),
        ]
        .spacing(space::S)
        .align_y(Alignment::Center),
    ]
    .spacing(space::L);
    dialog_card(roles, content.into(), 520.0)
}

fn license_dialog<'a>(roles: Roles, license: License) -> Element<'a, Message> {
    let r = &roles;
    let (name, body_text) = match license {
        License::Gpl2 => ("GNU GPL v2 (TuxRGB, OpenRGB)", crate::credits::GPL2_TEXT),
        License::Lgpl3 => ("GNU LGPL v3 (Qt)", crate::credits::LGPL3_TEXT),
        License::Apache2 => ("Apache License 2.0", crate::credits::APACHE2_TEXT),
        License::Mit => ("Licence MIT", crate::credits::MIT_TEXT),
    };
    let content = column![
        row![
            title(name),
            widgets::hfill(),
            widgets::round_icon_button(icons::CLOSE, r, false, Some(Message::ShowLicense(None)))
        ]
        .align_y(Alignment::Center),
        container(transition::smooth_scroll(
            scrollable(text(body_text).size(12.0).font(iced::Font::MONOSPACE))
                .height(Length::Fixed(480.0)),
            r,
        ))
        .padding(space::M)
        .style(style::card(*r, Surface::Lowest, shape::LARGE)),
    ]
    .spacing(space::L);
    dialog_card(roles, content.into(), 760.0)
}

fn snackbar<'a>(app: &'a App, msg: &'a str) -> Element<'a, Message> {
    let r = &app.roles;
    // Slides up from below the window edge with a slight overshoot.
    let t = app.snack_in.value();
    let lift = (1.0 - t) * -72.0;
    let alpha = t.clamp(0.0, 1.0);
    let bg = with_alpha(r.inverse_surface, alpha);
    let fg = with_alpha(r.inverse_on_surface, alpha);
    container(
        container(
            row![
                icon_filled(icons::INFO, 20.0).color(with_alpha(r.inverse_primary, alpha)),
                text(msg).size(14.0).color(fg).width(Length::Shrink),
            ]
            .spacing(space::M)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([14.0, 20.0]))
        .max_width(680.0)
        .style(move |_| container::Style {
            background: Some(Background::Color(bg)),
            border: iced::Border {
                radius: shape::LARGE.into(),
                ..iced::Border::default()
            },
            shadow: iced::Shadow {
                color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.3 * alpha),
                offset: iced::Vector::new(0.0, 3.0),
                blur_radius: 8.0,
            },
            ..container::Style::default()
        }),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(Alignment::Center)
    .align_y(Alignment::End)
    .padding(Padding {
        top: 0.0,
        right: 0.0,
        left: 0.0,
        bottom: (space::XL + lift).max(0.0),
    })
    .into()
}

/// Page header: big title + supporting text + trailing actions.
pub fn page_header<'a>(
    app: &'a App,
    heading: &'a str,
    supporting: impl crate::i18n::Txt<'a>,
    actions: Element<'a, Message>,
) -> Element<'a, Message> {
    let r = &app.roles;
    row![
        column![widgets::headline(heading), muted(supporting, r)]
            .spacing(space::XS)
            .width(Length::Fill),
        actions,
    ]
    .align_y(Alignment::Center)
    .spacing(space::L)
    .into()
}

pub fn empty_state<'a>(
    app: &'a App,
    glyph: char,
    heading: &'a str,
    supporting: &'a str,
) -> Element<'a, Message> {
    let r = &app.roles;
    center(
        column![
            icon(glyph, 48.0).color(r.on_surface_variant),
            title(heading),
            small(supporting).color(r.on_surface_variant),
        ]
        .spacing(space::S)
        .align_x(Alignment::Center),
    )
    .into()
}
