#!/usr/bin/env bash
# Builds TuxRGB for Windows from Linux:
#   target/x86_64-pc-windows-gnu/release/tuxrgb.exe   (cross-compiled)
#   dist/TuxRGB-<version>-Windows-Setup.exe           (NSIS installer)
#
# Needs:
#   - mingw-w64 (x86_64-w64-mingw32-gcc, x86_64-w64-mingw32-windres)
#   - NSIS 3 (makensis)
#       Debian / Ubuntu : sudo apt install mingw-w64 nsis
#       Fedora          : sudo dnf install mingw64-gcc mingw64-binutils mingw32-nsis
#       Arch            : sudo pacman -S mingw-w64-gcc nsis
#   - the Rust standard library for Windows:
#       rustup target add x86_64-pc-windows-gnu
#     (without rustup, the script rebuilds it from rust-src: RUSTC_BOOTSTRAP
#     + -Zbuild-std, used to produce the official builds).
#
# Usage: bash scripts/windows/build-windows.sh [--no-installer]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
TARGET=x86_64-pc-windows-gnu
BASE="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
VERSION="$BASE"
INSTALLER=1
[ "${1:-}" = "--no-installer" ] && INSTALLER=0

bleu() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
erreur() { printf '\033[1;31m✘ %s\033[0m\n' "$*" >&2; exit 1; }

command -v x86_64-w64-mingw32-gcc >/dev/null || erreur "x86_64-w64-mingw32-gcc introuvable (paquet mingw-w64)."
command -v x86_64-w64-mingw32-windres >/dev/null || erreur "x86_64-w64-mingw32-windres introuvable (paquet mingw-w64)."
[ "$INSTALLER" = 0 ] || command -v makensis >/dev/null || erreur "makensis introuvable (paquet nsis)."

export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
export WINDRES=x86_64-w64-mingw32-windres

# Standard library for the target: prebuilt (rustup) or rebuilt from source.
EXTRA=()
SYSROOT="$(rustc --print sysroot)"
if [ ! -d "$SYSROOT/lib/rustlib/$TARGET/lib" ]; then
  if command -v rustup >/dev/null && rustup target add "$TARGET" >/dev/null 2>&1; then
    echo "Cible $TARGET installée avec rustup."
  elif [ -d "$SYSROOT/lib/rustlib/src/rust/library" ]; then
    echo "Pas de bibliothèque standard précompilée pour $TARGET : reconstruction depuis rust-src."
    export RUSTC_BOOTSTRAP=1
    EXTRA=(-Zbuild-std=std,panic_unwind)
  else
    erreur "Installez la cible : rustup target add $TARGET (ou le composant rust-src)."
  fi
fi

bleu "Compilation de tuxrgb.exe ($VERSION)"
cargo build --release --locked --target "$TARGET" -p tuxrgb "${EXTRA[@]}"
EXE="target/$TARGET/release/tuxrgb.exe"
[ -f "$EXE" ] || erreur "$EXE introuvable."

# A Windows program built with MinGW must not need any MinGW DLL.
if command -v x86_64-w64-mingw32-objdump >/dev/null; then
  extra_dlls="$(x86_64-w64-mingw32-objdump -p "$EXE" | awk '/DLL Name/ {print $3}' |
    grep -viE '^(kernel32|user32|gdi32|advapi32|shell32|ole32|oleaut32|ws2_32|ntdll|bcrypt|bcryptprimitives|userenv|comctl32|comdlg32|dwmapi|uxtheme|imm32|winmm|dxgi|d3d12|d3dcompiler_47|opengl32|setupapi|version|shlwapi|propsys|uiautomationcore|api-ms-win-.*|msvcrt|crypt32|secur32|powrprof|dbghelp|winspool\.drv|hid|cfgmgr32|oleacc|dcomp|d2d1|dwrite|windowscodecs|mfplat|avrt|ksuser|mmdevapi|synchronization|combase|rpcrt4|iphlpapi|ncrypt|netapi32|psapi|wtsapi32)\.dll$' || true)"
  [ -z "$extra_dlls" ] || echo "Attention : DLL non système requises : $extra_dlls"
fi

[ "$INSTALLER" = 1 ] || { echo "$EXE"; exit 0; }

bleu "Préparation de l'installateur"
STAGE="build/windows/stage"
rm -rf "$STAGE"
mkdir -p "$STAGE/licenses" dist
cp "$EXE" "$STAGE/tuxrgb.exe"
cp packaging/windows/tuxrgb.ico packaging/windows/installer-side.bmp "$STAGE/"
# PawnIO: the official setup, signed by its author, shipped in the installer
# (no download, no script at install time).
echo "1f519a22e47187f70a1379a48ca604981c4fcf694f4e65b734aaa74a9fba3032  packaging/windows/vendor/PawnIO_setup.exe" \
  | sha256sum -c --quiet || erreur "packaging/windows/vendor/PawnIO_setup.exe : somme SHA-256 inattendue."
cp packaging/windows/vendor/PawnIO_setup.exe "$STAGE/"
# Text files with Windows line endings (Notepad).
sed 's/\r\{0,1\}$/\r/' LICENSE > "$STAGE/LICENSE.txt"
sed 's/\r\{0,1\}$/\r/' packaging/windows/LIRE-MOI-Windows.txt > "$STAGE/LIRE-MOI-Windows.txt"
for f in licenses/*; do
  sed 's/\r\{0,1\}$/\r/' "$f" > "$STAGE/licenses/$(basename "$f")"
done

SETUP="dist/TuxRGB-$VERSION-Windows-Setup.exe"
bleu "Installateur $SETUP"
makensis -V2 \
  -DVERSION="$VERSION" \
  -DVERSION_NUM="$BASE.0" \
  -DSTAGE="$ROOT/$STAGE" \
  -DOUTFILE="$ROOT/$SETUP" \
  packaging/windows/tuxrgb.nsi
(cd dist && sha256sum "$(basename "$SETUP")" > "$(basename "$SETUP").sha256")
ls -la "$SETUP"
echo "Terminé : $SETUP"
