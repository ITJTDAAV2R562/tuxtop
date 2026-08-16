#!/usr/bin/env python3
"""Generate Tuxtop's icon set with no image-library dependency.

Writes PNGs and a Windows .ico into src-tauri/icons/. Pure stdlib (zlib +
struct) so it runs anywhere, including a bare WSL box with no ImageMagick.

The mark is the core grid itself: four bars at different fill levels on a
dark rounded square. That is the app's whole identity, and bars stay legible
at 16px where a detailed glyph would turn to mush.
"""

import os
import struct
import zlib

# Palette matches the app's accent tokens (see src/index.html).
BG = (0x14, 0x18, 0x20, 255)  # dark slate ground
BAR_HI = (0x58, 0xBD, 0xF7, 255)  # accent, dark-theme value
BAR_LO = (0x2B, 0x5A, 0x7A, 255)  # unfilled remainder of a bar
TRANSPARENT = (0, 0, 0, 0)

# Relative fill of each of the four bars — deliberately uneven, so the mark
# reads as *activity* rather than a static logo.
FILLS = [0.45, 0.80, 0.30, 0.95]


def rounded_rect_mask(size, radius):
    """True where a rounded square covers the pixel."""
    mask = [[True] * size for _ in range(size)]
    r2 = radius * radius
    for y in range(size):
        for x in range(size):
            # Only the four corner boxes need a distance test.
            cx = cy = None
            if x < radius and y < radius:
                cx, cy = radius, radius
            elif x >= size - radius and y < radius:
                cx, cy = size - radius - 1, radius
            elif x < radius and y >= size - radius:
                cx, cy = radius, size - radius - 1
            elif x >= size - radius and y >= size - radius:
                cx, cy = size - radius - 1, size - radius - 1
            if cx is not None:
                dx, dy = x - cx, y - cy
                if dx * dx + dy * dy > r2:
                    mask[y][x] = False
    return mask


def render(size):
    """Return RGBA rows for one icon size."""
    radius = max(2, round(size * 0.22))
    mask = rounded_rect_mask(size, radius)
    px = [[TRANSPARENT] * size for _ in range(size)]

    for y in range(size):
        for x in range(size):
            if mask[y][x]:
                px[y][x] = BG

    # Four bars inset from the edges.
    pad = max(1, round(size * 0.20))
    inner = size - 2 * pad
    gap = max(1, round(size * 0.05))
    bar_w = max(1, (inner - gap * (len(FILLS) - 1)) // len(FILLS))

    for i, fill in enumerate(FILLS):
        x0 = pad + i * (bar_w + gap)
        x1 = min(x0 + bar_w, size - pad)
        top_lo = pad
        filled_h = max(1, round(inner * fill))
        top_hi = pad + inner - filled_h

        for y in range(top_lo, pad + inner):
            for x in range(x0, x1):
                if 0 <= x < size and 0 <= y < size and mask[y][x]:
                    px[y][x] = BAR_HI if y >= top_hi else BAR_LO

    return px


def png_bytes(px):
    """Encode RGBA rows as a PNG."""
    size = len(px)
    raw = bytearray()
    for row in px:
        raw.append(0)  # filter type 0 (None)
        for r, g, b, a in row:
            raw += bytes((r, g, b, a))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)  # 8-bit RGBA
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def ico_bytes(pngs):
    """Wrap PNGs in an .ico container (Vista+ accepts PNG-compressed entries)."""
    count = len(pngs)
    header = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count
    entries, blobs = b"", b""

    for size, data in pngs:
        # 256 is encoded as 0 in the single width/height byte.
        dim = 0 if size >= 256 else size
        entries += struct.pack(
            "<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset
        )
        blobs += data
        offset += len(data)

    return header + entries + blobs


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    out = os.path.join(here, "..", "src-tauri", "icons")
    os.makedirs(out, exist_ok=True)

    # Sizes Tauri expects for a Windows build.
    sizes = [32, 128, 256]
    rendered = {s: png_bytes(render(s)) for s in sizes}

    named = {
        "32x32.png": 32,
        "128x128.png": 128,
        "128x128@2x.png": 256,
        "icon.png": 256,
    }
    for name, s in named.items():
        path = os.path.join(out, name)
        with open(path, "wb") as f:
            f.write(rendered[s])
        print(f"wrote {name} ({len(rendered[s])} bytes)")

    ico_sizes = [16, 32, 48, 256]
    ico_pngs = [(s, rendered.get(s) or png_bytes(render(s))) for s in ico_sizes]
    ico = ico_bytes(ico_pngs)
    with open(os.path.join(out, "icon.ico"), "wb") as f:
        f.write(ico)
    print(f"wrote icon.ico ({len(ico)} bytes, {len(ico_sizes)} sizes)")


if __name__ == "__main__":
    main()
