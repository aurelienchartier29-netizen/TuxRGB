//! PawnIO (Windows): the signed driver that runs small signed modules
//! giving access to a few hardware registers. TuxRGB uses its official
//! modules for the CPU temperature (`AMDFamily17`, `IntelMSR`) and for the
//! fan controllers of the motherboard (`LpcIO`), like LibreHardwareMonitor.
//!
//! The device needs administrator rights.

use windows::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
use windows::core::w;

const DEVICE_TYPE: u32 = 41394 << 16;
const IOCTL_LOAD_BINARY: u32 = DEVICE_TYPE | (0x821 << 2);
const IOCTL_EXECUTE: u32 = DEVICE_TYPE | (0x841 << 2);
const NAME_LEN: usize = 32;

/// The official modules TuxRGB ships (see `assets/pawnio/LISEZ-MOI.txt`).
pub static AMD_FAMILY_17: &[u8] = include_bytes!("../assets/pawnio/AMDFamily17.bin");
pub static INTEL_MSR: &[u8] = include_bytes!("../assets/pawnio/IntelMSR.bin");
pub static LPC_IO: &[u8] = include_bytes!("../assets/pawnio/LpcIO.bin");

/// One PawnIO handle with its module loaded.
pub struct PawnIo(HANDLE);

// The handle is a plain kernel handle, used from one thread at a time.
unsafe impl Send for PawnIo {}

impl PawnIo {
    /// `None` without PawnIO, without administrator rights, or when the
    /// driver refuses the module.
    pub fn load(blob: &[u8]) -> Option<Self> {
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
            .map_err(|e| {
                tracing::info!("PawnIO unavailable ({e}): needs PawnIO and administrator mode")
            })
            .ok()?;
            let m = Self(h);
            // Without OVERLAPPED, the byte count pointer is mandatory.
            let mut got = 0u32;
            DeviceIoControl(
                h,
                IOCTL_LOAD_BINARY,
                Some(blob.as_ptr().cast()),
                blob.len() as u32,
                None,
                0,
                Some(&mut got),
                None,
            )
            .map_err(|e| tracing::info!("PawnIO refused its module: {e}"))
            .ok()?;
            Some(m)
        }
    }

    /// Runs `name(input…)` in the module and returns its `out_len` results.
    pub fn execute(&self, name: &str, input: &[u64], out_len: usize) -> Option<Vec<u64>> {
        let mut buf = vec![0u8; NAME_LEN + 8 * input.len()];
        let n = name.len().min(NAME_LEN - 1);
        buf[..n].copy_from_slice(&name.as_bytes()[..n]);
        for (i, v) in input.iter().enumerate() {
            buf[NAME_LEN + 8 * i..NAME_LEN + 8 * (i + 1)].copy_from_slice(&v.to_le_bytes());
        }
        let mut out = vec![0u8; 8 * out_len.max(1)];
        let mut got = 0u32;
        // SAFETY: valid buffers for the synchronous call.
        unsafe {
            DeviceIoControl(
                self.0,
                IOCTL_EXECUTE,
                Some(buf.as_ptr().cast()),
                buf.len() as u32,
                Some(out.as_mut_ptr().cast()),
                (8 * out_len) as u32,
                Some(&mut got),
                None,
            )
            .ok()?;
        }
        if (got as usize) < 8 * out_len {
            return None;
        }
        Some(
            out.chunks_exact(8)
                .take(out_len)
                .map(|c| u64::from_le_bytes(c.try_into().expect("8 bytes")))
                .collect(),
        )
    }

    /// One argument, one result.
    pub fn call(&self, name: &str, arg: u64) -> Option<u64> {
        self.execute(name, &[arg], 1)?.first().copied()
    }
}

impl Drop for PawnIo {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `load`.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// The ISA bus lock shared by the monitoring tools of Windows
/// (LibreHardwareMonitor, HWiNFO, AIDA64, OpenRGB…): held around every
/// access to a Super I/O chip so that two programs never interleave their
/// register sequences.
pub struct IsaBus(HANDLE);

unsafe impl Send for IsaBus {}

impl IsaBus {
    pub fn open() -> Option<Self> {
        // SAFETY: named mutex, closed on drop.
        unsafe { CreateMutexW(None, false, w!("Global\\Access_ISABUS.HTP.Method")) }
            .ok()
            .map(Self)
    }

    /// Runs `f` with the bus held (`None` if another program kept it more
    /// than `timeout_ms`).
    pub fn with<T>(&self, timeout_ms: u32, f: impl FnOnce() -> T) -> Option<T> {
        // SAFETY: valid mutex handle; released right after `f`.
        let r = unsafe { WaitForSingleObject(self.0, timeout_ms) };
        if r != WAIT_OBJECT_0 && r != WAIT_ABANDONED {
            return None;
        }
        let out = f();
        // SAFETY: owned by this thread since the wait above.
        unsafe {
            let _ = ReleaseMutex(self.0);
        }
        Some(out)
    }
}

impl Drop for IsaBus {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `open`.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
