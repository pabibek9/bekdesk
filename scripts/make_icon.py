"""Draw the 1024px source icon (two overlapping screens) with the stdlib only.
Run: python -I scripts/make_icon.py app-icon.png, then `npx @tauri-apps/cli@2 icon app-icon.png`."""
import struct, sys, zlib

N = 1024

def inside_round(x, y, x0, y0, x1, y1, r):
    if not (x0 <= x < x1 and y0 <= y < y1):
        return False
    cx = min(max(x, x0 + r), x1 - r)
    cy = min(max(y, y0 + r), y1 - r)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r

def bg(x, y):
    # diagonal blue gradient
    t = (x + y) / (2 * N)
    return (int(64 - 34 * t), int(118 - 58 * t), int(255 - 35 * t), 255)

def pixel(x, y):
    if not inside_round(x, y, 64, 64, 960, 960, 200):
        return (0, 0, 0, 0)
    px = bg(x, y)
    # back screen (gray outline) and front screen (white), slightly offset
    # back screen (translucent white), front screen (solid white with blue panel)
    if inside_round(x, y, 330, 230, 840, 590, 44):
        px = (190, 210, 255, 255) if not inside_round(x, y, 378, 278, 792, 542, 16) else bg(x, y)
    if inside_round(x, y, 184, 370, 714, 740, 48):
        px = (255, 255, 255, 255) if not inside_round(x, y, 234, 420, 664, 690, 18) else (226, 235, 255, 255)
    if inside_round(x, y, 374, 776, 524, 818, 21):
        px = (255, 255, 255, 255)
    return px

rows = bytearray()
for y in range(N):
    rows.append(0)
    for x in range(N):
        rows.extend(pixel(x, y))

def chunk(t, d):
    c = struct.pack(">I", len(d)) + t + d
    return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", N, N, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(bytes(rows), 9)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)
