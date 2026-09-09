#!/usr/bin/env python3
"""Draw the application icon.

A 1024² PNG, written with nothing but the standard library, so the icon is
reproducible from the repository rather than being a binary someone has to
trust. `npx tauri icon` derives every platform size from it.

The mark is the product in one shape: a page, and a speech balloon on it with
its text gone. Colours are `--bg` and `--accent` from `src/app.css`, dark
variant, because the icon sits on both desktops.
"""

import struct
import sys
import zlib
from pathlib import Path

SIZE = 1024
PAPER = (0xE8, 0xE7, 0xE3)
INK = (0x14, 0x15, 0x14)
BALLOON = (0xF6, 0xF5, 0xF2)


def rounded_rect(x, y, w, h, r):
    """A predicate: is (px, py) inside this rounded rectangle?"""

    def inside(px, py):
        if px < x or py < y or px >= x + w or py >= y + h:
            return False
        cx = min(max(px, x + r), x + w - r)
        cy = min(max(py, y + r), y + h - r)
        return (px - cx) ** 2 + (py - cy) ** 2 <= r * r

    return inside


def ellipse(cx, cy, rx, ry):
    def inside(px, py):
        return ((px - cx) / rx) ** 2 + ((py - cy) / ry) ** 2 <= 1.0

    return inside


def draw():
    page = rounded_rect(160, 96, SIZE - 320, SIZE - 192, 56)
    balloon = ellipse(SIZE / 2, 460, 250, 190)
    # The balloon's tail, pointing down-left the way a manga tail does.
    tail = rounded_rect(420, 600, 90, 190, 40)

    rows = []
    for y in range(SIZE):
        row = bytearray()
        for x in range(SIZE):
            if balloon(x, y) or (tail(x, y) and page(x, y)):
                row += bytes(BALLOON)
            elif page(x, y):
                row += bytes(INK)
            else:
                row += bytes(PAPER)
        rows.append(bytes(row))
    return rows


def chunk(kind, data):
    return (
        struct.pack(">I", len(data))
        + kind
        + data
        + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    )


def write_png(path, rows):
    raw = b"".join(b"\x00" + row for row in rows)
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    path.write_bytes(png)


if __name__ == "__main__":
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "design/icon.png")
    out.parent.mkdir(parents=True, exist_ok=True)
    write_png(out, draw())
    print(f"wrote {out} ({out.stat().st_size} bytes)")
