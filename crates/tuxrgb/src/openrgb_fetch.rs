//! Windows installer: fetches the official OpenRGB 1.1 build for Windows
//! and unpacks it next to `tuxrgb.exe` (`tuxrgb.exe --fetch-openrgb <dir>`).
//!
//! This used to be a PowerShell script started by the installer with
//! `-ExecutionPolicy Bypass`: an installer that runs a script to download
//! programs is exactly what antivirus heuristics flag (Windows Defender did).
//! TuxRGB now does it itself, with the HTTPS client of its updater, so the
//! installer runs no script at all.
//!
//! Exit codes: 0 done, 2 download failed, 3 unexpected archive content.

#![cfg_attr(not(windows), allow(dead_code))]

use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::time::Duration;

const TAG: &str = "release_1.1";
const FILE: &str = "OpenRGB_1.1_Windows_64_8e16de2.zip";
/// The archive is about 20 MB; anything much smaller is an error page.
const MIN_BYTES: usize = 1024 * 1024;
const MAX_BYTES: u64 = 256 * 1024 * 1024;

pub const EXIT_OK: i32 = 0;
pub const EXIT_DOWNLOAD: i32 = 2;
pub const EXIT_ARCHIVE: i32 = 3;

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .user_agent(&format!("TuxRGB-Installer/{}", env!("CARGO_PKG_VERSION")))
        .try_proxy_from_env(true)
        .build()
}

/// Windows 64-bit zip assets of a Codeberg (Gitea) release document.
fn assets_of(doc: &serde_json::Value) -> Vec<String> {
    doc.get("assets")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let name = a.get("name")?.as_str()?;
            let url = a.get("browser_download_url")?.as_str()?;
            (name.contains("Windows") && name.contains("64") && name.ends_with(".zip"))
                .then(|| url.to_owned())
        })
        .collect()
}

fn candidate_urls(agent: &ureq::Agent) -> Vec<String> {
    let mut urls = agent
        .get(&format!(
            "https://codeberg.org/api/v1/repos/OpenRGB/OpenRGB/releases/tags/{TAG}"
        ))
        .call()
        .ok()
        .and_then(|r| r.into_string().ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .map(|doc| assets_of(&doc))
        .unwrap_or_default();
    for u in [
        format!("https://codeberg.org/OpenRGB/OpenRGB/releases/download/{TAG}/{FILE}"),
        format!("https://openrgb.org/releases/{TAG}/{FILE}"),
    ] {
        if !urls.contains(&u) {
            urls.push(u);
        }
    }
    urls
}

fn download(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, String> {
    println!("Téléchargement : {url}");
    let resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let mut body = Vec::new();
    resp.into_reader()
        .take(MAX_BYTES)
        .read_to_end(&mut body)
        .map_err(|e| e.to_string())?;
    if body.len() < MIN_BYTES || !body.starts_with(b"PK") {
        return Err(format!("fichier inattendu ({} octets)", body.len()));
    }
    Ok(body)
}

/// Unpacks the folder of the archive that holds `OpenRGB.exe` into `dest`
/// (replaced). Entries outside that folder, or escaping it, are skipped.
pub fn unpack<R: Read + Seek>(archive: R, dest: &Path) -> Result<usize, String> {
    let mut zip = zip::ZipArchive::new(archive).map_err(|e| e.to_string())?;
    // Folder of OpenRGB.exe inside the archive ("" or "OpenRGB Windows 64-bit/").
    let root: PathBuf = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok()?.enclosed_name())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("OpenRGB.exe"))
        })
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .ok_or("OpenRGB.exe absent de l'archive")?;
    let staging = dest.with_extension("new");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let mut count = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = entry
            .enclosed_name()
            .and_then(|p| p.strip_prefix(&root).ok().map(Path::to_path_buf))
        else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = staging.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut f = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
        count += 1;
    }
    if !staging.join("OpenRGB.exe").is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("OpenRGB.exe absent de l'archive".into());
    }
    let _ = std::fs::remove_dir_all(dest);
    std::fs::rename(&staging, dest).map_err(|e| e.to_string())?;
    Ok(count)
}

/// `tuxrgb.exe --fetch-openrgb <dir>`.
pub fn main(dest: &Path) -> i32 {
    let agent = agent();
    let mut archive = None;
    for url in candidate_urls(&agent) {
        match download(&agent, &url) {
            Ok(body) => {
                archive = Some(body);
                break;
            }
            Err(e) => println!("  échec : {e}"),
        }
    }
    let Some(body) = archive else {
        return EXIT_DOWNLOAD;
    };
    match unpack(std::io::Cursor::new(body), dest) {
        Ok(n) => {
            println!("OpenRGB installé dans {} ({n} fichiers)", dest.display());
            EXIT_OK
        }
        Err(e) => {
            println!("Archive OpenRGB : {e}");
            EXIT_ARCHIVE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn archive(files: &[(&str, &[u8])]) -> std::io::Cursor<Vec<u8>> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, data) in files {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(data).unwrap();
        }
        let mut c = w.finish().unwrap();
        c.set_position(0);
        c
    }

    #[test]
    fn unpacks_the_folder_of_openrgb() {
        let dir = std::env::temp_dir().join(format!("tuxrgb-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("openrgb");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("old.dll"), b"old").unwrap();
        let zip = archive(&[
            ("OpenRGB Windows 64-bit/OpenRGB.exe", b"MZ"),
            ("OpenRGB Windows 64-bit/Qt5Core.dll", b"dll"),
            ("OpenRGB Windows 64-bit/platforms/qwindows.dll", b"p"),
            ("README.txt", b"outside"),
            ("../evil.exe", b"no"),
        ]);
        assert_eq!(unpack(zip, &dest).unwrap(), 3);
        assert_eq!(std::fs::read(dest.join("OpenRGB.exe")).unwrap(), b"MZ");
        assert!(dest.join("platforms/qwindows.dll").is_file());
        assert!(!dest.join("old.dll").exists());
        assert!(!dest.join("README.txt").exists());
        assert!(!dir.join("evil.exe").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_an_archive_without_openrgb() {
        let dir = std::env::temp_dir().join(format!("tuxrgb-fetch-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("openrgb");
        assert!(unpack(archive(&[("x/readme.txt", b"hi")]), &dest).is_err());
        assert!(!dest.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn codeberg_assets_are_filtered() {
        let doc = serde_json::json!({"assets": [
            {"name": "OpenRGB_1.1_Windows_64_8e16de2.zip", "browser_download_url": "https://a/w64.zip"},
            {"name": "OpenRGB_1.1_Windows_32_8e16de2.zip", "browser_download_url": "https://a/w32.zip"},
            {"name": "OpenRGB_1.1_x86_64.AppImage", "browser_download_url": "https://a/x.AppImage"}
        ]});
        assert_eq!(assets_of(&doc), vec!["https://a/w64.zip".to_owned()]);
    }
}
