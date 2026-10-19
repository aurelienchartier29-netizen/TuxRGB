//! TuxRGB – RGB lighting control for Linux.
//!
//! 100 % Rust application (iced 0.14, native Wayland), Material 3 Expressive
//! interface themed from the desktop accent colour (KDE, GNOME, COSMIC), in
//! French or English, with the OpenRGB hardware engine embedded as a
//! private SDK server.
//!
//! Linux is the reference platform. The Windows beta build compiles the
//! same interface and engine, with the system services of `win/` (WASAPI,
//! Win32 tray, power events, registry accent) in place of the Linux ones.

// Windows: a GUI program, no console window.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[macro_use]
mod i18n;
mod app;
#[cfg_attr(windows, path = "win/autostart.rs")]
mod autostart;
mod config;
mod credits;
mod desktop;
#[cfg_attr(windows, path = "win/instance.rs")]
mod instance;
#[cfg_attr(windows, path = "win/integration.rs")]
mod integration;
mod paths;
mod privileges;
mod services;
mod theme;
mod tour;
#[cfg_attr(windows, path = "win/tray.rs")]
mod tray;
mod ui;
#[cfg(windows)]
mod win;

use std::sync::OnceLock;

#[derive(Debug, Default, Clone)]
pub struct Args {
    /// Start in the tray without opening a window (autostart).
    pub minimized: bool,
    /// Reset hardware access state and show the setup wizard.
    pub reset_access: bool,
    pub software_render: bool,
    /// Developer tool: save a PNG of every page into this directory, then exit.
    pub screenshot_tour: Option<std::path::PathBuf>,
    /// Windows: started by the administrator-mode scheduled task.
    #[cfg(windows)]
    pub elevated_handoff: bool,
    /// Windows: never hand over to administrator mode (troubleshooting).
    #[cfg(windows)]
    pub no_admin: bool,
}

static ARGS: OnceLock<Args> = OnceLock::new();

pub fn args() -> &'static Args {
    ARGS.get_or_init(Args::default)
}

#[cfg(not(windows))]
const HELP: &str = "TuxRGB – éclairage RGB pour Linux (Wayland)

Utilisation : tuxrgb [OPTIONS]

  --minimized          Démarrer dans la zone de notification, sans fenêtre
  --reset-hardware     Relancer l'assistant d'accès matériel au démarrage
  --software-render    Forcer le rendu logiciel (tiny-skia)
  --screenshot-tour D  (développeurs) Capturer chaque page en PNG dans D puis quitter
  --version            Afficher la version
  -h, --help           Afficher cette aide

Variables utiles :
  TUXRGB_OPENRGB=/chemin/openrgb   Utiliser un autre binaire OpenRGB
  RUST_LOG=tuxrgb=debug            Journalisation détaillée
";

#[cfg(not(windows))]
const HELP_EN: &str = "TuxRGB – RGB lighting for Linux (Wayland)

Usage: tuxrgb [OPTIONS]

  --minimized          Start in the system tray, without a window
  --reset-hardware     Run the hardware access wizard again at start-up
  --software-render    Force software rendering (tiny-skia)
  --screenshot-tour D  (developers) Save every page as PNG into D, then quit
  --version            Print the version
  -h, --help           Print this help

Useful variables:
  TUXRGB_OPENRGB=/path/openrgb     Use another OpenRGB binary
  RUST_LOG=tuxrgb=debug            Verbose logging
";

#[cfg(windows)]
const HELP: &str = "TuxRGB – éclairage RGB (Windows Beta Build)

Utilisation : tuxrgb.exe [OPTIONS]

  --minimized          Démarrer dans la zone de notification, sans fenêtre
  --reset-hardware     Relancer l'assistant d'accès matériel au démarrage
  --software-render    Forcer le rendu logiciel (tiny-skia)
  --no-admin           Ne pas passer en mode administrateur
  --screenshot-tour D  (développeurs) Capturer chaque page en PNG dans D puis quitter
  --version            Afficher la version
  -h, --help           Afficher cette aide

Variables utiles :
  TUXRGB_OPENRGB=C:\\chemin\\OpenRGB.exe   Utiliser un autre OpenRGB
  RUST_LOG=tuxrgb=debug                    Journalisation détaillée
";

#[cfg(windows)]
const HELP_EN: &str = "TuxRGB – RGB lighting (Windows Beta Build)

Usage: tuxrgb.exe [OPTIONS]

  --minimized          Start in the system tray, without a window
  --reset-hardware     Run the hardware access wizard again at start-up
  --software-render    Force software rendering (tiny-skia)
  --no-admin           Never switch to administrator mode
  --screenshot-tour D  (developers) Save every page as PNG into D, then quit
  --version            Print the version
  -h, --help           Print this help

Useful variables:
  TUXRGB_OPENRGB=C:\\path\\OpenRGB.exe     Use another OpenRGB
  RUST_LOG=tuxrgb=debug                    Verbose logging
";

/// Version shown by `--version`, the window title and « À propos ».
pub fn version_label() -> &'static str {
    #[cfg(windows)]
    return win::VERSION;
    #[cfg(not(windows))]
    return env!("CARGO_PKG_VERSION");
}

fn parse_args() -> Option<Args> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--screenshot-tour" => a.screenshot_tour = it.next().map(Into::into),
            "--minimized" | "--tray" => a.minimized = true,
            "--reset-hardware" | "--reset-permissions" => a.reset_access = true,
            "--software-render" => a.software_render = true,
            #[cfg(windows)]
            "--elevated-handoff" => a.elevated_handoff = true,
            #[cfg(windows)]
            "--no-admin" => a.no_admin = true,
            "--version" | "-V" => {
                #[cfg(windows)]
                win::attach_parent_console();
                #[cfg(windows)]
                println!("TuxRGB {} ({})", version_label(), win::BETA_LABEL);
                #[cfg(not(windows))]
                println!("TuxRGB {}", version_label());
                return None;
            }
            "-h" | "--help" => {
                #[cfg(windows)]
                win::attach_parent_console();
                match tuxrgb_core::i18n::system_language() {
                    tuxrgb_core::i18n::Lang::Fr => print!("{HELP}"),
                    tuxrgb_core::i18n::Lang::En => print!("{HELP_EN}"),
                }
                return None;
            }
            other => eprintln!(
                "{}",
                crate::i18n::trf!("option inconnue ignorée : {}", other)
            ),
        }
    }
    Some(a)
}

/// Must run before any thread exists (environment is process-global).
fn configure_environment(args: &Args) {
    let config = config::Config::load_initial();
    tuxrgb_core::i18n::set_language(config.language.resolve());
    // SAFETY: called at the very start of `main`, single-threaded.
    unsafe {
        // Native Wayland only: never fall back to X11 / XWayland.
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                std::env::remove_var("DISPLAY");
            }
            std::env::set_var("WINIT_UNIX_BACKEND", "wayland");
        }
        // Low-power GPU by default (iced defaults to the discrete GPU).
        if std::env::var_os("WGPU_POWER_PREF").is_none() {
            let pref = match config.renderer {
                config::RendererPref::GpuHighPerformance => "high",
                _ => "low",
            };
            std::env::set_var("WGPU_POWER_PREF", pref);
        }
        if args.software_render || config.renderer == config::RendererPref::Software {
            std::env::set_var("ICED_BACKEND", "tiny-skia");
        } else if paths::paths().gpu_marker.exists() {
            // The previous start died while creating its window with the
            // GPU renderer (driver crash): fall back to software rendering.
            std::env::set_var("ICED_BACKEND", "tiny-skia");
            std::env::set_var("TUXRGB_GPU_FALLBACK", "1");
            let _ = std::fs::remove_file(&paths::paths().gpu_marker);
        }
    }
}

/// Logs to stderr and to `~/.local/state/tuxrgb/tuxrgb.log` (the previous
/// run is kept as `tuxrgb.log.old`), and records panics there: a launch
/// that fails from the application menu leaves a trace.
fn init_logging() {
    use tracing_subscriber::fmt::writer::MakeWriterExt;
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            "warn,tuxrgb=info,tuxrgb_openrgb=info,tuxrgb_capture=info,tuxrgb_core=info".into()
        })
    };
    let log = &paths::paths().app_log;
    let file = log.parent().and_then(|dir| {
        std::fs::create_dir_all(dir).ok()?;
        let _ = std::fs::rename(log, log.with_extension("log.old"));
        std::fs::File::create(log).ok()
    });
    match file {
        Some(f) => tracing_subscriber::fmt()
            .with_env_filter(filter())
            .with_target(false)
            .with_ansi(false)
            .with_writer(std::io::stderr.and(std::sync::Mutex::new(f)))
            .init(),
        None => tracing_subscriber::fmt()
            .with_env_filter(filter())
            .with_target(false)
            .init(),
    }
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        tracing::error!(
            "panic in thread {}: {info}\n{}",
            thread.name().unwrap_or("?"),
            std::backtrace::Backtrace::force_capture()
        );
        default_hook(info);
    }));
}

/// A Wayland compositor socket is reachable.
#[cfg(target_os = "linux")]
fn wayland_available() -> bool {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from);
    match std::env::var_os("WAYLAND_DISPLAY") {
        Some(name) => {
            let p = std::path::PathBuf::from(&name);
            p.is_absolute() || runtime.is_none_or(|r| r.join(&p).exists())
        }
        None => runtime.is_some_and(|r| r.join("wayland-0").exists()),
    }
}

fn main() -> iced::Result {
    // Windows: `tuxrgb.exe --windows-setup <action> [report]`, started with
    // the UAC prompt (or by the installer) – no interface, no instance.
    #[cfg(windows)]
    {
        let argv: Vec<String> = std::env::args().collect();
        if argv.get(1).map(String::as_str) == Some("--windows-setup") {
            let action = argv.get(2).map(String::as_str).unwrap_or("status");
            let report = argv.get(3).map(std::path::Path::new);
            std::process::exit(win::elevation::setup_main(action, report));
        }
    }
    #[cfg_attr(not(windows), allow(unused_mut))]
    let Some(mut args) = parse_args() else {
        return Ok(());
    };
    // Started by the administrator-mode task: what the launch that handed
    // over was asked for.
    #[cfg(windows)]
    if args.elevated_handoff
        && let Some(req) = win::elevation::take_handoff()
    {
        args.minimized |= req.minimized;
        args.reset_access |= req.reset_hardware;
    }
    configure_environment(&args);
    let _ = ARGS.set(args);

    // Single instance: a second launch brings the running TuxRGB forward
    // (decided before logging so it never truncates the running one's log).
    if crate::args().screenshot_tour.is_none() {
        let request = instance::ActivateRequest {
            reset_hardware: crate::args().reset_access,
            minimized: crate::args().minimized,
        };
        #[cfg(windows)]
        let role = instance::claim_with(request.clone(), crate::args().elevated_handoff);
        #[cfg(not(windows))]
        let role = instance::claim(request);
        if let instance::Role::Secondary = role {
            if crate::args().minimized {
                println!("{}", i18n::t("TuxRGB est déjà lancé."));
            } else {
                println!(
                    "{}",
                    i18n::t("TuxRGB est déjà lancé : sa fenêtre est affichée.")
                );
            }
            return Ok(());
        }
        // Windows, administrator mode configured: this normal launch hands
        // over to the elevated instance (scheduled task, no prompt).
        #[cfg(windows)]
        if !crate::args().elevated_handoff
            && !crate::args().no_admin
            && win::elevation::hand_over(&request)
        {
            return Ok(());
        }
    }

    init_logging();
    #[cfg(windows)]
    tracing::info!(
        version = version_label(),
        build = win::BETA_LABEL,
        package = paths::package_kind().label(),
        os = win::os_name(),
        elevated = win::elevation::is_elevated(),
        renderer = std::env::var("ICED_BACKEND").unwrap_or_else(|_| "wgpu".into()),
        language = tuxrgb_core::i18n::language().code(),
        "TuxRGB starting"
    );
    #[cfg(not(windows))]
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        package = paths::package_kind().label(),
        desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
        session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default(),
        renderer = std::env::var("ICED_BACKEND").unwrap_or_else(|_| "wgpu".into()),
        distro = desktop::session().distro.as_str(),
        language = tuxrgb_core::i18n::language().code(),
        "TuxRGB starting"
    );
    #[cfg(target_os = "linux")]
    if !wayland_available() {
        // Native Wayland only: an X11 session cannot open the window.
        let msg = i18n::t(
            "Session X11 non prise en charge : TuxRGB est une application Wayland native. Ouvrez une session Wayland (Plasma, GNOME, COSMIC…) pour utiliser TuxRGB.",
        );
        tracing::error!("no Wayland compositor: {msg}");
        eprintln!("TuxRGB : {msg}");
        let _ = std::process::Command::new("notify-send")
            .args([
                "--app-name=TuxRGB",
                "--icon=io.github.tuxrgb.TuxRGB",
                "TuxRGB",
                msg,
            ])
            .status();
        std::process::exit(1);
    }

    // Windows: the generic font families must name fonts Windows has.
    #[cfg(windows)]
    win::configure_fonts();

    iced::daemon(app::App::boot, app::App::update, app::App::view)
        .title(app::App::title)
        .theme(app::App::theme)
        .subscription(app::App::subscription)
        .font(include_bytes!("../assets/fonts/TuxRGBSymbols.ttf").as_slice())
        .font(include_bytes!("../assets/fonts/TuxRGBSymbolsFill.ttf").as_slice())
        .antialiasing(true)
        .run()
}
