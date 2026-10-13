#!/usr/bin/env python3
"""Builds the app icon and the menu bar template glyphs from the source logo.

Usage: python3 scripts/make-icons.py <source.png> <out-dir>

Writes into <out-dir>:
  icon-1024.png                 macOS app icon artwork (824 px squircle on a
                                1024 canvas, soft shadow)
  AppIcon.icns                  bundle icon (scripts/bundle.sh copies it)
  app-icon-512.png              embedded in the binary as the runtime
                                application icon and the About page image
  menubar-template.png          18 pt glyph at 3x (54 px), black on clear
  menubar-template-active.png   filled variant shown while items are revealed

Requires Pillow and Xcode's iconutil.
"""

import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

CANVAS = 1024
BODY = 824  # Apple's macOS icon grid: 824 px body, 100 px margins.
CORNER = 0.2237  # Corner radius as a fraction of the body size.
GLYPH_PX = 54  # 18 pt at 3x.
GLYPH_PADDING = 0.04  # Fraction of the glyph left clear on each side.
OUTLINE_ERODE_HALF = 45  # MinFilter size at half resolution (about 44 px at full).

# A pixel belongs to the cube artwork when its red channel clears this; the
# background blue stays far below it, the white outline, the light blue and
# the orange accents far above it.
FOREGROUND_RED = 110
FOREGROUND_SOFT = 40


def squircle_mask(size: int, supersample: int = 4) -> Image.Image:
    big = size * supersample
    mask = Image.new("L", (big, big), 0)
    radius = int(big * CORNER)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, big - 1, big - 1), radius, fill=255)
    return mask.resize((size, size), Image.LANCZOS)


def body_bbox(src: Image.Image) -> tuple[int, int, int, int]:
    """Bounding box of the opaque squircle, ignoring stray low-alpha specks."""
    alpha = src.getchannel("A").point(lambda v: 255 if v > 200 else 0)
    w, h = src.size
    px = alpha.load()
    rows = [y for y in range(h) if sum(1 for x in range(w) if px[x, y]) > w // 4]
    cols = [x for x in range(w) if sum(1 for y in range(h) if px[x, y]) > h // 4]
    return cols[0], rows[0], cols[-1] + 1, rows[-1] + 1


def background_gradient(src: Image.Image, box: tuple[int, int, int, int], size: int) -> Image.Image:
    """Vertical gradient between the source's top and bottom background blues."""
    left, top, right, bottom = box
    x = left + (right - left) // 8  # Away from the cube, inside the body.
    top_rgb = src.getpixel((x, top + 40))[:3]
    bottom_rgb = src.getpixel((x, bottom - 40))[:3]
    grad = Image.new("RGB", (1, size))
    for y in range(size):
        t = y / (size - 1)
        grad.putpixel((0, y), tuple(round(a + (b - a) * t) for a, b in zip(top_rgb, bottom_rgb)))
    return grad.resize((size, size))


def app_icon(src: Image.Image) -> Image.Image:
    box = body_bbox(src)
    left, top, right, bottom = box
    side = min(right - left, bottom - top)
    cx, cy = (left + right) / 2, (top + bottom) / 2
    square = (round(cx - side / 2), round(cy - side / 2), round(cx + side / 2), round(cy + side / 2))

    art = src.crop(square)
    # Flatten onto the sampled gradient so corners and edge haze never show.
    base = background_gradient(src, box, art.width).convert("RGBA")
    base.alpha_composite(art)
    body = base.resize((BODY, BODY), Image.LANCZOS)
    body.putalpha(squircle_mask(BODY))

    canvas = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    offset = (CANVAS - BODY) // 2
    shadow = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    shadow_alpha = Image.new("L", (CANVAS, CANVAS), 0)
    shadow_alpha.paste(squircle_mask(BODY).point(lambda v: v * 0.35), (offset, offset + 12))
    shadow.putalpha(shadow_alpha.filter(ImageFilter.GaussianBlur(14)))
    canvas.alpha_composite(shadow)
    canvas.alpha_composite(body, (offset, offset))
    return canvas


def foreground_alpha(src: Image.Image) -> Image.Image:
    """Coverage of the cube artwork (outline and accents) inside the body."""
    red = src.getchannel("R")
    lo, hi = FOREGROUND_RED - FOREGROUND_SOFT, FOREGROUND_RED + FOREGROUND_SOFT
    ramp = red.point(lambda v: 0 if v <= lo else 255 if v >= hi else round((v - lo) * 255 / (hi - lo)))
    opaque = src.getchannel("A").point(lambda v: 255 if v > 200 else 0)
    return ImageChops.multiply(ramp, opaque)


def fill_holes(mask: Image.Image) -> Image.Image:
    """Solid silhouette: everything not reachable from the border is filled."""
    binary = mask.point(lambda v: 255 if v > 127 else 0)
    flood = binary.copy()
    ImageDraw.floodfill(flood, (0, 0), 128)
    return flood.point(lambda v: 0 if v == 128 else 255)


def glyph(mask: Image.Image, size: int) -> Image.Image:
    bbox = mask.point(lambda v: 255 if v > 127 else 0).getbbox()
    cropped = mask.crop(bbox)
    side = round(max(cropped.size) * (1 + 2 * GLYPH_PADDING))
    square = Image.new("L", (side, side), 0)
    square.paste(cropped, ((side - cropped.width) // 2, (side - cropped.height) // 2))
    small = square.resize((size, size), Image.LANCZOS)
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.putalpha(small)
    return out


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    src = Image.open(sys.argv[1]).convert("RGBA")
    out = Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)

    icon = app_icon(src)
    icon.save(out / "icon-1024.png")
    icon.resize((512, 512), Image.LANCZOS).save(out / "app-icon-512.png")
    with tempfile.TemporaryDirectory() as tmp:
        iconset = Path(tmp) / "AppIcon.iconset"
        iconset.mkdir()
        for points in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                px = points * scale
                name = f"icon_{points}x{points}{'@2x' if scale == 2 else ''}.png"
                icon.resize((px, px), Image.LANCZOS).save(iconset / name)
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(out / "AppIcon.icns")], check=True)

    outline = foreground_alpha(src)
    glyph(outline, GLYPH_PX).save(out / "menubar-template.png")
    # Active: the solid cube with the inner edges and accents cut out. The
    # erosion must be wider than the outer outline stroke (about 25 px plus antialiasing in the
    # 1254 px source) so only the inner lines count as cutouts; it runs at
    # half size to keep MinFilter fast.
    silhouette = fill_holes(outline)
    strokes = outline.point(lambda v: 255 if v > 127 else 0)
    half = (silhouette.width // 2, silhouette.height // 2)
    inside = silhouette.resize(half).filter(ImageFilter.MinFilter(OUTLINE_ERODE_HALF)).resize(silhouette.size)
    cutouts = ImageChops.multiply(strokes, inside)
    active = ImageChops.subtract(silhouette, cutouts)
    glyph(active, GLYPH_PX).save(out / "menubar-template-active.png")
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
