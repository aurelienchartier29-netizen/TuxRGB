//! Fan control on Linux: a small root service (`tuxrgb-fans.service`) that
//! drives the PWM outputs of `/sys/class/hwmon` with the settings TuxRGB
//! sends it, the way `fancontrol` (lm-sensors) or `fan2go` do.
//!
//! * `enable-fans` (through pkexec, from TuxRGB) – loads the sensor driver
//!   of the motherboard if needed (`nct6775`, `it87`), installs this binary
//!   in `/usr/local/libexec/tuxrgb/` and the systemd service, and starts it.
//!   « Réinitialiser les permissions » removes all of it.
//! * `fan-daemon` – the service itself. It listens on `/run/tuxrgb/fans.sock`
//!   (owned by the user who enabled it, mode 0600), applies the settings
//!   every second and keeps them in `/var/lib/tuxrgb/fans.conf`, so the
//!   curves also run before anyone logs in.
//! * `fan-restore` – run by systemd after the service stops, crashes or the
//!   system shuts down: every fan it touched gets its original mode and
//!   speed back (the motherboard's own control).
//!
//! The protocol is a few text lines (see [`handle`]); every value received
//! is checked, and the service only ever writes the `pwmN` / `pwmN_enable`
//! files of the fans it found itself.

use crate::fancurve::{self, Driver, Mode, Output, Setting, Status};
use crate::hwmon::{self, Fan, Hwmon};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SERVICE: &str = "tuxrgb-fans.service";
pub const UNIT_PATH: &str = "/etc/systemd/system/tuxrgb-fans.service";
pub const INSTALL_DIR: &str = "/usr/local/libexec/tuxrgb";
pub const INSTALLED_HELPER: &str = "/usr/local/libexec/tuxrgb/tuxrgb-helper";
pub const SETTINGS_PATH: &str = "/var/lib/tuxrgb/fans.conf";
pub const OWNER_PATH: &str = "/var/lib/tuxrgb/fans.owner";
pub const RUN_DIR: &str = "/run/tuxrgb";
pub const SOCKET_PATH: &str = "/run/tuxrgb/fans.sock";
/// Original state of every fan taken over (for `fan-restore`).
pub const RESTORE_PATH: &str = "/run/tuxrgb/fans.restore";
pub const MODPROBE_PATH: &str = "/etc/modprobe.d/tuxrgb-fans.conf";
pub const MODULES_LOAD_PATH: &str = "/etc/modules-load.d/tuxrgb-fans.conf";
/// Bumped when the protocol changes incompatibly.
pub const PROTOCOL: u32 = 1;

const MAX_LINE: usize = 512;
const MAX_LINES: usize = 80;
/// `TEST` (identify) lasts this long.
const TEST_DURATION: Duration = Duration::from_secs(5);
/// A fan the driver refuses to hand over is retried after this.
const RETRY: Duration = Duration::from_secs(60);
const RESCAN: Duration = Duration::from_secs(30);

/// Original `pwmN_enable` / `pwmN` of a fan.
#[derive(Clone, Debug, PartialEq)]
struct Original {
    pwm: PathBuf,
    enable: PathBuf,
    mode: u8,
    value: u8,
}

/// Paths of a restore record must be hwmon channel files.
fn channel_file(p: &Path, enable: bool) -> bool {
    let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let digits = if enable {
        name.strip_prefix("pwm")
            .and_then(|r| r.strip_suffix("_enable"))
    } else {
        name.strip_prefix("pwm")
    };
    p.starts_with("/sys/")
        && !p
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        && digits
            .is_some_and(|d| !d.is_empty() && d.len() <= 2 && d.chars().all(|c| c.is_ascii_digit()))
}

fn format_restore(list: &[Original]) -> String {
    list.iter()
        .map(|o| {
            format!(
                "{} {} {} {}\n",
                o.pwm.display(),
                o.enable.display(),
                o.mode,
                o.value
            )
        })
        .collect()
}

fn parse_restore(text: &str, check_paths: bool) -> Vec<Original> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_ascii_whitespace();
            let o = Original {
                pwm: PathBuf::from(w.next()?),
                enable: PathBuf::from(w.next()?),
                mode: w.next()?.parse().ok()?,
                value: w.next()?.parse().ok()?,
            };
            let ok = !check_paths || (channel_file(&o.pwm, false) && channel_file(&o.enable, true));
            ok.then_some(o)
        })
        .collect()
}

fn write_value(p: &Path, v: u8) -> std::io::Result<()> {
    std::fs::write(p, format!("{v}\n"))
}

/// One fan under control.
struct Driven {
    driver: Driver,
    status: Status,
    /// Last value written.
    written: Option<u8>,
    /// The driver refused manual mode: retry later.
    refused_until: Option<Instant>,
}

/// The controller: settings, fans, temperatures. No I/O outside the paths
/// it is built with (tests use a fake sysfs).
pub struct Controller {
    root: PathBuf,
    restore_path: PathBuf,
    hw: Hwmon,
    settings: Vec<Setting>,
    driven: BTreeMap<String, Driven>,
    originals: Vec<Original>,
    safety: bool,
    tests: BTreeMap<String, (u8, Instant)>,
    temps: BTreeMap<String, f32>,
    nvidia: Option<(Instant, Option<f32>)>,
    last_scan: Instant,
}

impl Controller {
    pub fn new(root: &Path, restore_path: &Path, settings: Vec<Setting>) -> Self {
        Self {
            root: root.to_owned(),
            restore_path: restore_path.to_owned(),
            hw: hwmon::discover(root),
            settings,
            driven: BTreeMap::new(),
            originals: Vec::new(),
            safety: false,
            tests: BTreeMap::new(),
            temps: BTreeMap::new(),
            nvidia: None,
            last_scan: Instant::now(),
        }
    }

    pub fn set_settings(&mut self, settings: Vec<Setting>) {
        self.settings = settings;
    }

    /// Temperature of a curve source.
    fn temp_of(&self, source: &str) -> Option<f32> {
        self.temps.get(source).copied()
    }

    fn read_temps(&mut self) {
        self.temps.clear();
        for t in &self.hw.temps {
            if let Some(v) = hwmon::read_temp(t) {
                self.temps.insert(t.id.clone(), v);
            }
        }
        if let Some(v) = hwmon::cpu_sensor(&self.hw.temps).and_then(hwmon::read_temp) {
            self.temps.insert("cpu".into(), v);
        }
        let gpu = hwmon::gpu_sensor(&self.hw.temps)
            .and_then(hwmon::read_temp)
            .or_else(|| self.nvidia_temp());
        if let Some(v) = gpu {
            self.temps.insert("gpu".into(), v);
        }
    }

    /// NVIDIA cards have no hwmon sensor: `nvidia-smi`, every 2 s.
    fn nvidia_temp(&mut self) -> Option<f32> {
        if self.root != Path::new(hwmon::SYSFS_ROOT) {
            return None;
        }
        // Every 2 s; after a failure (no card, driver busy) every 30 s, so a
        // slow `nvidia-smi` never holds up the control loop for long.
        if let Some((at, v)) = self.nvidia
            && at.elapsed() < Duration::from_secs(if v.is_some() { 2 } else { 30 })
        {
            return v;
        }
        let v = if crate::system::has_tool("nvidia-smi") {
            crate::system::output(
                "nvidia-smi",
                &[
                    "--query-gpu=temperature.gpu",
                    "--format=csv,noheader,nounits",
                ],
            )
            .and_then(|o| o.lines().next().and_then(|l| l.trim().parse::<f32>().ok()))
        } else {
            None
        };
        self.nvidia = Some((Instant::now(), v));
        v
    }

    fn save_originals(&self) {
        let _ = std::fs::write(&self.restore_path, format_restore(&self.originals));
    }

    /// Takes a fan over (manual mode), remembering how it was.
    fn take(&mut self, fan: &Fan) -> bool {
        let (Some(pwm), Some(enable)) = (fan.pwm.clone(), fan.enable.clone()) else {
            return false;
        };
        if !self.originals.iter().any(|o| o.pwm == pwm) {
            let (Some(mode), Some(value)) = (hwmon::read_enable(fan), hwmon::read_pwm(fan)) else {
                return false;
            };
            self.originals.push(Original {
                pwm,
                enable,
                mode,
                value,
            });
            // Written before anything changes: a crash right after still
            // restores it.
            self.save_originals();
        }
        hwmon::write_enable(fan, 1).is_ok() && hwmon::read_enable(fan) == Some(1)
    }

    /// Gives a fan back to the hardware.
    fn release(&mut self, pwm: &Path) {
        if let Some(i) = self.originals.iter().position(|o| o.pwm == pwm) {
            let o = self.originals.remove(i);
            // Speed first, then the mode (some drivers latch the value).
            let _ = write_value(&o.pwm, o.value);
            let _ = write_value(&o.enable, o.mode);
            self.save_originals();
        }
    }

    #[cfg(test)]
    pub fn release_all(&mut self) {
        let all: Vec<PathBuf> = self.originals.iter().map(|o| o.pwm.clone()).collect();
        for p in all {
            self.release(&p);
        }
        self.driven.clear();
    }

    pub fn test(&mut self, fan: &str, pct: u8) -> Result<(), String> {
        if !self.hw.fans.iter().any(|f| f.id == fan && f.controllable()) {
            return Err("ventilateur inconnu ou non pilotable".into());
        }
        self.tests.insert(
            fan.to_owned(),
            (pct.min(100), Instant::now() + TEST_DURATION),
        );
        Ok(())
    }

    /// One second of control.
    pub fn tick(&mut self, dt: f32) {
        if self.last_scan.elapsed() >= RESCAN {
            self.last_scan = Instant::now();
            let hw = hwmon::discover(&self.root);
            // Fans that disappeared are forgotten (their chip is gone).
            self.driven
                .retain(|id, _| hw.fans.iter().any(|f| &f.id == id));
            self.hw = hw;
        }
        self.read_temps();
        let hottest = [self.temp_of("cpu"), self.temp_of("gpu")]
            .into_iter()
            .flatten()
            .reduce(f32::max);
        self.safety = fancurve::safety_state(self.safety, hottest);
        let now = Instant::now();
        self.tests.retain(|_, (_, until)| *until > now);

        let fans: Vec<Fan> = self
            .hw
            .fans
            .iter()
            .filter(|f| f.controllable())
            .cloned()
            .collect();
        for fan in fans {
            let mode = self
                .settings
                .iter()
                .find(|s| s.fan == fan.id)
                .map(|s| s.mode.clone())
                .unwrap_or(Mode::Auto);
            let test = self.tests.get(&fan.id).map(|(p, _)| *p);
            let temp = match &mode {
                Mode::Curve(c) => self.temp_of(&c.source),
                _ => None,
            };
            let safety = self.safety;
            let entry = self.driven.entry(fan.id.clone()).or_insert_with(|| Driven {
                driver: Driver::default(),
                status: Status::Auto,
                written: None,
                refused_until: None,
            });
            let (out, status) = match test {
                Some(p) => (Output::Duty(f32::from(p)), Status::Driven),
                None => entry.driver.step(&mode, temp, safety, dt),
            };
            entry.status = status;
            match out {
                Output::Auto => {
                    entry.written = None;
                    entry.refused_until = None;
                    let pwm = fan.pwm.clone().unwrap_or_default();
                    self.release(&pwm);
                }
                Output::Duty(pct) => {
                    if entry.refused_until.is_some_and(|t| t > now) {
                        continue;
                    }
                    let value = fancurve::pwm_of(pct);
                    // Manual mode (again, if the firmware took it back
                    // after a resume).
                    let manual = hwmon::read_enable(&fan) == Some(1);
                    let entry_written = entry.written;
                    if !manual && !self.take(&fan) {
                        let e = self.driven.get_mut(&fan.id).expect("just inserted");
                        e.refused_until = Some(now + RETRY);
                        e.written = None;
                        let pwm = fan.pwm.clone().unwrap_or_default();
                        self.release(&pwm);
                        continue;
                    }
                    let e = self.driven.get_mut(&fan.id).expect("just inserted");
                    e.refused_until = None;
                    if (entry_written != Some(value) || !manual)
                        && hwmon::write_pwm(&fan, value).is_ok()
                    {
                        e.written = Some(value);
                    }
                }
            }
        }
    }

    /// `FAN <id> <pwm|-> <rpm|-> <status>` lines and `TEMP <id> <°C>`.
    pub fn state(&self) -> String {
        let mut s = String::new();
        for f in &self.hw.fans {
            let status = self
                .driven
                .get(&f.id)
                .map(|d| {
                    if d.refused_until.is_some() {
                        "refused"
                    } else {
                        d.status.as_str()
                    }
                })
                .unwrap_or("auto");
            let pwm = hwmon::read_pwm(f).map_or("-".into(), |v| v.to_string());
            let rpm = hwmon::read_rpm(f).map_or("-".into(), |v| v.to_string());
            let ctl = if f.controllable() { "ctl" } else { "ro" };
            s.push_str(&format!("FAN {} {pwm} {rpm} {ctl} {status}\n", f.id));
        }
        for (id, v) in &self.temps {
            s.push_str(&format!("TEMP {id} {v:.1}\n"));
        }
        if self.safety {
            s.push_str("SAFETY on\n");
        }
        s
    }
}

/// One client's commands (the socket only accepts its owner).
///
/// * `PING` → `OK tuxrgb-fand <version> <protocol>`
/// * `SET` + `FAN …` lines + `END` → `OK` / `ERR <why>` (kept on disk)
/// * `STATE` → `FAN …` / `TEMP …` lines + `END`
/// * `TEST <fan> <pct>` → `OK`: that speed for 5 s (finding a fan)
fn handle(stream: UnixStream, ctl: &Arc<Mutex<Controller>>, settings_path: &Path) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    let mut writer = writer;
    let mut reader = BufReader::new(stream.take(64 * 1024));
    let mut line = String::new();
    let mut lines = 0usize;
    loop {
        line.clear();
        match reader.by_ref().take(MAX_LINE as u64).read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        lines += 1;
        if lines > MAX_LINES * 4 || !line.ends_with('\n') {
            let _ = writer.write_all(b"ERR ligne trop longue\n");
            return;
        }
        let cmd = line.trim();
        let reply = match cmd.split_ascii_whitespace().collect::<Vec<_>>().as_slice() {
            ["PING"] => format!("OK tuxrgb-fand {} {PROTOCOL}\n", env!("CARGO_PKG_VERSION")),
            ["STATE"] => {
                let c = ctl.lock().unwrap_or_else(|e| e.into_inner());
                format!("{}END\n", c.state())
            }
            ["TEST", fan, pct] => match pct.parse::<u8>() {
                Ok(p) if p <= 100 && fancurve::valid_id(fan) => {
                    match ctl.lock().unwrap_or_else(|e| e.into_inner()).test(fan, p) {
                        Ok(()) => "OK\n".into(),
                        Err(e) => format!("ERR {e}\n"),
                    }
                }
                _ => "ERR vitesse invalide\n".into(),
            },
            ["SET"] => {
                let mut settings = Vec::new();
                let mut error = None;
                loop {
                    line.clear();
                    match reader.by_ref().take(MAX_LINE as u64).read_line(&mut line) {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    lines += 1;
                    let l = line.trim();
                    if l == "END" {
                        break;
                    }
                    if settings.len() >= MAX_LINES || lines > MAX_LINES * 4 {
                        error = Some("trop de lignes".to_owned());
                        continue;
                    }
                    match Setting::parse(l) {
                        Ok(s) if !settings.iter().any(|x: &Setting| x.fan == s.fan) => {
                            settings.push(s)
                        }
                        Ok(_) => error = Some("ventilateur en double".into()),
                        Err(e) => error = Some(e),
                    }
                }
                match error {
                    Some(e) => format!("ERR {e}\n"),
                    None => {
                        let text = format!(
                            "# TuxRGB fan settings (written by tuxrgb-fans.service)\n{}",
                            fancurve::format_settings(&settings)
                        );
                        let _ = crate::system::write_atomic(
                            &settings_path.to_string_lossy(),
                            text.as_bytes(),
                            0o644,
                        );
                        ctl.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .set_settings(settings);
                        "OK\n".into()
                    }
                }
            }
            _ => "ERR commande inconnue\n".into(),
        };
        if writer.write_all(reply.as_bytes()).is_err() {
            return;
        }
    }
}

fn owner_uid() -> Option<u32> {
    std::fs::read_to_string(OWNER_PATH)
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|u| *u > 0)
}

/// `fan-daemon`: runs until stopped (systemd).
pub fn daemon() -> Result<(), String> {
    let uid = owner_uid().ok_or("propriétaire du service inconnu (réactivez depuis TuxRGB)")?;
    crate::system::ensure_dir(RUN_DIR, 0o755).map_err(|e| e.to_string())?;
    let _ = std::fs::set_permissions(RUN_DIR, std::fs::Permissions::from_mode(0o755));
    let _ = std::fs::remove_file(SOCKET_PATH);
    let listener = UnixListener::bind(SOCKET_PATH).map_err(|e| e.to_string())?;
    std::os::unix::fs::chown(SOCKET_PATH, Some(uid), None).map_err(|e| e.to_string())?;
    std::fs::set_permissions(SOCKET_PATH, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;

    // A previous run that died without its restore: put things back first.
    restore(RESTORE_PATH);
    let settings =
        fancurve::parse_settings(&std::fs::read_to_string(SETTINGS_PATH).unwrap_or_default());
    let ctl = Arc::new(Mutex::new(Controller::new(
        Path::new(hwmon::SYSFS_ROOT),
        Path::new(RESTORE_PATH),
        settings,
    )));
    println!("tuxrgb-fand {} started", env!("CARGO_PKG_VERSION"));

    let server = ctl.clone();
    std::thread::Builder::new()
        .name("fand-socket".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                handle(stream, &server, Path::new(SETTINGS_PATH));
            }
        })
        .map_err(|e| e.to_string())?;

    let mut last = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let dt = last.elapsed().as_secs_f32().clamp(0.1, 5.0);
        last = Instant::now();
        ctl.lock().unwrap_or_else(|e| e.into_inner()).tick(dt);
    }
}

/// `fan-restore`: every fan back to its original mode and speed.
pub fn restore(path: &str) {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    for o in parse_restore(&text, true) {
        let _ = write_value(&o.pwm, o.value);
        let _ = write_value(&o.enable, o.mode);
        println!("STEP fan-restored {}", o.pwm.display());
    }
    let _ = std::fs::remove_file(path);
}

/// The systemd unit for `helper` (absolute path).
pub fn unit(helper: &str) -> String {
    format!(
        "# Installed by TuxRGB – fan control. Remove with: TuxRGB → Réglages →\n\
         # Réinitialiser les permissions / Accès matériel.\n\
         [Unit]\n\
         Description=TuxRGB fan control\n\
         After=systemd-modules-load.service\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={helper} fan-daemon\n\
         ExecStopPost={helper} fan-restore\n\
         Restart=on-failure\n\
         RestartSec=3\n\
         NoNewPrivileges=yes\n\
         PrivateTmp=yes\n\
         ProtectHome=yes\n\
         ProtectSystem=full\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hwmon::fake::Tree;

    fn board(t: &Tree) {
        t.chip(
            "hwmon3",
            Some("it87.2624"),
            &[
                ("name", "it8689"),
                ("fan1_input", "1200"),
                ("pwm1", "128"),
                ("pwm1_enable", "2"),
                ("fan2_input", "900"),
                ("pwm2", "77"),
                ("pwm2_enable", "2"),
            ],
        );
        t.chip(
            "hwmon1",
            Some("0000:00:18.3"),
            &[
                ("name", "k10temp"),
                ("temp1_input", "50000"),
                ("temp1_label", "Tctl"),
            ],
        );
    }

    fn set_cpu(t: &Tree, c: f32) {
        std::fs::write(
            t.class().join("hwmon1/temp1_input"),
            format!("{}\n", (c * 1000.0) as i64),
        )
        .unwrap();
    }

    #[test]
    fn curve_takes_over_and_restore_puts_everything_back() {
        let t = Tree::new("fand-curve");
        board(&t);
        let restore_path = t.0.join("fans.restore");
        let fan1 = "it8689-it87.2624-fan1";
        let settings = fancurve::parse_settings(&format!(
            "FAN {fan1} curve cpu 40:20,80:100\nFAN it8689-it87.2624-fan2 auto\n"
        ));
        let mut c = Controller::new(&t.class(), &restore_path, settings);
        c.tick(1.0);
        // 50 °C → 40 %: manual mode, PWM 102.
        assert_eq!(t.read("hwmon3", "pwm1_enable"), "1");
        assert_eq!(t.read("hwmon3", "pwm1"), "102");
        // The auto fan is untouched.
        assert_eq!(t.read("hwmon3", "pwm2_enable"), "2");
        assert_eq!(t.read("hwmon3", "pwm2"), "77");
        // Originals on disk before any change.
        let saved = parse_restore(&std::fs::read_to_string(&restore_path).unwrap(), false);
        assert_eq!(saved.len(), 1);
        assert_eq!((saved[0].mode, saved[0].value), (2, 128));
        // The firmware takes the fan back (resume): taken again.
        std::fs::write(t.class().join("hwmon3/pwm1_enable"), "2\n").unwrap();
        c.tick(1.0);
        assert_eq!(t.read("hwmon3", "pwm1_enable"), "1");
        // Safety: 91 °C → full speed at once.
        set_cpu(&t, 91.0);
        c.tick(1.0);
        assert_eq!(t.read("hwmon3", "pwm1"), "255");
        assert!(c.state().contains("SAFETY on"));
        assert!(
            c.state()
                .contains(&format!("FAN {fan1} 255 1200 ctl safety"))
        );
        // Back to auto: the original mode and speed.
        set_cpu(&t, 50.0);
        c.set_settings(Vec::new());
        c.tick(1.0);
        assert_eq!(t.read("hwmon3", "pwm1_enable"), "2");
        assert_eq!(t.read("hwmon3", "pwm1"), "128");
        assert_eq!(std::fs::read_to_string(&restore_path).unwrap(), "");
    }

    #[test]
    fn identify_and_lost_sensor() {
        let t = Tree::new("fand-test");
        board(&t);
        let restore_path = t.0.join("fans.restore");
        let fan2 = "it8689-it87.2624-fan2";
        let mut c = Controller::new(
            &t.class(),
            &restore_path,
            fancurve::parse_settings(&format!("FAN {fan2} curve cpu 40:20,80:100\n")),
        );
        assert!(c.test("nope", 50).is_err());
        c.test("it8689-it87.2624-fan1", 100).unwrap();
        c.tick(1.0);
        assert_eq!(t.read("hwmon3", "pwm1"), "255");
        assert_eq!(t.read("hwmon3", "pwm1_enable"), "1");
        // The sensor disappears: after the grace period, the hardware.
        std::fs::remove_file(t.class().join("hwmon1/temp1_input")).unwrap();
        for _ in 0..3 {
            c.tick(1.0);
        }
        assert_eq!(t.read("hwmon3", "pwm2_enable"), "2");
        assert_eq!(t.read("hwmon3", "pwm2"), "77");
        assert!(
            c.state()
                .contains(&format!("FAN {fan2} 77 900 ctl nosensor"))
        );
        c.release_all();
        assert_eq!(t.read("hwmon3", "pwm1_enable"), "2");
        assert_eq!(t.read("hwmon3", "pwm1"), "128");
    }

    #[test]
    fn graphics_card_fans_are_never_driven() {
        let t = Tree::new("fand-gpu");
        board(&t);
        t.chip(
            "hwmon5",
            Some("0000:03:00.0"),
            &[
                ("name", "amdgpu"),
                ("fan1_input", "800"),
                ("pwm1", "60"),
                ("pwm1_enable", "2"),
                ("temp1_input", "45000"),
                ("temp1_label", "edge"),
            ],
        );
        let gpu = "amdgpu-0000:03:00.0-fan1";
        let mut c = Controller::new(
            &t.class(),
            &t.0.join("fans.restore"),
            fancurve::parse_settings(&format!("FAN {gpu} fixed 100\n")),
        );
        assert!(c.test(gpu, 100).is_err());
        c.tick(1.0);
        assert_eq!(t.read("hwmon5", "pwm1_enable"), "2");
        assert_eq!(t.read("hwmon5", "pwm1"), "60");
        assert!(c.state().contains(&format!("FAN {gpu} 60 800 ro auto")));
        // Its temperature still drives the curves of the other fans.
        assert!(c.state().contains("TEMP gpu 45.0"));
    }

    #[test]
    fn restore_records_only_name_channel_files() {
        let good = "/sys/devices/platform/it87.2624/hwmon/hwmon3/pwm1 /sys/devices/platform/it87.2624/hwmon/hwmon3/pwm1_enable 2 128\n";
        assert_eq!(parse_restore(good, true).len(), 1);
        for bad in [
            "/etc/passwd /sys/x/pwm1_enable 2 1\n",
            "/sys/x/pwm1 /etc/shadow 2 1\n",
            "/sys/../etc/pwm1 /sys/x/pwm1_enable 2 1\n",
            "/sys/x/pwm1 /sys/x/pwm1_enable 300 1\n",
            "/sys/x/pwmA /sys/x/pwmA_enable 2 1\n",
        ] {
            assert!(parse_restore(bad, true).is_empty(), "{bad}");
        }
    }

    #[test]
    fn socket_protocol() {
        let t = Tree::new("fand-socket");
        board(&t);
        let dir = t.0.clone();
        let sock = dir.join("s.sock");
        let settings_path = dir.join("fans.conf");
        let ctl = Arc::new(Mutex::new(Controller::new(
            &t.class(),
            &dir.join("fans.restore"),
            Vec::new(),
        )));
        let listener = UnixListener::bind(&sock).unwrap();
        let server = ctl.clone();
        let sp = settings_path.clone();
        std::thread::spawn(move || {
            for s in listener.incoming().flatten() {
                handle(s, &server, &sp);
            }
        });
        let mut s = UnixStream::connect(&sock).unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut line = String::new();
        s.write_all(b"PING\n").unwrap();
        r.read_line(&mut line).unwrap();
        assert!(line.starts_with("OK tuxrgb-fand "), "{line}");
        line.clear();
        s.write_all(b"SET\nFAN it8689-it87.2624-fan1 fixed 30\nEND\n")
            .unwrap();
        r.read_line(&mut line).unwrap();
        assert_eq!(line, "OK\n");
        assert!(
            std::fs::read_to_string(&settings_path)
                .unwrap()
                .contains("FAN it8689-it87.2624-fan1 fixed 30")
        );
        line.clear();
        s.write_all(b"SET\nFAN x fixed 300\nEND\n").unwrap();
        r.read_line(&mut line).unwrap();
        assert!(line.starts_with("ERR"), "{line}");
        // The refused settings changed nothing.
        assert_eq!(ctl.lock().unwrap().settings.len(), 1);
        ctl.lock().unwrap().tick(1.0);
        assert_eq!(t.read("hwmon3", "pwm1"), "77");
        line.clear();
        s.write_all(b"STATE\n").unwrap();
        let mut state = String::new();
        while !state.ends_with("END\n") {
            line.clear();
            r.read_line(&mut line).unwrap();
            state.push_str(&line);
        }
        assert!(
            state.contains("FAN it8689-it87.2624-fan1 77 1200 ctl driven"),
            "{state}"
        );
        assert!(state.contains("TEMP cpu 50.0"), "{state}");
        line.clear();
        s.write_all(b"RM -rf\n").unwrap();
        r.read_line(&mut line).unwrap();
        assert!(line.starts_with("ERR"));
        ctl.lock().unwrap().release_all();
    }

    #[test]
    fn unit_runs_the_restore_after_stopping() {
        let u = unit(INSTALLED_HELPER);
        assert!(u.contains("ExecStart=/usr/local/libexec/tuxrgb/tuxrgb-helper fan-daemon"));
        assert!(u.contains("ExecStopPost=/usr/local/libexec/tuxrgb/tuxrgb-helper fan-restore"));
        assert!(u.contains("WantedBy=multi-user.target"));
    }
}
