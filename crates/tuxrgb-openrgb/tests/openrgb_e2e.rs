//! End-to-end test against a real OpenRGB server with virtual "debug" devices.
//!
//! Ignored by default (needs an OpenRGB binary):
//!   TUXRGB_OPENRGB=/path/to/openrgb cargo test -p tuxrgb-openrgb -- --ignored
//!
//! It starts the bridge exactly like the app does (private embedded server),
//! streams one frame, then checks with an independent SDK client that the
//! server really holds the colours TuxRGB sent.
//!
//! Linux only (signals the server like the system does at logout).
#![cfg(unix)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tuxrgb_core::Rgb;
use tuxrgb_core::engine::{DeviceFrame, Frame};
use tuxrgb_openrgb::{BridgeCommand, BridgeConfig, BridgeEvent, BridgeStatus, ServerMode};

fn debug_config(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    let json = r#"{ "DebugDevices": { "devices": [
        { "type": "motherboard", "resizable": true }, { "type": "dram" }, { "type": "keyboard" }, { "type": "argb" }
    ] } }"#;
    std::fs::write(dir.join("OpenRGB.json"), json).unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "needs an OpenRGB 1.0 binary (TUXRGB_OPENRGB)"]
async fn streams_frames_to_a_real_openrgb_server() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let Some(found) = tuxrgb_openrgb::locate::find_openrgb() else {
        panic!("no OpenRGB binary: set TUXRGB_OPENRGB");
    };
    eprintln!("using {}", found.path.display());
    let tmp = std::env::temp_dir().join(format!("tuxrgb-e2e-{}", std::process::id()));
    debug_config(&tmp.join("openrgb"));

    // Servers already listening (another TuxRGB) must not be mistaken for ours.
    let busy: Vec<u16> = (6743..6760)
        .filter(|p| tuxrgb_openrgb::server::port_is_open(*p))
        .collect();
    let (frame_tx, frame_rx) = watch::channel(Arc::new(Frame::default()));
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let config = BridgeConfig {
        mode: ServerMode::Embedded,
        openrgb_config_dir: tmp.join("openrgb"),
        log_path: tmp.join("server.log"),
        client_name: "TuxRGB e2e".into(),
        disabled: BTreeSet::new(),
        hardware_modes: Default::default(),
        network: Default::default(),
        smbus: tuxrgb_openrgb::SmbusLighting {
            ram: true,
            motherboard: true,
        },
        // Raw colours: the test compares what the server received.
        calibration: tuxrgb_core::led_color::LedCalibration {
            enabled: false,
            ..Default::default()
        },
    };
    // Same entry point as the app: dedicated I/O thread + current-thread runtime.
    let thread = tuxrgb_openrgb::bridge::spawn_thread(config, frame_rx, cmd_rx, ev_tx)
        .expect("bridge thread");
    let bridge = tokio::task::spawn_blocking(move || {
        let _ = thread.join();
    });

    // Wait for the device list.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut devices = Vec::new();
    let mut port = None;
    while Instant::now() < deadline && devices.len() < 4 {
        match tokio::time::timeout(Duration::from_secs(5), ev_rx.recv()).await {
            Ok(Some(BridgeEvent::Devices(d))) => devices = d,
            Ok(Some(BridgeEvent::Status(BridgeStatus::Connected { .. }))) => {}
            Ok(Some(BridgeEvent::Status(BridgeStatus::Error(e)))) => panic!("bridge error: {e}"),
            Ok(Some(_)) => {}
            Ok(None) => panic!("bridge stopped"),
            Err(_) => {}
        }
        if port.is_none() {
            port = (6743..6760)
                .find(|p| !busy.contains(p) && tuxrgb_openrgb::server::port_is_open(*p));
        }
    }
    assert_eq!(devices.len(), 4, "debug devices not detected: {devices:?}");
    assert!(devices.iter().all(|d| d.controllable), "{devices:?}");

    // Stream one frame: device i gets a distinct colour.
    let palette = [
        Rgb::new(255, 0, 0),
        Rgb::new(0, 255, 0),
        Rgb::new(0, 0, 255),
        Rgb::new(255, 128, 0),
    ];
    let frame = Frame {
        seq: 1,
        devices: devices
            .iter()
            .zip(palette)
            .map(|(d, c)| DeviceFrame {
                id: d.id,
                colors: vec![c; d.led_count],
            })
            .collect(),
        level: 1.0,
        backdrop: None,
    };
    frame_tx.send_replace(Arc::new(frame));
    tokio::time::sleep(Duration::from_millis(700)).await;

    // Independent client: read the colours back from the server.
    let port = port.expect("embedded server port");
    let client = openrgb2::OpenRgbClient::connect_to(("127.0.0.1", port), 6)
        .await
        .expect("second client");
    let group = client.get_all_controllers().await.expect("controllers");
    for (d, expected) in devices.iter().zip(palette) {
        let c = group
            .iter()
            .find(|c| c.id() as u32 == d.id)
            .expect("controller still present");
        let colors = c.colors();
        assert_eq!(colors.len(), d.led_count);
        assert!(
            colors
                .iter()
                .all(|x| x.r == expected.r && x.g == expected.g && x.b == expected.b),
            "{}: {:?}",
            d.name,
            &colors[..colors.len().min(3)]
        );
    }

    // LED colour correction: the same frame, written again through the
    // table at once (no new frame needed), as the hardware gets it.
    // The first device has a white of its own (RAM and case LEDs differ).
    let mut calibration = tuxrgb_core::led_color::LedCalibration::default();
    calibration
        .devices
        .insert(devices[0].key.0.clone(), [0.5, 1.0, 0.25]);
    cmd_tx
        .send(BridgeCommand::SetCalibration(calibration.clone()))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(700)).await;
    let group = client.get_all_controllers().await.expect("controllers");
    for (d, sent) in devices.iter().zip(palette) {
        let expected = calibration.grade_of(&d.key.0).apply(sent, 1.0);
        let c = group
            .iter()
            .find(|c| c.id() as u32 == d.id)
            .expect("controller still present");
        assert!(
            c.colors()
                .iter()
                .all(|x| x.r == expected.r && x.g == expected.g && x.b == expected.b),
            "{} calibrated: {:?} expected {expected:?}",
            d.name,
            &c.colors()[..c.colors().len().min(3)]
        );
    }
    // Back to raw colours for the rest of the test.
    cmd_tx
        .send(BridgeCommand::SetCalibration(
            tuxrgb_core::led_color::LedCalibration {
                enabled: false,
                ..Default::default()
            },
        ))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    // ARGB header: the motherboard has an empty resizable zone (0..=100).
    let mobo = devices
        .iter()
        .find(|d| d.zones.iter().any(|z| z.resizable()))
        .expect("resizable zone")
        .clone();
    let (zi, zone) = mobo
        .zones
        .iter()
        .enumerate()
        .find(|(_, z)| z.resizable())
        .unwrap();
    assert_eq!(zone.led_count, 0);
    assert!(zone.needs_size());
    let _ = cmd_tx.send(BridgeCommand::ResizeZones(vec![
        tuxrgb_openrgb::ZoneResize {
            device: mobo.key.clone(),
            zone: zi,
            leds: 24,
        },
    ]));
    let resized = wait_for_zone(&mut ev_rx, &mobo.key, zi, 24).await;
    assert_eq!(resized.led_count, mobo.led_count + 24);
    assert!(resized.controllable);

    // The size is kept by OpenRGB across a server restart.
    let _ = cmd_tx.send(BridgeCommand::Restart);
    let (after, devices) = wait_for_zone_list(&mut ev_rx, &mobo.key, zi, 24).await;
    assert_eq!(after.led_count, mobo.led_count + 24);
    // The latest device list (ids may change with the new server).
    assert_eq!(devices.len(), 4);
    let port = (6743..6760)
        .find(|p| !busy.contains(p) && tuxrgb_openrgb::server::port_is_open(*p))
        .expect("restarted server port");
    let reader = openrgb2::OpenRgbClient::connect_to(("127.0.0.1", port), 6)
        .await
        .expect("second client");
    let colours_of = |group: &openrgb2::ControllerGroup, id: u32| -> Vec<(u8, u8, u8)> {
        group
            .iter()
            .find(|c| c.id() as u32 == id)
            .map(|c| c.colors().iter().map(|x| (x.r, x.g, x.b)).collect())
            .unwrap_or_default()
    };
    let solid = |d: &tuxrgb_core::DeviceInfo, leds: usize, c: Rgb| DeviceFrame {
        id: d.id,
        colors: vec![c; leds],
    };

    // Hot-plug safety: a frame built for the old layout of the resized
    // device (stale scene) and one for a device that no longer exists are
    // skipped; the other devices are still written and the session lives.
    let mobo_now = devices.iter().find(|d| d.key == mobo.key).unwrap().clone();
    let reference = Rgb::new(10, 20, 30);
    let mut good = Frame {
        seq: 2,
        devices: devices
            .iter()
            .map(|d| solid(d, d.led_count, reference))
            .collect(),
        level: 1.0,
        backdrop: None,
    };
    frame_tx.send_replace(Arc::new(good.clone()));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let white = Rgb::new(250, 250, 250);
    let mut stale: Vec<DeviceFrame> = devices
        .iter()
        .filter(|d| d.key != mobo.key)
        .map(|d| solid(d, d.led_count, white))
        .collect();
    stale.push(solid(&mobo_now, mobo.led_count, white));
    stale.push(DeviceFrame {
        id: 999_999,
        colors: vec![white; 12],
    });
    frame_tx.send_replace(Arc::new(Frame {
        seq: 3,
        devices: stale,
        level: 1.0,
        backdrop: None,
    }));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let group = reader.get_all_controllers().await.expect("controllers");
    assert!(
        colours_of(&group, mobo_now.id)
            .iter()
            .all(|c| *c == (10, 20, 30)),
        "stale frame written to the resized device"
    );
    for d in devices.iter().filter(|d| d.key != mobo.key) {
        assert!(
            colours_of(&group, d.id)
                .iter()
                .all(|c| *c == (250, 250, 250)),
            "{} not updated",
            d.name
        );
    }
    good.seq = 4;
    frame_tx.send_replace(Arc::new(good));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let group = reader.get_all_controllers().await.expect("controllers");
    assert!(
        colours_of(&group, mobo_now.id)
            .iter()
            .all(|c| *c == (10, 20, 30))
    );
    while let Ok(ev) = ev_rx.try_recv() {
        if let BridgeEvent::Status(s) = ev {
            assert!(
                !matches!(s, BridgeStatus::Reconnecting(_) | BridgeStatus::Error(_)),
                "session lost: {s:?}"
            );
        }
    }

    // A zone resized by another program (OpenRGB's own window…) is noticed
    // from the server's notification, without waiting for the full check.
    let started = Instant::now();
    let c = group
        .iter()
        .find(|c| c.id() as u32 == mobo_now.id)
        .expect("motherboard");
    c.get_zone(zi)
        .expect("zone")
        .resize(30)
        .await
        .expect("resize by another client");
    let resized = wait_for_zone(&mut ev_rx, &mobo.key, zi, 30).await;
    assert_eq!(resized.led_count, mobo.led_count + 30);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "external change noticed after {:?}",
        started.elapsed()
    );

    // Debug devices only have a direct mode: no hardware effect to offer.
    assert!(
        devices
            .iter()
            .all(|d| d.hardware_modes.is_empty() && d.hardware_mode.is_none())
    );

    // Blackout (sleep, shutdown): every LED of every device to black,
    // confirmed within the strict budget.
    let done = Arc::new(tokio::sync::Notify::new());
    let started = Instant::now();
    let _ = cmd_tx.send(BridgeCommand::Blackout(Some(done.clone())));
    tokio::time::timeout(Duration::from_secs(10), done.notified())
        .await
        .expect("blackout confirmed");
    let took = started.elapsed();
    eprintln!("blackout confirmed in {took:?}");
    assert!(
        took <= tuxrgb_openrgb::bridge::BLACKOUT_BUDGET + Duration::from_millis(100),
        "blackout took {took:?}"
    );
    let third = openrgb2::OpenRgbClient::connect_to(("127.0.0.1", port), 6)
        .await
        .expect("third client");
    // A frame the engine rendered just before turning its output off can
    // reach the bridge after the blackout: it must not light anything again.
    let current = third.get_all_controllers().await.expect("controllers");
    frame_tx.send_replace(Arc::new(Frame {
        seq: 99,
        devices: current
            .iter()
            .map(|c| DeviceFrame {
                id: c.id() as u32,
                colors: vec![Rgb::new(255, 0, 0); c.num_leds()],
            })
            .collect(),
        level: 1.0,
        backdrop: None,
    }));
    tokio::time::sleep(Duration::from_millis(600)).await;
    let dark = third
        .get_all_controllers()
        .await
        .expect("controllers after blackout");
    for c in dark.iter() {
        assert!(
            c.led_iter().all(|l| {
                let col = l.color();
                (col.r, col.g, col.b) == (0, 0, 0)
            }),
            "{} not black after the blackout",
            c.name()
        );
    }

    // Resume: the frames light the devices again.
    let _ = cmd_tx.send(BridgeCommand::Reapply);
    tokio::time::sleep(Duration::from_millis(400)).await;
    frame_tx.send_modify(|f| {
        let mut lit = (**f).clone();
        lit.seq += 1;
        *f = Arc::new(lit);
    });
    tokio::time::sleep(Duration::from_millis(800)).await;
    let lit = third
        .get_all_controllers()
        .await
        .expect("controllers after resume");
    assert!(
        lit.iter().all(|c| c
            .led_iter()
            .all(|l| (l.color().r, l.color().g, l.color().b) == (255, 0, 0))),
        "devices not lit again after the resume"
    );

    // Quitting with "turn the LEDs off": within the budget as well.
    let started = Instant::now();
    let _ = cmd_tx.send(BridgeCommand::Shutdown { turn_off: true });
    let _ = tokio::time::timeout(Duration::from_secs(5), bridge).await;
    eprintln!("quit (blackout + server stop) in {:?}", started.elapsed());
    let _ = std::fs::remove_dir_all(&tmp);
}

async fn wait_for_zone(
    ev_rx: &mut mpsc::UnboundedReceiver<BridgeEvent>,
    key: &tuxrgb_core::DeviceKey,
    zone: usize,
    leds: usize,
) -> tuxrgb_core::DeviceInfo {
    wait_for_zone_list(ev_rx, key, zone, leds).await.0
}

/// The device once its zone has `leds` LEDs, and the whole list then.
async fn wait_for_zone_list(
    ev_rx: &mut mpsc::UnboundedReceiver<BridgeEvent>,
    key: &tuxrgb_core::DeviceKey,
    zone: usize,
    leds: usize,
) -> (tuxrgb_core::DeviceInfo, Vec<tuxrgb_core::DeviceInfo>) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if let Ok(Some(BridgeEvent::Devices(d))) =
            tokio::time::timeout(Duration::from_secs(5), ev_rx.recv()).await
            && let Some(dev) = d.iter().find(|d| d.key == *key)
            && dev.zones.get(zone).map(|z| z.led_count) == Some(leds)
        {
            return (dev.clone(), d);
        }
    }
    panic!("zone {zone} never reached {leds} LEDs");
}

/// Logout / shutdown: systemd sends SIGTERM to every process of the session
/// at once. The embedded server must survive it (so TuxRGB can still turn
/// the LEDs off) and stop when TuxRGB asks.
#[test]
#[ignore = "needs an OpenRGB 1.0 binary (TUXRGB_OPENRGB)"]
fn embedded_server_outlives_sigterm() {
    let found = tuxrgb_openrgb::locate::find_openrgb().expect("TUXRGB_OPENRGB");
    let tmp = std::env::temp_dir().join(format!("tuxrgb-sig-{}", std::process::id()));
    debug_config(&tmp.join("openrgb"));
    let port = tuxrgb_openrgb::server::pick_port(6799);
    let mut server = tuxrgb_openrgb::server::EmbeddedServer::spawn(
        &found.path,
        port,
        &tmp.join("openrgb"),
        &tmp.join("openrgb.log"),
    )
    .expect("server starts");
    std::thread::sleep(Duration::from_millis(1500));
    assert!(server.is_running());
    unsafe {
        libc::kill(server.pid() as libc::pid_t, libc::SIGTERM);
    }
    std::thread::sleep(Duration::from_millis(1500));
    assert!(server.is_running(), "SIGTERM must not stop the server");
    let started = Instant::now();
    server.stop(Duration::from_secs(5));
    assert!(!server.is_running());
    eprintln!("stopped in {:?}", started.elapsed());
    let _ = std::fs::remove_dir_all(tmp);
}

/// Network lights through the real OpenRGB: a simulated WLED controller
/// (DDP receiver) and a simulated Philips WiZ bulb (JSON over UDP), both on
/// the loopback. Colours, the bulb's own scene (hardware effect) and the
/// blackout must reach them.
#[tokio::test(flavor = "current_thread")]
#[ignore = "needs an OpenRGB 1.0 binary (TUXRGB_OPENRGB)"]
async fn network_lights_through_openrgb() {
    use std::sync::Mutex;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    assert!(
        tuxrgb_openrgb::locate::find_openrgb().is_some(),
        "set TUXRGB_OPENRGB"
    );
    let tmp = std::env::temp_dir().join(format!("tuxrgb-net-e2e-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join("openrgb")).unwrap();

    // Simulated WLED: every DDP packet received.
    let ddp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    ddp.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let ddp_port = ddp.local_addr().unwrap().port();
    let ddp_packets: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (packets, stop) = (ddp_packets.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok((n, _)) = ddp.recv_from(&mut buf) {
                    packets.lock().unwrap().push(buf[..n].to_vec());
                }
            }
        });
    }
    // Simulated WiZ bulb (fixed port 38899): answers like a real RGB bulb.
    let wiz =
        std::net::UdpSocket::bind("127.0.0.1:38899").expect("port 38899 free for the WiZ bulb");
    wiz.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let wiz_msgs: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    {
        let (msgs, stop) = (wiz_msgs.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let Ok((n, from)) = wiz.recv_from(&mut buf) else {
                    continue;
                };
                let text = String::from_utf8_lossy(&buf[..n])
                    .trim_end_matches('\0')
                    .to_owned();
                let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                let method = msg["method"].as_str().unwrap_or_default().to_owned();
                let reply = match method.as_str() {
                    "getSystemConfig" => {
                        serde_json::json!({"method": "getSystemConfig", "env": "pro", "result": {
                        "mac": "a8bb50e5f1c2", "homeId": 1, "roomId": 1,
                        "moduleName": "ESP01_SHRGB1C_31", "fwVersion": "1.25.0"}})
                    }
                    _ => {
                        serde_json::json!({"method": method, "env": "pro", "result": {"success": true}})
                    }
                };
                let _ = wiz.send_to(reply.to_string().as_bytes(), from);
                msgs.lock().unwrap().push(msg);
            }
        });
    }
    // Simulated LIFX bulb (fixed port 56700): the packet types received.
    let lifx = std::net::UdpSocket::bind("127.0.0.1:56700").expect("port 56700 free for LIFX");
    lifx.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let lifx_types: Arc<Mutex<Vec<u16>>> = Arc::default();
    {
        let (types, stop) = (lifx_types.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok((n, _)) = lifx.recv_from(&mut buf)
                    && n >= 36
                {
                    types
                        .lock()
                        .unwrap()
                        .push(u16::from_le_bytes([buf[32], buf[33]]));
                }
            }
        });
    }
    // Simulated Yeelight (fixed TCP port 55443): one command per connection.
    let yeelight =
        std::net::TcpListener::bind("127.0.0.1:55443").expect("port 55443 free for Yeelight");
    yeelight.set_nonblocking(true).unwrap();
    let yeelight_cmds: Arc<Mutex<Vec<(Instant, String)>>> = Arc::default();
    {
        let (cmds, stop) = (yeelight_cmds.clone(), stop.clone());
        std::thread::spawn(move || {
            use std::io::Read;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                match yeelight.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_millis(300)));
                        let mut text = Vec::new();
                        let _ = s.read_to_end(&mut text);
                        let text = String::from_utf8_lossy(&text)
                            .trim_matches('\0')
                            .trim()
                            .to_owned();
                        if !text.is_empty() {
                            cmds.lock().unwrap().push((Instant::now(), text));
                        }
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        });
    }

    let (frame_tx, frame_rx) = watch::channel(Arc::new(Frame::default()));
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let config = BridgeConfig {
        mode: ServerMode::Embedded,
        openrgb_config_dir: tmp.join("openrgb"),
        log_path: tmp.join("server.log"),
        client_name: "TuxRGB e2e".into(),
        disabled: BTreeSet::new(),
        hardware_modes: Default::default(),
        network: {
            use tuxrgb_openrgb::network::{LifxLight, YeelightLight};
            use tuxrgb_openrgb::{NetLight, WizLight, WledLight};
            let mut n = tuxrgb_openrgb::NetworkLights::default();
            n.add(NetLight::Wled(WledLight {
                name: "WLED bureau".into(),
                host: "127.0.0.1".into(),
                port: ddp_port,
                leds: 30,
            }));
            n.add(NetLight::Wiz(WizLight {
                host: "127.0.0.1".into(),
                warm_white: false,
            }));
            n.add(NetLight::Lifx(LifxLight {
                host: "127.0.0.1".into(),
                name: "Étagère".into(),
                multizone: false,
                extended_multizone: false,
            }));
            n.add(NetLight::Yeelight(YeelightLight {
                host: "127.0.0.1".into(),
                name: String::new(),
            }));
            n
        },
        // Prudence: the memory bus detectors are written off.
        smbus: tuxrgb_openrgb::SmbusLighting::OFF,
        calibration: tuxrgb_core::led_color::LedCalibration {
            enabled: false,
            ..Default::default()
        },
    };
    let thread = tuxrgb_openrgb::bridge::spawn_thread(config, frame_rx, cmd_rx, ev_tx)
        .expect("bridge thread");
    let bridge = tokio::task::spawn_blocking(move || {
        let _ = thread.join();
    });

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut devices = Vec::new();
    while Instant::now() < deadline && devices.len() < 4 {
        if let Ok(Some(BridgeEvent::Devices(d))) =
            tokio::time::timeout(Duration::from_secs(5), ev_rx.recv()).await
        {
            devices = d;
        }
    }
    eprintln!(
        "{:#?}",
        devices
            .iter()
            .map(|d| (&d.name, d.kind, d.led_count, &d.hardware_modes))
            .collect::<Vec<_>>()
    );
    let strip = devices
        .iter()
        .find(|d| d.led_count == 30 && d.kind == tuxrgb_core::DeviceKind::LedStrip)
        .expect("WLED controller detected")
        .clone();
    let bulb = devices
        .iter()
        .find(|d| d.kind == tuxrgb_core::DeviceKind::Light && d.name.contains("Wiz"))
        .expect("WiZ bulb detected")
        .clone();
    assert_eq!(strip.name, "WLED bureau");
    assert!(strip.controllable && bulb.controllable);
    let lifx_dev = devices
        .iter()
        .find(|d| d.name.contains("Étagère"))
        .expect("LIFX bulb detected")
        .clone();
    let yeelight_dev = devices
        .iter()
        .find(|d| d.name.contains("Yeelight"))
        .expect("Yeelight detected")
        .clone();
    assert!(lifx_dev.controllable && yeelight_dev.controllable);
    // The memory bus was left alone.
    let settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.join("openrgb/OpenRGB.json")).unwrap())
            .unwrap();
    assert_eq!(
        settings.pointer("/Detectors/detectors/ENE SMBus DRAM"),
        Some(&serde_json::json!(false))
    );
    assert!(
        bulb.hardware_modes.iter().any(|m| m == "Ocean"),
        "{:?}",
        bulb.hardware_modes
    );

    // Colours: the strip red, the bulb blue.
    frame_tx.send_replace(Arc::new(Frame {
        seq: 1,
        devices: vec![
            DeviceFrame {
                id: strip.id,
                colors: vec![Rgb::new(255, 0, 0); 30],
            },
            DeviceFrame {
                id: bulb.id,
                colors: vec![Rgb::new(0, 0, 255)],
            },
            DeviceFrame {
                id: lifx_dev.id,
                colors: vec![Rgb::new(0, 255, 0)],
            },
            DeviceFrame {
                id: yeelight_dev.id,
                colors: vec![Rgb::new(255, 0, 255)],
            },
        ],
        level: 1.0,
        backdrop: None,
    }));
    tokio::time::sleep(Duration::from_millis(600)).await;
    // LIFX: SetColor (102); Yeelight: a one-frame colour flow.
    assert!(
        lifx_types.lock().unwrap().contains(&102),
        "{:?}",
        lifx_types.lock().unwrap()
    );
    assert!(
        yeelight_cmds
            .lock()
            .unwrap()
            .iter()
            .any(|(_, c)| c.contains("start_cf")),
        "{:?}",
        yeelight_cmds.lock().unwrap()
    );
    // A fast effect on the Yeelight: paced to its quota (one per 1.1 s).
    let before = yeelight_cmds.lock().unwrap().len();
    for i in 0..20u8 {
        frame_tx.send_replace(Arc::new(Frame {
            seq: 100 + i as u64,
            devices: vec![DeviceFrame {
                id: yeelight_dev.id,
                colors: vec![Rgb::new(i * 10, 0, 255 - i * 10)],
            }],
            level: 1.0,
            backdrop: None,
        }));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let cmds = yeelight_cmds.lock().unwrap()[before..].to_vec();
    eprintln!("Yeelight: {} commands for 20 frames", cmds.len());
    assert!((1..=3).contains(&cmds.len()), "{cmds:?}");
    for w in cmds.windows(2) {
        assert!(w[1].0 - w[0].0 >= Duration::from_millis(1000), "{cmds:?}");
    }
    // The last colour lands.
    let last = (190u32 << 16) | (255 - 190);
    assert!(
        cmds.last().unwrap().1.contains(&last.to_string()),
        "{cmds:?}"
    );
    let last_ddp = |packets: &Mutex<Vec<Vec<u8>>>| {
        packets.lock().unwrap().last().cloned().expect("DDP packet")
    };
    let p = last_ddp(&ddp_packets);
    assert_eq!(p[0] & 0xC0, 0x40, "DDP version 1");
    assert_eq!(u32::from_be_bytes([p[4], p[5], p[6], p[7]]), 0, "offset");
    assert_eq!(u16::from_be_bytes([p[8], p[9]]) as usize, 90, "length");
    assert!(
        p[10..].chunks(3).all(|c| c == [255, 0, 0]),
        "{:?}",
        &p[10..16]
    );
    let pilot = |msgs: &Mutex<Vec<serde_json::Value>>| -> Option<serde_json::Value> {
        msgs.lock()
            .unwrap()
            .iter()
            .rev()
            .find(|m| m["method"] == "setPilot")
            .cloned()
    };
    let lit = pilot(&wiz_msgs).expect("setPilot sent to the bulb");
    assert_eq!(
        (
            lit["params"]["r"].as_u64(),
            lit["params"]["g"].as_u64(),
            lit["params"]["b"].as_u64()
        ),
        (Some(0), Some(0), Some(255)),
        "{lit}"
    );
    assert_eq!(lit["params"]["state"], serde_json::json!(true));

    // The bulb's own scene: TuxRGB lets go of it.
    let mut modes = std::collections::BTreeMap::new();
    modes.insert(bulb.key.clone(), "Ocean".to_owned());
    let _ = cmd_tx.send(BridgeCommand::SetHardwareModes(modes));
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut applied = false;
    while Instant::now() < deadline && !applied {
        if let Ok(Some(BridgeEvent::Devices(d))) =
            tokio::time::timeout(Duration::from_secs(5), ev_rx.recv()).await
        {
            applied = d
                .iter()
                .any(|d| d.key == bulb.key && d.hardware_mode.as_deref() == Some("Ocean"));
        }
    }
    assert!(applied, "hardware effect not reported");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let scene = pilot(&wiz_msgs).expect("scene sent");
    assert!(scene["params"]["sceneId"].as_u64().is_some(), "{scene}");
    let count = wiz_msgs.lock().unwrap().len();
    frame_tx.send_replace(Arc::new(Frame {
        seq: 2,
        devices: vec![
            DeviceFrame {
                id: strip.id,
                colors: vec![Rgb::new(0, 255, 0); 30],
            },
            DeviceFrame {
                id: bulb.id,
                colors: vec![Rgb::new(255, 255, 0)],
            },
        ],
        level: 1.0,
        backdrop: None,
    }));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        wiz_msgs.lock().unwrap()[count..]
            .iter()
            .all(|m| m["method"] != "setPilot"),
        "colours sent to a bulb playing its own scene"
    );
    assert!(
        last_ddp(&ddp_packets)[10..]
            .chunks(3)
            .all(|c| c == [0, 255, 0])
    );

    // Blackout: both dark, the bulb in its scene too, within the budget.
    let done = Arc::new(tokio::sync::Notify::new());
    let started = Instant::now();
    let _ = cmd_tx.send(BridgeCommand::Blackout(Some(done.clone())));
    tokio::time::timeout(Duration::from_secs(5), done.notified())
        .await
        .expect("blackout");
    let took = started.elapsed();
    eprintln!("network blackout in {took:?}");
    assert!(took <= tuxrgb_openrgb::bridge::BLACKOUT_BUDGET + Duration::from_millis(100));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        last_ddp(&ddp_packets)[10..].iter().all(|b| *b == 0),
        "strip not dark"
    );
    let off = pilot(&wiz_msgs).expect("bulb turned off");
    assert_eq!(off["params"]["state"], serde_json::json!(false), "{off}");

    let _ = cmd_tx.send(BridgeCommand::Shutdown { turn_off: false });
    let _ = tokio::time::timeout(Duration::from_secs(5), bridge).await;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Another RGB program running: the RAM and SMBus motherboard detectors stay
/// off until it closes, then the engine takes them back by itself.
#[tokio::test(flavor = "current_thread")]
#[ignore = "needs an OpenRGB 1.0 binary (TUXRGB_OPENRGB)"]
async fn smbus_waits_for_other_rgb_programs() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    assert!(tuxrgb_openrgb::locate::find_openrgb().is_some());
    let tmp = std::env::temp_dir().join(format!("tuxrgb-e2e-smbus-{}", std::process::id()));
    debug_config(&tmp.join("openrgb"));
    // A stand-in "openrgb" process (a copy of sleep).
    let fake_dir = tmp.join("other");
    std::fs::create_dir_all(&fake_dir).unwrap();
    let fake = fake_dir.join("openrgb");
    std::fs::copy("/bin/sleep", &fake).unwrap();
    let mut other = std::process::Command::new(&fake)
        .arg("600")
        .spawn()
        .unwrap();
    // The settings are written just after the event: allow a moment.
    let ram_detector = |want: bool| {
        let name = tuxrgb_openrgb::detectors::SMBUS_RAM_DETECTORS[0];
        let mut got = None;
        for _ in 0..50 {
            let text =
                std::fs::read_to_string(tmp.join("openrgb/OpenRGB.json")).unwrap_or_default();
            let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            got = json["Detectors"]["detectors"][name].as_bool();
            if got == Some(want) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("{name}: {got:?}, expected {want}");
    };

    let (_frame_tx, frame_rx) = watch::channel(Arc::new(Frame::default()));
    let (_cmd_tx, cmd_rx) = mpsc::unbounded_channel::<BridgeCommand>();
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let config = BridgeConfig {
        mode: ServerMode::Embedded,
        openrgb_config_dir: tmp.join("openrgb"),
        log_path: tmp.join("server.log"),
        client_name: "TuxRGB e2e".into(),
        disabled: BTreeSet::new(),
        hardware_modes: Default::default(),
        network: Default::default(),
        smbus: tuxrgb_openrgb::SmbusLighting {
            ram: true,
            motherboard: true,
        },
        calibration: Default::default(),
    };
    let _thread = tuxrgb_openrgb::bridge::spawn_thread(config, frame_rx, cmd_rx, ev_tx)
        .expect("bridge thread");

    async fn wait_blocked(
        ev_rx: &mut mpsc::UnboundedReceiver<BridgeEvent>,
        want_empty: bool,
    ) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(40);
        while Instant::now() < deadline {
            if let Ok(Some(BridgeEvent::SmbusBlocked(list))) =
                tokio::time::timeout(Duration::from_secs(1), ev_rx.recv()).await
                && list.is_empty() == want_empty
            {
                return list;
            }
        }
        panic!("no SmbusBlocked(empty: {want_empty})");
    }

    let blocked = wait_blocked(&mut ev_rx, false).await;
    assert_eq!(blocked, ["OpenRGB"]);
    // The embedded server (also named openrgb) never counts.
    tokio::time::sleep(Duration::from_secs(2)).await;
    ram_detector(false);

    let _ = other.kill();
    let _ = other.wait();
    let t = Instant::now();
    wait_blocked(&mut ev_rx, true).await;
    eprintln!(
        "RAM taken back {:?} after the other program closed",
        t.elapsed()
    );
    ram_detector(true);
    let _ = std::fs::remove_dir_all(&tmp);
}
