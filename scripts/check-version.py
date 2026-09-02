#!/usr/bin/env python3
"""Assert every version in the repo agrees, and optionally matches a tag.

Six files carry the version and nothing keeps them together. A release built
from a tag whose binaries report a different number is worse than no release:
the artefact and the tag disagree, and the tag is what anyone will quote when
reporting a bug.

    python3 scripts/check-version.py            # they all agree
    python3 scripts/check-version.py v0.2.0     # ...and match the tag

The two `Cargo.lock`s are checked because a stale one **fails the release
build**, not just the reporting. `src-tauri` is outside the workspace
(ADR-006) and so carries its own lock, which a workspace build never touches;
bump the four obvious files, and the release job dies at `cargo xwin build
--locked` with "cannot update the lock file". That is what happened to the
first v0.5.0 tag, after the four-file check had said OK.
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
# Every crate in this repo, and the lock each one is recorded in. `--locked`
# in CI means a lock that disagrees with its manifest is a build failure.
LOCKS = {
    ROOT / "Cargo.lock": ["tuxtop-core", "tuxtop-serve"],
    ROOT / "src-tauri" / "Cargo.lock": ["tuxtop", "tuxtop-core"],
}


def locked_versions(lock: Path, names: list[str]) -> dict[str, str]:
    """Read the recorded version of each named package out of a lock file."""
    text = lock.read_text(encoding="utf-8")
    out = {}
    for name in names:
        m = re.search(
            rf'^name = "{re.escape(name)}"\nversion = "([^"]+)"', text, re.M
        )
        if m:
            out[f"{lock.relative_to(ROOT)} ({name})"] = m.group(1)
    return out


def main() -> int:
    found = {}
    for c in CARGOS:
        m = re.search(r'^version\s*=\s*"([^"]+)"', c.read_text(encoding="utf-8"), re.M)
        if not m:
            print(f"check-version: no version in {c.relative_to(ROOT)}")
            return 1
        found[str(c.relative_to(ROOT))] = m.group(1)
    found[str(CONF.relative_to(ROOT))] = json.loads(CONF.read_text(encoding="utf-8"))["version"]

    for lock, names in LOCKS.items():
        if not lock.exists():
            print(f"check-version: missing {lock.relative_to(ROOT)}")
            return 1
        got = locked_versions(lock, names)
        missing = [n for n in names if f"{lock.relative_to(ROOT)} ({n})" not in got]
        if missing:
            print(f"check-version: {', '.join(missing)} not in {lock.relative_to(ROOT)}")
            return 1
        found.update(got)

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
            print("           Bump the versions, or tag the version that exists.")
            return 1
        print(f"version OK — {version}, matching the tag")
        return 0

    print(f"version OK — {version} in all {len(found)} places")
    return 0


if __name__ == "__main__":
    sys.exit(main())
