"""Draws the 1024x1024 source icon using only the standard library."""
import struct, sys, zlib

S = 1024
BG = (61, 90, 254)
FG = (255, 255, 255)


def inside_round_rect(x, y, m=48, r=210):
    a, b = m, S - m
    if not (a <= x < b and a <= y < b):
        return False
    cx = min(max(x, a + r), b - r)
    cy = min(max(y, a + r), b - r)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r


def inside_glyph(x, y):
    # Down arrow over a tray.
    if 442 <= x <= 582 and 220 <= y <= 560:
        return True
    if 500 <= y <= 700:
        half = 700 - y
        if abs(x - 512) <= half:
            return True
    return 262 <= x <= 762 and 756 <= y <= 830


rows = bytearray()
for y in range(S):
    rows.append(0)
    for x in range(S):
        if inside_round_rect(x, y):
            rows.extend((*(FG if inside_glyph(x, y) else BG), 255))
        else:
            rows.extend((0, 0, 0, 0))


def chunk(t, d):
    return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)


png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", S, S, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(bytes(rows), 9)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)
print("wrote", sys.argv[1], len(png))
