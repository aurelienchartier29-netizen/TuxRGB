//! Sensors of the "Moniteur matériel" effect and of the hardware displays
//! on Windows.
//!
//! * CPU load – `GetSystemTimes` deltas;
//! * CPU clock – `CallNtPowerInformation(ProcessorInformation)`;
//! * memory – `GlobalMemoryStatusEx`;
//! * GPU temperature / load / power / clock – `nvidia-smi` (installed with
//!   the NVIDIA driver), as on Linux;
//! * CPU temperature and power, sensors of AMD and Intel graphics cards –
//!   LibreHardwareMonitor through WMI, when it runs (see `hardware_lhm.rs`);
//! * GPU load of any card – the "GPU Engine" counters of the Task Manager
//!   (see `hardware_pdh.rs`), when nothing above gave it;
//! * GPU temperature of any card – the graphics kernel, as the Task Manager
//!   (see `hardware_d3dkmt.rs`), when nothing above gave it;
//! * CPU temperature and power – PawnIO (installed with TuxRGB) in
//!   administrator mode (see `hardware_pawnio.rs`), when LibreHardwareMonitor
//!   does not run.
//!
//! Windows exposes no CPU temperature without a kernel driver: without
//! LibreHardwareMonitor the effect follows the CPU load, exactly like on a
//! Linux machine whose sensor is unreadable.

#[path = "hardware_d3dkmt.rs"]
mod d3dkmt;

#[path = "hardware_adl.rs"]
mod adl;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tuxrgb_core::signal::{HardwareSample, SharedInputs};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Power::{CallNtPowerInformation, ProcessorInformation};
use windows::Win32::System::SystemInformation::{
    GetSystemInfo, GlobalMemoryStatusEx, MEMORYSTATUSEX, SYSTEM_INFO,
};
use windows::Win32::System::Threading::GetSystemTimes;

fn ticks(t: FILETIME) -> u64 {
    ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
}

#[derive(Default)]
struct CpuLoad {
    prev: Option<(u64, u64)>,
}

impl CpuLoad {
    /// Load since the previous call: (kernel + user − idle) / (kernel + user).
    fn update(&mut self) -> Option<f32> {
        let (mut idle, mut kernel, mut user) = Default::default();
        // SAFETY: three valid out-pointers.
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
        // Kernel time includes the idle time.
        let total = ticks(kernel) + ticks(user);
        let idle = ticks(idle);
        let (pt, pi) = self.prev.replace((total, idle))?;
        let dt = total.saturating_sub(pt);
        if dt == 0 {
            return None;
        }
        let busy = dt.saturating_sub(idle.saturating_sub(pi));
        Some((busy as f32 / dt as f32).clamp(0.0, 1.0))
    }
}

/// Memory in use: share, used GiB, installed GiB.
fn memory() -> Option<(f32, f32, f32)> {
    let mut m = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: dwLength is set, as the API requires.
    unsafe { GlobalMemoryStatusEx(&mut m) }.ok()?;
    (m.ullTotalPhys > 0).then(|| {
        let used = m.ullTotalPhys.saturating_sub(m.ullAvailPhys);
        const GIB: f32 = 1024.0 * 1024.0 * 1024.0;
        (
            (used as f32 / m.ullTotalPhys as f32).clamp(0.0, 1.0),
            used as f32 / GIB,
            m.ullTotalPhys as f32 / GIB,
        )
    })
}

/// `PROCESSOR_POWER_INFORMATION` (winnt.h; not in the metadata).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ProcessorPower {
    number: u32,
    max_mhz: u32,
    current_mhz: u32,
    mhz_limit: u32,
    max_idle_state: u32,
    current_idle_state: u32,
}

/// Fastest logical processor right now, MHz.
fn cpu_clock() -> Option<f32> {
    let mut info = SYSTEM_INFO::default();
    // SAFETY: valid out-pointer.
    unsafe { GetSystemInfo(&mut info) };
    let n = (info.dwNumberOfProcessors as usize).clamp(1, 1024);
    let mut cpus = vec![ProcessorPower::default(); n];
    // SAFETY: the buffer holds `n` entries of the documented layout.
    let status = unsafe {
        CallNtPowerInformation(
            ProcessorInformation,
            None,
            0,
            Some(cpus.as_mut_ptr().cast()),
            (n * std::mem::size_of::<ProcessorPower>()) as u32,
        )
    };
    if status.is_err() {
        return None;
    }
    cpus.iter()
        .map(|c| c.current_mhz)
        .max()
        .filter(|m| *m > 0)
        .map(|m| m as f32)
}

/// `nvidia-smi.exe`: System32 on current drivers, NVSMI on older ones.
fn find_nvidia_smi() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(root) = std::env::var_os("SystemRoot") {
        candidates.push(PathBuf::from(root).join("System32").join("nvidia-smi.exe"));
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        candidates.push(
            PathBuf::from(pf)
                .join("NVIDIA Corporation")
                .join("NVSMI")
                .join("nvidia-smi.exe"),
        );
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// How often LibreHardwareMonitor is looked for while it is not running.
const LHM_RETRY: Duration = Duration::from_secs(30);

/// LibreHardwareMonitor, found or not, and its last reading.
#[derive(Default)]
struct Monitor {
    connection: Option<super::lhm::Lhm>,
    tried: Option<Instant>,
    reading: Option<(Instant, super::lhm::LhmReading)>,
}

impl Monitor {
    fn update(&mut self) -> super::lhm::LhmReading {
        if self.connection.is_none() && self.tried.is_none_or(|t| t.elapsed() >= LHM_RETRY) {
            self.tried = Some(Instant::now());
            self.connection = super::lhm::Lhm::connect();
        }
        if let Some(c) = &self.connection {
            let due = self
                .reading
                .is_none_or(|(at, _)| at.elapsed() >= super::NVIDIA_PERIOD);
            if due {
                match c.read() {
                    Ok(sensors) => {
                        self.reading = Some((Instant::now(), super::lhm::pick(&sensors)));
                    }
                    Err(e) => {
                        tracing::debug!(%e, "hardware monitor closed");
                        self.connection = None;
                        self.reading = None;
                    }
                }
            }
        }
        self.reading.map(|(_, r)| r).unwrap_or_default()
    }
}

pub(crate) fn run(inputs: SharedInputs, stop: std::sync::Arc<AtomicBool>, period: Duration) {
    // SAFETY: once on this thread, balanced at the end.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    sample_until_stopped(&inputs, &stop, period, com);
    if com {
        // SAFETY: every COM object of this thread is dropped by now.
        unsafe { CoUninitialize() };
    }
}

fn sample_until_stopped(inputs: &SharedInputs, stop: &AtomicBool, period: Duration, com: bool) {
    let smi = find_nvidia_smi();
    tracing::info!(nvidia = smi.is_some(), "hardware sensors (Windows)");
    let mut cpu = CpuLoad::default();
    let mut nvidia: Option<(Instant, super::NvidiaReading)> = None;
    let mut monitor = Monitor::default();
    let gpu_counters = super::pdh::GpuCounters::open();
    let gpu_kernel = d3dkmt::GpuKernel::open();
    let mut cpu_sensors = super::pawnio::CpuSensors::open();
    let amd = adl::AmdGpu::open();
    tracing::info!(
        cpu_pawnio = cpu_sensors.is_some(),
        amd_adl = amd.is_some(),
        gpu_kernel = gpu_kernel.is_some(),
        "sensor sources (Windows)"
    );
    tracing::info!(gpu_counters = gpu_counters.is_some(), "GPU engine counters");
    while !stop.load(Ordering::Relaxed) {
        let mem = memory();
        let mut sample = HardwareSample {
            cpu_load: cpu.update(),
            cpu_freq: cpu_clock(),
            mem_used: mem.map(|m| m.0),
            mem_used_gib: mem.map(|m| m.1),
            mem_total_gib: mem.map(|m| m.2),
            captured_at: Instant::now(),
            ..HardwareSample::default()
        };
        if let Some(smi) = &smi {
            let due = nvidia.is_none_or(|(at, _)| at.elapsed() >= super::NVIDIA_PERIOD);
            if due {
                nvidia = Some((Instant::now(), super::query_nvidia(smi)));
            }
            if let Some((_, r)) = nvidia {
                sample.gpu_temp = r.temp;
                sample.gpu_load = r.load;
                sample.gpu_power = r.power;
                sample.gpu_freq = r.clock;
            }
        }
        if com {
            // nvidia-smi first for an NVIDIA card; the monitor fills the gaps.
            let m = monitor.update();
            sample.cpu_temp = m.cpu_temp;
            sample.cpu_power = m.cpu_power;
            sample.gpu_temp = sample.gpu_temp.or(m.gpu_temp);
            sample.gpu_load = sample.gpu_load.or(m.gpu_load);
            sample.gpu_power = sample.gpu_power.or(m.gpu_power);
            sample.gpu_freq = sample.gpu_freq.or(m.gpu_freq);
        }
        // AMD cards: AMD's own library, the most complete.
        if let Some(a) = amd.as_ref().and_then(|g| g.read()) {
            sample.gpu_temp = sample.gpu_temp.or(a.temp);
            sample.gpu_load = sample.gpu_load.or(a.load);
            sample.gpu_power = sample.gpu_power.or(a.power);
            sample.gpu_freq = sample.gpu_freq.or(a.clock);
        }
        // Read every period (a rate since the previous collection).
        let engines = gpu_counters.as_ref().and_then(|c| c.read());
        sample.gpu_load = sample.gpu_load.or(engines);
        if let Some(c) = cpu_sensors.as_mut() {
            let power = c.power();
            if sample.cpu_temp.is_none() {
                sample.cpu_temp = c.temperature();
            }
            sample.cpu_power = sample.cpu_power.or(power);
        }
        if sample.gpu_temp.is_none() {
            sample.gpu_temp = gpu_kernel.as_ref().and_then(|k| k.temperature());
        }
        inputs.set_hardware(Some(sample));
        std::thread::park_timeout(period);
    }
    inputs.set_hardware(None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_are_readable() {
        let mut c = CpuLoad::default();
        assert_eq!(c.update(), None);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = c.update();
        let m = memory().expect("memory");
        assert!((0.0..=1.0).contains(&m.0));
        assert!(m.2 > 0.0);
    }
}
