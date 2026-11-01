#!/usr/bin/env bash
# Cross-compiles OpenRGB 1.1 for Windows (x86_64, MinGW-w64) from
# third_party/openrgb, with TuxRGB's patches (third_party/patches), so the
# Windows installer ships the same engine as the AppImage: one that sleeps
# between updates instead of checking every controller a thousand times a
# second.
#
# Everything is built from source, on Linux:
#   * Qt 5.15.16 LTS (qtbase only: core, gui, widgets), github.com/qt/qtbase
#   * Mbed TLS 3.2.1 (the version OpenRGB ships for Windows), github.com/Mbed-TLS/mbedtls
#   * OpenRGB itself (qmake, MinGW posix threads)
# libusb, hidapi and PawnIOLib are the prebuilt DLLs OpenRGB ships in
# third_party/openrgb/dependencies (plain C interfaces).
#
# Needs: mingw-w64 (g++-mingw-w64-x86-64-posix), cmake, ninja, make, git,
# a host g++ and Qt 6 lrelease (qt6-l10n-tools) for the translations.
#
# Output: build/openrgb-windows/ (OpenRGB.exe + DLLs + PawnIO modules),
# picked up by scripts/windows/build-windows.sh.
#
# Usage: scripts/windows/build-openrgb-windows.sh [qt|mbedtls|openrgb|all]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DEPS="$ROOT/build/win-deps"
OUT="$ROOT/build/openrgb-windows"
JOBS="${JOBS:-$(nproc)}"
QT_TAG="v5.15.16-lts-lgpl"
MBEDTLS_TAG="v3.2.1"
QT_PREFIX="$DEPS/qt5-mingw"
MBEDTLS_PREFIX="$DEPS/mbedtls-mingw"
TRIPLE="x86_64-w64-mingw32"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

# MinGW with posix threads (std::thread, std::mutex): wrappers named like the
# plain cross compiler, first in PATH, so Qt's mkspec and CMake find them.
TOOLS="$DEPS/toolchain"
mkdir -p "$TOOLS"
for t in gcc g++; do
    real="$(command -v "$TRIPLE-$t-posix")" || { echo "missing $TRIPLE-$t-posix" >&2; exit 1; }
    ln -sf "$real" "$TOOLS/$TRIPLE-$t"
done
for t in ar ranlib windres strip objdump nm dlltool; do
    ln -sf "$(command -v "$TRIPLE-$t")" "$TOOLS/$TRIPLE-$t"
done
export PATH="$TOOLS:$PATH"

build_qt() {
    if [ -x "$QT_PREFIX/bin/qmake" ]; then
        log "Qt: already built ($QT_PREFIX)"
        return
    fi
    [ -d "$DEPS/qtbase-src" ] ||
        git clone --depth 1 --branch "$QT_TAG" https://github.com/qt/qtbase "$DEPS/qtbase-src"
    rm -rf "$DEPS/qtbase-build"
    mkdir -p "$DEPS/qtbase-build"
    cd "$DEPS/qtbase-build"
    log "Qt: configure"
    "$DEPS/qtbase-src/configure" \
        -xplatform win32-g++ -device-option CROSS_COMPILE="$TRIPLE-" \
        -prefix "$QT_PREFIX" -opensource -confirm-license \
        -release -shared -strip \
        -nomake examples -nomake tests \
        -no-opengl -no-dbus -no-icu -no-openssl -no-fontconfig \
        -no-sql-sqlite -no-sql-odbc -no-sql-psql -no-sql-mysql \
        -qt-zlib -qt-pcre -qt-libpng -qt-libjpeg -qt-freetype -qt-harfbuzz \
        -silent
    log "Qt: build ($JOBS jobs)"
    make -j"$JOBS" > "$DEPS/qtbase-build.log" 2>&1 || { tail -40 "$DEPS/qtbase-build.log"; exit 1; }
    make install > /dev/null
    # Translations: Qt 6's lrelease writes the same .qm files.
    ln -sf "$(command -v lrelease 2>/dev/null || echo /usr/lib/qt6/bin/lrelease)" "$QT_PREFIX/bin/lrelease"
    log "Qt: installed in $QT_PREFIX"
}

build_mbedtls() {
    if [ -f "$MBEDTLS_PREFIX/lib/libmbedtls.a" ]; then
        log "Mbed TLS: already built"
        return
    fi
    [ -d "$DEPS/mbedtls-src" ] ||
        git clone --depth 1 --branch "$MBEDTLS_TAG" https://github.com/Mbed-TLS/mbedtls "$DEPS/mbedtls-src"
    rm -rf "$DEPS/mbedtls-build"
    cmake -S "$DEPS/mbedtls-src" -B "$DEPS/mbedtls-build" -G Ninja \
        -DCMAKE_SYSTEM_NAME=Windows \
        -DCMAKE_C_COMPILER="$TRIPLE-gcc" -DCMAKE_RC_COMPILER="$TRIPLE-windres" \
        -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$MBEDTLS_PREFIX" \
        -DENABLE_TESTING=OFF -DENABLE_PROGRAMS=OFF -DMBEDTLS_FATAL_WARNINGS=OFF \
        -DUSE_STATIC_MBEDTLS_LIBRARY=ON -DUSE_SHARED_MBEDTLS_LIBRARY=OFF > /dev/null
    cmake --build "$DEPS/mbedtls-build" -j"$JOBS" > /dev/null
    cmake --install "$DEPS/mbedtls-build" > /dev/null
    log "Mbed TLS: installed in $MBEDTLS_PREFIX"
}

build_openrgb() {
    local src="$ROOT/third_party/openrgb" build="$DEPS/openrgb-build" shim="$DEPS/openrgb-shim"
    local deps="$src/dependencies"
    [ -f "$src/OpenRGB.pro" ] || { echo "OpenRGB sources missing: run third_party/fetch-openrgb.sh" >&2; exit 1; }
    [ -x "$QT_PREFIX/bin/qmake" ] || build_qt
    [ -f "$MBEDTLS_PREFIX/lib/libmbedtls.a" ] || build_mbedtls

    # Windows headers spelled with capitals (<Windows.h>, <WinIoCtl.h>…):
    # Windows ignores case, MinGW's headers are lower case.
    # Written only when missing: the build depends on them, a new date
    # would recompile everything.
    local inc="/usr/$TRIPLE/include"
    mkdir -p "$shim"
    grep -rhoE '#[[:space:]]*include[[:space:]]*[<"][A-Za-z0-9_]+\.h[>"]' "$src" \
        --include=*.c --include=*.cpp --include=*.h |
        sed -E 's/.*[<"]([^>"]+)[>"]/\1/' | sort -u | while read -r h; do
            local lower
            lower="$(echo "$h" | tr '[:upper:]' '[:lower:]')"
            if [ "$lower" != "$h" ] && [ ! -e "$inc/$h" ] && [ -e "$inc/$lower" ] && [ ! -e "$shim/$h" ]; then
                echo "#include <$lower>" > "$shim/$h"
            fi
        done

    # libusb's MinGW DLL calls itself msys-usb-1.0.dll inside: linking to it
    # directly would make OpenRGB look for that name. Import library from
    # the .def OpenRGB ships instead.
    mkdir -p "$DEPS/libusb-import"
    "$TRIPLE-dlltool" -d "$deps/libusb-1.0.27/libusb-1.0.def" -D libusb-1.0.dll \
        -l "$DEPS/libusb-import/libusb-1.0.dll.a"

    mkdir -p "$build"
    cd "$build"
    log "OpenRGB: qmake"
    # OpenRGB.pro targets MSVC on Windows: the MSVC-only flag, the prebuilt
    # MSVC Mbed TLS and the MSVC copy step are replaced; the architecture,
    # read from the Visual Studio environment there, is given; comsupp.cpp
    # stands in for MSVC's comsupp.lib (see the file).
    "$QT_PREFIX/bin/qmake" "$src/OpenRGB.pro" -spec win32-g++ \
        CONFIG+=release QMAKE_TARGET.arch=x86_64 \
        -after \
        "QMAKE_CXXFLAGS -= /utf-8" \
        "QMAKE_EXTRA_TARGETS -= first copydata" \
        "INCLUDEPATH = $shim \$\$INCLUDEPATH" \
        "SOURCES += $ROOT/scripts/windows/openrgb-mingw/comsupp.cpp" \
        "LIBS = -L$MBEDTLS_PREFIX/lib -lmbedtls -lmbedx509 -lmbedcrypto \
               -L$DEPS/libusb-import -lusb-1.0 \
               -L$deps/hidapi-hotplug-win/x64 -lhidapi-hotplug \
               -L$deps/PawnIO -lPawnIOLib \
               -lws2_32 -liphlpapi -lcrypt32 -lwbemuuid -ladvapi32 -lsetupapi -lole32 -loleaut32 \
               -luuid -lwinhttp -lhid -lcfgmgr32 -lbcrypt -luser32 -lshell32 -lpowrprof -lversion" \
        > "$DEPS/openrgb-qmake.log" 2>&1 || { tail -30 "$DEPS/openrgb-qmake.log"; exit 1; }
    log "OpenRGB: build ($JOBS jobs, this takes a while)"
    # Always linked again: the libraries are not dependencies in the Makefile.
    rm -f "$build/release/OpenRGB.exe"
    make -k -j"$JOBS" > "$DEPS/openrgb-build.log" 2>&1 || {
        grep -E "error|Error" "$DEPS/openrgb-build.log" | head -40
        exit 1
    }

    log "OpenRGB: collecting into $OUT"
    rm -rf "$OUT"
    mkdir -p "$OUT/platforms" "$OUT/styles" "$OUT/imageformats" "$OUT/licenses"
    install -m644 "$build/release/OpenRGB.exe" "$OUT/OpenRGB.exe"
    "$TRIPLE-strip" "$OUT/OpenRGB.exe"
    for m in Core Gui Widgets; do cp "$QT_PREFIX/bin/Qt5$m.dll" "$OUT/"; done
    cp "$QT_PREFIX/plugins/platforms/qwindows.dll" "$OUT/platforms/"
    cp "$QT_PREFIX/plugins/styles/qwindowsvistastyle.dll" "$OUT/styles/" 2>/dev/null || true
    cp "$QT_PREFIX/plugins/imageformats/qico.dll" "$OUT/imageformats/" 2>/dev/null || true
    local gcclib
    gcclib="$(dirname "$("$TRIPLE-g++" -print-libgcc-file-name)")"
    cp "$gcclib/libstdc++-6.dll" "$gcclib/libgcc_s_seh-1.dll" "$OUT/"
    cp "$("$TRIPLE-g++" -print-file-name=libwinpthread-1.dll)" "$OUT/"
    "$TRIPLE-strip" --strip-unneeded "$OUT/libstdc++-6.dll" "$OUT/libgcc_s_seh-1.dll" "$OUT/libwinpthread-1.dll"
    cp "$deps/libusb-1.0.27/MinGW64/dll/libusb-1.0.dll" "$deps/hidapi-hotplug-win/x64/hidapi-hotplug.dll" \
       "$deps/PawnIO/PawnIOLib.dll" "$deps/PawnIO/modules/"*.bin "$OUT/"
    # Every DLL OpenRGB needs is either a Windows one or shipped here, but
    # the Visual C++ runtime of the prebuilt hidapi DLL OpenRGB ships (the
    # official OpenRGB build needs it too, for all of its files).
    local missing
    missing="$(for f in "$OUT"/*.exe "$OUT"/*.dll "$OUT"/*/*.dll; do
        "$TRIPLE-objdump" -p "$f" | awk '/DLL Name/ {print tolower($3)}'
    done | sort -u | while read -r d; do
        ls "$OUT" | tr '[:upper:]' '[:lower:]' | grep -qx "$d" && continue
        [ -f "/usr/$TRIPLE/lib/lib${d%.dll}.a" ] || [ -f "/usr/$TRIPLE/lib/lib$(echo "${d%.dll}" | sed 's/^api-ms-.*/kernel32/').a" ] || echo "$d"
    done | grep -vx 'vcruntime140.dll' || true)"
    [ -z "$missing" ] || { echo "DLLs neither from Windows nor shipped: $missing" >&2; exit 1; }
    # Licences (text files with Windows line endings).
    lic() { sed 's/\r\{0,1\}$/\r/' "$1" > "$OUT/licenses/$2"; }
    lic "$src/LICENSE" "OpenRGB-GPL-2.0.txt"
    lic "$DEPS/qtbase-src/LICENSE.LGPLv3" "Qt-LGPL-3.0.txt"
    lic "$DEPS/qtbase-src/LICENSE.GPL2" "Qt-GPL-2.0.txt"
    lic "$DEPS/mbedtls-src/LICENSE" "MbedTLS-Apache-2.0.txt"
    lic /usr/share/common-licenses/LGPL-2.1 "libusb-LGPL-2.1.txt"
    lic "$ROOT/licenses/GPL-3.0.txt" "GCC-runtime-GPL-3.0.txt"
    {
        echo "OpenRGB 1.1 for Windows, built by TuxRGB from source with MinGW-w64:"
        echo "  OpenRGB          GPL-2.0, $(cat "$ROOT/third_party/openrgb.commit"), with TuxRGB's patches"
        echo "                   (third_party/patches in the TuxRGB sources)"
        echo "  Qt $QT_TAG  LGPL-3.0 (qtbase: Core, Gui, Widgets), unmodified"
        echo "  Mbed TLS $MBEDTLS_TAG  Apache-2.0, unmodified"
        echo "  libusb 1.0.27, hidapi (hotplug), PawnIOLib: the DLLs OpenRGB ships"
        echo "  libstdc++, libgcc: GPL-3.0 with the GCC Runtime Library Exception;"
        echo "  winpthreads: MIT (MinGW-w64)"
        echo "Sources: https://github.com/aurelienchartier29-netizen/TuxRGB (third_party/),"
        echo "https://codeberg.org/OpenRGB/OpenRGB, https://github.com/qt/qtbase,"
        echo "https://github.com/Mbed-TLS/mbedtls."
    } | sed 's/$/\r/' > "$OUT/licenses/LISEZ-MOI.txt"
    log "OpenRGB: $(du -sh "$OUT" | cut -f1) in $OUT"
}

case "${1:-all}" in
    qt) build_qt ;;
    mbedtls) build_mbedtls ;;
    openrgb) build_openrgb ;;
    all) build_qt; build_mbedtls; build_openrgb ;;
    *) echo "usage: $0 [qt|mbedtls|openrgb|all]" >&2; exit 2 ;;
esac
