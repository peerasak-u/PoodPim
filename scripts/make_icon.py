"""Draw the PoodPim app icon (white microphone on a teal rounded square) and build an .icns.

Standard library only. Usage: python3 make_icon.py <out.icns>
"""

import math
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib


def coverage(d: float, aa: float) -> float:
    return max(0.0, min(1.0, 0.5 - d / aa))


def pixel(u: float, v: float, aa: float):
    # rounded square background (macOS icon grid: ~10% margin)
    m, r = 0.1, 0.18
    qx = max(abs(u - 0.5) - (0.5 - m - r), 0.0)
    qy = max(abs(v - 0.5) - (0.5 - m - r), 0.0)
    bg = coverage(math.hypot(qx, qy) - r, aa)
    if bg == 0:
        return (0, 0, 0, 0)
    t = v  # vertical gradient
    top, bottom = (0x14, 0xB8, 0xA6), (0x0F, 0x76, 0x6E)
    base = [top[i] + (bottom[i] - top[i]) * t for i in range(3)]
    # microphone glyph, scaled into the middle 60%
    x, y = (u - 0.2) / 0.6, (v - 0.2) / 0.6
    cy = min(max(y, 0.22), 0.46)
    head = math.hypot(x - 0.5, y - cy) - 0.13
    ring = abs(math.hypot(x - 0.5, y - 0.46) - 0.25) - 0.035 if y >= 0.46 else 1.0
    stem = abs(x - 0.5) - 0.035 if 0.71 <= y <= 0.86 else 1.0
    base_bar = abs(y - 0.86) - 0.035 if 0.31 <= x <= 0.69 else 1.0
    mic = coverage(min(head, ring, stem, base_bar) * 0.6, aa)
    rgb = [round(c * (1 - mic) + 255 * mic) for c in base]
    return (*rgb, round(255 * bg))


def png(size: int) -> bytes:
    aa = 1.5 / size
    rows = []
    for py in range(size):
        row = bytearray([0])
        for px in range(size):
            row += bytes(pixel((px + 0.5) / size, (py + 0.5) / size, aa))
        rows.append(bytes(row))

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b"")


def main(out: str) -> None:
    tmp = tempfile.mkdtemp()
    iconset = os.path.join(tmp, "AppIcon.iconset")
    os.mkdir(iconset)
    cache = {}
    for pt in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            n = pt * scale
            cache.setdefault(n, png(n))
            name = f"icon_{pt}x{pt}{'@2x' if scale == 2 else ''}.png"
            with open(os.path.join(iconset, name), "wb") as f:
                f.write(cache[n])
    subprocess.run(["iconutil", "-c", "icns", iconset, "-o", out], check=True)
    shutil.rmtree(tmp)


if __name__ == "__main__":
    main(sys.argv[1])
