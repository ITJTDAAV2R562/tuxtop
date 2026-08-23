#!/usr/bin/env python3
"""Assert every Tauri command is actually called by the frontend.

A backend command nothing invokes is a feature that shipped unreachable. It
compiles, it tests, it reviews clean, and no user can get to it. Three of them
had accumulated before anyone noticed — `set_host_group` (grouping was
reachable only for a host that did not exist yet), `history_usage` (the
settings panel showed an estimate while the measurement sat unused), and
`active_hosts` (dead outright).

None of that is visible from either side alone, which is exactly why it needs
a check rather than care.

**This checks commands, not config fields.** Host `os` shipped unreachable —
the backend had it, `hosts.toml` had it, and the Add host dialog simply never
sent it — and no automated check can catch that reliably: `HostFacts` also has
an `os`, so a search cannot tell `cfg.os` from `facts.os`. A check that cannot
decide would give false confidence, which is worse than none. The rule lives
in CLAUDE.md instead, where it can be read by whoever adds the next field.

Run: python3 scripts/check-commands-reachable.py
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MAIN = ROOT / "src-tauri" / "src" / "main.rs"
APP = ROOT / "src" / "app.js"


def main() -> int:
    rust = MAIN.read_text(encoding="utf-8")
    js = APP.read_text(encoding="utf-8")

    m = re.search(r"generate_handler!\[(.*?)\]", rust, re.S)
    if not m:
        print("check-commands: could not find generate_handler! in main.rs")
        return 1

    commands = [c.strip() for c in m.group(1).split(",") if c.strip()]
    if not commands:
        print("check-commands: no commands registered — has the macro moved?")
        return 1

    orphans = [c for c in commands if f"'{c}'" not in js and f'"{c}"' not in js]

    if orphans:
        for c in orphans:
            print(f"check-commands: '{c}' is registered but never invoked from src/app.js.")
        print()
        print("           A command no frontend code calls is a feature that shipped")
        print("           unreachable. Either wire it to a control a user can reach,")
        print("           or delete it — dead commands are not free, they read as")
        print("           working features to the next session.")
        return 1

    print(f"commands OK — all {len(commands)} reachable from the frontend")
    return 0


if __name__ == "__main__":
    sys.exit(main())
