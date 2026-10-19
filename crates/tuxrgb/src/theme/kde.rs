//! Desktop appearance: dark/light preference, accent colour, reduced motion.
//!
//! Primary source, on every desktop: `org.freedesktop.portal.Settings`
//! (namespace `org.freedesktop.appearance`, keys `color-scheme`,
//! `accent-color`, `reduced-motion`), implemented by KDE Plasma 6, GNOME 47+
//! and COSMIC. Fallbacks, by desktop (see `crate::desktop`):
//!
//! * KDE – `~/.config/kdeglobals` (`[General] AccentColor`), which also
//!   works on Plasma versions whose portal misreports the scheme;
//! * GNOME / Budgie / Pantheon – `gsettings org.gnome.desktop.interface
//!   accent-color` (named colours);
//! * COSMIC – `~/.config/cosmic/com.system76.CosmicTheme.*/v1/accent`.
//!
//! Changes are pushed (portal `SettingChanged`, KDE `ConfigChanged`); only
//! the COSMIC file fallback is checked every few seconds.

use ashpd::desktop::settings::{ColorScheme, ReducedMotion, Settings};
use futures::StreamExt;
use ini::Ini;
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tuxrgb_core::Rgb;

#[derive(Clone, Debug, PartialEq)]
pub struct SystemAppearance {
    /// `None` = the desktop has no preference.
    pub dark: Option<bool>,
    pub accent: Option<Rgb>,
    pub reduced_motion: bool,
    pub source: &'static str,
}

impl Default for SystemAppearance {
    fn default() -> Self {
        Self {
            dark: None,
            accent: None,
            reduced_motion: false,
            source: "défaut",
        }
    }
}

fn is_kde() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_ascii_uppercase().contains("KDE"))
        .unwrap_or(false)
}

fn kdeglobals_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.join("kdeglobals").exists())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_default()
        .join("kdeglobals")
}

fn parse_rgb_triplet(s: &str) -> Option<Rgb> {
    let mut it = s.split(',').map(|p| p.trim().parse::<u8>());
    match (it.next(), it.next(), it.next()) {
        (Some(Ok(r)), Some(Ok(g)), Some(Ok(b))) => Some(Rgb::new(r, g, b)),
        _ => None,
    }
}

/// Reads `kdeglobals`: `(dark, accent)`.
pub fn read_kdeglobals() -> (Option<bool>, Option<Rgb>) {
    let Ok(ini) = Ini::load_from_file(kdeglobals_path()) else {
        return (None, None);
    };
    let get = |section: &str, key: &str| ini.section(Some(section)).and_then(|s| s.get(key));
    let accent = get("General", "AccentColor")
        .and_then(parse_rgb_triplet)
        .or_else(|| get("Colors:Selection", "BackgroundNormal").and_then(parse_rgb_triplet));
    let dark = get("Colors:Window", "BackgroundNormal")
        .and_then(parse_rgb_triplet)
        .map(|bg| bg.luminance() < 0.25)
        .or_else(|| get("General", "ColorScheme").map(|s| s.to_ascii_lowercase().contains("dark")));
    (dark, accent)
}

async fn gnome_gsettings_accent() -> Option<Rgb> {
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "accent-color"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    crate::desktop::gnome_accent(&String::from_utf8_lossy(&out.stdout))
}

fn cosmic_config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_default()
        .join("cosmic")
}

/// COSMIC: `(dark, accent)` from its theme files.
fn read_cosmic() -> (Option<bool>, Option<Rgb>) {
    let dir = cosmic_config_dir();
    let dark = std::fs::read_to_string(dir.join("com.system76.CosmicTheme.Mode/v1/is_dark"))
        .ok()
        .map(|s| s.trim() == "true");
    let theme = if dark.unwrap_or(true) {
        "Dark"
    } else {
        "Light"
    };
    let accent =
        std::fs::read_to_string(dir.join(format!("com.system76.CosmicTheme.{theme}/v1/accent")))
            .ok()
            .and_then(|s| crate::desktop::parse_cosmic_color(&s));
    (dark, accent)
}

/// Reads the current appearance (portal first, then the desktop's own
/// configuration).
/// Longest wait for one portal call: a portal that hangs (backend still
/// starting at login, broken backend) must not hold the theme back.
const PORTAL_TIMEOUT: Duration = Duration::from_secs(3);

async fn portal<T, E>(call: impl Future<Output = Result<T, E>>) -> Option<T> {
    tokio::time::timeout(PORTAL_TIMEOUT, call).await.ok()?.ok()
}

pub async fn read() -> SystemAppearance {
    let mut out = SystemAppearance::default();
    if let Some(settings) = portal(Settings::new()).await {
        out.source = "portail freedesktop";
        if let Some(scheme) = portal(settings.color_scheme()).await {
            out.dark = match scheme {
                ColorScheme::PreferDark => Some(true),
                ColorScheme::PreferLight => Some(false),
                ColorScheme::NoPreference => None,
            };
        }
        if let Some(c) = portal(settings.accent_color()).await {
            let (r, g, b) = (c.red(), c.green(), c.blue());
            // Out-of-range components mean "no accent configured".
            if [r, g, b].iter().all(|v| (0.0..=1.0).contains(v)) {
                out.accent = Some(Rgb::from_f32(r as f32, g as f32, b as f32));
            }
        }
        if let Some(m) = portal(settings.reduced_motion()).await {
            out.reduced_motion = m == ReducedMotion::ReducedMotion;
        }
    }
    let (kde_dark, kde_accent) = read_kdeglobals();
    if is_kde() && kde_dark.is_some() {
        // Plasma's kdeglobals is authoritative (works around portal glitches).
        out.dark = kde_dark;
        if out.source == "défaut" {
            out.source = "kdeglobals";
        }
    } else if out.dark.is_none() {
        out.dark = kde_dark;
    }
    if out.accent.is_none() {
        use crate::desktop::Desktop;
        match crate::desktop::session().desktop {
            Desktop::Gnome | Desktop::Budgie | Desktop::Pantheon => {
                if let Some(c) = gnome_gsettings_accent().await {
                    out.accent = Some(c);
                    out.source = "gsettings";
                }
            }
            Desktop::Cosmic => {
                let (dark, accent) = read_cosmic();
                if accent.is_some() {
                    out.accent = accent;
                    out.source = "cosmic";
                }
                if out.dark.is_none() {
                    out.dark = dark;
                }
            }
            _ => {}
        }
    }
    if out.accent.is_none() {
        out.accent = kde_accent;
        if kde_accent.is_some() && out.source == "défaut" {
            out.source = "kdeglobals";
        }
    }
    out
}

/// Pushes a fresh [`SystemAppearance`] every time the desktop changes.
///
/// A theme change arrives as a burst of signals (scheme, accent, a dozen
/// kdeglobals keys): they are gathered for a moment and the appearance is
/// read once, and only sent when it actually changed.
pub async fn watch(tx: UnboundedSender<SystemAppearance>) {
    let (dirty_tx, mut dirty_rx) = tokio::sync::mpsc::unbounded_channel::<()>();

    let reader = async {
        let mut last = read().await;
        let _ = tx.send(last.clone());
        while dirty_rx.recv().await.is_some() {
            // Let the desktop finish writing (Plasma rewrites kdeglobals),
            // and fold the rest of the burst into this read.
            tokio::time::sleep(Duration::from_millis(150)).await;
            while dirty_rx.try_recv().is_ok() {}
            let now = read().await;
            if now != last {
                last = now.clone();
                let _ = tx.send(now);
            }
        }
    };

    let portal_signals = {
        let dirty = dirty_tx.clone();
        async move {
            let Some(settings) = portal(Settings::new()).await else {
                return futures::future::pending::<()>().await;
            };
            let Some(mut stream) = portal(settings.receive_setting_changed()).await else {
                return futures::future::pending::<()>().await;
            };
            while let Some(setting) = stream.next().await {
                let ns = setting.namespace();
                if ns == "org.freedesktop.appearance"
                    || ns.starts_with("org.kde.kdeglobals")
                    || ns == "org.gnome.desktop.interface"
                    || ns.starts_with("com.system76.CosmicTheme")
                {
                    let _ = dirty.send(());
                }
            }
            futures::future::pending::<()>().await
        }
    };

    let kconfig = {
        let dirty = dirty_tx.clone();
        async move {
            let Ok(conn) = zbus::Connection::session().await else {
                return futures::future::pending::<()>().await;
            };
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface("org.kde.kconfig.notify")
                .and_then(|b| b.member("ConfigChanged"))
                .and_then(|b| b.path("/kdeglobals"))
                .map(|b| b.build());
            let Ok(rule) = rule else {
                return futures::future::pending::<()>().await;
            };
            let Ok(mut stream) = zbus::MessageStream::for_match_rule(rule, &conn, Some(8)).await
            else {
                return futures::future::pending::<()>().await;
            };
            while stream.next().await.is_some() {
                let _ = dirty.send(());
            }
            futures::future::pending::<()>().await
        }
    };

    // COSMIC without accent in its portal: the theme file is checked every
    // few seconds (a stat + small read, nothing else).
    let cosmic = {
        let dirty = dirty_tx.clone();
        async move {
            if crate::desktop::session().desktop != crate::desktop::Desktop::Cosmic {
                return futures::future::pending::<()>().await;
            }
            let mut last = read_cosmic();
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let now = read_cosmic();
                if now != last {
                    last = now;
                    let _ = dirty.send(());
                }
            }
        }
    };
    drop(dirty_tx);

    futures::join!(reader, portal_signals, kconfig, cosmic);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kde_triplets() {
        assert_eq!(
            parse_rgb_triplet("61,174,233"),
            Some(Rgb::new(61, 174, 233))
        );
        assert_eq!(parse_rgb_triplet(" 1, 2 ,3"), Some(Rgb::new(1, 2, 3)));
        assert_eq!(parse_rgb_triplet("1,2"), None);
        assert_eq!(parse_rgb_triplet("300,2,3"), None);
    }
}
