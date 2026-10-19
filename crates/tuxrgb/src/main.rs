//! TuxRGB – RGB lighting control for Linux.
//!
//! 100 % Rust application (iced 0.14, native Wayland), Material 3 Expressive
//! interface themed from the desktop accent colour (KDE, GNOME, COSMIC), in
//! French or English, with the OpenRGB hardware engine embedded as a
//! private SDK server.

#[macro_use]
mod i18n;
mod app;
mod autostart;
mod config;
mod credits;
mod desktop;
mod instance;
mod integration;
mod paths;
mod privileges;
mod services;
mod theme;
mod tour;
mod tray;
mod ui;

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
}

static ARGS: OnceLock<Args> = OnceLock::new();

pub fn args() -> &'static Args {
    ARGS.get_or_init(Args::default)
}

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

fn parse_args() -> Option<Args> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--screenshot-tour" => a.screenshot_tour = it.next().map(Into::into),
            "--minimized" | "--tray" => a.minimized = true,
            "--reset-hardware" | "--reset-permissions" => a.reset_access = true,
            "--software-render" => a.software_render = true,
            "--version" | "-V" => {
                println!("TuxRGB {}", env!("CARGO_PKG_VERSION"));
                return None;
            }
            "-h" | "--help" => {
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
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            std::env::remove_var("DISPLAY");
        }
        std::env::set_var("WINIT_UNIX_BACKEND", "wayland");
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
    let Some(args) = parse_args() else {
        return Ok(());
    };
    configure_environment(&args);
    let _ = ARGS.set(args);

    // Single instance: a second launch brings the running TuxRGB forward
    // (decided before logging so it never truncates the running one's log).
    if crate::args().screenshot_tour.is_none() {
        let request = instance::ActivateRequest {
            reset_hardware: crate::args().reset_access,
            minimized: crate::args().minimized,
        };
        if let instance::Role::Secondary = instance::claim(request) {
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
    }

    init_logging();
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

    iced::daemon(app::App::boot, app::App::update, app::App::view)
        .title(app::App::title)
        .theme(app::App::theme)
        .subscription(app::App::subscription)
        .font(include_bytes!("../assets/fonts/TuxRGBSymbols.ttf").as_slice())
        .font(include_bytes!("../assets/fonts/TuxRGBSymbolsFill.ttf").as_slice())
        .antialiasing(true)
        .run()
}
