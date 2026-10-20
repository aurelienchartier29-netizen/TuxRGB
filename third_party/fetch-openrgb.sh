#!/usr/bin/env bash
# Replaces third_party/openrgb with a clean copy of OpenRGB.
#
#   third_party/fetch-openrgb.sh               # pinned commit (see below)
#   third_party/fetch-openrgb.sh master        # latest development tree
#   third_party/fetch-openrgb.sh release_1.1   # a release tag
#
# TuxRGB ships the latest OpenRGB release (1.1, 2026-10-16), which already
# contains every device added to master since 1.0. Between releases, master
# can be used to pick up newer hardware. After updating, rebuild the AppImage
# (packaging/appimage/build-appimage.sh): it also regenerates
# data/udev/60-openrgb.rules from the new binary.
set -euo pipefail
PINNED_COMMIT="8e16de27858cf2be6af54a33ce003df74cb9fd3e" # tag release_1.1, 2026-10-16
REF="${1:-$PINNED_COMMIT}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

clone() {
  git clone --quiet --filter=blob:none "$1" "$TMP/openrgb"
}
clone "${OPENRGB_GIT:-https://gitlab.com/CalcProgrammer1/OpenRGB.git}" \
  || clone https://github.com/CalcProgrammer1/OpenRGB.git
git -C "$TMP/openrgb" checkout --quiet "$REF"
COMMIT="$(git -C "$TMP/openrgb" rev-parse HEAD)"
DATE="$(git -C "$TMP/openrgb" log -1 --format=%cs)"
rm -rf "$HERE/openrgb"
mkdir -p "$HERE/openrgb"
git -C "$TMP/openrgb" archive HEAD | tar -x -C "$HERE/openrgb"
echo "$COMMIT" > "$HERE/openrgb.commit"
echo "OpenRGB $REF → $COMMIT ($DATE) → $HERE/openrgb"
echo "Update OPENRGB_VERSION in crates/tuxrgb-openrgb/src/lib.rs and third_party/README.md."
