//! Direct access to an ITE environment controller (address port base + 5,
//! data port base + 6) through `/dev/port`, for the fan service only (root).
//!
//! Used for one register per fan output – the PWM control register of the
//! IT8689E, which the Linux `it87` driver sets wrong in manual mode – and
//! only after the chip has answered with ITE's vendor id. The kernel refuses
//! `/dev/port` when it is locked down (Secure Boot): the fan is then
//! reported as refused, nothing else changes.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;

/// Register access, faked in the tests.
pub trait Ports: Send {
    fn read(&mut self, base: u16, reg: u8) -> Option<u8>;
    fn write(&mut self, base: u16, reg: u8, value: u8) -> bool;
}

/// ITE vendor id register and its values.
const VENDOR_REG: u8 = 0x58;

/// The chip at `base` is an ITE environment controller.
pub fn is_ite(p: &mut dyn Ports, base: u16) -> bool {
    matches!(p.read(base, VENDOR_REG), Some(0x90 | 0x7F))
}

/// The same value read twice (the `it87` driver may use the chip at the
/// same time: a read caught in between is not trusted).
pub fn read_stable(p: &mut dyn Ports, base: u16, reg: u8) -> Option<u8> {
    let a = p.read(base, reg)?;
    let b = p.read(base, reg)?;
    (a == b).then_some(a)
}

/// `/dev/port`, opened on first use.
#[derive(Default)]
pub struct DevPort {
    file: Option<File>,
}

impl DevPort {
    fn file(&mut self) -> Option<&File> {
        if self.file.is_none() {
            self.file = OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/port")
                .map_err(|e| eprintln!("tuxrgb-fand: /dev/port: {e}"))
                .ok();
        }
        self.file.as_ref()
    }
}

impl Ports for DevPort {
    fn read(&mut self, base: u16, reg: u8) -> Option<u8> {
        let f = self.file()?;
        f.write_at(&[reg], u64::from(base) + 5).ok()?;
        let mut b = [0u8];
        f.read_at(&mut b, u64::from(base) + 6).ok()?;
        Some(b[0])
    }

    fn write(&mut self, base: u16, reg: u8, value: u8) -> bool {
        let Some(f) = self.file() else {
            return false;
        };
        f.write_at(&[reg], u64::from(base) + 5).is_ok()
            && f.write_at(&[value], u64::from(base) + 6).is_ok()
    }
}

#[cfg(test)]
pub mod fake {
    use super::Ports;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// Registers of fake chips, shared with the test.
    #[derive(Clone, Default)]
    pub struct FakePorts(pub Arc<Mutex<HashMap<(u16, u8), u8>>>);

    impl FakePorts {
        pub fn ite(base: u16) -> Self {
            let p = Self::default();
            p.set(base, 0x58, 0x90);
            p
        }
        pub fn set(&self, base: u16, reg: u8, v: u8) {
            self.0.lock().unwrap().insert((base, reg), v);
        }
        pub fn get(&self, base: u16, reg: u8) -> Option<u8> {
            self.0.lock().unwrap().get(&(base, reg)).copied()
        }
    }

    impl Ports for FakePorts {
        fn read(&mut self, base: u16, reg: u8) -> Option<u8> {
            self.get(base, reg)
        }
        fn write(&mut self, base: u16, reg: u8, value: u8) -> bool {
            self.set(base, reg, value);
            true
        }
    }
}
