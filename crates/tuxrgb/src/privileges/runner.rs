//! Runs `tuxrgb-helper` with administrator rights through Polkit.

use crate::paths::{APP_ID, PackageKind, package_kind};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const HELPER: &str = "tuxrgb-helper";

#[derive(Clone, Debug, PartialEq)]
pub struct HelperReport {
    pub steps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HelperError {
    /// The user closed the Polkit dialog or typed a wrong password.
    Cancelled,
    /// No `pkexec` / `flatpak-spawn` on this system.
    NoPolkit,
    NotFound,
    /// The change cannot be made automatically here; the text says how.
    Manual(String),
    Failed(String),
}

impl std::fmt::Display for HelperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str(crate::i18n::t("authentification annulée")),
            Self::NoPolkit => f.write_str(crate::i18n::t(
                "pkexec (Polkit) est introuvable sur ce système",
            )),
            Self::NotFound => f.write_str(crate::i18n::t(
                "le programme d'aide tuxrgb-helper est introuvable",
            )),
            Self::Manual(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}

/// Where the helper ships, by packaging format.
fn helper_source() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(appdir) = std::env::var_os("APPDIR") {
        candidates.push(Path::new(&appdir).join("usr/libexec/tuxrgb").join(HELPER));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("../libexec/tuxrgb").join(HELPER));
        candidates.push(dir.join(HELPER));
    }
    candidates.push(PathBuf::from("/app/libexec/tuxrgb").join(HELPER));
    candidates.push(PathBuf::from("/usr/libexec/tuxrgb").join(HELPER));
    candidates.into_iter().find(|p| p.is_file())
}

/// Copies the helper where root (or the host, for Flatpak) can execute it.
///
/// * AppImage: the image is a FUSE mount that root cannot read, so the
///   helper is staged in `$XDG_RUNTIME_DIR/tuxrgb/` (mode 0700).
/// * Flatpak: `$XDG_RUNTIME_DIR/app/<app-id>/` is shared with the host at the
///   same path, so the host's pkexec can run it.
///
/// Hardening: the directory must be a real directory owned by the user
/// (no symlink planted in its place), the copy gets a fresh unique name
/// created with O_EXCL (never follows a link), read-only and executable by
/// the user only (0500), its content is checked against the bundled helper,
/// and it is deleted as soon as pkexec returns.
fn stage_helper(src: &Path) -> Result<PathBuf, HelperError> {
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let fail = |e: std::io::Error| HelperError::Failed(e.to_string());
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = if package_kind() == PackageKind::Flatpak {
        runtime.join("app").join(APP_ID)
    } else {
        runtime.join("tuxrgb")
    };
    std::fs::create_dir_all(&dir).map_err(fail)?;
    let meta = std::fs::symlink_metadata(&dir).map_err(fail)?;
    let uid = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.uid() != uid {
        return Err(HelperError::Failed(crate::i18n::trf!(
            "dossier de préparation suspect : {}",
            dir.display()
        )));
    }
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    // Leftovers of an interrupted run.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            if e.file_name()
                .to_string_lossy()
                .starts_with("tuxrgb-helper-")
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let bytes = std::fs::read(src).map_err(fail)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dst = dir.join(format!("tuxrgb-helper-{}-{nonce:x}", std::process::id()));
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o500)
            .open(&dst)
            .map_err(fail)?;
        f.write_all(&bytes).map_err(fail)?;
        f.sync_all().map_err(fail)?;
    }
    if std::fs::read(&dst).map_err(fail)? != bytes {
        let _ = std::fs::remove_file(&dst);
        return Err(HelperError::Failed(
            crate::i18n::t("copie du programme d'aide altérée").into(),
        ));
    }
    Ok(dst)
}

fn helper_path() -> Result<PathBuf, HelperError> {
    let src = helper_source().ok_or(HelperError::NotFound)?;
    // Installed under /usr: run in place so the dedicated Polkit action
    // (io.github.tuxrgb.TuxRGB.policy) applies.
    if src.starts_with("/usr/") && package_kind() == PackageKind::Native {
        return Ok(src);
    }
    stage_helper(&src)
}

/// Runs `verb` (`install` / `reset` / `status`) as root.
pub async fn run_helper(
    verb: &'static str,
    stdin_data: Option<String>,
) -> Result<HelperReport, HelperError> {
    let helper = helper_path()?;
    let staged = helper
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with("tuxrgb-helper-"));
    let mut cmd = if package_kind() == PackageKind::Flatpak {
        let mut c = Command::new("flatpak-spawn");
        c.arg("--host").arg("pkexec").arg(&helper).arg(verb);
        c
    } else {
        let mut c = Command::new("pkexec");
        c.arg(&helper).arg(verb);
        c
    };
    cmd.stdin(if stdin_data.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            HelperError::NoPolkit
        } else {
            HelperError::Failed(e.to_string())
        }
    })?;
    if let (Some(data), Some(mut stdin)) = (stdin_data, child.stdin.take()) {
        // The helper may stop before reading everything (refused
        // authentication, early error): its own report below says why, a
        // broken pipe here must not hide it.
        if let Err(e) = stdin.write_all(data.as_bytes()).await {
            tracing::info!("helper stopped reading its input: {e}");
        }
        drop(stdin);
    }
    let out = child.wait_with_output().await;
    if staged {
        let _ = std::fs::remove_file(&helper);
    }
    let out = out.map_err(|e| HelperError::Failed(e.to_string()))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let steps: Vec<String> = stdout
        .lines()
        .filter_map(|l| l.strip_prefix("STEP ").map(str::to_owned))
        .collect();
    match out.status.code() {
        Some(0) if stdout.contains("RESULT ok") => Ok(HelperReport { steps }),
        // pkexec: 126 = not authorized / dialog dismissed, 127 = auth failed.
        Some(126) | Some(127) => Err(HelperError::Cancelled),
        // Bootloader the helper does not edit (NixOS…): instructions.
        Some(6) => Err(HelperError::Manual(
            stdout
                .lines()
                .find_map(|l| l.strip_prefix("RESULT error "))
                .unwrap_or("modification manuelle nécessaire")
                .to_owned(),
        )),
        code => {
            let msg = stdout
                .lines()
                .find_map(|l| l.strip_prefix("RESULT error "))
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    let e = stderr.trim();
                    if e.is_empty() {
                        crate::i18n::trf!("échec (code {})", code.unwrap_or(-1))
                    } else {
                        e.to_owned()
                    }
                });
            Err(HelperError::Failed(msg))
        }
    }
}

/// Commands the user can paste in a terminal when Polkit is unavailable.
pub fn manual_commands() -> String {
    "sudo cp ~/.config/tuxrgb/60-openrgb.rules /etc/udev/rules.d/60-openrgb.rules\n\
     echo -e 'i2c-dev' | sudo tee /etc/modules-load.d/tuxrgb-i2c.conf\n\
     sudo modprobe i2c-dev\n\
     sudo udevadm control --reload-rules && sudo udevadm trigger"
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the AppImage path end to end: helper found under $APPDIR,
    /// staged into $XDG_RUNTIME_DIR, started through "pkexec" (a stub that
    /// simply execs its arguments), stdout parsed.
    #[tokio::test]
    #[ignore = "needs target/debug/tuxrgb-helper (cargo build -p tuxrgb-helper)"]
    async fn runs_staged_helper_through_pkexec() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let built = root.join("target/debug/tuxrgb-helper");
        assert!(built.exists(), "build the helper first");
        let tmp = std::env::temp_dir().join(format!("tuxrgb-runner-{}", std::process::id()));
        let appdir = tmp.join("AppDir");
        std::fs::create_dir_all(appdir.join("usr/libexec/tuxrgb")).unwrap();
        std::fs::copy(&built, appdir.join("usr/libexec/tuxrgb/tuxrgb-helper")).unwrap();
        let bin = tmp.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("pkexec"), "#!/bin/sh\nexec \"$@\"\n").unwrap();
        std::fs::set_permissions(bin.join("pkexec"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let runtime = tmp.join("run");
        std::fs::create_dir_all(&runtime).unwrap();
        // SAFETY: single test touching these variables.
        unsafe {
            std::env::set_var("APPDIR", &appdir);
            std::env::set_var("XDG_RUNTIME_DIR", &runtime);
            let path = std::env::var("PATH").unwrap_or_default();
            std::env::set_var("PATH", format!("{}:{path}", bin.display()));
        }
        let report = run_helper("status", None).await.expect("helper ran");
        // Staged under a unique name, then removed once pkexec returned.
        let leftovers = std::fs::read_dir(runtime.join("tuxrgb")).unwrap().count();
        assert_eq!(leftovers, 0, "staged helper must be deleted after use");
        let _ = report;
        // Invalid rules must be rejected with the helper's own message.
        let err = run_helper(
            "install",
            Some("SUBSYSTEM==\"usb\", RUN+=\"/bin/sh\"\n".into()),
        )
        .await;
        match err {
            Err(HelperError::Failed(m)) => {
                assert!(m.contains("interdite") || m.contains("root"), "{m}")
            }
            other => panic!("unexpected {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
