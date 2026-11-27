#!/usr/bin/env python3
"""Regenerates crates/tuxrgb/assets/third_party.tsv (shown in the app under
Réglages → Crédits & licences) from `cargo metadata`.

Only crates that end up in the Linux x86_64 build are listed.
Run after every dependency change:  python3 scripts/gen-third-party.py
"""

import json
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "crates/tuxrgb/assets/third_party.tsv"


def main():
    meta = json.loads(
        subprocess.check_output(
            [
                "cargo", "metadata", "--format-version", "1", "--locked",
                "--filter-platform", "x86_64-unknown-linux-gnu",
            ],
            cwd=ROOT,
        )
    )
    members = set(meta["workspace_members"])
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    packages = {p["id"]: p for p in meta["packages"]}

    # Walk normal + build dependencies from the workspace members.
    seen, stack = set(), list(members)
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            kinds = {k["kind"] for k in dep["dep_kinds"]}
            if kinds - {"dev"}:
                stack.append(dep["pkg"])

    rows = []
    for pid in seen - members:
        p = packages[pid]
        rows.append(
            (
                p["name"],
                p["version"],
                (p.get("license") or p.get("license_file") or "voir dépôt").replace("/", " OR "),
                p.get("repository") or "",
            )
        )
    rows.sort(key=lambda r: (r[0].lower(), r[1]))
    lines = ["# name\tversion\tlicense\trepository"]
    lines += ["\t".join(r) for r in rows]
    OUT.write_text("\n".join(lines) + "\n")
    print(f"{len(rows)} crates written to {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
