//! Settings: appearance, performance, start-up, hardware access, engine, about.

use super::motion::button;
use super::widgets::{
    self, badge, body, icon_filled, icon_label_button, label, muted, section, segmented,
    setting_row, small, switch, title, title_medium,
};
use super::{icons, page_header};
use crate::app::{App, Message, SettingsView};
use crate::config::{
    AccentSource, LanguagePref, MotionPref, RendererPref, SchemeVariant, ServerKind, ThemeMode,
};
use crate::paths::PackageKind;
use crate::privileges::diagnostics::RulesState;
use crate::privileges::{self, Readiness};
use crate::theme::style::{self, ButtonKind, Surface};
use crate::theme::to_iced;
use crate::theme::tokens::{shape, space};
use iced::widget::{Space, column, container, row, scrollable};
use iced::{Alignment, Element, Length, Padding};
use tuxrgb_core::Rgb;
use tuxrgb_openrgb::BridgeStatus;

const ACCENTS: [Rgb; 8] = [
    Rgb::new(61, 174, 233),
    Rgb::new(124, 77, 255),
    Rgb::new(233, 30, 99),
    Rgb::new(255, 87, 34),
    Rgb::new(255, 193, 7),
    Rgb::new(76, 175, 80),
    Rgb::new(0, 150, 136),
    Rgb::new(96, 125, 139),
];

fn group<'a>(
    app: &'a App,
    glyph: char,
    heading: &'a str,
    rows: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let r = &app.roles;
    let mut col = column![section(glyph, heading, r)].spacing(space::XS);
    for row in rows {
        col = col.push(row);
    }
    container(col)
        .padding(Padding::from([space::L, space::XL]))
        .width(Length::Fill)
        .style(style::card(*r, Surface::Default, shape::EXTRA_LARGE))
        .into()
}

fn appearance(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let a = &app.config.appearance;
    let session = crate::desktop::session();
    let detected = match app.system.accent {
        Some(c) => trf!(
            "Accent du bureau {} : {} (via {})",
            session.desktop.short_name(),
            c.to_hex().to_uppercase(),
            app.system.source
        ),
        None => {
            crate::i18n::t("Aucune couleur d'accent fournie par le bureau : bleu Breeze utilisé.")
                .to_owned()
        }
    };
    let system_lang = tuxrgb_core::i18n::system_language().native_name();
    let mut swatches = row![].spacing(space::S);
    for c in ACCENTS {
        let selected = a.accent == AccentSource::Custom && a.custom_accent == c;
        swatches = swatches.push(
            button(Space::new().width(28).height(28))
                .padding(0)
                .style({
                    let col = to_iced(c);
                    let ring = r.on_surface;
                    move |_, status| {
                        // Hover: the circle morphs towards a squircle and
                        // shows a faint ring (animated by the motion button).
                        let hot = matches!(
                            status,
                            iced::widget::button::Status::Hovered
                                | iced::widget::button::Status::Pressed
                        );
                        iced::widget::button::Style {
                            background: Some(iced::Background::Color(col)),
                            border: iced::Border {
                                color: if selected {
                                    ring
                                } else if hot {
                                    crate::theme::with_alpha(ring, 0.45)
                                } else {
                                    crate::theme::with_alpha(ring, 0.0)
                                },
                                width: if selected || hot { 3.0 } else { 0.0 },
                                radius: if selected || hot {
                                    10.0.into()
                                } else {
                                    14.0.into()
                                },
                            },
                            ..Default::default()
                        }
                    }
                })
                .on_press(Message::SetCustomAccent(c)),
        );
    }
    let variants: Vec<(SchemeVariant, &str)> =
        SchemeVariant::ALL.iter().map(|v| (*v, v.label())).collect();
    group(
        app,
        icons::PALETTE,
        "Apparence",
        vec![
            setting_row(
                icons::TRANSLATE,
                "Langue",
                trf!("Langue de l'interface. Langue du système : {}", system_lang),
                segmented(
                    &[
                        (LanguagePref::System, "Système"),
                        (LanguagePref::French, "Français"),
                        (LanguagePref::English, "English"),
                    ],
                    app.config.language,
                    r,
                    Message::SetLanguage,
                ),
                r,
            ),
            setting_row(
                icons::CONTRAST,
                "Thème",
                "Suit le mode clair/sombre du bureau par défaut.",
                segmented(
                    &[
                        (ThemeMode::System, "Système"),
                        (ThemeMode::Light, "Clair"),
                        (ThemeMode::Dark, "Sombre"),
                    ],
                    a.theme,
                    r,
                    Message::SetThemeMode,
                ),
                r,
            ),
            setting_row(
                icons::COLORIZE,
                "Couleur d'accent",
                detected,
                segmented(
                    &[
                        (AccentSource::Desktop, "Bureau"),
                        (AccentSource::Custom, "Personnalisée"),
                    ],
                    a.accent,
                    r,
                    Message::SetAccentSource,
                ),
                r,
            ),
            container(swatches)
                .padding(Padding {
                    left: 56.0,
                    ..Padding::ZERO
                })
                .into(),
            setting_row(
                icons::FORMAT_PAINT,
                "Palette Material",
                "Couleurs dynamiques Material (HCT) générées depuis l'accent ; « Vive » garde la teinte du bureau, « Expressive » la fait tourner.",
                segmented(&variants, a.variant, r, Message::SetVariant),
                r,
            ),
            setting_row(
                icons::BOLT,
                "Animations",
                "Ressorts M3 Expressive, ou mouvements sobres.",
                segmented(
                    &[
                        (MotionPref::System, "Système"),
                        (MotionPref::Expressive, "Expressives"),
                        (MotionPref::Reduced, "Réduites"),
                    ],
                    a.motion,
                    r,
                    Message::SetMotion,
                ),
                r,
            ),
            setting_row(
                icons::SPEED,
                "Vitesse des animations de l'application",
                "Durée de toutes les transitions et animations de l'interface (pas celles de l'éclairage).",
                row![
                    container(
                        super::motion::slider(
                            crate::theme::motion::SPEED_MIN..=crate::theme::motion::SPEED_MAX,
                            a.animation_speed,
                            Message::SetAnimationSpeed,
                        )
                        .step(0.05)
                        .stops(&[1.0])
                        .format(|v| format!("{:.0} %", v * 100.0))
                        .colors(super::motion::SliderColors::standard(r))
                        .height(40.0),
                    )
                    .width(Length::Fixed(260.0)),
                    iced::widget::text(format!("{:.0} %", a.animation_speed * 100.0))
                        .size(14.0)
                        .color(r.on_surface_variant)
                        .width(Length::Fixed(52.0))
                        .align_x(iced::alignment::Horizontal::Right),
                ]
                .spacing(space::S)
                .align_y(Alignment::Center),
                r,
            ),
            setting_row(
                icons::ANIMATION,
                "Transitions douces des LED",
                "Fondus entre couleurs, effets, préréglages et accent du bureau sur tout le matériel.",
                switch(app.config.smooth_transitions, r, Message::SetTransitions),
                r,
            ),
        ],
    )
}

fn performance(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    group(
        app,
        icons::SPEED,
        "Performances",
        vec![
            setting_row(
                icons::TIMER,
                "Fréquence du moteur d'effets",
                "Images par seconde envoyées aux LED (les effets statiques ne consomment rien).",
                segmented(
                    &[(30u32, "30"), (60, "60"), (90, "90"), (120, "120")],
                    app.config.fps,
                    r,
                    Message::SetFps,
                ),
                r,
            ),
            setting_row(
                icons::MONITOR,
                "Rendu de l'interface",
                if app.restart_required {
                    "Redémarrez TuxRGB pour appliquer le nouveau moteur de rendu."
                } else {
                    "wgpu sur le GPU intégré par défaut, pour ne pas réveiller la carte graphique dédiée."
                },
                segmented(
                    &[
                        (RendererPref::GpuLowPower, "GPU éco"),
                        (RendererPref::GpuHighPerformance, "GPU perf."),
                        (RendererPref::Software, "Logiciel"),
                    ],
                    app.config.renderer,
                    r,
                    Message::SetRenderer,
                ),
                r,
            ),
        ],
    )
}

fn startup(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let b = &app.config.behavior;
    let tray_note = if app.tray_available {
        "Le bouton ✕ range TuxRGB dans la zone de notification ; « Réduire » le garde dans la barre des tâches."
    } else {
        "Zone de notification indisponible (sous GNOME, installez l'extension AppIndicator)."
    };
    group(
        app,
        icons::ROCKET_LAUNCH,
        "Démarrage et zone de notification",
        vec![
            setting_row(
                icons::LOGIN,
                "Lancer à l'ouverture de session",
                "Réapplique votre éclairage dès la connexion.",
                switch(b.autostart, r, Message::SetAutostart),
                r,
            ),
            setting_row(
                icons::CLOSE_FULLSCREEN,
                "Toujours démarrer réduit",
                "Même lancé depuis le menu (à l'ouverture de session, TuxRGB démarre de toute façon dans la zone de notification).",
                switch(b.start_minimized, r, Message::SetStartMinimized),
                r,
            ),
            setting_row(
                icons::DESKTOP_WINDOWS,
                "Fermer = réduire dans la zone de notification",
                tray_note,
                switch(b.close_to_tray, r, Message::SetCloseToTray),
                r,
            ),
            setting_row(
                icons::POWER_SETTINGS_NEW,
                "Éteindre les LED en quittant",
                "Sinon le dernier effet reste affiché par le matériel.",
                switch(b.turn_off_on_exit, r, Message::SetTurnOffOnExit),
                r,
            ),
        ],
    )
}

fn check_row<'a>(
    app: &'a App,
    ok: Option<bool>,
    heading: &'a str,
    value: String,
) -> Element<'a, Message> {
    let r = &app.roles;
    let (glyph, color) = match ok {
        Some(true) => (icons::CHECK_CIRCLE, r.success),
        Some(false) => (icons::ERROR, r.error),
        None => (icons::INFO, r.on_surface_variant),
    };
    row![
        icon_filled(glyph, 20.0).color(color),
        label(heading).width(Length::FillPortion(2)),
        small(value)
            .color(r.on_surface_variant)
            .width(Length::FillPortion(3)),
    ]
    .spacing(space::M)
    .align_y(Alignment::Center)
    .padding(Padding::from([space::XS, 0.0]))
    .into()
}

pub fn diagnostics_list(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let Some(d) = &app.diagnostics else {
        return muted("Vérification en cours…", r).into();
    };
    let flatpak = d.package == PackageKind::Flatpak;
    column![
        check_row(
            app,
            match d.rules {
                RulesState::Unknown => None,
                RulesState::Duplicate => Some(false),
                r => Some(r.ok()),
            },
            "Règles udev (60-openrgb.rules)",
            match (&d.rules_path, d.rules) {
                (Some(p), RulesState::Duplicate) => trf!(
                    "En double : {} masque celles de la distribution – « Configurer l'accès » corrige",
                    p.display()
                ),
                (Some(p), _) => format!(
                    "{} – {}{}",
                    crate::i18n::t(d.rules.label()),
                    p.display(),
                    if d.extra_rules {
                        crate::i18n::t(" + compléments TuxRGB")
                    } else {
                        ""
                    }
                ),
                (None, _) => crate::i18n::t(d.rules.label()).to_owned(),
            }
        ),
        check_row(
            app,
            Some(d.world_accessible_nodes.is_empty() && !d.rules_outdated),
            "Permissions strictes",
            if !d.world_accessible_nodes.is_empty() {
                trf!(
                    "accès pour tous sur {} – règle trop large installée par un autre logiciel",
                    d.world_accessible_nodes.join(", ")
                )
            } else if d.rules_outdated {
                crate::i18n::t("règles d'une version précédente : « Configurer l'accès » les renforce").into()
            } else {
                crate::i18n::t("accès réservé à votre session (uaccess), aucun droit universel").into()
            }
        ),
        check_row(
            app,
            Some(d.i2c_dev_loaded),
            "Module i2c-dev",
            crate::i18n::t(if d.i2c_dev_loaded {
                "chargé"
            } else {
                "non chargé"
            })
            .into()
        ),
        check_row(
            app,
            Some(d.smbus_driver.is_some()),
            "Pilote SMBus du chipset",
            d.smbus_driver
                .clone()
                .unwrap_or_else(|| crate::i18n::t("aucun (i2c-piix4 / i2c-i801)").into())
        ),
        check_row(
            app,
            Some(d.i2c_nodes > 0 && d.i2c_accessible > 0),
            "Bus I2C accessibles",
            format!("{} / {}", d.i2c_accessible, d.i2c_nodes)
        ),
        check_row(
            app,
            Some(d.hidraw_nodes == 0 || d.hidraw_accessible > 0),
            "Périphériques HID accessibles",
            format!("{} / {}", d.hidraw_accessible, d.hidraw_nodes)
        ),
        check_row(
            app,
            Some(d.pkexec_available),
            "Polkit",
            if flatpak {
                "flatpak-spawn --host pkexec".into()
            } else if d.pkexec_available {
                crate::i18n::t("pkexec disponible").into()
            } else {
                crate::i18n::t("pkexec introuvable").into()
            }
        ),
        check_row(
            app,
            Some(d.openrgb_binary.is_some()),
            "Moteur OpenRGB",
            match &d.openrgb_binary {
                Some(p) => format!("{} ({})", p.display(), crate::i18n::t(d.openrgb_origin)),
                None => crate::i18n::t("introuvable").into(),
            }
        ),
        check_row(
            app,
            None,
            "Format de l'application",
            crate::i18n::t(d.package.label()).to_owned()
        ),
        check_row(
            app,
            Some(crate::desktop::session().desktop.has_accent()),
            "Bureau",
            {
                let s = crate::desktop::session();
                let name = if s.desktop_raw.is_empty() {
                    crate::i18n::t("inconnu").to_owned()
                } else {
                    s.desktop_raw.clone()
                };
                format!(
                    "{name} · {}",
                    if s.wayland { "Wayland" } else { "X11" }
                )
            }
        ),
        check_row(
            app,
            Some(crate::desktop::session().distro_supported()),
            "Distribution",
            {
                let s = crate::desktop::session();
                if s.immutable {
                    trf!("{} (système immuable)", s.distro)
                } else {
                    s.distro.clone()
                }
            }
        ),
    ]
    .spacing(space::XXS)
    .into()
}

/// Shown when OpenRGB is connected but no RAM module was detected.
fn ram_hints(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let connected = app.bridge_status.is_connected();
    let has_ram = app
        .devices
        .iter()
        .any(|d| d.kind == tuxrgb_core::DeviceKind::Dram);
    let Some(findings) = app.findings.clone().filter(|_| connected && !has_ram) else {
        return Space::new().height(0).into();
    };
    let findings = privileges::report::Findings {
        dram_detected: 0,
        ..findings
    };
    let mut col = column![
        row![
            icon_filled(icons::MEMORY_ALT, 20.0).color(r.tertiary),
            label("Aucune barrette de RAM RGB détectée"),
        ]
        .spacing(space::S)
        .align_y(Alignment::Center)
    ]
    .spacing(space::XS);
    for h in findings.ram_hints() {
        col = col.push(small(format!("• {}", crate::i18n::t(&h))).color(r.on_surface_variant));
    }
    if let Some(how) = &app.smbus_manual {
        col = col.push(
            container(
                column![
                    row![
                        icon_filled(icons::TERMINAL, 20.0).color(r.primary),
                        label("À faire à la main sur ce système"),
                    ]
                    .spacing(space::S)
                    .align_y(Alignment::Center),
                    small(how.clone()).color(r.on_surface_variant),
                ]
                .spacing(space::XS),
            )
            .padding(space::M)
            .style(style::card(*r, Surface::Highest, shape::MEDIUM)),
        );
    }
    if app.smbus_reboot_pending {
        col = col.push(
            row![
                icon_filled(icons::RESTART_ALT, 20.0).color(r.primary),
                label(
                    "Correctif appliqué : redémarrez l'ordinateur pour que la RAM soit détectée."
                ),
            ]
            .spacing(space::S)
            .align_y(Alignment::Center),
        );
    } else if findings.smbus_unbound && !findings.acpi_lax {
        col = col.push(
            row![
                icon_label_button(
                    icons::LOCK_OPEN,
                    if app.helper_busy {
                        "Configuration…"
                    } else {
                        "Débloquer l'accès SMBus"
                    },
                    ButtonKind::Filled,
                    r,
                    (!app.helper_busy).then_some(Message::EnableSmbus),
                ),
                small(
                    "Mot de passe administrateur demandé, puis redémarrage. TuxRGB choisit la \
                     méthode de votre système (rpm-ostree, grubby, GRUB, systemd-boot…) et ne \
                     change rien si l'accès fonctionne déjà. Annulable avec « Réinitialiser les \
                     permissions ».",
                )
                .color(r.on_surface_variant),
            ]
            .spacing(space::M)
            .align_y(Alignment::Center),
        );
    }
    col = col.push(
        small(
            "Le rapport matériel rassemble tout ce qu'il faut pour diagnostiquer le problème \
             (bus I2C, pilotes, journal du noyau et de détection).",
        )
        .color(r.on_surface_variant),
    );
    container(col)
        .padding(space::L)
        .width(Length::Fill)
        .style(style::card(*r, Surface::High, shape::LARGE))
        .into()
}

fn hardware(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let readiness = app.diagnostics.as_ref().map(|d| d.readiness());
    let (surface, glyph, verdict) = match readiness {
        Some(Readiness::Ready) => (
            Surface::Secondary,
            icons::TASK_ALT,
            "Accès matériel complet",
        ),
        Some(Readiness::Partial) => (
            Surface::Tertiary,
            icons::WARNING,
            "Accès partiel : USB/HID uniquement",
        ),
        Some(Readiness::NotConfigured) => {
            (Surface::Error, icons::LOCK, "Accès matériel non configuré")
        }
        None => (Surface::High, icons::SYNC, "Vérification…"),
    };
    let busy = app.helper_busy;
    group(
        app,
        icons::ADMIN_PANEL_SETTINGS,
        "Accès matériel",
        vec![
            // Status change: the colour eases, the verdict fades in.
            widgets::animated_surface(
                super::transition::swap(
                    readiness.map_or(0, |r| r as u64 + 1),
                    style::surface_colors(r, surface).0,
                    row![icon_filled(glyph, 24.0), title_medium(verdict)]
                        .spacing(space::M)
                        .align_y(Alignment::Center),
                ),
                r,
                surface,
                shape::LARGE,
                Padding::from([space::M, space::L]),
            ),
            container(diagnostics_list(app)).padding(Padding::from([space::S, 0.0])).into(),
            ram_hints(app),
            row![
                icon_label_button(icons::REFRESH, "Revérifier", ButtonKind::Outlined, r, Some(Message::RunDiagnostics)),
                icon_label_button(icons::CONTENT_COPY, "Copier le rapport matériel", ButtonKind::Outlined, r, Some(Message::CopyReport)),
                icon_label_button(
                    icons::LOCK_OPEN,
                    if busy { "Configuration…" } else { "Configurer l'accès" },
                    ButtonKind::Tonal,
                    r,
                    (!busy).then_some(Message::StartHardwareSetup)
                ),
                widgets::hfill(),
                icon_label_button(
                    icons::RESTART_ALT,
                    "Réinitialiser les permissions / Accès matériel",
                    ButtonKind::Danger,
                    r,
                    (!busy).then_some(Message::AskResetAccess(true))
                ),
            ]
            .spacing(space::S)
            .align_y(Alignment::Center)
            .into(),
            small(
                "La réinitialisation retire les règles udev et la configuration I2C installées par TuxRGB, \
                 efface l'état du premier lancement et relance l'assistant – utile après un changement de composant.",
            )
            .color(r.on_surface_variant)
            .into(),
        ],
    )
}

fn engine(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let status = match &app.bridge_status {
        BridgeStatus::Connected {
            devices,
            controllable,
            protocol,
            server,
        } => {
            trf!(
                "{} · protocole SDK v{} · {}/{} pilotables",
                crate::i18n::t(server),
                protocol,
                controllable,
                devices
            )
        }
        BridgeStatus::Error(e) => trf!("Erreur : {}", crate::i18n::t(e)),
        BridgeStatus::Reconnecting(e) => trf!("Reconnexion ({})", crate::i18n::t(e)),
        _ => app.status_text(),
    };
    group(
        app,
        icons::DNS,
        "Moteur matériel (OpenRGB)",
        vec![
            setting_row(
                icons::HUB,
                "Source",
                "Embarqué : rien à installer. Externe : un serveur OpenRGB déjà lancé (port 6742).",
                segmented(
                    &[
                        (ServerKind::Embedded, "Embarqué"),
                        (ServerKind::External, "Externe"),
                    ],
                    app.config.server.kind,
                    r,
                    Message::SetServerKind,
                ),
                r,
            ),
            setting_row(
                icons::TERMINAL,
                "État",
                status,
                row![
                    widgets::round_icon_button(icons::REFRESH, r, false, Some(Message::Rescan)),
                    widgets::round_icon_button(
                        icons::RESTART_ALT,
                        r,
                        false,
                        Some(Message::RestartEngine)
                    ),
                ]
                .spacing(space::XS),
                r,
            ),
            setting_row(
                icons::BLUR_ON,
                "Appareils de démonstration",
                "Afficher un setup virtuel tant qu'aucun matériel n'est détecté.",
                switch(app.config.show_demo_devices, r, Message::SetDemoDevices),
                r,
            ),
        ],
    )
}

fn about(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    container(
        row![
            super::logo(72.0),
            column![
                title(format!("TuxRGB {}", env!("CARGO_PKG_VERSION"))),
                body(trf!(
                    "Éclairage RGB pour Linux · Wayland natif · moteur {} embarqué",
                    tuxrgb_openrgb::OPENRGB_LABEL
                ))
                .color(r.on_surface_variant),
                row![
                    badge(small("GPL-2.0-or-later"), r, Surface::Highest),
                    badge(small("100 % Rust (interface)"), r, Surface::Highest),
                    badge(small("io.github.tuxrgb.TuxRGB"), r, Surface::Highest),
                ]
                .spacing(space::XS),
            ]
            .spacing(space::XS)
            .width(Length::Fill),
            column![
                icon_label_button(
                    icons::VOLUNTEER_ACTIVISM,
                    "Crédits & licences open source",
                    ButtonKind::Filled,
                    r,
                    Some(Message::ShowSettings(SettingsView::Credits))
                ),
                icon_label_button(
                    icons::LOGOUT,
                    "Quitter TuxRGB",
                    ButtonKind::Outlined,
                    r,
                    Some(Message::Quit)
                ),
            ]
            .spacing(space::S)
            .align_x(Alignment::End),
        ]
        .spacing(space::XL)
        .align_y(Alignment::Center),
    )
    .padding(space::XL)
    .style(style::card(
        *r,
        Surface::Default,
        shape::EXTRA_LARGE_INCREASED,
    ))
    .into()
}

pub fn view(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let content = column![
        appearance(app),
        hardware(app),
        engine(app),
        startup(app),
        performance(app),
        about(app),
        Space::new().height(space::XL),
    ]
    .spacing(space::L);
    column![
        page_header(
            app,
            "Réglages",
            "Apparence, accès matériel, démarrage et performances.",
            Space::new().into()
        ),
        super::transition::smooth_scroll(
            scrollable(container(content).padding(Padding {
                right: space::L,
                ..Padding::ZERO
            }))
            .height(Length::Fill),
            r,
        ),
    ]
    .spacing(space::XL)
    .into()
}
