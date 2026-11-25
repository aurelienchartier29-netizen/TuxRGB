//! GPU load on Windows from the "GPU Engine" performance counters – the
//! numbers of the Task Manager. They exist for every graphics card (AMD,
//! Intel, NVIDIA) with a WDDM 2 driver, need no program and no rights.
//!
//! One counter instance per process and per engine of a card
//! (`pid_1234_luid_0x0_0xC3A1_phys_0_eng_0_engtype_3D`): the load of an
//! engine is the sum over the processes, the load of the card is its busiest
//! engine, as the Task Manager shows it. The busiest card wins.
//!
//! The grouping is plain code, tested on every platform; only the counters
//! are Windows-only.

#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::HashMap;

/// Engine of a counter instance (`luid_…_phys_0_eng_0`), without the
/// process part.
fn engine(instance: &str) -> Option<&str> {
    let start = instance.find("luid_")?;
    let rest = &instance[start..];
    let end = rest.find("_engtype_").unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Card of an engine (`luid_0x00000000_0x0000C3A1`).
fn card(engine: &str) -> &str {
    engine.find("_phys_").map_or(engine, |i| &engine[..i])
}

/// GPU load `0..=1` from `(instance, percent)` pairs.
pub(crate) fn gpu_load<'a>(values: impl IntoIterator<Item = (&'a str, f64)>) -> Option<f32> {
    let mut engines: HashMap<&str, f64> = HashMap::new();
    for (name, v) in values {
        if let Some(e) = engine(name)
            && v.is_finite()
            && v >= 0.0
        {
            *engines.entry(e).or_default() += v;
        }
    }
    let mut cards: HashMap<&str, f64> = HashMap::new();
    for (e, v) in engines {
        let c = cards.entry(card(e)).or_default();
        *c = c.max(v);
    }
    cards
        .into_values()
        .reduce(f64::max)
        .map(|v| (v / 100.0).clamp(0.0, 1.0) as f32)
}

#[cfg(windows)]
pub(crate) use counters::GpuCounters;

#[cfg(windows)]
mod counters {
    use windows::Win32::System::Performance::{
        PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA, PdhAddEnglishCounterW,
        PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    };
    use windows::core::{PCWSTR, w};

    /// `PDH_CSTATUS_VALID_DATA`, `PDH_CSTATUS_NEW_DATA`.
    const VALID: [u32; 2] = [0, 1];

    /// An open query on the GPU engine counters.
    pub(crate) struct GpuCounters {
        query: isize,
        counter: isize,
    }

    impl GpuCounters {
        /// `None` without such counters (old driver, Wine…).
        pub(crate) fn open() -> Option<Self> {
            let mut query = 0isize;
            // SAFETY: valid out-pointers; the query is closed on drop.
            unsafe {
                if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                    return None;
                }
                let mut counter = 0isize;
                let me = Self { query, counter: 0 };
                if PdhAddEnglishCounterW(
                    query,
                    w!("\\GPU Engine(*)\\Utilization Percentage"),
                    0,
                    &mut counter,
                ) != 0
                {
                    return None;
                }
                // A rate: the first collection only sets the start point.
                PdhCollectQueryData(query);
                Some(Self { counter, ..me })
            }
        }

        /// Load `0..=1` since the previous call; `None` when unreadable.
        pub(crate) fn read(&self) -> Option<f32> {
            // SAFETY: the buffer is sized by the first call, as documented;
            // the names point into it and are read before it is freed.
            unsafe {
                if PdhCollectQueryData(self.query) != 0 {
                    return None;
                }
                let mut bytes = 0u32;
                let mut count = 0u32;
                let r = PdhGetFormattedCounterArrayW(
                    self.counter,
                    PDH_FMT_DOUBLE,
                    &mut bytes,
                    &mut count,
                    None,
                );
                if r != PDH_MORE_DATA || bytes == 0 {
                    // No instance at all: nothing runs on the card.
                    return (r == 0).then_some(0.0);
                }
                let item = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
                let mut buf = vec![0u64; (bytes as usize).div_ceil(8)];
                let items = buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
                if PdhGetFormattedCounterArrayW(
                    self.counter,
                    PDH_FMT_DOUBLE,
                    &mut bytes,
                    &mut count,
                    Some(items),
                ) != 0
                {
                    return None;
                }
                let n = (count as usize).min(bytes as usize / item);
                let slice = std::slice::from_raw_parts(items, n);
                let named: Vec<(String, f64)> = slice
                    .iter()
                    .filter(|i| VALID.contains(&i.FmtValue.CStatus))
                    .filter_map(|i| {
                        Some((i.szName.to_string().ok()?, i.FmtValue.Anonymous.doubleValue))
                    })
                    .collect();
                super::gpu_load(named.iter().map(|(n, v)| (n.as_str(), *v))).or(Some(0.0))
            }
        }
    }

    impl Drop for GpuCounters {
        fn drop(&mut self) {
            // SAFETY: the query was opened by `open`.
            unsafe { PdhCloseQuery(self.query) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_manager_arithmetic() {
        let values = [
            // Two processes on the 3D engine of the card: 30 + 25 = 55 %.
            (
                "pid_1200_luid_0x00000000_0x0000C3A1_phys_0_eng_0_engtype_3D",
                30.0,
            ),
            (
                "pid_8812_luid_0x00000000_0x0000C3A1_phys_0_eng_0_engtype_3D",
                25.0,
            ),
            // Video decode of the same card: 40 %, less busy.
            (
                "pid_8812_luid_0x00000000_0x0000C3A1_phys_0_eng_3_engtype_VideoDecode",
                40.0,
            ),
            // The integrated graphics, almost idle.
            (
                "pid_4_luid_0x00000000_0x00009F21_phys_0_eng_0_engtype_3D",
                2.0,
            ),
        ];
        let load = gpu_load(values).unwrap();
        assert!((load - 0.55).abs() < 1e-6, "{load}");
    }

    #[cfg(windows)]
    #[test]
    fn counters_are_harmless() {
        if let Some(c) = GpuCounters::open() {
            let _ = c.read();
        }
    }

    #[test]
    fn capped_and_robust() {
        let values = [
            ("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 80.0),
            ("pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 60.0),
            ("garbage", 99.0),
            ("pid_3_luid_0x0_0x1_phys_0_eng_1_engtype_Copy", f64::NAN),
        ];
        assert_eq!(gpu_load(values), Some(1.0));
        assert_eq!(gpu_load([]), None);
        assert_eq!(
            engine("pid_9_luid_0xA_0xB_phys_1_eng_2_engtype_3D"),
            Some("luid_0xA_0xB_phys_1_eng_2")
        );
        assert_eq!(card("luid_0xA_0xB_phys_1_eng_2"), "luid_0xA_0xB");
    }
}
