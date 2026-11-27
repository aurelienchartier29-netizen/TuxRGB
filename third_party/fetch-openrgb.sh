#!/usr/bin/env bash
# Replaces third_party/openrgb with a clean copy of OpenRGB.
#
#   third_party/fetch-openrgb.sh               # pinned commit (see below)
#   third_party/fetch-openrgb.sh master        # latest development tree
#   third_party/fetch-openrgb.sh release_1.1   # a release tag
#
# TuxRGB 4.0 ships OpenRGB master of 2026-11-24: the 1.1 release (2026-10-16)
# plus 88 commits of new devices and fixes, same SDK protocol (v6). Use
# `release_1.1` to go back to the release. After updating, rebuild the AppImage
# (packaging/appimage/build-appimage.sh): it also regenerates
# data/udev/60-openrgb.rules from the new binary.
set -euo pipefail
PINNED_COMMIT="f899bc88bb38bc47b6940e45cd1da50567e026ee" # master, 2026-11-24 (release_1.1 + 88)
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
# TuxRGB's patches (see patches/ and README.md).
for patch in "$HERE"/patches/*.patch; do
  [ -e "$patch" ] || continue
  patch -d "$HERE/openrgb" -p1 --forward --quiet < "$patch" \
    || echo "warning: $(basename "$patch") does not apply to $REF, check it by hand" >&2
done
echo "OpenRGB $REF → $COMMIT ($DATE) → $HERE/openrgb"
echo "Update OPENRGB_VERSION in crates/tuxrgb-openrgb/src/lib.rs and third_party/README.md."
