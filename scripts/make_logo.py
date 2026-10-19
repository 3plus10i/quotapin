"""Turn the source logo art into the assets QuotaPin ships.

The source is a 1254x1254 PNG; this script trims it to its ink, resamples it
down with an area filter, and writes:

    assets/mark.bin      96x96 premultiplied BGRA, embedded into the exe and
                         resampled at runtime for every tray / window icon size
    assets/icon.ico      16..256, low sizes as 32-bit DIB, 128/256 as PNG
    assets/logo.png      256x256 RGBA, for docs and the release zip

ffmpeg is only used as a PNG decoder. Pure stdlib otherwise.

    python scripts/make_logo.py tmp/极简蓝色沙漏图标.png tmp/ffmpeg.exe
"""
import re
import struct
import subprocess
import sys
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MASTER = 96           # embedded size; divides evenly into 16/24/32/48
FILL = 0.94           # content height as a fraction of the icon box

# Sizes baked on top of the master, with the strength of a palette snap applied
# after resampling. At 16-20px the art's four inks get averaged into mush by the
# downscale; pulling each pixel back toward its nearest ink restores the crisp
# frame that makes an hourglass readable at tray size.
SMALL = {16: 0.6, 20: 0.6}

# The art's own inks, sampled from the source: glass, frame, sand, rim highlight.
INKS = [(0xE9, 0xF6, 0xFC), (0x01, 0x2C, 0x60), (0x1B, 0xBD, 0xFC), (0x1E, 0x6F, 0xE0)]

ICO_SIZES = [16, 20, 24, 32, 48, 64, 128, 256]
PNG_FROM = 128        # entries at or above this size are PNG-compressed


# ---------------------------------------------------------------- decode

def decode(src, ffmpeg):
    """Return (w, h, bytearray RGBA) straight from ffmpeg's rawvideo."""
    probe = subprocess.run(
        [ffmpeg, "-hide_banner", "-i", src],
        capture_output=True, text=True, errors="ignore",
    )
    line = next(l for l in probe.stderr.splitlines() if "Video: png" in l)
    dims = re.search(r"(\d+)x(\d+)", line)
    w, h = int(dims.group(1)), int(dims.group(2))
    raw = subprocess.run(
        [ffmpeg, "-hide_banner", "-i", src, "-frames:v", "1",
         "-f", "rawvideo", "-pix_fmt", "rgba", "-"],
        capture_output=True,
    ).stdout
    assert len(raw) == w * h * 4, f"got {len(raw)} bytes for {w}x{h}"
    return w, h, bytearray(raw)


def ink_box(w, h, buf, thresh=8):
    """Bounding box of the non-transparent pixels."""
    x0, y0, x1, y1 = w, h, -1, -1
    for y in range(h):
        row = y * w * 4
        for x in range(w):
            if buf[row + x * 4 + 3] > thresh:
                if x < x0:
                    x0 = x
                if x > x1:
                    x1 = x
                if y < y0:
                    y0 = y
                if y > y1:
                    y1 = y
    return x0, y0, x1 + 1, y1 + 1


# ---------------------------------------------------------------- resample

def area_resample(w, h, buf, box, out_w, out_h):
    """Separable area-average resample of `box` into an out_w x out_h buffer."""
    x0, y0, x1, y1 = box
    sw, sh = x1 - x0, y1 - y0

    def weights(src_n, dst_n):
        span = src_n / dst_n
        out = []
        for i in range(dst_n):
            a, b = i * span, (i + 1) * span
            ws = []
            for j in range(int(a), min(int(b) + 1, src_n)):
                cover = min(b, j + 1) - max(a, j)
                if cover > 0:
                    ws.append((j, cover / span))
            out.append(ws)
        return out

    wx = weights(sw, out_w)
    # horizontal pass: sh rows -> sh x out_w (float, 4 channels)
    tmp = [0.0] * (sh * out_w * 4)
    for yy in range(sh):
        srow = (y0 + yy) * w * 4
        for xx, ws in enumerate(wx):
            r = g = b = a = 0.0
            for j, c in ws:
                i = srow + (x0 + j) * 4
                r += buf[i] * c
                g += buf[i + 1] * c
                b += buf[i + 2] * c
                a += buf[i + 3] * c
            o = (yy * out_w + xx) * 4
            tmp[o:o + 4] = (r, g, b, a)

    wy = weights(sh, out_h)
    out = bytearray(out_w * out_h * 4)
    for y, ws in enumerate(wy):
        base = y * out_w * 4
        for x in range(out_w):
            r = g = b = a = 0.0
            for j, c in ws:
                o = (j * out_w + x) * 4
                r += tmp[o] * c
                g += tmp[o + 1] * c
                b += tmp[o + 2] * c
                a += tmp[o + 3] * c
            i = base + x * 4
            out[i] = min(255, round(r))
            out[i + 1] = min(255, round(g))
            out[i + 2] = min(255, round(b))
            out[i + 3] = min(255, round(a))
    return out


def snap_to_inks(rgba, strength):
    out = bytearray(rgba)
    for i in range(0, len(rgba), 4):
        if rgba[i + 3] == 0:
            continue
        r, g, b = rgba[i], rgba[i + 1], rgba[i + 2]
        ink = min(INKS, key=lambda k: (k[0] - r) ** 2 + (k[1] - g) ** 2 + (k[2] - b) ** 2)
        for c in range(3):
            out[i + c] = round(rgba[i + c] + (ink[c] - rgba[i + c]) * strength)
    return out


def fit_square(w, h, buf, box, size, fill=FILL):
    """Centre the ink in a square, keeping its aspect ratio."""
    x0, y0, x1, y1 = box
    aspect = (x1 - x0) / (y1 - y0)
    box_h = max(1, round(size * fill))
    box_w = max(1, round(box_h * aspect))
    art = area_resample(w, h, buf, box, box_w, box_h)
    out = bytearray(size * size * 4)
    ox, oy = (size - box_w) // 2, (size - box_h) // 2
    for y in range(box_h):
        d = ((oy + y) * size + ox) * 4
        out[d:d + box_w * 4] = art[y * box_w * 4:(y + 1) * box_w * 4]
    return out


# ---------------------------------------------------------------- encoders

def png_bytes(w, h, rgba):
    def chunk(tag, data):
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))

    stride = w * 4
    raw = b"".join(b"\x00" + rgba[y * stride:(y + 1) * stride] for y in range(h))
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b""))


def dib_entry(w, h, rgba):
    """32-bit bottom-up DIB with the AND mask that ICO still requires."""
    xor = bytearray()
    for y in reversed(range(h)):
        row = rgba[y * w * 4:(y + 1) * w * 4]
        for x in range(w):
            r, g, b, a = row[x * 4:x * 4 + 4]
            xor += bytes((b, g, r, a))
    mask_stride = ((w + 31) // 32) * 4
    mask = bytearray(mask_stride * h)
    for y in reversed(range(h)):
        for x in range(w):
            if rgba[(y * w + x) * 4 + 3] == 0:
                mask[y * mask_stride + (x // 8)] |= 0x80 >> (x % 8)
    header = struct.pack("<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, len(xor) + len(mask), 0, 0, 0, 0)
    return header + bytes(xor) + bytes(mask)


def ico_bytes(images):
    """images: [(size, payload)] with payload already encoded."""
    out = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    dirs, body = b"", b""
    for size, payload in images:
        dim = 0 if size >= 256 else size
        dirs += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(payload), offset + len(body))
        body += payload
    return out + dirs + body


def premul_bgra(size, rgba):
    out = bytearray(size * size * 4)
    for i in range(0, len(rgba), 4):
        r, g, b, a = rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]
        if a == 0:
            continue
        out[i] = (b * a + 127) // 255
        out[i + 1] = (g * a + 127) // 255
        out[i + 2] = (r * a + 127) // 255
        out[i + 3] = a
    return out


# ---------------------------------------------------------------- main

def main(src, ffmpeg):
    w, h, buf = decode(src, ffmpeg)
    box = ink_box(w, h, buf)
    print(f"source {w}x{h}, ink {box[2]-box[0]}x{box[3]-box[1]} at {box[:2]}")

    assets = ROOT / "assets"
    assets.mkdir(exist_ok=True)

    # mark.bin = "QP1" + count + sizes + premultiplied blobs (master last, so a
    # size that was not baked falls back to it)
    blobs = []
    for size, strength in sorted(SMALL.items()):
        art = snap_to_inks(fit_square(w, h, buf, box, size), strength)
        blobs.append((size, premul_bgra(size, art)))
    blobs.append((MASTER, premul_bgra(MASTER, fit_square(w, h, buf, box, MASTER))))
    pack = b"QP1" + bytes([len(blobs)]) + bytes(s for s, _ in blobs)
    pack += b"".join(b for _, b in blobs)
    (assets / "mark.bin").write_bytes(pack)
    print(f"mark.bin      {[s for s,_ in blobs]}  {len(pack)} B")

    entries = []
    for size in ICO_SIZES:
        if size in SMALL:
            art = snap_to_inks(fit_square(w, h, buf, box, size), SMALL[size])
        else:
            art = fit_square(w, h, buf, box, size)
        payload = png_bytes(size, size, art) if size >= PNG_FROM else dib_entry(size, size, art)
        entries.append((size, payload))
    ico = ico_bytes(entries)
    (assets / "icon.ico").write_bytes(ico)
    sizes = " ".join(f"{s}:{len(p)//1024}K" for s, p in entries)
    print(f"icon.ico      {len(ico)//1024} KB total ({sizes})")

    logo = fit_square(w, h, buf, box, 256)
    (assets / "logo.png").write_bytes(png_bytes(256, 256, logo))
    print(f"logo.png      256x256  {(assets/'logo.png').stat().st_size//1024} KB")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
