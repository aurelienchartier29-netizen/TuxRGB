//! Sensors of AMD Radeon graphics cards on Windows, through AMD's own
//! library (`atiadlxx.dll`, installed with every AMD driver), as
//! LibreHardwareMonitor and the AMD software read them: temperature, load,
//! power and clock. No program and no rights needed.
//!
//! The library is loaded at run time: without an AMD card nothing happens.

use std::ffi::c_void;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{s, w};

/// `ADL_PMLOG_MAX_SENSORS`.
const MAX_SENSORS: usize = 256;
const CLK_GFXCLK: usize = 1;
const TEMPERATURE_EDGE: usize = 8;
const INFO_ACTIVITY_GFX: usize = 19;
const ASIC_POWER: usize = 23;
const TEMPERATURE_HOTSPOT: usize = 27;
const TEMPERATURE_GFX: usize = 28;
const BOARD_POWER: usize = 73;

/// `ADLSingleSensorData`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SensorData {
    supported: i32,
    value: i32,
}

/// `ADLPMLogDataOutput`.
#[repr(C)]
struct LogData {
    size: i32,
    sensors: [SensorData; MAX_SENSORS],
}

type Malloc = unsafe extern "system" fn(i32) -> *mut c_void;
type CreateFn = unsafe extern "system" fn(Malloc, i32, *mut *mut c_void) -> i32;
type DestroyFn = unsafe extern "system" fn(*mut c_void) -> i32;
type CountFn = unsafe extern "system" fn(*mut c_void, *mut i32) -> i32;
type QueryFn = unsafe extern "system" fn(*mut c_void, i32, *mut LogData) -> i32;

/// ADL hands buffers it allocates back to the caller (not used here, but
/// the callback is mandatory).
unsafe extern "system" fn adl_malloc(size: i32) -> *mut c_void {
    unsafe extern "C" {
        fn malloc(size: usize) -> *mut c_void;
    }
    // SAFETY: plain C allocation, size checked.
    unsafe { malloc(size.max(1) as usize) }
}

/// What the card reports.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct AmdReading {
    pub temp: Option<f32>,
    /// `0..=1`.
    pub load: Option<f32>,
    pub power: Option<f32>,
    pub clock: Option<f32>,
}

/// Picks the values from the sensor table (plain code, tested).
fn reading(sensors: &[SensorData; MAX_SENSORS]) -> AmdReading {
    let get = |i: usize| {
        let s = sensors[i];
        (s.supported != 0).then_some(s.value as f32)
    };
    let positive = |v: Option<f32>, max: f32| v.filter(|v| *v > 0.0 && *v <= max);
    AmdReading {
        // Edge (the "GPU" temperature of AMD's software); RDNA 3 reports it
        // as "GFX".
        temp: positive(get(TEMPERATURE_EDGE), 150.0)
            .or(positive(get(TEMPERATURE_GFX), 150.0))
            .or(positive(get(TEMPERATURE_HOTSPOT), 150.0)),
        load: get(INFO_ACTIVITY_GFX)
            .filter(|v| (0.0..=100.0).contains(v))
            .map(|v| v / 100.0),
        power: positive(get(ASIC_POWER), 2000.0).or(positive(get(BOARD_POWER), 2000.0)),
        clock: positive(get(CLK_GFXCLK), 10_000.0),
    }
}

pub(crate) struct AmdGpu {
    _lib: HMODULE,
    context: *mut c_void,
    destroy: DestroyFn,
    query: QueryFn,
    /// ADL adapter index that answers (one per output of the card).
    adapter: i32,
}

// The context is only used from the sensor thread that created it.
unsafe impl Send for AmdGpu {}

impl AmdGpu {
    pub(crate) fn open() -> Option<Self> {
        // SAFETY: the signatures are those of the ADL SDK (adl_sdk.h);
        // every pointer passed is valid for the call.
        unsafe {
            let lib = LoadLibraryW(w!("atiadlxx.dll")).ok()?;
            let create: CreateFn =
                std::mem::transmute(GetProcAddress(lib, s!("ADL2_Main_Control_Create"))?);
            let destroy: DestroyFn =
                std::mem::transmute(GetProcAddress(lib, s!("ADL2_Main_Control_Destroy"))?);
            let count: CountFn = std::mem::transmute(GetProcAddress(
                lib,
                s!("ADL2_Adapter_NumberOfAdapters_Get"),
            )?);
            let query: QueryFn =
                std::mem::transmute(GetProcAddress(lib, s!("ADL2_New_QueryPMLogData_Get"))?);
            let mut context = std::ptr::null_mut();
            if create(adl_malloc, 1, &mut context) != 0 || context.is_null() {
                tracing::info!("AMD ADL present but could not start");
                return None;
            }
            let mut n = 0i32;
            if count(context, &mut n) != 0 {
                n = 0;
            }
            let mut me = Self {
                _lib: lib,
                context,
                destroy,
                query,
                adapter: -1,
            };
            for i in 0..n.clamp(0, 64) {
                me.adapter = i;
                if me
                    .read()
                    .is_some_and(|r| r.temp.is_some() || r.load.is_some())
                {
                    tracing::info!(adapter = i, "AMD GPU sensors through ADL");
                    return Some(me);
                }
            }
            tracing::info!(adapters = n, "AMD ADL: no adapter reports sensors");
            None
        }
    }

    pub(crate) fn read(&self) -> Option<AmdReading> {
        let mut data = Box::new(LogData {
            size: 0,
            sensors: [SensorData::default(); MAX_SENSORS],
        });
        // SAFETY: valid context and output buffer of the documented layout.
        let r = unsafe { (self.query)(self.context, self.adapter, &mut *data) };
        (r == 0).then(|| reading(&data.sensors))
    }
}

impl Drop for AmdGpu {
    fn drop(&mut self) {
        // SAFETY: the context was created by `open`.
        unsafe { (self.destroy)(self.context) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_adl_sdk() {
        assert_eq!(std::mem::size_of::<LogData>(), 4 + 8 * MAX_SENSORS);
    }

    #[test]
    fn values_from_the_sensor_table() {
        let mut t = [SensorData::default(); MAX_SENSORS];
        t[TEMPERATURE_GFX] = SensorData {
            supported: 1,
            value: 54,
        };
        t[TEMPERATURE_HOTSPOT] = SensorData {
            supported: 1,
            value: 71,
        };
        t[INFO_ACTIVITY_GFX] = SensorData {
            supported: 1,
            value: 37,
        };
        t[ASIC_POWER] = SensorData {
            supported: 1,
            value: 141,
        };
        t[CLK_GFXCLK] = SensorData {
            supported: 1,
            value: 2405,
        };
        let r = reading(&t);
        // No edge sensor (RDNA 3): GFX.
        assert_eq!(r.temp, Some(54.0));
        assert!((r.load.unwrap() - 0.37).abs() < 1e-6);
        assert_eq!(r.power, Some(141.0));
        assert_eq!(r.clock, Some(2405.0));
        t[TEMPERATURE_EDGE] = SensorData {
            supported: 1,
            value: 52,
        };
        assert_eq!(reading(&t).temp, Some(52.0));
        assert_eq!(
            reading(&[SensorData::default(); MAX_SENSORS]),
            AmdReading::default()
        );
    }

    #[test]
    fn opening_is_harmless_without_an_amd_card() {
        if let Some(g) = AmdGpu::open() {
            let _ = g.read();
        }
    }
}
