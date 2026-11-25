//! DeepCool digital displays (coolers and cases), over USB HID.
//!
//! The reports are those of deepcool-digital-linux (Nortank12, GPL-3.0),
//! re-implemented here: one 64-byte report per refresh, report number 16
//! (the "SE" variants of the AK series have no report number: every byte
//! moves one place to the left), the CH510 a line of text.

use super::hardware_display::{
    Capabilities, Family, Frame, HardwareDisplay, Metric, Model, Surface,
};
use super::hid::Transport;
use std::io;
use std::time::Duration;

const VID: u16 = 0x3633;
const CH510_VID: u16 = 0x34d3;
const CH510_PID: u16 = 0x1100;

/// The report family of a DeepCool display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// AK400 / AK500 / AK500S / AK620 DIGITAL (and SE): one value, a load
    /// bar, an alarm.
    Ak,
    /// LS520 / LS720 SE DIGITAL: temperature or power.
    Ls,
    /// AG400 / AG620 DIGITAL: two digits.
    Ag,
    /// LD240 / LD360: everything at once (power, temperature, load).
    Ld,
    /// LP240 / LP360: 14 × 14 dot matrix, one or two values.
    Lp,
    /// LQ240 / LQ360, Assassin IV VC Vision, AK… G2 / AK700 DIGITAL NYX:
    /// everything at once, with the clock.
    Lq,
    /// AK400 DIGITAL PRO: everything at once.
    AkPro,
    /// AK500 / AK620 DIGITAL PRO: everything at once, with the clock.
    AkPro2,
    /// CH560 DIGITAL, MORPHEUS, CH360 DIGITAL: CPU and GPU, temperature or
    /// load.
    Ch,
    /// CH170 / CH270 / CH690 DIGITAL: CPU or GPU page.
    ChGen2,
    /// CH510 MESH DIGITAL: CPU or GPU, as text.
    Ch510,
}

const fn model(pid: u16, name: &'static str, protocol: Protocol) -> Model {
    Model {
        vendor_id: VID,
        product_id: pid,
        vendor: "DeepCool",
        name,
        family: Family::DeepCool(protocol),
    }
}

pub const MODELS: &[Model] = &[
    model(1, "AK400 DIGITAL", Protocol::Ak),
    model(2, "AK620 DIGITAL", Protocol::Ak),
    model(3, "AK500 DIGITAL", Protocol::Ak),
    model(4, "AK500S DIGITAL", Protocol::Ak),
    model(5, "CH560 DIGITAL", Protocol::Ch),
    model(6, "LS520 / LS720 SE DIGITAL", Protocol::Ls),
    model(7, "MORPHEUS", Protocol::Ch),
    model(8, "AG400 / AG620 DIGITAL", Protocol::Ag),
    model(10, "LD240 / LD360", Protocol::Ld),
    model(12, "LP240 / LP360", Protocol::Lp),
    model(13, "LQ240 / LQ360", Protocol::Lq),
    model(15, "ASSASSIN IV VC VISION", Protocol::Lq),
    model(16, "AK400 DIGITAL PRO", Protocol::AkPro),
    model(17, "AK500 DIGITAL PRO", Protocol::AkPro2),
    model(18, "AK620 DIGITAL PRO", Protocol::AkPro2),
    model(19, "CH170 DIGITAL", Protocol::ChGen2),
    model(21, "CH360 DIGITAL", Protocol::Ch),
    model(22, "CH270 DIGITAL", Protocol::ChGen2),
    model(27, "CH690 DIGITAL", Protocol::ChGen2),
    model(31, "ASSASSIN IV VC VISION", Protocol::Lq),
    model(41, "AK620 G2 DIGITAL NYX", Protocol::Lq),
    model(42, "AK700 DIGITAL NYX", Protocol::Lq),
    model(43, "AK400 G2 DIGITAL NYX", Protocol::Lq),
    model(44, "AK500 G2 DIGITAL NYX", Protocol::Lq),
    Model {
        vendor_id: CH510_VID,
        product_id: CH510_PID,
        vendor: "DeepCool",
        name: "CH510 MESH DIGITAL",
        family: Family::DeepCool(Protocol::Ch510),
    },
];

/// Temperature above which the alarm blinks (°C, as the devices do).
const ALARM_C: f32 = 90.0;

pub fn capabilities(p: Protocol) -> Capabilities {
    use Metric::*;
    let (metrics, at_once, fixed, alarm, fahrenheit): (&'static [Metric], usize, bool, bool, bool) =
        match p {
            Protocol::Ak => (&[CpuTemp, CpuLoad], 1, false, true, true),
            Protocol::Ls => (&[CpuTemp, CpuPower], 1, false, true, true),
            Protocol::Ag => (&[CpuTemp, CpuLoad], 1, false, true, false),
            Protocol::Ld | Protocol::AkPro => (&[CpuTemp, CpuLoad, CpuPower], 3, true, false, true),
            Protocol::Lq | Protocol::AkPro2 => {
                (&[CpuTemp, CpuLoad, CpuPower, CpuFreq], 4, true, false, true)
            }
            Protocol::Lp => (
                &[CpuLoad, CpuTemp, CpuPower, GpuLoad, GpuTemp, GpuPower],
                2,
                false,
                false,
                true,
            ),
            Protocol::Ch => (&[CpuTemp, CpuLoad, GpuTemp, GpuLoad], 2, false, false, true),
            Protocol::ChGen2 => (
                &[
                    CpuTemp, CpuLoad, CpuPower, CpuFreq, GpuTemp, GpuLoad, GpuPower, GpuFreq,
                ],
                4,
                false,
                false,
                true,
            ),
            Protocol::Ch510 => (&[CpuTemp, CpuLoad, GpuTemp, GpuLoad], 2, false, false, true),
        };
    Capabilities {
        surface: if p == Protocol::Lp {
            Surface::Matrix
        } else {
            Surface::Digits
        },
        metrics,
        at_once,
        fixed,
        text: false,
        image: false,
        brightness: false,
        rotation: p == Protocol::Lp,
        builtin: None,
        alarm,
        fahrenheit,
        min_interval: Duration::from_millis(250),
        auto_start: true,
        note: None,
    }
}

/// Values of one refresh, rounded as the displays show them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Readings {
    /// °C (or °F when asked), whole degrees.
    pub cpu_temp: u16,
    pub cpu_load: u8,
    pub cpu_power: u16,
    pub cpu_freq: u16,
    pub gpu_temp: u16,
    pub gpu_load: u8,
    pub gpu_power: u16,
    pub gpu_freq: u16,
    pub fahrenheit: bool,
    /// The CPU is above the alarm temperature (and the alarm is on).
    pub alarm: bool,
}

impl Readings {
    pub fn of(frame: &Frame<'_>) -> Self {
        let s = frame.sample;
        let temp = |t: Option<f32>| {
            let c = t.unwrap_or(0.0).max(0.0);
            let v = if frame.fahrenheit {
                c * 9.0 / 5.0 + 32.0
            } else {
                c
            };
            v.round().min(999.0) as u16
        };
        let pct = |v: Option<f32>| (v.unwrap_or(0.0).clamp(0.0, 1.0) * 100.0).round() as u8;
        let num = |v: Option<f32>| v.unwrap_or(0.0).max(0.0).round().min(65_535.0) as u16;
        Self {
            cpu_temp: temp(s.cpu_temp),
            cpu_load: pct(s.cpu_load),
            cpu_power: num(s.cpu_power),
            cpu_freq: num(s.cpu_freq),
            gpu_temp: temp(s.gpu_temp),
            gpu_load: pct(s.gpu_load),
            gpu_power: num(s.gpu_power),
            gpu_freq: num(s.gpu_freq),
            fahrenheit: frame.fahrenheit,
            alarm: frame.settings.alarm && s.cpu_temp.is_some_and(|t| t >= ALARM_C),
        }
    }
}

/// 64-byte report, report number 16.
fn report() -> [u8; 64] {
    let mut d = [0u8; 64];
    d[0] = 16;
    d
}

/// Load bar of the AK / LS / CH displays: 1 to 10 segments.
fn bar(load: u8) -> u8 {
    if load < 15 {
        1
    } else {
        (load as f32 / 10.0).round() as u8
    }
}

fn digits3(d: &mut [u8], v: u16) {
    d[0] = (v / 100 % 10) as u8;
    d[1] = (v % 100 / 10) as u8;
    d[2] = (v % 10) as u8;
}

fn checksum(d: &[u8]) -> u8 {
    (d.iter().map(|&x| u32::from(x)).sum::<u32>() % 256) as u8
}

/// Unit byte of the first displays: °C, °F, % (or W on the LS).
fn unit(metric: Metric, fahrenheit: bool) -> u8 {
    if metric.is_temperature() {
        if fahrenheit { 35 } else { 19 }
    } else {
        76
    }
}

/// AK / LS series: one value (temperature, load or power), the load bar.
pub fn ak_report(metric: Metric, r: &Readings) -> [u8; 64] {
    let mut d = report();
    let value = match metric {
        Metric::CpuLoad => u16::from(r.cpu_load),
        Metric::CpuPower => r.cpu_power,
        _ => r.cpu_temp,
    };
    d[1] = unit(metric, r.fahrenheit);
    digits3(&mut d[3..6], value);
    d[2] = bar(r.cpu_load);
    d[6] = r.alarm as u8;
    d
}

/// The start of the AK / LS / CH displays.
pub fn init_report() -> [u8; 64] {
    let mut d = report();
    d[1] = 170;
    d
}

/// AG series: two digits (99 at most).
pub fn ag_report(metric: Metric, r: &Readings) -> [u8; 64] {
    let mut d = report();
    let (unit, v) = match metric {
        Metric::CpuLoad => (76, u16::from(r.cpu_load)),
        _ => (19, r.cpu_temp),
    };
    d[1] = unit;
    if v < 100 {
        d[3] = (v % 100 / 10) as u8;
        d[4] = (v % 10) as u8;
    } else {
        d[3] = 9;
        d[4] = 9;
    }
    d[5] = r.alarm as u8;
    d
}

/// LD series: start-up reports (leading zeros off).
pub fn ld_init() -> [[u8; 64]; 2] {
    let mut d = report();
    d[1] = 104;
    d[2] = 1;
    d[3] = 1;
    d[4] = 2;
    d[5] = 3;
    d[6] = 1;
    d[7] = 112;
    d[8] = 22;
    let first = d;
    d[5] = 2;
    d[6] = 0;
    d[7] = 110;
    [first, d]
}

/// Full status: power, temperature (f32), load (and clock), checksum, 22.
fn full_report(head: &[u8], r: &Readings, freq: bool) -> [u8; 64] {
    let mut d = report();
    d[1..1 + head.len()].copy_from_slice(head);
    let mut i = 1 + head.len();
    d[i..i + 2].copy_from_slice(&r.cpu_power.to_be_bytes());
    i += 2;
    d[i] = r.fahrenheit as u8;
    i += 1;
    d[i..i + 4].copy_from_slice(&(r.cpu_temp as f32).to_be_bytes());
    i += 4;
    d[i] = r.cpu_load;
    i += 1;
    if freq {
        d[i..i + 2].copy_from_slice(&r.cpu_freq.to_be_bytes());
        i += 2;
    }
    d[i] = checksum(&d[1..i]);
    d[i + 1] = 22;
    d
}

/// LD series.
pub fn ld_report(r: &Readings) -> [u8; 64] {
    full_report(&[104, 1, 1, 11, 1, 2, 5], r, false)
}

/// AK400 DIGITAL PRO.
pub fn ak_pro_report(r: &Readings) -> [u8; 64] {
    full_report(&[104, 1, 2, 11, 1, 2, 5], r, false)
}

/// AK500 / AK620 DIGITAL PRO.
pub fn ak_pro2_report(r: &Readings) -> [u8; 64] {
    full_report(&[104, 1, 4, 13, 1, 2, 8], r, true)
}

/// LQ series (and the NYX / Vision coolers).
pub fn lq_report(r: &Readings) -> [u8; 64] {
    full_report(&[104, 1, 8, 12, 1, 2], r, true)
}

/// CH560 / MORPHEUS / CH360: CPU (temperature or load) and GPU.
pub fn ch_report(cpu: Metric, gpu: Option<Metric>, r: &Readings) -> [u8; 64] {
    let mut d = report();
    let (u, v) = if cpu == Metric::CpuLoad {
        (76, u16::from(r.cpu_load))
    } else {
        (unit(Metric::CpuTemp, r.fahrenheit), r.cpu_temp)
    };
    d[1] = u;
    digits3(&mut d[3..6], v);
    if let Some(g) = gpu {
        let (u, v) = if g == Metric::GpuLoad {
            (76, u16::from(r.gpu_load))
        } else {
            (unit(Metric::GpuTemp, r.fahrenheit), r.gpu_temp)
        };
        d[6] = u;
        digits3(&mut d[8..11], v);
    }
    d[2] = bar(r.cpu_load);
    d[7] = bar(r.gpu_load);
    d
}

/// CH170 / CH270 / CH690: the CPU page or the GPU page.
pub fn ch_gen2_report(gpu: bool, r: &Readings) -> [u8; 64] {
    let mut d = report();
    d[1] = 104;
    d[2] = 1;
    d[3] = 6;
    d[4] = 35;
    d[5] = 1;
    d[9] = r.fahrenheit as u8;
    if gpu {
        d[6] = 4;
        d[19..21].copy_from_slice(&r.gpu_power.to_be_bytes());
        d[21..25].copy_from_slice(&(r.gpu_temp as f32).to_be_bytes());
        d[25] = r.gpu_load;
        d[26..28].copy_from_slice(&r.gpu_freq.to_be_bytes());
    } else {
        d[6] = 2;
        d[7..9].copy_from_slice(&r.cpu_power.to_be_bytes());
        d[10..14].copy_from_slice(&(r.cpu_temp as f32).to_be_bytes());
        d[14] = r.cpu_load;
        d[15..17].copy_from_slice(&r.cpu_freq.to_be_bytes());
    }
    d[40] = checksum(&d[1..40]);
    d[41] = 22;
    d
}

/// CH510 MESH DIGITAL: a line of text.
pub fn ch510_line(gpu: bool, r: &Readings) -> Vec<u8> {
    let (load, temp) = if gpu {
        (r.gpu_load, r.gpu_temp)
    } else {
        (r.cpu_load, r.cpu_temp)
    };
    format!(
        "HLXDATA({},{},0,0,{})\r\n",
        load,
        temp,
        if r.fahrenheit { "F" } else { "C" }
    )
    .into_bytes()
}

/// LP series: the 14 × 14 matrix with one value (centred) or two.
pub fn lp_report(values: &[(u16, MatrixUnit)], rotation: u16) -> [u8; 64] {
    let mut m = [[false; 14]; 14];
    match values {
        [one] => insert_value(&mut m, 5, *one),
        [a, b, ..] => {
            insert_value(&mut m, 1, *a);
            insert_value(&mut m, 8, *b);
        }
        [] => {}
    }
    let m = rotate(m, rotation);
    let mut d = report();
    d[1] = 104;
    d[2] = 1;
    d[3] = 5;
    d[4] = 29;
    d[5] = 1;
    d[6..34].copy_from_slice(&matrix_bytes(&m));
    d[34] = checksum(&d[1..34]);
    d[35] = 22;
    d
}

/// Unit drawn next to a value of the LP matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatrixUnit {
    Percent,
    Celsius,
    Fahrenheit,
    Watt,
}

impl MatrixUnit {
    fn pattern(self) -> [[bool; 5]; 5] {
        const T: bool = true;
        const F: bool = false;
        match self {
            Self::Percent => [
                [T, T, F, F, T],
                [T, T, F, T, F],
                [F, F, T, F, F],
                [F, T, F, T, T],
                [T, F, F, T, T],
            ],
            Self::Celsius => [
                [T, F, F, F, F],
                [F, F, T, T, F],
                [F, T, F, F, F],
                [F, T, F, F, F],
                [F, F, T, T, F],
            ],
            Self::Fahrenheit => [
                [T, F, T, T, F],
                [F, F, T, F, F],
                [F, F, T, T, F],
                [F, F, T, F, F],
                [F, F, T, F, F],
            ],
            Self::Watt => [
                [F, F, F, F, F],
                [T, F, T, F, T],
                [T, F, T, F, T],
                [T, F, T, F, T],
                [F, T, F, T, F],
            ],
        }
    }
}

/// 3 × 5 digit of the LP matrix.
fn digit(n: u16) -> [[bool; 3]; 5] {
    const T: bool = true;
    const F: bool = false;
    match n {
        0 => [[T, T, T], [T, F, T], [T, F, T], [T, F, T], [T, T, T]],
        1 => [[F, T, F], [T, T, F], [F, T, F], [F, T, F], [T, T, T]],
        2 => [[T, T, T], [F, F, T], [F, T, F], [T, F, F], [T, T, T]],
        3 => [[T, T, T], [F, F, T], [T, T, T], [F, F, T], [T, T, T]],
        4 => [[T, F, T], [T, F, T], [T, T, T], [F, F, T], [F, F, T]],
        5 => [[T, T, T], [T, F, F], [T, T, T], [F, F, T], [T, T, T]],
        6 => [[T, T, T], [T, F, F], [T, T, T], [T, F, T], [T, T, T]],
        7 => [[T, T, T], [F, F, T], [F, T, F], [F, T, F], [F, T, F]],
        8 => [[T, T, T], [T, F, T], [T, T, T], [T, F, T], [T, T, T]],
        9 => [[T, T, T], [T, F, T], [T, T, T], [F, F, T], [T, T, T]],
        _ => [[F; 3]; 5],
    }
}

fn insert<const M: usize, const N: usize>(
    m: &mut [[bool; 14]; 14],
    pattern: [[bool; M]; N],
    row: usize,
    col: usize,
) {
    for (i, line) in pattern.iter().enumerate().take(14usize.saturating_sub(row)) {
        for (j, v) in line.iter().enumerate().take(14usize.saturating_sub(col)) {
            m[row + i][col + j] = *v;
        }
    }
}

fn insert_value(m: &mut [[bool; 14]; 14], row: usize, (value, unit): (u16, MatrixUnit)) {
    let value = value.min(999);
    if value < 100 {
        insert(m, digit(value / 10), row, 1);
        insert(m, digit(value % 10), row, 5);
        insert(m, unit.pattern(), 5, 9);
    } else {
        insert(m, digit(value / 100), row, 1);
        insert(m, digit(value % 100 / 10), row, 5);
        insert(m, digit(value % 10), row, 9);
        insert(m, unit.pattern(), 5, 13);
    }
}

fn rotate(m: [[bool; 14]; 14], degrees: u16) -> [[bool; 14]; 14] {
    let mut r = [[false; 14]; 14];
    match degrees {
        90 => (0..14).for_each(|i| (0..14).for_each(|j| r[j][13 - i] = m[i][j])),
        180 => (0..14).for_each(|i| (0..14).for_each(|j| r[13 - i][13 - j] = m[i][j])),
        270 => (0..14).for_each(|i| (0..14).for_each(|j| r[13 - j][i] = m[i][j])),
        _ => return m,
    }
    r
}

/// The matrix as the 28 bytes the LP expects.
pub fn matrix_bytes(m: &[[bool; 14]; 14]) -> [u8; 28] {
    const ROW_VALUES: [u8; 7] = [16, 32, 64, 128, 1, 2, 4];
    let mut out = [0u8; 28];
    for col in 0..14 {
        let (mut odd, mut even) = (0u8, 0u8);
        for (row, value) in ROW_VALUES.iter().enumerate() {
            if m[row * 2][col] {
                odd += value;
            }
            if m[row * 2 + 1][col] {
                even += value;
            }
        }
        out[col] = odd;
        out[27 - col] = even;
    }
    out
}

/// The matrix a frame shows (preview of the LP).
pub fn lp_matrix(values: &[(u16, MatrixUnit)], rotation: u16) -> [[bool; 14]; 14] {
    let mut m = [[false; 14]; 14];
    match values {
        [one] => insert_value(&mut m, 5, *one),
        [a, b, ..] => {
            insert_value(&mut m, 1, *a);
            insert_value(&mut m, 8, *b);
        }
        [] => {}
    }
    rotate(m, rotation)
}

/// Value and unit of a metric on the LP matrix.
pub fn lp_value(metric: Metric, r: &Readings) -> (u16, MatrixUnit) {
    let temp_unit = if r.fahrenheit {
        MatrixUnit::Fahrenheit
    } else {
        MatrixUnit::Celsius
    };
    match metric {
        Metric::CpuTemp => (r.cpu_temp, temp_unit),
        Metric::GpuTemp => (r.gpu_temp, temp_unit),
        Metric::CpuPower => (r.cpu_power, MatrixUnit::Watt),
        Metric::GpuPower => (r.gpu_power, MatrixUnit::Watt),
        Metric::GpuLoad => (u16::from(r.gpu_load), MatrixUnit::Percent),
        _ => (u16::from(r.cpu_load), MatrixUnit::Percent),
    }
}

/// The reports of one refresh for the values `metrics` (the alternation
/// already chosen by the manager).
pub fn reports(
    protocol: Protocol,
    metrics: &[Metric],
    r: &Readings,
    rotation: u16,
) -> Vec<Vec<u8>> {
    let first = metrics.first().copied().unwrap_or(Metric::CpuTemp);
    let gpu_first = metrics.iter().any(|m| {
        matches!(
            m,
            Metric::GpuTemp | Metric::GpuLoad | Metric::GpuPower | Metric::GpuFreq
        )
    }) && !metrics.iter().any(|m| {
        matches!(
            m,
            Metric::CpuTemp | Metric::CpuLoad | Metric::CpuPower | Metric::CpuFreq
        )
    });
    let one = |p: [u8; 64]| vec![p.to_vec()];
    match protocol {
        Protocol::Ak => one(ak_report(
            if first == Metric::CpuLoad {
                Metric::CpuLoad
            } else {
                Metric::CpuTemp
            },
            r,
        )),
        Protocol::Ls => one(ak_report(
            if first == Metric::CpuPower {
                Metric::CpuPower
            } else {
                Metric::CpuTemp
            },
            r,
        )),
        Protocol::Ag => one(ag_report(first, r)),
        Protocol::Ld => one(ld_report(r)),
        Protocol::AkPro => one(ak_pro_report(r)),
        Protocol::AkPro2 => one(ak_pro2_report(r)),
        Protocol::Lq => one(lq_report(r)),
        Protocol::Lp => {
            let values: Vec<(u16, MatrixUnit)> =
                metrics.iter().take(2).map(|m| lp_value(*m, r)).collect();
            one(lp_report(&values, rotation))
        }
        Protocol::Ch => {
            let cpu = metrics
                .iter()
                .copied()
                .find(|m| matches!(m, Metric::CpuTemp | Metric::CpuLoad))
                .unwrap_or(Metric::CpuTemp);
            let gpu = metrics
                .iter()
                .copied()
                .find(|m| matches!(m, Metric::GpuTemp | Metric::GpuLoad));
            one(ch_report(cpu, gpu, r))
        }
        Protocol::ChGen2 => one(ch_gen2_report(gpu_first, r)),
        Protocol::Ch510 => vec![ch510_line(gpu_first, r)],
    }
}

/// A DeepCool display.
pub struct DeepCool {
    transport: Box<dyn Transport>,
    protocol: Protocol,
    /// "SE" models: no report number.
    no_report_id: bool,
    started: bool,
    caps: Capabilities,
}

impl DeepCool {
    pub fn new(transport: Box<dyn Transport>, protocol: Protocol, product: &str) -> Self {
        // AK400 → "AK400 DIGITAL", A400 → "AK400 DIGITAL SE".
        let no_report_id = protocol == Protocol::Ak
            && !product.trim().is_empty()
            && !product.trim_start().to_ascii_uppercase().starts_with("AK");
        Self {
            transport,
            protocol,
            no_report_id,
            started: false,
            caps: capabilities(protocol),
        }
    }

    fn send(&mut self, report: &[u8]) -> io::Result<()> {
        if self.no_report_id && report.first() == Some(&16) && report.len() == 64 {
            // Every byte one place to the left, as the SE models expect.
            let mut shifted = [0u8; 64];
            shifted[..63].copy_from_slice(&report[1..]);
            return self.transport.write(&shifted);
        }
        self.transport.write(report)
    }
}

impl HardwareDisplay for DeepCool {
    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn show(&mut self, frame: &Frame<'_>) -> io::Result<()> {
        if !self.started {
            match self.protocol {
                Protocol::Ak | Protocol::Ls | Protocol::Ch => self.send(&init_report())?,
                Protocol::Ld => {
                    for r in ld_init() {
                        self.send(&r)?;
                    }
                }
                _ => {}
            }
            self.started = true;
        }
        let readings = Readings::of(frame);
        for report in reports(
            self.protocol,
            frame.metrics,
            &readings,
            frame.settings.rotation,
        ) {
            self.send(&report)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::displays::hardware_display::DisplaySettings;
    use crate::displays::hid::mock::MockHid;
    use tuxrgb_core::signal::HardwareSample;

    fn readings() -> Readings {
        Readings {
            cpu_temp: 63,
            cpu_load: 47,
            cpu_power: 85,
            cpu_freq: 4875,
            gpu_temp: 58,
            gpu_load: 91,
            gpu_power: 230,
            gpu_freq: 2520,
            fahrenheit: false,
            alarm: false,
        }
    }

    #[test]
    fn ak_reports_match_deepcool_digital_linux() {
        let r = readings();
        let t = ak_report(Metric::CpuTemp, &r);
        assert_eq!(&t[..7], &[16, 19, 5, 0, 6, 3, 0]);
        let l = ak_report(Metric::CpuLoad, &r);
        assert_eq!(&l[..7], &[16, 76, 5, 0, 4, 7, 0]);
        // Idle CPU: one segment; Fahrenheit and the alarm.
        let hot = Readings {
            cpu_temp: 203,
            cpu_load: 3,
            fahrenheit: true,
            alarm: true,
            ..r
        };
        assert_eq!(
            &ak_report(Metric::CpuTemp, &hot)[..7],
            &[16, 35, 1, 2, 0, 3, 1]
        );
        assert!(t[7..].iter().all(|b| *b == 0));
        assert_eq!(&init_report()[..3], &[16, 170, 0]);
    }

    #[test]
    fn ag_shows_two_digits() {
        let r = readings();
        assert_eq!(&ag_report(Metric::CpuTemp, &r)[..6], &[16, 19, 0, 6, 3, 0]);
        let full = Readings { cpu_load: 100, ..r };
        assert_eq!(
            &ag_report(Metric::CpuLoad, &full)[..6],
            &[16, 76, 0, 9, 9, 0]
        );
    }

    #[test]
    fn full_status_reports_carry_floats_and_a_checksum() {
        let r = readings();
        let ld = ld_report(&r);
        assert_eq!(&ld[..8], &[16, 104, 1, 1, 11, 1, 2, 5]);
        assert_eq!(&ld[8..10], &85u16.to_be_bytes());
        assert_eq!(ld[10], 0);
        assert_eq!(&ld[11..15], &63.0f32.to_be_bytes());
        assert_eq!(ld[15], 47);
        assert_eq!(ld[16], checksum(&ld[1..16]));
        assert_eq!(ld[17], 22);
        let lq = lq_report(&r);
        assert_eq!(&lq[..7], &[16, 104, 1, 8, 12, 1, 2]);
        assert_eq!(&lq[7..9], &85u16.to_be_bytes());
        assert_eq!(&lq[10..14], &63.0f32.to_be_bytes());
        assert_eq!(lq[14], 47);
        assert_eq!(&lq[15..17], &4875u16.to_be_bytes());
        assert_eq!(lq[17], checksum(&lq[1..17]));
        assert_eq!(lq[18], 22);
        let pro = ak_pro2_report(&r);
        assert_eq!(&pro[..8], &[16, 104, 1, 4, 13, 1, 2, 8]);
        assert_eq!(&pro[16..18], &4875u16.to_be_bytes());
        assert_eq!(pro[18], checksum(&pro[1..18]));
        assert_eq!(pro[19], 22);
        let pro1 = ak_pro_report(&r);
        assert_eq!(&pro1[..8], &[16, 104, 1, 2, 11, 1, 2, 5]);
        assert_eq!(pro1[17], 22);
        let [a, b] = ld_init();
        assert_eq!(&a[..9], &[16, 104, 1, 1, 2, 3, 1, 112, 22]);
        assert_eq!(&b[..9], &[16, 104, 1, 1, 2, 2, 0, 110, 22]);
    }

    #[test]
    fn cases_show_cpu_and_gpu() {
        let r = readings();
        let ch = ch_report(Metric::CpuTemp, Some(Metric::GpuLoad), &r);
        assert_eq!(&ch[..11], &[16, 19, 5, 0, 6, 3, 76, 9, 0, 9, 1]);
        let gen2 = ch_gen2_report(true, &r);
        assert_eq!(&gen2[..7], &[16, 104, 1, 6, 35, 1, 4]);
        assert_eq!(&gen2[19..21], &230u16.to_be_bytes());
        assert_eq!(&gen2[21..25], &58.0f32.to_be_bytes());
        assert_eq!(gen2[25], 91);
        assert_eq!(&gen2[26..28], &2520u16.to_be_bytes());
        assert_eq!(gen2[40], checksum(&gen2[1..40]));
        assert_eq!(gen2[41], 22);
        assert_eq!(ch510_line(false, &r), b"HLXDATA(47,63,0,0,C)\r\n".to_vec());
    }

    #[test]
    fn the_lp_matrix_packs_like_the_reference() {
        // One value: "47 %" centred on row 5.
        let m = lp_matrix(&[(47, MatrixUnit::Percent)], 0);
        assert!(m[5][1] && m[5][3] && !m[5][2], "4: top row X.X");
        let bytes = matrix_bytes(&m);
        // Column 0 is empty, column 1 has the left of the "4".
        assert_eq!(bytes[0], 0);
        assert_ne!(bytes[1], 0);
        let report = lp_report(&[(47, MatrixUnit::Percent)], 0);
        assert_eq!(&report[..6], &[16, 104, 1, 5, 29, 1]);
        assert_eq!(&report[6..34], &bytes);
        assert_eq!(report[34], checksum(&report[1..34]));
        assert_eq!(report[35], 22);
        // Turning it four quarter turns gives it back.
        let r4 = rotate(rotate(rotate(rotate(m, 90), 90), 90), 90);
        assert_eq!(r4, m);
        assert_ne!(rotate(m, 90), m);
    }

    #[test]
    fn se_models_drop_the_report_number_and_start_first() {
        let (mock, log) = MockHid::silent();
        let mut d = DeepCool::new(Box::new(mock), Protocol::Ak, "A400");
        let settings = DisplaySettings::default();
        let sample = HardwareSample {
            cpu_temp: Some(63.4),
            cpu_load: Some(0.47),
            ..HardwareSample::default()
        };
        let frame = Frame {
            settings: &settings,
            sample: &sample,
            metrics: &[Metric::CpuTemp],
            fahrenheit: false,
            picture: None,
        };
        d.show(&frame).unwrap();
        d.show(&frame).unwrap();
        let sent = log.take();
        assert_eq!(sent.len(), 3, "start-up report once, then one per refresh");
        assert_eq!(&sent[0][..2], &[170, 0]);
        assert_eq!(&sent[1][..6], &[19, 5, 0, 6, 3, 0]);
        // The regular model keeps it.
        let (mock, log) = MockHid::silent();
        let mut d = DeepCool::new(Box::new(mock), Protocol::Ak, "AK620-DIGITAL");
        d.show(&frame).unwrap();
        assert_eq!(&log.take()[1][..7], &[16, 19, 5, 0, 6, 3, 0]);
    }
}
