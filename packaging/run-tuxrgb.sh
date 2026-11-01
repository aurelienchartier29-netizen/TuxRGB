#!/usr/bin/env bash
# TuxRGB launcher for immutable distributions (Bazzite, Aurora, Bluefin,
# Fedora Atomic…) – and any other distro. Nothing is compiled or layered:
# the prebuilt AppImage next to this script is used as is.
#
#   ./run-tuxrgb.sh                 run TuxRGB (extra arguments are passed on)
#   ./run-tuxrgb.sh --install       copy to ~/Applications + add a menu entry
#   ./run-tuxrgb.sh --uninstall     remove the menu entry and the copied AppImage
#   ./run-tuxrgb.sh --check         show what this system provides
#
# The first launch opens TuxRGB's setup wizard, which installs the OpenRGB
# udev rules and I2C configuration in /etc through Polkit (your password is
# asked by KDE/GNOME). /etc stays writable on rpm-ostree systems, so no
# reboot or `rpm-ostree install` is needed (except if a kernel parameter like
# acpi_enforce_resources=lax is required for your motherboard, see README).
set -euo pipefail

APP_ID="io.github.tuxrgb.TuxRGB"
HERE="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)"
INSTALL_DIR="${TUXRGB_INSTALL_DIR:-$HOME/Applications}"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"

find_appimage() {
  local candidates=("$HERE"/TuxRGB-*.AppImage "$INSTALL_DIR/TuxRGB.AppImage")
  for f in "${candidates[@]}"; do
    [[ -f "$f" ]] && { echo "$f"; return 0; }
  done
  echo "TuxRGB AppImage not found next to $0" >&2
  return 1
}

fuse_ok() {
  [[ -e /dev/fuse ]] || return 1
  command -v fusermount3 >/dev/null 2>&1 || command -v fusermount >/dev/null 2>&1
}

run() {
  local img
  img="$(find_appimage)"
  chmod +x "$img" 2>/dev/null || true
  if ! fuse_ok; then
    # No FUSE (some containers / minimal installs): unpack on the fly.
    export APPIMAGE_EXTRACT_AND_RUN=1
  fi
  if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "Warning: no Wayland session detected – TuxRGB is a native Wayland application." >&2
  fi
  exec "$img" "$@"
}

install_app() {
  local img dst
  img="$(find_appimage)"
  mkdir -p "$INSTALL_DIR" "$DATA_HOME/applications" "$DATA_HOME/icons/hicolor/256x256/apps"
  dst="$INSTALL_DIR/TuxRGB.AppImage"
  if [[ "$(readlink -f "$img")" != "$(readlink -f "$dst")" ]]; then
    cp -f "$img" "$dst"
  fi
  chmod +x "$dst"
  # Icon: extract it from the AppImage.
  local tmp
  tmp="$(mktemp -d)"
  ( cd "$tmp" && APPIMAGE_EXTRACT_AND_RUN=1 "$dst" --appimage-extract "$APP_ID.png" >/dev/null 2>&1 || true )
  if [[ -f "$tmp/squashfs-root/$APP_ID.png" ]]; then
    cp -f "$(readlink -f "$tmp/squashfs-root/$APP_ID.png")" "$DATA_HOME/icons/hicolor/256x256/apps/$APP_ID.png" 2>/dev/null \
      || cp -f "$tmp/squashfs-root/$APP_ID.png" "$DATA_HOME/icons/hicolor/256x256/apps/$APP_ID.png"
  fi
  rm -rf "$tmp"
  cat > "$DATA_HOME/applications/$APP_ID.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=TuxRGB
GenericName=Contrôle de l'éclairage RGB
Comment=Pilotez l'éclairage RGB de tout votre PC
Exec="$dst"
Icon=$APP_ID
Terminal=false
Categories=Settings;HardwareSettings;
Keywords=RGB;LED;OpenRGB;éclairage;
SingleMainWindow=true
StartupWMClass=$APP_ID
Actions=Minimized;ResetHardware;

[Desktop Action Minimized]
Name=Démarrer dans la zone de notification
Exec="$dst" --minimized

[Desktop Action ResetHardware]
Name=Réinitialiser l'accès matériel
Exec="$dst" --reset-hardware
EOF
  command -v update-desktop-database >/dev/null && update-desktop-database "$DATA_HOME/applications" >/dev/null 2>&1 || true
  command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$DATA_HOME/icons/hicolor" >/dev/null 2>&1 || true
  echo "TuxRGB installed: $dst"
  echo "It is now in your application menu (KDE: Paramètres / Matériel)."
}

uninstall_app() {
  rm -f "$DATA_HOME/applications/$APP_ID.desktop" \
        "$DATA_HOME/icons/hicolor/256x256/apps/$APP_ID.png" \
        "$INSTALL_DIR/TuxRGB.AppImage" \
        "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/$APP_ID.desktop"
  echo "TuxRGB removed. Settings are kept in ~/.config/tuxrgb (delete it to reset everything)."
  echo "To remove the hardware access rules first: TuxRGB → Réglages → Réinitialiser les permissions."
}

check() {
  echo "Session : ${XDG_SESSION_TYPE:-?} / ${XDG_CURRENT_DESKTOP:-?}"
  echo "Wayland : ${WAYLAND_DISPLAY:-absent}"
  if fuse_ok; then echo "FUSE    : ok"; else echo "FUSE    : absent (extract-and-run will be used)"; fi
  if command -v pkexec >/dev/null; then echo "Polkit  : pkexec ok"; else echo "Polkit  : pkexec missing"; fi
  if [[ -f /run/ostree-booted ]]; then echo "System  : immutable (rpm-ostree / bootc)"; fi
  for f in /etc/udev/rules.d/60-openrgb.rules /usr/lib/udev/rules.d/60-openrgb.rules; do
    [[ -f "$f" ]] && echo "udev    : $f"
  done
  [[ -d /sys/module/i2c_dev ]] && echo "i2c-dev : loaded" || echo "i2c-dev : not loaded"
  ldd --version 2>/dev/null | head -1 | sed 's/^/glibc   : /'
}

case "${1:-}" in
  --install) install_app ;;
  --uninstall) uninstall_app ;;
  --check) check ;;
  *) run "$@" ;;
esac
