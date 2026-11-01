//! CPU / GPU / memory sensors for the "Moniteur matériel" effect (the
//! native equivalent of the OpenRGB Hardware Sync plugin).
//!
//! Everything comes from the kernel, no daemon and no root needed:
//!
//! * CPU temperature – hwmon `k10temp` / `zenpower` (AMD), `coretemp`
//!   (Intel), then the `x86_pkg_temp` thermal zone;
//! * CPU load – deltas of `/proc/stat`;
//! * GPU temperature / load – hwmon `amdgpu` / `nouveau` / `radeon`,
//!   `gpu_busy_percent`, and `nvidia-smi` for the proprietary NVIDIA driver;
//! * memory – `MemAvailable` / `MemTotal` of `/proc/meminfo`;
//! * CPU power – RAPL (`intel-rapl:0`) or the `zenergy` / `amd_energy`
//!   counters (RAPL is readable by root only on most distributions: the
//!   value is then simply missing);
//! * CPU clock – the fastest `scaling_cur_freq` of the cores;
//! * GPU power / clock – hwmon `power1_average` / `freq1_input`, and
//!   `nvidia-smi` for the proprietary NVIDIA driver.
//!
//! The sampler thread wakes up once per period (one second by default,
//! shorter when a hardware display refreshes faster) and only runs while
//! the effect or a display needs it. The Windows build samples its own
//! counters (`hardware_windows.rs`) and shares the `nvidia-smi` query.
#![cfg_attr(windows, allow(dead_code))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tuxrgb_core::signal::{HardwareSample, SharedInputs};

/// Default sampling period.
pub const PERIOD: Duration = Duration::from_millis(1000);
/// Fastest sampling period accepted.
pub const MIN_PERIOD: Duration = Duration::from_millis(250);
/// `nvidia-smi` is a process launch: not more often than this.
const NVIDIA_PERIOD: Duration = Duration::from_secs(2);

#[cfg(windows)]
#[path = "hardware_windows.rs"]
mod windows_sensors;

#[path = "hardware_lhm.rs"]
mod lhm;

#[path = "hardware_pdh.rs"]
mod pdh;

#[path = "hardware_pawnio.rs"]
mod pawnio;

pub struct HardwareMonitor {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl HardwareMonitor {
    pub fn start(inputs: SharedInputs, period: Duration) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let period = period.max(MIN_PERIOD);
        let thread = std::thread::Builder::new()
            .name("tuxrgb-sensors".into())
            .spawn(move || {
                #[cfg(windows)]
                windows_sensors::run(inputs, flag, period);
                #[cfg(not(windows))]
                run(inputs, flag, period);
            })?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for HardwareMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

fn run(inputs: SharedInputs, stop: Arc<AtomicBool>, period: Duration) {
    let sensors = Sensors::discover(Path::new("/sys"));
    tracing::info!(
        cpu_temp = ?sensors.cpu_temp,
        cpu_energy = ?sensors.cpu_energy,
        cpu_freq = sensors.cpu_freq.len(),
        gpu_temp = ?sensors.gpu_temp,
        gpu_busy = ?sensors.gpu_busy,
        gpu_power = ?sensors.gpu_power,
        nvidia = sensors.nvidia_smi.is_some(),
        nvidia_pm = ?sensors.nvidia_pm,
        "hardware sensors"
    );
    let mut cpu = CpuLoad::default();
    let mut energy = EnergyRate::default();
    let mut nvidia: Option<(Instant, NvidiaReading)> = None;
    // Read buffers kept for the whole run (no allocation per sample).
    let (mut stat, mut meminfo, mut small) = (String::new(), String::new(), String::new());
    while !stop.load(Ordering::Relaxed) {
        let mem = read_into("/proc/meminfo".as_ref(), &mut meminfo).and_then(memory);
        let mut sample = HardwareSample {
            cpu_temp: sensors
                .cpu_temp
                .as_deref()
                .and_then(|p| read_millidegrees_with(p, &mut small)),
            cpu_load: read_into("/proc/stat".as_ref(), &mut stat).and_then(|s| cpu.update(s)),
            cpu_power: sensors.cpu_energy.as_ref().and_then(|e| {
                let uj = read_number_with(&e.path, &mut small)? as f64;
                energy.update(uj, e.max_uj)
            }),
            cpu_freq: sensors
                .cpu_freq
                .iter()
                .filter_map(|p| read_number_with(p, &mut small))
                .reduce(f32::max)
                .map(|khz| khz / 1000.0),
            gpu_temp: sensors
                .gpu_temp
                .as_deref()
                .and_then(|p| read_millidegrees_with(p, &mut small)),
            gpu_load: sensors
                .gpu_busy
                .as_deref()
                .and_then(|p| read_number_with(p, &mut small))
                .map(|p| (p / 100.0).clamp(0.0, 1.0)),
            gpu_power: sensors
                .gpu_power
                .as_deref()
                .and_then(|p| read_number_with(p, &mut small))
                .map(|uw| uw / 1_000_000.0),
            gpu_freq: sensors
                .gpu_freq
                .as_deref()
                .and_then(|p| read_number_with(p, &mut small))
                .map(|hz| hz / 1_000_000.0),
            mem_used: mem.map(|m| m.used),
            mem_used_gib: mem.map(|m| m.used_gib),
            mem_total_gib: mem.map(|m| m.total_gib),
            captured_at: Instant::now(),
        };
        if let Some(smi) = &sensors.nvidia_smi
            && (sample.gpu_temp.is_none() || sample.gpu_load.is_none())
        {
            if sensors.nvidia_asleep(&mut small) {
                // Runtime-suspended discrete GPU (hybrid laptop): querying
                // it would wake it up and drain the battery. It is idle;
                // its last known temperature is kept.
                let temp = nvidia.and_then(|(_, r)| r.temp);
                nvidia = Some((
                    Instant::now(),
                    NvidiaReading {
                        temp,
                        load: Some(0.0),
                        ..NvidiaReading::default()
                    },
                ));
            } else {
                let due = nvidia.is_none_or(|(at, _)| at.elapsed() >= NVIDIA_PERIOD);
                if due {
                    nvidia = Some((Instant::now(), query_nvidia(smi)));
                }
            }
            if let Some((_, r)) = nvidia {
                sample.gpu_temp = sample.gpu_temp.or(r.temp);
                sample.gpu_load = sample.gpu_load.or(r.load);
                sample.gpu_power = sample.gpu_power.or(r.power);
                sample.gpu_freq = sample.gpu_freq.or(r.clock);
            }
        }
        inputs.set_hardware(Some(sample));
        std::thread::park_timeout(period);
    }
    inputs.set_hardware(None);
}

/// Power from a cumulative energy counter (µJ): the rate between two
/// readings.
#[derive(Default)]
struct EnergyRate {
    prev: Option<(f64, Instant)>,
}

impl EnergyRate {
    fn update(&mut self, uj: f64, max_uj: Option<f64>) -> Option<f32> {
        let now = Instant::now();
        let (prev, at) = self.prev.replace((uj, now))?;
        let dt = now.duration_since(at).as_secs_f64();
        if dt < 0.05 {
            return None;
        }
        let delta = if uj >= prev {
            uj - prev
        } else {
            // A bounded counter wrapped (RAPL); unbounded ones only go back
            // when reset.
            max_uj? + uj - prev
        };
        let watts = delta / dt / 1_000_000.0;
        (0.0..2000.0).contains(&watts).then_some(watts as f32)
    }
}

/// A cumulative CPU energy counter.
#[derive(Debug, Clone, PartialEq)]
pub struct EnergyCounter {
    pub path: PathBuf,
    /// Value at which it wraps (RAPL).
    pub max_uj: Option<f64>,
}

/// Paths of the sensor files found on this machine.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Sensors {
    pub cpu_temp: Option<PathBuf>,
    /// Package energy (power is its rate), when readable.
    pub cpu_energy: Option<EnergyCounter>,
    /// `scaling_cur_freq` of every core (kHz).
    pub cpu_freq: Vec<PathBuf>,
    pub gpu_temp: Option<PathBuf>,
    pub gpu_busy: Option<PathBuf>,
    /// hwmon power (µW) and core clock (Hz) of the GPU.
    pub gpu_power: Option<PathBuf>,
    pub gpu_freq: Option<PathBuf>,
    pub nvidia_smi: Option<PathBuf>,
    /// `power/runtime_status` of the NVIDIA GPU (PCI runtime power
    /// management, laptops with a discrete GPU).
    pub nvidia_pm: Option<PathBuf>,
}

impl Sensors {
    /// The NVIDIA GPU is powered down by runtime power management.
    fn nvidia_asleep(&self, buf: &mut String) -> bool {
        self.nvidia_pm
            .as_deref()
            .and_then(|p| read_into(p, buf))
            .is_some_and(|s| s.trim() == "suspended")
    }
}

impl Sensors {
    /// Looks for sensors under `sys` (normally `/sys`).
    pub fn discover(sys: &Path) -> Self {
        let mut s = Sensors::default();
        let mut cpu_rank = usize::MAX;
        let mut gpu_rank = usize::MAX;
        for dir in list_dir(&sys.join("class/hwmon")) {
            let Some(name) = read_trimmed(&dir.join("name")) else {
                continue;
            };
            // Preferred CPU sensor first, with its preferred label.
            let cpu = match name.as_str() {
                "k10temp" => Some((0, &["Tctl", "Tdie"][..])),
                "zenpower" => Some((1, &["Tdie", "Tctl"][..])),
                "coretemp" => Some((2, &["Package id 0"][..])),
                "cpu_thermal" | "soc_thermal" => Some((3, &[][..])),
                _ => None,
            };
            if let Some((rank, labels)) = cpu
                && rank < cpu_rank
                && let Some(p) = temp_input(&dir, labels)
            {
                cpu_rank = rank;
                s.cpu_temp = Some(p);
            }
            let gpu = match name.as_str() {
                "amdgpu" => Some((0, &["edge", "junction"][..])),
                "nouveau" => Some((1, &[][..])),
                "radeon" => Some((2, &[][..])),
                _ => None,
            };
            if let Some((rank, labels)) = gpu
                && rank < gpu_rank
                && let Some(p) = temp_input(&dir, labels)
            {
                gpu_rank = rank;
                s.gpu_temp = Some(p);
                // hwmon/hwmonN/device → the PCI device of the GPU.
                let busy = dir.join("device/gpu_busy_percent");
                if busy.exists() {
                    s.gpu_busy = Some(busy);
                }
                s.gpu_power = ["power1_average", "power1_input"]
                    .iter()
                    .map(|f| dir.join(f))
                    .find(|p| readable(p));
                s.gpu_freq = Some(dir.join("freq1_input")).filter(|p| readable(p));
            }
            // AMD package energy (zenergy / amd_energy modules).
            if s.cpu_energy.is_none()
                && matches!(name.as_str(), "zenergy" | "amd_energy")
                && let Some(path) = socket_energy(&dir)
            {
                s.cpu_energy = Some(EnergyCounter { path, max_uj: None });
            }
        }
        // Intel / AMD RAPL: often root-only (it is then skipped).
        let rapl = sys.join("class/powercap/intel-rapl/intel-rapl:0");
        if readable(&rapl.join("energy_uj")) {
            s.cpu_energy = Some(EnergyCounter {
                path: rapl.join("energy_uj"),
                max_uj: read_trimmed(&rapl.join("max_energy_range_uj"))
                    .and_then(|v| v.parse().ok()),
            });
        }
        s.cpu_freq = list_dir(&sys.join("devices/system/cpu"))
            .into_iter()
            .filter(|d| {
                d.file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_prefix("cpu"))
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
            .map(|d| d.join("cpufreq/scaling_cur_freq"))
            .filter(|p| p.exists())
            .collect();
        if s.cpu_temp.is_none() {
            for zone in list_dir(&sys.join("class/thermal")) {
                if matches!(
                    read_trimmed(&zone.join("type")).as_deref(),
                    Some("x86_pkg_temp" | "TCPU" | "cpu-thermal")
                ) {
                    s.cpu_temp = Some(zone.join("temp"));
                    break;
                }
            }
        }
        if s.gpu_busy.is_none() {
            for card in list_dir(&sys.join("class/drm")) {
                let busy = card.join("device/gpu_busy_percent");
                if busy.exists() {
                    s.gpu_busy = Some(busy);
                    break;
                }
            }
        }
        if Path::new("/proc/driver/nvidia/version").exists() {
            s.nvidia_smi = [
                "/usr/bin/nvidia-smi",
                "/usr/local/bin/nvidia-smi",
                "/bin/nvidia-smi",
            ]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.exists());
            s.nvidia_pm = list_dir(&sys.join("bus/pci/devices"))
                .into_iter()
                .find(|d| {
                    read_trimmed(&d.join("vendor")).as_deref() == Some("0x10de")
                        && read_trimmed(&d.join("class")).is_some_and(|c| c.starts_with("0x03"))
                })
                .map(|d| d.join("power/runtime_status"))
                .filter(|p| p.exists());
        }
        s
    }
}

/// The file exists and this user can read a value from it.
fn readable(p: &Path) -> bool {
    std::fs::read_to_string(p).is_ok_and(|s| !s.trim().is_empty())
}

/// `energyN_input` of the package (`Esocket…` label) of an AMD hwmon.
fn socket_energy(dir: &Path) -> Option<PathBuf> {
    (1..=64).find_map(|i| {
        read_trimmed(&dir.join(format!("energy{i}_label")))
            .filter(|l| l.starts_with("Esocket"))
            .map(|_| dir.join(format!("energy{i}_input")))
            .filter(|p| readable(p))
    })
}

fn list_dir(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|it| it.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn read_trimmed(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_owned())
}

/// Reads `p` into `buf` (its allocation is reused).
fn read_into<'a>(p: &Path, buf: &'a mut String) -> Option<&'a str> {
    use std::io::Read;
    buf.clear();
    std::fs::File::open(p).ok()?.read_to_string(buf).ok()?;
    Some(buf.as_str())
}

#[cfg(test)]
fn read_number(p: &Path) -> Option<f32> {
    read_number_with(p, &mut String::new())
}

fn read_number_with(p: &Path, buf: &mut String) -> Option<f32> {
    read_into(p, buf)?.trim().parse::<f32>().ok()
}

#[cfg(test)]
fn read_millidegrees(p: &Path) -> Option<f32> {
    read_millidegrees_with(p, &mut String::new())
}

fn read_millidegrees_with(p: &Path, buf: &mut String) -> Option<f32> {
    let v = read_number_with(p, buf)? / 1000.0;
    (-40.0..200.0).contains(&v).then_some(v)
}

/// `tempN_input` whose `tempN_label` matches one of `labels` (in order), or
/// the first one.
fn temp_input(dir: &Path, labels: &[&str]) -> Option<PathBuf> {
    for wanted in labels {
        for i in 1..=16 {
            if read_trimmed(&dir.join(format!("temp{i}_label"))).as_deref() == Some(*wanted) {
                let p = dir.join(format!("temp{i}_input"));
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    (1..=16)
        .map(|i| dir.join(format!("temp{i}_input")))
        .find(|p| p.exists())
}

#[derive(Default)]
struct CpuLoad {
    prev: Option<(u64, u64)>,
}

impl CpuLoad {
    /// Load since the previous call, from the `cpu` line of `/proc/stat`.
    fn update(&mut self, stat: &str) -> Option<f32> {
        let line = stat.lines().find(|l| l.starts_with("cpu "))?;
        let v: Vec<u64> = line
            .split_whitespace()
            .skip(1)
            .filter_map(|x| x.parse().ok())
            .collect();
        if v.len() < 4 {
            return None;
        }
        let idle = v[3] + v.get(4).copied().unwrap_or(0);
        let total: u64 = v.iter().take(8).sum();
        let prev = self.prev.replace((total, idle));
        let (pt, pi) = prev?;
        let dt = total.saturating_sub(pt);
        if dt == 0 {
            return None;
        }
        let busy = dt.saturating_sub(idle.saturating_sub(pi));
        Some((busy as f32 / dt as f32).clamp(0.0, 1.0))
    }
}

/// Memory in use: share and GiB.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Memory {
    used: f32,
    used_gib: f32,
    total_gib: f32,
}

fn memory(meminfo: &str) -> Option<Memory> {
    let field = |name: &str| -> Option<f32> {
        meminfo
            .lines()
            .find(|l| l.starts_with(name))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    };
    let total = field("MemTotal:")?;
    let available = field("MemAvailable:")?;
    (total > 0.0).then(|| {
        let used_kib = (total - available).max(0.0);
        Memory {
            used: (used_kib / total).clamp(0.0, 1.0),
            used_gib: used_kib / 1_048_576.0,
            total_gib: total / 1_048_576.0,
        }
    })
}

/// What `nvidia-smi` reports for the first GPU.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct NvidiaReading {
    pub temp: Option<f32>,
    pub load: Option<f32>,
    pub power: Option<f32>,
    pub clock: Option<f32>,
}

pub(crate) fn query_nvidia(smi: &Path) -> NvidiaReading {
    let mut cmd = std::process::Command::new(smi);
    #[cfg(windows)]
    {
        // No console window flashing up every two seconds.
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.args([
        "--query-gpu=temperature.gpu,utilization.gpu,power.draw,clocks.gr",
        "--format=csv,noheader,nounits",
        "--id=0",
    ]);
    // A driver in trouble can make nvidia-smi hang: never wait more than
    // this (the sensors thread, and stopping the effect, depend on it).
    let Some(out) =
        tuxrgb_core::process::output_with_timeout(cmd, Duration::from_millis(1500), 4096)
    else {
        return NvidiaReading::default();
    };
    parse_nvidia(&String::from_utf8_lossy(&out))
}

fn parse_nvidia(text: &str) -> NvidiaReading {
    // "[N/A]" or "[Not Supported]" for what the GPU does not report.
    let mut parts = text
        .lines()
        .next()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().parse::<f32>().ok());
    NvidiaReading {
        temp: parts.next().flatten(),
        load: parts.next().flatten().map(|p| (p / 100.0).clamp(0.0, 1.0)),
        power: parts.next().flatten(),
        clock: parts.next().flatten(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_load_from_proc_stat() {
        let mut c = CpuLoad::default();
        assert_eq!(c.update("cpu  100 0 100 800 0 0 0 0 0 0\n"), None);
        let load = c
            .update("cpu  200 0 200 900 0 0 0 0 0 0\n")
            .expect("second sample");
        // 200 busy out of 300 ticks.
        assert!((load - 2.0 / 3.0).abs() < 1e-3, "{load}");
    }

    #[test]
    fn memory_from_meminfo() {
        let m = "MemTotal:       32000000 kB\nMemFree:  1000 kB\nMemAvailable:   24000000 kB\n";
        let mem = memory(m).unwrap();
        assert!((mem.used - 0.25).abs() < 1e-4);
        assert!((mem.used_gib - 7.629).abs() < 1e-2, "{mem:?}");
        assert!((mem.total_gib - 30.518).abs() < 1e-2, "{mem:?}");
    }

    #[test]
    fn nvidia_smi_output() {
        let r = parse_nvidia("63, 41, 182.37, 2520\n");
        assert_eq!(r.temp, Some(63.0));
        assert_eq!(r.load, Some(0.41));
        assert_eq!(r.power, Some(182.37));
        assert_eq!(r.clock, Some(2520.0));
        let partial = parse_nvidia("55, 3, [N/A], 1410\n");
        assert_eq!(partial.power, None);
        assert_eq!(partial.clock, Some(1410.0));
    }

    #[test]
    fn power_is_the_rate_of_the_energy_counter() {
        let mut e = EnergyRate::default();
        assert_eq!(e.update(1_000_000.0, None), None);
        std::thread::sleep(Duration::from_millis(100));
        // 10 J in ~0.1 s: ~100 W.
        let w = e.update(11_000_000.0, None).unwrap();
        assert!((70.0..130.0).contains(&w), "{w}");
        // A wrapping RAPL counter.
        std::thread::sleep(Duration::from_millis(100));
        let w = e.update(1_000_000.0, Some(20_000_000.0)).unwrap();
        assert!((70.0..130.0).contains(&w), "{w}");
        // An unbounded counter going back: unknown.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(e.update(0.0, None), None);
    }

    #[test]
    fn discovers_amd_sensors_in_a_fake_sysfs() {
        let root = std::env::temp_dir().join(format!("tuxrgb-sysfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cpu = root.join("class/hwmon/hwmon1");
        let gpu = root.join("class/hwmon/hwmon3");
        std::fs::create_dir_all(&cpu).unwrap();
        std::fs::create_dir_all(gpu.join("device")).unwrap();
        std::fs::write(cpu.join("name"), "k10temp\n").unwrap();
        std::fs::write(cpu.join("temp1_label"), "Tctl\n").unwrap();
        std::fs::write(cpu.join("temp1_input"), "61250\n").unwrap();
        std::fs::write(cpu.join("temp3_label"), "Tccd1\n").unwrap();
        std::fs::write(cpu.join("temp3_input"), "55000\n").unwrap();
        std::fs::write(gpu.join("name"), "amdgpu\n").unwrap();
        std::fs::write(gpu.join("temp2_label"), "junction\n").unwrap();
        std::fs::write(gpu.join("temp2_input"), "70000\n").unwrap();
        std::fs::write(gpu.join("temp1_label"), "edge\n").unwrap();
        std::fs::write(gpu.join("temp1_input"), "52000\n").unwrap();
        std::fs::write(gpu.join("device/gpu_busy_percent"), "37\n").unwrap();
        std::fs::write(gpu.join("power1_average"), "123000000\n").unwrap();
        std::fs::write(gpu.join("freq1_input"), "2100000000\n").unwrap();
        for (cpu, khz) in [("cpu0", "3600000"), ("cpu1", "4950000")] {
            let d = root.join("devices/system/cpu").join(cpu).join("cpufreq");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("scaling_cur_freq"), format!("{khz}\n")).unwrap();
        }
        std::fs::create_dir_all(root.join("devices/system/cpu/cpufreq")).unwrap();
        let s = Sensors::discover(&root);
        assert_eq!(s.cpu_freq.len(), 2);
        assert_eq!(s.gpu_power, Some(gpu.join("power1_average")));
        assert_eq!(s.gpu_freq, Some(gpu.join("freq1_input")));
        assert_eq!(s.cpu_temp, Some(cpu.join("temp1_input")));
        assert_eq!(s.gpu_temp, Some(gpu.join("temp1_input")));
        assert_eq!(read_millidegrees(s.cpu_temp.as_ref().unwrap()), Some(61.25));
        assert_eq!(read_number(s.gpu_busy.as_ref().unwrap()), Some(37.0));
        let _ = std::fs::remove_dir_all(&root);
    }
}
