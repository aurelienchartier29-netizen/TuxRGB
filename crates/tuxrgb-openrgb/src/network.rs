//! Network lights (TuxRGB 3.0): WLED controllers over DDP and Philips WiZ
//! bulbs.
//!
//! OpenRGB 1.1 drives both natively – its DDP and Philips Wiz controllers –
//! from its own settings file. TuxRGB writes the lights the user added into
//! the private server's `OpenRGB.json` before starting it (everything else
//! in the file is kept), and they appear as ordinary devices: on the
//! canvas, in every effect and in the blackout of a shutdown. A WiZ bulb's
//! own scenes ("Ocean", "Fireplace"…) are its hardware effects.
//!
//! Helpers for the settings page: what a WLED controller says about itself
//! (`/json/info`: name, LED count) and the WiZ bulbs answering on the local
//! network.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::Path;
use std::time::{Duration, Instant};

/// DDP port of WLED (and of OpenRGB's DDP controller).
pub const DDP_PORT: u16 = 4048;
/// UDP port of Philips WiZ bulbs (fixed).
pub const WIZ_PORT: u16 = 38899;

/// The network lights the user added.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkLights {
    pub wled: Vec<WledLight>,
    pub wiz: Vec<WizLight>,
}

impl NetworkLights {
    pub fn is_empty(&self) -> bool {
        self.wled.is_empty() && self.wiz.is_empty()
    }
}

/// A WLED controller (or anything that takes DDP).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WledLight {
    pub name: String,
    /// IP address or host name.
    pub host: String,
    #[serde(default = "ddp_port")]
    pub port: u16,
    pub leds: usize,
}

fn ddp_port() -> u16 {
    DDP_PORT
}

/// A Philips WiZ bulb or strip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WizLight {
    /// IP address or host name.
    pub host: String,
    /// Mix in the bulb's warm white LEDs for pale colours.
    #[serde(default)]
    pub warm_white: bool,
}

/// A host as typed by the user: an IP address or a host name, no spaces,
/// no scheme or path.
pub fn valid_host(host: &str) -> bool {
    let h = host.trim();
    !h.is_empty()
        && h.len() <= 253
        && h.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
}

/// Writes the lights into OpenRGB's settings (`DDPDevices`,
/// `PhilipsWizDevices`), keeping everything else. `Ok(true)` when the file
/// changed. A file OpenRGB cannot have written (not JSON) is kept aside as
/// `OpenRGB.json.bak` and started again.
pub fn write_settings(config_dir: &Path, lights: &NetworkLights) -> std::io::Result<bool> {
    let path = config_dir.join("OpenRGB.json");
    let mut root = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.is_object() => v,
            _ => {
                let _ = std::fs::rename(&path, config_dir.join("OpenRGB.json.bak"));
                json!({})
            }
        },
        Err(_) => json!({}),
    };
    let before = root.clone();
    let wled: Vec<Value> = lights
        .wled
        .iter()
        .filter(|l| valid_host(&l.host) && l.leds > 0)
        .map(|l| {
            json!({
                "name": l.name,
                "ip": l.host.trim(),
                "port": l.port,
                "num_leds": l.leds,
            })
        })
        .collect();
    let wiz: Vec<Value> = lights
        .wiz
        .iter()
        .filter(|l| valid_host(&l.host))
        .map(|l| {
            json!({
                "ip": l.host.trim(),
                "use_cool_white": false,
                "use_warm_white": l.warm_white,
                "selected_white_strategy": if l.warm_white { "Minimum" } else { "Average" },
            })
        })
        .collect();
    let obj = root.as_object_mut().expect("checked above");
    obj.insert("DDPDevices".into(), json!({ "devices": wled }));
    obj.insert("PhilipsWizDevices".into(), json!({ "devices": wiz }));
    if root == before && path.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(config_dir)?;
    let tmp = config_dir.join("OpenRGB.json.tuxrgb-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&root)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(true)
}

/// What a WLED controller reports about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WledInfo {
    pub name: String,
    pub leds: usize,
}

/// Asks a WLED controller (`http://host[:port]/json/info`) for its name and
/// LED count. Plain HTTP on the local network, bounded by `timeout`.
pub fn wled_info(host: &str, timeout: Duration) -> Result<WledInfo, String> {
    let host = host.trim();
    if !valid_host(host) {
        return Err("adresse invalide".into());
    }
    let target = if host.contains(':') && !host.starts_with('[') {
        host.to_owned()
    } else {
        format!("{host}:80")
    };
    let deadline = Instant::now() + timeout;
    let addr = target
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or_else(|| "adresse introuvable".to_owned())?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
    let left = || {
        deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1))
    };
    stream
        .set_write_timeout(Some(left()))
        .map_err(|e| e.to_string())?;
    let request = format!(
        "GET /json/info HTTP/1.0\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
        host.split(':').next().unwrap_or(host)
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut body = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        stream
            .set_read_timeout(Some(left()))
            .map_err(|e| e.to_string())?;
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                body.extend_from_slice(&buf[..n]);
                if body.len() > 256 * 1024 {
                    return Err("réponse trop longue".into());
                }
            }
            Err(e) => return Err(e.to_string()),
        }
        if Instant::now() >= deadline {
            return Err("délai dépassé".into());
        }
    }
    let text = String::from_utf8_lossy(&body);
    let (head, json_text) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| "réponse HTTP invalide".to_owned())?;
    if !head.starts_with("HTTP/1.") || !head.split_whitespace().nth(1).is_some_and(|c| c == "200") {
        return Err(head.lines().next().unwrap_or("HTTP").to_owned());
    }
    let info: Value =
        serde_json::from_str(json_text.trim()).map_err(|_| "pas un contrôleur WLED".to_owned())?;
    let leds = info
        .pointer("/leds/count")
        .and_then(Value::as_u64)
        .ok_or_else(|| "pas un contrôleur WLED".to_owned())? as usize;
    let name = info
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("WLED")
        .trim()
        .to_owned();
    Ok(WledInfo { name, leds })
}

/// A WiZ bulb that answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WizFound {
    pub host: String,
    pub mac: String,
}

/// Where a WiZ discovery is sent: the local broadcast address.
pub fn wiz_broadcast() -> Vec<SocketAddr> {
    vec![SocketAddr::from(([255, 255, 255, 255], WIZ_PORT))]
}

/// WiZ bulbs answering a `getPilot` request sent to `targets` (broadcast
/// addresses, or single bulbs) within `timeout`, one entry per bulb.
pub fn discover_wiz(targets: &[SocketAddr], timeout: Duration) -> Vec<WizFound> {
    let Ok(socket) = UdpSocket::bind(("0.0.0.0", 0)) else {
        return Vec::new();
    };
    let _ = socket.set_broadcast(true);
    let request = br#"{"method":"getPilot","params":{}}"#;
    for t in targets {
        if let Err(e) = socket.send_to(request, t) {
            tracing::debug!("WiZ discovery to {t}: {e}");
        }
    }
    let deadline = Instant::now() + timeout;
    let mut found: Vec<WizFound> = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let _ = socket.set_read_timeout(Some(left));
        let Ok((n, from)) = socket.recv_from(&mut buf) else {
            break;
        };
        let Ok(reply) = serde_json::from_slice::<Value>(&buf[..n]) else {
            continue;
        };
        if reply.get("method").and_then(Value::as_str) != Some("getPilot") {
            continue;
        }
        let mac = reply
            .pointer("/result/mac")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let host = from.ip().to_string();
        if !found.iter().any(|f| f.host == host) {
            found.push(WizFound { host, mac });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("tuxrgb-net-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn settings_are_merged_into_openrgb_json() {
        let dir = tmp("merge");
        std::fs::write(
            dir.join("OpenRGB.json"),
            r#"{ "Detectors": { "detectors": { "ASUS Aura": false } }, "DDPDevices": { "devices": [] } }"#,
        )
        .unwrap();
        let lights = NetworkLights {
            wled: vec![WledLight {
                name: "Bureau".into(),
                host: "192.168.1.40".into(),
                port: DDP_PORT,
                leds: 60,
            }],
            wiz: vec![WizLight {
                host: "192.168.1.41".into(),
                warm_white: true,
            }],
        };
        assert!(write_settings(&dir, &lights).unwrap());
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("OpenRGB.json")).unwrap())
                .unwrap();
        // OpenRGB's own settings are kept.
        assert_eq!(
            v.pointer("/Detectors/detectors/ASUS Aura"),
            Some(&json!(false))
        );
        assert_eq!(
            v.pointer("/DDPDevices/devices/0/ip"),
            Some(&json!("192.168.1.40"))
        );
        assert_eq!(
            v.pointer("/DDPDevices/devices/0/num_leds"),
            Some(&json!(60))
        );
        assert_eq!(v.pointer("/DDPDevices/devices/0/port"), Some(&json!(4048)));
        assert_eq!(
            v.pointer("/PhilipsWizDevices/devices/0/ip"),
            Some(&json!("192.168.1.41"))
        );
        assert_eq!(
            v.pointer("/PhilipsWizDevices/devices/0/use_warm_white"),
            Some(&json!(true))
        );
        // Nothing new: not rewritten.
        assert!(!write_settings(&dir, &lights).unwrap());
        // Removed again.
        assert!(write_settings(&dir, &NetworkLights::default()).unwrap());
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("OpenRGB.json")).unwrap())
                .unwrap();
        assert_eq!(v.pointer("/DDPDevices/devices"), Some(&json!([])));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn broken_settings_are_kept_aside() {
        let dir = tmp("broken");
        std::fs::write(dir.join("OpenRGB.json"), "{ not json").unwrap();
        assert!(write_settings(&dir, &NetworkLights::default()).unwrap());
        assert!(dir.join("OpenRGB.json.bak").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hosts_are_checked() {
        assert!(valid_host("192.168.1.40"));
        assert!(valid_host("wled-bureau.local"));
        assert!(!valid_host(""));
        assert!(!valid_host("http://192.168.1.40/"));
        assert!(!valid_host("a b"));
    }

    #[test]
    fn wled_info_reads_name_and_led_count() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = [0u8; 1024];
            let n = s.read(&mut req).unwrap();
            assert!(String::from_utf8_lossy(&req[..n]).starts_with("GET /json/info HTTP/1.0"));
            let body = r#"{"ver":"0.15.0","leds":{"count":144,"rgbw":false},"name":"Étagère"}"#;
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
        });
        let info = wled_info(&format!("127.0.0.1:{port}"), Duration::from_secs(2)).unwrap();
        assert_eq!(
            info,
            WledInfo {
                name: "Étagère".into(),
                leds: 144
            }
        );
        server.join().unwrap();
        // Nothing listening: an error, quickly.
        let started = Instant::now();
        assert!(wled_info("127.0.0.1:9", Duration::from_millis(500)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn wiz_bulbs_are_discovered() {
        let bulb = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = bulb.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            let (n, from) = bulb.recv_from(&mut buf).unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).contains("getPilot"));
            let reply = r#"{"method":"getPilot","env":"pro","result":{"mac":"a8bb50e5f1c2","rssi":-60,"state":true}}"#;
            bulb.send_to(reply.as_bytes(), from).unwrap();
        });
        let found = discover_wiz(&[addr], Duration::from_millis(800));
        t.join().unwrap();
        assert_eq!(
            found,
            vec![WizFound {
                host: "127.0.0.1".into(),
                mac: "a8bb50e5f1c2".into()
            }]
        );
    }
}
