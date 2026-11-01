//! Unprivileged checks of everything OpenRGB needs.

use crate::paths::{PackageKind, package_kind};
use std::ffi::CString;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RulesState {
    /// No `60-openrgb.rules` anywhere.
    Missing,
    /// Installed by TuxRGB's helper.
    TuxRgb,
    /// Shipped by a distribution / OpenRGB package.
    Vendor,
    /// Present in /etc but not written by TuxRGB.
    Manual,
    /// `60-openrgb.rules` both in /etc and in /usr/lib: the /etc copy hides
    /// the distribution's file (OpenRGB warns about it).
    Duplicate,
    /// Could not be checked (sandbox).
    Unknown,
}

impl RulesState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Missing => "Absentes",
            Self::TuxRgb => "Installées par TuxRGB",
            Self::Vendor => "Fournies par la distribution",
            Self::Manual => "Installées manuellement",
            Self::Duplicate => "En double (/etc et /usr/lib)",
            Self::Unknown => "Inconnu (bac à sable)",
        }
    }

    pub fn ok(self) -> bool {
        matches!(
            self,
            Self::TuxRgb | Self::Vendor | Self::Manual | Self::Duplicate
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// Rules + I2C ready: every device class can be used.
    Ready,
    /// USB/HID devices work, motherboard/RAM (SMBus) do not.
    Partial,
    NotConfigured,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostics {
    pub package: PackageKind,
    pub rules: RulesState,
    pub rules_path: Option<PathBuf>,
    /// TuxRGB's complement of the distribution rules is installed.
    pub extra_rules: bool,
    /// Rules installed by an older TuxRGB (before the "no world-writable
    /// grant" format): re-running the setup hardens them.
    pub rules_outdated: bool,
    /// `/dev/i2c-*` / `/dev/hidraw*` nodes readable or writable by every
    /// user (a too permissive rule from elsewhere).
    pub world_accessible_nodes: Vec<String>,
    pub i2c_dev_loaded: bool,
    pub smbus_driver: Option<String>,
    pub i2c_nodes: usize,
    pub i2c_accessible: usize,
    pub hidraw_nodes: usize,
    pub hidraw_accessible: usize,
    pub pkexec_available: bool,
    pub openrgb_binary: Option<PathBuf>,
    pub openrgb_bundled: bool,
    pub openrgb_origin: &'static str,
}

impl Diagnostics {
    pub fn readiness(&self) -> Readiness {
        let hid_ok = self.rules.ok()
            || (self.hidraw_nodes > 0 && self.hidraw_accessible == self.hidraw_nodes);
        let smbus_ok = self.i2c_dev_loaded && self.i2c_nodes > 0 && self.i2c_accessible > 0;
        match (hid_ok, smbus_ok) {
            (true, true) => Readiness::Ready,
            (true, false) => Readiness::Partial,
            _ => Readiness::NotConfigured,
        }
    }
}

fn accessible(path: &Path) -> bool {
    let Ok(c) = CString::new(path.as_os_str().as_encoded_bytes()) else {
        return false;
    };
    unsafe { libc::access(c.as_ptr(), libc::R_OK | libc::W_OK) == 0 }
}

/// Device nodes whose permissions give access to every user.
fn world_accessible(prefixes: &[&str]) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let Ok(dir) = std::fs::read_dir("/dev") else {
        return Vec::new();
    };
    let mut out: Vec<String> = dir
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            prefixes.iter().any(|p| n.starts_with(p))
        })
        .filter(|e| {
            e.metadata()
                .map(|m| m.permissions().mode() & 0o006 != 0)
                .unwrap_or(false)
        })
        .map(|e| format!("/dev/{}", e.file_name().to_string_lossy()))
        .collect();
    out.sort();
    out
}

/// TuxRGB rules written before the hardened format.
fn rules_outdated() -> bool {
    let root = host_root();
    [
        "/etc/udev/rules.d/60-openrgb.rules",
        "/etc/udev/rules.d/61-tuxrgb-openrgb-extra.rules",
    ]
    .iter()
    .filter_map(|p| std::fs::read_to_string(format!("{root}{p}")).ok())
    .any(|text| {
        text.starts_with("# Installed by TuxRGB") && !text.contains("TuxRGB rules format 2")
    })
}

fn count_nodes(prefix: &str) -> (usize, usize) {
    let Ok(dir) = std::fs::read_dir("/dev") else {
        return (0, 0);
    };
    let nodes: Vec<PathBuf> = dir
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .map(|e| e.path())
        .collect();
    let ok = nodes.iter().filter(|p| accessible(p)).count();
    (nodes.len(), ok)
}

fn host_root() -> &'static str {
    // Inside Flatpak the host /etc is only visible through /run/host.
    if package_kind() == PackageKind::Flatpak {
        "/run/host"
    } else {
        ""
    }
}

fn rules_state() -> (RulesState, Option<PathBuf>) {
    for root in [host_root()] {
        let etc = PathBuf::from(format!("{root}/etc/udev/rules.d/60-openrgb.rules"));
        let vendor = [
            "/usr/lib/udev/rules.d/60-openrgb.rules",
            "/lib/udev/rules.d/60-openrgb.rules",
        ]
        .iter()
        .map(|v| PathBuf::from(format!("{root}{v}")))
        .find(|p| p.exists());
        if etc.exists() && vendor.is_some() {
            return (RulesState::Duplicate, Some(etc));
        }
        if etc.exists() {
            let ours = std::fs::read_to_string(&etc)
                .map(|s| s.starts_with("# Installed by TuxRGB"))
                .unwrap_or(false);
            return (
                if ours {
                    RulesState::TuxRgb
                } else {
                    RulesState::Manual
                },
                Some(etc),
            );
        }
        for vendor in [
            "/usr/lib/udev/rules.d/60-openrgb.rules",
            "/lib/udev/rules.d/60-openrgb.rules",
        ] {
            let p = PathBuf::from(format!("{root}{vendor}"));
            if p.exists() {
                return (RulesState::Vendor, Some(p));
            }
        }
    }
    if package_kind() == PackageKind::Flatpak && !Path::new("/run/host/etc").exists() {
        return (RulesState::Unknown, None);
    }
    (RulesState::Missing, None)
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
        || ["/usr/bin", "/bin"]
            .iter()
            .any(|d| Path::new(d).join(name).is_file())
}

/// Runs every check (blocking file-system reads; call from a worker).
pub fn run() -> Diagnostics {
    let (rules, rules_path) = rules_state();
    let extra_rules = Path::new(&format!(
        "{}/etc/udev/rules.d/61-tuxrgb-openrgb-extra.rules",
        host_root()
    ))
    .exists();
    let (i2c_nodes, i2c_accessible) = count_nodes("i2c-");
    let (hidraw_nodes, hidraw_accessible) = count_nodes("hidraw");
    let smbus_driver = ["i2c_piix4", "i2c_i801", "i2c_nforce2"]
        .iter()
        .find(|m| Path::new(&format!("/sys/module/{m}")).exists())
        .map(|m| m.replace('_', "-"));
    let located = tuxrgb_openrgb::locate::find_openrgb();
    Diagnostics {
        package: package_kind(),
        rules,
        rules_path,
        extra_rules,
        rules_outdated: rules_outdated(),
        world_accessible_nodes: world_accessible(&["i2c-", "hidraw"]),
        i2c_dev_loaded: Path::new("/sys/module/i2c_dev").exists() || i2c_nodes > 0,
        smbus_driver,
        i2c_nodes,
        i2c_accessible,
        hidraw_nodes,
        hidraw_accessible,
        pkexec_available: if package_kind() == PackageKind::Flatpak {
            which("flatpak-spawn")
        } else {
            which("pkexec")
        },
        openrgb_bundled: located
            .as_ref()
            .map(|l| l.origin == tuxrgb_openrgb::locate::Origin::Bundled)
            .unwrap_or(false),
        openrgb_origin: match located.as_ref().map(|l| &l.origin) {
            Some(tuxrgb_openrgb::locate::Origin::Bundled) => "embarqué",
            Some(tuxrgb_openrgb::locate::Origin::Override) => "remplacé via TUXRGB_OPENRGB",
            Some(tuxrgb_openrgb::locate::Origin::System) => "installation système",
            None => "introuvable",
        },
        openrgb_binary: located.map(|l| l.path),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn diagnostics_run_anywhere() {
        let d = super::run();
        assert!(d.i2c_accessible <= d.i2c_nodes);
        assert!(d.hidraw_accessible <= d.hidraw_nodes);
    }
}
