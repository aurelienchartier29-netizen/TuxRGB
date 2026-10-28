//! Linux: talks to `tuxrgb-fans.service` (the settings, the state of the
//! fans it drives) and reads `/sys/class/hwmon` for what the page shows.

use super::{
    Access, Command, Events, FanInfo, FanKind, FanStatus, FansConfig, FansState, TempInfo, curve,
    hwmon,
};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

pub const SOCKET: &str = "/run/tuxrgb/fans.sock";
/// Same as the service (`fand::PROTOCOL`).
const PROTOCOL: u32 = 1;
const RESCAN: Duration = Duration::from_secs(10);

/// One request to the service: `request` (a command, or a block), and the
/// reply lines up to `END`, `OK` or `ERR …`.
fn ask(request: &str) -> std::io::Result<Vec<String>> {
    let mut s = UnixStream::connect(SOCKET)?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    s.set_write_timeout(Some(Duration::from_secs(3)))?;
    s.write_all(request.as_bytes())?;
    let mut r = BufReader::new(s);
    let mut out = Vec::new();
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(out);
        }
        let line = line.trim_end().to_owned();
        let last =
            line == "END" || line == "OK" || line.starts_with("OK ") || line.starts_with("ERR");
        out.push(line);
        if last || out.len() > 400 {
            return Ok(out);
        }
    }
}

/// The service's version, if it answers (`Err`: another protocol).
fn ping() -> Option<Result<String, String>> {
    let reply = ask("PING\n").ok()?;
    let mut w = reply.first()?.split_ascii_whitespace();
    if w.next() != Some("OK") || w.next() != Some("tuxrgb-fand") {
        return None;
    }
    let version = w.next()?.to_owned();
    let protocol: u32 = w.next()?.parse().ok()?;
    Some(if protocol == PROTOCOL {
        Ok(version)
    } else {
        Err(version)
    })
}

fn push(config: &FansConfig) -> Result<(), String> {
    let mut req = String::from("SET\n");
    req.push_str(&curve::format_settings(&config.settings()));
    req.push_str("END\n");
    let reply = ask(&req).map_err(|e| e.to_string())?;
    match reply.first().map(String::as_str) {
        Some("OK") => Ok(()),
        Some(e) => Err(e.trim_start_matches("ERR ").to_owned()),
        None => Err("pas de réponse".into()),
    }
}

/// What the service says: each fan's (pwm, status), and the safety mode.
type ServiceState = (BTreeMap<String, (Option<u8>, FanStatus)>, bool);

fn service_state() -> Option<ServiceState> {
    let reply = ask("STATE\n").ok()?;
    if reply.last().map(String::as_str) != Some("END") {
        return None;
    }
    let mut fans = BTreeMap::new();
    let mut safety = false;
    for l in &reply {
        let w: Vec<&str> = l.split_ascii_whitespace().collect();
        match w.as_slice() {
            ["FAN", id, pwm, _rpm, _ctl, status] => {
                let status = match *status {
                    "refused" => FanStatus::Refused,
                    s => curve::Status::parse(s).map_or(FanStatus::Auto, FanStatus::from_curve),
                };
                fans.insert((*id).to_owned(), (pwm.parse().ok(), status));
            }
            ["SAFETY", "on"] => safety = true,
            _ => {}
        }
    }
    Some((fans, safety))
}

fn kind_of(chip: &str) -> FanKind {
    match hwmon::chip_kind(chip) {
        "motherboard" => FanKind::Motherboard,
        "gpu" => FanKind::Gpu,
        _ => FanKind::Other,
    }
}

/// Readable name of a sensor.
fn temp_label(t: &hwmon::Temp) -> String {
    let label = t.label.clone();
    match (t.chip.as_str(), label.as_deref()) {
        ("k10temp" | "zenpower" | "coretemp", l) => match l {
            Some("Tctl") | Some("Tdie") | Some("Package id 0") | None => {
                crate::i18n::t("Processeur").to_owned()
            }
            Some(l) => format!("{} · {l}", crate::i18n::t("Processeur")),
        },
        ("amdgpu", Some("edge")) | ("nouveau" | "radeon", _) => {
            crate::i18n::t("Carte graphique").to_owned()
        }
        ("amdgpu", Some("junction")) => crate::i18n::t("Carte graphique · point chaud").to_owned(),
        ("amdgpu", Some("mem")) => crate::i18n::t("Mémoire vidéo").to_owned(),
        ("nvme", l) => match l {
            Some("Composite") | None => "SSD NVMe".to_owned(),
            Some(l) => format!("SSD NVMe · {l}"),
        },
        (chip, l) if hwmon::chip_kind(chip) == "motherboard" => {
            let n = t.id.rsplit("temp").next().unwrap_or("");
            match l {
                Some(l) => format!("{} · {l}", crate::i18n::t("Carte mère")),
                None => crate::i18n::trf!("Carte mère {}", n),
            }
        }
        (chip, Some(l)) => format!("{chip} · {l}"),
        (chip, None) => chip.to_owned(),
    }
}

fn snapshot(hw: &hwmon::Hwmon, service: Option<&ServiceState>, access: Access) -> FansState {
    // Graphics card fans are not shown: the page points to LACT for them.
    let fans = hw
        .fans
        .iter()
        .filter(|f| !f.on_gpu())
        .map(|f| {
            let from_service = service.and_then(|(m, _)| m.get(&f.id));
            let pwm = from_service
                .and_then(|(p, _)| *p)
                .or_else(|| hwmon::read_pwm(f));
            FanInfo {
                id: f.id.clone(),
                chip: f.chip.clone(),
                index: f.index,
                kind: kind_of(&f.chip),
                label: f.label.clone(),
                rpm: hwmon::read_rpm(f),
                duty: pwm.map(curve::pct_of),
                controllable: f.controllable(),
                status: from_service.map_or(FanStatus::Auto, |(_, s)| *s),
            }
        })
        .collect();
    let mut temps = Vec::new();
    let cpu = hwmon::cpu_sensor(&hw.temps);
    let gpu = hwmon::gpu_sensor(&hw.temps);
    temps.push(TempInfo {
        id: "cpu".into(),
        label: crate::i18n::t("Processeur").into(),
        value: cpu.and_then(hwmon::read_temp),
    });
    temps.push(TempInfo {
        id: "gpu".into(),
        label: crate::i18n::t("Carte graphique").into(),
        value: gpu.and_then(hwmon::read_temp),
    });
    for t in &hw.temps {
        if Some(t) == cpu || Some(t) == gpu {
            continue;
        }
        temps.push(TempInfo {
            id: t.id.clone(),
            label: temp_label(t),
            value: hwmon::read_temp(t),
        });
    }
    FansState {
        access,
        fans,
        temps,
        safety: service.is_some_and(|(_, s)| *s),
        demo: false,
    }
}

pub fn run(rx: Receiver<Command>, events: Events) {
    let root = Path::new(hwmon::SYSFS_ROOT);
    let mut hw = hwmon::discover(root);
    let mut scanned = Instant::now();
    let mut config: Option<FansConfig> = None;
    // The configuration still has to reach the service.
    let mut pending = false;
    let mut page = false;
    let mut last: Option<FansState> = None;
    let mut last_version: Option<String> = None;
    loop {
        let wait = if page {
            Duration::from_secs(1)
        } else if pending {
            Duration::from_secs(10)
        } else {
            Duration::from_secs(3600)
        };
        match rx.recv_timeout(wait) {
            Ok(Command::Configure(c)) => {
                pending |= config.as_ref() != Some(&c);
                config = Some(c);
                // Drags of a curve point: only the last of a burst.
                while let Ok(cmd) = rx.recv_timeout(Duration::from_millis(150)) {
                    match cmd {
                        Command::Configure(c) => {
                            pending |= config.as_ref() != Some(&c);
                            config = Some(c);
                        }
                        Command::Page(p) => page = p,
                        Command::Quit => return,
                        Command::Release(Some(done)) => {
                            let _ = done.send(());
                        }
                        Command::Identify(id) => {
                            let _ = ask(&format!("TEST {id} 100\n"));
                        }
                        Command::Rescan => {
                            hw = hwmon::discover(root);
                            scanned = Instant::now();
                            last = None;
                            last_version = None;
                            pending = true;
                        }
                        Command::Release(None) => {}
                    }
                }
            }
            Ok(Command::Page(p)) => {
                page = p;
                if p {
                    last = None;
                }
            }
            Ok(Command::Identify(id)) => {
                let _ = ask(&format!("TEST {id} 100\n"));
            }
            Ok(Command::Rescan) => {
                hw = hwmon::discover(root);
                scanned = Instant::now();
                last = None;
                last_version = None;
                pending = config.is_some();
            }
            // The service keeps the fans when TuxRGB quits: nothing to do.
            Ok(Command::Release(done)) => {
                if let Some(d) = done {
                    let _ = d.send(());
                }
                continue;
            }
            Ok(Command::Quit) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if scanned.elapsed() >= RESCAN {
            hw = hwmon::discover(root);
            scanned = Instant::now();
        }
        let access = match ping() {
            Some(Ok(version)) => {
                // (Re)started service: it gets TuxRGB's settings.
                if last_version.as_deref() != Some(&version) {
                    last_version = Some(version.clone());
                    pending |= config.is_some();
                }
                if pending && let Some(c) = &config {
                    match push(c) {
                        Ok(()) => pending = false,
                        Err(e) => tracing::warn!("fan settings refused: {e}"),
                    }
                }
                if version == env!("CARGO_PKG_VERSION") {
                    Access::Ready
                } else {
                    Access::ServiceOutdated(version)
                }
            }
            Some(Err(version)) => {
                last_version = None;
                Access::ServiceOutdated(version)
            }
            None => {
                last_version = None;
                Access::ServiceMissing
            }
        };
        if !page && !matches!(access, Access::ServiceMissing) && last.is_some() {
            continue;
        }
        let service = if access == Access::Ready || matches!(access, Access::ServiceOutdated(_)) {
            service_state()
        } else {
            None
        };
        let state = snapshot(&hw, service.as_ref(), access);
        if last.as_ref() != Some(&state) {
            events(state.clone());
            last = Some(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwmon::fake::Tree;

    #[test]
    fn page_snapshot_from_sysfs() {
        let t = Tree::new("gui-snapshot");
        t.chip(
            "hwmon3",
            Some("it87.2624"),
            &[
                ("name", "it8689"),
                ("fan1_input", "1200"),
                ("pwm1", "128"),
                ("pwm1_enable", "2"),
                ("temp1_input", "36000"),
            ],
        );
        t.chip(
            "hwmon1",
            Some("0000:00:18.3"),
            &[
                ("name", "k10temp"),
                ("temp1_input", "52000"),
                ("temp1_label", "Tctl"),
            ],
        );
        let hw = hwmon::discover(&t.class());
        let mut service = BTreeMap::new();
        service.insert(
            "it8689-it87.2624-fan1".to_owned(),
            (Some(255u8), FanStatus::Safety),
        );
        let s = snapshot(&hw, Some(&(service, true)), Access::Ready);
        assert!(s.safety);
        assert_eq!(s.fans.len(), 1);
        assert_eq!(s.fans[0].rpm, Some(1200));
        assert_eq!(s.fans[0].duty, Some(100.0));
        assert_eq!(s.fans[0].status, FanStatus::Safety);
        assert_eq!(s.fans[0].kind, FanKind::Motherboard);
        assert_eq!(s.temps[0].id, "cpu");
        assert_eq!(s.temps[0].value, Some(52.0));
        assert_eq!(s.temps[1].value, None);
        assert!(s.temps.iter().any(|t| t.id == "it8689-it87.2624-temp1"));
        // Without the service: the values of sysfs.
        let s = snapshot(&hw, None, Access::ServiceMissing);
        assert!((s.fans[0].duty.unwrap() - 50.2).abs() < 0.1);
        assert_eq!(s.fans[0].status, FanStatus::Auto);
    }
}
