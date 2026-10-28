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
use tuxrgb_core::layout::Target;
use tuxrgb_core::{
    DeviceInfo, DeviceKey, EffectKind, ParamValue, PartLighting, Placement, Rgb, Scene,
};
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
    /// Hardware displays (AIO screens, digital displays).
    Displays,
    /// Fan speeds (3.2).
    Fans,
    Settings,
}

impl Page {
    pub const ALL: [Page; 6] = [
        Page::Home,
        Page::Effects,
        Page::Devices,
        Page::Displays,
        Page::Fans,
        Page::Settings,
    ];

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

/// The "network lights" form of the settings.
#[derive(Clone, Debug, Default)]
pub struct NetForm {
    pub wled_host: String,
    pub wled_leds: String,
    /// Name reported by the controller (`/json/info`).
    pub wled_name: String,
    pub wled_probing: bool,
    /// Result of the last detection (text for the user), `Err` in red.
    pub wled_status: Option<Result<String, String>>,
    pub wiz_host: String,
    pub wiz_searching: bool,
    /// Bulbs that answered the last search (not added yet).
    pub wiz_found: Vec<tuxrgb_openrgb::network::WizFound>,
    /// A search ran (so "none found" can be said).
    pub wiz_searched: bool,
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
    /// A canvas item (device or fan of a split header) picked.
    CanvasSelect(Option<Target>),
    CanvasMove(Target, Placement),
    CanvasDragEnd,
    ResizeSelected(f32),
    /// Orientation of the selected item, in degrees clockwise (any angle,
    /// from the rotation slider).
    SetRotation(f32),
    /// Mirrors the selected item (LED order reversed).
    MirrorSelected,
    /// Shows / hides a canvas item (device or fan key); it stays connected
    /// and lit by the effect.
    ToggleHidden(DeviceKey),
    /// Leaves a device to one of its own effects (`None`: TuxRGB's).
    SetHardwareMode(DeviceKey, Option<String>),
    // Network lights (WLED, Philips WiZ)
    NetWledHost(String),
    NetWledLeds(String),
    /// Asks the WLED controller typed in for its name and LED count.
    NetWledProbe,
    NetWledProbed(Result<tuxrgb_openrgb::network::WledInfo, String>),
    NetAddWled,
    NetRemoveWled(usize),
    NetWizHost(String),
    NetAddWiz(String),
    NetRemoveWiz(usize),
    /// Looks for WiZ bulbs on the local network.
    NetWizSearch,
    NetWizFound(Vec<tuxrgb_openrgb::network::WizFound>),
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
    /// Per-zone / per-fan lighting: one part of a zone (fan of a header,
    /// or the whole zone) follows the effect, takes a colour or is dark.
    SetPartLighting(DeviceKey, usize, usize, PartLighting),
    /// How many fans (equal parts) a zone carries.
    SetZoneParts(DeviceKey, usize, usize),
    /// LED count of one fan of a split header (its neighbour takes the
    /// difference).
    SetPartSize(DeviceKey, usize, usize, usize),
    /// The same lighting on every zone and fan of a device.
    SetDeviceLighting(DeviceKey, PartLighting),
    /// Flashes one part of a zone (find which fan is which).
    IdentifyPart(DeviceKey, usize, usize),
    /// A part (zone `usize::MAX`: the whole device) plays an effect of its
    /// own, independent from the global one (TuxRGB 3.1).
    SetOwnEffect(DeviceKey, usize, usize, EffectKind),
    /// A setting of a part's own effect.
    SetOwnParam(DeviceKey, usize, usize, &'static str, ParamValue),
    /// Every device, zone and fan follows the global effect again (fans
    /// stay split, sizes kept).
    FollowGlobalEverywhere,
    /// Opens / closes the own-effect settings of a part.
    OpenOwnEditor(DeviceKey, usize, usize),
    /// Opens / closes the picker of a colour of a part's own effect.
    OpenOwnPicker(DeviceKey, usize, usize, &'static str),
    /// That picker finished collapsing.
    OwnPickerHidden,
    /// Opens / closes the colour picker of a part.
    OpenPartPicker(DeviceKey, usize, usize),
    /// A part's colour picker finished collapsing.
    PartPickerHidden,
    ResetLayout,
    Rescan,
    RestartEngine,
    SetDemoDevices(bool),
    // Hardware displays (AIO screens, digital displays)
    /// Driven by TuxRGB or left alone.
    DisplayEnabled(String, bool),
    DisplayContent(String, crate::displays::Content),
    /// Adds / removes a value shown (the order of the choice is kept).
    DisplayMetric(String, crate::displays::Metric),
    DisplayText(String, String),
    DisplayImagePath(String, String),
    /// Opens the file chooser for a display's picture.
    DisplayPickImage(String),
    DisplayImagePicked(String, Option<std::path::PathBuf>),
    /// Refresh period, seconds.
    DisplayInterval(String, f32),
    /// Seconds each set of values stays (more values than fit at once).
    DisplayCycle(String, f32),
    DisplayBrightness(String, f32),
    DisplayRotation(String, u16),
    DisplayAlarm(String, bool),
    DisplaysFahrenheit(bool),
    DisplaysRescan,
    /// Ventilation page.
    FanMode(String, crate::fans::FanMode),
    FanFixed(String, u8),
    FanSource(String, String),
    FanPreset(String, &'static str),
    /// A point of a curve dragged: (fan, index, (°C, %)).
    FanPoint(String, usize, (u8, u8)),
    /// Renaming a fan: started (`Some`) / text typed / done (`None`).
    FanRename(Option<String>),
    FanRenameInput(String),
    FanIdentify(String),
    FansRescan,
    /// Linux: installs the fan service (Polkit); Windows: administrator mode.
    FansEnable,
    FansEnableDone(Result<HelperReport, HelperError>),
    // Settings
    SetThemeMode(ThemeMode),
    SetAccentSource(AccentSource),
    SetCustomAccent(Rgb),
    SetVariant(SchemeVariant),
    SetMotion(MotionPref),
    SetAnimationSpeed(f32),
    SetTransitions(bool),
    /// LED colour correction (Réglages → Couleurs des LED).
    LedColorEnabled(bool),
    LedGamma(f32),
    /// Weight of one channel (0 red, 1 green, 2 blue).
    LedWhite(usize, f32),
    LedColorReset,
    /// Every LED in white while on, to set the white balance.
    LedTestWhite(bool),
    SetLanguage(LanguagePref),
    SetFps(u32),
    SetRenderer(RendererPref),
    SetAutostart(bool),
    AutostartDone(Result<(), String>),
    SetStartMinimized(bool),
    SetCloseToTray(bool),
    SetTurnOffOnExit(bool),
    SetLedsOffWithScreen(bool),
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
    /// [`Message::LedTestWhite`] (never saved: off when leaving Réglages).
    pub led_test_white: bool,
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
    /// The fan of the selected device picked on the canvas, if any.
    pub selected_segment: Option<usize>,
    /// What is being typed in the network lights section.
    pub net: NetForm,
    /// Hardware displays found (from the displays thread), and their
    /// previews as images (made once per new picture).
    pub displays: Vec<crate::displays::DisplayState>,
    /// Ventilation page: the fans and temperatures, and the fan being
    /// renamed (id, text).
    pub fans: crate::fans::FansState,
    pub fan_rename: Option<(String, String)>,
    /// Every screen is off (DPMS, Windows display state).
    pub screen_off: bool,
    pub display_previews: std::collections::HashMap<String, (usize, iced::widget::image::Handle)>,
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
    pub nav: [Spring; 6],
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
    /// Part (device, zone, part) whose colour picker is open.
    pub part_picker: Option<(DeviceKey, usize, usize)>,
    /// The one closed last, kept while it collapses.
    pub part_picker_last: Option<(DeviceKey, usize, usize)>,
    part_picker_seen: Option<(DeviceKey, usize, usize)>,
    /// Part (device, zone, part) whose own-effect settings are open (they
    /// take the place of the zones under the canvas).
    pub own_editor: Option<(DeviceKey, usize, usize)>,
    /// Colour picker of a part's own effect: (device, zone, part, setting).
    pub own_picker: Option<(DeviceKey, usize, usize, &'static str)>,
    pub own_picker_last: Option<(DeviceKey, usize, usize, &'static str)>,
    own_picker_seen: Option<(DeviceKey, usize, usize, &'static str)>,
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
            selected_segment: None,
            net: NetForm::default(),
            displays: Vec::new(),
            fans: Default::default(),
            fan_rename: None,
            screen_off: false,
            display_previews: std::collections::HashMap::new(),
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
            part_picker: None,
            part_picker_last: None,
            part_picker_seen: None,
            own_editor: None,
            own_picker: None,
            own_picker_last: None,
            own_picker_seen: None,
            license: None,
            restart_required: false,
            led_test_white: false,
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
        // The screens use the accent too.
        self.push_displays();
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

    /// What the effects need: the global one, and the effects of their own
    /// played by devices, zones and fans of the setup.
    fn effect_requirements(&self) -> tuxrgb_core::effects::Requirements {
        let mut req = self.config.effect.requirements();
        for d in &self.scene.devices {
            if !d.enabled || !d.info.controllable || d.info.hardware_mode.is_some() {
                continue;
            }
            for z in d.lighting.values() {
                for p in &z.lighting {
                    if let Some(kind) = p.own_effect() {
                        let r = kind.requirements();
                        req.animated |= r.animated;
                        req.audio |= r.audio;
                        req.screen |= r.screen;
                        req.hardware |= r.hardware;
                        req.shader |= r.shader;
                    }
                }
            }
        }
        req
    }

    /// The LEDs are lit: the switch is on, and the screens are not off (when
    /// the LEDs follow them).
    pub fn effective_output(&self) -> bool {
        self.config.output_enabled
            && !(self.screen_off && self.config.behavior.leds_off_with_screen)
    }

    /// Lights the LEDs, or fades them out, as [`Self::effective_output`]
    /// says (and stops the inputs nothing uses then).
    fn push_output(&self) {
        if let Some(s) = &self.services {
            s.engine(EngineCommand::SetOutputEnabled {
                on: self.effective_output(),
                fade: true,
            });
        }
        self.push_capture();
    }

    fn push_capture(&self) {
        if let Some(s) = &self.services {
            let req = self.effect_requirements();
            let on = self.effective_output();
            let displays = self.displays_sensors();
            let shader = (req.shader && on).then(|| self.shader_settings());
            // Audio-reactive custom shaders get the spectrum too.
            let shader_audio = shader
                .as_ref()
                .is_some_and(|sh| sh.source.contains("iAudio"));
            s.send(ServiceCommand::Capture(CaptureRequest {
                audio: (req.audio || shader_audio) && on,
                screen: req.screen && on,
                // Windows: the fan curves follow the sensor sampler.
                hardware: (req.hardware && on)
                    || displays.is_some()
                    || (cfg!(windows) && self.config.fans.any_driven()),
                hardware_period: displays,
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
    /// The device (as detected, or from the demo rig) with this key.
    fn device_info(&self, key: &DeviceKey) -> Option<DeviceInfo> {
        self.scene
            .devices
            .iter()
            .find(|d| d.info.key == *key)
            .map(|d| d.info.clone())
    }

    /// What a part of a zone shows. `zone == usize::MAX`: the whole device,
    /// `None` when its zones / fans differ.
    fn part_value(&self, device: &DeviceKey, zone: usize, part: usize) -> Option<PartLighting> {
        let lighting = self.config.lighting.get(device);
        if zone != usize::MAX {
            return Some(
                lighting
                    .and_then(|l| l.get(&zone))
                    .map(|z| z.part(part))
                    .unwrap_or_default(),
            );
        }
        let info = self.device_info(device)?;
        let mut seen: Option<PartLighting> = None;
        let none = tuxrgb_core::lighting::ZoneLighting::default();
        for (zi, z) in info.zones.iter().enumerate() {
            let zl = lighting.and_then(|l| l.get(&zi)).unwrap_or(&none);
            for p in 0..tuxrgb_core::lighting::parts_of(z, Some(zl)) {
                let v = zl.part_ref(p);
                match &seen {
                    None => seen = Some(v.clone()),
                    Some(s) if s != v => return None,
                    _ => {}
                }
            }
        }
        Some(seen.unwrap_or_default())
    }

    /// Lighting of a device changed: drop what is back to the default,
    /// give the scene the new settings, the engine crossfades to them.
    fn store_lighting(&mut self, device: &DeviceKey) {
        if let Some(l) = self.config.lighting.get_mut(device) {
            l.retain(|_, z| !z.is_default());
            if l.is_empty() {
                self.config.lighting.remove(device);
            }
        }
        self.scene
            .set_lighting(&self.config.lighting, &self.config.layout);
        if self.selected_target().is_none_or(|t| t.segment.is_none()) {
            self.selected_segment = None;
        }
        self.canvas_cache.clear();
        self.push_scene();
        // An own effect may need the microphone, the screen or the sensors.
        self.push_capture();
        self.mark_dirty();
    }

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
            let wanted = self.window.is_some()
                && !matches!(self.page, Page::Settings | Page::Displays | Page::Fans);
            s.send(ServiceCommand::SetPreview(wanted));
            // The displays page: previews and live values.
            s.send(ServiceCommand::Displays(
                crate::displays::Command::Previews(
                    self.window.is_some() && self.page == Page::Displays,
                ),
            ));
            s.send(ServiceCommand::Fans(crate::fans::Command::Page(
                self.window.is_some() && self.page == Page::Fans,
            )));
        }
    }

    /// Changes the settings of a fan (its defaults when never changed), then
    /// sends every fan's settings.
    fn edit_fan(&mut self, id: &str, f: impl FnOnce(&mut crate::fans::FanPrefs)) {
        let kind = self
            .fans
            .fans
            .iter()
            .find(|x| x.id == id)
            .map_or(crate::fans::FanKind::Motherboard, |x| x.kind);
        let driven = self.config.fans.any_driven();
        f(self
            .config
            .fans
            .fans
            .entry(id.to_owned())
            .or_insert_with(|| crate::fans::FanPrefs::for_fan(kind)));
        if let Some(s) = &self.services {
            s.send(ServiceCommand::Fans(crate::fans::Command::Configure(
                self.config.fans.clone(),
            )));
        }
        // Windows: a driven fan needs the sensor sampler.
        if cfg!(windows) && self.config.fans.any_driven() != driven {
            self.push_capture();
        }
        self.mark_dirty();
    }

    /// The displays' settings (and the demo setup, the accent) to their
    /// thread.
    fn push_displays(&self) {
        if let Some(s) = &self.services {
            s.send(ServiceCommand::Displays(
                crate::displays::Command::Configure {
                    config: self.config.displays.clone(),
                    demo: self.using_demo,
                    accent: self.accent_seed(),
                },
            ));
        }
    }

    /// The displays need the sensors: one is driven with values, or the
    /// displays page shows live values. With the shortest refresh period.
    fn displays_sensors(&self) -> Option<Duration> {
        use crate::displays::Content;
        let page_open = self.window.is_some() && self.page == Page::Displays;
        let mut period: Option<Duration> = None;
        for d in &self.displays {
            let settings = self.config.displays.settings_of(&d.key, &d.caps);
            let values = settings.content == Content::Metrics || !d.caps.is_screen();
            // A demo display has no hardware: values only for the page.
            if (d.driving && values && !d.demo) || (page_open && d.connected) {
                let p = settings.interval().max(d.caps.min_interval);
                period = Some(period.map_or(p, |q: Duration| q.min(p)));
            }
        }
        period.map(|p| p.clamp(Duration::from_millis(250), Duration::from_secs(1)))
    }

    /// Changes the settings of a display (its first settings when never
    /// changed), then sends them.
    fn edit_display(
        &mut self,
        key: &str,
        edit: impl FnOnce(&mut crate::displays::DisplaySettings),
    ) {
        let Some(caps) = self
            .displays
            .iter()
            .find(|d| d.key == key)
            .map(|d| d.caps.clone())
        else {
            return;
        };
        let demo = key.starts_with("demo:");
        let entry = self
            .config
            .displays
            .displays
            .entry(key.to_owned())
            .or_insert_with(|| {
                let mut s = crate::displays::DisplaySettings::for_display(&caps);
                if demo {
                    s.enabled = true;
                }
                s
            });
        edit(entry);
        self.push_displays();
        self.push_capture();
        self.mark_dirty();
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
        let selected_fan = self
            .selected_target()
            .and_then(|t| self.scene.key_of(t))
            .filter(|_| self.selected_segment.is_some())
            .cloned();
        let real = !self.devices.is_empty();
        let mut source: Vec<DeviceInfo> = if real {
            self.devices.clone()
        } else if self.config.show_demo_devices {
            demo_devices()
        } else {
            Vec::new()
        };
        // Demo rig: the hardware effects chosen are only pretended.
        for d in source.iter_mut().filter(|d| d.is_virtual) {
            d.hardware_mode = self
                .config
                .hardware_modes
                .get(&d.key)
                .filter(|m| d.hardware_modes.contains(m))
                .cloned();
        }
        self.using_demo = !real && !source.is_empty();
        self.scene = Scene::build(&source, &self.config.layout, &self.config.disabled_devices);
        self.scene
            .set_lighting(&self.config.lighting, &self.config.layout);
        let now = Instant::now();
        for d in &self.scene.devices {
            self.device_seen.entry(d.info.key.clone()).or_insert(now);
        }
        self.selected_device =
            selected_key.and_then(|key| self.scene.devices.iter().position(|d| d.info.key == key));
        self.selected_segment = selected_fan.and_then(|key| {
            self.selected_device.and_then(|i| {
                self.scene.devices[i]
                    .segments
                    .iter()
                    .position(|s| s.key == key)
            })
        });
        self.canvas_cache.clear();
        self.push_scene();
        self.push_suspended();
        // Devices with an effect of their own may have come or gone.
        self.push_capture();
        // Demo displays come with the demo setup.
        self.push_displays();
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
        // The LEDs first (instantly, no fade; the bridge has a strict time
        // limit), the configuration is written meanwhile.
        if let Some(s) = &self.services {
            if turn_off {
                s.engine(EngineCommand::SetOutputEnabled {
                    on: false,
                    fade: false,
                });
            }
            s.send(ServiceCommand::Shutdown { turn_off });
        }
        if let Err(e) = self.config.save_now() {
            tracing::warn!("cannot save configuration: {e}");
        }
        // A normal exit is not a GPU crash.
        let _ = std::fs::remove_file(&paths::paths().gpu_marker);
        match &self.services {
            // Safety net if the services never confirm.
            Some(_) => Task::perform(tokio::time::sleep(Duration::from_secs(6)), |_| {
                Message::ForceExit
            }),
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
            hardware_modes: self.config.hardware_modes.clone(),
            network: self.config.network.clone(),
            calibration: self.config.led_color,
        }
    }

    /// The LED colour correction changed: the bridge writes the colours
    /// through it, the engine keeps the brightness slider's feel.
    fn push_led_color(&mut self) {
        self.config.led_color = self.config.led_color.sanitized();
        if let Some(s) = &self.services {
            s.bridge(BridgeCommand::SetCalibration(self.config.led_color));
            s.engine(EngineCommand::SetLedGamma(
                self.config.led_color.effective_gamma(),
            ));
        }
        self.mark_dirty();
    }

    fn set_test_white(&mut self, on: bool) {
        if on != self.led_test_white {
            self.led_test_white = on;
            if let Some(s) = &self.services {
                s.engine(EngineCommand::SetTestWhite(on));
            }
        }
    }

    /// The network lights changed: the engine restarts with them.
    fn push_network(&mut self) {
        if let Some(s) = &self.services {
            s.bridge(BridgeCommand::SetNetwork(self.config.network.clone()));
        }
        self.mark_dirty();
    }

    /// The canvas item selected (device, or one of its fans).
    pub fn selected_target(&self) -> Option<Target> {
        let device = self.selected_device?;
        let d = self.scene.devices.get(device)?;
        Some(Target {
            device,
            segment: self.selected_segment.filter(|s| *s < d.segments.len()),
        })
    }

    /// Moves / turns / resizes the selected item and remembers it.
    fn place_selected(&mut self, make: impl FnOnce(Placement) -> Placement) {
        let Some(t) = self.selected_target() else {
            return;
        };
        let Some(current) = self.scene.placement_of(t) else {
            return;
        };
        self.scene.set_target_placement(t, make(current));
        if let (Some(key), Some(p)) = (self.scene.key_of(t).cloned(), self.scene.placement_of(t)) {
            self.config.layout.insert(key, p);
        }
        self.canvas_cache.clear();
        self.push_scene();
        self.mark_dirty();
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
        // Frames, preview images and pointer drags (sliders, canvas, text
        // typed) change nothing the banners depend on: those messages come
        // dozens of times a second and go straight to the screen.
        let light = matches!(
            message,
            Message::Tick(_)
                | Message::Service(ServiceEvent::Preview(_))
                | Message::CanvasMove(..)
                | Message::SetBrightness(_)
                | Message::SetSaturation(_)
                | Message::LedGamma(_)
                | Message::LedWhite(..)
                | Message::SetParam(..)
                | Message::SetOwnParam(..)
                | Message::DisplayInterval(..)
                | Message::DisplayCycle(..)
                | Message::DisplayBrightness(..)
                | Message::DisplayText(..)
                | Message::DisplayImagePath(..)
                | Message::Service(ServiceEvent::Displays(_))
                | Message::Service(ServiceEvent::Fans(_))
                | Message::FanPoint(..)
                | Message::FanFixed(..)
                | Message::FanRenameInput(_)
                | Message::PickerChanged(_)
                | Message::HexInput(_)
                | Message::ResizeSelected(_)
                | Message::SetRotation(_)
                | Message::ZoneSizeInput(..)
                | Message::NetWledHost(_)
                | Message::NetWledLeds(_)
                | Message::NetWizHost(_)
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
        if self.part_picker != self.part_picker_seen {
            if self.part_picker_seen.is_some() {
                self.part_picker_last = self.part_picker_seen.clone();
            }
            self.part_picker_seen = self.part_picker.clone();
        }
        if self.part_picker_last.is_some() && self.part_picker_last == self.part_picker {
            self.part_picker_last = None;
        }
        if self.own_picker != self.own_picker_seen {
            if self.own_picker_seen.is_some() {
                self.own_picker_last = self.own_picker_seen.clone();
            }
            self.own_picker_seen = self.own_picker.clone();
        }
        if self.own_picker_last.is_some() && self.own_picker_last == self.own_picker {
            self.own_picker_last = None;
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
    fn home_issues_of(all: Vec<Issue>) -> Vec<Issue> {
        let mut shown = Vec::new();
        let problems = all.iter().filter(|i| !i.info).count();
        for issue in all {
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
        // One evaluation of the checks for both pages.
        let all = self.issues();
        let devices: Vec<Issue> = all.iter().filter(|i| i.key == "argb").cloned().collect();
        let home = Self::home_issues_of(all);
        self.home_banners
            .sync(home.into_iter().map(|i| (i.key, i)).collect());
        self.device_banners
            .sync(devices.into_iter().map(|i| (i.key, i)).collect());
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Navigate(page) => {
                if page != self.page {
                    // The white test belongs to the settings page.
                    self.set_test_white(false);
                    self.page = page;
                    self.settings_view = SettingsView::Main;
                    self.picker = None;
                    let spec = self.spatial();
                    for (i, s) in self.nav.iter_mut().enumerate() {
                        s.set_spec(spec);
                        s.set_target(if i == page.index() { 1.0 } else { 0.0 });
                    }
                    self.push_preview();
                    // Live values on the displays page need the sensors.
                    self.push_capture();
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
                    "devices-zones" => {
                        // Per-fan lighting on the first device's ARGB header:
                        // three fans, the second red, the third off.
                        let target = self.scene.devices.iter().enumerate().find_map(|(i, d)| {
                            d.info
                                .zones
                                .iter()
                                .position(|z| z.resizable() && z.led_count >= 3)
                                .map(|zi| (i, d.info.key.clone(), zi))
                        });
                        if let Some((index, key, zi)) = target {
                            self.selected_device = Some(index);
                            {
                                let _ = self.update(Message::SetZoneParts(key.clone(), zi, 3));
                                let _ = self.update(Message::SetPartLighting(
                                    key.clone(),
                                    zi,
                                    1,
                                    PartLighting::Color {
                                        color: Rgb::new(255, 40, 40),
                                    },
                                ));
                                let _ = self.update(Message::SetPartLighting(
                                    key,
                                    zi,
                                    2,
                                    PartLighting::Off,
                                ));
                            }
                        }
                        self.canvas_cache.clear();
                    }
                    "devices-fans" => {
                        // The fans of that header are canvas items: the
                        // third one moved on its own and turned a quarter,
                        // the second selected.
                        if let Some(i) = self.selected_device
                            && self.scene.devices[i].segments.len() >= 3
                        {
                            let _ = self.update(Message::CanvasSelect(Some(Target::segment(i, 2))));
                            let _ = self.update(Message::CanvasMove(
                                Target::segment(i, 2),
                                Placement::new(0.40, 0.06, 0.07, 0.125),
                            ));
                            let _ = self.update(Message::CanvasDragEnd);
                            // Any angle (3.1): a fan mounted at a slant,
                            // selected (its rotation knob shows).
                            let _ = self.update(Message::SetRotation(30.0));
                        }
                        self.canvas_cache.clear();
                    }
                    "devices-own" => {
                        // The first fan of that header plays its own
                        // effect (a comet) while the others follow the
                        // global one; its settings are open.
                        if let Some(i) = self.selected_device
                            && let Some(s) = self.scene.devices[i].segments.first()
                        {
                            let key = self.scene.devices[i].info.key.clone();
                            let zone = s.zone;
                            let _ = self.update(Message::SetOwnEffect(
                                key.clone(),
                                zone,
                                0,
                                EffectKind::Comet,
                            ));
                            let _ = self.update(Message::CanvasSelect(Some(Target::segment(i, 0))));
                            self.own_editor = Some((key, zone, 0));
                        }
                        self.canvas_cache.clear();
                    }
                    "devices-hardware" => {
                        // A keyboard left to its own effect.
                        if let Some((i, key, mode)) =
                            self.scene.devices.iter().enumerate().find_map(|(i, d)| {
                                (d.info.kind == tuxrgb_core::DeviceKind::Keyboard)
                                    .then(|| d.info.hardware_modes.get(1).cloned())
                                    .flatten()
                                    .map(|m| (i, d.info.key.clone(), m))
                            })
                        {
                            let _ = self.update(Message::SetHardwareMode(key, Some(mode)));
                            let _ = self.update(Message::CanvasSelect(Some(Target::device(i))));
                        }
                        self.canvas_cache.clear();
                    }
                    "displays" => {
                        // The demo displays: the AIO screen with two values,
                        // the digital display alternating.
                        let _ = self.update(Message::Navigate(Page::Displays));
                    }
                    "fans" => {
                        // The virtual fans: the first on a curve, the
                        // second at a fixed speed, the others left alone.
                        let _ = self.update(Message::Navigate(Page::Fans));
                        let _ = self.update(Message::FanMode(
                            "demo-it8689-fan1".into(),
                            crate::fans::FanMode::Curve,
                        ));
                        let _ = self.update(Message::FanMode(
                            "demo-it8689-fan2".into(),
                            crate::fans::FanMode::Fixed,
                        ));
                        let _ = self.update(Message::FanFixed("demo-it8689-fan2".into(), 40));
                    }
                    "settings" => {
                        let _ = self.update(Message::Navigate(Page::Settings));
                    }
                    "settings-led" => {
                        // The LED colour correction, right under Apparence.
                        return Task::batch([
                            iced::widget::operation::snap_to(
                                ui::settings::SCROLL_ID,
                                iced::widget::scrollable::RelativeOffset { x: 0.0, y: 0.16 },
                            ),
                            Task::perform(
                                tokio::time::sleep(Duration::from_millis(1800)),
                                move |_| Message::TourShoot(i),
                            ),
                        ]);
                    }
                    "settings-network" => {
                        // A WLED strip and a WiZ bulb added, another bulb
                        // found by a search.
                        if self.config.network.is_empty() {
                            self.config.network.wled.push(tuxrgb_openrgb::WledLight {
                                name: "Étagère".into(),
                                host: "192.168.1.40".into(),
                                port: tuxrgb_openrgb::network::DDP_PORT,
                                leds: 144,
                            });
                            self.config.network.wiz.push(tuxrgb_openrgb::WizLight {
                                host: "192.168.1.41".into(),
                                warm_white: false,
                            });
                        }
                        self.net.wled_host = "192.168.1.42".into();
                        self.net.wled_leds = "60".into();
                        self.net.wled_status = Some(Ok(trf!("WLED « {} » · {} LED", "Bureau", 60)));
                        self.net.wiz_found = vec![tuxrgb_openrgb::network::WizFound {
                            host: "192.168.1.43".into(),
                            mac: "a8bb50e5f1c2".into(),
                        }];
                        self.net.wiz_searched = true;
                        return Task::batch([
                            iced::widget::operation::snap_to(
                                ui::settings::SCROLL_ID,
                                iced::widget::scrollable::RelativeOffset { x: 0.0, y: 0.61 },
                            ),
                            Task::perform(
                                tokio::time::sleep(Duration::from_millis(1800)),
                                move |_| Message::TourShoot(i),
                            ),
                        ]);
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
                    self.set_test_white(false);
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
                self.part_picker = None;
                self.own_picker = None;
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
                self.part_picker = None;
                self.own_picker = None;
                Task::none()
            }
            Message::PickerChanged(color) => {
                if let Some((device, zone, part, key)) = self.own_picker.clone() {
                    self.hex_input = color.to_hex();
                    return self.update(Message::SetOwnParam(
                        device,
                        zone,
                        part,
                        key,
                        ParamValue::Color(color),
                    ));
                }
                if let Some((device, zone, part)) = self.part_picker.clone() {
                    self.hex_input = color.to_hex();
                    return self.update(Message::SetPartLighting(
                        device,
                        zone,
                        part,
                        PartLighting::Color { color },
                    ));
                }
                if let Some((kind, key)) = self.picker {
                    self.hex_input = color.to_hex();
                    return self.update(Message::SetParam(kind, key, ParamValue::Color(color)));
                }
                Task::none()
            }
            Message::HexInput(s) => {
                self.hex_input = s.clone();
                if let (Some((device, zone, part, key)), Some(color)) =
                    (self.own_picker.clone(), Rgb::from_hex(&s))
                {
                    return self.update(Message::SetOwnParam(
                        device,
                        zone,
                        part,
                        key,
                        ParamValue::Color(color),
                    ));
                }
                if let (Some((device, zone, part)), Some(color)) =
                    (self.part_picker.clone(), Rgb::from_hex(&s))
                {
                    return self.update(Message::SetPartLighting(
                        device,
                        zone,
                        part,
                        PartLighting::Color { color },
                    ));
                }
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
                self.push_output();
                self.push_tray();
                self.mark_dirty();
                Task::none()
            }

            // ---------------- Devices ----------------
            Message::CanvasSelect(t) => {
                let device = t.map(|t| t.device);
                let segment = t.and_then(|t| t.segment);
                if self.selected_device != device || self.selected_segment != segment {
                    // The settings of another device's own effect close.
                    let picked = device
                        .and_then(|i| self.scene.devices.get(i))
                        .map(|d| d.info.key.clone());
                    if self
                        .own_editor
                        .as_ref()
                        .is_some_and(|(k, _, _)| Some(k) != picked.as_ref())
                    {
                        self.own_editor = None;
                        self.own_picker = None;
                    }
                    self.selected_device = device;
                    self.selected_segment = segment;
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            Message::CanvasMove(t, placement) => {
                self.scene.set_target_placement(t, placement);
                self.canvas_cache.clear();
                Task::none()
            }
            Message::CanvasDragEnd => {
                for d in &self.scene.devices {
                    self.config.layout.insert(d.info.key.clone(), d.placement);
                    // Fans moved on their own keep their place; the others
                    // stay with their device.
                    for s in d.segments.iter().filter(|s| s.pinned) {
                        self.config.layout.insert(s.key.clone(), s.placement);
                    }
                }
                self.push_scene();
                self.mark_dirty();
                Task::none()
            }
            Message::ResizeSelected(scale) => {
                let Some(t) = self.selected_target() else {
                    return Task::none();
                };
                let Some(kind) = self.scene.devices.get(t.device).map(|d| d.info.kind) else {
                    return Task::none();
                };
                // A fan's reference size: a cooler's ring.
                let base = if t.segment.is_some() {
                    tuxrgb_core::DeviceKind::Cooler.default_size()
                } else {
                    kind.default_size()
                };
                self.place_selected(|current| {
                    // The rectangle before the turn: the kind's own shape.
                    let (w, h) = base;
                    let (cx, cy) = current.center();
                    let (w, h) = ((w * scale).clamp(0.02, 1.0), (h * scale).clamp(0.02, 1.0));
                    Placement {
                        x: cx - w * 0.5,
                        y: cy - h * 0.5,
                        w,
                        h,
                        ..current
                    }
                });
                Task::none()
            }
            Message::SetRotation(degrees) => {
                // Any angle; the right angles are magnetic (a few degrees)
                // so the usual orientations are easy to hit with the slider.
                let snapped = tuxrgb_core::layout::snap_angle(degrees);
                self.place_selected(|current| current.with_rotation(snapped));
                Task::none()
            }
            Message::MirrorSelected => {
                self.place_selected(|current| Placement {
                    mirror: !current.mirror,
                    ..current
                });
                Task::none()
            }
            Message::ToggleHidden(key) => {
                if !self.config.hidden_on_canvas.remove(&key) {
                    self.config.hidden_on_canvas.insert(key.clone());
                }
                // Showing a device shows its fans too.
                if !self.config.hidden_on_canvas.contains(&key) {
                    self.config
                        .hidden_on_canvas
                        .retain(|k| !tuxrgb_core::layout::is_segment_of(k, &key));
                }
                self.canvas_cache.clear();
                self.mark_dirty();
                Task::none()
            }
            Message::NetWledHost(h) => {
                self.net.wled_host = h;
                self.net.wled_status = None;
                Task::none()
            }
            Message::NetWledLeds(n) => {
                self.net.wled_leds = n.chars().filter(char::is_ascii_digit).take(5).collect();
                Task::none()
            }
            Message::NetWledProbe => {
                let host = self.net.wled_host.trim().to_owned();
                if !tuxrgb_openrgb::network::valid_host(&host) {
                    self.net.wled_status = Some(Err(crate::i18n::t("Adresse invalide").to_owned()));
                    return Task::none();
                }
                self.net.wled_probing = true;
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            tuxrgb_openrgb::network::wled_info(&host, Duration::from_secs(3))
                        })
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()))
                    },
                    Message::NetWledProbed,
                )
            }
            Message::NetWledProbed(result) => {
                self.net.wled_probing = false;
                self.net.wled_status = Some(match result {
                    Ok(info) => {
                        self.net.wled_leds = info.leds.to_string();
                        self.net.wled_name = info.name.clone();
                        Ok(trf!("WLED « {} » · {} LED", info.name, info.leds))
                    }
                    Err(e) => Err(trf!("WLED introuvable : {}", crate::i18n::t(&e))),
                });
                Task::none()
            }
            Message::NetAddWled => {
                let host = self.net.wled_host.trim().to_owned();
                let leds = self.net.wled_leds.parse::<usize>().unwrap_or(0).min(4096);
                if !tuxrgb_openrgb::network::valid_host(&host) || leds == 0 {
                    self.net.wled_status = Some(Err(crate::i18n::t(
                        "Indiquez l'adresse du contrôleur et son nombre de LED (bouton Détecter).",
                    )
                    .to_owned()));
                    return Task::none();
                }
                let name = if self.net.wled_name.trim().is_empty() {
                    format!("WLED {host}")
                } else {
                    self.net.wled_name.trim().to_owned()
                };
                self.config.network.wled.retain(|l| l.host.trim() != host);
                self.config.network.wled.push(tuxrgb_openrgb::WledLight {
                    name: name.clone(),
                    host,
                    port: tuxrgb_openrgb::network::DDP_PORT,
                    leds,
                });
                self.net = NetForm {
                    wiz_found: std::mem::take(&mut self.net.wiz_found),
                    wiz_searched: self.net.wiz_searched,
                    ..Default::default()
                };
                self.push_network();
                self.snackbar(trf!("« {} » ajouté : le moteur redémarre…", name))
            }
            Message::NetRemoveWled(i) => {
                if i < self.config.network.wled.len() {
                    self.config.network.wled.remove(i);
                    self.push_network();
                }
                Task::none()
            }
            Message::NetWizHost(h) => {
                self.net.wiz_host = h;
                Task::none()
            }
            Message::NetAddWiz(host) => {
                let host = host.trim().to_owned();
                if !tuxrgb_openrgb::network::valid_host(&host) {
                    return self.snackbar("Adresse invalide");
                }
                if !self
                    .config
                    .network
                    .wiz
                    .iter()
                    .any(|l| l.host.trim() == host)
                {
                    self.config.network.wiz.push(tuxrgb_openrgb::WizLight {
                        host: host.clone(),
                        warm_white: false,
                    });
                    self.push_network();
                }
                self.net.wiz_host.clear();
                self.net.wiz_found.retain(|f| f.host != host);
                self.snackbar(trf!("Ampoule WiZ {} ajoutée : le moteur redémarre…", host))
            }
            Message::NetRemoveWiz(i) => {
                if i < self.config.network.wiz.len() {
                    self.config.network.wiz.remove(i);
                    self.push_network();
                }
                Task::none()
            }
            Message::NetWizSearch => {
                if self.net.wiz_searching {
                    return Task::none();
                }
                self.net.wiz_searching = true;
                Task::perform(
                    async {
                        tokio::task::spawn_blocking(|| {
                            tuxrgb_openrgb::network::discover_wiz(
                                &tuxrgb_openrgb::network::wiz_broadcast(),
                                Duration::from_millis(1500),
                            )
                        })
                        .await
                        .unwrap_or_default()
                    },
                    Message::NetWizFound,
                )
            }
            Message::NetWizFound(found) => {
                self.net.wiz_searching = false;
                self.net.wiz_searched = true;
                self.net.wiz_found = found
                    .into_iter()
                    .filter(|f| !self.config.network.wiz.iter().any(|l| l.host == f.host))
                    .collect();
                Task::none()
            }
            Message::SetHardwareMode(key, mode) => {
                match mode {
                    Some(m) => {
                        self.config.hardware_modes.insert(key.clone(), m);
                    }
                    None => {
                        self.config.hardware_modes.remove(&key);
                    }
                }
                if let Some(s) = &self.services {
                    s.bridge(BridgeCommand::SetHardwareModes(
                        self.config.hardware_modes.clone(),
                    ));
                }
                // Real devices: the bridge reports the mode once applied.
                if self.using_demo || self.devices.is_empty() {
                    self.rebuild_scene();
                }
                self.mark_dirty();
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
            Message::SetPartLighting(device, zone, part, value) => {
                // The "whole device" row (its picker included).
                if zone == usize::MAX {
                    return self.update(Message::SetDeviceLighting(device, value));
                }
                let entry = self.config.lighting.entry(device.clone()).or_default();
                let z = entry.entry(zone).or_default();
                z.set_part(part, value);
                self.store_lighting(&device);
                Task::none()
            }
            Message::SetZoneParts(device, zone, parts) => {
                let leds = self
                    .device_info(&device)
                    .and_then(|d| d.zones.get(zone).map(|z| z.led_count))
                    .unwrap_or(1)
                    .max(1);
                let parts = parts.clamp(1, tuxrgb_core::lighting::MAX_PARTS.min(leds));
                let entry = self.config.lighting.entry(device.clone()).or_default();
                let z = entry.entry(zone).or_default();
                z.parts = parts;
                z.sizes.clear();
                z.lighting.truncate(parts);
                // Places of fans that no longer exist.
                for gone in parts..tuxrgb_core::lighting::MAX_PARTS {
                    let key = tuxrgb_core::layout::segment_key(&device, zone, gone);
                    self.config.layout.remove(&key);
                    self.config.hidden_on_canvas.remove(&key);
                }
                if self.selected_segment.is_some() {
                    self.selected_segment = None;
                }
                if let Some((d, zi, p)) = &self.part_picker
                    && *d == device
                    && *zi == zone
                    && *p >= parts
                {
                    self.part_picker = None;
                }
                self.store_lighting(&device);
                Task::none()
            }
            Message::SetPartSize(device, zone, part, leds) => {
                let Some(info) = self.device_info(&device) else {
                    return Task::none();
                };
                let Some(z) = info.zones.get(zone) else {
                    return Task::none();
                };
                let current = self.config.lighting.get(&device).and_then(|l| l.get(&zone));
                let sizes = tuxrgb_core::lighting::part_sizes(z, current);
                if let Some(sizes) = tuxrgb_core::lighting::resize_part(&sizes, part, leds) {
                    let entry = self.config.lighting.entry(device.clone()).or_default();
                    let zl = entry.entry(zone).or_default();
                    zl.parts = sizes.len();
                    zl.sizes = sizes;
                    self.store_lighting(&device);
                }
                Task::none()
            }
            Message::SetDeviceLighting(device, value) => {
                let Some(info) = self.device_info(&device) else {
                    return Task::none();
                };
                let entry = self.config.lighting.entry(device.clone()).or_default();
                for (zi, zone) in info.zones.iter().enumerate() {
                    let z = entry.entry(zi).or_default();
                    for part in 0..tuxrgb_core::lighting::parts_of(zone, Some(z)) {
                        z.set_part(part, value.clone());
                    }
                }
                self.store_lighting(&device);
                Task::none()
            }
            Message::IdentifyPart(device, zone, part) => {
                if let Some(info) = self.device_info(&device) {
                    let leds = tuxrgb_core::lighting::part_leds(
                        &info,
                        self.config.lighting.get(&device),
                        zone,
                        part,
                    );
                    if let Some(s) = &self.services
                        && !leds.is_empty()
                    {
                        s.engine(EngineCommand::IdentifyLeds {
                            device: info.id,
                            leds,
                        });
                    }
                }
                Task::none()
            }
            Message::SetOwnEffect(device, zone, part, kind) => {
                // Settings already tried on this part are kept; an effect
                // new to it starts from the global settings of that effect.
                let mut params = match self.part_value(&device, zone, part) {
                    Some(PartLighting::Own { params, .. }) => params,
                    _ => tuxrgb_core::EffectParams::default(),
                };
                if !params.0.contains_key(&kind)
                    && let Some(global) = self.config.params.0.get(&kind)
                {
                    params.0.insert(kind, global.clone());
                }
                self.part_picker = None;
                self.own_picker = None;
                self.own_editor = Some((device.clone(), zone, part));
                self.update(Message::SetPartLighting(
                    device,
                    zone,
                    part,
                    PartLighting::Own {
                        effect: kind,
                        params,
                    },
                ))
            }
            Message::SetOwnParam(device, zone, part, key, value) => {
                let Some(PartLighting::Own { effect, mut params }) =
                    self.part_value(&device, zone, part)
                else {
                    return Task::none();
                };
                if params.value(effect, key) == value {
                    return Task::none();
                }
                params.set(effect, key, value);
                self.update(Message::SetPartLighting(
                    device,
                    zone,
                    part,
                    PartLighting::Own { effect, params },
                ))
            }
            Message::DisplayEnabled(key, on) => {
                self.edit_display(&key, |s| s.enabled = on);
                Task::none()
            }
            Message::DisplayContent(key, content) => {
                self.edit_display(&key, |s| s.content = content);
                Task::none()
            }
            Message::DisplayMetric(key, metric) => {
                self.edit_display(&key, |s| {
                    if let Some(i) = s.metrics.iter().position(|m| *m == metric) {
                        s.metrics.remove(i);
                    } else {
                        s.metrics.push(metric);
                    }
                });
                Task::none()
            }
            Message::DisplayText(key, text) => {
                let text: String = text.chars().take(80).collect();
                self.edit_display(&key, |s| s.text = text);
                Task::none()
            }
            Message::DisplayImagePath(key, path) => {
                let path = path.trim().to_owned();
                self.edit_display(&key, |s| {
                    s.image = (!path.is_empty()).then(|| std::path::PathBuf::from(path));
                });
                Task::none()
            }
            Message::DisplayPickImage(key) => {
                Task::perform(crate::desktop::pick_image(), move |p| {
                    Message::DisplayImagePicked(key.clone(), p)
                })
            }
            Message::DisplayImagePicked(key, path) => {
                if let Some(path) = path {
                    self.edit_display(&key, |s| {
                        s.image = Some(path);
                        s.content = crate::displays::Content::Image;
                    });
                }
                Task::none()
            }
            Message::DisplayInterval(key, seconds) => {
                let ms = (seconds.clamp(0.25, 10.0) * 1000.0).round() as u32;
                self.edit_display(&key, |s| s.interval_ms = ms);
                Task::none()
            }
            Message::DisplayCycle(key, seconds) => {
                let secs = seconds.round().clamp(2.0, 30.0) as u32;
                self.edit_display(&key, |s| s.cycle_s = secs);
                Task::none()
            }
            Message::DisplayBrightness(key, b) => {
                let b = (b.clamp(0.0, 1.0) * 100.0).round() as u8;
                self.edit_display(&key, |s| s.brightness = b);
                Task::none()
            }
            Message::DisplayRotation(key, r) => {
                self.edit_display(&key, |s| s.rotation = r % 360);
                Task::none()
            }
            Message::DisplayAlarm(key, on) => {
                self.edit_display(&key, |s| s.alarm = on);
                Task::none()
            }
            Message::DisplaysFahrenheit(on) => {
                self.config.displays.fahrenheit = on;
                self.push_displays();
                self.mark_dirty();
                Task::none()
            }
            Message::DisplaysRescan => {
                if let Some(s) = &self.services {
                    s.send(ServiceCommand::Displays(crate::displays::Command::Rescan));
                }
                Task::none()
            }
            Message::FanMode(id, mode) => {
                self.edit_fan(&id, |p| p.mode = mode);
                Task::none()
            }
            Message::FanFixed(id, pct) => {
                self.edit_fan(&id, |p| p.fixed = pct.min(100));
                Task::none()
            }
            Message::FanSource(id, source) => {
                if crate::fans::curve::valid_id(&source) {
                    self.edit_fan(&id, |p| p.source = source);
                }
                Task::none()
            }
            Message::FanPreset(id, preset) => {
                self.edit_fan(&id, |p| p.points = crate::fans::curve::preset(preset));
                Task::none()
            }
            Message::FanPoint(id, i, (t, d)) => {
                self.edit_fan(&id, |p| {
                    // Each point stays between its neighbours (1 °C apart).
                    let lo = if i == 0 {
                        0
                    } else {
                        p.points.get(i - 1).map_or(0, |q| q.0 + 1)
                    };
                    let hi = p
                        .points
                        .get(i + 1)
                        .map_or(crate::fans::curve::TEMP_MAX, |q| q.0.saturating_sub(1));
                    if let Some(pt) = p.points.get_mut(i) {
                        *pt = (t.clamp(lo, hi.max(lo)), d.min(100));
                    }
                });
                Task::none()
            }
            Message::FanRename(Some(id)) => {
                let current = self
                    .config
                    .fans
                    .fans
                    .get(&id)
                    .and_then(|p| p.name.clone())
                    .or_else(|| {
                        self.fans
                            .fans
                            .iter()
                            .find(|f| f.id == id)
                            .map(|f| f.default_name())
                    })
                    .unwrap_or_default();
                self.fan_rename = Some((id, current));
                Task::none()
            }
            Message::FanRename(None) => {
                if let Some((id, name)) = self.fan_rename.take() {
                    let name = name.trim().chars().take(40).collect::<String>();
                    self.edit_fan(&id, |p| p.name = (!name.is_empty()).then_some(name));
                }
                Task::none()
            }
            Message::FanRenameInput(text) => {
                if let Some((_, name)) = &mut self.fan_rename {
                    *name = text;
                }
                Task::none()
            }
            Message::FanIdentify(id) => {
                if let Some(s) = &self.services {
                    s.send(ServiceCommand::Fans(crate::fans::Command::Identify(id)));
                }
                Task::none()
            }
            Message::FansRescan => {
                if let Some(s) = &self.services {
                    s.send(ServiceCommand::Fans(crate::fans::Command::Rescan));
                }
                Task::none()
            }
            Message::FansEnable => {
                if cfg!(windows) {
                    return self.update(Message::StartHardwareSetup);
                }
                if self.helper_busy {
                    return Task::none();
                }
                self.helper_busy = true;
                Task::perform(
                    privileges::runner::run_helper("enable-fans", None),
                    Message::FansEnableDone,
                )
            }
            Message::FansEnableDone(result) => {
                self.helper_busy = false;
                let _ = self.update(Message::FansRescan);
                match result {
                    Ok(report) if report.steps.iter().any(|s| s == "fan-driver-none") => self
                        .snackbar(
                            "Service activé, mais aucun ventilateur pilotable : le capteur de la carte mère n'a pas de pilote ici",
                        ),
                    Ok(_) => self.snackbar("Contrôle des ventilateurs activé"),
                    Err(HelperError::Cancelled) => self.snackbar("Authentification annulée"),
                    Err(e) => self.snackbar(trf!("Échec : {}", e)),
                }
            }
            Message::FollowGlobalEverywhere => {
                let devices: Vec<DeviceKey> = self.config.lighting.keys().cloned().collect();
                for l in self.config.lighting.values_mut() {
                    for z in l.values_mut() {
                        z.lighting.clear();
                    }
                }
                self.own_editor = None;
                self.own_picker = None;
                self.part_picker = None;
                for d in devices {
                    self.store_lighting(&d);
                }
                self.snackbar("Tout le setup suit l'effet global")
            }
            Message::OpenOwnEditor(device, zone, part) => {
                let target = Some((device, zone, part));
                self.own_picker = None;
                self.part_picker = None;
                self.own_editor = if self.own_editor == target {
                    None
                } else {
                    target
                };
                Task::none()
            }
            Message::OpenOwnPicker(device, zone, part, key) => {
                let target = Some((device.clone(), zone, part, key));
                self.picker = None;
                self.part_picker = None;
                if self.own_picker == target {
                    self.own_picker = None;
                } else {
                    if let Some(PartLighting::Own { effect, params }) =
                        self.part_value(&device, zone, part)
                    {
                        self.hex_input = params.color(effect, key).to_hex();
                    }
                    self.own_picker = target;
                }
                Task::none()
            }
            Message::OwnPickerHidden => {
                self.own_picker_last = None;
                Task::none()
            }
            Message::OpenPartPicker(device, zone, part) => {
                let target = Some((device.clone(), zone, part));
                self.picker = None;
                self.own_picker = None;
                if self.part_picker == target {
                    self.part_picker = None;
                } else {
                    let current = self.part_value(&device, zone, part);
                    let color = match current {
                        Some(PartLighting::Color { color }) => color,
                        _ => self.desktop_accent(),
                    };
                    self.hex_input = color.to_hex();
                    self.part_picker = target;
                    if !matches!(current, Some(PartLighting::Color { .. })) {
                        return self.update(Message::SetPartLighting(
                            device,
                            zone,
                            part,
                            PartLighting::Color { color },
                        ));
                    }
                }
                Task::none()
            }
            Message::PartPickerHidden => {
                self.part_picker_last = None;
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
            Message::LedColorEnabled(on) => {
                self.config.led_color.enabled = on;
                self.push_led_color();
                Task::none()
            }
            Message::LedGamma(g) => {
                if (g - self.config.led_color.gamma).abs() < 1e-3 {
                    return Task::none();
                }
                self.config.led_color.gamma = g;
                self.push_led_color();
                Task::none()
            }
            Message::LedWhite(ch, v) => {
                if let Some(w) = self.config.led_color.white.get_mut(ch) {
                    if (v - *w).abs() < 1e-3 {
                        return Task::none();
                    }
                    *w = v;
                    self.push_led_color();
                }
                Task::none()
            }
            Message::LedColorReset => {
                self.config.led_color = tuxrgb_core::led_color::LedCalibration::default();
                self.push_led_color();
                Task::none()
            }
            Message::LedTestWhite(on) => {
                self.set_test_white(on);
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
            Message::SetLedsOffWithScreen(on) => {
                self.config.behavior.leds_off_with_screen = on;
                self.push_output();
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
                // OpenRGB's rules plus TuxRGB's own for the hardware
                // displays (AIO screens, digital displays).
                let rules = format!(
                    "{}{}",
                    rules.trim_end(),
                    crate::displays::hardware_display::udev_rules()
                );
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
                self.push_output();
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
                handle.engine(EngineCommand::SetLedGamma(
                    self.config.led_color.effective_gamma(),
                ));
                handle.engine(EngineCommand::SetFps(self.config.fps));
                handle.send(ServiceCommand::Fans(crate::fans::Command::Configure(
                    self.config.fans.clone(),
                )));
                handle.engine(EngineCommand::SetAccent(self.desktop_accent()));
                handle.engine(EngineCommand::SetOutputEnabled {
                    on: self.effective_output(),
                    fade: true,
                });
                self.services = Some(handle);
                self.push_preview();
                self.push_scene();
                self.push_suspended();
                self.push_capture();
                self.push_displays();
                Task::none()
            }
            ServiceEvent::Fans(state) => {
                self.fans = state;
                Task::none()
            }
            ServiceEvent::ScreensOff(off) => {
                if off != self.screen_off {
                    self.screen_off = off;
                    if self.config.behavior.leds_off_with_screen {
                        tracing::info!(off, "LEDs follow the screens");
                    }
                    self.push_output();
                }
                Task::none()
            }
            ServiceEvent::Displays(states) => {
                // Previews: an image per new picture only (the same Arc is
                // the same picture).
                for d in &states {
                    if let Some(p) = &d.preview {
                        let id = std::sync::Arc::as_ptr(p) as usize;
                        if self.display_previews.get(&d.key).map(|(i, _)| *i) != Some(id) {
                            let handle = iced::widget::image::Handle::from_rgba(
                                p.width,
                                p.height,
                                p.rgba.clone(),
                            );
                            self.display_previews.insert(d.key.clone(), (id, handle));
                        }
                    }
                }
                self.display_previews
                    .retain(|k, _| states.iter().any(|d| d.key == *k && d.preview.is_some()));
                let before = self.displays_sensors();
                self.displays = states;
                if self.displays_sensors() != before {
                    self.push_capture();
                }
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
