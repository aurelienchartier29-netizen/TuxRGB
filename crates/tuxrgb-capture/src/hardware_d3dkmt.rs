//! GPU temperature on Windows from the graphics kernel (`D3DKMT`), the
//! value the Task Manager shows under "GPU temperature". Every card with a
//! WDDM 2.4 driver reports it (AMD, NVIDIA, Intel Arc), no program needed.
//!
//! The functions are looked up in `gdi32.dll` at run time: an old Windows
//! (or Wine) without them only loses this reading.

use std::ffi::c_void;
use windows::Win32::Foundation::LUID;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{s, w};

/// `D3DKMT_ADAPTERINFO`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AdapterInfo {
    adapter: u32,
    luid: LUID,
    sources: u32,
    precise_present_regions: i32,
}

/// `D3DKMT_ENUMADAPTERS2`.
#[repr(C)]
struct EnumAdapters2 {
    count: u32,
    adapters: *mut AdapterInfo,
}

/// `D3DKMT_QUERYADAPTERINFO`.
#[repr(C)]
struct QueryAdapterInfo {
    adapter: u32,
    kind: u32,
    data: *mut c_void,
    size: u32,
}

/// `D3DKMT_CLOSEADAPTER`.
#[repr(C)]
struct CloseAdapter {
    adapter: u32,
}

/// `D3DKMT_ADAPTER_PERFDATA` (d3dkmthk.h, WDDM 2.4).
#[repr(C)]
#[derive(Default)]
struct PerfData {
    physical_adapter: u32,
    memory_frequency: u64,
    max_memory_frequency: u64,
    max_memory_frequency_oc: u64,
    memory_bandwidth: u64,
    pcie_bandwidth: u64,
    fan_rpm: u32,
    /// Tenths of a percent of the board power.
    power: u32,
    /// Tenths of a degree Celsius.
    temperature: u32,
    power_state_override: u8,
}

/// `KMTQAITYPE_ADAPTERPERFDATA`.
const ADAPTER_PERF_DATA: u32 = 62;

type EnumFn = unsafe extern "system" fn(*mut EnumAdapters2) -> i32;
type QueryFn = unsafe extern "system" fn(*const QueryAdapterInfo) -> i32;
type CloseFn = unsafe extern "system" fn(*const CloseAdapter) -> i32;

pub(crate) struct GpuKernel {
    enumerate: EnumFn,
    query: QueryFn,
    close: CloseFn,
}

impl GpuKernel {
    pub(crate) fn open() -> Option<Self> {
        // SAFETY: gdi32 stays loaded for the life of the process; the
        // signatures are those of d3dkmthk.h.
        unsafe {
            let gdi = LoadLibraryW(w!("gdi32.dll")).ok()?;
            let enumerate = GetProcAddress(gdi, s!("D3DKMTEnumAdapters2"))?;
            let query = GetProcAddress(gdi, s!("D3DKMTQueryAdapterInfo"))?;
            let close = GetProcAddress(gdi, s!("D3DKMTCloseAdapter"))?;
            Some(Self {
                enumerate: std::mem::transmute::<unsafe extern "system" fn() -> isize, EnumFn>(
                    enumerate,
                ),
                query: std::mem::transmute::<unsafe extern "system" fn() -> isize, QueryFn>(query),
                close: std::mem::transmute::<unsafe extern "system" fn() -> isize, CloseFn>(close),
            })
        }
    }

    /// Hottest graphics card, °C (an idle integrated GPU is cooler than the
    /// card that works); `None` when no card reports a temperature.
    pub(crate) fn temperature(&self) -> Option<f32> {
        // SAFETY: the calls follow d3dkmthk.h: count, then fill; every
        // opened adapter is closed.
        unsafe {
            let mut e = EnumAdapters2 {
                count: 0,
                adapters: std::ptr::null_mut(),
            };
            if (self.enumerate)(&mut e) != 0 || e.count == 0 {
                return None;
            }
            let mut adapters = vec![AdapterInfo::default(); e.count.min(64) as usize];
            e.count = adapters.len() as u32;
            e.adapters = adapters.as_mut_ptr();
            if (self.enumerate)(&mut e) != 0 {
                return None;
            }
            adapters.truncate(e.count as usize);
            let mut best: Option<f32> = None;
            for a in &adapters {
                let mut perf = PerfData::default();
                let q = QueryAdapterInfo {
                    adapter: a.adapter,
                    kind: ADAPTER_PERF_DATA,
                    data: (&mut perf as *mut PerfData).cast(),
                    size: std::mem::size_of::<PerfData>() as u32,
                };
                if (self.query)(&q) == 0 && perf.temperature > 0 {
                    let t = perf.temperature as f32 / 10.0;
                    if (1.0..150.0).contains(&t) {
                        best = Some(best.map_or(t, |b: f32| b.max(t)));
                    }
                }
                (self.close)(&CloseAdapter { adapter: a.adapter });
            }
            best
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_d3dkmthk() {
        assert_eq!(std::mem::size_of::<AdapterInfo>(), 20);
        assert_eq!(std::mem::size_of::<QueryAdapterInfo>(), 24);
        assert_eq!(std::mem::size_of::<PerfData>(), 64);
        assert_eq!(std::mem::offset_of!(PerfData, temperature), 56);
    }

    #[test]
    fn reading_is_harmless() {
        // Wine has no such data: no value, no crash.
        if let Some(k) = GpuKernel::open() {
            let _ = k.temperature();
        }
    }
}
