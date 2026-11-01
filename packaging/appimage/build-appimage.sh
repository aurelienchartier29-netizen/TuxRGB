#!/usr/bin/env bash
# Builds a self-contained TuxRGB AppImage (x86_64):
#   * TuxRGB (Rust, release) + the static privileged helper,
#   * OpenRGB (latest release, see third_party/README.md) compiled from
#     third_party/openrgb with its Qt 6, hidapi,
#     libusb and mbedtls libraries bundled next to it (RUNPATH=$ORIGIN/lib),
#   * desktop file, icons, AppStream metadata, licences and OpenRGB sources notice.
#
# Requirements (Debian/Ubuntu names): cargo/rustc ≥ 1.88, qt6-base-dev,
# qt6-l10n-tools, libhidapi-dev, libusb-1.0-0-dev, libmbedtls-dev,
# libpipewire-0.3-dev, clang, pkg-config, patchelf, file, curl.
# Build on the OLDEST distribution you want to support (glibc baseline);
# the reference build uses Ubuntu 24.04 (glibc 2.39) → runs on Fedora 40+,
# Bazzite, Aurora, Ubuntu 24.04+, Arch, openSUSE Tumbleweed…
#
# Usage:
#   packaging/appimage/build-appimage.sh                 # full build
#   OPENRGB_BIN=/path/to/openrgb packaging/appimage/build-appimage.sh   # reuse a build
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ARCH="${ARCH:-x86_64}"
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)"
WORK="${WORK:-$ROOT/build/appimage}"
APPDIR="$WORK/AppDir"
OUT_DIR="${OUT_DIR:-$ROOT/dist}"
OUT="$OUT_DIR/TuxRGB-$VERSION-$ARCH.AppImage"
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
JOBS="${JOBS:-$(nproc)}"

log() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
need() { command -v "$1" >/dev/null || { echo "missing tool: $1" >&2; exit 1; }; }
for t in cargo patchelf ldd readelf curl; do need "$t"; done

rm -rf "$APPDIR"
mkdir -p "$APPDIR"/usr/{bin,libexec/tuxrgb,lib/tuxrgb/openrgb/lib,share/{applications,metainfo,licenses/tuxrgb,polkit-1/actions}} "$OUT_DIR"

# ---------------------------------------------------------------- OpenRGB
if [[ -z "${OPENRGB_BIN:-}" ]]; then
  SRC="$ROOT/third_party/openrgb"
  [[ -f "$SRC/OpenRGB.pro" ]] || { echo "OpenRGB sources missing: run third_party/fetch-openrgb.sh" >&2; exit 1; }
  QMAKE="$(command -v qmake6 || command -v qmake-qt6 || true)"
  [[ -n "$QMAKE" ]] || { echo "qmake6 not found (install Qt 6 development files)" >&2; exit 1; }
  log "Building OpenRGB from $SRC ($JOBS jobs, this takes a while)"
  mkdir -p "$WORK/openrgb-build"
  ( cd "$WORK/openrgb-build" && "$QMAKE" "$SRC/OpenRGB.pro" CONFIG+=release >/dev/null && make -j"$JOBS" )
  OPENRGB_BIN="$WORK/openrgb-build/openrgb"
fi
log "Bundling OpenRGB from $OPENRGB_BIN"
# The udev rules are compiled into TuxRGB (shown instantly to pkexec): they
# must come from this exact OpenRGB build.
log "Generating udev rules from the bundled OpenRGB"
QT_QPA_PLATFORM=offscreen "$OPENRGB_BIN" --print-udev-rules > "$WORK/60-openrgb.rules.new"
if [[ "$(grep -c 'TAG+="uaccess"' "$WORK/60-openrgb.rules.new")" -lt 10 ]]; then
  echo "OpenRGB did not print udev rules" >&2; exit 1
fi
mv "$WORK/60-openrgb.rules.new" "$ROOT/data/udev/60-openrgb.rules"

# ---------------------------------------------------------------- TuxRGB
log "Building TuxRGB $VERSION (release)"
cargo build --manifest-path "$ROOT/Cargo.toml" --release --locked -p tuxrgb
log "Building the privileged helper (static glibc, runs as root on any distro)"
RUSTFLAGS="-C target-feature=+crt-static" \
  cargo build --manifest-path "$ROOT/Cargo.toml" --release --locked -p tuxrgb-helper \
  --target "$ARCH-unknown-linux-gnu"
install -Dm755 "$ROOT/target/release/tuxrgb" "$APPDIR/usr/bin/tuxrgb"
install -Dm755 "$ROOT/target/$ARCH-unknown-linux-gnu/release/tuxrgb-helper" "$APPDIR/usr/libexec/tuxrgb/tuxrgb-helper"

install -Dm755 "$OPENRGB_BIN" "$APPDIR/usr/lib/tuxrgb/openrgb/openrgb"

# Libraries the host must provide (driver / ABI sensitive, or universally
# present). Everything else OpenRGB links against is bundled.
EXCLUDE_RE='^(ld-linux|libc\.so|libm\.so|libdl\.so|librt\.so|libpthread\.so|libresolv\.so|libutil\.so|libstdc\+\+\.so|libgcc_s\.so|libgomp\.so|libGL|libEGL|libGLX|libGLdispatch|libOpenGL|libdrm|libgbm|libX|libxcb|libwayland|libxkbcommon|libfontconfig|libfreetype|libharfbuzz|libgraphite2|libpng16|libbrotli|libbz2|libz\.so|libzstd|liblzma|liblz4|libpcre2-8|libglib-2|libgobject-2|libgio-2|libgmodule-2|libdbus-1|libsystemd|libudev|libgcrypt|libgpg-error|libbsd|libmd\.so|libexpat|libcap|libmount|libblkid|libselinux|libffi)'

# Walks DT_NEEDED entries (not the full `ldd` closure), so libraries only
# reachable through host libraries (e.g. libsystemd → libgcrypt) are never
# bundled: the host's own versions must stay consistent with each other.
bundle_deps() {
  local bin="$1" dest="$2" name path
  for name in $(readelf -d "$bin" | awk -F'[][]' '/\(NEEDED\)/ {print $2}'); do
    [[ "$name" =~ $EXCLUDE_RE ]] && continue
    [[ -e "$dest/$name" ]] && continue
    path="$(ldd "$bin" | awk -v n="$name" '$1 == n {print $3}')"
    if [[ -z "$path" || ! -e "$path" ]]; then
      echo "warning: $name (needed by $(basename "$bin")) not found" >&2
      continue
    fi
    cp -L "$path" "$dest/$name"
    chmod 644 "$dest/$name"
    patchelf --set-rpath '$ORIGIN' "$dest/$name" 2>/dev/null || true
    bundle_deps "$dest/$name" "$dest"
  done
}
bundle_deps "$APPDIR/usr/lib/tuxrgb/openrgb/openrgb" "$APPDIR/usr/lib/tuxrgb/openrgb/lib"
patchelf --set-rpath '$ORIGIN/lib' "$APPDIR/usr/lib/tuxrgb/openrgb/openrgb"
log "Bundled $(ls "$APPDIR/usr/lib/tuxrgb/openrgb/lib" | wc -l) libraries for OpenRGB"

# Smoke test: the bundled server must start and report its version.
"$APPDIR/usr/lib/tuxrgb/openrgb/openrgb" --version | head -2

# ---------------------------------------------------------------- data
install -m644 "$ROOT/data/io.github.tuxrgb.TuxRGB.desktop" "$APPDIR/usr/share/applications/"
install -m644 "$ROOT/data/io.github.tuxrgb.TuxRGB.metainfo.xml" "$APPDIR/usr/share/metainfo/"
cp "$ROOT/data/io.github.tuxrgb.TuxRGB.metainfo.xml" "$APPDIR/usr/share/metainfo/io.github.tuxrgb.TuxRGB.appdata.xml"
install -m644 "$ROOT/data/io.github.tuxrgb.TuxRGB.policy" "$APPDIR/usr/share/polkit-1/actions/"
cp -r "$ROOT/data/icons" "$APPDIR/usr/share/"
install -m644 "$ROOT/LICENSE" "$APPDIR/usr/share/licenses/tuxrgb/LICENSE"
cp "$ROOT"/licenses/*.txt "$APPDIR/usr/share/licenses/tuxrgb/"
install -m644 "$ROOT/CREDITS.md" "$APPDIR/usr/share/licenses/tuxrgb/CREDITS.md"
cat > "$APPDIR/usr/share/licenses/tuxrgb/OPENRGB-SOURCE.txt" <<EOF
This AppImage contains OpenRGB (GPL-2.0-or-later), upstream commit
$(cat "$ROOT/third_party/openrgb.commit" 2>/dev/null || echo unknown) (tag release_1.1).
The complete corresponding source code is distributed with the TuxRGB project
archive (third_party/openrgb) and is available at
https://gitlab.com/CalcProgrammer1/OpenRGB
Bundled Qt 6 libraries are LGPL-3.0; their sources are available from your
distribution or https://download.qt.io/.
EOF

# AppImage root.
install -m755 "$ROOT/packaging/appimage/AppRun" "$APPDIR/AppRun"
cp "$ROOT/data/io.github.tuxrgb.TuxRGB.desktop" "$APPDIR/"
cp "$ROOT/data/icons/hicolor/256x256/apps/io.github.tuxrgb.TuxRGB.png" "$APPDIR/io.github.tuxrgb.TuxRGB.png"
ln -sf io.github.tuxrgb.TuxRGB.png "$APPDIR/.DirIcon"

# ---------------------------------------------------------------- pack
TOOL="$WORK/appimagetool-$ARCH.AppImage"
if [[ ! -x "$TOOL" ]]; then
  log "Downloading appimagetool"
  curl -fL --retry 3 -o "$TOOL" "$APPIMAGETOOL_URL"
  chmod +x "$TOOL"
fi
log "Packing $OUT"
ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream "$APPDIR" "$OUT"
chmod +x "$OUT"
( cd "$OUT_DIR" && sha256sum "$(basename "$OUT")" > "$(basename "$OUT").sha256" )
install -m755 "$ROOT/packaging/run-tuxrgb.sh" "$OUT_DIR/run-tuxrgb.sh"
ls -lh "$OUT"
log "Done. Run it with: $OUT   (or dist/run-tuxrgb.sh on Bazzite / Aurora)"
