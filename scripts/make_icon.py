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

def pixel(x, y):
    if not inside_round(x, y, 64, 64, 960, 960, 200):
        return (0, 0, 0, 0)
    px = (17, 17, 19, 255)
    # back screen (gray outline) and front screen (white), slightly offset
    for (x0, y0, x1, y1, col) in [(300, 250, 820, 600, (120, 120, 128, 255)), (204, 380, 724, 730, (245, 245, 247, 255))]:
        if inside_round(x, y, x0, y0, x1, y1, 36):
            px = col if not inside_round(x, y, x0 + 28, y0 + 28, x1 - 28, y1 - 28, 14) else (17, 17, 19, 255)
    if inside_round(x, y, 404, 760, 524, 790, 15):
        px = (245, 245, 247, 255)
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
