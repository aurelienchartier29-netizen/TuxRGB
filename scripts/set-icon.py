#!/usr/bin/env python3
"""Replace the TuxRGB icon everywhere from a single image.

    python3 scripts/set-icon.py mon-icone.svg     # recommended (sharp at every size)
    python3 scripts/set-icon.py mon-icone.png     # square PNG, 512x512 or more

Regenerates:
  * data/icons/hicolor/*/apps/io.github.tuxrgb.TuxRGB.png (menu, AppImage, Flatpak)
  * data/icons/hicolor/scalable/apps/io.github.tuxrgb.TuxRGB.svg (SVG input only)
  * assets/tuxrgb.svg (SVG input only, master copy)
  * assets/tuxrgb-256.png               (README) and assets/tuxrgb-icon-source.png (master)
  * crates/tuxrgb/assets/icon-128.rgba  (window icon)
  * crates/tuxrgb/assets/logo-256.rgba  (logo of the navigation rail / wizard, sharp on HiDPI)
  * crates/tuxrgb/assets/tray-{22,32,48,64,128}.argb (notification area, 1x to 2x)

The crates/tuxrgb/assets files and the 32/64/256 hicolor PNGs are compiled
into the program: rebuild afterwards (packaging/appimage/build-appimage.sh).

Needs Pillow (python3-pillow) and, for SVG, rsvg-convert (librsvg2-tools).
"""

import io
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
APP_ID = "io.github.tuxrgb.TuxRGB"
HICOLOR = [16, 22, 24, 32, 48, 64, 128, 256, 512]


def die(msg: str) -> None:
    print(f"erreur : {msg}", file=sys.stderr)
    sys.exit(1)


try:
    from PIL import Image
except ImportError:
    die("Pillow manque : installez python3-pillow (ou pip install --user pillow)")


def load(src: Path, size: int) -> "Image.Image":
    """Returns the icon as a size x size RGBA image."""
    if src.suffix.lower() == ".svg":
        if not shutil.which("rsvg-convert"):
            die("rsvg-convert manque (paquet librsvg2-tools / librsvg2-bin)")
        png = subprocess.run(
            ["rsvg-convert", "-w", str(size), "-h", str(size), "--keep-aspect-ratio", str(src)],
            check=True,
            capture_output=True,
        ).stdout
        img = Image.open(io.BytesIO(png)).convert("RGBA")
    else:
        img = Image.open(src).convert("RGBA")
    if img.size != (size, size):
        # Centre a non-square image on a transparent square, then scale.
        side = max(img.size)
        square = Image.new("RGBA", (side, side), (0, 0, 0, 0))
        square.paste(img, ((side - img.width) // 2, (side - img.height) // 2))
        img = square.resize((size, size), Image.LANCZOS)
    return img


def rgba_bytes(img: "Image.Image") -> bytes:
    return img.tobytes()  # straight RGBA, row-major (iced)


def argb_bytes(img: "Image.Image") -> bytes:
    # StatusNotifierItem pixmaps: ARGB32 in network byte order.
    rgba = img.tobytes()
    out = bytearray(len(rgba))
    out[0::4] = rgba[3::4]  # A
    out[1::4] = rgba[0::4]  # R
    out[2::4] = rgba[1::4]  # G
    out[3::4] = rgba[2::4]  # B
    return bytes(out)


def main() -> None:
    if len(sys.argv) != 2:
        print(__doc__)
        sys.exit(2)
    src = Path(sys.argv[1]).expanduser().resolve()
    if not src.is_file():
        die(f"fichier introuvable : {src}")
    if src.suffix.lower() != ".svg":
        w, h = Image.open(src).size
        if min(w, h) < 512:
            print(f"attention : image {w}x{h}, elle sera floue en grand (512x512 conseillé)")

    for size in HICOLOR:
        dst = ROOT / f"data/icons/hicolor/{size}x{size}/apps/{APP_ID}.png"
        dst.parent.mkdir(parents=True, exist_ok=True)
        load(src, size).save(dst, optimize=True)

    scalable = ROOT / f"data/icons/hicolor/scalable/apps/{APP_ID}.svg"
    if src.suffix.lower() == ".svg":
        shutil.copyfile(src, scalable)
        if src != (ROOT / "assets/tuxrgb.svg").resolve():
            shutil.copyfile(src, ROOT / "assets/tuxrgb.svg")
    elif scalable.exists():
        # An old SVG would win over the new PNGs in icon themes.
        scalable.unlink()
        print("note : l'ancienne icône SVG a été retirée (seules les PNG seront utilisées)")

    if src.suffix.lower() != ".svg":
        master = ROOT / "assets/tuxrgb-icon-source.png"
        if src != master.resolve():
            Image.open(src).convert("RGBA").save(master, optimize=True)
        # The previous SVG master would no longer match the icon.
        (ROOT / "assets/tuxrgb.svg").unlink(missing_ok=True)
    load(src, 256).save(ROOT / "assets/tuxrgb-256.png", optimize=True)

    assets = ROOT / "crates/tuxrgb/assets"
    for old in ("icon-64.rgba", "logo-112.rgba"):
        (assets / old).unlink(missing_ok=True)
    for old in assets.glob("tray-*.argb"):
        old.unlink()
    (assets / "icon-128.rgba").write_bytes(rgba_bytes(load(src, 128)))
    (assets / "logo-256.rgba").write_bytes(rgba_bytes(load(src, 256)))
    for size in (22, 32, 48, 64, 128):
        (assets / f"tray-{size}.argb").write_bytes(argb_bytes(load(src, size)))

    print("Icône remplacée. Reconstruisez l'AppImage : packaging/appimage/build-appimage.sh")


if __name__ == "__main__":
    main()
