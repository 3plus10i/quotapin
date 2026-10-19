"""Convert the 32-bit BMPs written by `QuotaPin.exe --render` into PNGs.

Pure stdlib (struct + zlib); used only for design review, never at runtime.
"""
import struct
import sys
import zlib
from pathlib import Path


def read_bmp(path):
    raw = Path(path).read_bytes()
    assert raw[:2] == b"BM", "not a BMP"
    off = struct.unpack_from("<I", raw, 10)[0]
    w, h = struct.unpack_from("<ii", raw, 18)
    bpp = struct.unpack_from("<H", raw, 28)[0]
    assert bpp == 32, f"expected 32bpp, got {bpp}"
    h = abs(h)
    stride = w * 4
    out = bytearray()
    for y in range(h):
        start = off + y * stride
        row = raw[start:start + stride]
        # BMP stores BGRA; PNG wants RGBA
        for x in range(0, len(row), 4):
            out += bytes((row[x + 2], row[x + 1], row[x], row[x + 3]))
    # the file was written top-down, so rows are already in display order
    return w, h, bytes(out)


def write_png(path, w, h, rgba):
    def chunk(tag, data):
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    stride = w * 4
    raw = b"".join(
        b"\x00" + rgba[y * stride:(y + 1) * stride] for y in range(h)
    )
    out = b"\x89PNG\r\n\x1a\n"
    out += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    out += chunk(b"IDAT", zlib.compress(raw, 9))
    out += chunk(b"IEND", b"")
    Path(path).write_bytes(out)


def main(directory):
    for bmp in sorted(Path(directory).glob("*.bmp")):
        w, h, data = read_bmp(bmp)
        write_png(bmp.with_suffix(".png"), w, h, data)
        print(f"{bmp.name} -> {w}x{h}")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else ".")
