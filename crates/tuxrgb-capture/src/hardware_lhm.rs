//! Sensors published by LibreHardwareMonitor (or the older
//! OpenHardwareMonitor) on Windows.
//!
//! Windows gives no CPU temperature – nor the sensors of AMD and Intel
//! graphics cards – to a program without a kernel driver. When
//! LibreHardwareMonitor runs, it publishes every sensor it reads in WMI
//! (namespace `root\LibreHardwareMonitor`, class `Sensor`), which any
//! program may query: TuxRGB reads them there, as many monitoring tools do.
//! Nothing is installed and nothing runs with privileges; without it the
//! values stay unknown, as before.
//!
//! The choice of the sensors is plain code, tested on every platform; only
//! the WMI query is Windows-only.

#![cfg_attr(not(windows), allow(dead_code))]

/// One row of the `Sensor` class.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Sensor {
    /// `/amdcpu/0/temperature/2`, `/gpu-amd/0/load/0`…
    pub id: String,
    /// `CPU Package`, `Core (Tctl/Tdie)`, `GPU Core`…
    pub name: String,
    /// `Temperature`, `Load` (%), `Power` (W), `Clock` (MHz)…
    pub kind: String,
    pub value: f32,
}

/// What TuxRGB takes from those sensors.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LhmReading {
    pub cpu_temp: Option<f32>,
    pub cpu_power: Option<f32>,
    pub gpu_temp: Option<f32>,
    /// `0..=1`.
    pub gpu_load: Option<f32>,
    pub gpu_power: Option<f32>,
    pub gpu_freq: Option<f32>,
}

/// Hardware kind of a sensor, from the first part of its identifier.
fn family(id: &str) -> &str {
    id.trim_start_matches('/').split('/').next().unwrap_or("")
}

fn is_cpu(id: &str) -> bool {
    matches!(family(id), "intelcpu" | "amdcpu" | "genericcpu")
}

/// Rank of a graphics card: a dedicated one before the integrated one.
/// LibreHardwareMonitor says `gpu-nvidia`, OpenHardwareMonitor `nvidiagpu`.
fn gpu_rank(id: &str) -> Option<u8> {
    match family(id) {
        "gpu-nvidia" | "nvidiagpu" => Some(0),
        "gpu-amd" | "atigpu" => Some(1),
        "gpu-intel" => Some(2),
        _ => None,
    }
}

/// Device part of an identifier (`/gpu-amd/0`), to keep one card.
fn device(id: &str) -> &str {
    let mut slashes = 0;
    for (i, c) in id.char_indices() {
        if c == '/' {
            slashes += 1;
            if slashes == 3 {
                return &id[..i];
            }
        }
    }
    id
}

/// First sensor of `kind` whose name is one of `names` (in that order of
/// preference) among `sensors`.
fn by_name<'a>(sensors: &[&'a Sensor], kind: &str, names: &[&str]) -> Option<&'a Sensor> {
    names.iter().find_map(|n| {
        sensors
            .iter()
            .find(|s| s.kind == kind && s.name.eq_ignore_ascii_case(n))
            .copied()
    })
}

fn plausible(v: f32, max: f32) -> Option<f32> {
    (v.is_finite() && v > 0.0 && v <= max).then_some(v)
}

pub(crate) fn pick(sensors: &[Sensor]) -> LhmReading {
    let cpu: Vec<&Sensor> = sensors.iter().filter(|s| is_cpu(&s.id)).collect();
    let cpu_temp = by_name(
        &cpu,
        "Temperature",
        &[
            "CPU Package",
            "Core (Tctl/Tdie)",
            "Core (Tdie)",
            "Core (Tctl)",
            "Tctl/Tdie",
            "Package",
            "Core Average",
            "Core Max",
        ],
    )
    .map(|s| s.value)
    .or_else(|| {
        // Otherwise the hottest core; "Distance to TjMax" is a margin, not a
        // temperature.
        cpu.iter()
            .filter(|s| s.kind == "Temperature" && !s.name.contains("Distance"))
            .map(|s| s.value)
            .filter(|v| v.is_finite())
            .reduce(f32::max)
    })
    .and_then(|v| plausible(v, 150.0));
    let cpu_power = by_name(&cpu, "Power", &["CPU Package", "Package", "CPU Cores"])
        .and_then(|s| plausible(s.value, 1000.0));

    // The best ranked card that has a sensor; one card only.
    let gpu_device = sensors
        .iter()
        .filter_map(|s| gpu_rank(&s.id).map(|r| (r, device(&s.id))))
        .min()
        .map(|(_, d)| d.to_owned());
    let gpu: Vec<&Sensor> = match &gpu_device {
        Some(d) => sensors.iter().filter(|s| device(&s.id) == d).collect(),
        None => Vec::new(),
    };
    LhmReading {
        cpu_temp,
        cpu_power,
        gpu_temp: by_name(&gpu, "Temperature", &["GPU Core", "GPU Temperature"])
            .and_then(|s| plausible(s.value, 150.0)),
        gpu_load: by_name(&gpu, "Load", &["GPU Core", "D3D 3D"])
            .map(|s| (s.value / 100.0).clamp(0.0, 1.0))
            .filter(|v| v.is_finite()),
        gpu_power: by_name(&gpu, "Power", &["GPU Package", "GPU Power", "GPU Core"])
            .and_then(|s| plausible(s.value, 2000.0)),
        gpu_freq: by_name(&gpu, "Clock", &["GPU Core"]).and_then(|s| plausible(s.value, 10_000.0)),
    }
}

#[cfg(windows)]
pub(crate) use wmi::Lhm;

#[cfg(windows)]
pub(crate) mod wmi {
    use super::Sensor;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, CoCreateInstance, CoSetProxyBlanket, EOAC_NONE,
        RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
    };
    use windows::Win32::System::Wmi::{
        IWbemClassObject, IWbemLocator, IWbemServices, WBEM_FLAG_FORWARD_ONLY,
        WBEM_FLAG_RETURN_IMMEDIATELY, WbemLocator,
    };
    use windows::core::{BSTR, PCWSTR, VARIANT, w};

    /// `RPC_C_AUTHN_WINNT`, `RPC_C_AUTHZ_NONE` (rpcdce.h).
    const AUTHN_WINNT: u32 = 10;
    const AUTHZ_NONE: u32 = 0;
    /// `VT_R4`, `VT_R8`, `VT_BSTR`.
    const VT_R4: u16 = 4;
    const VT_R8: u16 = 5;
    const VT_BSTR: u16 = 8;
    /// Longest wait for a batch of rows, ms.
    const NEXT_TIMEOUT: i32 = 2000;

    /// A connection to the namespace of LibreHardwareMonitor (or of
    /// OpenHardwareMonitor). COM must be initialised on the calling thread.
    pub(crate) struct Lhm {
        services: IWbemServices,
    }

    impl Lhm {
        pub(crate) fn connect() -> Option<Self> {
            ["ROOT\\LibreHardwareMonitor", "ROOT\\OpenHardwareMonitor"]
                .iter()
                .find_map(|ns| {
                    let found = Self::connect_to(ns)?;
                    tracing::info!(namespace = ns, "hardware monitor found in WMI");
                    Some(found)
                })
        }

        pub(super) fn connect_to(namespace: &str) -> Option<Self> {
            // SAFETY: plain COM calls with owned arguments.
            unsafe {
                let locator: IWbemLocator =
                    CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;
                let services = locator
                    .ConnectServer(
                        &BSTR::from(namespace),
                        &BSTR::new(),
                        &BSTR::new(),
                        &BSTR::new(),
                        0,
                        &BSTR::new(),
                        None,
                    )
                    .ok()?;
                // The process may have set no COM security of its own.
                let _ = CoSetProxyBlanket(
                    &services,
                    AUTHN_WINNT,
                    AUTHZ_NONE,
                    PCWSTR::null(),
                    RPC_C_AUTHN_LEVEL_CALL,
                    RPC_C_IMP_LEVEL_IMPERSONATE,
                    None,
                    EOAC_NONE,
                );
                Some(Self { services })
            }
        }

        /// Rows of a WQL query (at most a few thousand).
        pub(super) fn query(&self, wql: &str) -> windows::core::Result<Vec<IWbemClassObject>> {
            let mut out = Vec::new();
            // SAFETY: plain COM calls; the rows are owned by `out`.
            unsafe {
                let rows = self.services.ExecQuery(
                    &BSTR::from("WQL"),
                    &BSTR::from(wql),
                    WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                    None,
                )?;
                loop {
                    let mut batch: [Option<IWbemClassObject>; 32] = Default::default();
                    let mut n = 0u32;
                    let hr = rows.Next(NEXT_TIMEOUT, &mut batch, &mut n);
                    hr.ok()?;
                    out.extend(batch.into_iter().take(n as usize).flatten());
                    // S_OK: a full batch, maybe more; S_FALSE / timeout: done.
                    if hr.0 != 0 || n == 0 || out.len() > 4096 {
                        break;
                    }
                }
            }
            Ok(out)
        }

        /// Every sensor (none when the program is closed but its namespace
        /// remains).
        pub(crate) fn read(&self) -> windows::core::Result<Vec<Sensor>> {
            Ok(self
                .query("SELECT Identifier, Name, SensorType, Value FROM Sensor")?
                .iter()
                .filter_map(sensor)
                .collect())
        }
    }

    pub(super) fn property(row: &IWbemClassObject, name: PCWSTR) -> Option<VARIANT> {
        let mut v = VARIANT::default();
        // SAFETY: valid out-pointer; the variant owns what it receives.
        unsafe { row.Get(name, 0, &mut v, None, None) }.ok()?;
        Some(v)
    }

    pub(super) fn text(v: &VARIANT) -> Option<String> {
        let raw = v.as_raw();
        // SAFETY: the union is read according to its type tag; a BSTR is
        // preceded by its length in bytes.
        unsafe {
            if raw.Anonymous.Anonymous.vt != VT_BSTR {
                return None;
            }
            let p = raw.Anonymous.Anonymous.Anonymous.bstrVal;
            if p.is_null() {
                return Some(String::new());
            }
            let len = (*(p as *const u32).sub(1) / 2) as usize;
            Some(String::from_utf16_lossy(std::slice::from_raw_parts(
                p,
                len.min(1024),
            )))
        }
    }

    pub(super) fn number(v: &VARIANT) -> Option<f32> {
        let raw = v.as_raw();
        // SAFETY: the union is read according to its type tag.
        unsafe {
            match raw.Anonymous.Anonymous.vt {
                VT_R4 => Some(raw.Anonymous.Anonymous.Anonymous.fltVal),
                VT_R8 => Some(raw.Anonymous.Anonymous.Anonymous.dblVal as f32),
                _ => None,
            }
        }
    }

    fn sensor(row: &IWbemClassObject) -> Option<Sensor> {
        Some(Sensor {
            id: text(&property(row, w!("Identifier"))?)?,
            name: text(&property(row, w!("Name"))?)?,
            kind: text(&property(row, w!("SensorType"))?)?,
            value: number(&property(row, w!("Value"))?)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: &str, name: &str, kind: &str, value: f32) -> Sensor {
        Sensor {
            id: id.into(),
            name: name.into(),
            kind: kind.into(),
            value,
        }
    }

    #[test]
    fn ryzen_with_a_radeon() {
        let sensors = [
            s(
                "/amdcpu/0/temperature/2",
                "Core (Tctl/Tdie)",
                "Temperature",
                61.5,
            ),
            s(
                "/amdcpu/0/temperature/3",
                "CCD1 (Tdie)",
                "Temperature",
                58.0,
            ),
            s("/amdcpu/0/power/0", "Package", "Power", 74.2),
            s("/amdcpu/0/load/0", "CPU Total", "Load", 23.0),
            s("/gpu-amd/0/temperature/0", "GPU Core", "Temperature", 52.0),
            s(
                "/gpu-amd/0/temperature/1",
                "GPU Hot Spot",
                "Temperature",
                66.0,
            ),
            s("/gpu-amd/0/load/0", "GPU Core", "Load", 37.0),
            s("/gpu-amd/0/power/0", "GPU Package", "Power", 141.0),
            s("/gpu-amd/0/clock/0", "GPU Core", "Clock", 2405.0),
            s("/lpc/nct6798d/0/temperature/0", "CPU", "Temperature", 40.0),
        ];
        let r = pick(&sensors);
        assert_eq!(r.cpu_temp, Some(61.5));
        assert_eq!(r.cpu_power, Some(74.2));
        assert_eq!(r.gpu_temp, Some(52.0));
        assert!((r.gpu_load.unwrap() - 0.37).abs() < 1e-6);
        assert_eq!(r.gpu_power, Some(141.0));
        assert_eq!(r.gpu_freq, Some(2405.0));
    }

    #[test]
    fn intel_with_its_integrated_graphics_and_an_nvidia_card() {
        let sensors = [
            s("/intelcpu/0/temperature/0", "Core #1", "Temperature", 55.0),
            s(
                "/intelcpu/0/temperature/1",
                "Core #1 Distance to TjMax",
                "Temperature",
                45.0,
            ),
            s(
                "/intelcpu/0/temperature/8",
                "CPU Package",
                "Temperature",
                63.0,
            ),
            s("/intelcpu/0/power/0", "CPU Package", "Power", 48.5),
            s("/gpu-intel/0/load/0", "D3D 3D", "Load", 5.0),
            s(
                "/gpu-nvidia/0/temperature/0",
                "GPU Core",
                "Temperature",
                47.0,
            ),
            s("/gpu-nvidia/0/load/0", "GPU Core", "Load", 12.0),
        ];
        let r = pick(&sensors);
        assert_eq!(r.cpu_temp, Some(63.0));
        assert_eq!(r.cpu_power, Some(48.5));
        // The NVIDIA card, not the integrated one.
        assert_eq!(r.gpu_temp, Some(47.0));
        assert!((r.gpu_load.unwrap() - 0.12).abs() < 1e-6);
        assert_eq!(r.gpu_power, None);
    }

    #[test]
    fn falls_back_to_the_hottest_core_and_ignores_nonsense() {
        let sensors = [
            s("/intelcpu/0/temperature/0", "Core #1", "Temperature", 51.0),
            s("/intelcpu/0/temperature/1", "Core #2", "Temperature", 57.0),
            s(
                "/intelcpu/0/temperature/2",
                "Core #2 Distance to TjMax",
                "Temperature",
                90.0,
            ),
            s("/atigpu/0/temperature/0", "GPU Core", "Temperature", 0.0),
        ];
        let r = pick(&sensors);
        assert_eq!(r.cpu_temp, Some(57.0));
        assert_eq!(r.gpu_temp, None);
        assert_eq!(pick(&[]), LhmReading::default());
    }

    #[cfg(windows)]
    #[test]
    fn reads_wmi_rows_and_variants() {
        use super::wmi::*;
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        use windows::core::{BSTR, VARIANT};
        // SAFETY: COM for this test thread.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        assert_eq!(
            text(&VARIANT::from(BSTR::from("Core (Tctl/Tdie)"))).as_deref(),
            Some("Core (Tctl/Tdie)")
        );
        assert_eq!(number(&VARIANT::from(61.5f32)), Some(61.5));
        assert_eq!(number(&VARIANT::from(2.5f64)), Some(2.5));
        assert_eq!(text(&VARIANT::from(3u32)), None);
        // Absent on a test machine: looking for it must stay harmless.
        if let Some(m) = Lhm::connect() {
            let _ = m.read();
        }
        // The standard namespace answers everywhere (Wine too).
        let cim = Lhm::connect_to("ROOT\\CIMV2").expect("ROOT\\CIMV2");
        let rows = cim
            .query("SELECT Caption FROM Win32_OperatingSystem")
            .expect("query");
        assert!(!rows.is_empty());
        let caption = property(&rows[0], windows::core::w!("Caption")).and_then(|v| text(&v));
        assert!(caption.is_some_and(|c| !c.is_empty()));
    }

    #[test]
    fn device_of_an_identifier() {
        assert_eq!(device("/gpu-amd/1/load/0"), "/gpu-amd/1");
        assert_eq!(device("/amdcpu/0"), "/amdcpu/0");
        assert_eq!(family("/gpu-nvidia/0/load/0"), "gpu-nvidia");
    }
}
