//! Async bridge between the effect engine and the OpenRGB SDK server.

use crate::locate::{self, Origin};
use crate::server::{self, EmbeddedServer};
use futures::FutureExt;
use openrgb2::{
    ColorMode, Controller, ControllerGroup, DeviceType, LedPipe, ModeData, OpenRgbClient, PipeAck,
    ZoneType,
};
use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tuxrgb_core::engine::Frame;
use tuxrgb_core::{DeviceInfo, DeviceKey, DeviceKind, Rgb, ZoneInfo, ZoneKind};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ServerMode {
    /// Private server shipped with TuxRGB (default, zero dependency).
    Embedded,
    /// Connect to an OpenRGB server started elsewhere (e.g. a system service).
    External { host: String, port: u16 },
}

#[derive(Clone, Debug)]
pub struct BridgeConfig {
    pub mode: ServerMode,
    /// TuxRGB's private OpenRGB configuration directory.
    pub openrgb_config_dir: PathBuf,
    pub log_path: PathBuf,
    pub client_name: String,
    pub disabled: BTreeSet<DeviceKey>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BridgeStatus {
    Starting,
    /// No OpenRGB binary could be found (should not happen with the AppImage).
    NoServerBinary,
    /// Server running, hardware detection in progress.
    Detecting {
        seconds: u32,
    },
    Connected {
        devices: usize,
        controllable: usize,
        protocol: u32,
        server: String,
    },
    Reconnecting(String),
    Error(String),
    Stopped,
}

impl BridgeStatus {
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }
}

#[derive(Clone, Debug)]
pub enum BridgeEvent {
    Status(BridgeStatus),
    Devices(Vec<DeviceInfo>),
}

#[derive(Clone, Debug)]
pub enum BridgeCommand {
    /// Ask OpenRGB to re-run hardware detection.
    Rescan,
    /// Drop the connection (and restart the embedded server).
    Restart,
    SetDisabled(BTreeSet<DeviceKey>),
    SetMode(ServerMode),
    /// Sets the number of LEDs of resizable zones (ARGB headers / channels),
    /// then reloads the devices once. OpenRGB stores the sizes in its
    /// configuration, so they survive restarts.
    ResizeZones(Vec<ZoneResize>),
    /// Turn every managed LED off, then stop.
    Shutdown {
        turn_off: bool,
    },
    /// Turn every managed LED off right now and keep running (system
    /// shutdown, sleep, logout); `done` is notified once written.
    Blackout(Option<Arc<tokio::sync::Notify>>),
    /// Re-enter direct mode on every device and resend the current frame
    /// (after resume from sleep: some controllers reset themselves).
    Reapply,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneResize {
    pub device: DeviceKey,
    pub zone: usize,
    pub leds: usize,
}

type Events = mpsc::UnboundedSender<BridgeEvent>;

/// Event sink shared by the bridge loops.
struct Emitter(Events);

impl Emitter {
    fn emit(&self, e: BridgeEvent) {
        let _ = self.0.send(e);
    }
}

/// Starts the bridge on a dedicated I/O thread ("tuxrgb-openrgb") with its
/// own single-threaded Tokio runtime: device I/O never competes with the GUI
/// executor, and a stuck device cannot freeze the interface.
pub fn spawn_thread(
    config: BridgeConfig,
    frames: watch::Receiver<Arc<Frame>>,
    commands: mpsc::UnboundedReceiver<BridgeCommand>,
    events: Events,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("tuxrgb-openrgb".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = events.send(BridgeEvent::Status(BridgeStatus::Error(e.to_string())));
                    return;
                }
            };
            rt.block_on(run(config, frames, commands, events));
        })
}

/// Runs until [`BridgeCommand::Shutdown`] or until every sender is dropped.
pub async fn run(
    mut config: BridgeConfig,
    mut frames: watch::Receiver<Arc<Frame>>,
    mut commands: mpsc::UnboundedReceiver<BridgeCommand>,
    events: Events,
) {
    let mut server: Option<EmbeddedServer> = None;
    let send = Emitter(events);
    send.emit(BridgeEvent::Status(BridgeStatus::Starting));

    'outer: loop {
        // 1. Make sure a server exists.
        let (host, port, server_label) = match &config.mode {
            ServerMode::External { host, port } => {
                (host.clone(), *port, format!("externe {host}:{port}"))
            }
            ServerMode::Embedded => {
                let alive = server.as_mut().map(|s| s.is_running()).unwrap_or(false);
                if !alive {
                    if let Some(old) = server.as_ref() {
                        let tail = old.log_tail(8);
                        tracing::warn!("embedded OpenRGB server exited:\n{tail}");
                    }
                    server = None;
                    match spawn_embedded(&config) {
                        Ok(s) => server = Some(s),
                        Err(status) => {
                            send.emit(BridgeEvent::Status(status));
                            // Wait for a command (e.g. Restart after the user
                            // fixed something) or retry every 30 s.
                            match tokio::time::timeout(Duration::from_secs(30), commands.recv())
                                .await
                            {
                                Ok(Some(BridgeCommand::Shutdown { .. })) | Ok(None) => break 'outer,
                                Ok(Some(BridgeCommand::SetMode(m))) => config.mode = m,
                                Ok(Some(BridgeCommand::SetDisabled(d))) => config.disabled = d,
                                _ => {}
                            }
                            continue 'outer;
                        }
                    }
                }
                let s = server.as_ref().expect("server just ensured");
                let origin = if s.binary.starts_with("/usr/bin") {
                    "système"
                } else {
                    "embarqué"
                };
                (
                    "127.0.0.1".to_owned(),
                    s.port,
                    format!("{} {origin}", crate::OPENRGB_LABEL),
                )
            }
        };

        // 2. Connect (the server opens its port while detection still runs).
        let started = Instant::now();
        let client = loop {
            let addr = (host.as_str(), port);
            match tokio::time::timeout(Duration::from_secs(3), OpenRgbClient::connect_to(addr, 6))
                .await
            {
                Ok(Ok(c)) => break c,
                Ok(Err(e)) => tracing::debug!("connect: {e}"),
                Err(_) => tracing::debug!("connect: timeout"),
            }
            if let Some(s) = server.as_mut()
                && !s.is_running()
            {
                let tail = s.log_tail(6);
                send.emit(BridgeEvent::Status(BridgeStatus::Error(format!(
                    "Le moteur OpenRGB s'est arrêté.\n{tail}"
                ))));
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue 'outer;
            }
            send.emit(BridgeEvent::Status(BridgeStatus::Detecting {
                seconds: started.elapsed().as_secs() as u32,
            }));
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                cmd = commands.recv() => match cmd {
                    None | Some(BridgeCommand::Shutdown { .. }) => break 'outer,
                    Some(BridgeCommand::Restart) => { server = None; continue 'outer; }
                    Some(BridgeCommand::SetMode(m)) => { config.mode = m; server = None; continue 'outer; }
                    Some(BridgeCommand::SetDisabled(d)) => config.disabled = d,
                    Some(BridgeCommand::Blackout(done)) => {
                        if let Some(n) = done {
                            n.notify_one();
                        }
                    }
                    Some(BridgeCommand::Rescan)
                    | Some(BridgeCommand::ResizeZones(_))
                    | Some(BridgeCommand::Reapply) => {}
                },
            }
        };

        // 3. Session.
        let outcome = AssertUnwindSafe(session(
            client,
            (host.as_str(), port),
            &mut config,
            &server_label,
            &mut frames,
            &mut commands,
            &send,
        ))
        .catch_unwind()
        .await;
        match outcome {
            Ok(SessionEnd::Shutdown) => break 'outer,
            Ok(SessionEnd::Restart) => {
                server = None;
            }
            Ok(SessionEnd::Lost(reason)) => {
                tracing::warn!("OpenRGB connection lost: {reason}");
                send.emit(BridgeEvent::Status(BridgeStatus::Reconnecting(reason)));
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(_) => {
                tracing::error!("panic inside the OpenRGB session, restarting it");
                send.emit(BridgeEvent::Status(BridgeStatus::Reconnecting(
                    "erreur interne du client SDK".into(),
                )));
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    drop(server);
    send.emit(BridgeEvent::Status(BridgeStatus::Stopped));
}

fn spawn_embedded(config: &BridgeConfig) -> Result<EmbeddedServer, BridgeStatus> {
    let Some(found) = locate::find_openrgb() else {
        return Err(BridgeStatus::NoServerBinary);
    };
    if found.origin == Origin::System {
        tracing::info!(
            "bundled OpenRGB not found, using system binary {}",
            found.path.display()
        );
    }
    let port = server::pick_port(crate::EMBEDDED_PORT);
    EmbeddedServer::spawn(
        &found.path,
        port,
        &config.openrgb_config_dir,
        &config.log_path,
    )
    .map_err(|e| BridgeStatus::Error(format!("Impossible de démarrer OpenRGB : {e}")))
}

enum SessionEnd {
    Shutdown,
    Restart,
    Lost(String),
}

/// How a managed controller receives TuxRGB's colours.
enum Drive {
    /// Direct / Custom / per-LED Static mode: one colour per LED, every frame.
    PerLed,
    /// Mode with a single, mode-wide colour (e.g. "Static" on simple RAM or
    /// fan controllers): the average colour of the device is written to the
    /// mode, at most every [`MODE_COLOR_INTERVAL`].
    ModeColor {
        mode: ModeData,
        last: Option<Instant>,
    },
}

/// Every managed LED to black. Requests share one SDK connection, so they
/// go one after the other; each is bounded to 500 ms and the whole to 3 s:
/// a stuck SMBus device must never hold a system shutdown back.
async fn blackout(managed: &Managed) {
    let started = Instant::now();
    for c in managed.group.iter() {
        if !managed.active.contains_key(&(c.id() as u32)) {
            continue;
        }
        if started.elapsed() > Duration::from_secs(3) {
            tracing::warn!("blackout budget exceeded, remaining devices skipped");
            break;
        }
        let _ = tokio::time::timeout(Duration::from_millis(500), c.turn_off_leds()).await;
    }
    tracing::info!(
        ms = started.elapsed().as_millis() as u64,
        "all LEDs turned off"
    );
}

/// Windows: OpenRGB cannot be asked to quit (no Ctrl+C for a program
/// without console), it is ended right after the last frame. Before that,
/// a request answered after the blackout proves the server has handled it,
/// and its device threads get a moment to write black to the hardware
/// (otherwise the LEDs kept their colours when quitting). On Linux the
/// server stops gracefully (SIGINT) and nothing waits here.
#[cfg_attr(not(windows), allow(unused_variables))]
async fn settle(client: &mut OpenRgbClient, protocol: u32, managed: &Managed) {
    #[cfg(windows)]
    {
        // Black written twice, see `windows_settle`.
        windows_settle(client, protocol, managed).await;
    }
}

/// Device list watch once the start-up detection is over (a cheap probe,
/// see [`ListProbe`]), and the full comparison done every [`FULL_CHECK`].
const SLOW_POLL: Duration = Duration::from_secs(5);
const FULL_CHECK: Duration = Duration::from_secs(60);

/// Mode-colour updates are slow on SMBus devices: cap their rate. A colour
/// held back by this cap is written as soon as it is due (see
/// `Managed::deferred_until`), so the last value of a slider always lands.
const MODE_COLOR_INTERVAL: Duration = Duration::from_millis(100);

struct Managed {
    group: ControllerGroup,
    /// SDK ids of controllers TuxRGB drives, and how.
    active: BTreeMap<u32, Target>,
    /// Colours last written to each per-LED device (buffers reused).
    last_sent: Vec<(u32, Vec<openrgb2::Color>)>,
    /// A single-colour device has a newer colour waiting for its rate cap.
    deferred_until: Option<Instant>,
    /// Frame devices to write this frame (indices, reused buffer).
    pending: Vec<usize>,
}

/// A controller TuxRGB drives.
struct Target {
    drive: Drive,
    /// Its LED count when it was loaded: a frame built for another count
    /// (device list changed, zone resized, not yet seen by the GUI) is
    /// skipped instead of being written to the wrong LEDs.
    leds: usize,
}

/// Ids of the server's controllers: cheap to ask for, to notice
/// hot-plugged devices (count only before SDK v6).
#[derive(Clone, Debug, PartialEq, Eq)]
enum ListProbe {
    Ids(Vec<u32>),
    Count(u32),
}

async fn probe_list(client: &mut OpenRgbClient, protocol: u32) -> Result<ListProbe, String> {
    let probe = async {
        if protocol >= 6 {
            client.get_controller_ids().await.map(ListProbe::Ids)
        } else {
            client.get_controller_count().await.map(ListProbe::Count)
        }
    };
    match tokio::time::timeout(Duration::from_secs(5), probe).await {
        Ok(Ok(p)) => Ok(p),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("délai dépassé".into()),
    }
}

fn probe_of(group: &ControllerGroup, protocol: u32) -> ListProbe {
    if protocol >= 6 {
        ListProbe::Ids(group.iter().map(|c| c.id() as u32).collect())
    } else {
        ListProbe::Count(group.iter().count() as u32)
    }
}

/// Resolves when a held-back colour is due (never if there is none).
async fn deferred(until: Option<Instant>) {
    match until {
        Some(t) => tokio::time::sleep_until(t.into()).await,
        None => std::future::pending().await,
    }
}

/// Longest wait for the colours in flight before touching the controllers
/// (reload, zone resize): a slow SMBus / I2C write finishes well before.
const DRAIN: Duration = Duration::from_secs(5);
/// A controller with a packet unacknowledged this long: server stuck.
const PIPE_STALL: Duration = Duration::from_secs(20);
/// Writes slower than this are reported once in the log (hardware reports).
const SLOW_WRITE: Duration = Duration::from_millis(150);

/// Colours on a connection of their own (SDK v6, see `openrgb2::LedPipe`).
///
/// The server acknowledges a packet once the controller's write in
/// progress is done. Waiting for every acknowledge before the next frame
/// tied all the devices to the slowest one: with SMBus RAM or an I2C
/// graphics card, a mouse or a fan hub was updated once every few seconds
/// (and a two-second wait was even taken for a lost server). Here each
/// controller has at most one packet in flight and gets the latest colours
/// as soon as it is free: fast devices run at the engine's frame rate,
/// slow ones as fast as their bus allows, without a queue on the server.
struct Pipeline {
    pipe: LedPipe,
    /// Controllers with packets not acknowledged yet: since when, how many
    /// (one, except during a blackout).
    in_flight: BTreeMap<u32, (Instant, u32)>,
    /// Controllers that missed a frame while busy.
    dirty: BTreeSet<u32>,
    /// Slow controllers already reported in the log.
    reported: BTreeSet<u32>,
}

impl Pipeline {
    async fn connect(addr: (&str, u16), name: &str) -> Result<Self, String> {
        let pipe = tokio::time::timeout(Duration::from_secs(5), LedPipe::connect(addr, name))
            .await
            .map_err(|_| "délai dépassé".to_owned())?
            .map_err(|e| e.to_string())?;
        Ok(Self {
            pipe,
            in_flight: BTreeMap::new(),
            dirty: BTreeSet::new(),
            reported: BTreeSet::new(),
        })
    }

    /// Notes the next frame for a busy controller; `true` if it is free.
    fn claim(&mut self, id: u32) -> bool {
        if self.in_flight.contains_key(&id) {
            self.dirty.insert(id);
            false
        } else {
            true
        }
    }

    fn note_sent(&mut self, id: u32) {
        self.in_flight
            .entry(id)
            .and_modify(|(_, n)| *n += 1)
            .or_insert((Instant::now(), 1));
    }

    async fn send_leds(&mut self, id: u32, colors: &[openrgb2::Color]) -> Result<(), String> {
        self.note_sent(id);
        self.pipe
            .update_leds(id, colors)
            .await
            .map_err(|e| e.to_string())
    }

    async fn send_mode(&mut self, id: u32, mode: &ModeData) -> Result<(), String> {
        self.note_sent(id);
        self.pipe
            .update_mode(id, mode)
            .await
            .map_err(|e| e.to_string())
    }

    /// Records an acknowledge; `true` if the controller missed a frame.
    fn acked(&mut self, ack: PipeAck) -> bool {
        let id = ack.controller_id;
        let done = match self.in_flight.get_mut(&id) {
            Some((_, n)) if *n > 1 => {
                *n -= 1;
                None
            }
            Some(_) => self.in_flight.remove(&id).map(|(sent, _)| sent),
            None => None,
        };
        if let Some(sent) = done {
            let took = sent.elapsed();
            if took >= SLOW_WRITE && self.reported.insert(id) {
                tracing::info!(
                    controller = id,
                    ms = took.as_millis() as u64,
                    "slow controller: updated at its own pace, the others are not held back"
                );
            }
        }
        if !ack.ok {
            tracing::debug!(controller = id, "update refused by the server");
        }
        self.dirty.remove(&id)
    }

    /// Waits for the packets in flight (at most `budget`).
    async fn drain(&mut self, budget: Duration) {
        let deadline = tokio::time::Instant::now() + budget;
        while !self.in_flight.is_empty() {
            match tokio::time::timeout_at(deadline, self.pipe.next_ack()).await {
                Ok(Some(Ok(ack))) => {
                    self.acked(ack);
                }
                Ok(_) => break,
                Err(_) => {
                    tracing::warn!(
                        pending = self.in_flight.len(),
                        "colour updates still unacknowledged"
                    );
                    break;
                }
            }
        }
        self.in_flight.clear();
        self.dirty.clear();
    }

    /// A controller waiting for its acknowledge for too long.
    fn stalled(&self) -> Option<u32> {
        self.in_flight
            .iter()
            .find(|(_, (t, _))| t.elapsed() > PIPE_STALL)
            .map(|(id, _)| *id)
    }
}

/// Next acknowledge of the colour connection (never, without one).
async fn next_ack(pipeline: &mut Option<Pipeline>) -> Option<openrgb2::OpenRgbResult<PipeAck>> {
    match pipeline {
        Some(p) => p.pipe.next_ack().await,
        None => std::future::pending().await,
    }
}

/// Every managed LED to black through the colour connection: all the
/// controllers at once (behind the colours already in flight, in order),
/// each acknowledged after its write in progress.
async fn blackout_pipe(managed: &mut Managed, p: &mut Pipeline) {
    let started = Instant::now();
    let black = openrgb2::Color { r: 0, g: 0, b: 0 };
    for (id, target) in managed.active.iter_mut() {
        let sent = match &mut target.drive {
            Drive::PerLed => p.send_leds(*id, &vec![black; target.leds]).await,
            Drive::ModeColor { mode, last } => {
                mode.set_colors(&[black]);
                *last = Some(Instant::now());
                p.send_mode(*id, mode).await
            }
        };
        if let Err(e) = sent {
            tracing::warn!("blackout: {e}");
            break;
        }
    }
    p.drain(Duration::from_millis(3500)).await;
    managed.last_sent.clear();
    tracing::info!(
        ms = started.elapsed().as_millis() as u64,
        "all LEDs turned off"
    );
}

/// An acknowledge only says the server took the colours: the controller's
/// own thread writes them to the hardware afterwards. Right after a
/// blackout the system may sleep or power off (Linux and Windows), or
/// OpenRGB be ended (Windows): a slow controller – often the motherboard
/// with its ARGB fans – could keep its colours. Black is written a second
/// time, once the controller threads have picked up the first one: the
/// server acknowledges it only after their write in progress, so every
/// first black write is done when the second pass is confirmed.
async fn settle_pipe(managed: &mut Managed, p: &mut Pipeline) {
    tokio::time::sleep(Duration::from_millis(50)).await;
    blackout_pipe(managed, p).await;
    #[cfg(windows)]
    tokio::time::sleep(Duration::from_millis(150)).await;
}

/// Frame to the controllers through the colour connection (`only`: one
/// controller that just became free).
fn push_frame_pipe<'a>(
    managed: &'a mut Managed,
    p: &'a mut Pipeline,
    frame: &'a Frame,
    only: Option<u32>,
) -> BoxFut<'a, Result<(), String>> {
    Box::pin(async move {
        for dev in &frame.devices {
            if only.is_some_and(|id| id != dev.id) {
                continue;
            }
            let Some(target) = managed.active.get_mut(&dev.id) else {
                continue;
            };
            if dev.colors.len() != target.leds {
                continue;
            }
            match &mut target.drive {
                Drive::ModeColor { mode, last } => {
                    let avg = to_color(average(&dev.colors));
                    if mode.colors().first() == Some(&avg) || !p.claim(dev.id) {
                        continue;
                    }
                    if let Some(t) = last.filter(|t| t.elapsed() < MODE_COLOR_INTERVAL) {
                        let due = t + MODE_COLOR_INTERVAL;
                        managed.deferred_until =
                            Some(managed.deferred_until.map_or(due, |d| d.min(due)));
                        continue;
                    }
                    mode.set_colors(&[avg]);
                    *last = Some(Instant::now());
                    p.send_mode(dev.id, mode).await?;
                }
                Drive::PerLed => {
                    let slot = match managed.last_sent.iter().position(|(id, _)| *id == dev.id) {
                        Some(i) => i,
                        None => {
                            managed.last_sent.push((dev.id, Vec::new()));
                            managed.last_sent.len() - 1
                        }
                    };
                    let prev = &managed.last_sent[slot].1;
                    let unchanged = prev.len() == dev.colors.len()
                        && prev
                            .iter()
                            .zip(&dev.colors)
                            .all(|(a, c)| *a == to_color(*c));
                    if unchanged || !p.claim(dev.id) {
                        continue;
                    }
                    let buf = &mut managed.last_sent[slot].1;
                    buf.clear();
                    buf.extend(dev.colors.iter().map(|c| to_color(*c)));
                    p.send_leds(dev.id, buf).await?;
                }
            }
        }
        Ok(())
    })
}

/// The bridge runs on its own thread with a current-thread runtime (see
/// [`spawn_thread`]), so its futures do not need to be `Send`.
type BoxFut<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + 'a>>;

fn session<'a>(
    mut client: OpenRgbClient,
    addr: (&'a str, u16),
    config: &'a mut BridgeConfig,
    server_label: &'a str,
    frames: &'a mut watch::Receiver<Arc<Frame>>,
    commands: &'a mut mpsc::UnboundedReceiver<BridgeCommand>,
    send: &'a Emitter,
) -> BoxFut<'a, SessionEnd> {
    Box::pin(async move {
        let _ = client.set_name(config.client_name.clone()).await;
        let protocol = client.get_protocol_version();
        tracing::info!(protocol, "connected to OpenRGB SDK");

        let mut managed = match load(&client, config).await {
            Ok(m) => m,
            Err(e) => return SessionEnd::Lost(e),
        };
        publish_devices(&managed, config, protocol, server_label, send);
        // SDK v6: colours go through a connection of their own, every
        // controller at its own pace (see `Pipeline`). Older servers do not
        // acknowledge packets: frames are written as before.
        // After a blackout (sleep, shutdown) the LEDs stay dark until the
        // resume (`Reapply`): a frame the engine rendered just before it
        // turned its output off may reach the bridge after the blackout
        // and must not light a device again while the system goes down.
        let mut dark = false;
        let mut pipeline = if protocol >= 6 {
            match Pipeline::connect(addr, &config.client_name).await {
                Ok(p) => Some(p),
                Err(e) => return SessionEnd::Lost(e),
            }
        } else {
            None
        };

        let session_start = Instant::now();
        let mut last_print = fingerprint(&managed.group);
        let mut last_probe = probe_of(&managed.group, protocol);
        let mut last_full_check = Instant::now();
        let mut poll = tokio::time::interval(Duration::from_secs(1));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // A notification read before the first load is covered by it.
        let _ = client.take_device_list_updated();
        // Force sending the current frame right away.
        frames.mark_changed();

        loop {
            tokio::select! {
                biased;
                cmd = commands.recv() => match cmd {
                    None => return SessionEnd::Shutdown,
                    Some(BridgeCommand::Shutdown { turn_off }) => {
                        if turn_off {
                            match pipeline.as_mut() {
                                Some(p) => {
                                    blackout_pipe(&mut managed, p).await;
                                    settle_pipe(&mut managed, p).await;
                                }
                                None => {
                                    blackout(&managed).await;
                                    settle(&mut client, protocol, &managed).await;
                                }
                            }
                        }
                        return SessionEnd::Shutdown;
                    }
                    Some(BridgeCommand::Blackout(done)) => {
                        match pipeline.as_mut() {
                            Some(p) => {
                                blackout_pipe(&mut managed, p).await;
                                settle_pipe(&mut managed, p).await;
                            }
                            None => {
                                blackout(&managed).await;
                                settle(&mut client, protocol, &managed).await;
                            }
                        }
                        managed.last_sent.clear();
                        managed.deferred_until = None;
                        dark = true;
                        if let Some(n) = done {
                            n.notify_one();
                        }
                    }
                    Some(BridgeCommand::Reapply) => {
                        dark = false;
                        if let Some(p) = pipeline.as_mut() { p.drain(DRAIN).await; }
                        managed = match load(&client, config).await {
                            Ok(m) => m,
                            Err(e) => return SessionEnd::Lost(e),
                        };
                        last_print = fingerprint(&managed.group);
                        frames.mark_changed();
                    }
                    Some(BridgeCommand::Restart) => return SessionEnd::Restart,
                    Some(BridgeCommand::SetMode(m)) => {
                        config.mode = m;
                        return SessionEnd::Restart;
                    }
                    Some(BridgeCommand::ResizeZones(list)) => {
                        if let Some(p) = pipeline.as_mut() { p.drain(DRAIN).await; }
                        for r in &list {
                            resize_zone(&managed.group, &r.device, r.zone, r.leds).await;
                        }
                        // The controller layout changed: read everything again.
                        managed = match load(&client, config).await {
                            Ok(m) => m,
                            Err(e) => return SessionEnd::Lost(e),
                        };
                        last_print = fingerprint(&managed.group);
                        publish_devices(&managed, config, protocol, server_label, send);
                        frames.mark_changed();
                    }
                    Some(BridgeCommand::SetDisabled(d)) => {
                        config.disabled = d;
                        if let Some(p) = pipeline.as_mut() { p.drain(DRAIN).await; }
                        managed = match load(&client, config).await {
                            Ok(m) => m,
                            Err(e) => return SessionEnd::Lost(e),
                        };
                        publish_devices(&managed, config, protocol, server_label, send);
                        frames.mark_changed();
                    }
                    Some(BridgeCommand::Rescan) => {
                        if let Err(e) = client.rescan_devices().await {
                            tracing::warn!("rescan failed: {e}");
                        }
                        send.emit(BridgeEvent::Status(BridgeStatus::Detecting { seconds: 0 }));
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        if let Some(p) = pipeline.as_mut() { p.drain(DRAIN).await; }
                        managed = match load(&client, config).await {
                            Ok(m) => m,
                            Err(e) => return SessionEnd::Lost(e),
                        };
                        last_print = fingerprint(&managed.group);
                        publish_devices(&managed, config, protocol, server_label, send);
                        frames.mark_changed();
                    }
                },
                ack = next_ack(&mut pipeline) => {
                    let Some(p) = pipeline.as_mut() else { continue };
                    let ack = match ack {
                        Some(Ok(a)) => a,
                        Some(Err(e)) => return SessionEnd::Lost(e.to_string()),
                        None => return SessionEnd::Lost("connexion des couleurs fermée".into()),
                    };
                    // This controller was busy when a newer frame came:
                    // it gets the latest colours now.
                    if p.acked(ack) && !dark {
                        let frame = frames.borrow().clone();
                        if let Err(e) = push_frame_pipe(&mut managed, p, &frame, Some(ack.controller_id)).await {
                            return SessionEnd::Lost(e);
                        }
                    }
                }
                _ = deferred(managed.deferred_until) => {
                    managed.deferred_until = None;
                    frames.mark_changed();
                }
                changed = frames.changed() => {
                    if changed.is_err() {
                        return SessionEnd::Shutdown;
                    }
                    let frame = frames.borrow_and_update().clone();
                    if dark {
                        continue;
                    }
                    let pushed = match pipeline.as_mut() {
                        Some(p) => push_frame_pipe(&mut managed, p, &frame, None).await,
                        None => push_frame(&mut managed, &frame, protocol).await,
                    };
                    if let Err(e) = pushed {
                        return SessionEnd::Lost(e);
                    }
                    drop(frame);
                    // The server announced a new device list (hot-plug)
                    // while acknowledging the frame: reload now.
                    let announced = client.take_device_list_updated()
                        || pipeline.as_ref().is_some_and(|p| p.pipe.take_list_updated());
                    if announced {
                        poll.reset_immediately();
                    }
                }
                _ = poll.tick() => {
                    // Detection runs in the background on the server: devices
                    // appear progressively. Poll quickly at first, then slowly.
                    let fast = session_start.elapsed() < Duration::from_secs(45);
                    if !fast && poll.period() < SLOW_POLL {
                        poll = tokio::time::interval(SLOW_POLL);
                        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        poll.tick().await;
                    }
                    // Cheap probe (controller ids) every tick; the full
                    // controller data only when it or the server says the
                    // list changed, and once a minute as a safety net.
                    // A controller that never acknowledges: the server is stuck.
                    if let Some(id) = pipeline.as_ref().and_then(Pipeline::stalled) {
                        return SessionEnd::Lost(format!(
                            "le serveur OpenRGB ne répond plus (contrôleur {id})"
                        ));
                    }
                    let announced = client.take_device_list_updated()
                        || pipeline.as_ref().is_some_and(|p| p.pipe.take_list_updated());
                    let probe = match probe_list(&mut client, protocol).await {
                        Ok(p) => p,
                        Err(e) => return SessionEnd::Lost(e),
                    };
                    let due = last_full_check.elapsed() >= FULL_CHECK;
                    if !announced && !due && probe == last_probe {
                        continue;
                    }
                    last_probe = probe;
                    last_full_check = Instant::now();
                    let fresh = match tokio::time::timeout(Duration::from_secs(5), client.get_all_controllers()).await {
                        Ok(Ok(g)) => fingerprint(&g),
                        Ok(Err(e)) => return SessionEnd::Lost(e.to_string()),
                        Err(_) => return SessionEnd::Lost("délai dépassé".into()),
                    };
                    if fresh != last_print {
                        tracing::info!(count = fresh.len(), "device list changed");
                        if let Some(p) = pipeline.as_mut() { p.drain(DRAIN).await; }
                        managed = match load(&client, config).await {
                            Ok(m) => m,
                            Err(e) => return SessionEnd::Lost(e),
                        };
                        last_print = fingerprint(&managed.group);
                        last_probe = probe_of(&managed.group, protocol);
                        publish_devices(&managed, config, protocol, server_label, send);
                        frames.mark_changed();
                    }
                }
            }
        }
    })
}

/// Resizes one zone of the controller identified by `device`.
async fn resize_zone(group: &ControllerGroup, device: &DeviceKey, zone: usize, leds: usize) {
    let Some(c) = group.iter().find(|c| device_key(c) == *device) else {
        tracing::warn!("resize: device {device:?} not found");
        return;
    };
    let Ok(z) = c.get_zone(zone) else {
        tracing::warn!("resize: {} has no zone {zone}", c.name());
        return;
    };
    let size = leds.clamp(z.leds_min(), z.leds_max());
    tracing::info!(
        "{} / {}: {} -> {size} LED",
        c.name(),
        z.name(),
        z.num_leds()
    );
    match tokio::time::timeout(Duration::from_secs(5), z.resize(size)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("resize refused: {e}"),
        Err(_) => tracing::warn!("resize: no answer from OpenRGB"),
    }
}

/// Loads every controller and switches the managed ones to a software
/// controllable mode (Direct, Custom or Static – OpenRGB's own order).
fn load<'a>(
    client: &'a OpenRgbClient,
    config: &'a BridgeConfig,
) -> BoxFut<'a, Result<Managed, String>> {
    Box::pin(async move {
        let group = tokio::time::timeout(Duration::from_secs(20), client.get_all_controllers())
            .await
            .map_err(|_| "délai dépassé pendant la lecture des périphériques".to_owned())?
            .map_err(|e| e.to_string())?;
        let mut active = BTreeMap::new();
        for c in group.iter() {
            let key = device_key(c);
            log_controller(c);
            if config.disabled.contains(&key) || c.num_leds() == 0 {
                continue;
            }
            let Some(mode) = c.controllable_mode().cloned() else {
                continue;
            };
            // `set_controllable_mode` can panic on malformed controller data;
            // isolate it so one exotic device cannot break the session.
            let r = AssertUnwindSafe(c.set_controllable_mode())
                .catch_unwind()
                .await;
            match r {
                Ok(Ok(())) => {
                    let drive = if mode.color_mode() == ColorMode::ModeSpecific
                        && !mode.colors().is_empty()
                    {
                        let mut mode = mode;
                        if let Some(max) = mode.brightness_max() {
                            mode.set_brightness(max);
                        }
                        Drive::ModeColor { mode, last: None }
                    } else {
                        Drive::PerLed
                    };
                    active.insert(
                        c.id() as u32,
                        Target {
                            drive,
                            leds: c.num_leds(),
                        },
                    );
                }
                Ok(Err(e)) => tracing::warn!("{}: mode {} refused: {e}", c.name(), mode.name()),
                Err(_) => tracing::warn!("{}: invalid mode data, skipped", c.name()),
            }
        }
        Ok(Managed {
            group,
            active,
            last_sent: Vec::new(),
            deferred_until: None,
            pending: Vec::new(),
        })
    })
}

fn fingerprint(group: &ControllerGroup) -> Vec<(usize, String, usize)> {
    group
        .iter()
        .map(|c| {
            (
                c.id(),
                format!("{}|{}", c.name(), c.location()),
                c.num_leds(),
            )
        })
        .collect()
}

/// One line per controller and zone in the log (helps hardware reports).
fn log_controller(c: &Controller) {
    let mode = c
        .controllable_mode()
        .map(|m| format!("{} ({:?})", m.name(), m.color_mode()))
        .unwrap_or_else(|| "aucun mode pilotable".into());
    tracing::info!(
        "controller {} \"{}\" [{}] {:?} at {} – {} LED – mode {mode}",
        c.id(),
        c.name(),
        c.vendor(),
        c.device_type(),
        c.location(),
        c.num_leds()
    );
    for z in c.get_all_zones() {
        tracing::info!(
            "  zone {} \"{}\" {:?}: {} LED (min {}, max {})",
            z.zone_id(),
            z.name(),
            z.zone_type(),
            z.num_leds(),
            z.leds_min(),
            z.leds_max()
        );
    }
}

fn can_drive(c: &Controller) -> bool {
    c.num_leds() > 0 && c.controllable_mode().is_some()
}

fn device_key(c: &Controller) -> DeviceKey {
    DeviceKey::new(c.vendor(), c.name(), c.location(), c.serial())
}

fn convert(c: &Controller, managed: &Managed) -> DeviceInfo {
    let zones = c
        .get_all_zones()
        .map(|z| ZoneInfo {
            name: z.name().to_owned(),
            kind: match z.zone_type() {
                ZoneType::Single => ZoneKind::Single,
                ZoneType::Linear | ZoneType::Segmented => ZoneKind::Linear,
                ZoneType::LinearLoop => ZoneKind::Ring,
                ZoneType::Matrix | ZoneType::MatrixLoopX | ZoneType::MatrixLoopY => {
                    ZoneKind::Matrix
                }
            },
            led_count: z.num_leds(),
            matrix: None,
            leds_min: z.leds_min(),
            leds_max: z.leds_max(),
        })
        .collect();
    let id = c.id() as u32;
    let mode = c.controllable_mode();
    DeviceInfo {
        id: c.id() as u32,
        key: device_key(c),
        name: c.name().to_owned(),
        vendor: c.vendor().to_owned(),
        description: c.description().to_owned(),
        location: c.location().to_owned(),
        serial: c.serial().to_owned(),
        kind: match c.device_type() {
            DeviceType::Motherboard => DeviceKind::Motherboard,
            DeviceType::DRam => DeviceKind::Dram,
            DeviceType::Gpu => DeviceKind::Gpu,
            DeviceType::Cooler => DeviceKind::Cooler,
            DeviceType::LEDStrip => DeviceKind::LedStrip,
            DeviceType::Keyboard => DeviceKind::Keyboard,
            DeviceType::Mouse => DeviceKind::Mouse,
            DeviceType::MouseMat => DeviceKind::MouseMat,
            DeviceType::Headset => DeviceKind::Headset,
            DeviceType::HeadsetStand => DeviceKind::HeadsetStand,
            DeviceType::Gamepad => DeviceKind::Gamepad,
            DeviceType::Light => DeviceKind::Light,
            DeviceType::Speaker => DeviceKind::Speaker,
            DeviceType::Virtual => DeviceKind::Virtual,
            DeviceType::Storage => DeviceKind::Storage,
            DeviceType::Case => DeviceKind::Case,
            DeviceType::Microphone => DeviceKind::Microphone,
            DeviceType::Accessory => DeviceKind::Accessory,
            DeviceType::Keypad => DeviceKind::Keypad,
            DeviceType::Laptop => DeviceKind::Laptop,
            DeviceType::Monitor => DeviceKind::Monitor,
            DeviceType::Unknown => DeviceKind::Unknown,
        },
        zones,
        led_count: c.num_leds(),
        controllable: managed.active.contains_key(&id) || can_drive(c),
        single_color: match managed.active.get(&id).map(|t| &t.drive) {
            Some(Drive::ModeColor { .. }) => true,
            Some(Drive::PerLed) => false,
            None => mode.is_some_and(|m| m.color_mode() == ColorMode::ModeSpecific),
        },
        mode: mode.map(|m| m.name().to_owned()).unwrap_or_default(),
        is_virtual: false,
    }
}

fn publish_devices(
    managed: &Managed,
    config: &BridgeConfig,
    protocol: u32,
    server_label: &str,
    send: &Emitter,
) {
    let devices: Vec<DeviceInfo> = managed.group.iter().map(|c| convert(c, managed)).collect();
    let controllable = devices
        .iter()
        .filter(|d| d.controllable && !config.disabled.contains(&d.key))
        .count();
    send.emit(BridgeEvent::Status(BridgeStatus::Connected {
        devices: devices.len(),
        controllable,
        protocol,
        server: server_label.to_owned(),
    }));
    send.emit(BridgeEvent::Devices(devices));
}

fn push_frame<'a>(
    managed: &'a mut Managed,
    frame: &'a Frame,
    protocol: u32,
) -> BoxFut<'a, Result<(), String>> {
    Box::pin(async move {
        // No allocation per frame: the devices to write are noted by index
        // in a reused list, their colours are converted while being sent
        // and copied into the reused `last_sent` buffers once written.
        let mut pending = std::mem::take(&mut managed.pending);
        pending.clear();
        let mut mode_updates: Option<Vec<(u32, openrgb2::Color)>> = None;
        for (index, dev) in frame.devices.iter().enumerate() {
            let Some(target) = managed.active.get(&dev.id) else {
                continue;
            };
            if dev.colors.len() != target.leds {
                // Built for another layout of this device: the GUI sends a
                // matching scene right after the device list update.
                continue;
            }
            if let Drive::ModeColor { mode, last } = &target.drive {
                let avg = to_color(average(&dev.colors));
                if mode.colors().first() != Some(&avg) {
                    match last.filter(|t| t.elapsed() < MODE_COLOR_INTERVAL) {
                        None => mode_updates.get_or_insert_default().push((dev.id, avg)),
                        Some(t) => {
                            let due = t + MODE_COLOR_INTERVAL;
                            managed.deferred_until =
                                Some(managed.deferred_until.map_or(due, |d| d.min(due)));
                        }
                    }
                }
                continue;
            }
            // Skip devices whose colours did not change since the last frame.
            let unchanged = managed
                .last_sent
                .iter()
                .find(|(id, _)| *id == dev.id)
                .is_some_and(|(_, prev)| {
                    prev.len() == dev.colors.len()
                        && prev
                            .iter()
                            .zip(&dev.colors)
                            .all(|(p, c)| *p == to_color(*c))
                });
            if !unchanged {
                pending.push(index);
            }
        }
        // Single-colour devices: write the mode colour.
        for (id, color) in mode_updates.into_iter().flatten() {
            let Some(Target {
                drive: Drive::ModeColor { mode, last },
                ..
            }) = managed.active.get_mut(&id)
            else {
                continue;
            };
            mode.set_colors(&[color]);
            *last = Some(Instant::now());
            let Some(c) = managed.group.iter().find(|c| c.id() as u32 == id) else {
                continue;
            };
            match tokio::time::timeout(Duration::from_secs(2), c.apply_mode(mode)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(e.to_string()),
                Err(_) => return Err("le serveur OpenRGB ne répond plus".into()),
            }
        }
        if pending.is_empty() {
            managed.pending = pending;
            return Ok(());
        }
        let colors_of = |i: usize| frame.devices[i].colors.iter().map(|c| to_color(*c));
        let result = if protocol >= 6 {
            // Protocol v6 acknowledges every packet: pipeline them in one batch.
            let mut cmd = managed.group.cmd();
            for &i in &pending {
                let id = frame.devices[i].id;
                if let Err(e) = cmd.set_controller_leds(id as usize, colors_of(i)) {
                    tracing::debug!("skip controller {id}: {e}");
                }
            }
            tokio::time::timeout(Duration::from_secs(2), cmd.execute()).await
        } else {
            // Older servers do not acknowledge; send controller by controller.
            let group = &managed.group;
            tokio::time::timeout(Duration::from_secs(2), async {
                for &i in &pending {
                    let id = frame.devices[i].id;
                    if let Some(c) = group.iter().find(|c| c.id() as u32 == id) {
                        c.set_leds(colors_of(i).collect::<Vec<_>>()).await?;
                    }
                }
                Ok(())
            })
            .await
        };
        let outcome = match result {
            Ok(Ok(())) => {
                for &i in &pending {
                    let dev = &frame.devices[i];
                    match managed.last_sent.iter_mut().find(|(id, _)| *id == dev.id) {
                        Some((_, slot)) => {
                            slot.clear();
                            slot.extend(colors_of(i));
                        }
                        None => managed.last_sent.push((dev.id, colors_of(i).collect())),
                    }
                }
                Ok(())
            }
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("le serveur OpenRGB ne répond plus".into()),
        };
        managed.pending = pending;
        outcome
    })
}

/// Average colour of a device (single-colour controllers).
fn average(colors: &[Rgb]) -> Rgb {
    if colors.is_empty() {
        return Rgb::BLACK;
    }
    let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
    for c in colors {
        r += c.r as u32;
        g += c.g as u32;
        b += c.b as u32;
    }
    let n = colors.len() as u32;
    Rgb::new((r / n) as u8, (g / n) as u8, (b / n) as u8)
}

#[inline]
fn to_color(c: Rgb) -> openrgb2::Color {
    openrgb2::Color {
        r: c.r,
        g: c.g,
        b: c.b,
    }
}

/// See [`settle`] (Windows build). Black is written a second time:
/// some controllers (SMBus RAM, motherboards) ignore colours that arrive
/// right after the switch to their direct mode, and a PC shutdown gives no
/// second chance. Kept at the end of the file so that no line of the code
/// shared with Linux moves.
#[cfg(windows)]
async fn windows_settle(client: &mut OpenRgbClient, protocol: u32, managed: &Managed) {
    let _ = tokio::time::timeout(Duration::from_millis(800), probe_list(client, protocol)).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    blackout(managed).await;
    let _ = tokio::time::timeout(Duration::from_millis(800), probe_list(client, protocol)).await;
    tokio::time::sleep(Duration::from_millis(350)).await;
}
