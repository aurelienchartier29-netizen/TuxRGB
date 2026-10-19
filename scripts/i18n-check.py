#!/usr/bin/env python3
"""Lists the French interface strings that have no English translation.

TuxRGB's source strings are French (they are the translation keys, see
crates/tuxrgb-core/src/i18n). This script extracts every string literal of
the interface code, keeps those that look like user-facing text and prints
the ones missing from crates/tuxrgb-core/src/i18n/en.rs.

    python3 scripts/i18n-check.py            # missing keys
    python3 scripts/i18n-check.py --all      # every extracted key
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCES = [
    "crates/tuxrgb/src/app.rs",
    "crates/tuxrgb/src/desktop.rs",
    "crates/tuxrgb/src/tray.rs",
    "crates/tuxrgb/src/main.rs",
    "crates/tuxrgb/src/config.rs",
    "crates/tuxrgb/src/credits.rs",
    "crates/tuxrgb/src/services.rs",
    "crates/tuxrgb/src/privileges/diagnostics.rs",
    "crates/tuxrgb/src/privileges/report.rs",
    "crates/tuxrgb/src/privileges/runner.rs",
    "crates/tuxrgb/src/privileges/mod.rs",
    "crates/tuxrgb/src/paths.rs",
    "crates/tuxrgb/src/theme/windows.rs",
    "crates/tuxrgb/src/privileges/diagnostics_windows.rs",
    "crates/tuxrgb/src/privileges/report_windows.rs",
    "crates/tuxrgb/src/privileges/runner_windows.rs",
    "crates/tuxrgb-core/src/effects/mod.rs",
    "crates/tuxrgb-core/src/device.rs",
] + sorted(str(p.relative_to(ROOT)) for p in (ROOT / "crates/tuxrgb/src/ui").glob("*.rs")) \
  + sorted(str(p.relative_to(ROOT)) for p in (ROOT / "crates/tuxrgb/src/win").glob("*.rs"))

SKIP_CONTEXT = re.compile(
    r"(tracing::\w+!|debug!|info!|warn!|error!|trace!|\.expect\(|include_\w+!|env!|"
    r"concat!|#\[error|Font::with_name|icon_name:|application_id|\.arg\(|\.args\(|"
    r"Command::new|write_png|join\(\"|with_extension|PathBuf::from|Path::new|starts_with|"
    r"strip_prefix|contains\(|var\(|var_os\(|key: |eq_ignore|split|find\(|matches!)"
)


def unescape(body: str) -> str:
    out, i = [], 0
    while i < len(body):
        c = body[i]
        if c == "\\" and i + 1 < len(body):
            n = body[i + 1]
            if n == "\n":
                i += 2
                while i < len(body) and body[i] in " \t":
                    i += 1
                continue
            if n == "u" and body[i + 2 : i + 3] == "{":
                end = body.index("}", i)
                out.append(chr(int(body[i + 3 : end], 16)))
                i = end + 1
                continue
            out.append({"n": "\n", "t": "\t", '"': '"', "\\": "\\", "'": "'", "0": "\0"}.get(n, n))
            i += 2
            continue
        out.append(c)
        i += 1
    return "".join(out)


def literals(text: str):
    # Drop test modules.
    cut = text.find("#[cfg(test)]")
    if cut >= 0:
        text = text[:cut]
    i = 0
    n = len(text)
    while i < n:
        c = text[i]
        if text.startswith("//", i):
            j = text.find("\n", i)
            i = n if j < 0 else j
            continue
        if c == "r" and i + 1 < n and text[i + 1] in '#"' and (i == 0 or not text[i - 1].isalnum()):
            j = i + 1
            hashes = 0
            while j < n and text[j] == "#":
                hashes += 1
                j += 1
            if j < n and text[j] == '"':
                end = text.find('"' + "#" * hashes, j + 1)
                i = n if end < 0 else end + 1 + hashes
                continue
        if c == "'" and i + 2 < n:
            # char literal ('a', '\n', '\u{..}') – skip; lifetimes fall through
            m = re.match(r"'(\\u\{[0-9a-fA-F]+\}|\\.|[^\\'])'", text[i:])
            if m:
                i += m.end()
                continue
        if c == '"':
            j = i + 1
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == '"':
                    break
                j += 1
            line_start = text.rfind("\n", 0, i) + 1
            context = text[max(line_start - 200, 0) : i]
            yield unescape(text[i + 1 : j]), context
            i = j + 1
            continue
        i += 1


def user_facing(s: str, context: str) -> bool:
    if len(s) < 2 or not re.search(r"[A-Za-zÀ-ÿ]", s):
        return False
    tail = context.rsplit("\n", 2)[-1]
    if SKIP_CONTEXT.search(tail):
        return False
    if re.fullmatch(r"[a-z0-9_.\-/:]+", s):  # identifiers, keys, paths
        return False
    if s.startswith(("/", "--", "#", "org.", "io.", "http", "%", "{:", "0x")):
        return False
    if "::" in s or s.endswith((".rs", ".json", ".conf", ".rules", ".png")):
        return False
    return bool(re.search(r"[ éèêàâùûôîçÉÈÀ…’«]", s)) or s[:1].isupper()


def main():
    keys = []
    for rel in SOURCES:
        text = (ROOT / rel).read_text()
        for s, ctx in literals(text):
            if user_facing(s, ctx) and s not in keys:
                keys.append(s)
    en = (ROOT / "crates/tuxrgb-core/src/i18n/en.rs").read_text()
    known = {unescape(m.group(1)) for m in re.finditer(r'\(\s*"((?:[^"\\]|\\.)*)"\s*,', en)}
    show_all = "--all" in sys.argv
    missing = [k for k in keys if show_all or k not in known]
    for k in missing:
        print(repr(k))
    print(f"# {len(missing)} / {len(keys)}", file=sys.stderr)


if __name__ == "__main__":
    main()
