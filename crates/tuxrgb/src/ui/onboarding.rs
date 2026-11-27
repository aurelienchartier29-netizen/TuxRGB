//! First-run wizard (and the "reset hardware access" flow).

use super::icons;
use super::settings::diagnostics_list;
use super::widgets::{
    self, body, display, icon_filled, icon_label_button, label, muted, small, title, title_medium,
};
use crate::app::{App, DevicesAnswer, FAN_LED_CHOICES, Message, Wizard, WizardStep};
use crate::privileges::{HelperError, Readiness};
use crate::theme::style::{self, ButtonKind, Surface};
use crate::theme::tokens::{shape, space};
use iced::widget::{Space, center, column, container, row, scrollable};

/// The wizard's page (the screenshot tour scrolls it).
pub const SCROLL_ID: &str = "wizard-scroll";
use iced::{Alignment, Element, Length, Padding};

const STEPS: [(WizardStep, &str); 6] = [
    (WizardStep::Welcome, "Bienvenue"),
    (WizardStep::Checks, "Diagnostic"),
    (WizardStep::Access, "Accès matériel"),
    (WizardStep::Devices, "Appareils"),
    (WizardStep::Colors, "Couleurs"),
    (WizardStep::Done, "Terminé"),
];

fn progress(app: &App, current: WizardStep) -> Element<'_, Message> {
    let r = &app.roles;
    let idx = STEPS.iter().position(|(s, _)| *s == current).unwrap_or(0);
    let mut line = row![].spacing(space::S).align_y(Alignment::Center);
    for (i, (_, name)) in STEPS.iter().enumerate() {
        let (surface, glyph) = if i < idx {
            (Surface::Primary, Some(icons::CHECK))
        } else if i == idx {
            (Surface::Secondary, None)
        } else {
            (Surface::Highest, None)
        };
        let dot = container(match glyph {
            Some(g) => Element::from(widgets::icon(g, 16.0)),
            None => small(format!("{}", i + 1)).into(),
        })
        .center_x(Length::Fixed(28.0))
        .center_y(Length::Fixed(28.0))
        .style(style::card(*r, surface, shape::FULL));
        line = line.push(dot);
        line = line.push(if i == idx {
            label(*name)
        } else {
            small(*name).color(r.on_surface_variant)
        });
        if i + 1 < STEPS.len() {
            line = line.push(
                container(Space::new().width(20).height(2)).style(style::card(
                    *r,
                    if i < idx {
                        Surface::Primary
                    } else {
                        Surface::Highest
                    },
                    1.0,
                )),
            );
        }
    }
    line.into()
}

fn bullet<'a>(
    app: &'a App,
    glyph: char,
    heading: &'a str,
    text_body: &'a str,
) -> Element<'a, Message> {
    let r = &app.roles;
    row![
        container(icon_filled(glyph, 22.0).color(r.on_secondary_container))
            .center_x(Length::Fixed(44.0))
            .center_y(Length::Fixed(44.0))
            .style(style::card(*r, Surface::Secondary, shape::LARGE)),
        column![
            title_medium(heading),
            small(text_body).color(r.on_surface_variant)
        ]
        .spacing(space::XXS)
        .width(Length::Fill),
    ]
    .spacing(space::L)
    .align_y(Alignment::Center)
    .into()
}

fn welcome<'a>(app: &'a App, w: &'a Wizard) -> Element<'a, Message> {
    let r = &app.roles;
    column![
        super::logo(112.0),
        display(if w.from_reset { "Reconfigurons l'accès matériel" } else { "Bienvenue dans TuxRGB" }),
        body(if cfg!(windows) {
            "Un seul logiciel pour tout l'éclairage de votre PC : carte mère, RAM, carte graphique, \
             ventilateurs, watercooling, claviers, souris et bandes LED, avec le moteur OpenRGB \
             intégré : rien d'autre à installer."
        } else {
            "Un seul logiciel pour tout l'éclairage de votre PC sous Linux : carte mère, RAM, carte \
             graphique, ventilateurs, watercooling, claviers, souris et bandes LED. Wayland natif, \
             moteur OpenRGB intégré : rien d'autre à installer."
        })
        .color(r.on_surface_variant),
    ]
    .spacing(space::XL)
    .into()
}

fn checks(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    column![
        title("Diagnostic de votre système"),
        muted("TuxRGB vérifie, sans aucun privilège, ce dont OpenRGB a besoin pour accéder au matériel.", r),
        container(diagnostics_list(app))
            .padding(space::L)
            .width(Length::Fill)
            .style(style::card(*r, Surface::Default, shape::EXTRA_LARGE)),
    ]
    .spacing(space::L)
    .into()
}

/// Windows build: PawnIO and administrator mode instead of udev / I2C.
#[cfg(windows)]
fn access_intro(app: &App) -> iced::widget::Column<'_, Message> {
    let r = &app.roles;
    column![
        title("Autoriser l'accès au matériel"),
        muted(
            "Une seule fois, avec la confirmation administrateur de Windows (UAC). TuxRGB :",
            r
        ),
        bullet(app, icons::USB, "USB / HID : rien à faire", "Claviers, souris, casques et contrôleurs d'éclairage USB fonctionnent déjà, sans droits particuliers."),
        bullet(app, icons::MEMORY, "Pilote PawnIO (SMBus)", "Indispensable pour la carte mère et la RAM : installé s'il a été choisi dans l'installateur de TuxRGB."),
        bullet(app, icons::SHIELD, "Mode administrateur sans invite", "Une tâche planifiée de Windows relance TuxRGB avec les droits administrateur, sans confirmation aux démarrages suivants. Réversible depuis Réglages → Réinitialiser."),
    ]
    .spacing(space::L)
}

fn access<'a>(app: &'a App, w: &'a Wizard) -> Element<'a, Message> {
    let r = &app.roles;
    #[cfg(windows)]
    let mut col = access_intro(app);
    #[cfg(not(windows))]
    let mut col = column![
        title("Autoriser l'accès au matériel"),
        muted(
            "Une seule fois, avec votre mot de passe administrateur (fenêtre Polkit de votre bureau). \
             TuxRGB utilise un petit programme d'aide vérifié qui :",
            r
        ),
        bullet(app, icons::USB, "Installe les règles udev d'OpenRGB", "Générées par le moteur embarqué : accès USB/HID réservé à votre session (uaccess), jamais de droits pour tous."),
        bullet(app, icons::MEMORY, "Active l'accès SMBus/I2C", "Charge i2c-dev et le pilote de votre chipset (i2c-piix4 ou i2c-i801), aussi au démarrage."),
        bullet(app, icons::SHIELD, "Rien d'autre", "Aucun service permanent, aucun programme laissé en root, aucune commande libre : le programme d'aide n'accepte que quelques actions fixes. Réversible depuis Réglages → Réinitialiser."),
    ]
    .spacing(space::L);
    col = col.push(super::settings::smbus_choice(app));
    // Off by default: for those who want every frame for their games.
    col = col.push(super::settings::game_mode(app));
    if let Some(Err(e)) = &w.result {
        let msg = match e {
            HelperError::Cancelled if cfg!(windows) => crate::i18n::t(
                "Autorisation refusée. Vous pouvez réessayer ou continuer avec un accès limité (USB / HID).",
            )
            .to_owned(),
            HelperError::Cancelled => {
                "Authentification annulée. Vous pouvez réessayer ou continuer avec un accès limité."
                    .to_owned()
            }
            HelperError::NoPolkit => trf!(
                "Polkit (pkexec) est introuvable. Exécutez ces commandes dans un terminal :\n{}",
                crate::privileges::runner::manual_commands()
            ),
            other => trf!("La configuration a échoué : {}", other),
        };
        col = col.push(
            container(row![icon_filled(icons::ERROR, 22.0), body(msg)].spacing(space::M))
                .padding(space::L)
                .width(Length::Fill)
                .style(style::card(*r, Surface::Error, shape::LARGE)),
        );
    }
    col.into()
}

/// Labels of the LED counts offered for one fan.
const FAN_LED_LABELS: [&str; 6] = ["6", "8", "12", "16", "18", "24"];

/// The devices found, the motherboard's fans in one click, and "is
/// everything controlled?" (with the RAM that needs a restart).
fn devices_step<'a>(app: &'a App, w: &'a Wizard) -> Element<'a, Message> {
    let r = &app.roles;
    // The demo setup only for the screenshot tour.
    let tour = crate::args().screenshot_tour.is_some();
    let real: Vec<_> = app
        .scene
        .devices
        .iter()
        .map(|d| &d.info)
        .filter(|d| !d.is_virtual || tour)
        .collect();
    let leds: usize = real.iter().map(|d| d.led_count).sum();
    let detecting = matches!(
        app.bridge_status,
        tuxrgb_openrgb::BridgeStatus::Starting | tuxrgb_openrgb::BridgeStatus::Detecting { .. }
    );
    let mut col = column![
        title("Vos appareils"),
        muted(
            "Tout ce que TuxRGB pilote s'allume avec l'effet en cours. Vérifiez sur le PC que rien ne manque.",
            r
        ),
    ]
    .spacing(space::L);

    // What was found.
    let mut found = column![].spacing(space::S);
    let summary: Element<'_, Message> = if real.is_empty() {
        row![
            super::motion::loading_indicator(28.0, r.primary, detecting),
            body(if detecting {
                "Recherche des appareils…"
            } else {
                "Aucun appareil trouvé pour l'instant."
            }),
        ]
        .spacing(space::M)
        .align_y(Alignment::Center)
        .into()
    } else {
        row![
            title_medium(trf!("{} appareil(s) · {} LED", real.len(), leds)).width(Length::Fill),
            widgets::icon_label_button(
                icons::REFRESH,
                "Rechercher à nouveau",
                ButtonKind::Text,
                r,
                Some(Message::Rescan)
            ),
        ]
        .spacing(space::M)
        .align_y(Alignment::Center)
        .into()
    };
    found = found.push(summary);
    let mut chips = iced::widget::Row::new().spacing(space::S);
    for d in real.iter().take(12) {
        chips = chips.push(
            container(
                row![
                    widgets::icon(super::devices::kind_icon(d.kind), 18.0).color(r.primary),
                    small(d.display_name().to_owned()),
                ]
                .spacing(space::XS)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([space::XS, space::M]))
            .style(style::card(*r, Surface::Highest, shape::FULL)),
        );
    }
    if !real.is_empty() {
        found = found.push(chips.wrap());
    }
    col = col.push(
        container(found)
            .padding(space::L)
            .width(Length::Fill)
            .style(style::card(*r, Surface::Low, shape::EXTRA_LARGE)),
    );

    // The motherboard's ARGB headers, in one click.
    let headers = app.wizard_headers();
    if !headers.is_empty() {
        let mut fans = column![
            row![
                container(icon_filled(icons::MODE_FAN, 22.0).color(r.on_secondary_container))
                    .center_x(Length::Fixed(44.0))
                    .center_y(Length::Fixed(44.0))
                    .style(style::card(*r, Surface::Secondary, shape::LARGE)),
                column![
                    title_medium("Ventilateurs de la carte mère"),
                    small(
                        "Les connecteurs ARGB ne disent pas ce qui y est branché : indiquez le nombre de ventilateurs et leurs LED, TuxRGB règle chaque connecteur et sépare les ventilateurs sur le canevas."
                    )
                    .color(r.on_surface_variant),
                ]
                .spacing(space::XXS)
                .width(Length::Fill),
            ]
            .spacing(space::L)
            .align_y(Alignment::Center),
            row![
                label("LED par ventilateur").width(Length::Fill),
                widgets::segmented(
                    &FAN_LED_CHOICES
                        .iter()
                        .copied()
                        .zip(FAN_LED_LABELS)
                        .collect::<Vec<_>>(),
                    w.fan_leds,
                    r,
                    Message::WizardFanLeds,
                ),
            ]
            .spacing(space::M)
            .align_y(Alignment::Center),
        ]
        .spacing(space::M);
        for h in &headers {
            let (k1, k2, k3) = (h.device.clone(), h.device.clone(), h.device.clone());
            let zone = h.zone;
            let n = h.fans;
            let now = if h.leds == 0 {
                crate::i18n::t("non réglé").to_owned()
            } else {
                trf!("{} LED aujourd'hui", h.leds)
            };
            fans =
                fans.push(
                    row![
                        column![
                            label(crate::i18n::t(&h.name).to_owned()),
                            small(now).color(r.on_surface_variant)
                        ]
                        .spacing(space::XXS)
                        .width(Length::Fill),
                        widgets::round_icon_button(
                            icons::HIGHLIGHT,
                            r,
                            false,
                            (h.leds > 0).then(|| Message::IdentifyZone(k3, zone)),
                        ),
                        widgets::round_icon_button(
                            icons::REMOVE,
                            r,
                            false,
                            (n > 0).then(|| Message::WizardHeaderFans(k1, zone, n - 1)),
                        ),
                        container(body(trf!("{} ventilateur(s)", n)))
                            .width(Length::Fixed(130.0))
                            .align_x(Alignment::Center),
                        widgets::round_icon_button(
                            icons::ADD,
                            r,
                            false,
                            (n < tuxrgb_core::lighting::MAX_PARTS)
                                .then(|| Message::WizardHeaderFans(k2, zone, n + 1)),
                        ),
                    ]
                    .spacing(space::S)
                    .align_y(Alignment::Center),
                );
        }
        fans = fans.push(
            row![
                small(if w.fans_done {
                    "Réglé : chaque ventilateur est un élément du canevas (Appareils)."
                } else {
                    "Ajustable ensuite dans Appareils, ventilateur par ventilateur."
                })
                .color(r.on_surface_variant)
                .width(Length::Fill),
                icon_label_button(
                    if w.fans_done {
                        icons::CHECK
                    } else {
                        icons::AUTO_AWESOME
                    },
                    "Configurer automatiquement",
                    if w.fans_done {
                        ButtonKind::Tonal
                    } else {
                        ButtonKind::Filled
                    },
                    r,
                    Some(Message::WizardConfigureFans),
                ),
            ]
            .spacing(space::M)
            .align_y(Alignment::Center),
        );
        col = col.push(
            container(fans)
                .padding(space::L)
                .width(Length::Fill)
                .style(style::card(*r, Surface::Low, shape::EXTRA_LARGE)),
        );
    }

    // Is everything controlled?
    let mut ask = column![
        title_medium("Tous vos périphériques sont-ils contrôlés par TuxRGB ?"),
        row![
            icon_label_button(
                icons::CHECK_CIRCLE,
                "Oui, tout s'allume",
                if w.answer == Some(DevicesAnswer::AllGood) {
                    ButtonKind::Filled
                } else {
                    ButtonKind::Tonal
                },
                r,
                Some(Message::WizardDevicesAnswer(Some(DevicesAnswer::AllGood))),
            ),
            icon_label_button(
                icons::HELP,
                "Non, il en manque",
                if matches!(
                    w.answer,
                    Some(
                        DevicesAnswer::Missing
                            | DevicesAnswer::MissingRam
                            | DevicesAnswer::MissingOther
                    )
                ) {
                    ButtonKind::Filled
                } else {
                    ButtonKind::Outlined
                },
                r,
                Some(Message::WizardDevicesAnswer(Some(DevicesAnswer::Missing))),
            ),
        ]
        .spacing(space::S),
    ]
    .spacing(space::M);
    if matches!(
        w.answer,
        Some(DevicesAnswer::Missing | DevicesAnswer::MissingRam | DevicesAnswer::MissingOther)
    ) {
        ask = ask.push(
            row![
                small("Qu'est-ce qui manque ?").color(r.on_surface_variant),
                widgets::segmented(
                    &[
                        (Some(DevicesAnswer::MissingRam), "La RAM"),
                        (Some(DevicesAnswer::MissingOther), "Un autre appareil"),
                    ],
                    w.answer.filter(|a| *a != DevicesAnswer::Missing),
                    r,
                    Message::WizardDevicesAnswer,
                ),
            ]
            .spacing(space::M)
            .align_y(Alignment::Center),
        );
    }
    match w.answer {
        Some(DevicesAnswer::MissingRam) if !app.config.smbus_lighting => {
            ask = ask.push(
                column![
                    small(
                        "L'éclairage de la RAM est désactivé par prudence : activez-le, TuxRGB relance le moteur et cherche vos barrettes."
                    )
                    .color(r.on_surface_variant),
                    super::settings::smbus_choice(app),
                ]
                .spacing(space::S),
            );
        }
        Some(DevicesAnswer::MissingRam) => {
            ask = ask.push(
                container(
                    column![
                        row![
                            icon_filled(icons::RESTART_ALT, 24.0).color(r.on_tertiary_container),
                            title_medium("Redémarrez le PC").width(Length::Fill),
                        ]
                        .spacing(space::M)
                        .align_y(Alignment::Center),
                        small(
                            "La RAM n'est accessible qu'une fois le pilote SMBus chargé au démarrage du système. Redémarrez maintenant : TuxRGB se relance avec la session et l'assistant reprend à cette étape. Ou passez, et redémarrez plus tard."
                        ),
                        row![
                            widgets::hfill(),
                            widgets::text_button(
                                "Plus tard",
                                ButtonKind::Text,
                                r,
                                Some(Message::WizardNext)
                            ),
                            icon_label_button(
                                icons::RESTART_ALT,
                                "Redémarrer maintenant",
                                ButtonKind::Filled,
                                r,
                                Some(Message::WizardReboot),
                            ),
                        ]
                        .spacing(space::S)
                        .align_y(Alignment::Center),
                    ]
                    .spacing(space::S),
                )
                .padding(space::L)
                .width(Length::Fill)
                .style(style::card(*r, Surface::Tertiary, shape::LARGE)),
            );
        }
        Some(DevicesAnswer::MissingOther) => {
            let mut tips = column![].spacing(space::XS);
            for tip in [
                "Fermez les autres logiciels RGB (iCUE, Armoury Crate, SignalRGB, OpenRGB) : un seul programme à la fois.",
                "Branchez-le directement sur le PC (pas sur un hub), puis « Rechercher à nouveau ».",
                "Un appareil réseau (WLED, Hue, Govee…) s'ajoute dans Réglages → Lumières réseau.",
                "La liste des appareils pris en charge est sur openrgb.org/devices.",
            ] {
                tips = tips.push(
                    row![
                        widgets::icon(icons::ARROW_FORWARD, 16.0).color(r.primary),
                        small(tip).color(r.on_surface_variant).width(Length::Fill),
                    ]
                    .spacing(space::S),
                );
            }
            ask = ask.push(tips);
        }
        _ => {}
    }
    col = col.push(
        container(ask)
            .padding(space::L)
            .width(Length::Fill)
            .style(style::card(*r, Surface::Low, shape::EXTRA_LARGE)),
    );
    col.into()
}

/// White balance of the LEDs, device by device.
fn colors(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    column![
        title("Le blanc de vos LED"),
        muted(
            "Les LED de la RAM, du boîtier ou d'un clavier n'ont pas toutes le même blanc : réglez-les \
             une par une pour qu'il ressemble à celui de l'écran, et vos couleurs seront les mêmes \
             partout. Étape facultative.",
            r
        ),
        super::white_balance::panel(app),
    ]
    .spacing(space::L)
    .into()
}

fn done<'a>(app: &'a App, w: &'a Wizard) -> Element<'a, Message> {
    let r = &app.roles;
    let readiness = app.diagnostics.as_ref().map(|d| d.readiness());
    let configured = matches!(w.result, Some(Ok(_)));
    let (glyph, heading, text_body) = if configured || readiness == Some(Readiness::Ready) {
        (
            icons::TASK_ALT,
            "Tout est prêt !",
            "L'accès matériel est configuré. OpenRGB redémarre et détecte vos périphériques ; ils apparaîtront dans « Appareils » d'ici quelques secondes.",
        )
    } else {
        (
            icons::INFO,
            "Prêt, avec un accès limité",
            "Les périphériques déjà accessibles fonctionneront. Vous pourrez configurer l'accès complet plus tard dans Réglages → Accès matériel.",
        )
    };
    let mut col = column![
        container(icon_filled(glyph, 48.0).color(r.on_tertiary_container))
            .center_x(Length::Fixed(96.0))
            .center_y(Length::Fixed(96.0))
            .style(style::card(*r, Surface::Tertiary, shape::EXTRA_EXTRA_LARGE)),
        display(heading),
        body(text_body).color(r.on_surface_variant),
    ]
    .spacing(space::L);
    col = col.push(
        container(
            row![
                widgets::icon(icons::LOGIN, 22.0).color(r.primary),
                column![
                    label("Lancer TuxRGB à l'ouverture de session"),
                    small("Réapplique votre éclairage dès le démarrage, discrètement dans la zone de notification.")
                        .color(r.on_surface_variant),
                ]
                .spacing(space::XXS)
                .width(Length::Fill),
                widgets::switch(w.autostart, r, Message::WizardAutostart),
            ]
            .spacing(space::M)
            .align_y(Alignment::Center),
        )
        .padding(space::L)
        .width(Length::Fill)
        .style(style::card(*r, Surface::High, shape::LARGE)),
    );
    if let Some(Ok(report)) = &w.result {
        let mut steps = column![].spacing(space::XXS);
        for s in &report.steps {
            steps = steps.push(small(format!("✓ {s}")).color(r.on_surface_variant));
        }
        col = col.push(
            container(super::transition::smooth_scroll(
                scrollable(steps).height(Length::Fixed(120.0)),
                r,
            ))
            .padding(space::M)
            .width(Length::Fill)
            .style(style::card(*r, Surface::Lowest, shape::LARGE)),
        );
    }
    col.into()
}

pub fn view<'a>(app: &'a App, w: &'a Wizard) -> Element<'a, Message> {
    let r = &app.roles;
    let busy = app.helper_busy;
    let content: Element<'_, Message> = match w.step {
        WizardStep::Welcome => welcome(app, w),
        WizardStep::Checks => checks(app),
        WizardStep::Access => access(app, w),
        WizardStep::Devices => devices_step(app, w),
        WizardStep::Colors => colors(app),
        WizardStep::Done => done(app, w),
    };
    let back = (w.step != WizardStep::Welcome && w.step != WizardStep::Done)
        .then(|| widgets::text_button("Retour", ButtonKind::Text, r, Some(Message::WizardBack)));
    let actions: Element<'_, Message> = match w.step {
        WizardStep::Welcome => icon_label_button(
            icons::ARROW_FORWARD,
            "Commencer",
            ButtonKind::Filled,
            r,
            Some(Message::WizardNext),
        )
        .into(),
        WizardStep::Checks => icon_label_button(
            icons::ARROW_FORWARD,
            "Continuer",
            ButtonKind::Filled,
            r,
            Some(Message::WizardNext),
        )
        .into(),
        WizardStep::Access => row![
            widgets::text_button(
                "Plus tard",
                ButtonKind::Outlined,
                r,
                (!busy).then_some(Message::WizardSkipAccess)
            ),
            icon_label_button(
                icons::ADMIN_PANEL_SETTINGS,
                if busy && cfg!(windows) {
                    "En attente de la confirmation Windows…"
                } else if busy {
                    "En attente du mot de passe…"
                } else if cfg!(windows) {
                    "Autoriser (administrateur)"
                } else {
                    "Autoriser (mot de passe admin)"
                },
                ButtonKind::Filled,
                r,
                (!busy).then_some(Message::StartHardwareSetup)
            ),
        ]
        .spacing(space::S)
        .into(),
        WizardStep::Devices => icon_label_button(
            icons::ARROW_FORWARD,
            "Continuer",
            ButtonKind::Filled,
            r,
            Some(Message::WizardNext),
        )
        .into(),
        WizardStep::Colors => row![
            widgets::text_button("Passer", ButtonKind::Outlined, r, Some(Message::WizardNext)),
            icon_label_button(
                icons::ARROW_FORWARD,
                "Continuer",
                ButtonKind::Filled,
                r,
                Some(Message::WizardNext),
            ),
        ]
        .spacing(space::S)
        .into(),
        WizardStep::Done => icon_label_button(
            icons::ROCKET_LAUNCH,
            "Ouvrir TuxRGB",
            ButtonKind::Filled,
            r,
            Some(Message::WizardFinish),
        )
        .into(),
    };
    let mut footer = row![].spacing(space::S).align_y(Alignment::Center);
    if let Some(b) = back {
        footer = footer.push(b);
    }
    footer = footer.push(widgets::hfill()).push(actions);

    let sheet = container(
        column![
            progress(app, w.step),
            super::transition::smooth_scroll(
                scrollable(content).id(SCROLL_ID).height(Length::Fill),
                r,
            ),
            footer,
        ]
        .spacing(space::XXL),
    )
    .padding(Padding::from([space::XXL, space::XXXL]))
    .width(Length::Fixed(820.0))
    // As tall as the window allows, up to 720 px.
    .height(Length::Fill)
    .max_height(720.0)
    .style(style::card(*r, Surface::Default, shape::EXTRA_EXTRA_LARGE));
    center(sheet).padding(space::L).into()
}
