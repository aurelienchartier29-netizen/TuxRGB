//! Background services, running for the whole life of the process (even
//! with no window open):
//!
//! * the effect engine thread,
//! * the OpenRGB bridge (embedded server + SDK client),
//! * the tray icon,
//! * the desktop appearance watcher,
//! * audio / screen capture (only while a reactive effect is active),
//! * the throttled preview feed for the GUI (only while a window is open).
//!
//! It is exposed to iced as a single subscription stream; the GUI talks back
//! through the [`ServiceHandle`] delivered in [`ServiceEvent::Ready`].

use crate::theme::kde::{self, SystemAppearance};
use crate::tray::{self, TrayAction, TrayState};
use iced::futures::{SinkExt, Stream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tuxrgb_capture::CaptureError;
use tuxrgb_capture::audio::AudioCapture;
use tuxrgb_capture::hardware::HardwareMonitor;
use tuxrgb_capture::screen::ScreenCapture;
use tuxrgb_capture::shader::{ShaderRenderer, ShaderSettings};
use tuxrgb_core::engine::{self, EngineCommand, EngineConfig, EngineStats, Frame};
use tuxrgb_core::{EffectKind, Rgb, SharedInputs};
use tuxrgb_openrgb::{BridgeCommand, BridgeConfig, BridgeEvent};

#[derive(Clone)]
pub struct ServiceHandle {
    commands: mpsc::UnboundedSender<ServiceCommand>,
    engine: crossbeam_channel::Sender<EngineCommand>,
    pub stats: Arc<EngineStats>,
}

impl std::fmt::Debug for ServiceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServiceHandle")
    }
}

impl ServiceHandle {
    pub fn send(&self, cmd: ServiceCommand) {
        let _ = self.commands.send(cmd);
    }

    pub fn engine(&self, cmd: EngineCommand) {
        let _ = self.engine.try_send(cmd);
    }

    pub fn bridge(&self, cmd: BridgeCommand) {
        self.send(ServiceCommand::Bridge(cmd));
    }
}

pub struct StartConfig {
    pub bridge: BridgeConfig,
    pub tray: TrayState,
}

pub enum ServiceCommand {
    Start(Box<StartConfig>),
    Bridge(BridgeCommand),
    /// Enables the ≤30 Hz preview feed (window open).
    SetPreview(bool),
    Tray(TrayState),
    Capture(CaptureRequest),
    /// Desktop notification (freedesktop): "Open" plus, optionally, the
    /// issue's own action (`(key, label)`, reported back when clicked).
    Notify {
        summary: String,
        body: String,
        action: Option<(String, String)>,
    },
    Shutdown {
        turn_off: bool,
    },
}

/// Which inputs the active effect needs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptureRequest {
    pub audio: bool,
    pub screen: bool,
    pub hardware: bool,
    pub shader: Option<ShaderSettings>,
    pub screencast_token: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CaptureStatus {
    Off,
    Starting,
    Running,
    Failed(String),
    Cancelled,
}

/// A preview frame for the GUI, with the soft canvas background already
/// built from it (off the interface thread) for the given theme darkness.
#[derive(Clone, Debug)]
pub struct PreviewFrame {
    pub frame: Arc<Frame>,
    pub backdrop: Option<(bool, iced::widget::image::Handle)>,
}

#[derive(Clone, Debug)]
pub enum ServiceEvent {
    Ready(ServiceHandle),
    Bridge(BridgeEvent),
    Preview(PreviewFrame),
    Appearance(SystemAppearance),
    Tray(TrayAction),
    TrayAvailable(bool),
    Audio(CaptureStatus),
    Screen(CaptureStatus),
    Shader(CaptureStatus),
    ScreencastToken(Option<String>),
    /// A second launch asked this instance to show itself.
    Activate(crate::instance::ActivateRequest),
    /// SIGTERM / SIGINT / SIGHUP (logout, reboot, `kill`): save and stop.
    Terminate,
    /// The system is about to shut down / sleep: the LEDs are already off.
    PowerDown(PowerKind),
    /// Back from sleep: the LEDs can come back.
    Resumed,
    /// The user clicked a TuxRGB notification (`Some(key)`: its action
    /// button).
    NotificationClicked(Option<String>),
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerKind {
    Shutdown,
    Sleep,
}

/// From the logind watcher to the service loop.
enum PowerEvent {
    /// Turn everything off, then answer on `done` (releases the inhibitor).
    Before {
        kind: PowerKind,
        done: tokio::sync::oneshot::Sender<()>,
    },
    Resumed,
}

/// Holds a logind "delay" inhibitor so the system waits (≤ 5 s, logind's
/// `InhibitDelayMaxSec`) for TuxRGB to turn the LEDs off before it shuts
/// down or sleeps, and reports resume. Without logind (containers, some
/// init systems) it simply does nothing.
#[cfg(target_os = "linux")]
async fn logind_watch(power: mpsc::UnboundedSender<PowerEvent>) {
    use futures::StreamExt;
    let Ok(conn) = zbus::Connection::system().await else {
        tracing::info!("no system bus: no shutdown / sleep integration");
        return;
    };
    let proxy = match zbus::Proxy::new(
        &conn,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::info!("logind unavailable ({e}): no shutdown / sleep integration");
            return;
        }
    };
    let inhibit = || async {
        proxy
            .call::<_, _, zbus::zvariant::OwnedFd>(
                "Inhibit",
                &(
                    "shutdown:sleep",
                    "TuxRGB",
                    "Extinction de l'éclairage RGB",
                    "delay",
                ),
            )
            .await
    };
    let (Ok(mut shutdown), Ok(mut sleep)) = (
        proxy.receive_signal("PrepareForShutdown").await,
        proxy.receive_signal("PrepareForSleep").await,
    ) else {
        return;
    };
    let mut lock = match inhibit().await {
        Ok(fd) => Some(fd),
        Err(e) => {
            tracing::warn!("logind inhibitor refused: {e}");
            None
        }
    };
    tracing::info!(
        inhibitor = lock.is_some(),
        "logind shutdown / sleep integration active"
    );
    loop {
        let (kind, start) = tokio::select! {
            Some(m) = shutdown.next() => (PowerKind::Shutdown, m.body().deserialize::<bool>().unwrap_or(true)),
            Some(m) = sleep.next() => (PowerKind::Sleep, m.body().deserialize::<bool>().unwrap_or(true)),
            else => return,
        };
        if start {
            tracing::info!(?kind, "system going down: turning the LEDs off");
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = power.send(PowerEvent::Before { kind, done: tx });
            let _ = tokio::time::timeout(Duration::from_millis(4000), rx).await;
            // Releasing the inhibitor lets logind continue.
            drop(lock.take());
        } else {
            if kind == PowerKind::Sleep {
                tracing::info!("resumed from sleep");
                let _ = power.send(PowerEvent::Resumed);
            } else {
                // A shutdown that was cancelled: the LEDs come back too.
                tracing::info!("shutdown cancelled");
                let _ = power.send(PowerEvent::Resumed);
            }
            // Ready for the next shutdown or sleep.
            if lock.is_none() {
                lock = match inhibit().await {
                    Ok(fd) => Some(fd),
                    Err(e) => {
                        tracing::warn!("logind inhibitor refused: {e}");
                        None
                    }
                };
            }
        }
    }
}

/// Windows: `WM_QUERYENDSESSION` / `WM_POWERBROADCAST` of the
/// hidden window (`win::host`), turned into the same events as logind's:
/// the window procedure holds Windows back until the LEDs are dark.
#[cfg(windows)]
async fn windows_power_watch(power: mpsc::UnboundedSender<PowerEvent>) {
    use crate::win::host::{self, PowerSignal};
    let (tx, mut rx) = mpsc::unbounded_channel();
    host::set_power_sink(tx);
    let ready = tokio::task::spawn_blocking(|| host::hwnd().is_some())
        .await
        .unwrap_or(false);
    if !ready {
        tracing::info!("no Windows integration window: no shutdown / sleep integration");
        return;
    }
    tracing::info!("Windows shutdown / sleep integration active");
    while let Some(signal) = rx.recv().await {
        let (kind, answer) = match signal {
            PowerSignal::EndSession(answer) => (PowerKind::Shutdown, answer),
            PowerSignal::Suspend(answer) => (PowerKind::Sleep, answer),
            PowerSignal::EndSessionCancelled | PowerSignal::Resumed => {
                let _ = power.send(PowerEvent::Resumed);
                continue;
            }
        };
        let (done, wait) = tokio::sync::oneshot::channel();
        let _ = power.send(PowerEvent::Before { kind, done });
        let _ = tokio::time::timeout(Duration::from_millis(3800), wait).await;
        let _ = answer.send(());
    }
}

/// Windows: notifications as balloons of the tray icon (shown
/// as toasts by Windows 10/11); a click opens TuxRGB.
#[cfg(windows)]
#[derive(Clone)]
struct Notifier;

#[cfg(windows)]
impl Notifier {
    async fn new(_ev: mpsc::UnboundedSender<ServiceEvent>) -> Option<Self> {
        Some(Self)
    }

    async fn send(&self, summary: &str, body: &str, _action: Option<(String, String)>) {
        crate::tray::notify(summary, body);
    }
}

/// freedesktop notifications: sends them and reports clicks.
#[cfg(target_os = "linux")]
#[derive(Clone)]
struct Notifier {
    proxy: zbus::Proxy<'static>,
    ids: Arc<std::sync::Mutex<Vec<u32>>>,
}

#[cfg(target_os = "linux")]
impl Notifier {
    async fn new(ev: mpsc::UnboundedSender<ServiceEvent>) -> Option<Self> {
        use futures::StreamExt;
        let conn = zbus::Connection::session().await.ok()?;
        let proxy = zbus::Proxy::new(
            &conn,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
        )
        .await
        .ok()?;
        let ids = Arc::new(std::sync::Mutex::new(Vec::new()));
        if let Ok(mut clicks) = proxy.receive_signal("ActionInvoked").await {
            let ids = ids.clone();
            tokio::spawn(async move {
                while let Some(m) = clicks.next().await {
                    if let Ok((id, action)) = m.body().deserialize::<(u32, String)>()
                        && ids.lock().map(|v| v.contains(&id)).unwrap_or(false)
                    {
                        let key = action.strip_prefix("fix:").map(str::to_owned);
                        let _ = ev.send(ServiceEvent::NotificationClicked(key));
                    }
                }
            });
        }
        Some(Self { proxy, ids })
    }

    async fn send(&self, summary: &str, body: &str, action: Option<(String, String)>) {
        use std::collections::HashMap;
        use zbus::zvariant::Value;
        let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
        hints.insert("desktop-entry", Value::from(crate::paths::APP_ID));
        hints.insert("urgency", Value::from(1u8));
        let open = crate::i18n::t("Ouvrir TuxRGB");
        let fix = action.map(|(key, label)| (format!("fix:{key}"), label));
        let mut actions = vec!["default", open, "open", open];
        if let Some((key, label)) = &fix {
            actions.push(key);
            actions.push(label);
        }
        let r = self
            .proxy
            .call::<_, _, u32>(
                "Notify",
                &(
                    "TuxRGB",
                    0u32,
                    crate::paths::APP_ID,
                    summary,
                    body,
                    actions,
                    hints,
                    -1i32,
                ),
            )
            .await;
        match r {
            Ok(id) => {
                if let Ok(mut v) = self.ids.lock() {
                    v.push(id);
                }
            }
            Err(e) => tracing::info!("desktop notification not shown: {e}"),
        }
    }
}

/// Minimum spacing of tray (D-Bus) updates.
const TRAY_MIN_INTERVAL: Duration = Duration::from_millis(150);

/// Subscription entry point.
pub fn run() -> impl Stream<Item = ServiceEvent> {
    iced::stream::channel(128, async |mut output| {
        let inputs = SharedInputs::default();
        let (frame_tx, frame_rx) = watch::channel(Arc::new(Frame::default()));
        let engine = engine::spawn(
            EngineConfig {
                effect: EffectKind::RainbowWave,
                params: Arc::default(),
                brightness: 1.0,
                saturation: 1.0,
                fps: 60,
                accent: Rgb::new(61, 174, 233),
            },
            inputs.clone(),
            vec![Box::new(move |f: &Arc<Frame>| {
                frame_tx.send_replace(f.clone());
            })],
        );

        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<ServiceCommand>();
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<ServiceEvent>();
        let handle = ServiceHandle {
            commands: cmd_tx,
            engine: engine.sender(),
            stats: engine.stats.clone(),
        };
        let _ = output.send(ServiceEvent::Ready(handle)).await;

        // Second launches (single instance).
        if let Some(mut activations) = crate::instance::take_activations() {
            let ev = ev_tx.clone();
            tokio::spawn(async move {
                while let Some(r) = activations.recv().await {
                    let _ = ev.send(ServiceEvent::Activate(r));
                }
            });
        }

        // Logout / reboot / kill: the configuration is saved and the server
        // stopped cleanly instead of dying mid-write.
        #[cfg(unix)]
        {
            let ev = ev_tx.clone();
            tokio::spawn(async move {
                use tokio::signal::unix::{SignalKind, signal};
                let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
                    signal(SignalKind::terminate()),
                    signal(SignalKind::interrupt()),
                    signal(SignalKind::hangup()),
                ) else {
                    return;
                };
                tokio::select! {
                    _ = term.recv() => tracing::info!("SIGTERM received"),
                    _ = int.recv() => tracing::info!("SIGINT received"),
                    _ = hup.recv() => tracing::info!("SIGHUP received"),
                }
                let _ = ev.send(ServiceEvent::Terminate);
            });
        }

        // Windows: Ctrl+C / console closed (when started from a terminal).
        #[cfg(windows)]
        {
            let ev = ev_tx.clone();
            tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    tracing::info!("Ctrl+C received");
                    let _ = ev.send(ServiceEvent::Terminate);
                }
            });
        }

        // Shutdown / sleep (logind; Windows: WM_QUERYENDSESSION and
        // WM_POWERBROADCAST).
        let (power_tx, mut power_rx) = mpsc::unbounded_channel::<PowerEvent>();
        #[cfg(target_os = "linux")]
        tokio::spawn(logind_watch(power_tx));
        #[cfg(windows)]
        tokio::spawn(windows_power_watch(power_tx));

        // Desktop appearance (accent colour, dark mode, reduced motion).
        {
            let ev = ev_tx.clone();
            let (app_tx, mut app_rx) = mpsc::unbounded_channel();
            tokio::spawn(kde::watch(app_tx));
            tokio::spawn(async move {
                while let Some(a) = app_rx.recv().await {
                    let _ = ev.send(ServiceEvent::Appearance(a));
                }
            });
        }

        // Preview feed, only active while enabled.
        let (preview_tx, preview_rx) = watch::channel(false);
        tokio::spawn(preview_feed(frame_rx.clone(), preview_rx, ev_tx.clone()));

        // Capture manager.
        let (capture_tx, capture_rx) = mpsc::unbounded_channel();
        tokio::spawn(capture_manager(inputs.clone(), capture_rx, ev_tx.clone()));

        let mut bridge: Option<(
            mpsc::UnboundedSender<BridgeCommand>,
            std::thread::JoinHandle<()>,
        )> = None;
        let mut tray: Option<tray::Handle> = None;
        // Tray menu / tooltip updates are D-Bus signals: while a slider is
        // dragged they are coalesced (latest state wins, flushed when due).
        let mut tray_pending: Option<tray::TrayState> = None;
        let mut tray_due: Option<tokio::time::Instant> = None;
        let mut tray_sent = tokio::time::Instant::now() - TRAY_MIN_INTERVAL;
        let notifier = Notifier::new(ev_tx.clone()).await;

        loop {
            tokio::select! {
                _ = async {
                    match tray_due {
                        Some(t) => tokio::time::sleep_until(t).await,
                        None => std::future::pending().await,
                    }
                } => {
                    tray_due = None;
                    if let (Some(t), Some(state)) = (&tray, tray_pending.take()) {
                        t.update(move |tray| tray.state = state).await;
                        tray_sent = tokio::time::Instant::now();
                    }
                }
                Some(ev) = ev_rx.recv() => {
                    let _ = output.send(ev).await;
                }
                Some(p) = power_rx.recv() => match p {
                    PowerEvent::Before { kind, done } => {
                        // Immediately: no fade, every device, then confirm.
                        let _ = engine.sender().try_send(EngineCommand::SetOutputEnabled {
                            on: false,
                            fade: false,
                        });
                        match &bridge {
                            Some((btx, _)) => {
                                let notify = Arc::new(tokio::sync::Notify::new());
                                let _ = btx.send(BridgeCommand::Blackout(Some(notify.clone())));
                                tokio::spawn(async move {
                                    let _ = tokio::time::timeout(
                                        Duration::from_millis(3500),
                                        notify.notified(),
                                    )
                                    .await;
                                    let _ = done.send(());
                                });
                            }
                            None => {
                                let _ = done.send(());
                            }
                        }
                        let _ = output.send(ServiceEvent::PowerDown(kind)).await;
                    }
                    PowerEvent::Resumed => {
                        if let Some((btx, _)) = &bridge {
                            let _ = btx.send(BridgeCommand::Reapply);
                        }
                        let _ = output.send(ServiceEvent::Resumed).await;
                    }
                },
                cmd = cmd_rx.recv() => {
                    let Some(cmd) = cmd else { break };
                    match cmd {
                        ServiceCommand::Start(start) => {
                            if bridge.is_none() {
                                let (btx, brx) = mpsc::unbounded_channel();
                                let (bev_tx, mut bev_rx) = mpsc::unbounded_channel();
                                let task = match tuxrgb_openrgb::bridge::spawn_thread(
                                    start.bridge,
                                    frame_rx.clone(),
                                    brx,
                                    bev_tx,
                                ) {
                                    Ok(t) => t,
                                    Err(e) => {
                                        tracing::error!("cannot start the OpenRGB bridge thread: {e}");
                                        continue;
                                    }
                                };
                                let ev = ev_tx.clone();
                                tokio::spawn(async move {
                                    while let Some(e) = bev_rx.recv().await {
                                        let _ = ev.send(ServiceEvent::Bridge(e));
                                    }
                                });
                                bridge = Some((btx, task));
                            }
                            if tray.is_none() {
                                tray = tray::spawn(start.tray, ev_tx.clone()).await;
                            }
                        }
                        ServiceCommand::Bridge(c) => {
                            if let Some((btx, _)) = &bridge {
                                let _ = btx.send(c);
                            }
                        }
                        ServiceCommand::SetPreview(on) => {
                            let _ = preview_tx.send(on);
                            // The soft full-canvas background of the previews.
                            engine.send(EngineCommand::SetBackdrop(on));
                        }
                        ServiceCommand::Tray(state) => {
                            if let Some(t) = &tray {
                                let now = tokio::time::Instant::now();
                                if now >= tray_sent + TRAY_MIN_INTERVAL && tray_due.is_none() {
                                    t.update(move |tray| tray.state = state).await;
                                    tray_sent = now;
                                } else {
                                    tray_pending = Some(state);
                                    tray_due.get_or_insert(tray_sent + TRAY_MIN_INTERVAL);
                                }
                            }
                        }
                        ServiceCommand::Capture(req) => {
                            let _ = capture_tx.send(req);
                        }
                        ServiceCommand::Notify { summary, body, action } => {
                            // Never hold the service loop on the notification
                            // daemon (D-Bus activation can take seconds).
                            if let Some(n) = notifier.clone() {
                                tokio::spawn(async move {
                                    let _ = tokio::time::timeout(
                                        Duration::from_secs(10),
                                        n.send(&summary, &body, action),
                                    )
                                    .await;
                                });
                            }
                        }
                        ServiceCommand::Shutdown { turn_off } => {
                            let _ = capture_tx.send(CaptureRequest::default());
                            if let Some((btx, task)) = bridge.take() {
                                let _ = btx.send(BridgeCommand::Shutdown { turn_off });
                                let join = tokio::task::spawn_blocking(move || task.join());
                                let _ = tokio::time::timeout(Duration::from_secs(5), join).await;
                            }
                            if let Some(t) = tray.take() {
                                t.shutdown().await;
                            }
                            let _ = output.send(ServiceEvent::Stopped).await;
                        }
                    }
                }
            }
        }
        drop(engine);
    })
}

/// Time of the last redraw of the window, in milliseconds since
/// [`clock_ms`]'s origin (0: never). See [`note_redraw`].
static LAST_REDRAW_MS: AtomicU64 = AtomicU64::new(0);

fn clock_ms() -> u64 {
    static ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();
    ORIGIN
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
        + 1
}

/// Called for every redraw of the window. A minimised window, one on
/// another virtual desktop or hidden by the compositor is not redrawn
/// (Wayland sends it no frame callbacks): the preview feed notices and
/// slows down, so nothing is built for a picture nobody sees.
pub fn note_redraw() {
    LAST_REDRAW_MS.store(clock_ms(), Ordering::Relaxed);
}

/// Preview rate while the window is drawn (≤ 30 Hz) and while it is not:
/// a few frames per second keep it current and notice it coming back.
const PREVIEW_PERIOD: Duration = Duration::from_millis(33);
const HIDDEN_PERIOD: Duration = Duration::from_millis(250);

/// Notices a window that stopped drawing: a visible window redraws right
/// after each preview frame, a hidden one never does.
const UNDRAWN_AFTER_MS: u64 = 1000;

#[derive(Default)]
struct DrawWatch {
    /// When the oldest frame not yet followed by a redraw was sent (0: none).
    pending_since: u64,
}

impl DrawWatch {
    /// A frame was just sent at `now`; `last_redraw` is the time of the
    /// window's last redraw. `true` once frames have gone without any redraw
    /// for a second: a window that draws slowly (software rendering on a
    /// busy machine) still redraws within that time, a hidden one does not.
    fn on_send(&mut self, now: u64, last_redraw: u64) -> bool {
        if self.pending_since == 0 || last_redraw >= self.pending_since {
            self.pending_since = now;
        }
        now.saturating_sub(self.pending_since) >= UNDRAWN_AFTER_MS
            && last_redraw < self.pending_since
    }

    /// The window drew again since frames started piling up.
    fn drawn_again(&self, last_redraw: u64) -> bool {
        last_redraw >= self.pending_since
    }
}

async fn preview_feed(
    mut frames: watch::Receiver<Arc<Frame>>,
    mut enabled: watch::Receiver<bool>,
    ev: mpsc::UnboundedSender<ServiceEvent>,
) {
    loop {
        // Sleep until a window wants the preview.
        if enabled.wait_for(|on| *on).await.is_err() {
            return;
        }
        frames.mark_changed();
        let mut last_seq = None;
        let mut watch = DrawWatch::default();
        let mut was_hidden = false;
        loop {
            tokio::select! {
                changed = frames.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let f = frames.borrow_and_update().clone();
                    // The engine's keep-alive republishes the same frame:
                    // nothing new to show.
                    if last_seq == Some(f.seq) {
                        continue;
                    }
                    last_seq = Some(f.seq);
                    // The canvas glow, built here rather than on the
                    // interface thread (same image, see `ui::backdrop`).
                    let dark = crate::ui::backdrop::preview_dark();
                    let backdrop = f.backdrop.as_deref().map(|b| {
                        (dark, crate::ui::backdrop::soften(b, crate::ui::backdrop::strength(dark)))
                    });
                    if ev
                        .send(ServiceEvent::Preview(PreviewFrame { frame: f, backdrop }))
                        .is_err()
                    {
                        return;
                    }
                    let hidden = watch.on_send(clock_ms(), LAST_REDRAW_MS.load(Ordering::Relaxed));
                    if hidden != was_hidden {
                        was_hidden = hidden;
                        tracing::debug!(hidden, "preview: window drawn state changed");
                    }
                    if hidden {
                        // Next frame in a moment, or as soon as the window
                        // draws again (shown, or a long frame is over).
                        let until = tokio::time::Instant::now() + HIDDEN_PERIOD;
                        while tokio::time::Instant::now() < until
                            && !watch.drawn_again(LAST_REDRAW_MS.load(Ordering::Relaxed))
                        {
                            tokio::time::sleep(PREVIEW_PERIOD).await;
                        }
                    } else {
                        tokio::time::sleep(PREVIEW_PERIOD).await;
                    }
                }
                changed = enabled.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    if !*enabled.borrow() {
                        break;
                    }
                }
            }
        }
    }
}

async fn capture_manager(
    inputs: SharedInputs,
    mut rx: mpsc::UnboundedReceiver<CaptureRequest>,
    ev: mpsc::UnboundedSender<ServiceEvent>,
) {
    type ScreenStart = Result<(ScreenCapture, Option<String>), CaptureError>;
    let mut audio: Option<AudioCapture> = None;
    let mut screen: Option<ScreenCapture> = None;
    // The portal may be showing its picker: the start runs on its own so
    // audio, sensors and shaders keep following the requests meanwhile.
    let mut screen_starting: Option<tokio::task::JoinHandle<ScreenStart>> = None;
    let mut want_screen = false;
    let mut hardware: Option<HardwareMonitor> = None;
    let mut shader: Option<(ShaderRenderer, ShaderSettings)> = None;
    loop {
        let req = tokio::select! {
            req = rx.recv() => match req {
                Some(mut req) => {
                    // Only the latest request matters (effects switched
                    // quickly): no start / stop of what is already obsolete.
                    while let Ok(newer) = rx.try_recv() {
                        req = newer;
                    }
                    req
                }
                None => break,
            },
            started = async {
                match screen_starting.as_mut() {
                    Some(task) => task.await,
                    None => std::future::pending().await,
                }
            } => {
                screen_starting = None;
                match started {
                    Ok(Ok((s, new_token))) => {
                        let _ = ev.send(ServiceEvent::ScreencastToken(new_token));
                        if want_screen {
                            screen = Some(s);
                            let _ = ev.send(ServiceEvent::Screen(CaptureStatus::Running));
                        } else {
                            // No longer wanted while the picker was open.
                            s.stop().await;
                            inputs.set_screen(None);
                            let _ = ev.send(ServiceEvent::Screen(CaptureStatus::Off));
                        }
                    }
                    Ok(Err(CaptureError::Cancelled)) => {
                        let _ = ev.send(ServiceEvent::Screen(CaptureStatus::Cancelled));
                    }
                    Ok(Err(e)) => {
                        let _ = ev.send(ServiceEvent::Screen(CaptureStatus::Failed(e.to_string())));
                    }
                    Err(e) => {
                        let _ = ev.send(ServiceEvent::Screen(CaptureStatus::Failed(e.to_string())));
                    }
                }
                continue;
            }
        };
        let (want_audio, token) = (req.audio, req.screencast_token);
        want_screen = req.screen;
        // Sensors (cheap, no permission).
        match (req.hardware, hardware.is_some()) {
            (true, false) => match HardwareMonitor::start(inputs.clone()) {
                Ok(h) => hardware = Some(h),
                Err(e) => tracing::warn!("hardware sensors: {e}"),
            },
            (false, true) => {
                if let Some(h) = hardware.take() {
                    let _ = tokio::task::spawn_blocking(move || drop(h)).await;
                }
            }
            _ => {}
        }
        // GLSL shader on the GPU.
        match (req.shader, shader.take()) {
            (Some(want), None) => {
                let _ = ev.send(ServiceEvent::Shader(CaptureStatus::Starting));
                let i = inputs.clone();
                let settings = want.clone();
                match tokio::task::spawn_blocking(move || ShaderRenderer::start(i, settings)).await
                {
                    Ok(Ok(r)) => {
                        shader = Some((r, want));
                        let _ = ev.send(ServiceEvent::Shader(CaptureStatus::Running));
                    }
                    Ok(Err(e)) => {
                        let _ = ev.send(ServiceEvent::Shader(CaptureStatus::Failed(e.to_string())));
                    }
                    Err(e) => {
                        let _ = ev.send(ServiceEvent::Shader(CaptureStatus::Failed(e.to_string())));
                    }
                }
            }
            (Some(want), Some((r, current))) => {
                if want != current {
                    match r.update(want.clone()) {
                        Ok(()) => {
                            let _ = ev.send(ServiceEvent::Shader(CaptureStatus::Running));
                            shader = Some((r, want));
                        }
                        Err(e) => {
                            let _ = ev.send(ServiceEvent::Shader(CaptureStatus::Failed(e)));
                            shader = Some((r, current));
                        }
                    }
                } else {
                    shader = Some((r, current));
                }
            }
            (None, Some((r, _))) => {
                let _ = tokio::task::spawn_blocking(move || drop(r)).await;
                let _ = ev.send(ServiceEvent::Shader(CaptureStatus::Off));
            }
            (None, None) => {}
        }
        // Audio.
        match (want_audio, audio.is_some()) {
            (true, false) => {
                let _ = ev.send(ServiceEvent::Audio(CaptureStatus::Starting));
                let i = inputs.clone();
                match tokio::task::spawn_blocking(move || AudioCapture::start(i)).await {
                    Ok(Ok(a)) => {
                        audio = Some(a);
                        let _ = ev.send(ServiceEvent::Audio(CaptureStatus::Running));
                    }
                    Ok(Err(e)) => {
                        let _ = ev.send(ServiceEvent::Audio(CaptureStatus::Failed(e.to_string())));
                    }
                    Err(e) => {
                        let _ = ev.send(ServiceEvent::Audio(CaptureStatus::Failed(e.to_string())));
                    }
                }
            }
            (false, true) => {
                if let Some(a) = audio.take() {
                    let _ = tokio::task::spawn_blocking(move || drop(a)).await;
                }
                inputs.set_audio(None);
                let _ = ev.send(ServiceEvent::Audio(CaptureStatus::Off));
            }
            _ => {}
        }
        // Screen.
        match (want_screen, screen.is_some() || screen_starting.is_some()) {
            (true, false) => {
                let _ = ev.send(ServiceEvent::Screen(CaptureStatus::Starting));
                let i = inputs.clone();
                screen_starting = Some(tokio::spawn(ScreenCapture::start(i, token)));
            }
            (false, true) => {
                if let Some(s) = screen.take() {
                    s.stop().await;
                    inputs.set_screen(None);
                    let _ = ev.send(ServiceEvent::Screen(CaptureStatus::Off));
                }
                // A start still waiting for the picker is stopped as soon as
                // it completes (see above).
            }
            _ => {}
        }
    }
    if let Some(task) = screen_starting.take() {
        task.abort();
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn hidden_only_when_frames_stop_being_drawn() {
        let mut w = DrawWatch::default();
        // Visible: every frame is drawn ~10 ms after being sent.
        let mut redraw = 0;
        for i in 0..20u64 {
            let now = 1000 + i * 33;
            assert!(!w.on_send(now, redraw), "frame {i}");
            redraw = now + 10;
        }
        // Drawn slowly (one redraw every 400 ms): still visible.
        let start = 2000;
        for i in 0..30u64 {
            let now = start + i * 33;
            let drawn = start + (i * 33) / 400 * 400;
            assert!(!w.on_send(now, drawn.max(redraw)), "slow frame {i}");
        }
        redraw = start + 29 * 33;
        // Minimised: no more redraws. Hidden after ~1 s of frames.
        let first = start + 30 * 33;
        let hidden: Vec<bool> = (0..40u64)
            .map(|i| w.on_send(first + i * 33, redraw))
            .collect();
        assert!(hidden[..30].iter().all(|h| !*h), "{hidden:?}");
        assert!(hidden[31..].iter().all(|h| *h), "{hidden:?}");
        assert!(!w.drawn_again(redraw));
        assert!(w.drawn_again(first + 40 * 33));
        // Shown again: the next frame after a redraw counts as visible.
        let back = first + 40 * 33 + 400;
        assert!(!w.on_send(back, back - 5));
        // A static effect, nothing sent for a long time, then animated
        // again: visible straight away.
        assert!(!w.on_send(back + 60_000, back + 10));
        assert!(!w.on_send(back + 60_033, back + 60_010));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A fake logind on a private bus: hands out a pipe as the inhibitor.
    struct MockLogind {
        read_end: Arc<std::sync::Mutex<Option<OwnedFd>>>,
        inhibited: Arc<AtomicBool>,
    }

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl MockLogind {
        fn inhibit(
            &self,
            what: &str,
            _who: &str,
            _why: &str,
            mode: &str,
        ) -> zbus::zvariant::OwnedFd {
            assert_eq!(what, "shutdown:sleep");
            assert_eq!(mode, "delay");
            let mut fds = [0; 2];
            assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
            let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
            *self.read_end.lock().unwrap() = Some(r);
            self.inhibited.store(true, Ordering::SeqCst);
            zbus::zvariant::OwnedFd::from(w)
        }

        #[zbus(signal)]
        async fn prepare_for_shutdown(
            emitter: &zbus::object_server::SignalEmitter<'_>,
            start: bool,
        ) -> zbus::Result<()>;

        #[zbus(signal)]
        async fn prepare_for_sleep(
            emitter: &zbus::object_server::SignalEmitter<'_>,
            start: bool,
        ) -> zbus::Result<()>;
    }

    /// The inhibitor is held until TuxRGB confirmed the LEDs are off, then
    /// released so the shutdown continues.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_waits_for_the_blackout_then_releases_logind() {
        let Ok(mut daemon) = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .spawn()
        else {
            eprintln!("no dbus-daemon: skipped");
            return;
        };
        let mut addr = String::new();
        {
            let out = daemon.stdout.as_mut().unwrap();
            let mut byte = [0u8; 1];
            while out.read(&mut byte).unwrap_or(0) == 1 && byte[0] != b'\n' {
                addr.push(byte[0] as char);
            }
        }
        // SAFETY: only this test reads the system bus address.
        unsafe { std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", &addr) };

        let read_end = Arc::new(std::sync::Mutex::new(None));
        let inhibited = Arc::new(AtomicBool::new(false));
        let server = zbus::connection::Builder::address(addr.as_str())
            .unwrap()
            .name("org.freedesktop.login1")
            .unwrap()
            .serve_at(
                "/org/freedesktop/login1",
                MockLogind {
                    read_end: read_end.clone(),
                    inhibited: inhibited.clone(),
                },
            )
            .unwrap()
            .build()
            .await
            .expect("mock logind");

        let (power_tx, mut power_rx) = mpsc::unbounded_channel();
        tokio::spawn(logind_watch(power_tx));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !inhibited.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(inhibited.load(Ordering::SeqCst), "no inhibitor taken");
        tokio::time::sleep(Duration::from_millis(100)).await;

        let iface = server
            .object_server()
            .interface::<_, MockLogind>("/org/freedesktop/login1")
            .await
            .unwrap();
        MockLogind::prepare_for_shutdown(iface.signal_emitter(), true)
            .await
            .unwrap();

        let ev = tokio::time::timeout(Duration::from_secs(3), power_rx.recv())
            .await
            .expect("power event")
            .expect("channel open");
        let PowerEvent::Before { kind, done } = ev else {
            panic!("expected PowerEvent::Before");
        };
        assert_eq!(kind, PowerKind::Shutdown);

        // Still inhibited while the LEDs are being turned off…
        let fd = read_end.lock().unwrap().take().unwrap();
        let mut pipe = std::fs::File::from(fd);
        let reader = tokio::task::spawn_blocking(move || {
            let mut b = [0u8; 1];
            let t = std::time::Instant::now();
            let n = pipe.read(&mut b).unwrap_or(0);
            (n, t.elapsed())
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!reader.is_finished(), "inhibitor released too early");
        // … and released right after the confirmation.
        let _ = done.send(());
        let (n, waited) = tokio::time::timeout(Duration::from_secs(2), reader)
            .await
            .expect("inhibitor released")
            .unwrap();
        assert_eq!(n, 0, "EOF expected on the inhibitor pipe");
        assert!(waited >= Duration::from_millis(250));
        let _ = daemon.kill();
    }
}
