//! Sensors of the "Moniteur matériel" effect on Windows.
//!
//! * CPU load – `GetSystemTimes` deltas;
//! * memory – `GlobalMemoryStatusEx`;
//! * GPU temperature / load – `nvidia-smi` (installed with the NVIDIA
//!   driver), as on Linux.
//!
//! Windows exposes no CPU temperature without a kernel driver: the effect
//! then follows the CPU load, exactly like on a Linux machine whose sensor
//! is unreadable.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tuxrgb_core::signal::{HardwareSample, SharedInputs};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
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

fn memory_used() -> Option<f32> {
    let mut m = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: dwLength is set, as the API requires.
    unsafe { GlobalMemoryStatusEx(&mut m) }.ok()?;
    (m.ullTotalPhys > 0)
        .then(|| ((m.ullTotalPhys - m.ullAvailPhys) as f32 / m.ullTotalPhys as f32).clamp(0.0, 1.0))
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

pub(crate) fn run(inputs: SharedInputs, stop: std::sync::Arc<AtomicBool>) {
    let smi = find_nvidia_smi();
    tracing::info!(nvidia = smi.is_some(), "hardware sensors (Windows)");
    let mut cpu = CpuLoad::default();
    let mut nvidia: Option<(Instant, Option<f32>, Option<f32>)> = None;
    while !stop.load(Ordering::Relaxed) {
        let mut sample = HardwareSample {
            cpu_temp: None,
            cpu_load: cpu.update(),
            gpu_temp: None,
            gpu_load: None,
            mem_used: memory_used(),
            captured_at: Instant::now(),
        };
        if let Some(smi) = &smi {
            let due = nvidia.is_none_or(|(at, _, _)| at.elapsed() >= super::NVIDIA_PERIOD);
            if due {
                let (t, l) = super::query_nvidia(smi);
                nvidia = Some((Instant::now(), t, l));
            }
            if let Some((_, t, l)) = nvidia {
                sample.gpu_temp = t;
                sample.gpu_load = l;
            }
        }
        inputs.set_hardware(Some(sample));
        std::thread::park_timeout(super::PERIOD);
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
        let m = memory_used().expect("memory");
        assert!((0.0..=1.0).contains(&m));
    }
}
