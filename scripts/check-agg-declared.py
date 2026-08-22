#!/usr/bin/env python3
"""Assert every metric in the registry declares how it aggregates.

ADR-008 rule 3: there is no default aggregation. A metric with no `agg` is
excluded from group views rather than averaged, because an absent rule is a
missing decision and the honest rendering of a missing decision is nothing.

That rule protects the other two, and it only holds if someone notices when a
new metric arrives without one. `aggregateGroup` already returns null for an
undeclared metric, so the failure is silent by design — the metric simply is
not there. This turns that silence into a commit-time error.

Run: python3 scripts/check-agg-declared.py
"""

import re
import sys
from pathlib import Path

APP = Path(__file__).resolve().parent.parent / "src" / "app.js"
VALID = {"ratio", "sum", "max", "concat"}


def main() -> int:
    src = APP.read_text(encoding="utf-8")

    start = src.find("const METRICS = {")
    if start < 0:
        print("check-agg: could not find the METRICS registry in src/app.js")
        return 1

    # The registry ends at the first line that closes it at its own indent.
    end = src.find("\n  };", start)
    if end < 0:
        print("check-agg: could not find the end of the METRICS registry")
        return 1
    body = src[start:end]

    # Metric entries sit at four-space indent: `    cpu: {`
    entries = [(m.group(1), m.start()) for m in re.finditer(r"^    (\w+): \{", body, re.M)]
    if not entries:
        print("check-agg: no metrics found — has the registry moved?")
        return 1

    bounds = [(name, pos, entries[i + 1][1] if i + 1 < len(entries) else len(body))
              for i, (name, pos) in enumerate(entries)]

    missing, bad = [], []
    for name, lo, hi in bounds:
        m = re.search(r"\bagg: '(\w+)'", body[lo:hi])
        if not m:
            missing.append(name)
        elif m.group(1) not in VALID:
            bad.append((name, m.group(1)))

    if missing or bad:
        for name in missing:
            print(f"check-agg: metric '{name}' declares no `agg`.")
            print("           It would be silently absent from every group view.")
            print("           Pick one of: ratio, sum, max, concat — see ADR-008.")
        for name, kind in bad:
            print(f"check-agg: metric '{name}' declares agg '{kind}', which is not a rule.")
            print(f"           Valid: {', '.join(sorted(VALID))}")
        return 1

    print(f"agg rules OK — all {len(bounds)} metrics declare one")
    return 0


if __name__ == "__main__":
    sys.exit(main())
