#!/usr/bin/env python3
"""Generate image fixtures without third-party modules (zlib PNG writer)."""
import os, struct, sys, zlib

def png(w, h, pixel):
    raw = b"".join(b"\x00" + b"".join(bytes(pixel(x, y)) for x in range(w)) for y in range(h))
    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))

def logo(x, y):           # purple rounded-ish badge with a white "N" bar pattern
    inside = (x - 32) ** 2 + (y - 32) ** 2 < 30 ** 2
    stripe = 18 <= x <= 24 or 40 <= x <= 46 or (abs((x - 18) - (y - 14) * 28 / 36) < 4 and 14 <= y <= 50)
    if not inside:
        return (255, 255, 255)
    return (255, 255, 255) if stripe and 14 <= y <= 50 else (109, 74, 255)

def banner(x, y):         # horizontal gradient
    return (int(30 + 200 * x / 599), int(90 + 60 * y / 159), 220)

def chart(x, y):          # bar chart
    bars = [(20, 60), (80, 110), (140, 40), (200, 130)]
    for bx, bh in bars:
        if bx <= x < bx + 40 and y >= 160 - bh:
            return (10, 102, 194)
    return (243, 242, 239)

def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "www"
    os.makedirs(out, exist_ok=True)
    open(os.path.join(out, "logo.png"), "wb").write(png(64, 64, logo))
    open(os.path.join(out, "banner.png"), "wb").write(png(600, 160, banner))
    open(os.path.join(out, "chart.png"), "wb").write(png(260, 160, chart))
    # Not an image although named .png (must be rejected by magic-byte check).
    open(os.path.join(out, "fake.png"), "wb").write(b"<html><script>alert(1)</script></html>")
    # Too large (> 5 MB limit): valid PNG header + padding.
    with open(os.path.join(out, "huge.png"), "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + b"\0" * (6 * 1024 * 1024))

if __name__ == "__main__":
    main()
