//! Desktop environment and distribution detection.
//!
//! Used to name the colour synchronisation after the desktop ("Sync KDE",
//! "Sync GNOME", "Sync COSMIC", "Sync système"), to pick the accent colour
//! source and to warn clearly when something is not supported (X11 session,
//! a desktop without accent colour, no Polkit agent, an unknown system).

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desktop {
    Kde,
    Gnome,
    Cosmic,
    Xfce,
    Cinnamon,
    Mate,
    Budgie,
    Pantheon,
    Lxqt,
    Hyprland,
    Sway,
    Niri,
    Other,
}

impl Desktop {
    /// Short name for labels ("Sync KDE").
    pub fn short_name(self) -> &'static str {
        match self {
            Desktop::Kde => "KDE",
            Desktop::Gnome => "GNOME",
            Desktop::Cosmic => "COSMIC",
            Desktop::Xfce => "Xfce",
            Desktop::Cinnamon => "Cinnamon",
            Desktop::Mate => "MATE",
            Desktop::Budgie => "Budgie",
            Desktop::Pantheon => "Pantheon",
            Desktop::Lxqt => "LXQt",
            Desktop::Hyprland => "Hyprland",
            Desktop::Sway => "Sway",
            Desktop::Niri => "niri",
            Desktop::Other => "",
        }
    }

    /// The desktop publishes an accent colour TuxRGB can follow live.
    pub fn has_accent(self) -> bool {
        matches!(
            self,
            Desktop::Kde | Desktop::Gnome | Desktop::Cosmic | Desktop::Pantheon | Desktop::Budgie
        )
    }
}

/// Everything TuxRGB knows about the session.
#[derive(Clone, Debug)]
pub struct Session {
    pub desktop: Desktop,
    /// Raw `XDG_CURRENT_DESKTOP` (for the hardware report).
    pub desktop_raw: String,
    pub wayland: bool,
    /// `PRETTY_NAME` of /etc/os-release.
    pub distro: String,
    pub distro_id: String,
    /// `ID_LIKE` (e.g. "fedora", "debian ubuntu", "arch").
    pub distro_like: String,
    /// Image-based system (rpm-ostree, bootc, NixOS, transactional).
    pub immutable: bool,
}

impl Session {
    /// The distribution family is one TuxRGB is tested on.
    pub fn distro_supported(&self) -> bool {
        const KNOWN: &[&str] = &[
            "fedora",
            "rhel",
            "centos",
            "nobara",
            "bazzite",
            "aurora",
            "bluefin",
            "ublue",
            "debian",
            "ubuntu",
            "linuxmint",
            "pop",
            "elementary",
            "zorin",
            "neon",
            "arch",
            "manjaro",
            "endeavouros",
            "cachyos",
            "garuda",
            "opensuse",
            "suse",
            "nixos",
            "gentoo",
            "void",
            "solus",
            "mageia",
            "kali",
            "steamos",
            "chimeraos",
        ];
        let id = format!("{} {}", self.distro_id, self.distro_like).to_ascii_lowercase();
        KNOWN
            .iter()
            .any(|k| id.split_whitespace().any(|w| w.contains(k)))
    }
}

fn os_release_field(text: &str, key: &str) -> String {
    text.lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
        .map(|v| v.trim().trim_matches('"').to_owned())
        .unwrap_or_default()
}

/// Parses `XDG_CURRENT_DESKTOP` (colon-separated, e.g. "ubuntu:GNOME").
pub fn parse_desktop(raw: &str) -> Desktop {
    let parts: Vec<String> = raw
        .split(':')
        .map(|p| p.trim().to_ascii_lowercase())
        .collect();
    let has = |n: &str| parts.iter().any(|p| p == n);
    if has("kde") || has("plasma") {
        Desktop::Kde
    } else if has("cosmic") {
        Desktop::Cosmic
    } else if has("budgie") {
        Desktop::Budgie
    } else if has("pantheon") {
        Desktop::Pantheon
    } else if has("x-cinnamon") || has("cinnamon") {
        Desktop::Cinnamon
    } else if has("gnome") || has("unity") || has("gnome-classic") {
        Desktop::Gnome
    } else if has("xfce") {
        Desktop::Xfce
    } else if has("mate") {
        Desktop::Mate
    } else if has("lxqt") {
        Desktop::Lxqt
    } else if has("hyprland") {
        Desktop::Hyprland
    } else if has("sway") {
        Desktop::Sway
    } else if has("niri") {
        Desktop::Niri
    } else {
        Desktop::Other
    }
}

/// The session, detected once.
pub fn session() -> &'static Session {
    static S: OnceLock<Session> = OnceLock::new();
    S.get_or_init(|| {
        let raw = std::env::var("XDG_CURRENT_DESKTOP")
            .or_else(|_| std::env::var("XDG_SESSION_DESKTOP"))
            .or_else(|_| std::env::var("DESKTOP_SESSION"))
            .unwrap_or_default();
        let os = std::fs::read_to_string("/etc/os-release")
            .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
            .unwrap_or_default();
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland");
        let id = os_release_field(&os, "ID");
        let immutable = std::path::Path::new("/run/ostree-booted").exists()
            || id == "nixos"
            || std::path::Path::new("/usr/sbin/transactional-update").exists();
        Session {
            desktop: parse_desktop(&raw),
            desktop_raw: raw,
            wayland,
            distro: {
                let pretty = os_release_field(&os, "PRETTY_NAME");
                if pretty.is_empty() {
                    os_release_field(&os, "NAME")
                } else {
                    pretty
                }
            },
            distro_like: os_release_field(&os, "ID_LIKE"),
            distro_id: id,
            immutable,
        }
    })
}

/// "Synchroniser avec KDE" / "… GNOME" / "… le système": the label of the
/// accent synchronisation switch.
pub fn sync_label() -> String {
    let d = session().desktop;
    if d.has_accent() {
        trf!("Synchroniser avec l'accent {}", d.short_name())
    } else {
        crate::i18n::t("Synchroniser avec l'accent du système").to_owned()
    }
}

/// Where the live accent comes from, for captions.
pub fn accent_caption() -> String {
    match session().desktop {
        Desktop::Kde => crate::i18n::t("Couleur d'accentuation de Plasma, en direct").to_owned(),
        Desktop::Gnome => {
            crate::i18n::t("Couleur d'accentuation de GNOME (Paramètres → Apparence), en direct")
                .to_owned()
        }
        Desktop::Cosmic => {
            crate::i18n::t("Couleur d'accentuation de COSMIC, en direct").to_owned()
        }
        d if d.has_accent() => {
            trf!("Couleur d'accentuation de {}, en direct", d.short_name())
        }
        _ => crate::i18n::t(
            "Ce bureau ne publie pas de couleur d'accentuation : TuxRGB utilise celle du portail, sinon le bleu Breeze",
        )
        .to_owned(),
    }
}

/// Compatibility warnings shown on the home page (most important first).
pub fn warnings() -> Vec<(&'static str, String)> {
    let s = session();
    let mut out = Vec::new();
    if !s.wayland {
        out.push((
            "Session X11 non prise en charge",
            crate::i18n::t(
                "TuxRGB est une application Wayland native : ouvrez une session Wayland pour l'interface. L'éclairage continue de fonctionner.",
            )
            .to_owned(),
        ));
    }
    if !s.desktop.has_accent() {
        let name = if s.desktop_raw.is_empty() {
            crate::i18n::t("inconnu").to_owned()
        } else {
            s.desktop_raw.clone()
        };
        out.push((
            "Bureau partiellement pris en charge",
            trf!(
                "Bureau « {} » : pas de couleur d'accentuation à synchroniser ni d'intégration du thème. Le reste fonctionne normalement.",
                name
            ),
        ));
    }
    if !s.distro_supported() && !s.distro.is_empty() {
        out.push((
            "Distribution non testée",
            trf!(
                "{} n'a pas été testée : l'accès matériel (udev, SMBus) pourrait demander une configuration manuelle.",
                s.distro
            ),
        ));
    }
    out
}

/// GNOME's named accent colours (`org.gnome.desktop.interface accent-color`,
/// GNOME 47+), as libadwaita draws them.
pub fn gnome_accent(name: &str) -> Option<tuxrgb_core::Rgb> {
    use tuxrgb_core::Rgb;
    Some(match name.trim().trim_matches('\'') {
        "blue" => Rgb::new(0x35, 0x84, 0xe4),
        "teal" => Rgb::new(0x21, 0x90, 0xa4),
        "green" => Rgb::new(0x3a, 0x94, 0x4a),
        "yellow" => Rgb::new(0xc8, 0x88, 0x00),
        "orange" => Rgb::new(0xed, 0x5b, 0x00),
        "red" => Rgb::new(0xe6, 0x2d, 0x42),
        "pink" => Rgb::new(0xd5, 0x61, 0x99),
        "purple" => Rgb::new(0x91, 0x41, 0xac),
        "slate" => Rgb::new(0x6f, 0x83, 0x96),
        _ => return None,
    })
}

/// COSMIC theme accent: `~/.config/cosmic/com.system76.CosmicTheme.{Dark,
/// Light}/v1/accent` holds a RON colour `(red: …, green: …, blue: …)`.
pub fn parse_cosmic_color(ron: &str) -> Option<tuxrgb_core::Rgb> {
    let field = |name: &str| -> Option<f32> {
        let i = ron.find(&format!("{name}:"))? + name.len() + 1;
        let rest = ron[i..].trim_start();
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e'))
            .unwrap_or(rest.len());
        rest[..end].parse().ok()
    };
    let (r, g, b) = (field("red")?, field("green")?, field("blue")?);
    [r, g, b]
        .iter()
        .all(|v| (0.0..=1.0).contains(v))
        .then(|| tuxrgb_core::Rgb::from_f32(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktops_are_recognised() {
        assert_eq!(parse_desktop("KDE"), Desktop::Kde);
        assert_eq!(parse_desktop("ubuntu:GNOME"), Desktop::Gnome);
        assert_eq!(parse_desktop("COSMIC"), Desktop::Cosmic);
        assert_eq!(parse_desktop("X-Cinnamon"), Desktop::Cinnamon);
        assert_eq!(parse_desktop("Budgie:GNOME"), Desktop::Budgie);
        assert_eq!(parse_desktop("Hyprland"), Desktop::Hyprland);
        assert_eq!(parse_desktop(""), Desktop::Other);
        assert!(Desktop::Gnome.has_accent());
        assert!(!Desktop::Xfce.has_accent());
    }

    #[test]
    fn gnome_and_cosmic_accents() {
        assert_eq!(
            gnome_accent("'blue'"),
            Some(tuxrgb_core::Rgb::new(0x35, 0x84, 0xe4))
        );
        assert_eq!(gnome_accent("magenta"), None);
        let ron = "Some((\n    red: 0.3882353,\n    green: 0.8156863,\n    blue: 0.8745098,\n    alpha: 1.0,\n))";
        assert_eq!(
            parse_cosmic_color(ron),
            Some(tuxrgb_core::Rgb::from_f32(0.3882353, 0.8156863, 0.8745098))
        );
        assert_eq!(parse_cosmic_color("None"), None);
    }

    #[test]
    fn os_release_fields() {
        let os = "NAME=\"Bazzite\"\nID=bazzite\nID_LIKE=\"fedora\"\nPRETTY_NAME=\"Bazzite 42\"\n";
        assert_eq!(os_release_field(os, "ID"), "bazzite");
        assert_eq!(os_release_field(os, "ID_LIKE"), "fedora");
        assert_eq!(os_release_field(os, "PRETTY_NAME"), "Bazzite 42");
    }
}
