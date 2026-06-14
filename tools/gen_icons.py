#!/usr/bin/env python3
"""Generate PWA app icons (dark square + orange lightning bolt) with stdlib only.

No SVG rasterizer is installed on the dev box, so we rasterize the bolt polygon
directly (ray-cast fill, 2x supersampled for anti-aliasing) and encode PNGs via
zlib. Run from the repo/worktree root:  python3 tools/gen_icons.py
"""
import zlib
import struct
import os

BG = (0x14, 0x16, 0x1A)   # dark theme --bg (oklch(0.16 0.005 250))
FG = (0xF9, 0x73, 0x16)   # bolt orange (existing favicon stroke)
# Lightning-bolt vertices in a 24x24 viewBox (matches index.html favicon).
POLY = [(13, 2), (3, 14), (12, 14), (11, 22), (21, 10), (12, 10)]
SCALE = 0.62              # bolt fills ~62% of the square (maskable safe zone)
SS = 2                    # supersampling factor (anti-aliasing)
OUT = "server/static"
SIZES = {"icon-192.png": 192, "icon-512.png": 512, "icon-180.png": 180}


def inside(px, py, poly):
    """Even-odd ray-cast point-in-polygon test."""
    c = False
    j = len(poly) - 1
    for i in range(len(poly)):
        xi, yi = poly[i]
        xj, yj = poly[j]
        if ((yi > py) != (yj > py)) and (
            px < (xj - xi) * (py - yi) / (yj - yi) + xi
        ):
            c = not c
        j = i
    return c


def png_bytes(n, rows):
    """Encode square RGB image (rows: list of bytearray len n*3) as PNG."""
    raw = bytearray()
    for row in rows:
        raw.append(0)            # filter type 0 (None)
        raw.extend(row)

    def chunk(typ, data):
        return (
            struct.pack(">I", len(data)) + typ + data
            + struct.pack(">I", zlib.crc32(typ + data) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", n, n, 8, 2, 0, 0, 0))  # 8-bit RGB
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def gen(n, path):
    m = n * SS
    s = (m * SCALE) / 24.0
    t = m / 2.0 - 12.0 * s        # center the bolt's (12,12) bbox centre
    k = SS * SS
    rows = []
    for y in range(n):
        row = bytearray()
        for x in range(n):
            r = g = b = 0
            for sy in range(SS):
                for sx in range(SS):
                    vx = (x * SS + sx + 0.5 - t) / s
                    vy = (y * SS + sy + 0.5 - t) / s
                    col = FG if inside(vx, vy, POLY) else BG
                    r += col[0]
                    g += col[1]
                    b += col[2]
            row += bytes((r // k, g // k, b // k))
        rows.append(row)
    with open(path, "wb") as f:
        f.write(png_bytes(n, rows))
    print(f"wrote {path} ({n}x{n})")


if __name__ == "__main__":
    os.makedirs(OUT, exist_ok=True)
    for name, size in SIZES.items():
        gen(size, os.path.join(OUT, name))
