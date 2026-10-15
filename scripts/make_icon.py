"""Renders assets/icon.ico (PNG-compressed, 16..256 px): a rounded square with a hub ring and node."""
import io
import math
import os

from PIL import Image


def render(size: int) -> Image.Image:
    ss = 4
    n = size * ss
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    px = img.load()
    c = (n - 1) / 2
    half, rad = 0.47 * n, 0.125 * n
    for y in range(n):
        for x in range(n):
            dx, dy = abs(x - c) - (half - rad), abs(y - c) - (half - rad)
            d = math.hypot(max(dx, 0), max(dy, 0)) + min(max(dx, dy), 0) - rad
            if d >= 0:
                continue
            t = y / n
            r = math.hypot(x - c, y - c) / n
            if r < 0.17 or 0.27 < r < 0.33:
                px[x, y] = (255, 255, 255, 255)
            else:
                px[x, y] = (int(203 - 66 * t), int(166 + 14 * t), 247, 255)
    return img.resize((size, size), Image.LANCZOS)


sizes = [16, 32, 48, 64, 128, 256]
out = os.path.join(os.path.dirname(__file__), "..", "assets", "icon.ico")
render(256).save(out, format="ICO", sizes=[(s, s) for s in sizes])
print("wrote", os.path.normpath(out))
