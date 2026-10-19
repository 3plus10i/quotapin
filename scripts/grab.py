"""Dev helper: poke the running QuotaPin window and grab the popover pixels.

Works even when the desktop is not visible to the caller, as long as the app
runs in the same window station. Writes BMPs next to `out`.
"""
import ctypes
import ctypes.wintypes as w
import sys
import time

u = ctypes.WinDLL("user32", use_last_error=True)
g = ctypes.WinDLL("gdi32", use_last_error=True)

WM_APP = 0x8000
WM_TRAY = WM_APP + 10
WM_LBUTTONUP = 0x0202

u.FindWindowW.restype = w.HWND
u.FindWindowW.argtypes = [w.LPCWSTR, w.LPCWSTR]
u.GetWindowRect.argtypes = [w.HWND, ctypes.POINTER(w.RECT)]
u.PrintWindow.argtypes = [w.HWND, w.HDC, w.UINT]
u.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
u.GetDC.restype = w.HDC
u.GetDC.argtypes = [w.HWND]
u.IsWindowVisible.argtypes = [w.HWND]

g.CreateCompatibleDC.restype = w.HDC
g.CreateCompatibleDC.argtypes = [w.HDC]
g.CreateCompatibleBitmap.restype = w.HBITMAP
g.CreateCompatibleBitmap.argtypes = [w.HDC, ctypes.c_int, ctypes.c_int]
g.SelectObject.restype = w.HGDIOBJ
g.SelectObject.argtypes = [w.HDC, w.HGDIOBJ]
g.GetDIBits.argtypes = [
    w.HDC, w.HBITMAP, w.UINT, w.UINT, ctypes.c_void_p, ctypes.c_void_p, w.UINT
]


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [
        ("biSize", w.DWORD),
        ("biWidth", ctypes.c_long),
        ("biHeight", ctypes.c_long),
        ("biPlanes", w.WORD),
        ("biBitCount", w.WORD),
        ("biCompression", w.DWORD),
        ("biSizeImage", w.DWORD),
        ("biXPelsPerMeter", ctypes.c_long),
        ("biYPelsPerMeter", ctypes.c_long),
        ("biClrUsed", w.DWORD),
        ("biClrImportant", w.DWORD),
    ]


def save_bmp(path, w_, h_, data):
    size = 54 + len(data)
    hdr = b"BM" + size.to_bytes(4, "little") + (0).to_bytes(4, "little")
    hdr += (54).to_bytes(4, "little") + (40).to_bytes(4, "little")
    hdr += w_.to_bytes(4, "little", signed=True)
    hdr += (-h_).to_bytes(4, "little", signed=True)
    hdr += (1).to_bytes(2, "little") + (32).to_bytes(2, "little")
    hdr += (0).to_bytes(4, "little") + len(data).to_bytes(4, "little")
    hdr += (2835).to_bytes(4, "little", signed=True) * 2 + (0).to_bytes(8, "little")
    open(path, "wb").write(hdr + data)


def grab(hwnd, path, out_w=None, out_h=None):
    rc = w.RECT()
    u.GetWindowRect(hwnd, ctypes.byref(rc))
    W = out_w or (rc.right - rc.left)
    H = out_h or (rc.bottom - rc.top)
    if W <= 0 or H <= 0:
        print("bad rect", W, H)
        return False
    hdc_screen = u.GetDC(0)
    hdc = g.CreateCompatibleDC(hdc_screen)
    bmp = g.CreateCompatibleBitmap(hdc_screen, W, H)
    g.SelectObject(hdc, bmp)

    # composite of PrintWindow (works for layered windows with flag 2)
    ok = u.PrintWindow(hwnd, hdc, 2)

    bi = BITMAPINFOHEADER()
    bi.biSize = ctypes.sizeof(BITMAPINFOHEADER)
    bi.biWidth = W
    bi.biHeight = -H
    bi.biPlanes = 1
    bi.biBitCount = 32
    bi.biCompression = 0
    buf = ctypes.create_string_buffer(W * H * 4)
    g.GetDIBits(hdc, bmp, 0, H, buf, ctypes.byref(bi), 0)
    save_bmp(path, W, H, buf.raw)
    print(f"rect=({rc.left},{rc.top},{W}x{H}) printwindow={ok} visible={u.IsWindowVisible(hwnd)}")
    return True


def main():
    msg = u.FindWindowW("QuotaPin.Msg", None)
    pop = u.FindWindowW("QuotaPin.Popover", None)
    mini = u.FindWindowW("QuotaPin.Mini", None)
    print("msg hwnd =", msg, "popover hwnd =", pop, "mini hwnd =", mini)
    if not msg:
        return 1
    out = sys.argv[1] if len(sys.argv) > 1 else "grab.bmp"
    if pop:
        u.PostMessageW(msg, WM_TRAY, 0, WM_LBUTTONUP)
        time.sleep(0.9)
        pop = u.FindWindowW("QuotaPin.Popover", None)
        grab(pop, out)
    if mini and len(sys.argv) > 2:
        grab(mini, sys.argv[2])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
