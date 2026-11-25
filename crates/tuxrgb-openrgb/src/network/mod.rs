//! Network lights: WLED (DDP) and Philips WiZ since TuxRGB 3.0; Govee,
//! LIFX, Yeelight, Nanoleaf, Philips Hue, Elgato and TP-Link Kasa since
//! 3.4.
//!
//! OpenRGB 1.1 drives them all natively from its own settings file. TuxRGB
//! writes the lights the user added into the private server's
//! `OpenRGB.json` before starting it (everything else in the file is kept),
//! and they appear as ordinary devices: on the canvas, in every effect and
//! in the blackout of a shutdown. A WiZ bulb's own scenes ("Ocean",
//! "Fireplace"…) are its hardware effects.
//!
//! Helpers for the settings page: one search for every brand at once
//! ([`discover`]), the check of an address typed in ([`probe`]), and the
//! pairing Nanoleaf controllers and Hue bridges ask for ([`pair`]).

mod discovery;
mod http;
mod mdns;

pub use discovery::{ELGATO_PORT, NANOLEAF_PORT, discover};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

/// DDP port of WLED (and of OpenRGB's DDP controller).
pub const DDP_PORT: u16 = 4048;
/// UDP port of Philips WiZ bulbs (fixed).
pub const WIZ_PORT: u16 = discovery::WIZ_PORT;

/// The brands TuxRGB can add, in the order they are offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Brand {
    Wled,
    Hue,
    Wiz,
    Govee,
    Lifx,
    Nanoleaf,
    Yeelight,
    Elgato,
    Kasa,
}

impl Brand {
    pub const ALL: [Brand; 9] = [
        Brand::Wled,
        Brand::Hue,
        Brand::Wiz,
        Brand::Govee,
        Brand::Lifx,
        Brand::Nanoleaf,
        Brand::Yeelight,
        Brand::Elgato,
        Brand::Kasa,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Brand::Wled => "WLED",
            Brand::Hue => "Philips Hue",
            Brand::Wiz => "Philips WiZ",
            Brand::Govee => "Govee",
            Brand::Lifx => "LIFX",
            Brand::Nanoleaf => "Nanoleaf",
            Brand::Yeelight => "Yeelight",
            Brand::Elgato => "Elgato",
            Brand::Kasa => "TP-Link Kasa",
        }
    }

    /// A token must be obtained from the device first (button pressed).
    pub fn needs_pairing(self) -> bool {
        matches!(self, Brand::Nanoleaf | Brand::Hue)
    }
}

impl std::fmt::Display for Brand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// The network lights the user added.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkLights {
    pub wled: Vec<WledLight>,
    pub wiz: Vec<WizLight>,
    pub govee: Vec<GoveeLight>,
    pub lifx: Vec<LifxLight>,
    pub yeelight: Vec<YeelightLight>,
    pub nanoleaf: Vec<NanoleafLight>,
    /// OpenRGB drives one Hue bridge (with every light behind it).
    pub hue: Option<HueBridge>,
    pub elgato: Vec<ElgatoLight>,
    pub kasa: Vec<KasaLight>,
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

/// A Govee light with "LAN Control" on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoveeLight {
    pub host: String,
    /// Its model (SKU, `H6199`), when known.
    #[serde(default)]
    pub model: String,
}

/// A LIFX bulb, strip or beam.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifxLight {
    pub host: String,
    pub name: String,
    /// Several zones (LIFX Z, Beam, Neon…).
    #[serde(default)]
    pub multizone: bool,
    /// Zones written in one message (recent firmware).
    #[serde(default)]
    pub extended_multizone: bool,
}

/// A Yeelight colour bulb or strip with "LAN Control" on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct YeelightLight {
    pub host: String,
    #[serde(default)]
    pub name: String,
}

/// A Nanoleaf controller (panels, shapes, lines…).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NanoleafLight {
    pub host: String,
    #[serde(default = "nanoleaf_port")]
    pub port: u16,
    /// Given by the controller when paired (empty: not paired yet).
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub name: String,
}

fn nanoleaf_port() -> u16 {
    NANOLEAF_PORT
}

/// A Philips Hue bridge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HueBridge {
    pub host: String,
    pub mac: String,
    #[serde(default)]
    pub name: String,
    /// Given by the bridge when paired (empty: not paired yet).
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub clientkey: String,
    /// Entertainment areas (set up in the Hue app) instead of the lights
    /// one by one: much faster, made for effects.
    #[serde(default)]
    pub entertainment: bool,
}

/// An Elgato Key Light or Light Strip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElgatoLight {
    pub host: String,
    #[serde(default)]
    pub name: String,
    /// A Light Strip (colours) rather than a Key Light (white).
    #[serde(default)]
    pub strip: bool,
}

/// A TP-Link Kasa colour bulb or light strip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KasaLight {
    pub host: String,
    #[serde(default)]
    pub name: String,
}

/// One light of any brand: what a search finds, what the settings list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetLight {
    Wled(WledLight),
    Wiz(WizLight),
    Govee(GoveeLight),
    Lifx(LifxLight),
    Yeelight(YeelightLight),
    Nanoleaf(NanoleafLight),
    Hue(HueBridge),
    Elgato(ElgatoLight),
    Kasa(KasaLight),
}

impl NetLight {
    pub fn brand(&self) -> Brand {
        match self {
            NetLight::Wled(_) => Brand::Wled,
            NetLight::Wiz(_) => Brand::Wiz,
            NetLight::Govee(_) => Brand::Govee,
            NetLight::Lifx(_) => Brand::Lifx,
            NetLight::Yeelight(_) => Brand::Yeelight,
            NetLight::Nanoleaf(_) => Brand::Nanoleaf,
            NetLight::Hue(_) => Brand::Hue,
            NetLight::Elgato(_) => Brand::Elgato,
            NetLight::Kasa(_) => Brand::Kasa,
        }
    }

    pub fn host(&self) -> &str {
        match self {
            NetLight::Wled(l) => &l.host,
            NetLight::Wiz(l) => &l.host,
            NetLight::Govee(l) => &l.host,
            NetLight::Lifx(l) => &l.host,
            NetLight::Yeelight(l) => &l.host,
            NetLight::Nanoleaf(l) => &l.host,
            NetLight::Hue(l) => &l.host,
            NetLight::Elgato(l) => &l.host,
            NetLight::Kasa(l) => &l.host,
        }
    }

    /// Its own name, or its brand (and model) when it has none.
    pub fn name(&self) -> String {
        let own = match self {
            NetLight::Wled(l) => l.name.as_str(),
            NetLight::Lifx(l) => l.name.as_str(),
            NetLight::Yeelight(l) => l.name.as_str(),
            NetLight::Nanoleaf(l) => l.name.as_str(),
            NetLight::Hue(l) => l.name.as_str(),
            NetLight::Elgato(l) => l.name.as_str(),
            NetLight::Kasa(l) => l.name.as_str(),
            NetLight::Govee(l) if !l.model.is_empty() => return format!("Govee {}", l.model),
            NetLight::Govee(_) | NetLight::Wiz(_) => "",
        };
        if own.trim().is_empty() {
            self.brand().label().to_owned()
        } else {
            own.trim().to_owned()
        }
    }

    /// Still to be paired (Nanoleaf, Hue) before OpenRGB can use it.
    pub fn needs_pairing(&self) -> bool {
        match self {
            NetLight::Nanoleaf(l) => l.token.is_empty(),
            NetLight::Hue(b) => b.username.is_empty(),
            _ => false,
        }
    }
}

fn same_host(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

impl NetworkLights {
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.wled.len()
            + self.wiz.len()
            + self.govee.len()
            + self.lifx.len()
            + self.yeelight.len()
            + self.nanoleaf.len()
            + usize::from(self.hue.is_some())
            + self.elgato.len()
            + self.kasa.len()
    }

    /// Every light, brand by brand.
    pub fn lights(&self) -> Vec<NetLight> {
        let mut out = Vec::with_capacity(self.len());
        out.extend(self.wled.iter().cloned().map(NetLight::Wled));
        out.extend(self.hue.iter().cloned().map(NetLight::Hue));
        out.extend(self.wiz.iter().cloned().map(NetLight::Wiz));
        out.extend(self.govee.iter().cloned().map(NetLight::Govee));
        out.extend(self.lifx.iter().cloned().map(NetLight::Lifx));
        out.extend(self.nanoleaf.iter().cloned().map(NetLight::Nanoleaf));
        out.extend(self.yeelight.iter().cloned().map(NetLight::Yeelight));
        out.extend(self.elgato.iter().cloned().map(NetLight::Elgato));
        out.extend(self.kasa.iter().cloned().map(NetLight::Kasa));
        out
    }

    /// A light of `brand` at `host` is configured.
    pub fn contains(&self, brand: Brand, host: &str) -> bool {
        self.lights()
            .iter()
            .any(|l| l.brand() == brand && same_host(l.host(), host))
    }

    /// Adds a light, replacing the one of the same brand at the same
    /// address (a Hue bridge replaces the bridge: OpenRGB drives one).
    pub fn add(&mut self, light: NetLight) {
        self.remove(light.brand(), light.host());
        match light {
            NetLight::Wled(l) => self.wled.push(l),
            NetLight::Wiz(l) => self.wiz.push(l),
            NetLight::Govee(l) => self.govee.push(l),
            NetLight::Lifx(l) => self.lifx.push(l),
            NetLight::Yeelight(l) => self.yeelight.push(l),
            NetLight::Nanoleaf(l) => self.nanoleaf.push(l),
            NetLight::Hue(b) => self.hue = Some(b),
            NetLight::Elgato(l) => self.elgato.push(l),
            NetLight::Kasa(l) => self.kasa.push(l),
        }
    }

    /// Removes the light of `brand` at `host`; `true` if there was one.
    pub fn remove(&mut self, brand: Brand, host: &str) -> bool {
        let before = self.len();
        match brand {
            Brand::Wled => self.wled.retain(|l| !same_host(&l.host, host)),
            Brand::Wiz => self.wiz.retain(|l| !same_host(&l.host, host)),
            Brand::Govee => self.govee.retain(|l| !same_host(&l.host, host)),
            Brand::Lifx => self.lifx.retain(|l| !same_host(&l.host, host)),
            Brand::Yeelight => self.yeelight.retain(|l| !same_host(&l.host, host)),
            Brand::Nanoleaf => self.nanoleaf.retain(|l| !same_host(&l.host, host)),
            Brand::Hue => {
                if self.hue.as_ref().is_some_and(|b| same_host(&b.host, host)) {
                    self.hue = None;
                }
            }
            Brand::Elgato => self.elgato.retain(|l| !same_host(&l.host, host)),
            Brand::Kasa => self.kasa.retain(|l| !same_host(&l.host, host)),
        }
        self.len() != before
    }
}

/// A host as typed by the user: an IP address or a host name, no spaces,
/// no scheme or path.
pub fn valid_host(host: &str) -> bool {
    let h = host.trim();
    !h.is_empty()
        && h.len() <= 253
        && h.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_' | '[' | ']'))
}

/// Writes OpenRGB's settings before the private server starts: the network
/// lights (`DDPDevices`, `PhilipsWizDevices`, `GoveeDevices`…) and the
/// SMBus detectors `smbus` allows (see [`crate::detectors`]), keeping
/// everything else. `Ok(true)` when the file
/// changed. A file OpenRGB cannot have written (not JSON) is kept aside as
/// `OpenRGB.json.bak` and started again.
pub fn write_settings(
    config_dir: &Path,
    lights: &NetworkLights,
    smbus: crate::detectors::SmbusLighting,
) -> std::io::Result<bool> {
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
    let obj = root.as_object_mut().expect("checked above");
    for (key, section) in sections(lights) {
        obj.insert(key.into(), section);
    }
    crate::detectors::apply(obj, smbus);
    if root == before && path.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(config_dir)?;
    let tmp = config_dir.join("OpenRGB.json.tuxrgb-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&root)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(true)
}

/// OpenRGB's settings sections for the lights, every brand always written
/// (an empty list removes what was there). Lights still to be paired are
/// left out: OpenRGB would wait for a button press during detection.
fn sections(lights: &NetworkLights) -> Vec<(&'static str, Value)> {
    let ok = |host: &str| valid_host(host);
    let list = |items: Vec<Value>| json!({ "devices": items });
    let wled = lights
        .wled
        .iter()
        .filter(|l| ok(&l.host) && l.leds > 0)
        .map(|l| {
            json!({
                "name": l.name,
                "ip": l.host.trim(),
                "port": l.port,
                "num_leds": l.leds,
            })
        })
        .collect();
    let wiz = lights
        .wiz
        .iter()
        .filter(|l| ok(&l.host))
        .map(|l| {
            json!({
                "ip": l.host.trim(),
                "use_cool_white": false,
                "use_warm_white": l.warm_white,
                "selected_white_strategy": if l.warm_white { "Minimum" } else { "Average" },
            })
        })
        .collect();
    let govee = lights
        .govee
        .iter()
        .filter(|l| ok(&l.host))
        .map(|l| json!({ "ip": l.host.trim() }))
        .collect();
    // Every field is read without a check by OpenRGB: all of them, always.
    let lifx = lights
        .lifx
        .iter()
        .filter(|l| ok(&l.host))
        .map(|l| {
            json!({
                "ip": l.host.trim(),
                "name": if l.name.trim().is_empty() { "LIFX" } else { l.name.trim() },
                "multizone": l.multizone || l.extended_multizone,
                "extended_multizone": l.extended_multizone,
            })
        })
        .collect();
    // No music mode: OpenRGB would wait, without a time limit, for the
    // light to connect back to this computer (blocked by a firewall).
    let yeelight = lights
        .yeelight
        .iter()
        .filter(|l| ok(&l.host))
        .map(|l| json!({ "ip": l.host.trim(), "music_mode": false }))
        .collect();
    let mut nanoleaf = Map::new();
    for l in lights
        .nanoleaf
        .iter()
        .filter(|l| ok(&l.host) && !l.token.is_empty())
    {
        nanoleaf.insert(
            format!("{}:{}", l.host.trim(), l.port),
            json!({ "ip": l.host.trim(), "port": l.port, "auth_token": l.token }),
        );
    }
    let hue: Vec<Value> = lights
        .hue
        .iter()
        .filter(|b| ok(&b.host) && !b.username.is_empty() && !b.mac.is_empty())
        .map(|b| {
            json!({
                "ip": b.host.trim(),
                "mac": b.mac,
                "username": b.username,
                "clientkey": b.clientkey,
                "entertainment": b.entertainment,
                "autoconnect": false,
            })
        })
        .collect();
    let elgato = |strip: bool| {
        lights
            .elgato
            .iter()
            .filter(|l| ok(&l.host) && l.strip == strip)
            .map(|l| json!({ "ip": l.host.trim() }))
            .collect::<Vec<_>>()
    };
    let kasa = lights
        .kasa
        .iter()
        .filter(|l| ok(&l.host))
        .map(|l| {
            json!({
                "ip": l.host.trim(),
                "name": if l.name.trim().is_empty() { "Kasa" } else { l.name.trim() },
            })
        })
        .collect();
    vec![
        ("DDPDevices", list(wled)),
        ("PhilipsWizDevices", list(wiz)),
        ("GoveeDevices", list(govee)),
        ("LIFXDevices", list(lifx)),
        ("YeelightDevices", list(yeelight)),
        ("NanoleafDevices", json!({ "devices": nanoleaf })),
        ("PhilipsHueDevices", json!({ "bridges": hue })),
        ("ElgatoKeyLightDevices", list(elgato(false))),
        ("ElgatoLightStripDevices", list(elgato(true))),
        ("KasaSmartDevices", list(kasa)),
    ]
}

/// Lights that cannot take colours as fast as the engine sends them: the
/// shortest time between two updates, by OpenRGB controller description.
/// Their colours are held back meanwhile and the latest is sent when due.
pub fn min_interval(description: &str) -> Option<Duration> {
    let ms = match description {
        // 60 commands a minute without music mode.
        "Yeelight Device" => 1100,
        // The bridge's REST interface: about ten commands a second.
        "Philips Hue Device" => 200,
        // One HTTP request or TCP connection per update.
        "Elgato KeyLight Device" | "Elgato LightStrip Device" | "Kasa Smart Device" => 100,
        // At most 20 messages a second.
        "LIFX Device" | "Philips Wiz Device" => 50,
        _ => return None,
    };
    Some(Duration::from_millis(ms))
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
    let r = http::request(host, 80, "GET", "/json/info", None, timeout)?;
    if !r.ok() {
        return Err(format!("HTTP {}", r.status));
    }
    let info = r
        .json()
        .ok_or_else(|| "pas un contrôleur WLED".to_owned())?;
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

/// Checks the address typed in for a light of `brand` and returns it ready
/// to add (Nanoleaf, Hue: ready to pair). `leds`: the LED count of a WLED
/// controller, used when it does not answer.
pub fn probe(
    brand: Brand,
    host: &str,
    leds: Option<usize>,
    timeout: Duration,
) -> Result<NetLight, String> {
    let host = host.trim();
    if !valid_host(host) {
        return Err("adresse invalide".into());
    }
    let addr = |port: u16| http::target(host, port).map(|(a, _)| a);
    let one = |found: Vec<NetLight>, missing: &str| {
        found
            .into_iter()
            .next()
            .map(|mut l| {
                // Keep the address as typed (a host name stays a name).
                set_host(&mut l, host);
                l
            })
            .ok_or_else(|| missing.to_owned())
    };
    match brand {
        Brand::Wled => match wled_info(host, timeout) {
            Ok(info) => Ok(NetLight::Wled(WledLight {
                name: info.name,
                host: host.to_owned(),
                port: DDP_PORT,
                leds: info.leds,
            })),
            Err(_) if leds.is_some_and(|n| n > 0) => Ok(NetLight::Wled(WledLight {
                name: format!("WLED {host}"),
                host: host.to_owned(),
                port: DDP_PORT,
                leds: leds.unwrap_or_default(),
            })),
            Err(e) => Err(e),
        },
        Brand::Wiz => one(
            discovery::wiz(&[addr(WIZ_PORT)?], timeout),
            "aucune ampoule WiZ ne répond à cette adresse",
        ),
        Brand::Govee => one(
            discovery::govee(Some(addr(discovery::GOVEE_SCAN_PORT)?.ip()), timeout),
            "aucune réponse : activez « LAN Control » dans l'application Govee Home",
        ),
        Brand::Lifx => one(
            discovery::lifx(&[addr(discovery::LIFX_PORT)?], timeout),
            "aucune lumière LIFX ne répond à cette adresse",
        ),
        Brand::Kasa => one(
            discovery::kasa(&[addr(discovery::KASA_PORT)?], timeout),
            "aucune lumière Kasa en couleur ne répond à cette adresse",
        ),
        Brand::Yeelight => {
            reachable(addr(discovery::YEELIGHT_PORT)?, timeout).map_err(|_| {
                "aucune réponse : activez « Contrôle LAN » dans l'application Yeelight".to_owned()
            })?;
            Ok(NetLight::Yeelight(YeelightLight {
                host: host.to_owned(),
                name: "Yeelight".into(),
            }))
        }
        Brand::Nanoleaf => {
            let target = addr(NANOLEAF_PORT)?;
            reachable(target, timeout)
                .map_err(|_| "aucun contrôleur Nanoleaf ne répond à cette adresse".to_owned())?;
            Ok(NetLight::Nanoleaf(NanoleafLight {
                host: host.split(':').next().unwrap_or(host).to_owned(),
                port: target.port(),
                token: String::new(),
                name: "Nanoleaf".into(),
            }))
        }
        Brand::Hue => discovery::hue_config(host, timeout).map(NetLight::Hue),
        Brand::Elgato => discovery::elgato_info(host, timeout).map(NetLight::Elgato),
    }
}

fn set_host(light: &mut NetLight, host: &str) {
    let h = host.to_owned();
    match light {
        NetLight::Wled(l) => l.host = h,
        NetLight::Wiz(l) => l.host = h,
        NetLight::Govee(l) => l.host = h,
        NetLight::Lifx(l) => l.host = h,
        NetLight::Yeelight(l) => l.host = h,
        NetLight::Nanoleaf(l) => l.host = h,
        NetLight::Hue(l) => l.host = h,
        NetLight::Elgato(l) => l.host = h,
        NetLight::Kasa(l) => l.host = h,
    }
}

fn reachable(addr: SocketAddr, timeout: Duration) -> std::io::Result<()> {
    TcpStream::connect_timeout(&addr, timeout).map(drop)
}

/// Pairs a Nanoleaf controller or a Hue bridge: the user pressed its
/// button just before. Returns the light with its token; other lights come
/// back unchanged.
pub fn pair(light: NetLight, timeout: Duration) -> Result<NetLight, String> {
    match light {
        NetLight::Nanoleaf(mut l) => {
            let host = format!("{}:{}", l.host.trim(), l.port);
            let r = http::request(&host, l.port, "POST", "/api/v1/new", Some(""), timeout)?;
            if r.status == 403 || r.status == 401 {
                return Err(
                    "le contrôleur n'est pas prêt : maintenez son bouton marche 5 à 7 secondes, \
                     jusqu'à ce que ses voyants clignotent, puis associez-le dans les 30 secondes"
                        .into(),
                );
            }
            let token = r
                .json()
                .filter(|_| r.ok())
                .and_then(|v| {
                    v.get("auth_token")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .filter(|t| !t.is_empty())
                .ok_or_else(|| format!("réponse inattendue (HTTP {})", r.status))?;
            // Its name, for the list (not needed by OpenRGB).
            if let Ok(info) = http::request(
                &host,
                l.port,
                "GET",
                &format!("/api/v1/{token}/"),
                None,
                timeout,
            ) && let Some(name) = info
                .json()
                .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_owned))
                .filter(|n| !n.trim().is_empty())
            {
                l.name = name;
            }
            l.token = token;
            Ok(NetLight::Nanoleaf(l))
        }
        NetLight::Hue(mut b) => {
            if b.mac.is_empty() {
                b.mac = discovery::hue_config(&b.host, timeout)?.mac;
            }
            let r = http::request(
                &b.host,
                80,
                "POST",
                "/api",
                Some(r#"{"devicetype":"tuxrgb#pc","generateclientkey":true}"#),
                timeout,
            )?;
            let v = r
                .json()
                .ok_or_else(|| format!("réponse inattendue (HTTP {})", r.status))?;
            let first = v.get(0).cloned().unwrap_or(Value::Null);
            if let Some(err) = first.get("error") {
                return Err(if err.get("type").and_then(Value::as_u64) == Some(101) {
                    "appuyez sur le bouton rond du pont Hue, puis associez-le dans les 30 secondes"
                        .into()
                } else {
                    err.get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("refusé par le pont")
                        .to_owned()
                });
            }
            let ok = first
                .get("success")
                .ok_or_else(|| "réponse inattendue du pont".to_owned())?;
            b.username = ok
                .get("username")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            b.clientkey = ok
                .get("clientkey")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if b.username.is_empty() {
                return Err("réponse inattendue du pont".into());
            }
            Ok(NetLight::Hue(b))
        }
        other => Ok(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detectors::SmbusLighting;
    use std::io::Read;

    const RAM_AND_BOARD: SmbusLighting = SmbusLighting {
        ram: true,
        motherboard: true,
    };

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("tuxrgb-net-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn read(dir: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join("OpenRGB.json")).unwrap()).unwrap()
    }

    fn every_brand() -> NetworkLights {
        let mut n = NetworkLights::default();
        n.add(NetLight::Wled(WledLight {
            name: "Bureau".into(),
            host: "192.168.1.40".into(),
            port: DDP_PORT,
            leds: 60,
        }));
        n.add(NetLight::Wiz(WizLight {
            host: "192.168.1.41".into(),
            warm_white: true,
        }));
        n.add(NetLight::Govee(GoveeLight {
            host: "192.168.1.42".into(),
            model: "H6199".into(),
        }));
        n.add(NetLight::Lifx(LifxLight {
            host: "192.168.1.43".into(),
            name: "Beam".into(),
            multizone: false,
            extended_multizone: true,
        }));
        n.add(NetLight::Yeelight(YeelightLight {
            host: "192.168.1.44".into(),
            name: String::new(),
        }));
        n.add(NetLight::Nanoleaf(NanoleafLight {
            host: "192.168.1.45".into(),
            port: NANOLEAF_PORT,
            token: "tok".into(),
            name: "Shapes".into(),
        }));
        // Not paired: left out of OpenRGB's settings.
        n.add(NetLight::Nanoleaf(NanoleafLight {
            host: "192.168.1.46".into(),
            port: NANOLEAF_PORT,
            token: String::new(),
            name: String::new(),
        }));
        n.add(NetLight::Hue(HueBridge {
            host: "192.168.1.47".into(),
            mac: "ec:b5:fa:12:34:56".into(),
            name: "Hue".into(),
            username: "user".into(),
            clientkey: "key".into(),
            entertainment: true,
        }));
        n.add(NetLight::Elgato(ElgatoLight {
            host: "192.168.1.48".into(),
            name: "Key Light".into(),
            strip: false,
        }));
        n.add(NetLight::Elgato(ElgatoLight {
            host: "192.168.1.49".into(),
            name: "Light Strip".into(),
            strip: true,
        }));
        n.add(NetLight::Kasa(KasaLight {
            host: "192.168.1.50".into(),
            name: String::new(),
        }));
        n
    }

    #[test]
    fn settings_are_merged_into_openrgb_json() {
        let dir = tmp("merge");
        std::fs::write(
            dir.join("OpenRGB.json"),
            r#"{ "Detectors": { "detectors": { "Logitech G502": false } }, "DDPDevices": { "devices": [] } }"#,
        )
        .unwrap();
        let lights = every_brand();
        assert!(write_settings(&dir, &lights, SmbusLighting::OFF).unwrap());
        let v = read(&dir);
        // OpenRGB's own settings are kept.
        assert_eq!(
            v.pointer("/Detectors/detectors/Logitech G502"),
            Some(&json!(false))
        );
        // Prudence: the memory bus is left alone.
        assert_eq!(
            v.pointer("/Detectors/detectors/ENE SMBus DRAM"),
            Some(&json!(false))
        );
        let at = |p: &str| v.pointer(p).cloned().unwrap_or(Value::Null);
        assert_eq!(at("/DDPDevices/devices/0/ip"), json!("192.168.1.40"));
        assert_eq!(at("/DDPDevices/devices/0/num_leds"), json!(60));
        assert_eq!(at("/DDPDevices/devices/0/port"), json!(4048));
        assert_eq!(
            at("/PhilipsWizDevices/devices/0/use_warm_white"),
            json!(true)
        );
        assert_eq!(
            at("/GoveeDevices/devices/0"),
            json!({ "ip": "192.168.1.42" })
        );
        assert_eq!(
            at("/LIFXDevices/devices/0"),
            json!({ "ip": "192.168.1.43", "name": "Beam", "multizone": true, "extended_multizone": true })
        );
        assert_eq!(
            at("/YeelightDevices/devices/0"),
            json!({ "ip": "192.168.1.44", "music_mode": false })
        );
        assert_eq!(
            at("/NanoleafDevices/devices"),
            json!({ "192.168.1.45:16021": { "ip": "192.168.1.45", "port": 16021, "auth_token": "tok" } })
        );
        assert_eq!(at("/PhilipsHueDevices/bridges/0/username"), json!("user"));
        assert_eq!(
            at("/PhilipsHueDevices/bridges/0/entertainment"),
            json!(true)
        );
        assert_eq!(
            at("/ElgatoKeyLightDevices/devices"),
            json!([{ "ip": "192.168.1.48" }])
        );
        assert_eq!(
            at("/ElgatoLightStripDevices/devices"),
            json!([{ "ip": "192.168.1.49" }])
        );
        assert_eq!(
            at("/KasaSmartDevices/devices/0"),
            json!({ "ip": "192.168.1.50", "name": "Kasa" })
        );
        // Nothing new: not rewritten.
        assert!(!write_settings(&dir, &lights, SmbusLighting::OFF).unwrap());
        // The memory bus allowed: rewritten.
        assert!(write_settings(&dir, &lights, RAM_AND_BOARD).unwrap());
        assert_eq!(
            read(&dir).pointer("/Detectors/detectors/ENE SMBus DRAM"),
            Some(&json!(true))
        );
        // Removed again.
        assert!(write_settings(&dir, &NetworkLights::default(), RAM_AND_BOARD).unwrap());
        let v = read(&dir);
        assert_eq!(v.pointer("/DDPDevices/devices"), Some(&json!([])));
        assert_eq!(v.pointer("/PhilipsHueDevices/bridges"), Some(&json!([])));
        assert_eq!(v.pointer("/NanoleafDevices/devices"), Some(&json!({})));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lights_are_added_replaced_and_removed() {
        let mut n = every_brand();
        assert_eq!(n.len(), 11);
        assert_eq!(n.lights().len(), 11);
        assert!(n.contains(Brand::Kasa, " 192.168.1.50 "));
        assert!(!n.contains(Brand::Lifx, "192.168.1.50"));
        // Same brand and address: replaced.
        n.add(NetLight::Kasa(KasaLight {
            host: "192.168.1.50".into(),
            name: "Lamp".into(),
        }));
        assert_eq!(n.kasa.len(), 1);
        assert_eq!(n.kasa[0].name, "Lamp");
        // A second Hue bridge replaces the first.
        n.add(NetLight::Hue(HueBridge {
            host: "192.168.1.60".into(),
            mac: "x".into(),
            name: String::new(),
            username: String::new(),
            clientkey: String::new(),
            entertainment: false,
        }));
        assert_eq!(n.hue.as_ref().unwrap().host, "192.168.1.60");
        assert!(n.remove(Brand::Hue, "192.168.1.60"));
        assert!(!n.remove(Brand::Hue, "192.168.1.60"));
        assert!(n.remove(Brand::Wled, "192.168.1.40"));
        assert_eq!(n.len(), 9);
        // Names: their own, else the brand (and model).
        assert_eq!(
            NetLight::Govee(GoveeLight {
                host: "h".into(),
                model: "H6199".into()
            })
            .name(),
            "Govee H6199"
        );
        assert_eq!(
            NetLight::Yeelight(YeelightLight {
                host: "h".into(),
                name: " ".into()
            })
            .name(),
            "Yeelight"
        );
    }

    #[test]
    fn older_configurations_still_load() {
        let old = r#"{"wled":[{"name":"Bureau","host":"192.168.1.40","leds":60}],"wiz":[{"host":"192.168.1.41"}]}"#;
        let n: NetworkLights = serde_json::from_str(old).unwrap();
        assert_eq!(n.wled[0].port, DDP_PORT);
        assert_eq!(n.wiz.len(), 1);
        assert!(n.govee.is_empty() && n.hue.is_none());
        let back: NetworkLights =
            serde_json::from_str(&serde_json::to_string(&every_brand()).unwrap()).unwrap();
        assert_eq!(back, every_brand());
    }

    #[test]
    fn broken_settings_are_kept_aside() {
        let dir = tmp("broken");
        std::fs::write(dir.join("OpenRGB.json"), "{ not json").unwrap();
        assert!(write_settings(&dir, &NetworkLights::default(), SmbusLighting::OFF).unwrap());
        assert!(dir.join("OpenRGB.json.bak").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hosts_are_checked() {
        assert!(valid_host("192.168.1.40"));
        assert!(valid_host("wled-bureau.local"));
        assert!(valid_host("[fe80::1]:80"));
        assert!(!valid_host(""));
        assert!(!valid_host("http://192.168.1.40/"));
        assert!(!valid_host("a b"));
    }

    #[test]
    fn slow_lights_are_paced() {
        assert_eq!(
            min_interval("Yeelight Device"),
            Some(Duration::from_millis(1100))
        );
        assert!(min_interval("Philips Hue Entertainment Mode Device").is_none());
        assert!(min_interval("ASUS Aura").is_none());
    }

    #[test]
    fn wled_info_reads_name_and_led_count() {
        use http::tests::{json_response, serve_once};
        let (port, server) = serve_once(json_response(
            "200 OK",
            r#"{"ver":"0.15.0","leds":{"count":144,"rgbw":false},"name":"Étagère"}"#,
        ));
        let info = wled_info(&format!("127.0.0.1:{port}"), Duration::from_secs(2)).unwrap();
        assert_eq!(
            info,
            WledInfo {
                name: "Étagère".into(),
                leds: 144
            }
        );
        assert!(
            server
                .join()
                .unwrap()
                .starts_with("GET /json/info HTTP/1.0")
        );
        // Nothing listening: an error, quickly.
        let started = std::time::Instant::now();
        assert!(wled_info("127.0.0.1:9", Duration::from_millis(500)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        // Not reachable, but the LED count was typed: added anyway.
        let l = probe(
            Brand::Wled,
            "127.0.0.1:9",
            Some(30),
            Duration::from_millis(300),
        )
        .unwrap();
        assert!(matches!(l, NetLight::Wled(WledLight { leds: 30, .. })));
        assert!(probe(Brand::Wled, "127.0.0.1:9", None, Duration::from_millis(300)).is_err());
    }

    #[test]
    fn nanoleaf_pairing() {
        use http::tests::{json_response, serve_once};
        // Not in pairing mode.
        let (port, server) = serve_once(json_response("403 Forbidden", ""));
        let light = NetLight::Nanoleaf(NanoleafLight {
            host: "127.0.0.1".into(),
            port,
            token: String::new(),
            name: "Nanoleaf".into(),
        });
        let e = pair(light.clone(), Duration::from_secs(2)).unwrap_err();
        assert!(e.contains("5 à 7 secondes"));
        assert!(
            server
                .join()
                .unwrap()
                .starts_with("POST /api/v1/new HTTP/1.0")
        );
        // Paired: the token, then its name.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            for body in [r#"{"auth_token":"abc123"}"#, r#"{"name":"Shapes 4F2A"}"#] {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let _ = std::io::Write::write_all(&mut s, json_response("200 OK", body).as_bytes());
            }
        });
        let NetLight::Nanoleaf(l) = pair(
            NetLight::Nanoleaf(NanoleafLight {
                host: "127.0.0.1".into(),
                port,
                token: String::new(),
                name: "Nanoleaf".into(),
            }),
            Duration::from_secs(2),
        )
        .unwrap() else {
            panic!("not Nanoleaf")
        };
        t.join().unwrap();
        assert_eq!(l.token, "abc123");
        assert_eq!(l.name, "Shapes 4F2A");
    }

    #[test]
    fn hue_pairing() {
        use http::tests::{json_response, serve_once};
        let bridge = |port: u16| HueBridge {
            host: format!("127.0.0.1:{port}"),
            mac: "ec:b5:fa:12:34:56".into(),
            name: "Hue".into(),
            username: String::new(),
            clientkey: String::new(),
            entertainment: false,
        };
        let (port, server) = serve_once(json_response(
            "200 OK",
            r#"[{"error":{"type":101,"address":"","description":"link button not pressed"}}]"#,
        ));
        let e = pair(NetLight::Hue(bridge(port)), Duration::from_secs(2)).unwrap_err();
        assert!(e.contains("bouton rond"));
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /api HTTP/1.0"));
        assert!(req.contains(r#""generateclientkey":true"#));
        let (port, server) = serve_once(json_response(
            "200 OK",
            r#"[{"success":{"username":"83b7780291a6ceffbe0bd049104df","clientkey":"33DDAE4C9A1A1FCF"}}]"#,
        ));
        let NetLight::Hue(b) = pair(NetLight::Hue(bridge(port)), Duration::from_secs(2)).unwrap()
        else {
            panic!("not Hue")
        };
        server.join().unwrap();
        assert_eq!(b.username, "83b7780291a6ceffbe0bd049104df");
        assert_eq!(b.clientkey, "33DDAE4C9A1A1FCF");
        assert!(!NetLight::Hue(b).needs_pairing());
    }
}
