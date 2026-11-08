#!/usr/bin/env python3
"""Build the boot-splash masks from the brand artwork.

The masks live in the shared crate, next to the code that draws them.

The splash draws the Espressif mark, a heart and the Cactus Compute mark.
The marks are white line art on a transparent background, so the firmware
only needs their coverage: one byte per pixel, 0 (nothing) to 255 (full
white). On the black splash background, coverage is also the brightness, so
the firmware can fade the logos in without a second channel.

The masks are written next to the firmware that uses them, so the build does
not need Python. Re-run this script after changing the artwork:

    python3 tools/make_splash_assets.py

File format (little endian): u16 width, u16 height, then width * height
coverage bytes, row by row.
"""

import math
import pathlib
import struct

from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parents[1]
BRAND = ROOT / "assets" / "brand"

# The mark sizes per board: the height in pixels of each mark. The widths
# follow from the artwork's aspect ratio.
BOARDS = {
    # 1.85" round screen: the marks are small.
    "vocat": {"espressif": 52, "cactus": 52, "heart": 26},
    # 7" screen: room for bigger marks.
    "korvo": {"espressif": 80, "cactus": 80, "heart": 40},
}

# How much room the heart keeps from the marks it sits between.
HEART_MARGIN = 6


def coverage(image: Image.Image, width: int, height: int) -> Image.Image:
    """Resizes to width x height and returns the coverage in an L image."""
    if image.mode != "L":
        image = image.getchannel("A")
    return image.resize((width, height), Image.LANCZOS)


def mark(path: pathlib.Path, height: int) -> Image.Image:
    """The coverage of a brand mark, scaled to `height` pixels."""
    image = Image.open(path).convert("RGBA")
    ratio = image.width / image.height
    width = max(1, round(height * ratio))
    return coverage(image, width, height)


def heart(size: int) -> Image.Image:
    """A heart, drawn as the classic curve and antialiased by supersampling."""
    scale = 8
    canvas = Image.new("L", (size * scale, size * scale), 0)
    draw = ImageDraw.Draw(canvas)
    points = []
    for step in range(721):
        t = 2 * math.pi * step / 720
        x = 16 * math.sin(t) ** 3
        y = 13 * math.cos(t) - 5 * math.cos(2 * t) - 2 * math.cos(3 * t) - math.cos(4 * t)
        points.append((x, y))
    # The curve spans x in [-16, 16] and y in [-17, 12].
    min_x, max_x = -16.0, 16.0
    min_y, max_y = -17.0, 12.0
    def to_px(x, y):
        px = (x - min_x) / (max_x - min_x) * (size * scale)
        py = (max_y - y) / (max_y - min_y) * (size * scale)
        return (px, py)
    draw.polygon([to_px(x, y) for x, y in points], fill=255)
    return canvas.resize((size, size), Image.LANCZOS)


def write(path: pathlib.Path, image: Image.Image) -> None:
    w, h = image.size
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(struct.pack("<HH", w, h) + image.tobytes())
    print(f"{path.relative_to(ROOT)}: {w}x{h}")


def main() -> None:
    for board, sizes in BOARDS.items():
        out = ROOT / "common" / "assets" / board
        esp = mark(BRAND / "espressif-symbol.png", sizes["espressif"])
        cactus = mark(BRAND / "cactus-white.png", sizes["cactus"])
        heart_mask = heart(sizes["heart"])
        write(out / "splash_espressif.msk", esp)
        write(out / "splash_cactus.msk", cactus)
        write(out / "splash_heart.msk", heart_mask)


if __name__ == "__main__":
    main()
