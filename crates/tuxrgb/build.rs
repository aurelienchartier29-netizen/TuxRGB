//! Windows build only: links the icon, the version information and the
//! application manifest (`packaging/windows/`) into `tuxrgb.exe`.
//!
//! * GNU target (cross-compiled from Linux, or MSYS2): `windres`
//!   (`$WINDRES`, `x86_64-w64-mingw32-windres` or `windres`);
//! * MSVC target: `rc.exe` (`$RC`, `PATH`, or the Windows 10/11 SDK).
//!
//! Without a resource compiler the build still succeeds (with a warning):
//! the window and tray icons are set at run time anyway.
//!
//! Nothing at all happens for Linux builds.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let packaging = manifest_dir.join("../../packaging/windows");
    let icon = packaging.join("tuxrgb.ico");
    let manifest = packaging.join("tuxrgb.exe.manifest");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-env-changed=WINDRES");
    println!("cargo:rerun-if-env-changed=RC");

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    // The manifest carries the numeric version (3.1.3.0).
    let numeric = format!(
        "{}.{}.{}.0",
        std::env::var("CARGO_PKG_VERSION_MAJOR").unwrap_or_default(),
        std::env::var("CARGO_PKG_VERSION_MINOR").unwrap_or_default(),
        std::env::var("CARGO_PKG_VERSION_PATCH").unwrap_or_default()
    );
    let manifest_text = std::fs::read_to_string(&manifest)
        .expect("read packaging/windows/tuxrgb.exe.manifest")
        .replace("@VERSION@", &numeric);
    let manifest = out.join("tuxrgb.exe.manifest");
    std::fs::write(&manifest, manifest_text).expect("write the manifest");
    let rc = out.join("tuxrgb.rc");
    std::fs::write(&rc, resource_script(&icon, &manifest)).expect("write tuxrgb.rc");

    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let linked = if msvc {
        compile_msvc(&rc, &out)
    } else {
        compile_gnu(&rc, &out)
    };
    match linked {
        Some(obj) => println!("cargo:rustc-link-arg-bins={}", obj.display()),
        None => println!(
            "cargo:warning=no resource compiler found: tuxrgb.exe built without its icon / version information"
        ),
    }
}

/// Paths in .rc files: forward slashes, quoted.
fn rc_path(p: &Path) -> String {
    let p = p.canonicalize().unwrap_or_else(|_| p.to_owned());
    let s = p.to_string_lossy().replace('\\', "/");
    // `\\?\C:/…` from canonicalize on Windows.
    s.trim_start_matches("//?/").to_owned()
}

fn resource_script(icon: &Path, manifest: &Path) -> String {
    let v = |k: &str| {
        std::env::var(k)
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0)
    };
    let (major, minor, patch) = (
        v("CARGO_PKG_VERSION_MAJOR"),
        v("CARGO_PKG_VERSION_MINOR"),
        v("CARGO_PKG_VERSION_PATCH"),
    );
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 24 "{manifest}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040C04B0"
    BEGIN
      VALUE "CompanyName", "TuxRGB contributors"
      VALUE "FileDescription", "TuxRGB"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "tuxrgb"
      VALUE "LegalCopyright", "GPL-2.0-or-later"
      VALUE "OriginalFilename", "tuxrgb.exe"
      VALUE "ProductName", "TuxRGB"
      VALUE "ProductVersion", "{version}"
      VALUE "Comments", "RGB lighting for Windows 10 / 11, powered by OpenRGB."
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x040C, 1200
  END
END
"#,
        icon = rc_path(icon),
        manifest = rc_path(manifest),
    )
}

fn runs(cmd: &mut Command) -> bool {
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

fn compile_gnu(rc: &Path, out: &Path) -> Option<PathBuf> {
    let obj = out.join("tuxrgb-res.o");
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(w) = std::env::var("WINDRES") {
        candidates.push(w);
    }
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    candidates.push(format!("{arch}-w64-mingw32-windres"));
    candidates.push("windres".into());
    candidates.into_iter().find_map(|tool| {
        runs(
            Command::new(&tool)
                .arg("--input-format=rc")
                .arg("--output-format=coff")
                .arg("-c")
                .arg("65001")
                .arg("-i")
                .arg(rc)
                .arg("-o")
                .arg(&obj),
        )
        .then(|| obj.clone())
    })
}

/// `rc.exe` from `$RC`, `PATH`, then the newest Windows SDK.
fn find_rc() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rc) = std::env::var("RC") {
        out.push(PathBuf::from(rc));
    }
    out.push(PathBuf::from("rc.exe"));
    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    if let Ok(entries) = std::fs::read_dir(kits) {
        let mut versions: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path().join("x64").join("rc.exe"))
            .filter(|p| p.is_file())
            .collect();
        versions.sort();
        out.extend(versions.into_iter().rev());
    }
    out
}

fn compile_msvc(rc: &Path, out: &Path) -> Option<PathBuf> {
    let res = out.join("tuxrgb.res");
    find_rc().into_iter().find_map(|tool| {
        runs(
            Command::new(&tool)
                .arg("/nologo")
                .arg("/fo")
                .arg(&res)
                .arg(rc),
        )
        .then(|| res.clone())
    })
}
