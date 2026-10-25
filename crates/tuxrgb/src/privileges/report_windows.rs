//! Plain-text hardware report on Windows: system, PawnIO,
//! administrator rights, OpenRGB detection log and the detected devices.

use super::diagnostics::Diagnostics;
use std::fmt::Write as _;
use tuxrgb_core::{DeviceInfo, DeviceKind};

/// What the report found, used for the hints shown in the interface.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Findings {
    pub dram_detected: usize,
    /// PawnIO installed (SMBus access).
    pub dram_bus_supported: bool,
    /// SMBus usable: PawnIO and administrator rights.
    pub dram_bus_accessible: bool,
    pub spd_drivers: Vec<String>,
    /// Kept for the shared interface code (Linux kernel parameter).
    pub acpi_lax: bool,
    pub smbus_vendor: Option<u16>,
    /// Never on Windows (Linux firmware/ACPI refusal).
    pub smbus_unbound: bool,
    pub smbus_module_loaded: bool,
}

impl Findings {
    /// Human hints about missing RAM, most likely cause first.
    pub fn ram_hints(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.dram_detected > 0 {
            return out;
        }
        if !self.dram_bus_supported {
            out.push(
                "Le pilote PawnIO n'est pas installé : sans lui, OpenRGB ne peut pas lire le bus \
                 SMBus de la RAM et de la carte mère. Réinstallez TuxRGB en cochant « Pilote \
                 PawnIO », ou installez-le depuis pawnio.eu."
                    .into(),
            );
        } else if !self.dram_bus_accessible {
            out.push(
                "PawnIO est installé mais TuxRGB tourne sans droits administrateur : utilisez \
                 « Configurer l'accès » (une seule confirmation Windows)."
                    .into(),
            );
        }
        if out.is_empty() {
            out.push(
                "Le bus de la RAM est accessible mais aucune barrette RGB n'a répondu : votre \
                 modèle n'est peut-être pas encore pris en charge par OpenRGB. Copiez le rapport \
                 et indiquez la marque et le modèle exacts de la RAM."
                    .into(),
            );
        }
        out
    }
}

/// Builds the report (blocking file reads: call from a worker).
pub fn build(diag: Option<&Diagnostics>, devices: &[DeviceInfo]) -> (String, Findings) {
    let mut s = String::new();
    let mut f = Findings::default();
    let _ = writeln!(
        s,
        "=== Rapport matériel TuxRGB {} ({}) ===",
        crate::version_label(),
        crate::win::EDITION
    );
    let _ = writeln!(s, "Système : {}", crate::win::os_name());
    let _ = writeln!(s, "Architecture : {}", std::env::consts::ARCH);
    let _ = writeln!(s, "Moteur : {}", tuxrgb_openrgb::OPENRGB_VERSION);
    if let Some(d) = diag {
        f.dram_bus_supported = d.pawnio;
        f.dram_bus_accessible = d.pawnio && d.elevated;
        let _ = writeln!(s, "\n--- Accès matériel ---");
        let _ = writeln!(
            s,
            "Pilote PawnIO : {}",
            if d.pawnio { "installé" } else { "absent" }
        );
        let _ = writeln!(
            s,
            "Droits administrateur : {}",
            if d.elevated { "oui" } else { "non" }
        );
        let _ = writeln!(
            s,
            "Mode administrateur (tâche planifiée) : {}",
            if d.admin_mode {
                "configuré"
            } else {
                "non configuré"
            }
        );
        let _ = writeln!(
            s,
            "OpenRGB : {}",
            d.openrgb_binary
                .as_ref()
                .map(|p| format!("{} ({})", p.display(), d.openrgb_origin))
                .unwrap_or_else(|| "introuvable".into())
        );
        let _ = writeln!(s, "Format : {}", d.package.label());
    }

    let _ = writeln!(s, "\n--- Périphériques détectés ({}) ---", devices.len());
    for d in devices {
        if d.kind == DeviceKind::Dram {
            f.dram_detected += 1;
        }
        let _ = writeln!(
            s,
            "{:?} · {} · {} LED · {}",
            d.kind,
            d.display_name(),
            d.led_count,
            d.location
        );
    }

    // Detection log of the embedded server (SMBus buses, controllers).
    let log = std::fs::read_to_string(&crate::paths::paths().openrgb_log).unwrap_or_default();
    let lines: Vec<&str> = log
        .lines()
        .filter(|l| {
            let l = l.to_ascii_lowercase();
            l.contains("smbus")
                || l.contains("i2c")
                || l.contains("pawnio")
                || l.contains("detect")
                || l.contains("error")
        })
        .collect();
    let _ = writeln!(
        s,
        "\n--- Journal OpenRGB ({} lignes utiles) ---",
        lines.len()
    );
    for l in lines.iter().rev().take(200).rev() {
        let _ = writeln!(s, "{l}");
    }
    (s, f)
}
