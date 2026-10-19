"""Crop a rendered PNG and zoom it with nearest-neighbour sampling.

Design-time only: rendering is reviewed at 6-8x so glyph coverage, stem widths
and edge antialiasing can be judged pixel by pixel instead of guessed at.

    python scripts/crop.py in.png out.png x y w h zoom
"""
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bmp2png import write_png  # noqa: E402


def read_png(path):
    raw = Path(path).read_bytes()
    pos, w, h, idat = 8, 0, 0, b""
    while pos < len(raw):
        ln = int.from_bytes(raw[pos:pos + 4], "big")
        tag = raw[pos + 4:pos + 8]
        data = raw[pos + 8:pos + 8 + ln]
        if tag == b"IHDR":
            w = int.from_bytes(data[0:4], "big")
            h = int.from_bytes(data[4:8], "big")
        elif tag == b"IDAT":
            idat += data
        pos += 12 + ln
    buf = zlib.decompress(idat)
    stride = w * 4
    rows = [buf[y * (stride + 1) + 1:(y + 1) * (stride + 1)] for y in range(h)]
    return w, h, rows


def main(src, dst, x, y, w, h, z):
    sw, sh, rows = read_png(src)
    out = bytearray()
    for yy in range(h * z):
        sy = y + yy // z
        if not (0 <= sy < sh):
            out += b"\x00" * (w * z * 4)
            continue
        row = rows[sy]
        for xx in range(w * z):
            sx = x + xx // z
            if 0 <= sx < sw:
                i = sx * 4
                out += row[i:i + 4]
            else:
                out += b"\x00" * 4
    write_png(dst, w * z, h * z, bytes(out))
    print(f"{dst} <- {src} ({w}x{h} @{x},{y} x{z})")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], *map(int, sys.argv[3:8]))
