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

    let _ = cmd_tx.send(BridgeCommand::Shutdown { turn_off: false });
    let _ = tokio::time::timeout(Duration::from_secs(5), bridge).await;
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
