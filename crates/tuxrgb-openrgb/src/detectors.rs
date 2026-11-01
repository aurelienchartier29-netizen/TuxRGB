//! Prudence on the memory bus (TuxRGB 3.4): OpenRGB's RAM and SMBus
//! motherboard detectors stay off unless the user turns them on.
//!
//! Lighting the RAM, and the motherboards driven over SMBus (older ASUS
//! Aura, ASRock Polychrome, Gigabyte RGB Fusion SMBus, EVGA), means probing
//! and writing on the system's SMBus: the bus the SPD chips of the memory
//! sticks sit on. It works on most computers, but a wrong write there has
//! made sticks unreadable in rare cases, so TuxRGB only does it when asked.
//! Nothing else is concerned: USB / HID devices (most recent motherboards
//! included), graphics cards (their own I2C bus, separate detectors) and
//! network lights keep being detected.
//!
//! OpenRGB reads `Detectors.detectors.<name>` from its settings: with every
//! memory-module detector off it does not even read the SPD of the sticks
//! (`IsAnyDimmDetectorEnabled`).

use serde_json::{Map, Value, json};

/// What the user allowed on the system SMBus (3.5: RAM and motherboards
/// apart, so that lighting the RAM does not also probe the bus for
/// motherboards most computers do not have).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SmbusLighting {
    /// Memory sticks.
    pub ram: bool,
    /// Motherboards driven over the SMBus (ASRock Polychrome, older ASUS
    /// Aura and Gigabyte RGB Fusion, EVGA).
    pub motherboard: bool,
}

impl SmbusLighting {
    pub const OFF: Self = Self {
        ram: false,
        motherboard: false,
    };

    pub fn any(self) -> bool {
        self.ram || self.motherboard
    }
}

/// The motherboard detectors of the bundled OpenRGB 1.1 that probe the
/// system SMBus (`REGISTER_I2C_DETECTOR`).
pub const SMBUS_MOTHERBOARD_DETECTORS: &[&str] = &[
    "ASRock Motherboard SMBus Controllers",
    "ASUS Aura SMBus Motherboard",
    "EVGA Motherboard SMBus Controllers",
    "Gigabyte RGB Fusion",
    "Gigabyte RGB Fusion 2 SMBus",
];

/// The memory detectors: `REGISTER_I2C_DETECTOR` (older RAM, probed on the
/// bus) and `REGISTER_I2C_DRAM_DETECTOR` (RAM found through its SPD).
pub const SMBUS_RAM_DETECTORS: &[&str] = &[
    "Corsair DRAM",
    "Corsair Vengeance RGB DRAM",
    "Crucial Ballistix",
    "ENE SMBus DRAM",
    "Gigabyte RGB Fusion 2 DRAM",
    "HyperX DRAM",
    "Kingston Fury DDR4 DRAM",
    "Kingston Fury DDR5 DRAM",
    "Patriot Viper",
    "Patriot Viper Steel",
    "T-Force Xtreem DDR4 DRAM",
];

/// Turns the SMBus detectors on or off in OpenRGB's settings (`root`, the
/// whole `OpenRGB.json`), keeping every other detector as it is. Also keeps
/// OpenRGB's shared SMBus lock on (Windows: `Global\Access_SMBUS.HTP.Method`,
/// the lock HWiNFO, AIDA64 and other tools take before each transfer), in
/// case an older settings file turned it off.
pub fn apply(root: &mut Map<String, Value>, allowed: SmbusLighting) {
    let section = root.entry("Detectors").or_insert_with(|| json!({}));
    if !section.is_object() {
        *section = json!({});
    }
    let list = section
        .as_object_mut()
        .expect("made an object above")
        .entry("detectors")
        .or_insert_with(|| json!({}));
    if !list.is_object() {
        *list = json!({});
    }
    let list = list.as_object_mut().expect("made an object above");
    for name in SMBUS_MOTHERBOARD_DETECTORS {
        list.insert((*name).to_owned(), Value::Bool(allowed.motherboard));
    }
    for name in SMBUS_RAM_DETECTORS {
        list.insert((*name).to_owned(), Value::Bool(allowed.ram));
    }
    let drivers = root.entry("Drivers").or_insert_with(|| json!({}));
    if !drivers.is_object() {
        *drivers = json!({});
    }
    drivers
        .as_object_mut()
        .expect("made an object above")
        .insert("shared_smbus_access".into(), Value::Bool(true));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn detectors_are_switched_and_the_others_kept() {
        let mut root = json!({
            "Detectors": { "detectors": { "Logitech G502": false, "ENE SMBus DRAM": true } },
            "Theme": { "theme": "dark" }
        });
        apply(root.as_object_mut().unwrap(), SmbusLighting::OFF);
        assert_eq!(
            root.pointer("/Detectors/detectors/ENE SMBus DRAM"),
            Some(&json!(false))
        );
        assert_eq!(
            root.pointer("/Drivers/shared_smbus_access"),
            Some(&json!(true))
        );
        assert_eq!(
            root.pointer("/Detectors/detectors/Kingston Fury DDR5 DRAM"),
            Some(&json!(false))
        );
        // Not ours: untouched.
        assert_eq!(
            root.pointer("/Detectors/detectors/Logitech G502"),
            Some(&json!(false))
        );
        assert_eq!(root.pointer("/Theme/theme"), Some(&json!("dark")));
        // RAM only: the motherboard detectors stay off.
        apply(
            root.as_object_mut().unwrap(),
            SmbusLighting {
                ram: true,
                motherboard: false,
            },
        );
        assert_eq!(
            root.pointer("/Detectors/detectors/ENE SMBus DRAM"),
            Some(&json!(true))
        );
        assert_eq!(
            root.pointer("/Detectors/detectors/ASRock Motherboard SMBus Controllers"),
            Some(&json!(false))
        );
        // A section OpenRGB cannot have written is started again.
        let mut odd = json!({ "Detectors": [1, 2] });
        apply(odd.as_object_mut().unwrap(), SmbusLighting::OFF);
        assert_eq!(
            odd.pointer("/Detectors/detectors/Patriot Viper"),
            Some(&json!(false))
        );
    }

    /// Every SMBus detector registered in the bundled OpenRGB sources is in
    /// the list (and nothing else): a new OpenRGB that adds one must not
    /// slip past the prudence.
    #[test]
    fn list_matches_the_bundled_openrgb() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/openrgb/Controllers");
        if !root.is_dir() {
            eprintln!("OpenRGB sources not present, check skipped");
            return;
        }
        let mut found = BTreeSet::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "cpp") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                for name in registered(&text) {
                    found.insert(name);
                }
            }
        }
        let ours: BTreeSet<String> = SMBUS_MOTHERBOARD_DETECTORS
            .iter()
            .chain(SMBUS_RAM_DETECTORS)
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(found, ours);
    }

    /// Names given to `REGISTER_I2C_DETECTOR` / `REGISTER_I2C_DRAM_DETECTOR`
    /// (and their `_DISABLED` forms) in one source file, macros resolved.
    fn registered(text: &str) -> Vec<String> {
        let define = |name: &str| {
            text.lines().find_map(|l| {
                let rest = l.trim().strip_prefix("#define")?.trim_start();
                let rest = rest.strip_prefix(name)?;
                let value = rest.trim();
                value
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .map(str::to_owned)
            })
        };
        let mut out = Vec::new();
        for line in text.lines() {
            let line = line.trim_start();
            if line.starts_with("//") || line.starts_with("#define") {
                continue;
            }
            for macro_name in [
                "REGISTER_I2C_DETECTOR(",
                "REGISTER_I2C_DETECTOR_DISABLED(",
                "REGISTER_I2C_DRAM_DETECTOR(",
                "REGISTER_I2C_DRAM_DETECTOR_DISABLED(",
            ] {
                let Some(args) = line.strip_prefix(macro_name) else {
                    continue;
                };
                let first = args.split(',').next().unwrap_or_default().trim();
                let name = match first.strip_prefix('"') {
                    Some(quoted) => quoted.trim_end_matches('"').to_owned(),
                    None => {
                        define(first).unwrap_or_else(|| panic!("unresolved detector name {first}"))
                    }
                };
                out.push(name);
            }
        }
        out
    }
}
