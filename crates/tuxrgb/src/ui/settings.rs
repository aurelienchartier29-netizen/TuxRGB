//! Settings: appearance, performance, start-up, hardware access, engine,
//! updates, about.

use super::motion::button;
use super::widgets::{
    self, badge, body, icon_filled, icon_label_button, label, muted, section, segmented,
    setting_row, small, switch, text_button, title, title_medium,
};
use super::{icons, page_header};
use crate::app::{App, Message, SettingsView};
use crate::config::{
    AccentSource, LanguagePref, MotionPref, RendererPref, SchemeVariant, ServerKind, ThemeMode,
};
#[cfg(not(windows))]
use crate::paths::PackageKind;
#[cfg(not(windows))]
use crate::privileges::diagnostics::RulesState;
use crate::privileges::{self, Readiness};
use crate::theme::style::{self, ButtonKind, Surface};
use crate::theme::to_iced;
use crate::theme::tokens::{shape, space};
use iced::widget::{Space, column, container, row, scrollable};
use iced::{Alignment, Element, Length, Padding};
use tuxrgb_core::Rgb;
use tuxrgb_openrgb::BridgeStatus;

/// The settings page scroller (the screenshot tour scrolls it).
pub const SCROLL_ID: &str = "settings-scroll";

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

/// One slider of the LED colour correction, with its value on the right.
fn led_slider<'a>(
    app: &'a App,
    range: std::ops::RangeInclusive<f32>,
    value: f32,
    stop: f32,
    format: fn(f32) -> String,
    tint: Option<iced::Color>,
    on_change: impl Fn(f32) -> Message + 'a,
) -> Element<'a, Message> {
    let r = &app.roles;
    let mut colors = super::motion::SliderColors::standard(r);
    if let Some(t) = tint {
        colors.active = t;
        colors.handle = t;
    }
    row![
        container(
            super::motion::slider(range, value, on_change)
                .step(0.01)
                .stops(&[stop])
                .format(format)
                .colors(colors)
                .height(40.0),
        )
        .width(Length::Fixed(260.0)),
        iced::widget::text(format(value))
            .size(14.0)
            .color(r.on_surface_variant)
            .width(Length::Fixed(52.0))
            .align_x(iced::alignment::Horizontal::Right),
    ]
    .spacing(space::S)
    .align_y(Alignment::Center)
    .into()
}

/// LED colour correction: the LEDs show the colours of the interface.
fn led_colors(app: &App) -> Element<'_, Message> {
    use tuxrgb_core::led_color::{DEFAULT_GAMMA, DEFAULT_WHITE, GAMMA_RANGE};
    let r = &app.roles;
    let c = &app.config.led_color;
    let mut rows = vec![setting_row(
        icons::COLORIZE,
        "Couleurs fidèles à l'écran",
        "Corrige la courbe et la balance des blancs des LED pour qu'elles montrent les mêmes couleurs que l'application (sans correction, un rose part vers le violet et les teintes pastel paraissent délavées).",
        switch(c.enabled, r, Message::LedColorEnabled),
        r,
    )];
    if c.enabled {
        let percent: fn(f32) -> String = |v| format!("{:.0} %", v * 100.0);
        rows.push(setting_row(
            icons::CONTRAST,
            "Gamma",
            "Plus haut : couleurs plus franches, demi-teintes plus sombres. 2,2 correspond à un écran.",
            led_slider(
                app,
                GAMMA_RANGE,
                c.gamma,
                DEFAULT_GAMMA,
                |v| {
                    let s = format!("{v:.2}");
                    if tuxrgb_core::i18n::language() == tuxrgb_core::i18n::Lang::Fr {
                        s.replace('.', ",")
                    } else {
                        s
                    }
                },
                None,
                Message::LedGamma,
            ),
            r,
        ));
        let channels = [
            ("Rouge", iced::Color::from_rgb8(0xE5, 0x39, 0x35)),
            ("Vert", iced::Color::from_rgb8(0x43, 0xA0, 0x47)),
            ("Bleu", iced::Color::from_rgb8(0x1E, 0x88, 0xE5)),
        ];
        for (ch, (name, tint)) in channels.into_iter().enumerate() {
            rows.push(setting_row(
                icons::OPACITY,
                name,
                trf!(
                    "Part du {} dans le blanc des LED.",
                    crate::i18n::t(name).to_lowercase()
                ),
                led_slider(
                    app,
                    0.0..=1.0,
                    c.white[ch],
                    DEFAULT_WHITE[ch],
                    percent,
                    Some(tint),
                    move |v| Message::LedWhite(ch, v),
                ),
                r,
            ));
        }
    }
    rows.push(setting_row(
        icons::LIGHTBULB,
        "Blanc de test",
        "Allume toutes les LED en blanc : réglez le vert et le bleu jusqu'à obtenir un blanc neutre, sans reflet bleuté ni verdâtre.",
        row![
            text_button(
                "Réinitialiser",
                ButtonKind::Text,
                r,
                Some(Message::LedColorReset)
            ),
            switch(app.led_test_white, r, Message::LedTestWhite),
        ]
        .spacing(space::M)
        .align_y(Alignment::Center),
        r,
    ));
    group(app, icons::PALETTE, "Couleurs des LED", rows)
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
    } else if cfg!(windows) {
        "Zone de notification indisponible pour le moment (barre des tâches de Windows pas encore prête)."
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

/// Windows build: OpenRGB, PawnIO, administrator rights, system.
#[cfg(windows)]
pub fn diagnostics_list(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    let Some(d) = &app.diagnostics else {
        return muted("Vérification en cours…", r).into();
    };
    column![
        check_row(
            app,
            Some(true),
            "Périphériques USB / HID",
            crate::i18n::t("accessibles sans droits particuliers").into()
        ),
        check_row(
            app,
            Some(d.pawnio),
            "Pilote PawnIO (SMBus)",
            crate::i18n::t(if d.pawnio {
                "installé"
            } else {
                "absent : carte mère et RAM indisponibles"
            })
            .into()
        ),
        check_row(
            app,
            Some(d.elevated),
            "Droits administrateur",
            crate::i18n::t(if d.elevated {
                "actifs (mode administrateur)"
            } else if d.admin_mode {
                "mode administrateur configuré, non actif pour ce lancement"
            } else {
                "non : « Configurer l'accès » active le mode administrateur"
            })
            .into()
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
        check_row(app, Some(true), "Système", d.os.clone()),
    ]
    .spacing(space::XXS)
    .into()
}

#[cfg(not(windows))]
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
            small(if cfg!(windows) {
                "La réinitialisation retire le mode administrateur configuré par TuxRGB, efface l'état \
                 du premier lancement et relance l'assistant – utile après un changement de composant."
            } else {
                "La réinitialisation retire les règles udev et la configuration I2C installées par TuxRGB, \
                 efface l'état du premier lancement et relance l'assistant – utile après un changement de composant."
            })
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

/// WLED controllers (DDP) and Philips WiZ lights, driven by the embedded
/// OpenRGB like any device.
fn network(app: &App) -> Element<'_, Message> {
    use iced::widget::{text, text_input};
    let r = &app.roles;
    let net = &app.net;
    let external = app.config.server.kind == ServerKind::External;
    let mut rows: Vec<Element<'_, Message>> = vec![
        muted(
            "Les contrôleurs WLED (protocole DDP) et les ampoules Philips WiZ deviennent des appareils comme les autres : canvas, effets, couleur par zone et extinction à l'arrêt du PC.",
            r,
        )
        .size(13.0)
        .into(),
    ];
    if external {
        rows.push(
            badge(
                small("Serveur OpenRGB externe : ajoutez ces lumières dans OpenRGB lui-même (DDP, Philips Wiz)."),
                r,
                Surface::Tertiary,
            ),
        );
    }
    // What is configured.
    for (i, l) in app.config.network.wled.iter().enumerate() {
        rows.push(setting_row(
            icons::LINEAR_SCALE,
            l.name.as_str(),
            trf!("WLED · {} · {} LED", l.host, l.leds),
            widgets::round_icon_button(icons::CLOSE, r, false, Some(Message::NetRemoveWled(i))),
            r,
        ));
    }
    for (i, l) in app.config.network.wiz.iter().enumerate() {
        rows.push(setting_row(
            icons::LIGHTBULB,
            "Philips WiZ",
            l.host.clone(),
            widgets::round_icon_button(icons::CLOSE, r, false, Some(Message::NetRemoveWiz(i))),
            r,
        ));
    }
    // Add a WLED controller: its address, then "Detect" fills the LED count.
    let field = |placeholder: &'static str, value: &str, width: f32, on: fn(String) -> Message| {
        super::transition::text_field(
            text_input(crate::i18n::t(placeholder), value)
                .on_input(on)
                .padding(10)
                .size(14.0)
                .width(Length::Fixed(width)),
            r,
        )
    };
    let can_add = !external;
    let wled_row = row![
        field(
            "Adresse IP de WLED",
            &net.wled_host,
            190.0,
            Message::NetWledHost
        ),
        widgets::text_button(
            if net.wled_probing {
                "Détection…"
            } else {
                "Détecter"
            },
            ButtonKind::Tonal,
            r,
            (!net.wled_probing && !net.wled_host.trim().is_empty())
                .then_some(Message::NetWledProbe),
        ),
        field("LED", &net.wled_leds, 76.0, Message::NetWledLeds),
        widgets::icon_label_button(
            icons::ADD,
            "Ajouter",
            ButtonKind::Filled,
            r,
            can_add.then_some(Message::NetAddWled),
        ),
    ]
    .spacing(space::S)
    .align_y(Alignment::Center);
    rows.push(setting_row(
        icons::LINEAR_SCALE,
        "Ajouter un contrôleur WLED",
        "Même réseau local. « Détecter » lit son nom et son nombre de LED.",
        wled_row,
        r,
    ));
    if let Some(status) = &net.wled_status {
        let (txt, color) = match status {
            Ok(t) => (t.clone(), r.primary),
            Err(t) => (t.clone(), r.error),
        };
        rows.push(
            container(text(txt).size(13.0).color(color))
                .padding(Padding {
                    left: 56.0,
                    ..Padding::ZERO
                })
                .into(),
        );
    }
    // Add a WiZ bulb: search the network, or type its address.
    let wiz_row = row![
        widgets::text_button(
            if net.wiz_searching {
                "Recherche…"
            } else {
                "Rechercher sur le réseau"
            },
            ButtonKind::Tonal,
            r,
            (!net.wiz_searching).then_some(Message::NetWizSearch),
        ),
        field(
            "Adresse IP de l'ampoule",
            &net.wiz_host,
            190.0,
            Message::NetWizHost
        ),
        widgets::icon_label_button(
            icons::ADD,
            "Ajouter",
            ButtonKind::Filled,
            r,
            (can_add && !net.wiz_host.trim().is_empty())
                .then(|| Message::NetAddWiz(net.wiz_host.clone())),
        ),
    ]
    .spacing(space::S)
    .align_y(Alignment::Center);
    rows.push(setting_row(
        icons::LIGHTBULB,
        "Ajouter une ampoule Philips WiZ",
        "Ses propres scènes (Océan, Feu de cheminée…) deviennent ses effets matériels dans Appareils.",
        wiz_row,
        r,
    ));
    for f in &net.wiz_found {
        rows.push(
            row![
                Space::new().width(56.0),
                small(if f.mac.is_empty() {
                    trf!("Ampoule WiZ trouvée : {}", f.host)
                } else {
                    trf!("Ampoule WiZ trouvée : {} ({})", f.host, f.mac)
                })
                .width(Length::Fill),
                widgets::text_button(
                    "Ajouter",
                    ButtonKind::Tonal,
                    r,
                    can_add.then(|| Message::NetAddWiz(f.host.clone())),
                ),
            ]
            .spacing(space::S)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    if net.wiz_searched && net.wiz_found.is_empty() && !net.wiz_searching {
        rows.push(
            container(
                muted("Aucune nouvelle ampoule WiZ n'a répondu sur ce réseau.", r).size(13.0),
            )
            .padding(Padding {
                left: 56.0,
                ..Padding::ZERO
            })
            .into(),
        );
    }
    group(app, icons::HUB, "Lumières réseau (WLED, Philips WiZ)", rows)
}

/// New versions from the GitHub releases (see `update.rs`).
fn updates(app: &App) -> Element<'_, Message> {
    use crate::update::{Method, Ready, State};
    let r = &app.roles;
    let method = crate::update::method();
    let how = match method {
        Method::AppImage => {
            "Mise à jour intégrée : l'AppImage est remplacée, vos réglages sont conservés."
        }
        Method::WindowsInstaller => {
            "Mise à jour intégrée : l'installateur s'exécute puis relance TuxRGB, vos réglages sont conservés."
        }
        Method::Manual if crate::paths::package_kind() == crate::paths::PackageKind::Flatpak => {
            "Flatpak : la mise à jour passe par votre logithèque (Discover, GNOME Logiciels…)."
        }
        Method::Manual => "Cette installation se met à jour avec son gestionnaire de paquets.",
    };
    let check_button = icon_label_button(
        icons::SYNC,
        "Rechercher",
        ButtonKind::Tonal,
        r,
        (!app.update.busy()).then_some(Message::CheckUpdates),
    );
    let mut rows = vec![setting_row(
        icons::UPDATE,
        "Rechercher des mises à jour",
        trf!(
            "Version installée : {}. {}",
            crate::version_label(),
            crate::i18n::t(how)
        ),
        check_button,
        r,
    )];
    let status_row = |glyph: char,
                      color: iced::Color,
                      heading: String,
                      text: String,
                      actions: Element<'static, Message>|
     -> Element<'static, Message> {
        row![
            container(icon_filled(glyph, 24.0).color(color))
                .center_x(Length::Fixed(40.0))
                .center_y(Length::Fixed(40.0)),
            column![
                title_medium(heading),
                muted(text, r).size(crate::theme::tokens::typescale::BODY_SMALL)
            ]
            .spacing(space::XXS)
            .width(Length::Fill),
            actions,
        ]
        .spacing(space::L)
        .align_y(Alignment::Center)
        .padding(Padding::from([space::M, 0.0]))
        .into()
    };
    let none = || -> Element<'static, Message> { Space::new().into() };
    let t = crate::i18n::t;
    let status: Option<Element<'_, Message>> = match &app.update {
        State::Idle => None,
        State::Checking => Some(status_row(
            icons::SYNC,
            r.primary,
            t("Recherche en cours…").into(),
            t("Interrogation des versions publiées sur GitHub.").into(),
            none(),
        )),
        State::UpToDate(latest) => Some(status_row(
            icons::CHECK_CIRCLE,
            r.success,
            t("TuxRGB est à jour").into(),
            trf!("Dernière version publiée : {}.", latest),
            none(),
        )),
        State::Available(release) => Some(status_row(
            icons::NEW_RELEASES,
            r.primary,
            trf!("TuxRGB {} est disponible", release.version),
            match method {
                Method::Manual => t("Téléchargez-la depuis la page de la version sur GitHub.").into(),
                _ if release.asset.is_none() => {
                    t("Cette version ne contient pas de fichier pour ce système.").into()
                }
                _ => t("Téléchargement vérifié (SHA-256) puis installation automatique.").into(),
            },
            row![
                text_button(
                    "Notes de version",
                    ButtonKind::Text,
                    r,
                    Some(Message::OpenUrl(release.page.clone()))
                ),
                icon_label_button(
                    if method == Method::Manual || release.asset.is_none() {
                        icons::OPEN_IN_NEW
                    } else {
                        icons::SYSTEM_UPDATE_ALT
                    },
                    if method == Method::Manual || release.asset.is_none() {
                        "Ouvrir GitHub"
                    } else {
                        "Mettre à jour"
                    },
                    ButtonKind::Filled,
                    r,
                    Some(Message::InstallUpdate)
                ),
            ]
            .spacing(space::S)
            .align_y(Alignment::Center)
            .into(),
        )),
        State::Downloading {
            release,
            done,
            total,
        } => {
            let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
            let progress = if *total > 0 {
                trf!(
                    "{} % · {} / {} Mo",
                    (*done as f64 * 100.0 / *total as f64).floor() as u32,
                    format!("{:.1}", mb(*done)),
                    format!("{:.1}", mb(*total))
                )
            } else {
                trf!("{} Mo", format!("{:.1}", mb(*done)))
            };
            Some(status_row(
                icons::DOWNLOAD,
                r.primary,
                trf!("Téléchargement de TuxRGB {}…", release.version),
                progress,
                none(),
            ))
        }
        State::Ready { release, ready } => Some(match ready {
            Ready::AppImage(_) => status_row(
                icons::TASK_ALT,
                r.success,
                trf!("TuxRGB {} est installé", release.version),
                t("Redémarrez TuxRGB pour utiliser la nouvelle version.").into(),
                icon_label_button(
                    icons::RESTART_ALT,
                    "Redémarrer TuxRGB",
                    ButtonKind::Filled,
                    r,
                    Some(Message::ApplyUpdate),
                )
                .into(),
            ),
            Ready::Installer(_) => status_row(
                icons::SYSTEM_UPDATE_ALT,
                r.primary,
                trf!("Installation de TuxRGB {}…", release.version),
                t("Confirmez l'installation dans la fenêtre de Windows : TuxRGB se fermera puis redémarrera tout seul.").into(),
                none(),
            ),
        }),
        State::Failed { release, error } => Some(status_row(
            icons::ERROR,
            r.error,
            t("La mise à jour n'a pas abouti").into(),
            error.clone(),
            row![
                text_button(
                    "Ouvrir GitHub",
                    ButtonKind::Text,
                    r,
                    Some(Message::OpenUrl(
                        release
                            .as_ref()
                            .map(|r| r.page.clone())
                            .unwrap_or_else(|| format!("{}/releases", crate::update::REPO_URL))
                    ))
                ),
                icon_label_button(
                    icons::REFRESH,
                    "Réessayer",
                    ButtonKind::Tonal,
                    r,
                    Some(if release.is_some() {
                        Message::InstallUpdate
                    } else {
                        Message::CheckUpdates
                    })
                ),
            ]
            .spacing(space::S)
            .align_y(Alignment::Center)
            .into(),
        )),
    };
    if let Some(s) = status {
        rows.push(s);
    }
    rows.push(setting_row(
        icons::SCHEDULE,
        "Vérifier automatiquement",
        "Au démarrage de TuxRGB, au plus toutes les 6 heures. Rien n'est installé sans votre accord.",
        switch(app.config.updates.auto_check, r, Message::SetAutoUpdateCheck),
        r,
    ));
    group(app, icons::SYSTEM_UPDATE_ALT, "Mises à jour", rows)
}

fn about(app: &App) -> Element<'_, Message> {
    let r = &app.roles;
    #[cfg(windows)]
    let tagline = trf!(
        "Éclairage RGB pour Windows 10 / 11 · moteur {} embarqué",
        tuxrgb_openrgb::OPENRGB_LABEL
    );
    #[cfg(not(windows))]
    let tagline = trf!(
        "Éclairage RGB pour Linux · Wayland natif · moteur {} embarqué",
        tuxrgb_openrgb::OPENRGB_LABEL
    );
    let badges = row![
        badge(small("GPL-2.0-or-later"), r, Surface::Highest),
        badge(small("100 % Rust (interface)"), r, Surface::Highest),
        badge(small("io.github.tuxrgb.TuxRGB"), r, Surface::Highest),
    ]
    .spacing(space::XS);
    #[cfg(windows)]
    let badges = badges.push(badge(small(crate::win::EDITION), r, Surface::Tertiary));
    container(
        row![
            super::logo(72.0),
            column![
                title(format!("TuxRGB {}", crate::version_label())),
                body(tagline).color(r.on_surface_variant),
                badges,
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
        led_colors(app),
        hardware(app),
        engine(app),
        network(app),
        startup(app),
        performance(app),
        updates(app),
        about(app),
        Space::new().height(space::XL),
    ]
    .spacing(space::L);
    column![
        page_header(
            app,
            "Réglages",
            "Apparence, accès matériel, démarrage, performances et mises à jour.",
            Space::new().into()
        ),
        super::transition::smooth_scroll(
            scrollable(container(content).padding(Padding {
                right: space::L,
                ..Padding::ZERO
            }))
            .id(SCROLL_ID)
            .height(Length::Fill),
            r,
        ),
    ]
    .spacing(space::XL)
    .into()
}
