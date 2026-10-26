//! CPU temperature and package power on Windows through PawnIO, the signed
//! driver TuxRGB already installs for the motherboard and RAM lights.
//!
//! Windows gives no CPU temperature to programs: it sits in registers of
//! the processor that only a driver may read. PawnIO runs small signed
//! modules that allow just a few reads; TuxRGB loads the official ones
//! (`AMDFamily17` for Ryzen, `IntelMSR` for Intel Core, as
//! LibreHardwareMonitor does) and reads:
//! * AMD – `THM_TCON_CUR_TMP` (SMN 0x59800), like Linux `k10temp`;
//! * Intel – `IA32_PACKAGE_THERM_STATUS` against `IA32_TEMPERATURE_TARGET`,
//!   like Linux `coretemp`;
//! * both – the package energy counter (RAPL), for the power.
//!
//! The PawnIO device needs administrator rights: without the "mode
//! administrateur" (or without PawnIO) the values stay unknown.
//!
//! The arithmetic is plain code, tested on every platform; only the device
//! is Windows-only.

#![cfg_attr(not(windows), allow(dead_code))]

/// °C from `THM_TCON_CUR_TMP` (bits 31:21 in 0.125 °C; −49 °C when the
/// range bit 19 or both TJ_SEL bits 17:16 are set), minus the Tctl offset of
/// a few early Ryzen (`k10temp`).
pub(crate) fn amd_temperature(raw: u32, brand: &str) -> f32 {
    let mut t = (raw >> 21) as f32 * 0.125;
    if raw & 0x8_0000 != 0 || raw & 0x3_0000 == 0x3_0000 {
        t -= 49.0;
    }
    let offset = if ["1600X", "1700X", "1800X"]
        .iter()
        .any(|m| brand.contains(m))
    {
        20.0
    } else if brand.contains("Threadripper 19") || brand.contains("Threadripper 29") {
        27.0
    } else if brand.contains("2700X") {
        10.0
    } else {
        0.0
    };
    t - offset
}

/// °C from `IA32_PACKAGE_THERM_STATUS` (digital readout, bits 22:16, below
/// TjMax; bit 31 valid) and `IA32_TEMPERATURE_TARGET` (TjMax, bits 23:16).
pub(crate) fn intel_temperature(status: u64, target: u64) -> Option<f32> {
    if status & 0x8000_0000 == 0 {
        return None;
    }
    let tj_max = match ((target >> 16) & 0xFF) as f32 {
        t if t > 0.0 => t,
        _ => 100.0,
    };
    Some(tj_max - ((status >> 16) & 0x7F) as f32)
}

/// Joules of one unit of the energy counters (`RAPL_POWER_UNIT`, bits 12:8).
pub(crate) fn energy_unit(power_unit: u64) -> f64 {
    1.0 / f64::from(1u32 << ((power_unit >> 8) & 0x1F))
}

/// Watts between two readings of the 32-bit energy counter (wraps).
pub(crate) fn power(prev: u32, now: u32, unit: f64, seconds: f64) -> Option<f32> {
    if seconds <= 0.0 {
        return None;
    }
    let w = f64::from(now.wrapping_sub(prev)) * unit / seconds;
    (w.is_finite() && w < 2000.0).then_some(w as f32)
}

#[cfg(windows)]
pub(crate) use device::CpuSensors;

#[cfg(windows)]
mod device {
    use std::time::Instant;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::core::w;

    const DEVICE_TYPE: u32 = 41394 << 16;
    const IOCTL_LOAD_BINARY: u32 = DEVICE_TYPE | (0x821 << 2);
    const IOCTL_EXECUTE: u32 = DEVICE_TYPE | (0x841 << 2);
    const NAME_LEN: usize = 32;

    static AMD_MODULE: &[u8] = include_bytes!("../assets/pawnio/AMDFamily17.bin");
    static INTEL_MODULE: &[u8] = include_bytes!("../assets/pawnio/IntelMSR.bin");

    const THM_TCON_CUR_TMP: u64 = 0x0005_9800;
    const IA32_PACKAGE_THERM_STATUS: u64 = 0x1B1;
    const IA32_TEMPERATURE_TARGET: u64 = 0x1A2;
    const INTEL_RAPL_POWER_UNIT: u64 = 0x606;
    const INTEL_PKG_ENERGY_STATUS: u64 = 0x611;
    const AMD_PWR_UNIT: u64 = 0xC001_0299;
    const AMD_PKG_ENERGY_STAT: u64 = 0xC001_029B;

    #[derive(Clone, Copy, PartialEq)]
    enum Vendor {
        Amd,
        Intel,
    }

    /// One PawnIO handle with its module loaded.
    struct Module(HANDLE);

    impl Module {
        fn load(blob: &[u8]) -> Option<Self> {
            // SAFETY: plain Win32 calls; the handle is closed on drop.
            unsafe {
                let h = CreateFileW(
                    w!("\\\\?\\GLOBALROOT\\Device\\PawnIO"),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                )
                .ok()?;
                let m = Module(h);
                DeviceIoControl(
                    h,
                    IOCTL_LOAD_BINARY,
                    Some(blob.as_ptr().cast()),
                    blob.len() as u32,
                    None,
                    0,
                    None,
                    None,
                )
                .ok()?;
                Some(m)
            }
        }

        /// Runs `name(arg)` in the module; its single 64-bit result.
        fn call(&self, name: &str, arg: u64) -> Option<u64> {
            let mut input = [0u8; NAME_LEN + 8];
            let n = name.len().min(NAME_LEN - 1);
            input[..n].copy_from_slice(&name.as_bytes()[..n]);
            input[NAME_LEN..].copy_from_slice(&arg.to_le_bytes());
            let mut out = [0u8; 8];
            let mut got = 0u32;
            // SAFETY: valid buffers for the synchronous call.
            unsafe {
                DeviceIoControl(
                    self.0,
                    IOCTL_EXECUTE,
                    Some(input.as_ptr().cast()),
                    input.len() as u32,
                    Some(out.as_mut_ptr().cast()),
                    out.len() as u32,
                    Some(&mut got),
                    None,
                )
                .ok()?;
            }
            (got >= 8).then(|| u64::from_le_bytes(out))
        }
    }

    impl Drop for Module {
        fn drop(&mut self) {
            // SAFETY: the handle was opened by `load`.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn cpuid(leaf: u32) -> [u32; 4] {
        // SAFETY: CPUID exists on every x86-64 processor (a safe function
        // in recent Rust, unsafe in the oldest supported one).
        #[allow(unused_unsafe)]
        let r = unsafe { std::arch::x86_64::__cpuid(leaf) };
        [r.eax, r.ebx, r.ecx, r.edx]
    }

    fn vendor_and_brand() -> Option<(Vendor, String)> {
        let v = cpuid(0);
        let mut id = Vec::new();
        for r in [v[1], v[3], v[2]] {
            id.extend_from_slice(&r.to_le_bytes());
        }
        let family = {
            let a = cpuid(1)[0];
            ((a >> 8) & 0xF)
                + if (a >> 8) & 0xF == 0xF {
                    (a >> 20) & 0xFF
                } else {
                    0
                }
        };
        let vendor = match &id[..] {
            b"AuthenticAMD" | b"HygonGenuine" if family >= 0x17 => Vendor::Amd,
            b"GenuineIntel" if family == 6 => Vendor::Intel,
            _ => return None,
        };
        let mut brand = Vec::new();
        for leaf in 0x8000_0002..=0x8000_0004u32 {
            for r in cpuid(leaf) {
                brand.extend_from_slice(&r.to_le_bytes());
            }
        }
        let brand = String::from_utf8_lossy(&brand)
            .trim_matches(char::from(0))
            .trim()
            .to_owned();
        Some((vendor, brand))
    }

    /// The CPU sensors through PawnIO.
    pub(crate) struct CpuSensors {
        module: Module,
        vendor: Vendor,
        brand: String,
        tj_max: u64,
        unit: Option<f64>,
        energy: Option<(u32, Instant)>,
    }

    impl CpuSensors {
        /// `None` without PawnIO, without administrator rights, or on a
        /// processor without a module.
        pub(crate) fn open() -> Option<Self> {
            let (vendor, brand) = vendor_and_brand()?;
            let module = Module::load(match vendor {
                Vendor::Amd => AMD_MODULE,
                Vendor::Intel => INTEL_MODULE,
            })?;
            let (tj_max, unit) = match vendor {
                Vendor::Amd => (
                    0,
                    module
                        .call("ioctl_read_msr", AMD_PWR_UNIT)
                        .map(super::energy_unit),
                ),
                Vendor::Intel => (
                    module
                        .call("ioctl_read_msr", IA32_TEMPERATURE_TARGET)
                        .unwrap_or(0),
                    module
                        .call("ioctl_read_msr", INTEL_RAPL_POWER_UNIT)
                        .map(super::energy_unit),
                ),
            };
            let me = Self {
                module,
                vendor,
                brand,
                tj_max,
                unit,
                energy: None,
            };
            // A module that loads but reads nothing is of no use.
            me.temperature()?;
            tracing::info!(brand = %me.brand, "CPU sensors through PawnIO");
            Some(me)
        }

        pub(crate) fn temperature(&self) -> Option<f32> {
            let t = match self.vendor {
                Vendor::Amd => {
                    let raw = self.module.call("ioctl_read_smn", THM_TCON_CUR_TMP)?;
                    super::amd_temperature(raw as u32, &self.brand)
                }
                Vendor::Intel => {
                    let status = self
                        .module
                        .call("ioctl_read_msr", IA32_PACKAGE_THERM_STATUS)?;
                    super::intel_temperature(status, self.tj_max)?
                }
            };
            (1.0..150.0).contains(&t).then_some(t)
        }

        /// Package power since the previous call, W.
        pub(crate) fn power(&mut self) -> Option<f32> {
            let unit = self.unit?;
            let msr = match self.vendor {
                Vendor::Amd => AMD_PKG_ENERGY_STAT,
                Vendor::Intel => INTEL_PKG_ENERGY_STATUS,
            };
            let now = self.module.call("ioctl_read_msr", msr)? as u32;
            let at = Instant::now();
            let prev = self.energy.replace((now, at))?;
            super::power(prev.0, now, unit, at.duration_since(prev.1).as_secs_f64())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amd_readout_like_k10temp() {
        // 61.5 °C: 492 × 0.125.
        assert_eq!(
            amd_temperature(492 << 21, "AMD Ryzen 7 7800X3D 8-Core Processor"),
            61.5
        );
        // Range bit: −49 °C.
        assert_eq!(
            amd_temperature((880 << 21) | 0x8_0000, "AMD Ryzen 9 5950X"),
            61.0
        );
        // TJ_SEL both set: −49 °C too.
        assert_eq!(
            amd_temperature((880 << 21) | 0x3_0000, "AMD Ryzen 9 7950X"),
            61.0
        );
        // Tctl offset of the first Ryzen.
        assert_eq!(
            amd_temperature(560 << 21, "AMD Ryzen 7 1800X Eight-Core Processor"),
            50.0
        );
        assert_eq!(
            amd_temperature(480 << 21, "AMD Ryzen 7 2700X Eight-Core Processor"),
            50.0
        );
    }

    #[test]
    fn intel_readout_like_coretemp() {
        // TjMax 100, 37 below: 63 °C.
        assert_eq!(
            intel_temperature(0x8000_0000 | (37 << 16), 100 << 16),
            Some(63.0)
        );
        // TjMax unknown: 100.
        assert_eq!(intel_temperature(0x8000_0000 | (40 << 16), 0), Some(60.0));
        // Not valid.
        assert_eq!(intel_temperature(37 << 16, 100 << 16), None);
    }

    #[cfg(windows)]
    #[test]
    fn opening_is_harmless_without_the_driver() {
        // No PawnIO on a test machine (nor in Wine): no value, no crash.
        if let Some(mut c) = CpuSensors::open() {
            let _ = (c.temperature(), c.power());
        }
    }

    #[test]
    fn energy_counter_gives_watts() {
        // Unit 1/2^14 J (Intel, AMD default: 0x0E in bits 12:8).
        let unit = energy_unit(0x000A_0E03);
        assert_eq!(unit, 1.0 / 16384.0);
        // 65 W for one second.
        let w = power(1000, 1000 + 65 * 16384, unit, 1.0).unwrap();
        assert!((w - 65.0).abs() < 1e-3);
        // Wrap of the 32-bit counter.
        let w = power(u32::MAX - 16383, 16384, unit, 2.0).unwrap();
        assert!((w - 1.0).abs() < 1e-3, "{w}");
        assert_eq!(power(0, 1, unit, 0.0), None);
    }
}
