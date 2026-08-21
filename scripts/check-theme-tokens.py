#!/usr/bin/env python3
"""Assert every CSS custom property is defined in all three theme states.

The page renders in three states, not two: an explicit light choice, an
explicit dark choice, and the default where only prefers-color-scheme
applies. A token defined in only some of them renders one theme's colour on
another theme's ground - and it fails silently, in one direction only, which
is exactly the kind of bug that survives a casual look.

This has now bitten twice: --viz-* and later the metal/reveal tokens both
landed in the media block and missed :root[data-theme="dark"].

Run: python3 scripts/check-theme-tokens.py
"""

import re
import sys
from pathlib import Path

CSS = Path(__file__).resolve().parent.parent / "src" / "styles.css"


def block(text: str, start_pat: str) -> str:
    """Return the body of the first brace-balanced block matching start_pat."""
    m = re.search(start_pat, text, re.M)
    if not m:
        raise SystemExit(f"could not find a block matching {start_pat!r}")
    i = text.index("{", m.start())
    depth, j = 0, i
    while j < len(text):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return text[i + 1 : j]
        j += 1
    raise SystemExit(f"unbalanced braces after {start_pat!r}")


def tokens(body: str) -> set[str]:
    # Only declarations at this level, not ones nested in an inner rule.
    return set(re.findall(r"(--[a-z0-9-]+)\s*:", body))


def main() -> int:
    css = CSS.read_text(encoding="utf-8")

    light = tokens(block(css, r"^:root\{"))
    media_outer = block(css, r"^@media \(prefers-color-scheme:dark\)")
    media = tokens(block(media_outer, r":root:not\(\[data-theme=\"light\"\]\)"))
    dark = tokens(block(css, r'^:root\[data-theme="dark"\]\{'))

    # Colour tokens must be redefined per theme. Structural ones (fonts,
    # radii) are deliberately defined once on :root and inherited.
    STRUCTURAL = {"--font-ui", "--font-mono", "--r"}
    expected = light - STRUCTURAL

    problems = []
    for name, got in (("@media dark", media), ('[data-theme="dark"]', dark)):
        missing = expected - got
        if missing:
            problems.append(f"  {name} is missing: {', '.join(sorted(missing))}")
        extra = got - light
        if extra:
            problems.append(f"  {name} defines tokens absent from :root: {', '.join(sorted(extra))}")

    if problems:
        print("theme token check FAILED", file=sys.stderr)
        print("\n".join(problems), file=sys.stderr)
        return 1

    print(f"theme tokens OK - {len(expected)} tokens defined in all three states")
    return 0


if __name__ == "__main__":
    sys.exit(main())
