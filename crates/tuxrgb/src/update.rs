//! Updates from the GitHub releases of TuxRGB.
//!
//! One request to the GitHub API (`releases/latest`) gives the newest
//! version. Linux and Windows are published together in the same release,
//! and each build takes its own file from it: the AppImage on Linux, the
//! installer on Windows. A download is checked before anything is replaced:
//! size, SHA-256 (the `.sha256` file published next to it) and file format.
//!
//! * AppImage: the new file replaces the running one in its folder (same
//!   name, or the name with the new version number), then TuxRGB restarts
//!   from it. The menu entry and the autostart follow the new path.
//! * Windows: the installer runs in update mode (`/S /UPDATE`): silent,
//!   settings and administrator mode kept, TuxRGB started again at the end.
//! * Flatpak, distribution packages, source builds: the release page opens,
//!   the package manager does the update.
//!
//! Developer hooks: `TUXRGB_DEV_UPDATE_URL` (another `releases/latest`
//! document, `http://` accepted), `TUXRGB_DEV_PRETEND_VERSION` (compare
//! as if this build were older) and `TUXRGB_DEV_AUTO_UPDATE` (install and
//! restart without clicking: end-to-end tests).

use crate::paths::{PackageKind, package_kind};
use futures::SinkExt;
use futures::Stream;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Project page (releases, sources).
pub const REPO_URL: &str = "https://github.com/aurelienchartier29-netizen/TuxRGB";
/// Newest published release (drafts and pre-releases excluded by GitHub).
const LATEST_API: &str =
    "https://api.github.com/repos/aurelienchartier29-netizen/TuxRGB/releases/latest";
/// Largest file accepted (the AppImage is about 40 MB).
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;

/// A newer release and what this build needs from it.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// `2.0.6` (no leading `v`).
    pub version: String,
    /// Release page on GitHub.
    pub page: String,
    /// The file for this platform, if the release has one.
    pub asset: Option<Asset>,
    /// URL of `<asset>.sha256`, if published.
    pub checksum: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// Result of a check.
#[derive(Clone, Debug, PartialEq)]
pub enum Check {
    /// This build is the newest (the latest published version).
    UpToDate(String),
    Available(Release),
}

/// How this build updates itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Replace the AppImage file, then restart.
    AppImage,
    /// Run the new installer in update mode.
    #[cfg_attr(not(windows), allow(dead_code))]
    WindowsInstaller,
    /// Open the release page (Flatpak, packages, source builds).
    Manual,
}

pub fn method() -> Method {
    match package_kind() {
        PackageKind::AppImage if std::env::var_os("APPIMAGE").is_some() => Method::AppImage,
        #[cfg(windows)]
        PackageKind::WindowsInstaller => Method::WindowsInstaller,
        _ => Method::Manual,
    }
}

/// Where the update stands, for the interface.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    /// Not checked during this session.
    #[default]
    Idle,
    Checking,
    UpToDate(String),
    Available(Release),
    Downloading {
        release: Release,
        done: u64,
        total: u64,
    },
    /// Downloaded and verified: restart (AppImage) or installer starting.
    Ready {
        release: Release,
        ready: Ready,
    },
    Failed {
        release: Option<Release>,
        error: String,
    },
}

impl State {
    /// The release on offer, if any.
    pub fn release(&self) -> Option<&Release> {
        match self {
            Self::Available(r)
            | Self::Downloading { release: r, .. }
            | Self::Ready { release: r, .. } => Some(r),
            Self::Failed { release, .. } => release.as_ref(),
            _ => None,
        }
    }

    pub fn busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Ready { .. }
        )
    }
}

/// Download progress, then the result.
#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    Downloading { done: u64, total: u64 },
    Done(Result<Ready, String>),
}

/// A verified update, ready to be applied.
#[derive(Clone, Debug, PartialEq)]
pub enum Ready {
    /// The AppImage was replaced: restart from this file.
    #[cfg_attr(windows, allow(dead_code))]
    AppImage(PathBuf),
    /// The installer to run.
    #[cfg_attr(not(windows), allow(dead_code))]
    Installer(PathBuf),
}

fn parse_version(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.trim().trim_start_matches(['v', 'V'])).ok()
}

/// Version this build compares with.
pub fn current_version() -> semver::Version {
    std::env::var("TUXRGB_DEV_PRETEND_VERSION")
        .ok()
        .and_then(|v| parse_version(&v))
        .or_else(|| parse_version(env!("CARGO_PKG_VERSION")))
        .unwrap_or_else(|| semver::Version::new(0, 0, 0))
}

/// `TUXRGB_DEV_AUTO_UPDATE`: an update found is installed and applied
/// without clicking (end-to-end tests).
pub fn dev_auto_update() -> bool {
    std::env::var_os("TUXRGB_DEV_AUTO_UPDATE").is_some()
}

/// Is `name` the file this platform installs?
fn is_platform_asset(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if cfg!(windows) {
        lower.ends_with("setup.exe")
    } else {
        lower.ends_with(&format!("{}.appimage", std::env::consts::ARCH))
    }
}

/// Reads a GitHub `release` document; `None` if it is not one.
fn parse_release(doc: &serde_json::Value) -> Option<(semver::Version, Release)> {
    let tag = doc.get("tag_name")?.as_str()?;
    let version = parse_version(tag)?;
    let page = doc
        .get("html_url")
        .and_then(|u| u.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{REPO_URL}/releases/tag/{tag}"));
    let assets: Vec<Asset> = doc
        .get("assets")
        .and_then(|a| a.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    Some(Asset {
                        name: a.get("name")?.as_str()?.to_owned(),
                        url: a.get("browser_download_url")?.as_str()?.to_owned(),
                        size: a.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let asset = assets.iter().find(|a| is_platform_asset(&a.name)).cloned();
    let checksum = asset.as_ref().and_then(|file| {
        let wanted = format!("{}.sha256", file.name);
        assets
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(&wanted))
            .map(|a| a.url.clone())
    });
    Some((
        version.clone(),
        Release {
            version: version.to_string(),
            page,
            asset,
            checksum,
        },
    ))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .user_agent(&format!(
            "TuxRGB/{} ({})",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS
        ))
        .try_proxy_from_env(true)
        .build()
}

fn http_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(404, _) => {
            crate::i18n::t("Aucune version publiée sur GitHub pour le moment.").into()
        }
        ureq::Error::Status(403 | 429, _) => {
            crate::i18n::t("GitHub limite le nombre de vérifications : réessayez dans une heure.")
                .into()
        }
        ureq::Error::Status(code, _) => trf!("GitHub a répondu avec l'erreur {}.", code),
        ureq::Error::Transport(t) => {
            tracing::info!("update check: {t}");
            crate::i18n::t("GitHub injoignable : vérifiez la connexion Internet.").into()
        }
    }
}

fn check_blocking() -> Result<Check, String> {
    let url = std::env::var("TUXRGB_DEV_UPDATE_URL").unwrap_or_else(|_| LATEST_API.into());
    let body = agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(http_error)?
        .into_string()
        .map_err(|e| trf!("Réponse de GitHub illisible : {}", e))?;
    let doc: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| trf!("Réponse de GitHub illisible : {}", e))?;
    let (latest, release) = parse_release(&doc)
        .ok_or_else(|| crate::i18n::t("Réponse de GitHub inattendue.").to_owned())?;
    let current = current_version();
    tracing::info!(%current, %latest, "update check");
    Ok(if latest > current {
        Check::Available(release)
    } else {
        Check::UpToDate(latest.to_string())
    })
}

/// Asks GitHub for the newest release.
pub async fn check() -> Result<Check, String> {
    tokio::task::spawn_blocking(check_blocking)
        .await
        .unwrap_or_else(|e| Err(e.to_string()))
}

/// The SHA-256 published in a `.sha256` file (`<hex>  <name>`).
fn parse_checksum(text: &str) -> Option<String> {
    let token = text.split_whitespace().next()?.to_ascii_lowercase();
    (token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())).then_some(token)
}

/// Is this the file format this platform expects?
fn looks_valid(head: &[u8]) -> bool {
    if cfg!(windows) {
        head.starts_with(b"MZ")
    } else {
        // ELF with the AppImage type 2 magic at offset 8.
        head.len() >= 11 && head.starts_with(b"\x7fELF") && &head[8..11] == b"AI\x02"
    }
}

/// Where the download goes, and where the final file lands.
fn destination(release: &Release, asset: &Asset) -> Result<(PathBuf, PathBuf), String> {
    match method() {
        Method::AppImage => {
            let current = PathBuf::from(std::env::var_os("APPIMAGE").unwrap_or_default());
            let dir = current
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| crate::i18n::t("Emplacement de l'AppImage inconnu.").to_owned())?;
            let tmp = dir.join(format!(".{}.part", asset.name));
            // `TuxRGB-2.0.4-x86_64.AppImage` becomes `TuxRGB-2.0.5-…`; a
            // name without the version (a launcher's copy) is kept.
            let name = current
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let old = current_version().to_string();
            let target = if !name.is_empty() && name.contains(&old) {
                dir.join(name.replace(&old, &release.version))
            } else {
                current
            };
            Ok((tmp, target))
        }
        Method::WindowsInstaller => {
            let dir = std::env::temp_dir().join("TuxRGB-update");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            Ok((
                dir.join(format!("{}.part", asset.name)),
                dir.join(&asset.name),
            ))
        }
        Method::Manual => Err(crate::i18n::t(
            "Cette installation se met à jour avec son gestionnaire de paquets.",
        )
        .into()),
    }
}

fn download_blocking(release: &Release, report: &mut dyn FnMut(u64, u64)) -> Result<Ready, String> {
    let asset = release.asset.as_ref().ok_or_else(|| {
        crate::i18n::t("Cette version ne contient pas de fichier pour ce système.").to_owned()
    })?;
    let agent = agent();
    let expected =
        match &release.checksum {
            Some(url) => {
                let text = agent
                    .get(url)
                    .call()
                    .map_err(http_error)?
                    .into_string()
                    .map_err(|e| e.to_string())?;
                Some(parse_checksum(&text).ok_or_else(|| {
                    crate::i18n::t("Somme de contrôle publiée illisible.").to_owned()
                })?)
            }
            None => {
                tracing::warn!("no .sha256 published for {}", asset.name);
                None
            }
        };
    let (tmp, target) = destination(release, asset)?;
    let result = (|| -> Result<(), String> {
        let response = agent.get(&asset.url).call().map_err(http_error)?;
        let total = response
            .header("Content-Length")
            .and_then(|l| l.parse::<u64>().ok())
            .unwrap_or(asset.size);
        if total > MAX_DOWNLOAD {
            return Err(crate::i18n::t("Fichier de mise à jour trop volumineux.").into());
        }
        let mut file = std::fs::File::create(&tmp).map_err(|e| {
            trf!(
                "Impossible d'écrire dans {} : {}",
                tmp.parent().unwrap_or(&tmp).display(),
                e
            )
        })?;
        let mut reader = response.into_reader().take(MAX_DOWNLOAD + 1);
        let mut hasher = Sha256::new();
        let mut head = Vec::with_capacity(16);
        let mut buf = vec![0u8; 64 * 1024];
        let mut done = 0u64;
        let mut last = Instant::now();
        report(0, total);
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| trf!("Téléchargement interrompu : {}", e))?;
            if n == 0 {
                break;
            }
            if head.len() < 16 {
                let take = (16 - head.len()).min(n);
                head.extend_from_slice(&buf[..take]);
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            if last.elapsed() >= Duration::from_millis(100) {
                last = Instant::now();
                report(done, total);
            }
        }
        file.sync_all().map_err(|e| e.to_string())?;
        report(done, total);
        if (total > 0 && done != total) || (asset.size > 0 && done != asset.size) {
            return Err(crate::i18n::t("Téléchargement incomplet.").into());
        }
        if let Some(expected) = &expected {
            let got: String = hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            if &got != expected {
                return Err(crate::i18n::t(
                    "Le fichier téléchargé ne correspond pas à sa somme de contrôle.",
                )
                .into());
            }
        }
        if !looks_valid(&head) {
            return Err(crate::i18n::t(
                "Le fichier téléchargé n'est pas une mise à jour de TuxRGB.",
            )
            .into());
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    put_in_place(&tmp, &target)
}

#[cfg(unix)]
fn put_in_place(tmp: &Path, target: &Path) -> Result<Ready, String> {
    use std::os::unix::fs::PermissionsExt;
    let current = PathBuf::from(std::env::var_os("APPIMAGE").unwrap_or_default());
    let fail = |e: std::io::Error| {
        let _ = std::fs::remove_file(tmp);
        e.to_string()
    };
    std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o755)).map_err(fail)?;
    // The running AppImage stays mounted from its open file: replacing or
    // removing the path does not disturb it.
    std::fs::rename(tmp, target).map_err(fail)?;
    if target != current && current.is_file() {
        let _ = std::fs::remove_file(&current);
    }
    tracing::info!("AppImage updated: {}", target.display());
    Ok(Ready::AppImage(target.to_path_buf()))
}

#[cfg(windows)]
fn put_in_place(tmp: &Path, target: &Path) -> Result<Ready, String> {
    let _ = std::fs::remove_file(target);
    std::fs::rename(tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(tmp);
        e.to_string()
    })?;
    tracing::info!("installer downloaded: {}", target.display());
    Ok(Ready::Installer(target.to_path_buf()))
}

/// Downloads and verifies the update, reporting progress.
pub fn download(release: Release) -> impl Stream<Item = Progress> {
    iced::stream::channel(32, async move |mut output| {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();
        let job = tokio::task::spawn_blocking(move || {
            let progress = tx.clone();
            let mut report = move |done, total| {
                let _ = progress.send(Progress::Downloading { done, total });
            };
            let result = download_blocking(&release, &mut report);
            let _ = tx.send(Progress::Done(result));
        });
        while let Some(p) = rx.recv().await {
            let last = matches!(p, Progress::Done(_));
            let _ = output.send(p).await;
            if last {
                break;
            }
        }
        if job.await.is_err() {
            let _ = output
                .send(Progress::Done(Err(crate::i18n::t(
                    "Téléchargement interrompu.",
                )
                .into())))
                .await;
        }
    })
}

/// Linux: starts the new AppImage once this process has exited.
#[cfg(unix)]
pub fn restart(appimage: &Path) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg("while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; exec \"$2\"")
        .arg("tuxrgb-restart")
        .arg(std::process::id().to_string())
        .arg(appimage)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0);
    // The new AppImage sets up its own environment.
    for var in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
        cmd.env_remove(var);
    }
    if let Some(dirs) = std::env::var_os("XDG_DATA_DIRS") {
        // Drop the old mount point that AppRun put first.
        let appdir = std::env::var("APPDIR").unwrap_or_default();
        let kept: Vec<_> = std::env::split_paths(&dirs)
            .filter(|p| appdir.is_empty() || !p.starts_with(&appdir))
            .collect();
        if let Ok(joined) = std::env::join_paths(kept) {
            cmd.env("XDG_DATA_DIRS", joined);
        }
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Windows: runs the installer in update mode (UAC confirmation unless
/// TuxRGB already has administrator rights). Blocking until Windows has
/// started it or the confirmation was refused.
#[cfg(windows)]
pub fn run_installer(installer: &Path) -> Result<(), String> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, w};
    let file = HSTRING::from(installer.as_os_str());
    // SAFETY: plain ShellExecute with valid strings.
    let h = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &file,
            w!("/S /UPDATE"),
            None,
            SW_SHOWNORMAL,
        )
    };
    if (h.0 as isize) > 32 {
        Ok(())
    } else {
        Err(crate::i18n::t("L'installation de la mise à jour a été annulée.").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_doc(tag: &str, names: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("{REPO_URL}/releases/tag/{tag}"),
            "assets": names.iter().map(|n| serde_json::json!({
                "name": n,
                "browser_download_url": format!("https://example.invalid/{n}"),
                "size": 1234,
            })).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn each_platform_takes_its_own_file() {
        let doc = release_doc(
            "v2.0.6",
            &[
                "TuxRGB-2.0.6-x86_64.AppImage",
                "TuxRGB-2.0.6-x86_64.AppImage.sha256",
                "TuxRGB-2.0.6-Windows-Setup.exe",
                "TuxRGB-2.0.6-Windows-Setup.exe.sha256",
            ],
        );
        let (v, r) = parse_release(&doc).unwrap();
        assert_eq!(v, semver::Version::new(2, 0, 6));
        assert_eq!(r.version, "2.0.6");
        let asset = r.asset.unwrap();
        if cfg!(windows) {
            assert_eq!(asset.name, "TuxRGB-2.0.6-Windows-Setup.exe");
        } else {
            assert_eq!(
                asset.name,
                format!("TuxRGB-2.0.6-{}.AppImage", std::env::consts::ARCH)
            );
        }
        assert_eq!(
            r.checksum.unwrap(),
            format!("https://example.invalid/{}.sha256", asset.name)
        );
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(parse_version("v2.0.10").unwrap() > parse_version("2.0.9").unwrap());
        assert!(parse_version("v2.0.4-win-beta").unwrap() < parse_version("2.0.4").unwrap());
        assert!(parse_version("nightly").is_none());
    }

    #[test]
    fn checksum_files_are_parsed() {
        let h = "e5abaae2674533f0da4b1b01de5753db48d8d1420d70782b78caf80320defa4d";
        assert_eq!(
            parse_checksum(&format!("{h}  TuxRGB.exe\n")).as_deref(),
            Some(h)
        );
        assert_eq!(parse_checksum(&h.to_uppercase()).as_deref(), Some(h));
        assert!(parse_checksum("not a hash").is_none());
    }

    #[test]
    fn only_real_updates_are_accepted() {
        let mut appimage = b"\x7fELF\x02\x01\x01\x00AI\x02\x00\x00\x00\x00\x00".to_vec();
        assert_eq!(looks_valid(&appimage), !cfg!(windows));
        appimage[9] = b'X';
        assert!(!looks_valid(&appimage));
        assert_eq!(looks_valid(b"MZ\x90\x00"), cfg!(windows));
        assert!(!looks_valid(b"<html>"));
    }
}
