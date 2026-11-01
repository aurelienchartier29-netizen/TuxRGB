//! Finding the lights on the local network: every brand at once, each with
//! its own protocol (UDP broadcast, multicast scan, SSDP-style search or
//! mDNS), then a short follow-up for names, LED counts and capabilities.

use super::{
    ElgatoLight, GoveeLight, HueBridge, KasaLight, LifxLight, NanoleafLight, NetLight, WizLight,
    WledLight, YeelightLight, http, mdns,
};
use serde_json::Value;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

pub const WIZ_PORT: u16 = 38899;
pub const GOVEE_SCAN_PORT: u16 = 4001;
pub const GOVEE_REPLY_PORT: u16 = 4002;
pub const GOVEE_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
pub const LIFX_PORT: u16 = 56700;
pub const YEELIGHT_SEARCH_PORT: u16 = 1982;
pub const YEELIGHT_PORT: u16 = 55443;
pub const KASA_PORT: u16 = 9999;
pub const NANOLEAF_PORT: u16 = 16021;
pub const ELGATO_PORT: u16 = 9123;

const MDNS_WLED: &str = "_wled._tcp.local";
const MDNS_ELGATO: &str = "_elg._tcp.local";
const MDNS_HUE: &str = "_hue._tcp.local";
const MDNS_NANOLEAF: &str = "_nanoleafapi._tcp.local";

/// Every light answering on the local network: the searches all run
/// within `timeout`, the follow-up questions (names, LED counts) take at
/// most `timeout` more.
pub fn discover(timeout: Duration) -> Vec<NetLight> {
    std::thread::scope(|s| {
        let searches = [
            s.spawn(|| wiz(&broadcast(WIZ_PORT), timeout)),
            s.spawn(|| govee(None, timeout)),
            s.spawn(|| lifx(&broadcast(LIFX_PORT), timeout)),
            s.spawn(|| yeelight(timeout)),
            s.spawn(|| kasa(&broadcast(KASA_PORT), timeout)),
            s.spawn(|| mdns_lights(timeout)),
        ];
        let mut out: Vec<NetLight> = Vec::new();
        for h in searches {
            for light in h.join().unwrap_or_default() {
                if !out
                    .iter()
                    .any(|o| o.brand() == light.brand() && o.host() == light.host())
                {
                    out.push(light);
                }
            }
        }
        out
    })
}

/// The limited broadcast address on `port` (sent on the interface of the
/// default route: the local network in practice).
pub fn broadcast(port: u16) -> Vec<SocketAddr> {
    vec![SocketAddr::from((Ipv4Addr::BROADCAST, port))]
}

fn socket(broadcast: bool) -> Option<UdpSocket> {
    let s = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    if broadcast {
        let _ = s.set_broadcast(true);
    }
    Some(s)
}

/// Receives until `deadline`, handing each datagram to `each`.
fn receive(s: &UdpSocket, deadline: Instant, mut each: impl FnMut(&[u8], SocketAddr)) {
    let mut buf = [0u8; 4096];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let _ = s.set_read_timeout(Some(left));
        match s.recv_from(&mut buf) {
            Ok((n, from)) => each(&buf[..n], from),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            // Windows reports an ICMP "port unreachable" from an earlier
            // send as an error on the next receive: keep listening.
            Err(_) => continue,
        }
    }
}

// ---------------------------------------------------------------- WiZ ---

/// Philips WiZ bulbs answering a `getPilot` sent to `targets`.
pub fn wiz(targets: &[SocketAddr], timeout: Duration) -> Vec<NetLight> {
    let Some(s) = socket(true) else {
        return Vec::new();
    };
    for t in targets {
        if let Err(e) = s.send_to(br#"{"method":"getPilot","params":{}}"#, t) {
            tracing::debug!("WiZ discovery to {t}: {e}");
        }
    }
    let mut out: Vec<NetLight> = Vec::new();
    receive(&s, Instant::now() + timeout, |data, from| {
        let Ok(reply) = serde_json::from_slice::<Value>(data) else {
            return;
        };
        if reply.get("method").and_then(Value::as_str) != Some("getPilot") {
            return;
        }
        let host = from.ip().to_string();
        if !out.iter().any(|l| l.host() == host) {
            out.push(NetLight::Wiz(WizLight {
                host,
                warm_white: false,
            }));
        }
    });
    out
}

// -------------------------------------------------------------- Govee ---

/// Govee lights answering the LAN API scan ("LAN Control" on in the Govee
/// Home app); `only`: keep this address only.
pub fn govee(only: Option<IpAddr>, timeout: Duration) -> Vec<NetLight> {
    // The lights answer on port 4002 of the sender, whatever its port.
    let s = match UdpSocket::bind(("0.0.0.0", GOVEE_REPLY_PORT)) {
        Ok(s) => s,
        Err(e) => {
            tracing::info!("Govee search: port {GOVEE_REPLY_PORT} unavailable ({e})");
            return Vec::new();
        }
    };
    let _ = s.join_multicast_v4(&GOVEE_GROUP, &Ipv4Addr::UNSPECIFIED);
    let scan = br#"{"msg":{"cmd":"scan","data":{"account_topic":"reserve"}}}"#;
    if let Err(e) = s.send_to(scan, (GOVEE_GROUP, GOVEE_SCAN_PORT)) {
        tracing::debug!("Govee scan: {e}");
    }
    if let Some(ip) = only {
        let _ = s.send_to(scan, (ip, GOVEE_SCAN_PORT));
    }
    let mut out: Vec<NetLight> = Vec::new();
    receive(&s, Instant::now() + timeout, |data, from| {
        let Some(light) = govee_reply(data, from.ip()) else {
            return;
        };
        if only.is_some_and(|ip| ip.to_string() != light.host) {
            return;
        }
        if !out.iter().any(|l| l.host() == light.host) {
            out.push(NetLight::Govee(light));
        }
    });
    out
}

/// A scan reply: `{"msg":{"cmd":"scan","data":{"ip":…,"sku":…}}}`.
pub fn govee_reply(data: &[u8], from: IpAddr) -> Option<GoveeLight> {
    let v: Value = serde_json::from_slice(data).ok()?;
    if v.pointer("/msg/cmd").and_then(Value::as_str) != Some("scan") {
        return None;
    }
    let d = v.pointer("/msg/data")?;
    let host = d
        .get("ip")
        .and_then(Value::as_str)
        .filter(|ip| ip.parse::<IpAddr>().is_ok())
        .map(str::to_owned)
        .unwrap_or_else(|| from.to_string());
    Some(GoveeLight {
        host,
        model: d
            .get("sku")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    })
}

// --------------------------------------------------------------- LIFX ---

const LIFX_GET_SERVICE: u16 = 2;
const LIFX_STATE_SERVICE: u16 = 3;
const LIFX_GET_LABEL: u16 = 23;
const LIFX_STATE_LABEL: u16 = 25;
const LIFX_GET_COLOR_ZONES: u16 = 502;
const LIFX_STATE_ZONE: u16 = 503;
const LIFX_STATE_MULTI_ZONE: u16 = 506;
const LIFX_GET_EXTENDED_COLOR_ZONES: u16 = 511;
const LIFX_STATE_EXTENDED_COLOR_ZONES: u16 = 512;
/// Our identifier in LIFX packets (any non-zero value: replies come back
/// unicast).
const LIFX_SOURCE: u32 = 0x5458_5247;

/// A LIFX LAN protocol packet (`target`: `None` for every device).
pub fn lifx_packet(kind: u16, target: Option<[u8; 8]>, seq: u8, payload: &[u8]) -> Vec<u8> {
    let size = 36 + payload.len();
    let mut p = Vec::with_capacity(size);
    p.extend((size as u16).to_le_bytes());
    // Protocol 1024, addressable, tagged when sent to everyone.
    let flags: u16 = 1024 | 1 << 12 | if target.is_none() { 1 << 13 } else { 0 };
    p.extend(flags.to_le_bytes());
    p.extend(LIFX_SOURCE.to_le_bytes());
    p.extend(target.unwrap_or_default());
    p.extend([0u8; 6]);
    p.push(0x01); // a response is required
    p.push(seq);
    p.extend([0u8; 8]);
    p.extend(kind.to_le_bytes());
    p.extend([0u8; 2]);
    p.extend(payload);
    p
}

/// Type, target and payload of a LIFX packet.
pub fn lifx_parse(p: &[u8]) -> Option<(u16, [u8; 8], &[u8])> {
    if p.len() < 36 {
        return None;
    }
    let size = u16::from_le_bytes([p[0], p[1]]) as usize;
    let protocol = u16::from_le_bytes([p[2], p[3]]) & 0x0FFF;
    if size < 36 || size > p.len() || protocol != 1024 {
        return None;
    }
    let target: [u8; 8] = p[8..16].try_into().ok()?;
    let kind = u16::from_le_bytes([p[32], p[33]]);
    Some((kind, target, &p[36..size]))
}

/// LIFX lights answering on `targets` (broadcast, or one address): name
/// and whether they have zones (strips, beams).
pub fn lifx(targets: &[SocketAddr], timeout: Duration) -> Vec<NetLight> {
    let Some(s) = socket(true) else {
        return Vec::new();
    };
    let hello = lifx_packet(LIFX_GET_SERVICE, None, 1, &[]);
    for t in targets {
        let _ = s.send_to(&hello, t);
    }
    // Who is there.
    let mut devices: Vec<(SocketAddr, [u8; 8])> = Vec::new();
    receive(&s, Instant::now() + timeout / 2, |data, from| {
        if let Some((LIFX_STATE_SERVICE, target, _)) = lifx_parse(data)
            && !devices.iter().any(|(a, _)| a.ip() == from.ip())
        {
            // Lights answer from their LIFX port (56700).
            devices.push((from, target));
        }
    });
    if devices.is_empty() {
        return Vec::new();
    }
    // What they are.
    for (addr, target) in &devices {
        for (seq, (kind, payload)) in [
            (LIFX_GET_LABEL, &[][..]),
            (LIFX_GET_COLOR_ZONES, &[0u8, 255][..]),
            (LIFX_GET_EXTENDED_COLOR_ZONES, &[][..]),
        ]
        .into_iter()
        .enumerate()
        {
            let _ = s.send_to(
                &lifx_packet(kind, Some(*target), seq as u8 + 2, payload),
                addr,
            );
        }
    }
    let mut lights: Vec<LifxLight> = devices
        .iter()
        .map(|(a, _)| LifxLight {
            host: a.ip().to_string(),
            name: String::new(),
            multizone: false,
            extended_multizone: false,
        })
        .collect();
    receive(
        &s,
        Instant::now() + (timeout / 2).max(Duration::from_millis(500)),
        |data, from| {
            let Some((kind, _, payload)) = lifx_parse(data) else {
                return;
            };
            let host = from.ip().to_string();
            let Some(l) = lights.iter_mut().find(|l| l.host == host) else {
                return;
            };
            match kind {
                LIFX_STATE_LABEL => {
                    let raw = &payload[..payload.len().min(32)];
                    let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
                    l.name = String::from_utf8_lossy(&raw[..end]).trim().to_owned();
                }
                LIFX_STATE_ZONE | LIFX_STATE_MULTI_ZONE => l.multizone = true,
                LIFX_STATE_EXTENDED_COLOR_ZONES => {
                    l.multizone = true;
                    l.extended_multizone = true;
                }
                _ => {}
            }
        },
    );
    lights
        .into_iter()
        .map(|mut l| {
            if l.name.is_empty() {
                l.name = "LIFX".into();
            }
            NetLight::Lifx(l)
        })
        .collect()
}

// ----------------------------------------------------------- Yeelight ---

/// Yeelight lights answering the search ("LAN Control" on in the Yeelight
/// app), colour ones only.
pub fn yeelight(timeout: Duration) -> Vec<NetLight> {
    let Some(s) = socket(false) else {
        return Vec::new();
    };
    let search = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1982\r\nMAN: \"ssdp:discover\"\r\nST: wifi_bulb\r\n\r\n";
    if let Err(e) = s.send_to(search.as_bytes(), (GOVEE_GROUP, YEELIGHT_SEARCH_PORT)) {
        tracing::debug!("Yeelight search: {e}");
    }
    let mut out: Vec<NetLight> = Vec::new();
    receive(&s, Instant::now() + timeout, |data, _| {
        if let Some(l) = yeelight_reply(&String::from_utf8_lossy(data))
            && !out.iter().any(|o| o.host() == l.host)
        {
            out.push(NetLight::Yeelight(l));
        }
    });
    out
}

/// A search reply (HTTP-like headers): its address from `Location`, its
/// name, and whether it takes colours (`support` lists `set_rgb`).
pub fn yeelight_reply(text: &str) -> Option<YeelightLight> {
    let header = |name: &str| {
        text.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_owned())
        })
    };
    let location = header("Location")?;
    let host = location
        .strip_prefix("yeelight://")?
        .split(':')
        .next()?
        .to_owned();
    host.parse::<IpAddr>().ok()?;
    if header("support").is_some_and(|s| !s.split_whitespace().any(|m| m == "set_rgb")) {
        return None;
    }
    let name = header("name").filter(|n| !n.is_empty()).unwrap_or_else(|| {
        match header("model").filter(|m| !m.is_empty()) {
            Some(m) => format!("Yeelight {m}"),
            None => "Yeelight".into(),
        }
    });
    Some(YeelightLight { host, name })
}

// --------------------------------------------------------------- Kasa ---

/// TP-Link's "autokey" XOR obfuscation (key 171), as used on port 9999.
pub fn kasa_encrypt(plain: &[u8]) -> Vec<u8> {
    let mut key = 171u8;
    plain
        .iter()
        .map(|b| {
            key ^= b;
            key
        })
        .collect()
}

pub fn kasa_decrypt(cipher: &[u8]) -> Vec<u8> {
    let mut key = 171u8;
    cipher
        .iter()
        .map(|c| {
            let b = key ^ c;
            key = *c;
            b
        })
        .collect()
}

/// TP-Link Kasa colour bulbs and strips answering on `targets`.
pub fn kasa(targets: &[SocketAddr], timeout: Duration) -> Vec<NetLight> {
    let Some(s) = socket(true) else {
        return Vec::new();
    };
    let ask = kasa_encrypt(br#"{"system":{"get_sysinfo":{}}}"#);
    for t in targets {
        let _ = s.send_to(&ask, t);
    }
    let mut out: Vec<NetLight> = Vec::new();
    receive(&s, Instant::now() + timeout, |data, from| {
        if let Some(l) = kasa_reply(&kasa_decrypt(data), from.ip())
            && !out.iter().any(|o| o.host() == l.host)
        {
            out.push(NetLight::Kasa(l));
        }
    });
    out
}

/// A `get_sysinfo` answer: colour lights only (not plugs, white bulbs).
pub fn kasa_reply(plain: &[u8], from: IpAddr) -> Option<KasaLight> {
    let v: Value = serde_json::from_slice(plain).ok()?;
    let info = v.pointer("/system/get_sysinfo")?;
    let color = match info.get("is_color") {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_u64() == Some(1),
        _ => false,
    };
    if !color {
        return None;
    }
    let text = |k: &str| {
        info.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    Some(KasaLight {
        host: from.to_string(),
        name: text("alias")
            .or_else(|| text("model"))
            .unwrap_or_else(|| "Kasa".into()),
    })
}

// --------------------------------------------- mDNS: WLED, Elgato, Hue, Nanoleaf ---

/// The lights announcing themselves over mDNS, each asked for what TuxRGB
/// needs (name, LED count, kind) in parallel.
fn mdns_lights(timeout: Duration) -> Vec<NetLight> {
    let services = mdns::browse(&[MDNS_WLED, MDNS_ELGATO, MDNS_HUE, MDNS_NANOLEAF], timeout);
    std::thread::scope(|s| {
        let handles: Vec<_> = services
            .iter()
            .map(|svc| s.spawn(move || from_mdns(svc, timeout)))
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok().flatten())
            .collect()
    })
}

fn from_mdns(svc: &mdns::Service, timeout: Duration) -> Option<NetLight> {
    let host = svc.ip.to_string();
    match svc.service.as_str() {
        MDNS_WLED => {
            let info = super::wled_info(&host, timeout).ok()?;
            Some(NetLight::Wled(WledLight {
                name: info.name,
                host,
                port: super::DDP_PORT,
                leds: info.leds,
            }))
        }
        MDNS_ELGATO => Some(NetLight::Elgato(
            elgato_info(&host, timeout).unwrap_or_else(|_| ElgatoLight {
                strip: svc.instance.to_ascii_lowercase().contains("strip"),
                name: if svc.instance.is_empty() {
                    "Elgato".into()
                } else {
                    svc.instance.clone()
                },
                host: host.clone(),
            }),
        )),
        MDNS_HUE => hue_config(&host, timeout).ok().map(NetLight::Hue),
        MDNS_NANOLEAF => Some(NetLight::Nanoleaf(NanoleafLight {
            host,
            port: svc.port.unwrap_or(NANOLEAF_PORT),
            token: String::new(),
            name: if svc.instance.is_empty() {
                "Nanoleaf".into()
            } else {
                svc.instance.clone()
            },
        })),
        _ => None,
    }
}

/// What an Elgato light says about itself (`/elgato/accessory-info`).
pub fn elgato_info(host: &str, timeout: Duration) -> Result<ElgatoLight, String> {
    let r = http::request(
        host,
        ELGATO_PORT,
        "GET",
        "/elgato/accessory-info",
        None,
        timeout,
    )?;
    let v = r
        .json()
        .filter(|_| r.ok())
        .ok_or_else(|| "pas une lumière Elgato".to_owned())?;
    let product = v
        .get("productName")
        .and_then(Value::as_str)
        .ok_or_else(|| "pas une lumière Elgato".to_owned())?;
    let display = v
        .get("displayName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    Ok(ElgatoLight {
        host: host.trim().to_owned(),
        name: display.unwrap_or(product).to_owned(),
        strip: product.to_ascii_lowercase().contains("strip"),
    })
}

/// A Philips Hue bridge's public configuration (`/api/config`): its MAC
/// address and name. Not paired yet.
pub fn hue_config(host: &str, timeout: Duration) -> Result<HueBridge, String> {
    let r = http::request(host, 80, "GET", "/api/config", None, timeout)?;
    let v = r
        .json()
        .filter(|_| r.ok())
        .ok_or_else(|| "pas un pont Philips Hue".to_owned())?;
    let mac = v
        .get("mac")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| "pas un pont Philips Hue".to_owned())?;
    Ok(HueBridge {
        host: host.trim().to_owned(),
        mac: mac.to_owned(),
        name: v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Philips Hue")
            .to_owned(),
        username: String::new(),
        clientkey: String::new(),
        entertainment: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every search at once, on whatever network the test runs: bounded.
    #[test]
    fn discovery_finishes_in_time() {
        let started = Instant::now();
        let _ = discover(Duration::from_millis(300));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn lifx_packets_round_trip() {
        let p = lifx_packet(
            LIFX_GET_COLOR_ZONES,
            Some([1, 2, 3, 4, 5, 6, 0, 0]),
            9,
            &[0, 255],
        );
        assert_eq!(p.len(), 38);
        // Addressed to one device: not tagged.
        assert_eq!(u16::from_le_bytes([p[2], p[3]]), 0x1400);
        assert_eq!(p[23], 9);
        let (kind, target, payload) = lifx_parse(&p).unwrap();
        assert_eq!(kind, LIFX_GET_COLOR_ZONES);
        assert_eq!(target, [1, 2, 3, 4, 5, 6, 0, 0]);
        assert_eq!(payload, &[0, 255]);
        let hello = lifx_packet(LIFX_GET_SERVICE, None, 1, &[]);
        assert_eq!(u16::from_le_bytes([hello[2], hello[3]]), 0x3400);
        assert!(lifx_parse(&hello[..20]).is_none());
    }

    /// A LIFX strip on the loopback: found, named, with extended zones.
    #[test]
    fn lifx_lights_are_discovered() {
        let device = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = device.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            device
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut buf = [0u8; 256];
            let mac = [0xd0, 0x73, 0xd5, 1, 2, 3, 0, 0];
            let mut answered = 0;
            while answered < 4 {
                let Ok((n, from)) = device.recv_from(&mut buf) else {
                    break;
                };
                let (kind, _, _) = lifx_parse(&buf[..n]).unwrap();
                let reply = match kind {
                    LIFX_GET_SERVICE => {
                        lifx_packet(LIFX_STATE_SERVICE, Some(mac), 0, &[1, 0x7c, 0xdd, 0, 0])
                    }
                    LIFX_GET_LABEL => {
                        let mut label = [0u8; 32];
                        label[..9].copy_from_slice(b"Bar shelf");
                        lifx_packet(LIFX_STATE_LABEL, Some(mac), 0, &label)
                    }
                    LIFX_GET_COLOR_ZONES => {
                        lifx_packet(LIFX_STATE_MULTI_ZONE, Some(mac), 0, &[16, 0])
                    }
                    LIFX_GET_EXTENDED_COLOR_ZONES => {
                        lifx_packet(LIFX_STATE_EXTENDED_COLOR_ZONES, Some(mac), 0, &[16, 0])
                    }
                    _ => continue,
                };
                device.send_to(&reply, from).unwrap();
                answered += 1;
            }
        });
        // The real port is fixed: the reply's source is what counts.
        let found = lifx(&[addr], Duration::from_millis(800));
        t.join().unwrap();
        assert_eq!(found.len(), 1);
        let NetLight::Lifx(l) = &found[0] else {
            panic!("not LIFX")
        };
        assert_eq!(l.host, "127.0.0.1");
        assert_eq!(l.name, "Bar shelf");
        assert!(l.multizone && l.extended_multizone);
    }

    #[test]
    fn govee_replies_are_read() {
        let reply = br#"{"msg":{"cmd":"scan","data":{"ip":"192.168.1.23","device":"1F:80:C5:32:32:36:72:4E","sku":"H6199","bleVersionHard":"3.01.01","wifiVersionSoft":"1.02.11"}}}"#;
        let l = govee_reply(reply, "192.168.1.9".parse().unwrap()).unwrap();
        assert_eq!(l.host, "192.168.1.23");
        assert_eq!(l.model, "H6199");
        assert!(
            govee_reply(
                br#"{"msg":{"cmd":"devStatus","data":{}}}"#,
                l.host.parse().unwrap()
            )
            .is_none()
        );
    }

    #[test]
    fn yeelight_replies_are_read() {
        let reply = "HTTP/1.1 200 OK\r\nCache-Control: max-age=3600\r\nLocation: yeelight://192.168.1.239:55443\r\nServer: POSIX UPnP/1.0 YGLC/1\r\nid: 0x000000000015243f\r\nmodel: color\r\nfw_ver: 18\r\nsupport: get_prop set_default set_power toggle set_bright start_cf stop_cf set_scene cron_add cron_get cron_del set_ct_abx set_rgb\r\npower: on\r\nname: Salon\r\n";
        let l = yeelight_reply(reply).unwrap();
        assert_eq!(l.host, "192.168.1.239");
        assert_eq!(l.name, "Salon");
        // Unnamed: the model.
        let l = yeelight_reply(&reply.replace("name: Salon", "name: ")).unwrap();
        assert_eq!(l.name, "Yeelight color");
        // White only: no.
        assert!(yeelight_reply(&reply.replace(" set_rgb", "")).is_none());
        assert!(yeelight_reply("HTTP/1.1 200 OK\r\n").is_none());
    }

    #[test]
    fn kasa_obfuscation_round_trips() {
        let plain = br#"{"system":{"get_sysinfo":{}}}"#;
        let c = kasa_encrypt(plain);
        assert_eq!(c[0], 171 ^ b'{');
        assert_eq!(kasa_decrypt(&c), plain);
        let from: IpAddr = "192.168.1.50".parse().unwrap();
        let bulb = br#"{"system":{"get_sysinfo":{"alias":"Desk lamp","model":"KL130(EU)","mic_type":"IOT.SMARTBULB","is_color":1}}}"#;
        let l = kasa_reply(bulb, from).unwrap();
        assert_eq!(l.name, "Desk lamp");
        assert_eq!(l.host, "192.168.1.50");
        let plug = br#"{"system":{"get_sysinfo":{"alias":"Plug","model":"HS100(EU)","type":"IOT.SMARTPLUGSWITCH"}}}"#;
        assert!(kasa_reply(plug, from).is_none());
    }

    #[test]
    fn kasa_lights_are_discovered() {
        let bulb = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = bulb.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            let (n, from) = bulb.recv_from(&mut buf).unwrap();
            assert_eq!(kasa_decrypt(&buf[..n]), br#"{"system":{"get_sysinfo":{}}}"#);
            let reply =
                kasa_encrypt(br#"{"system":{"get_sysinfo":{"alias":"Strip","is_color":1}}}"#);
            bulb.send_to(&reply, from).unwrap();
        });
        let found = kasa(&[addr], Duration::from_millis(600));
        t.join().unwrap();
        assert_eq!(
            found,
            vec![NetLight::Kasa(KasaLight {
                host: "127.0.0.1".into(),
                name: "Strip".into()
            })]
        );
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
        let found = wiz(&[addr], Duration::from_millis(800));
        t.join().unwrap();
        assert_eq!(
            found,
            vec![NetLight::Wiz(WizLight {
                host: "127.0.0.1".into(),
                warm_white: false
            })]
        );
    }

    #[test]
    fn elgato_and_hue_are_asked_what_they_are() {
        use http::tests::{json_response, serve_once};
        let (port, server) = serve_once(json_response(
            "200 OK",
            r#"{"productName":"Elgato Light Strip","hardwareBoardType":70,"firmwareVersion":"1.0.4","serialNumber":"EW00","displayName":""}"#,
        ));
        let l = elgato_info(&format!("127.0.0.1:{port}"), Duration::from_secs(2)).unwrap();
        assert!(
            server
                .join()
                .unwrap()
                .starts_with("GET /elgato/accessory-info ")
        );
        assert!(l.strip);
        assert_eq!(l.name, "Elgato Light Strip");
        let (port, server) = serve_once(json_response(
            "200 OK",
            r#"{"name":"Hue Bridge","datastoreversion":"163","swversion":"1967054020","apiversion":"1.67.0","mac":"ec:b5:fa:12:34:56","bridgeid":"ECB5FAFFFE123456","factorynew":false,"modelid":"BSB002"}"#,
        ));
        let b = hue_config(&format!("127.0.0.1:{port}"), Duration::from_secs(2)).unwrap();
        server.join().unwrap();
        assert_eq!(b.mac, "ec:b5:fa:12:34:56");
        assert_eq!(b.name, "Hue Bridge");
        assert!(b.username.is_empty());
    }
}
