#!/usr/bin/env python3
"""Packs the TuxRGB project into TuxRGB_FullProject.zip.

Includes sources, vendored/third-party sources, packaging, docs and the
prebuilt AppImage in dist/. Build outputs (target/, build/) are excluded.

Usage: python3 scripts/make-archive.py [output.zip]
"""

import os
import pathlib
import stat
import sys
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT.parent / "TuxRGB_FullProject.zip"
EXCLUDED_DIRS = {"target", "build", "build-flatpak", ".flatpak-builder", "__pycache__", ".git"}
PREFIX = "TuxRGB"
# Already-compressed payloads are stored as is.
STORED = {".AppImage", ".png", ".jpg", ".ttf", ".zip", ".gz", ".bz2", ".xz"}


def main():
    count = 0
    with zipfile.ZipFile(OUT, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for dirpath, dirnames, filenames in os.walk(ROOT):
            rel_dir = pathlib.Path(dirpath).relative_to(ROOT)
            dirnames[:] = sorted(d for d in dirnames if d not in EXCLUDED_DIRS)
            for name in sorted(filenames):
                path = pathlib.Path(dirpath) / name
                if path.is_symlink() and not path.exists():
                    continue
                arcname = f"{PREFIX}/{rel_dir / name}" if str(rel_dir) != "." else f"{PREFIX}/{name}"
                info = zipfile.ZipInfo.from_file(path, arcname)
                # Keep the executable bit (scripts, AppImage) for unzip on Linux.
                mode = path.stat().st_mode
                info.external_attr = (stat.S_IFREG | (mode & 0o777)) << 16
                compress = zipfile.ZIP_STORED if path.suffix in STORED else zipfile.ZIP_DEFLATED
                info.compress_type = compress
                with open(path, "rb") as f:
                    z.writestr(info, f.read(), compress_type=compress, compresslevel=9)
                count += 1
    size = OUT.stat().st_size / (1024 * 1024)
    print(f"{OUT}: {count} files, {size:.1f} MiB")


if __name__ == "__main__":
    main()
