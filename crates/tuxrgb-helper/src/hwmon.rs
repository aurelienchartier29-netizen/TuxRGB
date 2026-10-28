//! Linux hardware monitoring (`/sys/class/hwmon`): fans, their PWM
//! controls and the temperature sensors. Shared, without dependencies, by
//! the fan service (root, writes) and by TuxRGB (reads, for the page).
//!
//! Ids are stable across reboots (the `hwmonN` numbers are not): driver name
//! + device (`it8689-it87.2624-fan1`, `amdgpu-0000:03:00.0-temp1`).

#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub const SYSFS_ROOT: &str = "/sys/class/hwmon";
/// Channels looked at per chip.
const MAX_CHANNELS: u8 = 12;

/// One fan header (tachometer and/or PWM output).
#[derive(Clone, Debug, PartialEq)]
pub struct Fan {
    pub id: String,
    /// Driver (`it8689`, `nct6798`, `amdgpu`…).
    pub chip: String,
    pub index: u8,
    pub label: Option<String>,
    pub rpm: Option<PathBuf>,
    /// `pwmN`: the fan can be driven.
    pub pwm: Option<PathBuf>,
    /// `pwmN_enable`: 1 = manual, other values = the hardware's modes.
    pub enable: Option<PathBuf>,
}

impl Fan {
    /// Has a PWM output TuxRGB may drive. Graphics cards are left to their
    /// own tools (LACT on Linux, the vendor's software on Windows): their
    /// fans are never taken over, even if their driver would allow it.
    pub fn controllable(&self) -> bool {
        self.pwm.is_some() && self.enable.is_some() && !self.on_gpu()
    }

    /// A fan of a graphics card.
    pub fn on_gpu(&self) -> bool {
        chip_kind(&self.chip) == "gpu"
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Temp {
    pub id: String,
    pub chip: String,
    pub label: Option<String>,
    pub input: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hwmon {
    pub fans: Vec<Fan>,
    pub temps: Vec<Temp>,
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Characters allowed in ids (see `fancurve::valid_id`).
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':') {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect()
}

/// Every hwmon chip under `root` (normally [`SYSFS_ROOT`]).
pub fn discover(root: &Path) -> Hwmon {
    let mut out = Hwmon::default();
    let Ok(dir) = std::fs::read_dir(root) else {
        return out;
    };
    let mut chips: Vec<PathBuf> = dir.flatten().map(|e| e.path()).collect();
    chips.sort();
    for chip in chips {
        let Some(name) = read_trim(&chip.join("name")) else {
            continue;
        };
        let device = std::fs::canonicalize(chip.join("device"))
            .ok()
            .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()));
        let tag = match &device {
            Some(d) => format!("{}-{}", clean(&name), clean(d)),
            None => clean(&name),
        };
        // Two chips with the same tag (no device link): keep them apart.
        let tag = if out
            .fans
            .iter()
            .map(|f| f.id.as_str())
            .chain(out.temps.iter().map(|t| t.id.as_str()))
            .any(|id| id.starts_with(&format!("{tag}-")))
        {
            let n = chip
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!("{tag}-{}", clean(&n))
        } else {
            tag
        };
        for i in 1..=MAX_CHANNELS {
            let rpm = chip.join(format!("fan{i}_input"));
            let pwm = chip.join(format!("pwm{i}"));
            let enable = chip.join(format!("pwm{i}_enable"));
            let (has_rpm, has_pwm) = (rpm.is_file(), pwm.is_file());
            if has_rpm || has_pwm {
                out.fans.push(Fan {
                    id: format!("{tag}-fan{i}"),
                    chip: name.clone(),
                    index: i,
                    label: read_trim(&chip.join(format!("fan{i}_label"))),
                    rpm: has_rpm.then_some(rpm),
                    pwm: has_pwm.then_some(pwm),
                    enable: (has_pwm && enable.is_file()).then_some(enable),
                });
            }
            let input = chip.join(format!("temp{i}_input"));
            if input.is_file() {
                out.temps.push(Temp {
                    id: format!("{tag}-temp{i}"),
                    chip: name.clone(),
                    label: read_trim(&chip.join(format!("temp{i}_label"))),
                    input,
                });
            }
        }
    }
    out
}

/// °C, or `None` when unreadable or absurd.
pub fn read_temp(t: &Temp) -> Option<f32> {
    let milli: i64 = read_trim(&t.input)?.parse().ok()?;
    let c = milli as f32 / 1000.0;
    (-40.0..=150.0).contains(&c).then_some(c)
}

pub fn read_rpm(f: &Fan) -> Option<u32> {
    read_trim(f.rpm.as_ref()?)?.parse().ok()
}

pub fn read_pwm(f: &Fan) -> Option<u8> {
    read_trim(f.pwm.as_ref()?)?.parse().ok()
}

pub fn read_enable(f: &Fan) -> Option<u8> {
    read_trim(f.enable.as_ref()?)?.parse().ok()
}

pub fn write_pwm(f: &Fan, value: u8) -> std::io::Result<()> {
    let p = f.pwm.as_ref().ok_or(std::io::ErrorKind::NotFound)?;
    std::fs::write(p, format!("{value}\n"))
}

pub fn write_enable(f: &Fan, value: u8) -> std::io::Result<()> {
    let p = f.enable.as_ref().ok_or(std::io::ErrorKind::NotFound)?;
    std::fs::write(p, format!("{value}\n"))
}

/// The processor's temperature sensor (package / Tctl).
pub fn cpu_sensor(temps: &[Temp]) -> Option<&Temp> {
    let label = |t: &Temp, l: &str| {
        t.label
            .as_deref()
            .is_some_and(|x| x.eq_ignore_ascii_case(l))
    };
    temps
        .iter()
        .find(|t| t.chip == "k10temp" && label(t, "Tctl"))
        .or_else(|| {
            temps
                .iter()
                .find(|t| t.chip == "zenpower" && label(t, "Tdie"))
        })
        .or_else(|| {
            temps
                .iter()
                .find(|t| t.chip == "coretemp" && label(t, "Package id 0"))
        })
        .or_else(|| {
            temps
                .iter()
                .find(|t| matches!(t.chip.as_str(), "k10temp" | "zenpower" | "coretemp"))
        })
        .or_else(|| temps.iter().find(|t| t.chip == "cpu_thermal"))
}

/// The graphics card's temperature sensor (edge / GPU core).
pub fn gpu_sensor(temps: &[Temp]) -> Option<&Temp> {
    temps
        .iter()
        .find(|t| t.chip == "amdgpu" && t.label.as_deref() == Some("edge"))
        .or_else(|| {
            temps
                .iter()
                .find(|t| matches!(t.chip.as_str(), "amdgpu" | "nouveau" | "radeon"))
        })
}

/// Where a fan is, for its default name.
pub fn chip_kind(chip: &str) -> &'static str {
    match chip {
        c if c.starts_with("it8") || c.starts_with("nct") || c.starts_with("w83") => "motherboard",
        c if c.starts_with("asus") || c.starts_with("gigabyte") || c == "dell_smm" => "motherboard",
        "amdgpu" | "nouveau" | "radeon" => "gpu",
        _ => "other",
    }
}

#[cfg(test)]
pub mod fake {
    //! A fake `/sys/class/hwmon` for tests.
    use std::path::{Path, PathBuf};

    pub struct Tree(pub PathBuf);

    impl Tree {
        pub fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "tuxrgb-hwmon-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("class")).unwrap();
            std::fs::create_dir_all(root.join("devices")).unwrap();
            Self(root)
        }

        pub fn class(&self) -> PathBuf {
            self.0.join("class")
        }

        /// A chip with `files` (name → content).
        pub fn chip(&self, hwmon: &str, device: Option<&str>, files: &[(&str, &str)]) -> PathBuf {
            let dir = self.class().join(hwmon);
            std::fs::create_dir_all(&dir).unwrap();
            if let Some(d) = device {
                let dev = self.0.join("devices").join(d);
                std::fs::create_dir_all(&dev).unwrap();
                std::os::unix::fs::symlink(&dev, dir.join("device")).unwrap();
            }
            for (f, c) in files {
                std::fs::write(dir.join(f), format!("{c}\n")).unwrap();
            }
            dir
        }

        pub fn read(&self, hwmon: &str, file: &str) -> String {
            std::fs::read_to_string(self.class().join(hwmon).join(file))
                .unwrap()
                .trim()
                .to_owned()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn exists(p: &Path) -> bool {
        p.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Tree;
    use super::*;

    #[test]
    fn finds_fans_and_sensors_with_stable_ids() {
        let t = Tree::new("discover");
        t.chip(
            "hwmon3",
            Some("it87.2624"),
            &[
                ("name", "it8689"),
                ("fan1_input", "1234"),
                ("pwm1", "128"),
                ("pwm1_enable", "2"),
                ("fan2_input", "0"),
                ("pwm2", "90"),
                ("pwm2_enable", "2"),
                ("fan5_input", "800"),
                ("temp1_input", "41000"),
            ],
        );
        t.chip(
            "hwmon1",
            None,
            &[
                ("name", "k10temp"),
                ("temp1_input", "52125"),
                ("temp1_label", "Tctl"),
            ],
        );
        t.chip(
            "hwmon5",
            Some("0000:03:00.0"),
            &[
                ("name", "amdgpu"),
                ("pwm1", "60"),
                ("pwm1_enable", "2"),
                ("fan1_input", "0"),
                ("temp1_input", "47000"),
                ("temp1_label", "edge"),
                ("temp2_input", "61000"),
                ("temp2_label", "junction"),
            ],
        );
        let h = discover(&t.class());
        let ids: Vec<&str> = h.fans.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "it8689-it87.2624-fan1",
                "it8689-it87.2624-fan2",
                "it8689-it87.2624-fan5",
                "amdgpu-0000:03:00.0-fan1"
            ]
        );
        assert!(h.fans[0].controllable());
        assert!(!h.fans[2].controllable());
        assert_eq!(read_rpm(&h.fans[0]), Some(1234));
        assert_eq!(read_enable(&h.fans[0]), Some(2));
        assert_eq!(
            cpu_sensor(&h.temps).map(|x| x.id.as_str()),
            Some("k10temp-temp1")
        );
        assert_eq!(
            gpu_sensor(&h.temps).map(|x| x.id.as_str()),
            Some("amdgpu-0000:03:00.0-temp1")
        );
        assert_eq!(read_temp(cpu_sensor(&h.temps).unwrap()), Some(52.125));
        assert_eq!(chip_kind("nct6798"), "motherboard");
        assert_eq!(chip_kind("amdgpu"), "gpu");
        for f in &h.fans {
            assert!(
                f.id.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c)),
                "{}",
                f.id
            );
        }
    }

    #[test]
    fn writes_go_to_the_channel_files() {
        let t = Tree::new("write");
        t.chip(
            "hwmon2",
            Some("nct6775.656"),
            &[("name", "nct6798"), ("pwm2", "100"), ("pwm2_enable", "5")],
        );
        let h = discover(&t.class());
        let f = &h.fans[0];
        write_enable(f, 1).unwrap();
        write_pwm(f, 200).unwrap();
        assert_eq!(t.read("hwmon2", "pwm2_enable"), "1");
        assert_eq!(t.read("hwmon2", "pwm2"), "200");
        assert_eq!(read_pwm(f), Some(200));
    }
}
