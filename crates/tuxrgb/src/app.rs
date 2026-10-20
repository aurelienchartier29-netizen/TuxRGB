//! Application state, messages and update logic (Elm architecture).

use crate::config::{
    AccentSource, Config, LanguagePref, MotionPref, Onboarding, RendererPref, SchemeVariant,
    ServerKind, ThemeMode,
};
use crate::paths;
use crate::privileges::{self, Diagnostics, HelperError, HelperReport};
use crate::services::{
    CaptureRequest, CaptureStatus, ServiceCommand, ServiceEvent, ServiceHandle, StartConfig,
};
use crate::theme::kde::SystemAppearance;
use crate::theme::motion::Spring;
use crate::theme::tokens::motion;
use crate::theme::{BREEZE_ACCENT, Roles};
use crate::tray::{TrayAction, TrayState};
use crate::ui;
use iced::widget::canvas::Cache;
use iced::{Size, Subscription, Task, window};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tuxrgb_core::device::demo_devices;
use tuxrgb_core::engine::{EngineCommand, EngineScene, Frame};
use tuxrgb_core::{DeviceInfo, DeviceKey, EffectKind, ParamValue, Placement, Rgb, Scene};
use tuxrgb_openrgb::{BridgeCommand, BridgeConfig, BridgeEvent, BridgeStatus, ZoneResize};

/// LED count given to an empty ARGB header by "Configuration automatique":
/// enough for three 8-to-20-LED fans or a short strip. Addressable LEDs
/// cannot report how many are chained, and declaring more than are present
/// is harmless (the extra data is ignored), declaring fewer leaves LEDs off.
pub const DEFAULT_ARGB_LEDS: usize = 60;

/// A problem or missing component, shown as a banner on the home page
/// (with a direct action) and as a desktop notification when TuxRGB runs
/// in the background.
#[derive(Clone, Debug)]
pub struct Issue {
    pub key: &'static str,
    pub glyph: char,
    pub heading: String,
    pub body: String,
    pub action: Option<(char, &'static str, Message)>,
    pub secondary: Option<(&'static str, Message)>,
    /// Worth a desktop notification when no window is open.
    pub notify: bool,
    /// Information rather than a problem (neutral colours).
    pub info: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Home,
    Effects,
    Devices,
    Settings,
}

impl Page {
    pub const ALL: [Page; 4] = [Page::Home, Page::Effects, Page::Devices, Page::Settings];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsView {
    Main,
    Credits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum License {
    Gpl2,
    Lgpl3,
    Apache2,
    Mit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WizardStep {
    Welcome,
    Checks,
    Access,
    Done,
}

/// What fills the window: a page (and its sub-view) or a wizard step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKey {
    Main(Page, SettingsView),
    Wizard(WizardStep),
}

/// Enough to draw the outgoing view of a transition.
#[derive(Clone, Debug)]
pub enum ViewSnap {
    Main(Page, SettingsView),
    Wizard(Box<Wizard>),
}

/// M3 "fade through" between views: the outgoing view fades into the
/// background, then the new one fades in while growing back to full size.
#[derive(Clone, Debug)]
pub struct ViewTransition {
    pub from: ViewSnap,
    /// The wizard is involved: the whole window changes (no rail).
    pub whole: bool,
}

/// Duration of the arrival of a newly detected device.
pub const DEVICE_APPEAR: f32 = 0.5;

/// Total duration of a view transition and the share of the fade out.
pub const VIEW_FADE: Duration = Duration::from_millis(320);
pub const VIEW_FADE_OUT: f32 = 0.3;

/// First automatic update check, once the start-up work is done.
const UPDATE_CHECK_DELAY: Duration = Duration::from_secs(8);
/// Automatic checks are at least this far apart (across restarts).
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 3600);

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Dialogs (kept while they animate out).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modal {
    ResetAccess,
    License(License),
}

/// Items that animate in and out (banners): the ones on screen, including
/// those collapsing after they stopped applying.
#[derive(Clone, Debug)]
pub struct Presence<T> {
    pub items: Vec<Present<T>>,
}

#[derive(Clone, Debug)]
pub struct Present<T> {
    pub key: &'static str,
    pub value: T,
    /// No longer applies: collapsing, removed once hidden.
    pub closing: bool,
    added: Instant,
}

impl<T> Present<T> {
    /// Newly added (animate its arrival), as opposed to shown again when
    /// its page is opened.
    pub fn fresh(&self) -> bool {
        self.added.elapsed() < Duration::from_millis(700)
    }
}

impl<T> Default for Presence<T> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl<T: Clone> Presence<T> {
    /// Takes the current list: new items appear, missing ones start
    /// closing where they were, items that come back reopen.
    pub fn sync(&mut self, current: Vec<(&'static str, T)>) {
        let now = Instant::now();
        let mut next: Vec<Present<T>> = current
            .iter()
            .map(|(k, v)| Present {
                key: k,
                value: v.clone(),
                closing: false,
                added: self
                    .items
                    .iter()
                    .find(|p| p.key == *k && !p.closing)
                    .map_or(now, |p| p.added),
            })
            .collect();
        for (i, p) in self.items.iter().enumerate() {
            if !current.iter().any(|(c, _)| *c == p.key) {
                next.insert(
                    i.min(next.len()),
                    Present {
                        closing: true,
                        ..p.clone()
                    },
                );
            }
        }
        self.items = next;
    }

    /// The item finished collapsing.
    pub fn remove(&mut self, key: &str) {
        self.items.retain(|p| !(p.key == key && p.closing));
    }

    /// Present from the start: no arrival animation.
    pub fn settle(&mut self) {
        let past = Instant::now() - Duration::from_secs(5);
        self.items.retain(|p| !p.closing);
        for p in &mut self.items {
            p.added = past;
        }
    }
}

#[derive(Clone, Debug)]
pub struct Wizard {
    pub step: WizardStep,
    pub result: Option<Result<HelperReport, HelperError>>,
    pub from_reset: bool,
    /// "Lancer à l'ouverture de session" choice on the last step.
    pub autostart: bool,
}

#[derive(Clone, Debug)]
pub enum Message {
    // Navigation & windows
    Navigate(Page),
    ShowSettings(SettingsView),
    WindowOpened,
    Maximize(window::Id),
    /// The GPU renderer survived its first seconds: clear the crash guard.
    GpuOk,
    Noop,
    WindowCloseRequested(window::Id),
    WindowClosed(window::Id),
    WindowFocus(bool),
    ForceExit,
    Quit,
    Tick(Instant),
    // Background services
    Service(ServiceEvent),
    // Effects
    SelectEffect(EffectKind),
    SetParam(EffectKind, &'static str, ParamValue),
    ResetParams(EffectKind),
    OpenPicker(EffectKind, &'static str),
    ClosePicker,
    PickerChanged(Rgb),
    HexInput(String),
    SetBrightness(f32),
    /// Global saturation, 0–2 (0–200 %).
    SetSaturation(f32),
    SetOutput(bool),
    // Devices & canvas
    CanvasSelect(Option<usize>),
    CanvasMove(usize, Placement),
    CanvasDragEnd,
    ResizeSelected(f32),
    /// Turns the selected device by 90° clockwise.
    RotateSelected,
    /// Mirrors the selected device (LED order reversed).
    MirrorSelected,
    ToggleDevice(DeviceKey, bool),
    /// Text typed in the LED-count field of a resizable zone.
    ZoneSizeInput(DeviceKey, usize, String),
    /// Gives every empty ARGB zone a working default LED count.
    AutoConfigureZones,
    /// Opens the Devices page on the first device with an empty ARGB zone.
    ShowUnsizedZone,
    /// − / + buttons next to that field.
    ZoneSizeStep(DeviceKey, usize, i64),
    /// Sends the new size to OpenRGB.
    ApplyZoneSize(DeviceKey, usize),
    Identify(u32),
    ResetLayout,
    Rescan,
    RestartEngine,
    SetDemoDevices(bool),
    // Settings
    SetThemeMode(ThemeMode),
    SetAccentSource(AccentSource),
    SetCustomAccent(Rgb),
    SetVariant(SchemeVariant),
    SetMotion(MotionPref),
    SetAnimationSpeed(f32),
    SetTransitions(bool),
    SetLanguage(LanguagePref),
    SetFps(u32),
    SetRenderer(RendererPref),
    SetAutostart(bool),
    AutostartDone(Result<(), String>),
    SetStartMinimized(bool),
    SetCloseToTray(bool),
    SetTurnOffOnExit(bool),
    SetServerKind(ServerKind),
    ShowLicense(Option<License>),
    OpenUrl(String),
    // Hardware access
    RunDiagnostics,
    DiagnosticsDone(Diagnostics),
    FindingsDone(privileges::report::Findings),
    /// Builds the hardware report, copies it and saves it to a file.
    CopyReport,
    ReportReady(String),
    StartHardwareSetup,
    RulesReady(String),
    HardwareSetupDone(Result<HelperReport, HelperError>),
    /// Kernel-side SMBus fix (acpi_enforce_resources=lax, spd5118).
    EnableSmbus,
    SmbusDone(Result<HelperReport, HelperError>),
    AskResetAccess(bool),
    ConfirmResetAccess,
    ResetAccessDone(Result<HelperReport, HelperError>),
    // Onboarding wizard
    WizardNext,
    WizardBack,
    WizardSkipAccess,
    WizardFinish,
    WizardAutostart(bool),
    // Developer screenshot tour
    TourStep(usize),
    TourShoot(usize),
    /// Motion tour: freeze the animations mid-way, then capture.
    TourShootNow(usize),
    TourShootFrozen(usize),
    TourSaved(usize, window::Screenshot),
    /// After resume from sleep: LEDs back to the saved state.
    RestoreOutput,
    /// Hides an information banner for good.
    DismissNotice(&'static str),
    // Motion
    /// A colour picker finished collapsing.
    PickerHidden(EffectKind, &'static str),
    /// A banner finished collapsing.
    BannerHidden(&'static str),
    // Updates (GitHub releases)
    CheckUpdates,
    /// Shortly after start-up, if enabled and not done recently.
    AutoCheckUpdates,
    UpdateChecked {
        manual: bool,
        result: Result<crate::update::Check, String>,
    },
    SetAutoUpdateCheck(bool),
    InstallUpdate,
    UpdateProgress(crate::update::Progress),
    /// Restart from the new AppImage / run the downloaded installer.
    ApplyUpdate,
    UpdateApplied(Result<(), String>),
    // Misc
    DismissSnackbar(u64),
    SaveConfig,
    /// The window is gone: hand the memory it used back to the system.
    TrimMemory,
}

pub struct App {
    pub config: Config,
    pub onboarding: Onboarding,
    pub wizard: Option<Wizard>,
    pub page: Page,
    pub settings_view: SettingsView,
    pub window: Option<window::Id>,
    pub services: Option<ServiceHandle>,

    // Hardware
    pub bridge_status: BridgeStatus,
    pub devices: Vec<DeviceInfo>,
    pub scene: Scene,
    pub using_demo: bool,
    pub preview: Option<Arc<Frame>>,
    /// Blurred glow of the effect behind the canvas devices.
    pub backdrop: Option<iced::widget::image::Handle>,
    pub selected_device: Option<usize>,
    pub diagnostics: Option<Diagnostics>,
    pub findings: Option<privileges::report::Findings>,
    /// The SMBus fix was applied: a reboot is needed.
    pub smbus_reboot_pending: bool,
    /// The bootloader must be changed by hand (NixOS…): instructions.
    pub smbus_manual: Option<String>,
    pub helper_busy: bool,
    pub audio_status: CaptureStatus,
    pub screen_status: CaptureStatus,
    pub shader_status: CaptureStatus,
    pub tray_available: bool,
    pub canvas_cache: Cache,

    // Theme & motion
    pub system: SystemAppearance,
    pub roles: Roles,
    roles_from: Roles,
    roles_to: Roles,
    theme_t: Spring,
    pub nav: [Spring; 4],
    /// View transition in progress and its progress (`0..=1`).
    pub transition: Option<ViewTransition>,
    pub transition_t: f32,
    view_seen: ViewKey,
    wizard_seen: Option<Wizard>,
    /// Dialog on screen (also while it fades out) and its appearance.
    pub modal: Option<Modal>,
    pub modal_t: Spring,
    /// Snackbar entrance (slides up with an expressive spring) and exit.
    pub snack_in: Spring,
    snack_closing: bool,
    /// The picker that was open last, kept while it collapses.
    pub picker_last: Option<(EffectKind, &'static str)>,
    picker_seen: Option<(EffectKind, &'static str)>,
    /// Banners of the home page and of the devices page.
    pub home_banners: Presence<Issue>,
    pub device_banners: Presence<Issue>,
    /// When each device was first seen: new devices fade and grow in.
    device_seen: std::collections::BTreeMap<DeviceKey, Instant>,
    /// Motion tour: animation state held still for a capture (a capture
    /// re-renders the last frame; text whose layout changed since would be
    /// missing from it).
    tour_frozen: bool,
    pub cards: [Spring; EffectKind::ALL.len()],
    last_tick: Option<Instant>,

    // Transient UI state
    pub picker: Option<(EffectKind, &'static str)>,
    pub hex_input: String,
    pub snackbar: Option<(u64, String)>,
    snack_seq: u64,
    pub confirm_reset: bool,
    /// Pending LED counts typed for resizable zones, by (device, zone).
    pub zone_edits: std::collections::BTreeMap<(DeviceKey, usize), String>,
    pub license: Option<License>,
    pub restart_required: bool,
    config_dirty: bool,
    quitting: bool,
    /// The window has keyboard focus (a new launch then has nothing to do).
    window_focused: bool,
    /// The window is being closed only to be opened again in front.
    reopen_pending: bool,
    restore_attempted: std::collections::BTreeSet<(DeviceKey, usize)>,
    /// Issues already sent as desktop notifications this session.
    notified: std::collections::BTreeSet<&'static str>,
    /// New version check and installation (see `update.rs`).
    pub update: crate::update::State,
}

impl App {
    pub fn boot() -> (Self, Task<Message>) {
        let args = crate::args();
        // Read once at start-up (main), before the window and the services.
        let config = Config::initial();
        for note in Config::take_load_notes() {
            tracing::warn!("{note}");
        }
        let mut onboarding = Onboarding::load();
        if args.reset_access {
            Onboarding::reset();
            onboarding = Onboarding::default();
        }
        // First paint uses kdeglobals synchronously to avoid a colour flash.
        let (kde_dark, kde_accent) = crate::theme::kde::read_kdeglobals();
        let system = SystemAppearance {
            dark: kde_dark,
            accent: kde_accent,
            reduced_motion: false,
            source: if cfg!(windows) {
                "registre Windows"
            } else {
                "kdeglobals"
            },
        };
        let mut app = Self {
            wizard: (!onboarding.completed).then_some(Wizard {
                step: WizardStep::Welcome,
                result: None,
                from_reset: args.reset_access,
                autostart: true,
            }),
            onboarding,
            page: Page::Home,
            settings_view: SettingsView::Main,
            window: None,
            services: None,
            bridge_status: BridgeStatus::Starting,
            devices: Vec::new(),
            scene: Scene::default(),
            using_demo: false,
            preview: None,
            backdrop: None,
            selected_device: None,
            diagnostics: None,
            findings: None,
            smbus_reboot_pending: false,
            smbus_manual: None,
            helper_busy: false,
            audio_status: CaptureStatus::Off,
            screen_status: CaptureStatus::Off,
            shader_status: CaptureStatus::Off,
            tray_available: false,
            canvas_cache: Cache::new(),
            system,
            roles: Roles::generate(BREEZE_ACCENT, true, SchemeVariant::Expressive),
            roles_from: Roles::generate(BREEZE_ACCENT, true, SchemeVariant::Expressive),
            roles_to: Roles::generate(BREEZE_ACCENT, true, SchemeVariant::Expressive),
            theme_t: Spring::new(1.0, motion::DEFAULT_EFFECTS),
            nav: std::array::from_fn(|i| {
                Spring::new(
                    if i == 0 { 1.0 } else { 0.0 },
                    motion::EXPRESSIVE_FAST_SPATIAL,
                )
            }),
            transition: None,
            transition_t: 1.0,
            view_seen: ViewKey::Main(Page::Home, SettingsView::Main),
            wizard_seen: None,
            modal: None,
            modal_t: Spring::new(0.0, motion::EXPRESSIVE_FAST_SPATIAL),
            snack_in: Spring::new(1.0, motion::EXPRESSIVE_DEFAULT_SPATIAL),
            snack_closing: false,
            picker_last: None,
            picker_seen: None,
            home_banners: Presence::default(),
            device_banners: Presence::default(),
            device_seen: Default::default(),
            tour_frozen: false,
            cards: std::array::from_fn(|_| Spring::new(0.0, motion::EXPRESSIVE_FAST_SPATIAL)),
            last_tick: None,
            picker: None,
            hex_input: String::new(),
            snackbar: None,
            snack_seq: 0,
            confirm_reset: false,
            zone_edits: Default::default(),
            license: None,
            restart_required: false,
            config_dirty: false,
            quitting: false,
            window_focused: false,
            reopen_pending: false,
            restore_attempted: Default::default(),
            notified: Default::default(),
            update: Default::default(),
            config,
        };
        crate::theme::motion::set_time_scale(app.config.appearance.animation_speed);
        let roles = app.target_roles();
        app.roles = roles;
        app.roles_from = roles;
        app.roles_to = roles;
        if let Some(i) = EffectKind::ALL.iter().position(|e| *e == app.config.effect) {
            app.cards[i].snap(1.0);
        }
        app.rebuild_scene();

        if args.screenshot_tour.is_some() {
            app.wizard = Some(Wizard {
                step: WizardStep::Access,
                result: None,
                from_reset: false,
                autostart: true,
            });
        }
        // Nothing to animate from at start-up.
        app.view_seen = app.view_key();
        app.wizard_seen = app.wizard.clone();
        app.sync_banners();
        app.home_banners.settle();
        app.device_banners.settle();
        let past = Instant::now() - Duration::from_secs(5);
        for t in app.device_seen.values_mut() {
            *t = past;
        }
        let minimized = (args.minimized || app.config.behavior.start_minimized)
            && app.wizard.is_none()
            && args.screenshot_tour.is_none();
        let mut tasks = vec![app.diagnostics_task()];
        if std::env::var_os("TUXRGB_GPU_FALLBACK").is_some() {
            tracing::warn!(
                "previous start crashed in the GPU renderer: software rendering enabled"
            );
            app.config.renderer = crate::config::RendererPref::Software;
            let _ = app.config.save();
            tasks.push(app.snackbar(
                "Le rendu GPU a échoué au dernier lancement : rendu logiciel activé (Réglages → Performances).",
            ));
        }
        if args.screenshot_tour.is_none() {
            tasks.push(Task::perform(
                async {
                    let _ = tokio::task::spawn_blocking(crate::integration::ensure).await;
                },
                |_| Message::Noop,
            ));
        }
        // The AppImage may have been moved or updated (new file name): keep
        // the login entry pointing at the binary that is running now.
        if app.config.behavior.autostart
            && paths::package_kind() != paths::PackageKind::Flatpak
            && args.screenshot_tour.is_none()
        {
            tasks.push(Task::perform(
                crate::autostart::set_enabled(true),
                Message::AutostartDone,
            ));
        }
        if args.screenshot_tour.is_none() {
            tasks.push(Task::perform(
                tokio::time::sleep(UPDATE_CHECK_DELAY),
                |_| Message::AutoCheckUpdates,
            ));
        }
        if !minimized {
            tasks.push(app.open_window());
        }
        (app, Task::batch(tasks))
    }

    pub fn title(&self, _window: window::Id) -> String {
        "TuxRGB".into()
    }

    pub fn theme(&self, _window: window::Id) -> iced::Theme {
        self.roles.iced_theme()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            Subscription::run(crate::services::run).map(Message::Service),
            window::close_requests().map(Message::WindowCloseRequested),
            window::close_events().map(Message::WindowClosed),
        ];
        if self.window.is_some() {
            subs.push(window::events().filter_map(|(_, e)| match e {
                window::Event::Focused => Some(Message::WindowFocus(true)),
                window::Event::Unfocused => Some(Message::WindowFocus(false)),
                _ => None,
            }));
            // Redraws are only noted, never turned into a message (that
            // would redraw again): the preview slows down while the window
            // is not drawn (minimised, on another desktop).
            subs.push(iced::event::listen_raw(|event, _status, _window| {
                if let iced::Event::Window(window::Event::RedrawRequested(_)) = event {
                    crate::services::note_redraw();
                }
                None::<Message>
            }));
        }
        if self.window.is_some() && self.is_animating() {
            subs.push(window::frames().map(Message::Tick));
        }
        if self.config_dirty {
            // Debounced: written about a second after the last change.
            subs.push(iced::time::every(Duration::from_secs(1)).map(|_| Message::SaveConfig));
        }
        Subscription::batch(subs)
    }

    pub fn view(&self, _window: window::Id) -> iced::Element<'_, Message> {
        ui::view(self)
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    /// Shows the window in front of the others (tray, second launch).
    ///
    /// Wayland does not let a client raise one of its existing windows
    /// without an activation token from the user's click, and winit's
    /// `focus_window` is a no-op there: a window that is open but buried is
    /// closed and opened again, which the compositor maps on top.
    fn bring_to_front(&mut self) -> Task<Message> {
        tracing::info!(
            open = self.window.is_some(),
            focused = self.window_focused,
            "showing the window"
        );
        match self.window {
            None => self.open_window(),
            Some(_) if self.window_focused => Task::none(),
            Some(id) => {
                self.reopen_pending = true;
                window::close(id)
            }
        }
    }

    fn open_window(&mut self) -> Task<Message> {
        if let Some(id) = self.window {
            return window::gain_focus(id);
        }
        let icon =
            window::icon::from_rgba(include_bytes!("../assets/icon-128.rgba").to_vec(), 128, 128)
                .ok();
        let (id, task) = window::open(window::Settings {
            size: Size::new(1320.0, 860.0),
            // Opened by the user (menu, tray, second launch): full size.
            // The screenshot tour keeps a fixed size.
            maximized: crate::args().screenshot_tour.is_none(),
            min_size: Some(Size::new(1000.0, 660.0)),
            icon,
            exit_on_close_request: false,
            #[cfg(target_os = "linux")]
            platform_specific: window::settings::PlatformSpecific {
                application_id: paths::APP_ID.into(),
                ..Default::default()
            },
            ..Default::default()
        });
        self.window = Some(id);
        self.push_preview();
        self.push_suspended();
        // GPU crash guard: removed once the window has drawn for a while.
        if std::env::var("ICED_BACKEND").as_deref() != Ok("tiny-skia") {
            let marker = &paths::paths().gpu_marker;
            if let Some(dir) = marker.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(marker, b"gpu window being created\n");
        }
        task.map(|_| Message::WindowOpened)
    }

    fn diagnostics_task(&self) -> Task<Message> {
        Task::perform(
            async {
                tokio::task::spawn_blocking(privileges::diagnostics::run)
                    .await
                    .ok()
            },
            |d| match d {
                Some(d) => Message::DiagnosticsDone(d),
                None => Message::SaveConfig,
            },
        )
    }

    pub fn reduced_motion(&self) -> bool {
        match self.config.appearance.motion {
            MotionPref::System => self.system.reduced_motion,
            MotionPref::Expressive => false,
            MotionPref::Reduced => true,
        }
    }

    fn spatial(&self) -> crate::theme::tokens::SpringSpec {
        if self.reduced_motion() {
            motion::STANDARD_FAST_SPATIAL
        } else {
            motion::EXPRESSIVE_FAST_SPATIAL
        }
    }

    /// Arrival of a device (`0` just detected → `1` settled).
    pub fn device_appear(&self, key: &DeviceKey) -> f32 {
        self.device_seen.get(key).map_or(1.0, |t| {
            (crate::theme::motion::scaled(t.elapsed().as_secs_f32()) / DEVICE_APPEAR).min(1.0)
        })
    }

    fn devices_appearing(&self) -> bool {
        self.device_seen
            .values()
            .any(|t| crate::theme::motion::scaled(t.elapsed().as_secs_f32()) < DEVICE_APPEAR)
    }

    pub fn is_animating(&self) -> bool {
        self.devices_appearing() && self.window.is_some()
            || self.theme_t.is_moving()
            || self.transition.is_some()
            || self.modal_t.is_moving()
            || self.modal.is_some() && self.modal_t.value() < 0.001
            || self.snack_in.is_moving()
            || self.nav.iter().any(Spring::is_moving)
            || self.cards.iter().any(Spring::is_moving)
    }

    pub fn is_dark(&self) -> bool {
        match self.config.appearance.theme {
            ThemeMode::System => self.system.dark.unwrap_or(true),
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
        }
    }

    /// KDE Plasma accent colour (portal / kdeglobals, Breeze blue fallback):
    /// what "Synchroniser avec le thème KDE" puts on the LEDs.
    pub fn desktop_accent(&self) -> Rgb {
        self.system.accent.unwrap_or(BREEZE_ACCENT)
    }

    pub fn accent_seed(&self) -> Rgb {
        match self.config.appearance.accent {
            AccentSource::Desktop => self.system.accent.unwrap_or(BREEZE_ACCENT),
            AccentSource::Custom => self.config.appearance.custom_accent,
        }
    }

    fn target_roles(&self) -> Roles {
        Roles::generate(
            self.accent_seed(),
            self.is_dark(),
            self.config.appearance.variant,
        )
    }

    /// Rebuilds the soft canvas background from the latest preview frame.
    fn refresh_backdrop(&mut self) {
        self.backdrop = self
            .preview
            .as_ref()
            .and_then(|f| f.backdrop.as_deref())
            .map(|b| ui::backdrop::soften(b, ui::backdrop::strength(self.roles.dark)));
    }

    fn retheme(&mut self) {
        let target = self.target_roles();
        if target == self.roles_to {
            return;
        }
        self.roles_from = self.roles;
        self.roles_to = target;
        if self.window.is_none() {
            self.roles = target;
            self.theme_t.snap(1.0);
        } else {
            self.theme_t.restart(0.0, 1.0);
        }
        self.canvas_cache.clear();
    }

    fn mark_dirty(&mut self) {
        self.config_dirty = true;
    }

    /// `&str` messages are French source strings (translated); `String`
    /// messages are built with `trf!`.
    fn snackbar<'a>(&mut self, text: impl crate::i18n::Txt<'a>) -> Task<Message> {
        self.snack_seq += 1;
        let id = self.snack_seq;
        self.snackbar = Some((id, text.tr().into_owned()));
        self.snack_closing = false;
        self.snack_in.set_spec(motion::EXPRESSIVE_DEFAULT_SPATIAL);
        if self.reduced_motion() {
            self.snack_in.snap(1.0);
        } else {
            self.snack_in.restart(0.0, 1.0);
        }
        Task::perform(tokio::time::sleep(Duration::from_secs(5)), move |_| {
            Message::DismissSnackbar(id)
        })
    }

    pub fn status_text(&self) -> String {
        match &self.bridge_status {
            BridgeStatus::Connected { controllable, .. } => {
                trf!("{} périphérique(s) piloté(s)", controllable)
            }
            BridgeStatus::Detecting { .. } => crate::i18n::t("Détection du matériel…").to_owned(),
            BridgeStatus::Starting => crate::i18n::t("Démarrage du moteur…").to_owned(),
            BridgeStatus::NoServerBinary => crate::i18n::t("Moteur OpenRGB introuvable").to_owned(),
            BridgeStatus::Reconnecting(_) => crate::i18n::t("Reconnexion au moteur…").to_owned(),
            BridgeStatus::Error(_) => crate::i18n::t("Erreur du moteur matériel").to_owned(),
            BridgeStatus::Stopped => crate::i18n::t("Moteur arrêté").to_owned(),
        }
    }

    fn tray_state(&self) -> TrayState {
        TrayState {
            effect: self.config.effect,
            brightness: self.config.brightness,
            output_enabled: self.config.output_enabled,
            status: self.status_text(),
            lang: tuxrgb_core::i18n::language(),
        }
    }

    fn push_tray(&self) {
        if let Some(s) = &self.services {
            s.send(ServiceCommand::Tray(self.tray_state()));
        }
    }

    fn push_capture(&self) {
        if let Some(s) = &self.services {
            let kind = self.config.effect;
            let req = kind.requirements();
            let on = self.config.output_enabled;
            let shader = (req.shader && on).then(|| self.shader_settings());
            // Audio-reactive custom shaders get the spectrum too.
            let shader_audio = shader
                .as_ref()
                .is_some_and(|sh| sh.source.contains("iAudio"));
            s.send(ServiceCommand::Capture(CaptureRequest {
                audio: (req.audio || shader_audio) && on,
                screen: req.screen && on,
                hardware: req.hardware && on,
                shader,
                screencast_token: self.config.screencast_token.clone(),
            }));
        }
    }

    /// Source and uniforms of the selected GLSL shader.
    fn shader_settings(&self) -> tuxrgb_capture::shader::ShaderSettings {
        let kind = EffectKind::Shader;
        let preset = self.config.params.choice(kind, "shader");
        let source = match tuxrgb_capture::shader::preset_source(preset) {
            Some(src) => src.to_owned(),
            None => {
                let path = paths::paths().config_dir.join("shaders/custom.frag");
                match std::fs::read_to_string(&path) {
                    Ok(src) => src,
                    Err(_) => {
                        if let Some(dir) = path.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        let _ = paths::write_atomic(&path, tuxrgb_capture::shader::CUSTOM_TEMPLATE);
                        tuxrgb_capture::shader::CUSTOM_TEMPLATE.to_owned()
                    }
                }
            }
        };
        tuxrgb_capture::shader::ShaderSettings {
            source,
            speed: self.config.params.number(kind, "speed"),
            accent: self.desktop_accent(),
        }
    }

    /// Everything that needs the user's attention, most important first.
    pub fn issues(&self) -> Vec<Issue> {
        use crate::ui::icons;
        let t = crate::i18n::t;
        let mut out = Vec::new();
        let busy = self.helper_busy;
        let readiness = self.diagnostics.as_ref().map(|d| d.readiness());
        let access_missing = matches!(
            readiness,
            Some(privileges::Readiness::NotConfigured) | Some(privileges::Readiness::Partial)
        ) && !self.onboarding.hardware_configured;
        if access_missing {
            out.push(Issue {
                key: "access",
                glyph: icons::ADMIN_PANEL_SETTINGS,
                heading: t("Accès matériel à configurer").into(),
                #[cfg(not(windows))]
                body: t("Les règles udev et le module I2C sont nécessaires pour piloter la carte mère, la RAM et les périphériques USB.").into(),
                #[cfg(windows)]
                body: t("Le pilote PawnIO et les droits administrateur sont nécessaires pour piloter la carte mère et la RAM (les périphériques USB fonctionnent déjà).").into(),
                action: (!busy).then_some((icons::LOCK_OPEN, "Configurer", Message::StartHardwareSetup)),
                secondary: None,
                notify: true,
                info: false,
            });
        }
        if let crate::update::State::Available(release) = &self.update {
            use crate::update::Method;
            let method = crate::update::method();
            out.push(Issue {
                key: "update",
                glyph: icons::NEW_RELEASES,
                heading: trf!("TuxRGB {} est disponible", release.version),
                body: t(match method {
                    Method::AppImage => "La mise à jour remplace l'AppImage puis redémarre TuxRGB. Vos réglages sont conservés.",
                    Method::WindowsInstaller => "L'installateur de la nouvelle version s'exécute puis relance TuxRGB. Vos réglages sont conservés.",
                    Method::Manual => "Mettez TuxRGB à jour avec votre gestionnaire de paquets, ou téléchargez la nouvelle version sur GitHub.",
                })
                .into(),
                action: Some(match method {
                    Method::Manual => (icons::OPEN_IN_NEW, "Voir la version", Message::InstallUpdate),
                    _ => (icons::SYSTEM_UPDATE_ALT, "Mettre à jour", Message::InstallUpdate),
                }),
                secondary: None,
                notify: true,
                info: true,
            });
        }
        // Windows: the installer downloads OpenRGB; it may have failed.
        #[cfg(windows)]
        if self.bridge_status == BridgeStatus::NoServerBinary {
            out.push(Issue {
                key: "openrgb-missing",
                glyph: icons::DNS,
                heading: t("OpenRGB 1.1 introuvable").into(),
                body: t("Le moteur matériel n'a pas pu être téléchargé à l'installation. Réinstallez TuxRGB avec une connexion Internet, ou décompressez OpenRGB 1.1 pour Windows dans le dossier « openrgb » de TuxRGB.").into(),
                action: Some((
                    icons::OPEN_IN_NEW,
                    "Télécharger OpenRGB",
                    Message::OpenUrl("https://openrgb.org/releases.html".into()),
                )),
                secondary: None,
                notify: true,
                info: false,
            });
        }
        #[cfg(not(windows))]
        if let Some(d) = &self.diagnostics {
            if d.rules == privileges::diagnostics::RulesState::Duplicate {
                out.push(Issue {
                    key: "duplicate-rules",
                    glyph: icons::WARNING,
                    heading: t("Règles udev en double").into(),
                    body: t("Un 60-openrgb.rules dans /etc masque celui fourni par votre distribution. TuxRGB peut le remplacer par un simple complément (l'original est sauvegardé).").into(),
                    action: (!busy).then_some((icons::BUILD, "Corriger", Message::StartHardwareSetup)),
                    secondary: None,
                    notify: false,
                    info: false,
                });
            } else if d.rules_outdated {
                out.push(Issue {
                    key: "rules-outdated",
                    glyph: icons::SHIELD,
                    heading: t("Renforcer la sécurité de l'accès matériel").into(),
                    body: t("Les règles installées par une version précédente laissaient quelques fichiers de claviers de portables modifiables par tous. Une mise à jour les remplace par un accès réservé à votre session.").into(),
                    action: (!busy).then_some((icons::LOCK, "Mettre à jour", Message::StartHardwareSetup)),
                    secondary: None,
                    notify: false,
                    info: false,
                });
            }
            if !d.i2c_dev_loaded && !access_missing {
                out.push(Issue {
                    key: "i2c-dev",
                    glyph: icons::MEMORY,
                    heading: t("Module I2C non chargé").into(),
                    body: t("Sans le module i2c-dev, la carte mère et la RAM ne peuvent pas être pilotées. TuxRGB peut le charger et le configurer pour les prochains démarrages.").into(),
                    action: (!busy).then_some((icons::BUILD, "Charger le module", Message::StartHardwareSetup)),
                    secondary: None,
                    notify: true,
                    info: false,
                });
            }
        }
        if self.smbus_reboot_pending {
            out.push(Issue {
                key: "reboot",
                glyph: icons::RESTART_ALT,
                heading: t("Redémarrage nécessaire").into(),
                body: t("Le correctif SMBus est en place : redémarrez l'ordinateur pour que la RAM et la carte mère soient détectées.").into(),
                action: None,
                secondary: None,
                notify: false,
                info: true,
            });
        } else if let Some(f) = &self.findings
            && f.smbus_unbound
            && !f.acpi_lax
            && !access_missing
        {
            out.push(Issue {
                key: "smbus",
                glyph: icons::MEMORY_ALT,
                heading: t("Accès SMBus refusé par le firmware").into(),
                body: t("La RAM et une partie de l'éclairage de la carte mère restent invisibles tant que le contrôleur SMBus est bloqué. Le correctif s'adapte à votre distribution et ne touche à rien si l'accès fonctionne déjà.").into(),
                action: (!busy).then_some((icons::LOCK_OPEN, "Débloquer l'accès SMBus", Message::EnableSmbus)),
                secondary: Some(("Détails", Message::Navigate(Page::Settings))),
                notify: true,
                info: false,
            });
        }
        let pending = self.unsized_zones();
        if !pending.is_empty() {
            let devices: std::collections::BTreeSet<&str> =
                pending.iter().map(|(name, _)| name.as_str()).collect();
            out.push(Issue {
                key: "argb",
                glyph: icons::MODE_FAN,
                heading: if pending.len() == 1 {
                    t("1 connecteur ARGB à configurer").into()
                } else {
                    trf!("{} connecteurs ARGB à configurer", pending.len())
                },
                body: trf!(
                    "{} : les ventilateurs et bandes branchés sur ces connecteurs restent éteints tant que leur nombre de LED est inconnu (0 LED). La configuration automatique les active avec {} LED chacun, à affiner ensuite si besoin.",
                    devices.into_iter().collect::<Vec<_>>().join(", "),
                    DEFAULT_ARGB_LEDS
                ),
                action: Some((icons::AUTO_AWESOME, "Configuration automatique", Message::AutoConfigureZones)),
                secondary: Some(("Régler à la main", Message::ShowUnsizedZone)),
                notify: true,
                info: false,
            });
        }
        for (heading, body) in crate::desktop::warnings() {
            if self.config.dismissed_notices.contains(heading) {
                continue;
            }
            out.push(Issue {
                key: heading,
                glyph: icons::INFO,
                heading: t(heading).into(),
                body,
                action: Some((
                    icons::SETTINGS,
                    "Voir les réglages",
                    Message::Navigate(Page::Settings),
                )),
                secondary: Some(("Ne plus afficher", Message::DismissNotice(heading))),
                notify: false,
                info: true,
            });
        }
        if self.using_demo {
            out.push(Issue {
                key: "demo",
                glyph: icons::INFO,
                heading: t("Aperçu avec des périphériques virtuels").into(),
                body: t("Aucun matériel RGB n'est encore détecté : la disposition affichée est une démonstration.").into(),
                action: Some((icons::REFRESH, "Relancer la détection", Message::Rescan)),
                secondary: None,
                notify: false,
                info: true,
            });
        }
        out
    }

    /// Desktop notifications for new problems while no window is open.
    fn notify_issues(&mut self) {
        if self.window.is_some() || crate::args().screenshot_tour.is_some() || self.wizard.is_some()
        {
            return;
        }
        let Some(s) = &self.services else { return };
        for issue in self.issues() {
            if issue.notify && self.notified.insert(issue.key) {
                s.send(ServiceCommand::Notify {
                    summary: issue.heading.clone(),
                    body: issue.body.clone(),
                    action: issue.action.as_ref().map(|(_, label, _)| {
                        (issue.key.to_owned(), crate::i18n::t(label).to_owned())
                    }),
                });
            }
        }
    }

    /// Empty addressable zones of real devices: (device name, zone index).
    pub fn unsized_zones(&self) -> Vec<(String, usize)> {
        self.devices
            .iter()
            .flat_map(|d| {
                d.zones
                    .iter()
                    .enumerate()
                    .filter(|(_, z)| z.needs_size())
                    .map(|(i, _)| (d.display_name().to_owned(), i))
            })
            .collect()
    }

    /// Re-applies the ARGB zone sizes saved in config.json that OpenRGB does
    /// not report (its own configuration was reset or lost).
    fn restore_zone_sizes(&mut self) {
        let mut list = Vec::new();
        for saved in &self.config.zone_sizes {
            let Some(d) = self.devices.iter().find(|d| d.key == saved.device) else {
                continue;
            };
            let Some(z) = d.zones.get(saved.zone) else {
                continue;
            };
            // Once per zone and session: a size OpenRGB refuses must not
            // turn into a resize / reload loop.
            if z.resizable()
                && z.name == saved.name
                && z.led_count != saved.leds
                && self
                    .restore_attempted
                    .insert((saved.device.clone(), saved.zone))
            {
                list.push(ZoneResize {
                    device: saved.device.clone(),
                    zone: saved.zone,
                    leds: saved.leds.clamp(z.leds_min, z.leds_max),
                });
            }
        }
        if !list.is_empty()
            && let Some(s) = &self.services
        {
            tracing::info!(count = list.len(), "restoring saved ARGB zone sizes");
            s.bridge(BridgeCommand::ResizeZones(list));
        }
    }

    /// Zone `zone` of the real device `key` (resizable zones editor).
    pub fn zone_info(&self, key: &DeviceKey, zone: usize) -> Option<tuxrgb_core::ZoneInfo> {
        self.devices
            .iter()
            .find(|d| d.key == *key)
            .and_then(|d| d.zones.get(zone).cloned())
    }

    /// The live LED preview (30 Hz, re-renders the page each time) only runs
    /// while a page that shows it is on screen.
    fn push_preview(&self) {
        if let Some(s) = &self.services {
            let wanted = self.window.is_some() && self.page != Page::Settings;
            s.send(ServiceCommand::SetPreview(wanted));
        }
    }

    /// Demo devices only feed the preview: with no window open the engine
    /// has nothing to render for and sleeps.
    fn push_suspended(&self) {
        if let Some(s) = &self.services {
            s.engine(EngineCommand::SetSuspended(
                self.window.is_none() && self.using_demo,
            ));
        }
    }

    fn push_scene(&self) {
        if let Some(s) = &self.services {
            s.engine(EngineCommand::SetScene(Arc::new(EngineScene::from_scene(
                &self.scene,
            ))));
        }
    }

    fn push_params(&self) {
        if let Some(s) = &self.services {
            s.engine(EngineCommand::SetParams(Arc::new(
                self.config.params.clone(),
            )));
        }
    }

    /// Real devices if any, otherwise the demo rig (preview only).
    fn rebuild_scene(&mut self) {
        // The selection follows its device (indices shift when a device is
        // plugged in or removed).
        let selected_key = self
            .selected_device
            .and_then(|i| self.scene.devices.get(i))
            .map(|d| d.info.key.clone());
        let real = !self.devices.is_empty();
        let source: Vec<DeviceInfo> = if real {
            self.devices.clone()
        } else if self.config.show_demo_devices {
            demo_devices()
        } else {
            Vec::new()
        };
        self.using_demo = !real && !source.is_empty();
        self.scene = Scene::build(&source, &self.config.layout, &self.config.disabled_devices);
        let now = Instant::now();
        for d in &self.scene.devices {
            self.device_seen.entry(d.info.key.clone()).or_insert(now);
        }
        self.selected_device =
            selected_key.and_then(|key| self.scene.devices.iter().position(|d| d.info.key == key));
        self.canvas_cache.clear();
        self.push_scene();
        self.push_suspended();
    }

    fn select_effect(&mut self, kind: EffectKind) {
        self.config.effect = kind;
        for (i, e) in EffectKind::ALL.iter().enumerate() {
            let spec = self.spatial();
            self.cards[i].set_spec(spec);
            self.cards[i].set_target(if *e == kind { 1.0 } else { 0.0 });
        }
        self.picker = None;
        if let Some(s) = &self.services {
            s.engine(EngineCommand::SetEffect(kind));
        }
        self.push_capture();
        self.push_tray();
        self.mark_dirty();
    }

    fn begin_quit(&mut self) -> Task<Message> {
        self.begin_quit_with(self.config.behavior.turn_off_on_exit)
    }

    fn begin_quit_with(&mut self, turn_off: bool) -> Task<Message> {
        if self.quitting {
            return Task::none();
        }
        self.quitting = true;
        if let Err(e) = self.config.save_now() {
            tracing::warn!("cannot save configuration: {e}");
        }
        // A normal exit is not a GPU crash.
        let _ = std::fs::remove_file(&paths::paths().gpu_marker);
        match &self.services {
            Some(s) => {
                if turn_off {
                    s.engine(EngineCommand::SetOutputEnabled {
                        on: false,
                        fade: false,
                    });
                }
                s.send(ServiceCommand::Shutdown { turn_off });
                // Safety net if the services never confirm.
                Task::perform(tokio::time::sleep(Duration::from_secs(6)), |_| {
                    Message::ForceExit
                })
            }
            None => iced::exit(),
        }
    }

    fn bridge_config(&self) -> BridgeConfig {
        let p = paths::paths();
        BridgeConfig {
            mode: self.config.server.to_mode(),
            openrgb_config_dir: p.openrgb_config_dir.clone(),
            log_path: p.openrgb_log.clone(),
            client_name: "TuxRGB".into(),
            disabled: self.config.disabled_devices.clone(),
        }
    }

    fn finish_onboarding(&mut self) {
        self.onboarding.completed = true;
        self.onboarding.completed_with = crate::version_label().into();
        let _ = self.onboarding.save();
        self.wizard = None;
    }

    // ------------------------------------------------------------------
    // Update
    // ------------------------------------------------------------------

    pub fn update(&mut self, message: Message) -> Task<Message> {
        // Frames and preview images change nothing the banners depend on.
        let light = matches!(
            message,
            Message::Tick(_) | Message::Service(ServiceEvent::Preview(_))
        );
        let task = self.handle(message);
        self.after_update(light);
        task
    }

    /// After every message: start the animations of whatever changed (view,
    /// dialog, picker, banners) so nothing on screen switches abruptly.
    fn after_update(&mut self, light: bool) {
        // Views: page, settings sub-view, wizard steps, wizard ↔ app.
        let key = self.view_key();
        if key != self.view_seen {
            let from = match self.view_seen {
                ViewKey::Main(p, v) => Some(ViewSnap::Main(p, v)),
                ViewKey::Wizard(_) => self
                    .wizard_seen
                    .clone()
                    .map(|w| ViewSnap::Wizard(Box::new(w))),
            };
            let whole =
                matches!(key, ViewKey::Wizard(_)) || matches!(self.view_seen, ViewKey::Wizard(_));
            self.transition = match from {
                Some(from) if self.window.is_some() && !self.reduced_motion() => {
                    Some(ViewTransition { from, whole })
                }
                _ => None,
            };
            self.transition_t = if self.transition.is_some() { 0.0 } else { 1.0 };
            self.view_seen = key;
        }
        if self.wizard.is_some() || self.wizard_seen.is_some() {
            self.wizard_seen = self.wizard.clone();
        }

        // Dialogs (also reopened while still fading out).
        let modal = if self.confirm_reset {
            Some(Modal::ResetAccess)
        } else {
            self.license.map(Modal::License)
        };
        match modal {
            Some(m) => {
                if self.modal_t.target() != 1.0 {
                    self.modal_t.set_spec(self.spatial());
                    if self.reduced_motion() {
                        self.modal_t.snap(1.0);
                    } else {
                        self.modal_t.set_target(1.0);
                    }
                }
                self.modal = Some(m);
            }
            None if self.modal.is_some() && self.modal_t.target() != 0.0 => {
                self.modal_t.set_spec(motion::DEFAULT_EFFECTS);
                if self.reduced_motion() {
                    self.modal_t.snap(0.0);
                } else {
                    self.modal_t.set_target(0.0);
                }
            }
            None => {}
        }

        // Inline colour picker: keep the previous one while it collapses.
        if self.picker != self.picker_seen {
            if self.picker_seen.is_some() {
                self.picker_last = self.picker_seen;
            }
            self.picker_seen = self.picker;
        }
        if self.picker_last.is_some() && self.picker_last == self.picker {
            self.picker_last = None;
        }

        if !light {
            self.sync_banners();
        }
        ui::backdrop::set_preview_dark(self.roles.dark);
    }

    /// What currently fills the window.
    pub fn view_key(&self) -> ViewKey {
        match &self.wizard {
            Some(w) => ViewKey::Wizard(w.step),
            None => ViewKey::Main(self.page, self.settings_view),
        }
    }

    /// Banners of the home page (problems first, at most two; an
    /// information only when there is no problem) and of the devices page.
    pub fn home_issues(&self) -> Vec<Issue> {
        let mut shown = Vec::new();
        let problems = self.issues().iter().filter(|i| !i.info).count();
        for issue in self.issues() {
            if shown.len() == 2 || (issue.info && problems > 0) {
                continue;
            }
            let info = issue.info;
            shown.push(issue);
            if info {
                break;
            }
        }
        shown
    }

    fn sync_banners(&mut self) {
        let home = self.home_issues();
        let devices: Vec<Issue> = self
            .issues()
            .into_iter()
            .filter(|i| i.key == "argb")
            .collect();
        self.home_banners
            .sync(home.into_iter().map(|i| (i.key, i)).collect());
        self.device_banners
            .sync(devices.into_iter().map(|i| (i.key, i)).collect());
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Navigate(page) => {
                if page != self.page {
                    self.page = page;
                    self.settings_view = SettingsView::Main;
                    self.picker = None;
                    let spec = self.spatial();
                    for (i, s) in self.nav.iter_mut().enumerate() {
                        s.set_spec(spec);
                        s.set_target(if i == page.index() { 1.0 } else { 0.0 });
                    }
                    self.push_preview();
                }
                Task::none()
            }
            Message::ShowSettings(view) => {
                self.settings_view = view;
                self.license = None;
                Task::none()
            }
            Message::WindowOpened => {
                self.last_tick = None;
                if crate::args().screenshot_tour.is_some() {
                    return Task::perform(tokio::time::sleep(Duration::from_secs(3)), |_| {
                        Message::TourStep(0)
                    });
                }
                // Opened by the user: full size. The request is also sent
                // after the first configure, for compositors that ignore
                // the initial maximised state.
                let gpu_ok = Task::perform(tokio::time::sleep(Duration::from_secs(4)), |_| {
                    Message::GpuOk
                });
                // Developer check of the minimised behaviour (no preview
                // work while the window is not drawn).
                if let (Some(secs), Some(id)) = (crate::tour::dev_minimize_after(), self.window) {
                    return Task::batch([
                        gpu_ok,
                        Task::perform(tokio::time::sleep(Duration::from_secs(secs)), move |_| id)
                            .then(|id| window::minimize(id, true)),
                    ]);
                }
                if let (Some(secs), Some(id)) = (crate::tour::dev_close_after(), self.window) {
                    return Task::batch([
                        gpu_ok,
                        Task::perform(tokio::time::sleep(Duration::from_secs(secs)), move |_| {
                            Message::WindowCloseRequested(id)
                        }),
                    ]);
                }
                match self.window {
                    Some(id) => Task::batch([
                        gpu_ok,
                        window::maximize(id, true),
                        Task::perform(tokio::time::sleep(Duration::from_millis(250)), move |_| {
                            Message::Maximize(id)
                        }),
                    ]),
                    None => Task::none(),
                }
            }
            Message::GpuOk => {
                let _ = std::fs::remove_file(&paths::paths().gpu_marker);
                Task::none()
            }
            Message::Maximize(id) => {
                if self.window == Some(id) {
                    window::maximize(id, true)
                } else {
                    Task::none()
                }
            }
            Message::Noop => Task::none(),
            Message::TourStep(i) => {
                let Some(stop) = crate::tour::stops().get(i) else {
                    return iced::exit();
                };
                if self.tour_frozen {
                    // Resume from where the animations were held.
                    self.tour_frozen = false;
                    self.last_tick = None;
                }
                self.config_dirty = false;
                if stop.starts_with("m-") {
                    match *stop {
                        "m-page-out" => {
                            let _ = self.update(Message::Navigate(Page::Effects));
                        }
                        "m-picker-open" => {
                            let _ = self
                                .update(Message::OpenPicker(EffectKind::GradientWave, "color_a"));
                        }
                        "m-effect-swap" => {
                            let _ = self.update(Message::SelectEffect(EffectKind::Aurora));
                        }
                        "m-dialog-in" => {
                            let _ = self.update(Message::ShowLicense(Some(License::Mit)));
                        }
                        "m-dialog-out" => {
                            let _ = self.update(Message::ShowLicense(None));
                        }
                        "m-snackbar" => {
                            let _ = self.snackbar("Configuration enregistrée");
                        }
                        "m-page-out2" => {
                            self.wizard = None;
                            let _ = self.update(Message::Navigate(Page::Settings));
                        }
                        "m-wizard" => {
                            self.wizard = Some(Wizard {
                                step: WizardStep::Welcome,
                                result: None,
                                from_reset: false,
                                autostart: true,
                            });
                        }
                        _ => {}
                    }
                    let delay = crate::tour::motion_delay(stop);
                    return Task::perform(
                        tokio::time::sleep(Duration::from_millis(delay)),
                        move |_| Message::TourShootNow(i),
                    );
                }
                match *stop {
                    "onboarding" => {}
                    "home" => {
                        self.wizard = None;
                        let _ = self.update(Message::Navigate(Page::Home));
                    }
                    "effects" => {
                        let _ = self.update(Message::Navigate(Page::Effects));
                    }
                    "effects-picker" => {
                        self.select_effect(EffectKind::GradientWave);
                        self.picker = Some((EffectKind::GradientWave, "color_a"));
                        self.hex_input = self
                            .config
                            .params
                            .color(EffectKind::GradientWave, "color_a")
                            .to_hex();
                    }
                    "effects-palette" => {
                        self.picker = None;
                        self.select_effect(EffectKind::Aurora);
                        self.config.params.set(
                            EffectKind::Aurora,
                            tuxrgb_core::effects::SYSTEM_PALETTE_KEY,
                            ParamValue::Toggle(true),
                        );
                        self.push_params();
                    }
                    "effects-monitor" => {
                        self.select_effect(EffectKind::HardwareMonitor);
                    }
                    "devices" => {
                        self.picker = None;
                        self.select_effect(EffectKind::RainbowWave);
                        let _ = self.update(Message::Navigate(Page::Devices));
                        self.selected_device = Some(0);
                        self.canvas_cache.clear();
                    }
                    "settings" => {
                        let _ = self.update(Message::Navigate(Page::Settings));
                    }
                    "settings-updates" => {
                        // A newer version on offer, as after a check.
                        let mut next = crate::update::current_version();
                        next.patch += 1;
                        self.update = crate::update::State::Available(crate::update::Release {
                            version: next.to_string(),
                            page: format!("{}/releases/latest", crate::update::REPO_URL),
                            asset: Some(crate::update::Asset {
                                name: String::new(),
                                url: String::new(),
                                size: 0,
                            }),
                            checksum: None,
                        });
                        return Task::batch([
                            iced::widget::operation::snap_to_end(ui::settings::SCROLL_ID),
                            Task::perform(
                                tokio::time::sleep(Duration::from_millis(1800)),
                                move |_| Message::TourShoot(i),
                            ),
                        ]);
                    }
                    "credits" => {
                        self.settings_view = SettingsView::Credits;
                    }
                    _ => {}
                }
                Task::perform(tokio::time::sleep(Duration::from_millis(1800)), move |_| {
                    Message::TourShoot(i)
                })
            }
            Message::TourShoot(i) => match self.window {
                _ if self.is_animating() => {
                    tracing::info!(
                        nav = ?self.nav.iter().map(|s| s.value()).collect::<Vec<_>>(),
                        transition = self.transition_t,
                        "tour: still animating, waiting"
                    );
                    Task::perform(tokio::time::sleep(Duration::from_millis(400)), move |_| {
                        Message::TourShoot(i)
                    })
                }
                Some(id) => window::screenshot(id).map(move |s| Message::TourSaved(i, s)),
                None => iced::exit(),
            },
            Message::TourShootNow(i) => {
                self.tour_frozen = true;
                Task::perform(tokio::time::sleep(Duration::from_millis(250)), move |_| {
                    Message::TourShootFrozen(i)
                })
            }
            Message::TourShootFrozen(i) => match self.window {
                Some(id) => window::screenshot(id).map(move |s| Message::TourSaved(i, s)),
                None => iced::exit(),
            },
            Message::TourSaved(i, shot) => {
                if let Some(dir) = &crate::args().screenshot_tour {
                    let _ = std::fs::create_dir_all(dir);
                    let path = dir.join(format!("{:02}-{}.png", i + 1, crate::tour::stops()[i]));
                    match crate::tour::write_png(
                        &path,
                        shot.size.width,
                        shot.size.height,
                        &shot.rgba,
                    ) {
                        Ok(()) => tracing::info!("screenshot saved: {}", path.display()),
                        Err(e) => tracing::warn!("screenshot failed: {e}"),
                    }
                }
                Task::done(Message::TourStep(i + 1))
            }
            Message::WindowCloseRequested(id) => {
                if self.config.behavior.close_to_tray
                    && self.tray_available
                    && self.wizard.is_none()
                {
                    window::close(id)
                } else {
                    self.begin_quit()
                }
            }
            Message::WindowFocus(focused) => {
                self.window_focused = focused;
                Task::none()
            }
            Message::WindowClosed(id) => {
                if self.window == Some(id) {
                    self.window = None;
                    self.window_focused = false;
                    if std::mem::take(&mut self.reopen_pending) {
                        return self.open_window();
                    }
                    // Free the preview and stop the feed: zero GUI work in tray.
                    self.preview = None;
                    self.backdrop = None;
                    self.picker = None;
                    if let Some(s) = &self.services {
                        s.send(ServiceCommand::SetPreview(false));
                    }
                    self.push_suspended();
                    let _ = self.config.save();
                    self.config_dirty = false;
                    if !self.tray_available && !self.quitting {
                        return self.begin_quit();
                    }
                    // Its renderer (GPU device, glyph and image caches) has
                    // just been freed: in the tray for hours, TuxRGB should
                    // not keep that memory reserved.
                    return Task::perform(tokio::time::sleep(Duration::from_millis(500)), |_| {
                        Message::TrimMemory
                    });
                }
                if !self.tray_available && !self.quitting {
                    return self.begin_quit();
                }
                Task::none()
            }
            Message::TrimMemory => {
                if self.window.is_none() {
                    paths::trim_memory();
                }
                Task::none()
            }
            Message::ForceExit => iced::exit(),
            Message::Quit => self.begin_quit(),
            Message::Tick(_) if self.tour_frozen => Task::none(),
            Message::Tick(now) => {
                // Animation time: real time at the chosen animation speed.
                let dt = crate::theme::motion::scaled(
                    self.last_tick
                        .map(|t| now.saturating_duration_since(t).as_secs_f32())
                        .unwrap_or(1.0 / 60.0)
                        .min(0.05),
                );
                self.last_tick = Some(now);
                if self.theme_t.is_moving() {
                    self.theme_t.step(dt);
                    self.roles =
                        Roles::lerp(&self.roles_from, &self.roles_to, self.theme_t.value());
                    self.canvas_cache.clear();
                    self.refresh_backdrop();
                }
                for s in self.nav.iter_mut().chain(self.cards.iter_mut()) {
                    s.step(dt);
                }
                if self.transition.is_some() {
                    self.transition_t += dt / VIEW_FADE.as_secs_f32();
                    if self.transition_t >= 1.0 {
                        self.transition = None;
                        self.transition_t = 1.0;
                    }
                }
                if self.devices_appearing() {
                    self.canvas_cache.clear();
                }
                self.modal_t.step(dt);
                if self.modal.is_some()
                    && self.modal_t.value() <= 0.001
                    && !self.modal_t.is_moving()
                    && !self.confirm_reset
                    && self.license.is_none()
                {
                    self.modal = None;
                }
                self.snack_in.step(dt);
                if self.snack_closing && !self.snack_in.is_moving() {
                    self.snack_closing = false;
                    self.snackbar = None;
                }
                if !self.is_animating() {
                    self.last_tick = None;
                }
                Task::none()
            }
            Message::Service(event) => self.on_service(event),

            // ---------------- Effects ----------------
            Message::SelectEffect(kind) => {
                self.select_effect(kind);
                Task::none()
            }
            Message::SetParam(kind, key, value) => {
                self.config.params.set(kind, key, value);
                self.push_params();
                if kind == EffectKind::Shader {
                    self.push_capture();
                }
                self.mark_dirty();
                Task::none()
            }
            Message::ResetParams(kind) => {
                self.config.params.reset(kind);
                self.push_params();
                if kind == EffectKind::Shader {
                    self.push_capture();
                }
                self.mark_dirty();
                Task::none()
            }
            Message::OpenPicker(kind, key) => {
                self.picker = if self.picker == Some((kind, key)) {
                    None
                } else {
                    Some((kind, key))
                };
                self.hex_input = self.config.params.color(kind, key).to_hex();
                Task::none()
            }
            Message::ClosePicker => {
                self.picker = None;
                Task::none()
            }
            Message::PickerChanged(color) => {
                if let Some((kind, key)) = self.picker {
                    self.hex_input = color.to_hex();
                    return self.update(Message::SetParam(kind, key, ParamValue::Color(color)));
                }
                Task::none()
            }
            Message::HexInput(s) => {
                self.hex_input = s.clone();
                if let (Some((kind, key)), Some(c)) = (self.picker, Rgb::from_hex(&s)) {
                    return self.update(Message::SetParam(kind, key, ParamValue::Color(c)));
                }
                Task::none()
            }
            Message::SetBrightness(b) => {
                let b = b.clamp(0.0, 1.0);
                // Pointer moves inside the same slider step: nothing to send.
                if (b - self.config.brightness).abs() < 1e-4 {
                    return Task::none();
                }
                self.config.brightness = b;
                if let Some(s) = &self.services {
                    s.engine(EngineCommand::SetBrightness(self.config.brightness));
                }
                self.push_tray();
                self.mark_dirty();
                Task::none()
            }
            Message::SetSaturation(v) => {
                let v = v.clamp(0.0, 2.0);
                if (v - self.config.saturation).abs() < 1e-4 {
                    return Task::none();
                }
                self.config.saturation = v;
                if let Some(s) = &self.services {
                    s.engine(EngineCommand::SetSaturation(self.config.saturation));
                }
                self.mark_dirty();
                Task::none()
            }
            Message::SetOutput(on) => {
                self.config.output_enabled = on;
                if let Some(s) = &self.services {
                    s.engine(EngineCommand::SetOutputEnabled { on, fade: true });
                }
                self.push_capture();
                self.push_tray();
                self.mark_dirty();
                Task::none()
            }

            // ---------------- Devices ----------------
            Message::CanvasSelect(i) => {
                if self.selected_device != i {
                    self.selected_device = i;
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            Message::CanvasMove(i, placement) => {
                self.scene.set_placement(i, placement);
                self.canvas_cache.clear();
                Task::none()
            }
            Message::CanvasDragEnd => {
                for d in &self.scene.devices {
                    self.config.layout.insert(d.info.key.clone(), d.placement);
                }
                self.push_scene();
                self.mark_dirty();
                Task::none()
            }
            Message::ResizeSelected(scale) => {
                if let Some(i) = self.selected_device
                    && let Some((kind, key, current)) = self
                        .scene
                        .devices
                        .get(i)
                        .map(|d| (d.info.kind, d.info.key.clone(), d.placement))
                {
                    let (w, h) = current.oriented_size(kind.default_size());
                    let (cx, cy) = current.center();
                    let (w, h) = ((w * scale).clamp(0.02, 1.0), (h * scale).clamp(0.02, 1.0));
                    let p = Placement {
                        x: cx - w * 0.5,
                        y: cy - h * 0.5,
                        w,
                        h,
                        ..current
                    };
                    self.scene.set_placement(i, p);
                    self.config
                        .layout
                        .insert(key, self.scene.devices[i].placement);
                    self.canvas_cache.clear();
                    self.push_scene();
                    self.mark_dirty();
                }
                Task::none()
            }
            Message::RotateSelected | Message::MirrorSelected => {
                let rotate = matches!(message, Message::RotateSelected);
                if let Some(i) = self.selected_device
                    && let Some((key, current)) = self
                        .scene
                        .devices
                        .get(i)
                        .map(|d| (d.info.key.clone(), d.placement))
                {
                    let p = if rotate {
                        current.rotated_cw()
                    } else {
                        Placement {
                            mirror: !current.mirror,
                            ..current
                        }
                    };
                    self.scene.set_placement(i, p);
                    self.config
                        .layout
                        .insert(key, self.scene.devices[i].placement);
                    self.canvas_cache.clear();
                    self.push_scene();
                    self.mark_dirty();
                }
                Task::none()
            }
            Message::ToggleDevice(key, enabled) => {
                if enabled {
                    self.config.disabled_devices.remove(&key);
                } else {
                    self.config.disabled_devices.insert(key);
                }
                if let Some(s) = &self.services {
                    s.bridge(BridgeCommand::SetDisabled(
                        self.config.disabled_devices.clone(),
                    ));
                }
                self.rebuild_scene();
                self.mark_dirty();
                Task::none()
            }
            Message::Identify(id) => {
                if let Some(s) = &self.services {
                    s.engine(EngineCommand::Identify(id));
                }
                Task::none()
            }
            Message::ResetLayout => {
                self.config.layout.clear();
                self.rebuild_scene();
                self.mark_dirty();
                self.snackbar("Disposition par défaut restaurée")
            }
            Message::ZoneSizeInput(key, zone, value) => {
                let digits: String = value.chars().filter(char::is_ascii_digit).take(4).collect();
                self.zone_edits.insert((key, zone), digits);
                Task::none()
            }
            Message::ZoneSizeStep(key, zone, delta) => {
                let Some(z) = self.zone_info(&key, zone) else {
                    return Task::none();
                };
                let current = self
                    .zone_edits
                    .get(&(key.clone(), zone))
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(z.led_count as i64);
                let next = (current + delta).clamp(z.leds_min as i64, z.leds_max as i64);
                self.zone_edits.insert((key, zone), next.to_string());
                Task::none()
            }
            Message::ApplyZoneSize(key, zone) => {
                let Some(z) = self.zone_info(&key, zone) else {
                    return Task::none();
                };
                let Some(leds) = self
                    .zone_edits
                    .get(&(key.clone(), zone))
                    .and_then(|v| v.parse::<usize>().ok())
                else {
                    return Task::none();
                };
                let leds = leds.clamp(z.leds_min, z.leds_max);
                self.zone_edits.remove(&(key.clone(), zone));
                self.config.set_zone_size(&key, zone, &z.name, leds);
                let _ = self.config.save();
                if let Some(s) = &self.services {
                    s.bridge(BridgeCommand::ResizeZones(vec![ZoneResize {
                        device: key,
                        zone,
                        leds,
                    }]));
                }
                self.snackbar(trf!("{} : {} LED – enregistré", z.name, leds))
            }
            Message::AutoConfigureZones => {
                let mut list = Vec::new();
                for d in &self.devices {
                    for (zi, z) in d.zones.iter().enumerate() {
                        if z.needs_size() {
                            let leds = DEFAULT_ARGB_LEDS.clamp(z.leds_min.max(1), z.leds_max);
                            list.push((d.key.clone(), zi, z.name.clone(), leds));
                        }
                    }
                }
                if list.is_empty() {
                    return Task::none();
                }
                let n = list.len();
                let mut resizes = Vec::with_capacity(n);
                for (key, zi, name, leds) in list {
                    self.config.set_zone_size(&key, zi, &name, leds);
                    resizes.push(ZoneResize {
                        device: key,
                        zone: zi,
                        leds,
                    });
                }
                let _ = self.config.save();
                if let Some(s) = &self.services {
                    s.bridge(BridgeCommand::ResizeZones(resizes));
                }
                self.snackbar(trf!(
                    "{} connecteur(s) ARGB activé(s) avec {} LED – ajustable dans Appareils",
                    n,
                    DEFAULT_ARGB_LEDS
                ))
            }
            Message::ShowUnsizedZone => {
                let first = self
                    .scene
                    .devices
                    .iter()
                    .position(|d| d.info.zones.iter().any(|z| z.needs_size()));
                let task = self.update(Message::Navigate(Page::Devices));
                self.selected_device = first;
                self.canvas_cache.clear();
                task
            }
            Message::Rescan => {
                if let Some(s) = &self.services {
                    s.bridge(BridgeCommand::Rescan);
                }
                self.snackbar("Nouvelle détection du matériel lancée")
            }
            Message::RestartEngine => {
                if let Some(s) = &self.services {
                    s.bridge(BridgeCommand::Restart);
                }
                self.snackbar("Redémarrage du moteur OpenRGB…")
            }
            Message::SetDemoDevices(on) => {
                self.config.show_demo_devices = on;
                self.rebuild_scene();
                self.mark_dirty();
                Task::none()
            }

            // ---------------- Settings ----------------
            Message::SetThemeMode(m) => {
                self.config.appearance.theme = m;
                self.retheme();
                self.mark_dirty();
                Task::none()
            }
            Message::SetAccentSource(a) => {
                self.config.appearance.accent = a;
                self.retheme();
                self.mark_dirty();
                Task::none()
            }
            Message::SetCustomAccent(c) => {
                self.config.appearance.custom_accent = c;
                self.config.appearance.accent = AccentSource::Custom;
                self.retheme();
                self.mark_dirty();
                Task::none()
            }
            Message::SetVariant(v) => {
                self.config.appearance.variant = v;
                self.retheme();
                self.mark_dirty();
                Task::none()
            }
            Message::SetMotion(m) => {
                self.config.appearance.motion = m;
                self.mark_dirty();
                Task::none()
            }
            Message::SetAnimationSpeed(v) => {
                crate::theme::motion::set_time_scale(v);
                self.config.appearance.animation_speed = crate::theme::motion::time_scale();
                self.mark_dirty();
                Task::none()
            }
            Message::SetTransitions(on) => {
                self.config.smooth_transitions = on;
                if let Some(s) = &self.services {
                    s.engine(EngineCommand::SetTransitions(if on { 1.0 } else { 0.0 }));
                }
                self.mark_dirty();
                Task::none()
            }
            Message::SetLanguage(lang) => {
                self.config.language = lang;
                tuxrgb_core::i18n::set_language(lang.resolve());
                self.push_tray();
                self.canvas_cache.clear();
                self.mark_dirty();
                Task::none()
            }
            Message::SetFps(fps) => {
                self.config.fps = fps;
                if let Some(s) = &self.services {
                    s.engine(EngineCommand::SetFps(fps));
                }
                self.mark_dirty();
                Task::none()
            }
            Message::SetRenderer(r) => {
                if r != self.config.renderer {
                    self.config.renderer = r;
                    self.restart_required = true;
                    self.mark_dirty();
                    return self.snackbar("Le moteur de rendu changera au prochain démarrage");
                }
                Task::none()
            }
            Message::SetAutostart(on) => {
                self.config.behavior.autostart = on;
                self.mark_dirty();
                Task::perform(crate::autostart::set_enabled(on), Message::AutostartDone)
            }
            Message::AutostartDone(r) => match r {
                Ok(()) => Task::none(),
                Err(e) => {
                    self.config.behavior.autostart = false;
                    self.snackbar(trf!("Démarrage automatique : {}", e))
                }
            },
            Message::SetStartMinimized(on) => {
                self.config.behavior.start_minimized = on;
                self.mark_dirty();
                Task::none()
            }
            Message::SetCloseToTray(on) => {
                self.config.behavior.close_to_tray = on;
                self.mark_dirty();
                Task::none()
            }
            Message::SetTurnOffOnExit(on) => {
                self.config.behavior.turn_off_on_exit = on;
                self.mark_dirty();
                Task::none()
            }
            Message::SetServerKind(kind) => {
                self.config.server.kind = kind;
                if let Some(s) = &self.services {
                    s.bridge(BridgeCommand::SetMode(self.config.server.to_mode()));
                }
                self.mark_dirty();
                Task::none()
            }
            Message::ShowLicense(l) => {
                self.license = l;
                Task::none()
            }
            Message::OpenUrl(url) => {
                paths::open_url(&url);
                Task::none()
            }

            // ---------------- Hardware access ----------------
            Message::RunDiagnostics => self.diagnostics_task(),
            Message::DiagnosticsDone(d) => {
                self.diagnostics = Some(d.clone());
                let devices = self.devices.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            privileges::report::build(Some(&d), &devices).1
                        })
                        .await
                        .unwrap_or_default()
                    },
                    Message::FindingsDone,
                )
            }
            Message::FindingsDone(f) => {
                self.findings = Some(f);
                self.notify_issues();
                Task::none()
            }
            Message::CopyReport => {
                let diag = self.diagnostics.clone();
                let devices = self.devices.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            privileges::report::build(diag.as_ref(), &devices).0
                        })
                        .await
                        .unwrap_or_default()
                    },
                    Message::ReportReady,
                )
            }
            Message::ReportReady(text) => {
                let path = paths::paths()
                    .openrgb_log
                    .with_file_name("rapport-materiel.txt");
                let saved = paths::write_atomic(&path, &text).is_ok();
                let msg = if saved {
                    trf!(
                        "Rapport copié dans le presse-papiers et enregistré : {}",
                        path.display()
                    )
                } else {
                    crate::i18n::t("Rapport copié dans le presse-papiers").to_owned()
                };
                Task::batch([iced::clipboard::write(text), self.snackbar(msg)])
            }
            Message::StartHardwareSetup => {
                if self.helper_busy {
                    return Task::none();
                }
                self.helper_busy = true;
                if let Some(w) = &mut self.wizard {
                    w.result = None;
                }
                // The bundled OpenRGB's rules are generated at build time
                // (data/udev/60-openrgb.rules, compiled in): no need to run
                // `openrgb --print-udev-rules`, which takes ~9 s before it
                // prints anything. pkexec starts immediately, so the
                // password dialog appears at once.
                // Windows: no udev rules, the setup ignores them.
                let bundled =
                    cfg!(windows) || self.diagnostics.as_ref().is_none_or(|d| d.openrgb_bundled);
                if bundled {
                    return self.update(Message::RulesReady(
                        tuxrgb_openrgb::rules::BUNDLED_RULES.to_owned(),
                    ));
                }
                // A different OpenRGB (TUXRGB_OPENRGB / system): ask it.
                let binary = self
                    .diagnostics
                    .as_ref()
                    .and_then(|d| d.openrgb_binary.clone());
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            tuxrgb_openrgb::rules::best_rules(binary.as_deref())
                        })
                        .await
                        .unwrap_or_else(|_| tuxrgb_openrgb::rules::BUNDLED_RULES.to_owned())
                    },
                    Message::RulesReady,
                )
            }
            Message::RulesReady(rules) => {
                // pkexec first (the dialog is what the user waits for), the
                // copy for the manual fallback instructions afterwards.
                let copy = rules.clone();
                Task::perform(
                    async move {
                        let result = privileges::runner::run_helper("install", Some(rules)).await;
                        #[cfg(not(windows))]
                        let _ = tokio::task::spawn_blocking(move || {
                            paths::write_atomic(
                                &paths::paths().config_dir.join("60-openrgb.rules"),
                                &copy,
                            )
                        })
                        .await;
                        #[cfg(windows)]
                        drop(copy);
                        result
                    },
                    Message::HardwareSetupDone,
                )
            }
            Message::HardwareSetupDone(result) => {
                self.helper_busy = false;
                let ok = result.is_ok();
                let task = match &result {
                    Ok(_) => {
                        self.onboarding.hardware_configured = true;
                        self.onboarding.skipped_hardware = false;
                        let _ = self.onboarding.save();
                        // Windows: administrator mode now configured, the
                        // elevated instance takes over (after the wizard).
                        #[cfg(windows)]
                        if self.wizard.is_none()
                            && crate::win::elevation::take_pending_handover(self.window.is_some())
                        {
                            return self.begin_quit_with(false);
                        }
                        // OpenRGB must reopen devices with the new permissions.
                        if let Some(s) = &self.services {
                            s.bridge(BridgeCommand::Restart);
                        }
                        Task::none()
                    }
                    Err(HelperError::Cancelled) => self.snackbar("Authentification annulée"),
                    Err(e) => self.snackbar(trf!("Échec de la configuration : {}", e)),
                };
                if let Some(w) = &mut self.wizard {
                    w.result = Some(result);
                    if ok {
                        w.step = WizardStep::Done;
                    }
                } else if ok {
                    return Task::batch([
                        self.diagnostics_task(),
                        self.snackbar("Accès matériel configuré"),
                    ]);
                }
                Task::batch([task, self.diagnostics_task()])
            }
            Message::EnableSmbus => {
                if self.helper_busy {
                    return Task::none();
                }
                self.helper_busy = true;
                Task::perform(
                    privileges::runner::run_helper("enable-smbus", None),
                    Message::SmbusDone,
                )
            }
            Message::SmbusDone(result) => {
                self.helper_busy = false;
                match result {
                    Ok(report) if report.steps.iter().any(|s| s == "smbus-already-functional") => {
                        Task::batch([
                            self.diagnostics_task(),
                            self.snackbar(
                                "L'accès SMBus fonctionne déjà : rien n'a été modifié",
                            ),
                        ])
                    }
                    Ok(report) if report.steps.iter().any(|s| s == "reboot-required") => {
                        self.smbus_reboot_pending = true;
                        let method = report
                            .steps
                            .iter()
                            .find_map(|s| s.strip_prefix("karg-tool "))
                            .unwrap_or("")
                            .to_owned();
                        tracing::info!(method, "SMBus fix applied");
                        self.snackbar(
                            "Accès SMBus débloqué : redémarrez l'ordinateur pour détecter la RAM",
                        )
                    }
                    Ok(_) => self.snackbar(
                        "Paramètre déjà actif : si la RAM reste invisible, vérifiez le BIOS (RGB Fusion / SMBus)",
                    ),
                    Err(HelperError::Manual(how)) => {
                        self.smbus_manual = Some(how);
                        self.snackbar("Modification manuelle nécessaire : voir Réglages → Accès matériel")
                    }
                    Err(HelperError::Cancelled) => self.snackbar("Authentification annulée"),
                    Err(e) => self.snackbar(trf!("Échec : {}", e)),
                }
            }
            Message::AskResetAccess(show) => {
                self.confirm_reset = show;
                Task::none()
            }
            Message::ConfirmResetAccess => {
                self.confirm_reset = false;
                self.helper_busy = true;
                Task::perform(
                    privileges::runner::run_helper("reset", None),
                    Message::ResetAccessDone,
                )
            }
            Message::ResetAccessDone(result) => {
                self.helper_busy = false;
                // Reset the start-up configuration and re-run the wizard,
                // whatever the helper outcome (the wizard reinstalls anyway).
                Onboarding::reset();
                self.onboarding = Onboarding::default();
                self.wizard = Some(Wizard {
                    step: WizardStep::Welcome,
                    result: None,
                    from_reset: true,
                    autostart: true,
                });
                let msg = match &result {
                    Ok(_) => crate::i18n::t(
                        "Permissions réinitialisées. L'assistant va reconfigurer l'accès matériel.",
                    )
                    .to_owned(),
                    Err(HelperError::Cancelled) => crate::i18n::t(
                        "Suppression des règles annulée ; l'assistant de configuration est relancé.",
                    )
                    .to_owned(),
                    Err(e) => {
                        trf!("Réinitialisation partielle ({}) ; l'assistant est relancé.", e)
                    }
                };
                if let Some(s) = &self.services {
                    s.bridge(BridgeCommand::Restart);
                }
                let open = self.open_window();
                Task::batch([open, self.diagnostics_task(), self.snackbar(msg)])
            }

            // ---------------- Wizard ----------------
            Message::WizardNext => {
                let mut task = Task::none();
                if let Some(w) = &mut self.wizard {
                    w.step = match w.step {
                        WizardStep::Welcome => WizardStep::Checks,
                        WizardStep::Checks => WizardStep::Access,
                        WizardStep::Access => WizardStep::Done,
                        WizardStep::Done => WizardStep::Done,
                    };
                    if w.step == WizardStep::Checks {
                        task = self.diagnostics_task();
                    }
                }
                task
            }
            Message::WizardBack => {
                if let Some(w) = &mut self.wizard {
                    w.step = match w.step {
                        WizardStep::Checks => WizardStep::Welcome,
                        WizardStep::Access => WizardStep::Checks,
                        WizardStep::Done => WizardStep::Access,
                        s => s,
                    };
                }
                Task::none()
            }
            Message::WizardSkipAccess => {
                self.onboarding.skipped_hardware = true;
                if let Some(w) = &mut self.wizard {
                    w.step = WizardStep::Done;
                }
                Task::none()
            }
            Message::WizardAutostart(on) => {
                if let Some(w) = &mut self.wizard {
                    w.autostart = on;
                }
                Task::none()
            }
            Message::WizardFinish => {
                let autostart = self.wizard.as_ref().map(|w| w.autostart);
                self.finish_onboarding();
                self.page = Page::Home;
                let task = match autostart {
                    Some(on) if on != self.config.behavior.autostart || on => {
                        self.update(Message::SetAutostart(on))
                    }
                    _ => Task::none(),
                };
                // Windows: administrator mode was configured in the wizard.
                #[cfg(windows)]
                if crate::win::elevation::take_pending_handover(self.window.is_some()) {
                    return Task::batch([task, self.begin_quit_with(false)]);
                }
                task
            }

            Message::DismissNotice(key) => {
                self.config.dismissed_notices.insert(key.to_owned());
                self.mark_dirty();
                Task::none()
            }
            Message::RestoreOutput => {
                if let Some(s) = &self.services {
                    s.engine(EngineCommand::SetOutputEnabled {
                        on: self.config.output_enabled,
                        fade: true,
                    });
                }
                self.push_capture();
                Task::none()
            }
            Message::CheckUpdates => {
                if self.update.busy() {
                    return Task::none();
                }
                self.update = crate::update::State::Checking;
                Task::perform(crate::update::check(), |result| Message::UpdateChecked {
                    manual: true,
                    result,
                })
            }
            Message::AutoCheckUpdates => {
                let now = unix_now();
                let recent = now.saturating_sub(self.config.updates.last_check)
                    < UPDATE_CHECK_INTERVAL.as_secs();
                if !self.config.updates.auto_check
                    || recent
                    || self.update != crate::update::State::Idle
                {
                    return Task::none();
                }
                self.update = crate::update::State::Checking;
                Task::perform(crate::update::check(), |result| Message::UpdateChecked {
                    manual: false,
                    result,
                })
            }
            Message::UpdateChecked { manual, result } => {
                use crate::update::{Check, State};
                self.config.updates.last_check = unix_now();
                self.mark_dirty();
                self.update = match result {
                    Ok(Check::UpToDate(latest)) => State::UpToDate(latest),
                    Ok(Check::Available(release)) => {
                        tracing::info!(version = %release.version, "update available");
                        if crate::update::dev_auto_update() {
                            self.update = State::Available(release);
                            return self.update(Message::InstallUpdate);
                        }
                        State::Available(release)
                    }
                    Err(error) if manual => State::Failed {
                        release: None,
                        error,
                    },
                    Err(error) => {
                        tracing::info!("automatic update check failed: {error}");
                        State::Idle
                    }
                };
                self.notify_issues();
                Task::none()
            }
            Message::SetAutoUpdateCheck(on) => {
                self.config.updates.auto_check = on;
                self.mark_dirty();
                Task::none()
            }
            Message::InstallUpdate => {
                use crate::update::{Method, State};
                let Some(release) = self.update.release().cloned() else {
                    return Task::none();
                };
                if matches!(self.update, State::Downloading { .. } | State::Ready { .. }) {
                    return Task::none();
                }
                if crate::update::method() == Method::Manual || release.asset.is_none() {
                    return self.update(Message::OpenUrl(release.page.clone()));
                }
                self.update = State::Downloading {
                    release: release.clone(),
                    done: 0,
                    total: release.asset.as_ref().map(|a| a.size).unwrap_or(0),
                };
                Task::run(crate::update::download(release), Message::UpdateProgress)
            }
            Message::UpdateProgress(p) => {
                use crate::update::{Progress, Ready, State};
                let Some(release) = self.update.release().cloned() else {
                    return Task::none();
                };
                match p {
                    Progress::Downloading { done, total } => {
                        if matches!(self.update, State::Downloading { .. }) {
                            self.update = State::Downloading {
                                release,
                                done,
                                total,
                            };
                        }
                        Task::none()
                    }
                    Progress::Done(Ok(ready)) => {
                        let installer = matches!(ready, Ready::Installer(_));
                        self.update = State::Ready { release, ready };
                        // Windows: the user asked for the update, the
                        // installer takes over right away.
                        if installer || crate::update::dev_auto_update() {
                            self.update(Message::ApplyUpdate)
                        } else {
                            Task::none()
                        }
                    }
                    Progress::Done(Err(error)) => {
                        tracing::warn!("update download failed: {error}");
                        self.update = State::Failed {
                            release: Some(release),
                            error,
                        };
                        Task::none()
                    }
                }
            }
            Message::ApplyUpdate => {
                let crate::update::State::Ready { ready, .. } = &self.update else {
                    return Task::none();
                };
                match ready.clone() {
                    #[cfg(unix)]
                    crate::update::Ready::AppImage(path) => {
                        self.update(Message::UpdateApplied(crate::update::restart(&path)))
                    }
                    #[cfg(windows)]
                    crate::update::Ready::Installer(path) => Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || crate::update::run_installer(&path))
                                .await
                                .unwrap_or_else(|e| Err(e.to_string()))
                        },
                        Message::UpdateApplied,
                    ),
                    #[allow(unreachable_patterns)]
                    _ => Task::none(),
                }
            }
            Message::UpdateApplied(result) => match result {
                // The LEDs keep their colours: the new version takes over.
                Ok(()) => {
                    tracing::info!("update applied: TuxRGB restarts");
                    self.begin_quit_with(false)
                }
                Err(error) => {
                    let release = self.update.release().cloned();
                    self.update = crate::update::State::Failed { release, error };
                    Task::none()
                }
            },
            Message::DismissSnackbar(id) => {
                if self.snackbar.as_ref().is_some_and(|(i, _)| *i == id) {
                    if self.reduced_motion() || self.window.is_none() {
                        self.snackbar = None;
                    } else {
                        // Slides back down and fades, then goes away.
                        self.snack_closing = true;
                        self.snack_in.set_spec(motion::STANDARD_DEFAULT_SPATIAL);
                        self.snack_in.set_target(0.0);
                    }
                }
                Task::none()
            }
            Message::PickerHidden(kind, key) => {
                if self.picker_last == Some((kind, key)) {
                    self.picker_last = None;
                }
                Task::none()
            }
            Message::BannerHidden(key) => {
                self.home_banners.remove(key);
                self.device_banners.remove(key);
                Task::none()
            }
            Message::SaveConfig => {
                if self.config_dirty {
                    if let Err(e) = self.config.save() {
                        tracing::warn!("cannot save configuration: {e}");
                    }
                    self.config_dirty = false;
                }
                Task::none()
            }
        }
    }

    fn on_service(&mut self, event: ServiceEvent) -> Task<Message> {
        match event {
            ServiceEvent::Ready(handle) => {
                handle.send(ServiceCommand::Start(Box::new(StartConfig {
                    bridge: self.bridge_config(),
                    tray: self.tray_state(),
                })));
                handle.engine(EngineCommand::SetParams(Arc::new(
                    self.config.params.clone(),
                )));
                handle.engine(EngineCommand::SetEffect(self.config.effect));
                handle.engine(EngineCommand::SetTransitions(
                    if self.config.smooth_transitions {
                        1.0
                    } else {
                        0.0
                    },
                ));
                handle.engine(EngineCommand::SetBrightness(self.config.brightness));
                handle.engine(EngineCommand::SetSaturation(self.config.saturation));
                handle.engine(EngineCommand::SetFps(self.config.fps));
                handle.engine(EngineCommand::SetAccent(self.desktop_accent()));
                handle.engine(EngineCommand::SetOutputEnabled {
                    on: self.config.output_enabled,
                    fade: true,
                });
                self.services = Some(handle);
                self.push_preview();
                self.push_scene();
                self.push_suspended();
                self.push_capture();
                Task::none()
            }
            ServiceEvent::Bridge(BridgeEvent::Status(status)) => {
                self.bridge_status = status;
                self.push_tray();
                Task::none()
            }
            ServiceEvent::Bridge(BridgeEvent::Devices(devices)) => {
                let first = self.devices.is_empty() && !devices.is_empty();
                self.devices = devices;
                self.restore_zone_sizes();
                self.rebuild_scene();
                if first {
                    // RAM / SMBus findings depend on what was detected.
                    return self.diagnostics_task();
                }
                self.notify_issues();
                Task::none()
            }
            ServiceEvent::NotificationClicked(fix) => {
                // The issue's own action straight from the notification,
                // then the window for its result.
                let action = fix.and_then(|key| {
                    self.issues()
                        .into_iter()
                        .find(|i| i.key == key)
                        .and_then(|i| i.action.map(|(_, _, m)| m))
                });
                let show = self.bring_to_front();
                match action {
                    Some(m) => Task::batch([show, self.update(m)]),
                    None => show,
                }
            }
            ServiceEvent::Preview(p) => {
                if self.window.is_some() {
                    self.preview = Some(p.frame);
                    // The glow comes ready from the feed (built off this
                    // thread); rebuilt here only if the theme changed since.
                    match p.backdrop {
                        Some((dark, handle)) if dark == self.roles.dark => {
                            self.backdrop = Some(handle);
                        }
                        _ => self.refresh_backdrop(),
                    }
                }
                Task::none()
            }
            ServiceEvent::Appearance(a) => {
                let accent_changed = a.accent != self.system.accent;
                self.system = a;
                self.retheme();
                // Live desktop accent → LEDs (synchronised effects and the
                // system palette), faded in by the engine.
                if accent_changed && let Some(s) = &self.services {
                    s.engine(EngineCommand::SetAccent(self.desktop_accent()));
                }
                if accent_changed && self.config.effect == EffectKind::Shader {
                    self.push_capture();
                }
                Task::none()
            }
            ServiceEvent::Activate(req) => {
                if req.minimized {
                    // Autostart while already running: nothing to show.
                    return Task::none();
                }
                if req.reset_hardware {
                    Onboarding::reset();
                    self.onboarding = Onboarding::default();
                    self.wizard = Some(Wizard {
                        step: WizardStep::Welcome,
                        result: None,
                        from_reset: true,
                        autostart: true,
                    });
                }
                self.bring_to_front()
            }
            // Logout / reboot / `kill`: the LEDs always go dark, whatever
            // the "turn off on exit" preference (that one is for "Quitter").
            ServiceEvent::Terminate => self.begin_quit_with(true),
            ServiceEvent::PowerDown(kind) => {
                // The LEDs are already off; make sure everything is saved so
                // the next start restores exactly this state.
                tracing::info!(?kind, "power down: configuration saved");
                if let Err(e) = self.config.save_now() {
                    tracing::warn!("cannot save configuration: {e}");
                }
                self.config_dirty = false;
                Task::none()
            }
            ServiceEvent::Resumed => {
                // Give USB / SMBus controllers a moment to come back.
                Task::perform(tokio::time::sleep(Duration::from_millis(1200)), |_| {
                    Message::RestoreOutput
                })
            }
            ServiceEvent::Tray(action) => match action {
                TrayAction::ShowWindow => self.bring_to_front(),
                TrayAction::OpenSettings => {
                    self.page = Page::Settings;
                    for (i, s) in self.nav.iter_mut().enumerate() {
                        s.snap(if i == Page::Settings.index() {
                            1.0
                        } else {
                            0.0
                        });
                    }
                    self.bring_to_front()
                }
                TrayAction::SetEffect(e) => {
                    self.select_effect(e);
                    Task::none()
                }
                TrayAction::SetBrightness(b) => self.update(Message::SetBrightness(b)),
                TrayAction::SetOutput(on) => self.update(Message::SetOutput(on)),
                TrayAction::Quit => self.begin_quit(),
            },
            ServiceEvent::TrayAvailable(available) => {
                self.tray_available = available;
                Task::none()
            }
            ServiceEvent::Audio(status) => {
                let task = match &status {
                    CaptureStatus::Failed(e) => {
                        self.snackbar(trf!("Capture audio impossible : {}", e))
                    }
                    _ => Task::none(),
                };
                self.audio_status = status;
                task
            }
            ServiceEvent::Screen(status) => {
                let task = match &status {
                    CaptureStatus::Failed(e) => {
                        self.snackbar(trf!("Capture d'écran impossible : {}", e))
                    }
                    CaptureStatus::Cancelled => self.snackbar("Partage d'écran refusé"),
                    _ => Task::none(),
                };
                self.screen_status = status;
                task
            }
            ServiceEvent::Shader(status) => {
                let task = match &status {
                    CaptureStatus::Failed(e) => self.snackbar(trf!("Shader : {}", e)),
                    _ => Task::none(),
                };
                self.shader_status = status;
                task
            }
            ServiceEvent::ScreencastToken(token) => {
                if token.is_some() && token != self.config.screencast_token {
                    self.config.screencast_token = token;
                    self.mark_dirty();
                }
                Task::none()
            }
            ServiceEvent::Stopped => iced::exit(),
        }
    }
}

#[cfg(test)]
mod presence_tests {
    use super::Presence;

    fn keys(p: &Presence<u32>) -> Vec<(&'static str, bool)> {
        p.items.iter().map(|i| (i.key, i.closing)).collect()
    }

    #[test]
    fn items_close_in_place_reopen_and_go_away() {
        let mut p = Presence::default();
        p.sync(vec![("a", 1), ("b", 2), ("c", 3)]);
        assert!(p.items.iter().all(|i| i.fresh()));
        p.settle();
        assert!(p.items.iter().all(|i| !i.fresh()));
        // "b" no longer applies: it closes where it was.
        p.sync(vec![("a", 1), ("c", 3)]);
        assert_eq!(keys(&p), [("a", false), ("b", true), ("c", false)]);
        // Something new appears while "b" is closing.
        p.sync(vec![("a", 1), ("c", 3), ("d", 4)]);
        assert_eq!(
            keys(&p),
            [("a", false), ("b", true), ("c", false), ("d", false)]
        );
        assert!(p.items[3].fresh() && !p.items[0].fresh());
        // "b" comes back before it finished collapsing: one item, open.
        p.sync(vec![("a", 1), ("b", 5), ("c", 3), ("d", 4)]);
        assert_eq!(
            keys(&p),
            [("a", false), ("b", false), ("c", false), ("d", false)]
        );
        assert_eq!(p.items[1].value, 5);
        // Removal only takes collapsed (closing) items away.
        p.remove("a");
        assert_eq!(p.items.len(), 4);
        p.sync(vec![("b", 5), ("c", 3), ("d", 4)]);
        p.remove("a");
        assert_eq!(keys(&p), [("b", false), ("c", false), ("d", false)]);
    }
}
