//! Windows: TuxRGB drives the motherboard's fan controller itself, in
//! administrator mode, through PawnIO (see `tuxrgb-capture/src/superio.rs`).
//!
//! Safety: what each fan was set to before is written to a file before the
//! first change, and a small guard process (`tuxrgb.exe --fan-guard`) waits
//! for TuxRGB to end: if it ends without giving the fans back (crash, killed
//! from the task manager), the guard puts them back as they were.

use super::{
    Access, Command, Events, FanInfo, FanKind, FanStatus, FansConfig, FansState, TempInfo, curve,
};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tuxrgb_capture::superio::{self, SuperIo};
use tuxrgb_core::SharedInputs;

const IDENTIFY: Duration = Duration::from_secs(5);
/// Driven fans are written again this often, whatever the firmware did.
const REASSERT: Duration = Duration::from_secs(10);
/// A sensor sample older than this is no reading.
const STALE: Duration = Duration::from_secs(5);

fn restore_path() -> PathBuf {
    crate::paths::paths().app_log.with_file_name("fans.restore")
}

fn fan_id(chip: &SuperIo, i: usize) -> String {
    format!(
        "{}-{:x}-fan{}",
        chip.name.to_ascii_lowercase(),
        chip.port,
        i + 1
    )
}

fn temp_id(chip: &SuperIo, i: usize) -> String {
    format!(
        "{}-{:x}-temp{}",
        chip.name.to_ascii_lowercase(),
        chip.port,
        i + 1
    )
}

struct Driven {
    driver: curve::Driver,
    status: FanStatus,
    written: Option<u8>,
}

struct Controller {
    chips: Vec<SuperIo>,
    /// By (chip, control).
    driven: std::collections::BTreeMap<(usize, usize), Driven>,
    tests: Vec<(String, Instant)>,
    safety: bool,
    guard: Option<std::process::Child>,
    last_tick: Instant,
    /// Last time every driven fan was written again (see [`REASSERT`]).
    last_assert: Instant,
}

impl Controller {
    fn new() -> Self {
        Self {
            chips: superio::detect(),
            driven: Default::default(),
            tests: Vec::new(),
            safety: false,
            guard: None,
            last_tick: Instant::now(),
            last_assert: Instant::now(),
        }
    }

    fn temps(&self, inputs: &SharedInputs) -> Vec<TempInfo> {
        let sample = inputs
            .latest_hardware()
            .filter(|s| s.captured_at.elapsed() < STALE);
        let mut out = vec![
            TempInfo {
                id: "cpu".into(),
                label: crate::i18n::t("Processeur").into(),
                value: sample.as_ref().and_then(|s| s.cpu_temp),
            },
            TempInfo {
                id: "gpu".into(),
                label: crate::i18n::t("Carte graphique").into(),
                value: sample.as_ref().and_then(|s| s.gpu_temp),
            },
        ];
        for chip in &self.chips {
            for (i, v) in chip.temperatures().into_iter().enumerate() {
                out.push(TempInfo {
                    id: temp_id(chip, i),
                    label: crate::i18n::trf!("Carte mère {}", i + 1),
                    value: v,
                });
            }
        }
        out
    }

    /// Restore records of every fan taken over, and the guard watching them.
    fn update_guard(&mut self) {
        let mut text = String::new();
        for chip in &self.chips {
            for (i, s) in chip.saved() {
                text.push_str(&superio::format_saved(chip.port, i, s));
                text.push('\n');
            }
        }
        let path = restore_path();
        if text.is_empty() {
            let _ = std::fs::remove_file(&path);
            return;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            let _ = std::fs::write(&path, &text);
        }
        let running = self
            .guard
            .as_mut()
            .is_some_and(|g| matches!(g.try_wait(), Ok(None)));
        if !running {
            self.guard = spawn_guard(&path);
        }
    }

    fn tick(&mut self, config: &FansConfig, temps: &[TempInfo]) {
        let dt = self.last_tick.elapsed().as_secs_f32().clamp(0.05, 5.0);
        self.last_tick = Instant::now();
        // The firmware may take the fans back (resume from sleep): every
        // driven fan is written again now and then, as on Linux.
        let reassert = self.last_assert.elapsed() >= REASSERT;
        if reassert {
            self.last_assert = Instant::now();
        }
        let value = |id: &str| temps.iter().find(|t| t.id == id).and_then(|t| t.value);
        let hottest = [value("cpu"), value("gpu")]
            .into_iter()
            .flatten()
            .reduce(f32::max);
        self.safety = curve::safety_state(self.safety, hottest);
        self.tests.retain(|(_, until)| *until > Instant::now());
        let mut changed = false;
        for c in 0..self.chips.len() {
            if self.chips[c].read_only {
                continue;
            }
            for i in 0..self.chips[c].controls() {
                let id = fan_id(&self.chips[c], i);
                let mode = config
                    .fans
                    .get(&id)
                    .map(|p| p.mode())
                    .unwrap_or(curve::Mode::Auto);
                let temp = match &mode {
                    curve::Mode::Curve(cv) => value(&cv.source),
                    _ => None,
                };
                let test = self.tests.iter().any(|(t, _)| *t == id);
                let d = self.driven.entry((c, i)).or_insert_with(|| Driven {
                    driver: curve::Driver::default(),
                    status: FanStatus::Auto,
                    written: None,
                });
                let (out, status) = if test {
                    (curve::Output::Duty(100.0), curve::Status::Driven)
                } else {
                    d.driver.step(&mode, temp, self.safety, dt)
                };
                d.status = FanStatus::from_curve(status);
                match out {
                    curve::Output::Auto => {
                        if d.written.take().is_some() {
                            self.chips[c].restore(i);
                            changed = true;
                        }
                    }
                    curve::Output::Duty(p) => {
                        let v = curve::pwm_of(p);
                        if d.written != Some(v) || reassert {
                            if self.chips[c].set(i, v) {
                                d.written = Some(v);
                            } else {
                                d.status = FanStatus::Refused;
                            }
                            changed = true;
                        }
                    }
                }
            }
        }
        if changed {
            self.update_guard();
        }
    }

    fn release(&mut self) {
        for chip in &mut self.chips {
            chip.restore_all();
        }
        self.driven.clear();
        self.update_guard();
    }

    fn state(&self, temps: Vec<TempInfo>) -> FansState {
        let mut fans = Vec::new();
        for (c, chip) in self.chips.iter().enumerate() {
            let rpm = chip.rpm();
            let duty = chip.duty();
            for i in 0..chip.fans() {
                fans.push(FanInfo {
                    id: fan_id(chip, i),
                    chip: chip.name.to_owned(),
                    index: i as u8 + 1,
                    kind: FanKind::Motherboard,
                    label: None,
                    rpm: rpm.get(i).copied().flatten(),
                    duty: duty.get(i).copied().flatten(),
                    controllable: !chip.read_only && i < chip.controls(),
                    status: self
                        .driven
                        .get(&(c, i))
                        .map_or(FanStatus::Auto, |d| d.status),
                });
            }
        }
        FansState {
            access: Access::Ready,
            fans,
            temps,
            safety: self.safety,
            demo: false,
        }
    }
}

fn spawn_guard(path: &Path) -> Option<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let exe = std::env::current_exe().ok()?;
    std::process::Command::new(exe)
        .arg("--fan-guard")
        .arg(std::process::id().to_string())
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| tracing::warn!("fan guard not started: {e}"))
        .ok()
}

/// `tuxrgb.exe --fan-guard <pid> <file>`: waits for TuxRGB to end, then
/// puts back the fans it left taken over.
pub fn guard_main(pid: &str, file: &Path) -> i32 {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    let Ok(pid) = pid.parse::<u32>() else {
        return 2;
    };
    // SAFETY: handle of the parent process, closed after the wait.
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            WaitForSingleObject(h, INFINITE);
            let _ = CloseHandle(h);
        }
    }
    // A normal exit removed the file: nothing to do.
    let _ = restore_leftovers(file);
    0
}

/// Puts back the fans recorded in `file` (left by a run that ended without
/// giving them back), then removes it.
fn restore_leftovers(file: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(file) else {
        return false;
    };
    let records: Vec<_> = text.lines().filter_map(superio::parse_saved).collect();
    let mut chips = superio::detect();
    for (port, i, saved) in records {
        if let Some(chip) = chips.iter_mut().find(|c| c.port == port) {
            chip.restore_record(i, saved);
        }
    }
    let _ = std::fs::remove_file(file);
    true
}

pub fn run(inputs: SharedInputs, rx: Receiver<Command>, events: Events) {
    let mut access = if !crate::win::elevation::is_elevated() {
        Access::NeedsAdmin
    } else if !crate::win::elevation::pawnio_installed() {
        Access::NeedsPawnIo
    } else {
        Access::Ready
    };
    // TuxRGB was stopped by force while it drove fans, its guard with it
    // (an update, the task manager…): the fans go back to the motherboard
    // before anything else.
    if access == Access::Ready && restore_path().is_file() {
        tracing::info!("fans left driven by a previous run: given back");
        let _ = restore_leftovers(&restore_path());
    }
    // The chips are only looked for once fan control is used (page opened,
    // or a fan set): nothing touches them otherwise.
    let mut ctl: Option<Controller> = None;
    let mut config = FansConfig::default();
    let mut page = false;
    let mut last: Option<FansState> = None;
    // Control steps once a second, whatever arrives in between (the drags
    // of a curve point send many configurations): the ramps stay exact and
    // the controller is not read more than needed.
    let mut next_tick = Instant::now();
    loop {
        let active = access == Access::Ready
            && (page || config.any_driven() || ctl.as_ref().is_some_and(|c| !c.driven.is_empty()));
        let wait = if active {
            next_tick.saturating_duration_since(Instant::now())
        } else {
            Duration::from_secs(3600)
        };
        match rx.recv_timeout(wait) {
            Ok(Command::Configure(c)) => config = c,
            Ok(Command::Page(p)) => {
                page = p;
                last = None;
            }
            Ok(Command::Identify(id)) => {
                if let Some(c) = &mut ctl {
                    c.tests.push((id, Instant::now() + IDENTIFY));
                }
            }
            Ok(Command::Rescan) => {
                if let Some(c) = &mut ctl {
                    c.release();
                }
                access = if !crate::win::elevation::is_elevated() {
                    Access::NeedsAdmin
                } else if !crate::win::elevation::pawnio_installed() {
                    Access::NeedsPawnIo
                } else {
                    Access::Ready
                };
                ctl = None;
                last = None;
            }
            Ok(Command::Release(done)) => {
                if let Some(c) = &mut ctl {
                    c.release();
                }
                if let Some(d) = done {
                    let _ = d.send(());
                }
                continue;
            }
            Ok(Command::Quit) | Err(RecvTimeoutError::Disconnected) => {
                if let Some(c) = &mut ctl {
                    c.release();
                }
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if ctl.is_none() && access == Access::Ready && (page || config.any_driven()) {
            let c = Controller::new();
            tracing::info!(chips = c.chips.len(), "fan controllers (Windows)");
            ctl = Some(c);
        }
        // Not due yet, and the page already has a state: wait.
        if last.is_some() && Instant::now() < next_tick {
            continue;
        }
        next_tick = Instant::now() + Duration::from_secs(1);
        let state = match &mut ctl {
            Some(c) => {
                let temps = c.temps(&inputs);
                c.tick(&config, &temps);
                c.state(temps)
            }
            None => FansState {
                access: access.clone(),
                ..FansState::default()
            },
        };
        // Live values for the page; the first state in any case.
        if (page && last.as_ref() != Some(&state)) || last.is_none() {
            events(state.clone());
            last = Some(state);
        }
    }
}
