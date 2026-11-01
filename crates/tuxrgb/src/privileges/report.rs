//! Plain-text hardware report (Réglages → Accès matériel → « Copier le
//! rapport »). It gathers, without any privilege, everything needed to
//! understand why a device – most often RAM – is not detected: SMBus
//! controllers and drivers, I2C buses and their permissions, kernel drivers
//! holding the memory SPD addresses, kernel parameters, and the relevant
//! lines of the embedded OpenRGB log.

use super::diagnostics::Diagnostics;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use tuxrgb_core::{DeviceInfo, DeviceKind};

/// SMBus controllers OpenRGB scans for RAM lighting (`IF_DRAM_SMBUS` in
/// OpenRGB's pci_ids.h): vendor, device, name.
const DRAM_SMBUS: &[(u16, u16, &str)] = &[
    (0x1022, 0x790b, "AMD FCH"),
    (0x8086, 0x3a30, "Intel ICH10"),
    (0x8086, 0xa123, "Intel Sunrise Point-H"),
    (0x8086, 0x2085, "Intel Skylake-X PCU"),
    (0x8086, 0xa2a3, "Intel Kaby Lake-H"),
    (0x8086, 0xa323, "Intel Cannon Lake-H"),
    (0x8086, 0x06a3, "Intel Comet Lake-H"),
    (0x8086, 0xa3a3, "Intel Comet Lake-V"),
    (0x8086, 0x43a3, "Intel Tiger Lake"),
    (0x8086, 0x7aa3, "Intel Alder Lake"),
    (0x8086, 0x7a23, "Intel Raptor Lake"),
    (0x8086, 0x7f23, "Intel Arrow Lake"),
];

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn hex(s: &str) -> u16 {
    u16::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0)
}

fn link_name(path: impl AsRef<Path>) -> Option<String> {
    fs::read_link(path)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
}

fn module_loaded(name: &str) -> bool {
    Path::new("/sys/module").join(name).exists()
}

fn rw_access(path: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return false;
    };
    unsafe { libc::access(c.as_ptr(), libc::R_OK | libc::W_OK) == 0 }
}

/// What the report found, used for the hints shown in the interface.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Findings {
    pub dram_detected: usize,
    /// An SMBus controller OpenRGB uses for RAM exists.
    pub dram_bus_supported: bool,
    /// `/dev/i2c-N` of that controller is readable and writable.
    pub dram_bus_accessible: bool,
    /// Kernel drivers bound to memory SPD EEPROMs (spd5118, ee1004…).
    pub spd_drivers: Vec<String>,
    pub acpi_lax: bool,
    pub smbus_vendor: Option<u16>,
    /// The RAM SMBus controller exists but no kernel driver is bound to it
    /// (firmware/ACPI refusal, typical on Gigabyte boards).
    pub smbus_unbound: bool,
    /// The chipset SMBus driver module is loaded.
    pub smbus_module_loaded: bool,
}

impl Findings {
    /// Human hints about missing RAM, most likely cause first.
    pub fn ram_hints(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.dram_detected > 0 {
            return out;
        }
        if self.smbus_unbound && !self.acpi_lax {
            out.push(
                "Le contrôleur SMBus de la RAM n'a aucun pilote : le firmware de la carte mère en \
                 réserve l'accès (fréquent sur les cartes Gigabyte AM5). Correction : bouton \
                 « Débloquer l'accès SMBus » ci-dessous (paramètre noyau \
                 acpi_enforce_resources=lax, et retrait du pilote spd5118 pour la DDR5), puis \
                 redémarrer."
                    .into(),
            );
            return out;
        }
        if self.smbus_unbound && self.acpi_lax {
            out.push(
                "acpi_enforce_resources=lax est actif mais le contrôleur SMBus n'a toujours pas de \
                 pilote : le journal du noyau (section « Noyau » du rapport) indique pourquoi."
                    .into(),
            );
        }
        if !self.dram_bus_supported {
            out.push(
                "Aucun contrôleur SMBus reconnu par OpenRGB pour la RAM n'a été trouvé \
                 (pilote i2c-piix4 sur AMD, i2c-i801 sur Intel)."
                    .into(),
            );
        } else if !self.dram_bus_accessible {
            out.push(
                "Le bus SMBus de la RAM existe mais n'est pas accessible : lancez « Configurer \
                 l'accès », puis redémarrez la session."
                    .into(),
            );
        }
        if !self.spd_drivers.is_empty() {
            out.push(trf!(
                "Le noyau a attaché le pilote {} aux barrettes : OpenRGB lit alors la mémoire par \
                 ce pilote. Si la RAM reste absente, indiquez son modèle exact dans le rapport.",
                self.spd_drivers.join(", ")
            ));
        }
        if self.smbus_vendor == Some(0x8086) && !self.acpi_lax && !self.smbus_unbound {
            out.push(
                "Sur certaines cartes mères Intel (et Gigabyte/ASUS), le BIOS réserve le SMBus : \
                 essayez le bouton « Débloquer l'accès SMBus » (paramètre noyau \
                 acpi_enforce_resources=lax), puis redémarrez."
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

/// Builds the report (blocking file-system reads: call from a worker).
pub fn build(diag: Option<&Diagnostics>, devices: &[DeviceInfo]) -> (String, Findings) {
    let mut s = String::new();
    let mut f = Findings::default();
    let _ = writeln!(
        s,
        "=== Rapport matériel TuxRGB {} ===",
        env!("CARGO_PKG_VERSION")
    );

    // System.
    let os = read("/etc/os-release")
        .lines()
        .find_map(|l| {
            l.strip_prefix("PRETTY_NAME=")
                .map(|v| v.trim_matches('"').to_owned())
        })
        .unwrap_or_else(|| "?".into());
    let cmdline = read("/proc/cmdline");
    f.acpi_lax = cmdline.contains("acpi_enforce_resources=lax");
    let cpu = read("/proc/cpuinfo")
        .lines()
        .find_map(|l| {
            l.strip_prefix("model name")
                .map(|v| v.trim_start_matches([' ', '\t', ':']).to_owned())
        })
        .unwrap_or_else(|| "?".into());
    let _ = writeln!(s, "\n[Système]");
    let _ = writeln!(s, "OS        : {os}");
    let _ = writeln!(s, "Noyau     : {}", read("/proc/sys/kernel/osrelease"));
    let _ = writeln!(
        s,
        "Immuable  : {}",
        if Path::new("/run/ostree-booted").exists() {
            "oui (rpm-ostree / bootc)"
        } else {
            "non"
        }
    );
    let _ = writeln!(s, "CPU       : {cpu}");
    let _ = writeln!(
        s,
        "Carte mère: {} {}",
        read("/sys/class/dmi/id/board_vendor"),
        read("/sys/class/dmi/id/board_name")
    );
    let _ = writeln!(s, "BIOS      : {}", read("/sys/class/dmi/id/bios_version"));
    let _ = writeln!(s, "Paramètres noyau : {cmdline}");
    if let Some(d) = diag {
        let _ = writeln!(s, "Format    : {}", d.package.label());
        let _ = writeln!(
            s,
            "OpenRGB   : {} – {}",
            tuxrgb_openrgb::OPENRGB_VERSION,
            d.openrgb_binary
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "introuvable".into())
        );
        let _ = writeln!(
            s,
            "Règles udev : {}{}",
            d.rules.label(),
            d.rules_path
                .as_ref()
                .map(|p| format!(" ({})", p.display()))
                .unwrap_or_default()
        );
    }

    // SMBus controllers (PCI class 0x0c05).
    let _ = writeln!(s, "\n[Contrôleurs SMBus (PCI)]");
    let mut smbus_pci = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/bus/pci/devices") {
        for e in entries.flatten() {
            let p = e.path();
            if !read(p.join("class")).starts_with("0x0c05") {
                continue;
            }
            let (ven, dev) = (hex(&read(p.join("vendor"))), hex(&read(p.join("device"))));
            let dram = DRAM_SMBUS.iter().find(|(v, d, _)| *v == ven && *d == dev);
            let driver = link_name(p.join("driver"));
            if dram.is_some() {
                f.dram_bus_supported = true;
                if driver.is_none() {
                    f.smbus_unbound = true;
                }
            }
            f.smbus_vendor.get_or_insert(ven);
            let _ = writeln!(
                s,
                "{} {:04x}:{:04x} pilote={} {}",
                e.file_name().to_string_lossy(),
                ven,
                dev,
                driver.clone().unwrap_or_else(|| "aucun".into()),
                match dram {
                    Some((_, _, name)) => format!("→ bus RAM OpenRGB ({name})"),
                    None => "→ non utilisé par OpenRGB pour la RAM".into(),
                }
            );
            smbus_pci.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    if smbus_pci.is_empty() {
        let _ = writeln!(s, "aucun");
    }

    // I2C adapters and clients.
    let _ = writeln!(s, "\n[Bus I2C]");
    let mut clients = Vec::new();
    let mut adapters = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/bus/i2c/devices") {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("i2c-") {
                adapters.push(name);
            } else {
                clients.push(name);
            }
        }
    }
    adapters.sort_by_key(|n| n[4..].parse::<u32>().unwrap_or(u32::MAX));
    for a in &adapters {
        let sys = Path::new("/sys/bus/i2c/devices").join(a);
        let real = fs::canonicalize(&sys).unwrap_or_default();
        let on_smbus = smbus_pci
            .iter()
            .any(|pci| real.to_string_lossy().contains(pci.as_str()));
        let dev = Path::new("/dev").join(a);
        let access = if !dev.exists() {
            "pas de /dev (i2c-dev ?)"
        } else if rw_access(&dev) {
            "lecture/écriture OK"
        } else {
            "accès refusé"
        };
        if on_smbus && dev.exists() && rw_access(&dev) {
            f.dram_bus_accessible = f.dram_bus_supported;
        }
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(&dev)
                .map(|m| format!(" (mode {:o})", m.permissions().mode() & 0o777))
                .unwrap_or_default()
        };
        let _ = writeln!(
            s,
            "{a}: \"{}\"{} – {access}{mode}",
            read(sys.join("name")),
            if on_smbus { " [SMBus chipset]" } else { "" }
        );
    }
    if adapters.is_empty() {
        let _ = writeln!(s, "aucun");
    }
    clients.sort();
    if !clients.is_empty() {
        let _ = writeln!(s, "Puces déclarées par le noyau :");
        for c in &clients {
            let sys = Path::new("/sys/bus/i2c/devices").join(c);
            let name = read(sys.join("name"));
            let driver = link_name(sys.join("driver"));
            if matches!(
                name.as_str(),
                "spd5118" | "ee1004" | "eeprom" | "at24" | "jc42"
            ) && let Some(d) = &driver
                && d != "jc42"
                && !f.spd_drivers.contains(d)
            {
                f.spd_drivers.push(d.clone());
            }
            let _ = writeln!(
                s,
                "  {c} {name} pilote={}",
                driver.unwrap_or_else(|| "aucun".into())
            );
        }
    }

    // Kernel modules.
    f.smbus_module_loaded = ["i2c_piix4", "i2c_i801", "i2c_nforce2"]
        .iter()
        .any(|m| module_loaded(m));
    let _ = writeln!(s, "\n[Modules]");
    for m in [
        "i2c_dev",
        "i2c_piix4",
        "i2c_i801",
        "i2c_nforce2",
        "spd5118",
        "ee1004",
        "at24",
        "jc42",
    ] {
        let _ = writeln!(s, "{m}: {}", if module_loaded(m) { "chargé" } else { "-" });
    }
    let _ = writeln!(
        s,
        "/etc/modules-load.d/tuxrgb-i2c.conf: {}",
        match fs::read_to_string("/etc/modules-load.d/tuxrgb-i2c.conf") {
            Ok(c) => c
                .lines()
                .filter(|l| !l.starts_with('#'))
                .collect::<Vec<_>>()
                .join(", "),
            Err(_) => "absent".into(),
        }
    );

    // Kernel log (readable without root by wheel/adm members on Fedora).
    let _ = writeln!(s, "\n[Noyau – SMBus / I2C]");
    match kernel_log() {
        Some(lines) if !lines.is_empty() => {
            for l in lines {
                let _ = writeln!(s, "{l}");
            }
        }
        Some(_) => {
            let _ = writeln!(s, "aucun message piix4 / i801 / i2c / spd5118");
        }
        None => {
            let _ = writeln!(s, "journal du noyau illisible sans droits administrateur");
        }
    }

    // Devices seen through OpenRGB.
    f.dram_detected = devices
        .iter()
        .filter(|d| d.kind == DeviceKind::Dram)
        .count();
    let _ = writeln!(
        s,
        "\n[Périphériques détectés par OpenRGB : {}]",
        devices.len()
    );
    for d in devices {
        let _ = writeln!(
            s,
            "- {} [{}] {} – {} LED – mode {} – {}",
            d.display_name(),
            d.vendor,
            d.kind.label(),
            d.led_count,
            if d.mode.is_empty() {
                "aucun"
            } else {
                d.mode.as_str()
            },
            d.location
        );
        for z in &d.zones {
            let _ = writeln!(
                s,
                "    zone \"{}\" {:?} {} LED (min {}, max {})",
                z.name, z.kind, z.led_count, z.leds_min, z.leds_max
            );
        }
    }

    // Fans (TuxRGB 3.2): hwmon chips, PWM channels and the service.
    {
        use crate::fans::hwmon;
        let hw = hwmon::discover(Path::new(hwmon::SYSFS_ROOT));
        let _ = writeln!(s, "\n[Ventilation]");
        let _ = writeln!(
            s,
            "Service tuxrgb-fans : {}",
            if Path::new("/run/tuxrgb/fans.sock").exists() {
                "actif"
            } else if Path::new("/etc/systemd/system/tuxrgb-fans.service").exists() {
                "installé, arrêté"
            } else {
                "non installé"
            }
        );
        for m in [
            "it87",
            "nct6775",
            "nct6683",
            "asus_ec_sensors",
            "gigabyte_waterforce",
        ] {
            if module_loaded(m) {
                let _ = writeln!(s, "Module    : {m}");
            }
        }
        for fan in &hw.fans {
            let _ = writeln!(
                s,
                "- {} : {} tr/min, pwm {} (mode {}){}",
                fan.id,
                hwmon::read_rpm(fan).map_or("–".into(), |v| v.to_string()),
                hwmon::read_pwm(fan).map_or("–".into(), |v| v.to_string()),
                hwmon::read_enable(fan).map_or("–".into(), |v| v.to_string()),
                if fan.controllable() {
                    ""
                } else {
                    ", lecture seule"
                }
            );
            // IT8689E: the control register TuxRGB fixes itself.
            if let Some(c) = fan.ite {
                let _ = writeln!(
                    s,
                    "    pilotage : registre 0x{:02X} de la puce (0x{:X}) corrigé par le service",
                    c.reg, c.base
                );
            }
        }
        if let Ok(l) = std::fs::read_to_string("/sys/kernel/security/lockdown") {
            let _ = writeln!(s, "Lockdown  : {}", l.trim());
        }
        if hw.fans.is_empty() {
            let _ = writeln!(s, "Aucun ventilateur dans /sys/class/hwmon");
        }
        for t in &hw.temps {
            let _ = writeln!(
                s,
                "  {} {} : {}",
                t.id,
                t.label.as_deref().unwrap_or(""),
                hwmon::read_temp(t).map_or("–".into(), |v| format!("{v:.1} °C"))
            );
        }
    }

    // OpenRGB log: detection lines first, then the end of the file.
    let log = fs::read_to_string(&crate::paths::paths().openrgb_log).unwrap_or_default();
    let _ = writeln!(s, "\n[Journal OpenRGB – détection I2C/SMBus/RAM]");
    let interesting: Vec<&str> = log
        .lines()
        .filter(|l| {
            let l = l.to_ascii_lowercase();
            [
                "i2c", "smbus", "dram", "spd", "ram", "piix4", "i801", "ene", "error", "warning",
            ]
            .iter()
            .any(|k| l.contains(k))
        })
        .collect();
    for l in interesting.iter().rev().take(80).rev() {
        let _ = writeln!(s, "{l}");
    }
    let _ = writeln!(s, "\n[Journal OpenRGB – fin]");
    let all: Vec<&str> = log.lines().collect();
    for l in &all[all.len().saturating_sub(30)..] {
        let _ = writeln!(s, "{l}");
    }

    let hints = f.ram_hints();
    if !hints.is_empty() {
        let _ = writeln!(s, "\n[Pistes pour la RAM]");
        for h in hints {
            let _ = writeln!(s, "- {h}");
        }
    }
    (s, f)
}

/// SMBus-related kernel messages of the current boot.
fn kernel_log() -> Option<Vec<String>> {
    let mut cmd = std::process::Command::new("journalctl");
    cmd.args(["-k", "-b", "--no-pager", "-q", "-o", "short-monotonic"]);
    // A huge or slow journal must not hold the report back (nor fill the
    // memory): bounded in time and size.
    let out = tuxrgb_core::process::output_with_timeout(
        cmd,
        std::time::Duration::from_secs(8),
        16 * 1024 * 1024,
    )?;
    let text = String::from_utf8_lossy(&out);
    let keys = [
        "piix4",
        "i801",
        "smbus",
        "i2c",
        "spd5118",
        "ee1004",
        "acpi_enforce",
        "resource conflict",
    ];
    let lines: Vec<String> = text
        .lines()
        .filter(|l| {
            let l = l.to_ascii_lowercase();
            keys.iter().any(|k| l.contains(k))
        })
        .map(str::to_owned)
        .collect();
    Some(lines[lines.len().saturating_sub(40)..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_runs_anywhere() {
        let (text, findings) = build(None, &tuxrgb_core::device::demo_devices());
        eprintln!("{text}");
        assert!(text.contains("[Contrôleurs SMBus (PCI)]"));
        assert_eq!(findings.dram_detected, 2);
        assert!(findings.ram_hints().is_empty());
    }

    #[test]
    fn hints_when_ram_missing() {
        let f = Findings {
            dram_bus_supported: true,
            dram_bus_accessible: true,
            spd_drivers: vec!["spd5118".into()],
            smbus_vendor: Some(0x1022),
            ..Default::default()
        };
        let hints = f.ram_hints();
        assert_eq!(hints.len(), 1);
        assert!(hints[0].contains("spd5118"));
    }

    #[test]
    fn unbound_smbus_points_to_the_kernel_fix() {
        // The B650 AORUS ELITE AX V2 report: 1022:790b, pilote=aucun.
        let f = Findings {
            dram_bus_supported: true,
            smbus_unbound: true,
            smbus_vendor: Some(0x1022),
            ..Default::default()
        };
        let hints = f.ram_hints();
        assert_eq!(hints.len(), 1);
        assert!(hints[0].contains("acpi_enforce_resources=lax"));
    }
}
