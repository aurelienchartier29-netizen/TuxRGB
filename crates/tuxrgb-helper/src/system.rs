//! Small, careful system helpers (no external crates).

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// No /usr/local: only directories owned by the distribution.
const SAFE_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";
const TOOL_DIRS: &[&str] = &["/usr/sbin", "/usr/bin", "/sbin", "/bin"];

pub fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    unsafe { geteuid() == 0 }
}

/// Drops every inherited variable that could influence child tools.
pub fn sanitize_environment() {
    let keep = ["LANG", "LC_ALL", "LC_MESSAGES"];
    let vars: Vec<String> = std::env::vars_os()
        .filter_map(|(k, _)| k.into_string().ok())
        .filter(|k| !keep.contains(&k.as_str()))
        .collect();
    for k in vars {
        // SAFETY: single-threaded at this point (called first thing in main).
        unsafe { std::env::remove_var(k) };
    }
    unsafe { std::env::set_var("PATH", SAFE_PATH) };
}

/// Least privilege for the files this process creates.
pub fn harden_process() {
    unsafe extern "C" {
        fn umask(mask: u32) -> u32;
    }
    // SAFETY: umask has no preconditions.
    unsafe {
        umask(0o022);
    }
}

/// Kernel arguments the helper may add or remove: an allowlist, plus a
/// strict `name=value` character check (defence in depth – the value never
/// comes from the caller, and no shell is ever used).
pub fn validated_karg(karg: &str) -> Result<&str, String> {
    const ALLOWED: &[&str] = &["acpi_enforce_resources=lax"];
    let well_formed = karg.len() < 64
        && karg.split_once('=').is_some_and(|(k, v)| {
            !k.is_empty()
                && !v.is_empty()
                && !k.starts_with('-')
                && k.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                && v.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        });
    if well_formed && ALLOWED.contains(&karg) {
        Ok(karg)
    } else {
        Err("paramètre noyau refusé".into())
    }
}

/// A system tool, only if it (and its directory) belong to root and cannot
/// be modified by anyone else.
fn tool(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let trusted = |p: &Path| {
        fs::metadata(p)
            .map(|m| m.uid() == 0 && m.mode() & 0o022 == 0)
            .unwrap_or(false)
    };
    TOOL_DIRS.iter().map(|d| Path::new(d).join(name)).find(|p| {
        let real = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        real.is_file() && trusted(&real) && real.parent().is_some_and(trusted)
    })
}

pub fn has_tool(name: &str) -> bool {
    tool(name).is_some()
}

/// A DDR5 SPD hub is handled by the kernel's spd5118 driver.
pub fn spd5118_in_use() -> bool {
    Path::new("/sys/module/spd5118").exists()
        || fs::read_dir("/sys/bus/i2c/devices")
            .map(|d| {
                d.flatten().any(|e| {
                    fs::read_to_string(e.path().join("name"))
                        .map(|n| n.trim() == "spd5118")
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
}

/// Runs a system tool by absolute path, output forwarded to our stdout.
pub fn run(name: &str, args: &[&str]) -> Result<(), String> {
    let path = tool(name).ok_or_else(|| format!("{name} introuvable"))?;
    let status = Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("code {}", status.code().unwrap_or(-1)))
    }
}

/// Runs a system tool by absolute path and returns its standard output
/// (bounded, with a deadline: a hung tool never blocks the caller).
pub fn output(name: &str, args: &[&str]) -> Option<String> {
    use std::io::Read;
    let path = tool(name)?;
    let mut child = Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
    let mut out = String::new();
    child
        .stdout
        .take()?
        .take(64 * 1024)
        .read_to_string(&mut out)
        .ok()?;
    Some(out)
}

/// Re-applies SELinux labels when available (Fedora Atomic: Bazzite, Aurora…).
pub fn restorecon(paths: &[&str]) {
    if let Some(p) = tool("restorecon") {
        let _ = Command::new(p)
            .arg("-F")
            .args(paths)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

pub fn ensure_dir(path: &str, mode: u32) -> io::Result<()> {
    if !Path::new(path).is_dir() {
        fs::create_dir_all(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

/// Like [`write_atomic`], for files on the EFI system partition (FAT has no
/// permissions: chmod errors are ignored there).
pub fn write_atomic_boot(path: &str, content: &[u8], mode: u32) -> io::Result<()> {
    write_atomic_inner(path, content, mode, true)
}

/// Writes through a temporary file in the same directory, then renames.
pub fn write_atomic(path: &str, content: &[u8], mode: u32) -> io::Result<()> {
    write_atomic_inner(path, content, mode, false)
}

fn write_atomic_inner(path: &str, content: &[u8], mode: u32, lenient: bool) -> io::Result<()> {
    let target = Path::new(path);
    let dir = target
        .parent()
        .ok_or_else(|| io::Error::other("no parent"))?;
    let tmp = dir.join(format!(
        ".{}.tuxrgb-tmp",
        target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
    ));
    // A stale temporary file (crash) is removed; `create_new` (O_EXCL) then
    // never follows a symbolic link planted in its place.
    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        f.write_all(content)?;
        f.sync_all()?;
    }
    match fs::set_permissions(&tmp, fs::Permissions::from_mode(mode)) {
        Ok(()) => {}
        Err(_) if lenient => {}
        Err(e) => return Err(e),
    }
    fs::rename(&tmp, target)
}

/// One SMBus controller of the chipset.
#[derive(Debug, Clone)]
pub struct SmbusController {
    pub address: String,
    pub vendor: String,
    /// Kernel driver bound to it (`piix4_smbus`, `i801_smbus`…), if any.
    pub driver: Option<String>,
}

/// SMBus controllers (PCI class 0x0c05) of AMD / Intel / NVIDIA chipsets.
pub fn smbus_controllers() -> Vec<SmbusController> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/bus/pci/devices") else {
        return out;
    };
    for e in entries.flatten() {
        let read = |f: &str| fs::read_to_string(e.path().join(f)).unwrap_or_default();
        if !read("class").trim().starts_with("0x0c05") {
            continue;
        }
        let vendor = read("vendor").trim().to_owned();
        if !["0x8086", "0x1022", "0x1d94", "0x10de"].contains(&vendor.as_str()) {
            continue;
        }
        let driver = fs::read_link(e.path().join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        out.push(SmbusController {
            address: e.file_name().to_string_lossy().into_owned(),
            vendor,
            driver,
        });
    }
    out.sort_by(|a, b| a.address.cmp(&b.address));
    out
}

/// SMBus drivers for the detected chipset (always with `i2c-dev`).
pub fn smbus_modules() -> Vec<&'static str> {
    let mut modules = vec!["i2c-dev"];
    let mut found = false;
    if let Ok(entries) = fs::read_dir("/sys/bus/pci/devices") {
        for e in entries.flatten() {
            let read = |f: &str| fs::read_to_string(e.path().join(f)).unwrap_or_default();
            // PCI class 0x0c05xx = SMBus controller.
            if !read("class").trim().starts_with("0x0c05") {
                continue;
            }
            let module = match read("vendor").trim() {
                "0x8086" => Some("i2c-i801"),
                "0x1022" | "0x1d94" => Some("i2c-piix4"),
                "0x10de" => Some("i2c-nforce2"),
                _ => None,
            };
            if let Some(m) = module {
                if !modules.contains(&m) {
                    modules.push(m);
                }
                found = true;
            }
        }
    }
    if !found {
        let cpu = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
        if cpu.contains("GenuineIntel") {
            modules.push("i2c-i801");
        } else if cpu.contains("AuthenticAMD") || cpu.contains("HygonGenuine") {
            modules.push("i2c-piix4");
        }
    }
    modules
}

/// Record of what `install` created.
#[derive(Default)]
pub struct Manifest {
    files: Vec<String>,
    kargs: Vec<String>,
    pub backup: Option<String>,
    /// How the kernel parameter was added (`rpm-ostree`, `grub`…).
    pub karg_method: Option<String>,
    /// Boot configuration files edited for it.
    karg_files: Vec<String>,
}

impl Manifest {
    pub fn load(path: &str) -> Self {
        let mut m = Self::default();
        for line in fs::read_to_string(path).unwrap_or_default().lines() {
            if let Some(f) = line.strip_prefix("file ") {
                m.files.push(f.trim().to_owned());
            } else if let Some(k) = line.strip_prefix("karg ") {
                m.kargs.push(k.trim().to_owned());
            } else if let Some(b) = line.strip_prefix("backup ") {
                m.backup = Some(b.trim().to_owned());
            } else if let Some(x) = line.strip_prefix("kargmethod ") {
                m.karg_method = Some(x.trim().to_owned());
            } else if let Some(f) = line.strip_prefix("kargfile ") {
                m.karg_files.push(f.trim().to_owned());
            }
        }
        m
    }

    pub fn owns(&self, path: &str) -> bool {
        self.files.iter().any(|f| f == path)
    }

    pub fn add(&mut self, path: &str) {
        if !self.owns(path) {
            self.files.push(path.to_owned());
        }
    }

    pub fn remove(&mut self, path: &str) {
        self.files.retain(|f| f != path);
    }

    pub fn add_karg(&mut self, karg: &str) {
        if !self.kargs.iter().any(|k| k == karg) {
            self.kargs.push(karg.to_owned());
        }
    }

    pub fn kargs(&self) -> impl Iterator<Item = &str> {
        self.kargs.iter().map(String::as_str)
    }

    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(String::as_str)
    }

    pub fn add_karg_file(&mut self, path: &str) {
        if !self.karg_files.iter().any(|f| f == path) {
            self.karg_files.push(path.to_owned());
        }
    }

    pub fn karg_files(&self) -> impl Iterator<Item = &str> {
        self.karg_files.iter().map(String::as_str)
    }

    pub fn save(&self, path: &str) -> io::Result<()> {
        let mut s = String::from("# TuxRGB privileged helper manifest – do not edit\nversion 1\n");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        s.push_str(&format!("date {now}\n"));
        for f in &self.files {
            s.push_str(&format!("file {f}\n"));
        }
        for k in &self.kargs {
            s.push_str(&format!("karg {k}\n"));
        }
        if let Some(b) = &self.backup {
            s.push_str(&format!("backup {b}\n"));
        }
        if let Some(m) = &self.karg_method {
            s.push_str(&format!("kargmethod {m}\n"));
        }
        for f in &self.karg_files {
            s.push_str(&format!("kargfile {f}\n"));
        }
        write_atomic(path, s.as_bytes(), 0o644)
    }
}

/// Human/machine readable status (`key=value` lines).
pub fn status_report() -> String {
    let yes = |b: bool| if b { "yes" } else { "no" };
    let rules = Path::new(crate::RULES_PATH).exists();
    let ours = fs::read_to_string(crate::RULES_PATH)
        .map(|s| s.starts_with("# Installed by TuxRGB"))
        .unwrap_or(false);
    let vendor_rules = [
        "/usr/lib/udev/rules.d/60-openrgb.rules",
        "/lib/udev/rules.d/60-openrgb.rules",
    ]
    .iter()
    .any(|p| Path::new(p).exists());
    let i2c_dev = Path::new("/sys/module/i2c_dev").exists();
    let i2c_nodes = fs::read_dir("/dev")
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("i2c-"))
                .count()
        })
        .unwrap_or(0);
    let smbus = ["i2c_piix4", "i2c_i801", "i2c_nforce2"]
        .iter()
        .filter(|m| Path::new(&format!("/sys/module/{m}")).exists())
        .copied()
        .collect::<Vec<_>>()
        .join(",");
    let controllers = smbus_controllers();
    let bound = controllers.iter().filter(|c| c.driver.is_some()).count();
    let method = crate::bootloader::detect(has_tool);
    format!(
        "smbus_controllers={}\nsmbus_bound={}\nkarg_method={}\nrules_installed={}\nrules_owned_by_tuxrgb={}\nvendor_rules={}\nextra_rules={}\nsmbus_karg={}\nmodules_config={}\ni2c_dev_loaded={}\ni2c_nodes={}\nsmbus_drivers={}\nmanifest={}\nrecommended_modules={}\n",
        controllers.len(),
        bound,
        method.name(),
        yes(rules),
        yes(ours),
        yes(vendor_rules),
        yes(Path::new(crate::EXTRA_RULES_PATH).exists()),
        yes(fs::read_to_string("/proc/cmdline")
            .unwrap_or_default()
            .split_whitespace()
            .any(|a| a == crate::SMBUS_KARG)),
        yes(Path::new(crate::MODULES_PATH).exists()),
        yes(i2c_dev),
        i2c_nodes,
        if smbus.is_empty() {
            "none".into()
        } else {
            smbus
        },
        yes(Path::new(crate::MANIFEST_PATH).exists()),
        smbus_modules().join(","),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrip() {
        let dir = std::env::temp_dir().join(format!("tuxrgb-helper-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("installed.list");
        let mut m = Manifest::default();
        m.add("/etc/udev/rules.d/60-openrgb.rules");
        m.add("/etc/udev/rules.d/60-openrgb.rules");
        m.backup = Some("/var/lib/tuxrgb/backup-60-openrgb.rules".into());
        m.add_karg("acpi_enforce_resources=lax");
        m.karg_method = Some("grub".into());
        m.add_karg_file("/etc/default/grub");
        m.add_karg_file("/etc/default/grub");
        m.save(path.to_str().unwrap()).unwrap();
        let back = Manifest::load(path.to_str().unwrap());
        assert_eq!(back.files().count(), 1);
        assert_eq!(
            back.kargs().collect::<Vec<_>>(),
            ["acpi_enforce_resources=lax"]
        );
        assert!(back.owns("/etc/udev/rules.d/60-openrgb.rules"));
        assert_eq!(
            back.backup.as_deref(),
            Some("/var/lib/tuxrgb/backup-60-openrgb.rules")
        );
        assert_eq!(back.karg_method.as_deref(), Some("grub"));
        assert_eq!(back.karg_files().collect::<Vec<_>>(), ["/etc/default/grub"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn modules_always_include_i2c_dev() {
        assert_eq!(smbus_modules()[0], "i2c-dev");
    }

    #[test]
    fn kernel_argument_allowlist_blocks_injection() {
        assert_eq!(
            validated_karg("acpi_enforce_resources=lax"),
            Ok("acpi_enforce_resources=lax")
        );
        for bad in [
            "acpi_enforce_resources=lax; rm -rf /",
            "acpi_enforce_resources=lax init=/bin/sh",
            "acpi_enforce_resources=$(id)",
            "acpi_enforce_resources=`id`",
            "--delete=quiet",
            "init=/bin/sh",
            "acpi_enforce_resources=strict",
            "acpi_enforce_resources=lax\n",
            "",
        ] {
            assert!(validated_karg(bad).is_err(), "accepted: {bad:?}");
        }
    }

    #[test]
    fn untrusted_tools_are_ignored() {
        // A name that cannot exist in the trusted directories.
        assert!(tool("tuxrgb-definitely-not-a-tool").is_none());
        assert!(!SAFE_PATH.contains("local"));
    }
}
