//! Other programs using the system SMBus (TuxRGB 3.5).
//!
//! Lighting a memory stick takes several SMBus transfers in a row (choose a
//! register, then write it). Each transfer is atomic, but a sequence is not:
//! two programs talking to the RAM at the same time can interleave theirs,
//! and a value then lands in the wrong register – the way sticks have been
//! made unreadable. OpenRGB itself warns to never run two RGB programs at
//! once. So while one of the programs below runs, TuxRGB leaves the RAM and
//! the SMBus motherboards alone (OpenRGB is started without their
//! detectors), and takes them back once it has closed.
//!
//! On Windows, monitoring tools such as HWiNFO or AIDA64 share the bus
//! through a global lock that OpenRGB also takes: they are not listed.

/// Process name (lower case) → product shown to the user.
#[cfg(not(windows))]
const KNOWN: &[(&str, &str)] = &[
    ("openrgb", "OpenRGB"),
    ("liquidctl", "liquidctl"),
    ("i2cdetect", "i2c-tools"),
    ("i2cdump", "i2c-tools"),
    ("i2cget", "i2c-tools"),
    ("i2cset", "i2c-tools"),
    ("i2ctransfer", "i2c-tools"),
    ("decode-dimms", "i2c-tools"),
];

#[cfg(windows)]
const KNOWN: &[(&str, &str)] = &[
    ("openrgb.exe", "OpenRGB"),
    ("icue.exe", "Corsair iCUE"),
    ("lightingservice.exe", "ASUS Armoury Crate / Aura"),
    ("msi_central_service.exe", "MSI Center"),
    ("msi.centralserver.exe", "MSI Dragon Center"),
    ("ledkeeper2.exe", "MSI Mystic Light"),
    ("rgbfusion.exe", "Gigabyte RGB Fusion"),
    ("signalrgb.exe", "SignalRGB"),
    ("signalrgblauncher.exe", "SignalRGB"),
];

/// The products in `names` (process names), each once, sorted.
pub fn match_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for n in names {
        let n = n.trim().to_ascii_lowercase();
        if let Some((_, product)) = KNOWN.iter().find(|(p, _)| *p == n)
            && !out.contains(product)
        {
            out.push(product);
        }
    }
    out.sort_unstable();
    out
}

/// Programs using the system SMBus right now, but `own` (the embedded
/// OpenRGB server, already started).
pub fn smbus_users(own: Option<u32>) -> Vec<&'static str> {
    let names: Vec<String> = processes()
        .into_iter()
        .filter(|(pid, _)| Some(*pid) != own && *pid != std::process::id())
        .map(|(_, name)| name)
        .collect();
    match_names(names.iter().map(String::as_str))
}

/// (pid, process name) of every process.
#[cfg(not(windows))]
fn processes() -> Vec<(u32, String)> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            // `pid (name) state …`: a process already gone (zombie) is not
            // using anything.
            let stat = std::fs::read_to_string(e.path().join("stat")).ok()?;
            let (name, rest) = stat.split_once('(')?.1.rsplit_once(')')?;
            if rest.trim_start().starts_with(['Z', 'X']) {
                return None;
            }
            Some((pid, name.to_owned()))
        })
        .collect()
}

#[cfg(windows)]
fn processes() -> Vec<(u32, String)> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    let mut out = Vec::new();
    // SAFETY: a process snapshot read entry by entry, then closed.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut entry).is_ok();
        while ok {
            let len = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            out.push((
                entry.th32ProcessID,
                String::from_utf16_lossy(&entry.szExeFile[..len]),
            ));
            ok = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_programs_are_recognised() {
        #[cfg(not(windows))]
        let names = ["bash", "OpenRGB", "openrgb", "i2cset", "i2cdump", "firefox"];
        #[cfg(windows)]
        let names = ["explorer.exe", "OpenRGB.exe", "iCUE.exe", "HWiNFO64.exe"];
        let found = match_names(names);
        assert!(found.contains(&"OpenRGB"));
        // Each product once.
        assert_eq!(
            found.iter().filter(|p| **p == "OpenRGB").count(),
            1,
            "{found:?}"
        );
        assert!(match_names(["bash", "firefox.exe", "hwinfo64.exe"]).is_empty());
    }

    #[test]
    fn the_process_list_is_readable() {
        // At least this test itself.
        assert!(!processes().is_empty());
        // Nothing of ours is reported.
        let _ = smbus_users(None);
    }
}
