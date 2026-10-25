//! First-run wizard (and the "reset hardware access" flow).

use super::icons;
use super::settings::diagnostics_list;
use super::widgets::{
    self, body, display, icon_filled, icon_label_button, label, muted, small, title, title_medium,
};
use crate::app::{App, Message, Wizard, WizardStep};
use crate::privileges::{HelperError, Readiness};
use crate::theme::style::{self, ButtonKind, Surface};
use crate::theme::tokens::{shape, space};
use iced::widget::{Space, center, column, container, row, scrollable};
use iced::{Alignment, Element, Length, Padding};

const STEPS: [(WizardStep, &str); 4] = [
    (WizardStep::Welcome, "Bienvenue"),
    (WizardStep::Checks, "Diagnostic"),
    (WizardStep::Access, "Accès matériel"),
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
                container(Space::new().width(32).height(2)).style(style::card(
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
        bullet(app, icons::SHIELD, "Rien d'autre", "Aucun service permanent, aucun programme laissé en root, aucune commande libre : le programme d'aide n'accepte que 4 actions fixes. Réversible depuis Réglages → Réinitialiser."),
    ]
    .spacing(space::L);
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
            super::transition::smooth_scroll(scrollable(content).height(Length::Fill), r,),
            footer,
        ]
        .spacing(space::XXL),
    )
    .padding(Padding::from([space::XXL, space::XXXL]))
    .width(Length::Fixed(820.0))
    .height(Length::Fixed(640.0))
    .style(style::card(*r, Surface::Default, shape::EXTRA_EXTRA_LARGE));
    center(sheet).into()
}
