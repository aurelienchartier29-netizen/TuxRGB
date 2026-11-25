#!/usr/bin/env python3
"""Losslessly shrinks the README/Flathub screenshots in docs/screenshots.

`tuxrgb --screenshot-tour <dir>` writes plain, uncompressed RGBA PNGs
(about 4.5 MB each). Run this after copying new ones into docs/screenshots:
opaque images become RGB and are recompressed (about 50 times smaller); the
pixels are checked to be identical before a file is replaced.

Usage: python3 scripts/optimize-screenshots.py [directory]
"""

import pathlib
import sys

from PIL import Image, ImageChops

ROOT = pathlib.Path(__file__).resolve().parent.parent
FOLDER = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "docs" / "screenshots"


def main():
    before_total = after_total = 0
    for path in sorted(FOLDER.rglob("*.png")):
        image = Image.open(path)
        image.load()
        opaque = image.mode != "RGBA" or image.getchannel("A").getextrema() == (255, 255)
        result = image.convert("RGB") if opaque else image
        tmp = path.with_name(path.name + ".tmp")
        result.save(tmp, format="PNG", optimize=True)
        check = Image.open(tmp)
        same = ImageChops.difference(check.convert("RGBA"), image.convert("RGBA")).getbbox() is None
        before, after = path.stat().st_size, tmp.stat().st_size
        if same and after < before:
            tmp.replace(path)
        else:
            tmp.unlink()
            after = before
        before_total += before
        after_total += after
    print(f"{FOLDER}: {before_total / 1e6:.2f} MB -> {after_total / 1e6:.2f} MB")


if __name__ == "__main__":
    main()
