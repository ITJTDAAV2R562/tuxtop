#!/usr/bin/env python3
"""Assert every version in the repo agrees, and optionally matches a tag.

Four files carry the version and nothing keeps them together. A release built
from a tag whose binaries report a different number is worse than no release:
the artefact and the tag disagree, and the tag is what anyone will quote when
reporting a bug.

    python3 scripts/check-version.py            # the four agree
    python3 scripts/check-version.py v0.2.0     # ...and match the tag
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CARGOS = [
    ROOT / "crates" / "tuxtop-core" / "Cargo.toml",
    ROOT / "crates" / "tuxtop-serve" / "Cargo.toml",
    ROOT / "src-tauri" / "Cargo.toml",
]
CONF = ROOT / "src-tauri" / "tauri.conf.json"


def main() -> int:
    found = {}
    for c in CARGOS:
        m = re.search(r'^version\s*=\s*"([^"]+)"', c.read_text(encoding="utf-8"), re.M)
        if not m:
            print(f"check-version: no version in {c.relative_to(ROOT)}")
            return 1
        found[str(c.relative_to(ROOT))] = m.group(1)
    found[str(CONF.relative_to(ROOT))] = json.loads(CONF.read_text(encoding="utf-8"))["version"]

    distinct = set(found.values())
    if len(distinct) != 1:
        print("check-version: versions disagree —")
        for k, v in found.items():
            print(f"    {v:<10} {k}")
        return 1

    version = distinct.pop()

    if len(sys.argv) > 1:
        tag = sys.argv[1].lstrip("v")
        if tag != version:
            print(f"check-version: tag v{tag} but the repo says {version}.")
            print("           Bump the four files, or tag the version that exists.")
            return 1
        print(f"version OK — {version}, matching the tag")
        return 0

    print(f"version OK — {version} in all four files")
    return 0


if __name__ == "__main__":
    sys.exit(main())
