// SPDX-License-Identifier: MPL-2.0
// This file is a Rust port of parts of LibreHardwareMonitor
// (Hardware/Motherboard/Lpc/IT87XX.cs, Nct677X.cs, LpcIO.cs, LpcPort.cs),
// Copyright (C) LibreHardwareMonitor and Contributors, partial Copyright (C)
// Michael Möller and Contributors, under the Mozilla Public License 2.0
// (licenses/MPL-2.0.txt), which allows its distribution in TuxRGB (GPL).

//! Fan controllers of the motherboard on Windows: the "Super I/O" chips of
//! ITE (Gigabyte, some ASRock / Biostar boards) and Nuvoton (ASUS, MSI,
//! ASRock), reached through PawnIO's official `LpcIO` module.
//!
//! The register maps and the take-over / give-back sequences are those of
//! LibreHardwareMonitor (`IT87XX.cs`, `Nct677X.cs`, `LpcIO.cs`, MPL-2.0),
//! themselves checked against the Linux `it87` and `nct6775` drivers. Only
//! the chips whose control LibreHardwareMonitor documents are driven; the
//! second ITE chip of Gigabyte boards (port 0x4E), whose fans a firmware
//! controller also drives, is only read.
//!
//! Every access holds the ISA bus lock shared with the other tools.

use crate::pawnio::{IsaBus, LPC_IO, PawnIo};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Ite,
    Nuvoton,
}

/// What the take-over of one fan changed (to give it back).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Saved {
    /// PWM control register, its bit in the main control register (fans
    /// 1–3), extended PWM register.
    Ite {
        ctrl: u8,
        main_bit: Option<bool>,
        ext: u8,
    },
    /// Mode register, PWM command register.
    Nuvoton { mode: u8, command: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ite {
    It8613,
    It8620,
    It8625,
    It8628,
    It8631,
    It8638,
    It8655,
    It8665,
    It8686,
    It8688,
    It8689,
    It8696,
    It8705,
    It8712,
    It8716,
    It8718,
    It8720,
    It8721,
    It8726,
    It8728,
    It8771,
    It8772,
    It8792,
    It87952,
}

impl Ite {
    fn from_id(id: u16) -> Option<(Self, &'static str)> {
        Some(match id {
            0x8613 => (Self::It8613, "IT8613E"),
            0x8620 => (Self::It8620, "IT8620E"),
            0x8625 => (Self::It8625, "IT8625E"),
            0x8628 => (Self::It8628, "IT8628E"),
            0x8631 => (Self::It8631, "IT8631E"),
            0x8638 => (Self::It8638, "IT8638E"),
            0x8655 => (Self::It8655, "IT8655E"),
            0x8665 => (Self::It8665, "IT8665E"),
            0x8686 => (Self::It8686, "IT8686E"),
            0x8688 => (Self::It8688, "IT8688E"),
            0x8689 => (Self::It8689, "IT8689E"),
            0x8696 => (Self::It8696, "IT8696E"),
            0x8705 => (Self::It8705, "IT8705F"),
            0x8712 => (Self::It8712, "IT8712F"),
            0x8716 => (Self::It8716, "IT8716F"),
            0x8718 => (Self::It8718, "IT8718F"),
            0x8720 => (Self::It8720, "IT8720F"),
            0x8721 => (Self::It8721, "IT8721F"),
            0x8726 => (Self::It8726, "IT8726F"),
            0x8728 => (Self::It8728, "IT8728F"),
            0x8771 => (Self::It8771, "IT8771E"),
            0x8772 => (Self::It8772, "IT8772E"),
            0x8733 => (Self::It8792, "IT8792E"),
            0x8695 => (Self::It87952, "IT87952E"),
            _ => return None,
        })
    }

    /// (temperatures, fans, controls)
    fn counts(self) -> (usize, usize, usize) {
        match self {
            Self::It8613 => (4, 5, 4),
            Self::It8625 => (3, 6, 6),
            Self::It8628 => (6, 6, 6),
            Self::It8631 | Self::It8638 => (2, 2, 2),
            Self::It8665 => (6, 6, 6),
            Self::It8686 => (7, 6, 5),
            Self::It8688 => (6, 6, 5),
            Self::It8689 | Self::It8696 => (6, 6, 6),
            Self::It87952 | Self::It8792 | Self::It8705 => (3, 3, 3),
            Self::It8655 => (6, 3, 3),
            Self::It8620 => (3, 5, 5),
            _ => (3, 5, 3),
        }
    }

    fn has_ext_reg(self) -> bool {
        matches!(
            self,
            Self::It8721
                | Self::It8728
                | Self::It8665
                | Self::It8686
                | Self::It8688
                | Self::It8689
                | Self::It87952
                | Self::It8628
                | Self::It8625
                | Self::It8620
                | Self::It8613
                | Self::It8792
                | Self::It8655
                | Self::It8631
                | Self::It8638
                | Self::It8696
                | Self::It8772
        )
    }

    fn pwm_ctrl_regs(self) -> &'static [u8] {
        match self {
            Self::It8665 | Self::It8625 => &[0x15, 0x16, 0x17, 0x1e, 0x1f, 0x92],
            Self::It8792 => &[0x15, 0x16, 0x17],
            _ => &[0x15, 0x16, 0x17, 0x7f, 0xa7, 0xaf],
        }
    }

    fn alt_6th_fan(self) -> bool {
        matches!(self, Self::It8665 | Self::It8625)
    }

    fn bank_select(self) -> bool {
        matches!(self, Self::It8665 | Self::It8655)
    }
}

const ITE_ADDRESS: u16 = 0x05;
const ITE_DATA: u16 = 0x06;
const ITE_VENDOR_ID: u8 = 0x58;
const ITE_CONFIGURATION: u8 = 0x00;
const ITE_BANK: u8 = 0x06;
const ITE_FAN_16BIT: u8 = 0x0C;
const ITE_FAN_16BIT_ALT: u8 = 0x0B;
const ITE_MAIN_CTRL: u8 = 0x13;
const ITE_TEMP_BASE: u8 = 0x29;
const ITE_PWM_EXT: [u8; 6] = [0x63, 0x6b, 0x73, 0x7b, 0xa3, 0xab];
const ITE_TACH: [u8; 6] = [0x0d, 0x0e, 0x0f, 0x80, 0x82, 0x4c];
const ITE_TACH_EXT: [u8; 6] = [0x18, 0x19, 0x1a, 0x81, 0x83, 0x4d];
const ITE_TACH_ALT: [u8; 6] = [0x0d, 0x0e, 0x0f, 0x80, 0x82, 0x93];
const ITE_TACH_EXT_ALT: [u8; 6] = [0x18, 0x19, 0x1a, 0x81, 0x83, 0x94];

const NCT_BANK_SELECT: u8 = 0x4E;
const NCT_VENDOR: u16 = 0x5CA3;
const NCT_FAN_COUNT: [u16; 7] = [0x4B0, 0x4B2, 0x4B4, 0x4B6, 0x4B8, 0x4BA, 0x4CC];
const NCT_PWM_COMMAND: [u16; 7] = [0x109, 0x209, 0x309, 0x809, 0x909, 0xA09, 0xB09];
const NCT_CONTROL_MODE: [u16; 7] = [0x102, 0x202, 0x302, 0x802, 0x902, 0xA02, 0xB02];
const NCT_PWM_OUT_A: [u16; 7] = [0x001, 0x003, 0x011, 0x013, 0x015, 0x017, 0x029];
const NCT_PWM_OUT_B: [u16; 7] = [0x001, 0x003, 0x011, 0x013, 0x015, 0xA09, 0xB09];

/// Nuvoton chips whose control LibreHardwareMonitor documents: (name,
/// fans, newer PWM output map).
fn nuvoton(id: u8, revision: u8) -> Option<(&'static str, usize, bool)> {
    Some(match (id, revision) {
        (0xC8, 0x03) => ("NCT6791D", 6, false),
        (0xC9, 0x11) => ("NCT6792D", 6, false),
        (0xC9, 0x13) => ("NCT6792D-A", 6, false),
        (0xD1, 0x21) => ("NCT6793D", 6, false),
        (0xD3, 0x52) => ("NCT6795D", 6, false),
        (0xD4, 0x23) => ("NCT6796D", 6, false),
        (0xD4, 0x2A) => ("NCT6796D-R", 7, false),
        (0xD4, 0x51) => ("NCT6797D", 7, true),
        (0xD4, 0x2B) => ("NCT6798D", 7, true),
        (0xD8, 0x02) => ("NCT6799D", 7, true),
        _ => return None,
    })
}

enum Kind {
    Ite {
        chip: Ite,
        disabled: [bool; 6],
        sixteen_bit: bool,
    },
    Nuvoton {
        fans: usize,
        newer: bool,
    },
}

/// One fan controller chip.
pub struct SuperIo {
    io: PawnIo,
    bus: IsaBus,
    kind: Kind,
    /// Register port (0x2E / 0x4E).
    pub port: u16,
    /// Environment controller / hardware monitor base address.
    base: u16,
    pub name: &'static str,
    pub family: Family,
    /// Fans read only (see the module notes).
    pub read_only: bool,
    saved: Vec<Option<Saved>>,
}

/// Raw port access through the module (bus held by the caller).
fn inb(io: &PawnIo, port: u16) -> Option<u8> {
    io.call("ioctl_pio_inb", u64::from(port)).map(|v| v as u8)
}

fn outb(io: &PawnIo, port: u16, value: u8) -> Option<()> {
    io.execute("ioctl_pio_outb", &[u64::from(port), u64::from(value)], 0)
        .map(|_| ())
}

fn sio_inb(io: &PawnIo, reg: u8) -> Option<u8> {
    io.call("ioctl_superio_inb", u64::from(reg))
        .map(|v| v as u8)
}

fn sio_inw(io: &PawnIo, reg: u8) -> Option<u16> {
    io.call("ioctl_superio_inw", u64::from(reg))
        .map(|v| v as u16)
}

fn sio_outb(io: &PawnIo, reg: u8, value: u8) -> Option<()> {
    io.execute("ioctl_superio_outb", &[u64::from(reg), u64::from(value)], 0)
        .map(|_| ())
}

fn valid_base(a: u16) -> bool {
    a >= 0x100 && a & 0xF007 == 0
}

/// Every supported chip of the board (none without PawnIO or rights).
pub fn detect() -> Vec<SuperIo> {
    let Some(bus) = IsaBus::open() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (slot, port) in [(0u64, 0x2Eu16), (1, 0x4E)] {
        let Some(io) = PawnIo::load(LPC_IO) else {
            return out;
        };
        let found = bus.with(500, || {
            io.execute("ioctl_select_slot", &[slot], 0)?;
            detect_nuvoton(&io, port).or_else(|| detect_ite(&io, port))
        });
        let Some(Some((kind, base, name, family))) = found else {
            continue;
        };
        let mut chip = SuperIo {
            io,
            bus: match IsaBus::open() {
                Some(b) => b,
                None => continue,
            },
            kind,
            port,
            base,
            name,
            family,
            read_only: family == Family::Ite && port == 0x4E,
            saved: Vec::new(),
        };
        if chip.init() {
            tracing::info!(chip = name, port, base, "fan controller found");
            out.push(chip);
        }
    }
    out
}

fn detect_nuvoton(io: &PawnIo, port: u16) -> Option<(Kind, u16, &'static str, Family)> {
    outb(io, port, 0x87)?;
    outb(io, port, 0x87)?;
    let id = sio_inb(io, 0x20)?;
    let revision = sio_inb(io, 0x21)?;
    let Some((name, fans, newer)) = nuvoton(id, revision) else {
        if id != 0 && id != 0xFF {
            let _ = outb(io, port, 0xAA);
        }
        return None;
    };
    io.execute("ioctl_find_bars", &[], 0)?;
    sio_outb(io, 0x07, 0x0B)?;
    let base = sio_inw(io, 0x60)?;
    std::thread::sleep(Duration::from_millis(1));
    let verify = sio_inw(io, 0x60)?;
    if base == verify {
        // The hardware monitor I/O space lock of NCT679x chips.
        let options = sio_inb(io, 0x28)?;
        if options & 0x10 != 0 {
            sio_outb(io, 0x28, options & !0x10)?;
        }
    }
    outb(io, port, 0xAA)?;
    if base != verify {
        return None;
    }
    let base = if base & 0x07 == 0x05 {
        base & 0xFFF8
    } else {
        base
    };
    valid_base(base).then_some((Kind::Nuvoton { fans, newer }, base, name, Family::Nuvoton))
}

fn detect_ite(io: &PawnIo, port: u16) -> Option<(Kind, u16, &'static str, Family)> {
    // The secondary chip may already be in configuration mode (its ID reads
    // without entering): it must then be left as it is.
    let mut id = if port == 0x4E {
        sio_inw(io, 0x20).unwrap_or(0xFFFF)
    } else {
        0xFFFF
    };
    if id == 0xFFFF {
        outb(io, port, 0x87)?;
        outb(io, port, 0x01)?;
        outb(io, port, 0x55)?;
        outb(io, port, if port == 0x4E { 0xAA } else { 0x55 })?;
        id = sio_inw(io, 0x20)?;
    }
    let exit = |io: &PawnIo| {
        if port != 0x4E {
            let _ = outb(io, port, 0x02);
            let _ = outb(io, port + 1, 0x02);
        }
    };
    let Some((chip, name)) = Ite::from_id(id) else {
        if id != 0 && id != 0xFFFF {
            exit(io);
        }
        return None;
    };
    io.execute("ioctl_find_bars", &[], 0)?;
    sio_outb(io, 0x07, 0x04)?;
    let base = sio_inw(io, 0x60)?;
    std::thread::sleep(Duration::from_millis(1));
    let verify = sio_inw(io, 0x60)?;
    exit(io);
    (base == verify && valid_base(base)).then_some((
        Kind::Ite {
            chip,
            disabled: [false; 6],
            sixteen_bit: true,
        },
        base,
        name,
        Family::Ite,
    ))
}

impl SuperIo {
    fn locked<T>(&self, f: impl FnOnce(&Self) -> Option<T>) -> Option<T> {
        self.bus.with(100, || f(self)).flatten()
    }

    // ITE environment controller: address / data ports.
    fn ite_read(&self, reg: u8) -> Option<u8> {
        let a = self.base + ITE_ADDRESS;
        outb(&self.io, a, reg)?;
        let v = inb(&self.io, self.base + ITE_DATA)?;
        let echo = inb(&self.io, a)?;
        let it8688 = matches!(
            self.kind,
            Kind::Ite {
                chip: Ite::It8688,
                ..
            }
        );
        (echo == reg || it8688).then_some(v)
    }

    fn ite_write(&self, reg: u8, value: u8) -> Option<()> {
        let a = self.base + ITE_ADDRESS;
        outb(&self.io, a, reg)?;
        outb(&self.io, self.base + ITE_DATA, value)?;
        inb(&self.io, a).map(|_| ())
    }

    // Nuvoton hardware monitor: banked registers.
    fn nct_read(&self, address: u16) -> Option<u8> {
        let (a, d) = (self.base + 0x05, self.base + 0x06);
        outb(&self.io, a, NCT_BANK_SELECT)?;
        outb(&self.io, d, (address >> 8) as u8)?;
        outb(&self.io, a, (address & 0xFF) as u8)?;
        inb(&self.io, d)
    }

    fn nct_write(&self, address: u16, value: u8) -> Option<()> {
        let (a, d) = (self.base + 0x05, self.base + 0x06);
        outb(&self.io, a, NCT_BANK_SELECT)?;
        outb(&self.io, d, (address >> 8) as u8)?;
        outb(&self.io, a, (address & 0xFF) as u8)?;
        outb(&self.io, d, value)
    }

    /// Identity checks and the chip's own configuration.
    fn init(&mut self) -> bool {
        let ok = self.locked(|c| match &c.kind {
            Kind::Ite { chip, .. } => {
                let vendor = c.ite_read(ITE_VENDOR_ID)?;
                if vendor != 0x90 && vendor != 0x7F {
                    return None;
                }
                let conf = c.ite_read(ITE_CONFIGURATION)?;
                if conf & 0x10 == 0 && !matches!(chip, Ite::It8655 | Ite::It8665) {
                    return None;
                }
                let (_, fans, _) = chip.counts();
                let modes = c.ite_read(ITE_FAN_16BIT)?;
                let mut disabled = [false; 6];
                if fans >= 5 {
                    disabled[3] = modes & (1 << 4) == 0;
                    disabled[4] = modes & (1 << 5) == 0;
                }
                if fans >= 6 {
                    disabled[5] = if *chip == Ite::It8665 {
                        c.ite_read(ITE_FAN_16BIT_ALT)
                            .is_some_and(|m| m & (1 << 3) == 0)
                    } else {
                        modes & (1 << 2) == 0
                    };
                }
                Some(disabled)
            }
            Kind::Nuvoton { .. } => {
                let v = (u16::from(c.nct_read(0x804F)?) << 8) | u16::from(c.nct_read(0x004F)?);
                (v == NCT_VENDOR).then_some([false; 6])
            }
        });
        match ok {
            Some(disabled) => {
                if let Kind::Ite {
                    disabled: d,
                    chip,
                    sixteen_bit,
                } = &mut self.kind
                {
                    *d = disabled;
                    // Older IT8705F / IT8712F revisions count on 8 bits:
                    // not handled, read as unknown.
                    *sixteen_bit = !matches!(chip, Ite::It8705 | Ite::It8712);
                }
                self.saved = vec![None; self.controls()];
                true
            }
            None => false,
        }
    }

    pub fn fans(&self) -> usize {
        match &self.kind {
            Kind::Ite { chip, .. } => chip.counts().1,
            Kind::Nuvoton { fans, .. } => *fans,
        }
    }

    pub fn controls(&self) -> usize {
        match &self.kind {
            Kind::Ite { chip, .. } => chip.counts().2.min(chip.pwm_ctrl_regs().len()),
            Kind::Nuvoton { fans, .. } => *fans,
        }
    }

    /// Readings of the chip's own temperature inputs, °C.
    pub fn temperatures(&self) -> Vec<Option<f32>> {
        match &self.kind {
            Kind::Ite { chip, .. } => {
                let n = chip.counts().0;
                self.locked(|c| {
                    if chip.bank_select() {
                        c.ite_select_bank0();
                    }
                    Some(
                        (0..n)
                            .map(|i| {
                                c.ite_read(ITE_TEMP_BASE + i as u8)
                                    .map(|v| v as i8)
                                    .filter(|v| *v > 0 && *v < i8::MAX)
                                    .map(f32::from)
                            })
                            .collect(),
                    )
                })
                .unwrap_or_default()
            }
            Kind::Nuvoton { .. } => Vec::new(),
        }
    }

    fn ite_select_bank0(&self) {
        if let Some(v) = self.ite_read(ITE_BANK) {
            let _ = self.ite_write(ITE_BANK, v & 0x9F);
        }
    }

    /// RPM of every fan input (`None`: not connected / unreadable).
    pub fn rpm(&self) -> Vec<Option<u32>> {
        let n = self.fans();
        self.locked(|c| {
            Some(match &c.kind {
                Kind::Ite {
                    chip,
                    disabled,
                    sixteen_bit,
                } => {
                    if chip.bank_select() {
                        c.ite_select_bank0();
                    }
                    (0..n)
                        .map(|i| {
                            if disabled[i] || !sixteen_bit {
                                return None;
                            }
                            let (lo, hi) = if chip.alt_6th_fan() {
                                (ITE_TACH_ALT[i], ITE_TACH_EXT_ALT[i])
                            } else {
                                (ITE_TACH[i], ITE_TACH_EXT[i])
                            };
                            let v = u32::from(c.ite_read(lo)?) | (u32::from(c.ite_read(hi)?) << 8);
                            if v > 0x3f {
                                Some(if v < 0xffff { 1_350_000 / (v * 2) } else { 0 })
                            } else {
                                None
                            }
                        })
                        .collect()
                }
                Kind::Nuvoton { .. } => (0..n)
                    .map(|i| {
                        let hi = u32::from(c.nct_read(NCT_FAN_COUNT[i])?);
                        let lo = u32::from(c.nct_read(NCT_FAN_COUNT[i] + 1)?);
                        let count = (hi << 5) | (lo & 0x1F);
                        if count >= 0x1FFF {
                            Some(0)
                        } else if count >= 0x15 {
                            Some(1_350_000 / count)
                        } else {
                            None
                        }
                    })
                    .collect(),
            })
        })
        .unwrap_or_default()
    }

    /// Output of every control, % (`None`: the hardware's automatic mode,
    /// value unknown).
    pub fn duty(&self) -> Vec<Option<f32>> {
        let n = self.controls();
        self.locked(|c| {
            Some(match &c.kind {
                Kind::Ite { chip, .. } => (0..n)
                    .map(|i| {
                        let ctrl = c.ite_read(chip.pwm_ctrl_regs()[i])?;
                        if ctrl & 0x80 != 0 {
                            None
                        } else if chip.has_ext_reg() {
                            c.ite_read(ITE_PWM_EXT[i])
                                .map(|v| f32::from(v) * 100.0 / 255.0)
                        } else {
                            Some(f32::from(ctrl & 0x7F) * 100.0 / 127.0)
                        }
                    })
                    .collect(),
                Kind::Nuvoton { newer, .. } => (0..n)
                    .map(|i| {
                        let reg = if *newer {
                            NCT_PWM_OUT_B[i]
                        } else {
                            NCT_PWM_OUT_A[i]
                        };
                        c.nct_read(reg).map(|v| f32::from(v) / 2.55)
                    })
                    .collect(),
            })
        })
        .unwrap_or_default()
    }

    /// Sets control `i` to `value` (0–255), taking it over first.
    pub fn set(&mut self, i: usize, value: u8) -> bool {
        if self.read_only || i >= self.controls() {
            return false;
        }
        if self.saved[i].is_none() {
            let saved = self.locked(|c| c.read_saved(i));
            let Some(saved) = saved else {
                return false;
            };
            self.saved[i] = Some(saved);
        }
        let initial = self.saved[i].expect("just saved");
        self.locked(|c| match (&c.kind, initial) {
            (Kind::Ite { chip, .. }, Saved::Ite { ctrl, main_bit, .. }) => {
                if main_bit == Some(false) {
                    let m = c.ite_read(ITE_MAIN_CTRL)?;
                    c.ite_write(ITE_MAIN_CTRL, m | (1 << i))?;
                }
                let reg = chip.pwm_ctrl_regs()[i];
                if chip.has_ext_reg() {
                    let mode = if *chip == Ite::It8689 {
                        0x7F
                    } else {
                        ctrl & 0x7F
                    };
                    c.ite_write(reg, mode)?;
                    c.ite_write(ITE_PWM_EXT[i], value)
                } else {
                    c.ite_write(reg, value >> 1)
                }
            }
            (Kind::Nuvoton { .. }, Saved::Nuvoton { .. }) => {
                c.nct_write(NCT_CONTROL_MODE[i], 0)?;
                c.nct_write(NCT_PWM_COMMAND[i], value)
            }
            _ => None,
        })
        .is_some()
    }

    fn read_saved(&self, i: usize) -> Option<Saved> {
        Some(match &self.kind {
            Kind::Ite { chip, .. } => Saved::Ite {
                ctrl: self.ite_read(chip.pwm_ctrl_regs()[i])?,
                main_bit: if i < 3 {
                    Some(self.ite_read(ITE_MAIN_CTRL)? & (1 << i) != 0)
                } else {
                    None
                },
                ext: if chip.has_ext_reg() {
                    self.ite_read(ITE_PWM_EXT[i])?
                } else {
                    0
                },
            },
            Kind::Nuvoton { .. } => Saved::Nuvoton {
                mode: self.nct_read(NCT_CONTROL_MODE[i])?,
                command: self.nct_read(NCT_PWM_COMMAND[i])?,
            },
        })
    }

    /// Gives control `i` back to the hardware, as it was.
    pub fn restore(&mut self, i: usize) {
        let Some(Some(saved)) = self.saved.get(i).copied() else {
            return;
        };
        if self.apply_saved(i, saved) {
            self.saved[i] = None;
        }
    }

    fn apply_saved(&self, i: usize, saved: Saved) -> bool {
        self.locked(|c| match (&c.kind, saved) {
            (
                Kind::Ite { chip, .. },
                Saved::Ite {
                    ctrl,
                    main_bit,
                    ext,
                },
            ) => {
                c.ite_write(chip.pwm_ctrl_regs()[i], ctrl)?;
                if let Some(bit) = main_bit {
                    let m = c.ite_read(ITE_MAIN_CTRL)?;
                    if (m & (1 << i) != 0) != bit {
                        c.ite_write(ITE_MAIN_CTRL, m ^ (1 << i))?;
                    }
                }
                if chip.has_ext_reg() {
                    c.ite_write(ITE_PWM_EXT[i], ext)?;
                }
                Some(())
            }
            (Kind::Nuvoton { .. }, Saved::Nuvoton { mode, command }) => {
                c.nct_write(NCT_CONTROL_MODE[i], mode)?;
                c.nct_write(NCT_PWM_COMMAND[i], command)
            }
            _ => None,
        })
        .is_some()
    }

    pub fn restore_all(&mut self) {
        for i in 0..self.saved.len() {
            self.restore(i);
        }
    }

    /// Controls taken over, with what to put back (for the crash guard).
    pub fn saved(&self) -> Vec<(usize, Saved)> {
        self.saved
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.map(|s| (i, s)))
            .collect()
    }

    /// Puts back a record written by another process (the crash guard).
    pub fn restore_record(&mut self, i: usize, saved: Saved) -> bool {
        i < self.controls() && self.apply_saved(i, saved)
    }
}

impl Drop for SuperIo {
    fn drop(&mut self) {
        self.restore_all();
    }
}

/// Text form of a take-over record (`<port> <index> ite <ctrl> <main> <ext>`
/// / `<port> <index> nct <mode> <command>`), for the crash guard.
pub fn format_saved(port: u16, i: usize, s: Saved) -> String {
    match s {
        Saved::Ite {
            ctrl,
            main_bit,
            ext,
        } => {
            let m = match main_bit {
                Some(true) => "1",
                Some(false) => "0",
                None => "-",
            };
            format!("{port} {i} ite {ctrl} {m} {ext}")
        }
        Saved::Nuvoton { mode, command } => format!("{port} {i} nct {mode} {command}"),
    }
}

pub fn parse_saved(line: &str) -> Option<(u16, usize, Saved)> {
    let w: Vec<&str> = line.split_ascii_whitespace().collect();
    let port: u16 = w.first()?.parse().ok()?;
    let i: usize = w.get(1)?.parse().ok().filter(|i| *i < 8)?;
    if port != 0x2E && port != 0x4E {
        return None;
    }
    let s = match (w.get(2)?, w.len()) {
        (&"ite", 6) => Saved::Ite {
            ctrl: w[3].parse().ok()?,
            main_bit: match w[4] {
                "1" => Some(true),
                "0" => Some(false),
                "-" => None,
                _ => return None,
            },
            ext: w[5].parse().ok()?,
        },
        (&"nct", 5) => Saved::Nuvoton {
            mode: w[3].parse().ok()?,
            command: w[4].parse().ok()?,
        },
        _ => return None,
    };
    Some((port, i, s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip() {
        for (port, i, s) in [
            (
                0x2E,
                0,
                Saved::Ite {
                    ctrl: 0x87,
                    main_bit: Some(false),
                    ext: 128,
                },
            ),
            (
                0x2E,
                4,
                Saved::Ite {
                    ctrl: 0x7F,
                    main_bit: None,
                    ext: 90,
                },
            ),
            (
                0x4E,
                2,
                Saved::Nuvoton {
                    mode: 5,
                    command: 200,
                },
            ),
        ] {
            assert_eq!(parse_saved(&format_saved(port, i, s)), Some((port, i, s)));
        }
        for bad in [
            "46 0 ite 1 0",
            "80 0 nct 1 2",
            "46 9 nct 1 2",
            "46 0 nct 1 x",
            "46 0 sh 1 2",
        ] {
            assert_eq!(parse_saved(bad), None, "{bad}");
        }
    }

    #[test]
    fn chip_tables() {
        assert_eq!(Ite::from_id(0x8689).map(|c| c.1), Some("IT8689E"));
        assert_eq!(Ite::from_id(0x8695).map(|c| c.1), Some("IT87952E"));
        assert_eq!(Ite::It8689.counts(), (6, 6, 6));
        assert!(Ite::It8689.has_ext_reg());
        assert_eq!(nuvoton(0xD4, 0x2B), Some(("NCT6798D", 7, true)));
        assert_eq!(nuvoton(0xD4, 0x99), None);
    }
}
