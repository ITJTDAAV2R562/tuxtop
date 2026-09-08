#!/usr/bin/env python3
"""Assert both lockfiles still agree with the manifests they belong to.

There are two Cargo trees. `src-tauri` is outside the workspace (ADR-006) and
carries **its own lockfile**, which a workspace `cargo build` never touches —
so changing a dependency of `tuxtop-core` updates the workspace lock and
silently leaves `src-tauri/Cargo.lock` behind. Nothing on a Linux dev box
notices, because nothing there builds that crate.

The first thing that notices is a gate running `--locked`, which does not
update a lock, it refuses:

    error: cannot update the lock file src-tauri/Cargo.lock
           because --locked was passed to prevent this

That is a one-minute Windows job failing on a message that names neither the
cause nor the fix. It has now cost three separate things: the first `v0.5.0`
tag (a version bump that missed the file), a `windows-sys` dependency added to
`tuxtop-core`, and every Dependabot PR that bumps a dependency shared with it.

This check is the same question asked in a second, on Linux, with the answer
attached. `cargo metadata --locked` resolves without compiling anything and
fails exactly when the lock would have to change.

    python3 scripts/check-locks.py

Run: in `verify.sh`, and in CI's `core` job beside the other checkers.
"""

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# manifest -> the command that refreshes its lock, for the error message.
#
# A plain resolve, deliberately, rather than the `cargo update -p tuxtop
# -p tuxtop-core` form that CLAUDE.md documents under Releasing. That form is
# right for a *version bump*, where nothing else can move. For a *dependency*
# change it re-resolves more widely: refreshing this lock for a `toml` bump
# with it moved `windows-sys` across three unrelated packages — eleven lines
# instead of one, none of them reviewed, in a commit whose stated purpose was
# to fix a lock. `cargo metadata` resolves and writes the lock without
# compiling, and changes only what actually had to change.
TREES = {
    ROOT / "Cargo.toml": "cargo metadata --format-version 1 >/dev/null",
    ROOT
    / "src-tauri"
    / "Cargo.toml": (
        "cargo metadata --manifest-path src-tauri/Cargo.toml "
        "--format-version 1 >/dev/null"
    ),
}


def stale(manifest: Path) -> str | None:
    """Return cargo's complaint if `manifest`'s lock needs updating, else None."""
    r = subprocess.run(
        [
            "cargo",
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--manifest-path",
            str(manifest),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    if r.returncode == 0:
        return None
    # Anything else — no cargo, no network, a broken manifest — is worth
    # showing rather than swallowing into a pass or a misleading failure.
    return r.stderr.strip().splitlines()[0] if r.stderr.strip() else "cargo failed"


def main() -> int:
    bad = False
    for manifest, fix in TREES.items():
        why = stale(manifest)
        rel = manifest.relative_to(ROOT)
        if why is None:
            continue
        bad = True
        print(f"check-locks: {rel.parent / 'Cargo.lock'} is stale")
        print(f"  {why}")
        print(f"  fix: {fix}")
    if bad:
        print()
        print("  Both locks are tracked, and a stale one is a build failure rather")
        print("  than a reporting discrepancy — CI builds src-tauri with --locked.")
        return 1
    print("locks OK — both lockfiles agree with their manifests")
    return 0


if __name__ == "__main__":
    sys.exit(main())
