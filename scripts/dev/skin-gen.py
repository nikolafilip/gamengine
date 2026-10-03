#!/usr/bin/env python3
"""The first skin (LOOK.md 2.2), drawn procedurally: dark leather panels in a bronze frame,
sunken wells and slots, bevelled buttons, bars, coins and marks. Writes every piece
`assets/content/ui/skin.toml` names, as PNGs beside it. A person who draws better ones
replaces the files; the tool does not care where a PNG came from (CONTENT.md 8).

    scripts/dev/skin-gen.py [assets/content/ui]
"""
import os
import random
import sys

from PIL import Image, ImageDraw

OUT = sys.argv[1] if len(sys.argv) > 1 else "assets/content/ui"
random.seed(14)

BRONZE = (176, 138, 60)
BRONZE_LIGHT = (222, 190, 110)
BRONZE_DARK = (96, 68, 26)
LEATHER = (38, 30, 26)
LEATHER_LIGHT = (62, 50, 42)
LEATHER_DARK = (22, 17, 15)
WELL = (16, 13, 12)
GOLD = (245, 204, 66)
SILVER = (205, 212, 224)
GREEN = (96, 190, 90)
RED = (200, 70, 60)
STEEL = (120, 124, 134)


def grain(img, amount=6, alpha_only_where_opaque=True):
    """A little noise so a stretched middle does not look flat."""
    px = img.load()
    w, h = img.size
    for y in range(h):
        for x in range(w):
            r, g, b, a = px[x, y]
            if a == 0:
                continue
            n = random.randint(-amount, amount)
            px[x, y] = (max(0, min(255, r + n)), max(0, min(255, g + n)), max(0, min(255, b + n)), a)


def bevel(d, box, light, dark, width=1):
    x0, y0, x1, y1 = box
    for i in range(width):
        d.line([(x0 + i, y0 + i), (x1 - i, y0 + i)], fill=light)
        d.line([(x0 + i, y0 + i), (x0 + i, y1 - i)], fill=light)
        d.line([(x0 + i, y1 - i), (x1 - i, y1 - i)], fill=dark)
        d.line([(x1 - i, y0 + i), (x1 - i, y1 - i)], fill=dark)


def panel(w, h, fill=LEATHER, border=4, rim=BRONZE, title_bar=False):
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    # The frame: bronze with a bevel, a dark hairline inside and out.
    d.rectangle([0, 0, w - 1, h - 1], fill=rim)
    bevel(d, (0, 0, w - 1, h - 1), BRONZE_LIGHT, BRONZE_DARK, 1)
    d.rectangle([border - 1, border - 1, w - border, h - border], fill=LEATHER_DARK)
    d.rectangle([border, border, w - border - 1, h - border - 1], fill=fill)
    if title_bar:
        d.rectangle([border, border, w - border - 1, h - border - 1], fill=LEATHER_DARK)
        d.line([(border, h - border - 1), (w - border - 1, h - border - 1)], fill=BRONZE)
    grain(img, 5)
    return img


def well(w, h, fill=WELL, rim=None):
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, w - 1, h - 1], fill=fill)
    # Sunken: dark on top and left, light below and right.
    bevel(d, (0, 0, w - 1, h - 1), LEATHER_DARK, LEATHER_LIGHT, 1)
    if rim:
        d.rectangle([0, 0, w - 1, h - 1], outline=rim)
    grain(img, 4)
    return img


def button(w, h, top=(96, 74, 38), bottom=(64, 48, 24), light=BRONZE_LIGHT, dark=BRONZE_DARK, sunken=False):
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    for y in range(h):
        t = y / max(1, h - 1)
        c = tuple(int(top[k] * (1 - t) + bottom[k] * t) for k in range(3))
        d.line([(0, y), (w - 1, y)], fill=c)
    if sunken:
        bevel(d, (0, 0, w - 1, h - 1), dark, light, 1)
    else:
        bevel(d, (0, 0, w - 1, h - 1), light, dark, 1)
    d.rectangle([0, 0, w - 1, h - 1], outline=LEATHER_DARK)
    bevel(d, (1, 1, w - 2, h - 2), light if not sunken else dark, dark if not sunken else light, 1)
    grain(img, 4)
    return img


def slot(size=36, rim=None, dim=False):
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, size - 1, size - 1], fill=LEATHER_DARK if not dim else (28, 24, 22))
    d.rectangle([1, 1, size - 2, size - 2], fill=WELL if not dim else (26, 22, 20))
    bevel(d, (1, 1, size - 2, size - 2), LEATHER_DARK, LEATHER_LIGHT, 1)
    if rim:
        d.rectangle([0, 0, size - 1, size - 1], outline=rim)
        d.rectangle([1, 1, size - 2, size - 2], outline=rim)
    grain(img, 3)
    return img


def bar_frame():
    img = Image.new("RGBA", (24, 12), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, 23, 11], fill=LEATHER_DARK)
    d.rectangle([0, 0, 23, 11], outline=BRONZE_DARK)
    d.rectangle([2, 2, 21, 9], fill=WELL)
    return img


def bar_fill():
    img = Image.new("RGBA", (8, 8), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    for y in range(8):
        v = 255 - int(abs(y - 2.5) * 22)
        d.line([(0, y), (7, y)], fill=(v, v, v, 255))
    return img


def portrait_frame():
    s = 48
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, s - 1, s - 1], fill=BRONZE)
    bevel(d, (0, 0, s - 1, s - 1), BRONZE_LIGHT, BRONZE_DARK, 2)
    d.rectangle([5, 5, s - 6, s - 6], fill=LEATHER_DARK)
    d.rectangle([6, 6, s - 7, s - 7], fill=(40, 36, 44))
    # Corner studs.
    for x, y in [(2, 2), (s - 4, 2), (2, s - 4), (s - 4, s - 4)]:
        d.rectangle([x, y, x + 1, y + 1], fill=BRONZE_LIGHT)
    grain(img, 4)
    return img


def tooltip():
    img = Image.new("RGBA", (32, 32), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, 31, 31], fill=(14, 12, 12, 236))
    d.rectangle([0, 0, 31, 31], outline=BRONZE_DARK)
    d.rectangle([1, 1, 30, 30], outline=BRONZE)
    return img


def check(on):
    img = Image.new("RGBA", (16, 16), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, 15, 15], fill=WELL)
    bevel(d, (0, 0, 15, 15), LEATHER_DARK, LEATHER_LIGHT, 1)
    d.rectangle([0, 0, 15, 15], outline=BRONZE_DARK)
    if on:
        d.line([(3, 8), (7, 12), (13, 4)], fill=GOLD, width=2)
    return img


def rail(w, h):
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, w - 1, h - 1], fill=WELL)
    bevel(d, (0, 0, w - 1, h - 1), LEATHER_DARK, LEATHER_LIGHT, 1)
    return img


def knob(w, h):
    return button(w, h, top=(140, 110, 52), bottom=(90, 68, 30))


def cursor(drag=False):
    img = Image.new("RGBA", (16, 24), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    if not drag:
        pts = [(0, 0), (0, 17), (4, 13), (7, 20), (10, 19), (7, 12), (12, 12)]
        d.polygon(pts, fill=(240, 236, 220), outline=(20, 16, 14))
    else:
        # A closed hand.
        d.rounded_rectangle([2, 8, 13, 20], radius=3, fill=(240, 236, 220), outline=(20, 16, 14))
        for x in [3, 6, 9]:
            d.rectangle([x, 5, x + 2, 9], fill=(240, 236, 220), outline=(20, 16, 14))
    return img


def coin(colour, dark):
    img = Image.new("RGBA", (12, 12), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.ellipse([0, 0, 11, 11], fill=dark)
    d.ellipse([1, 1, 10, 10], fill=colour)
    d.ellipse([3, 3, 8, 8], outline=dark)
    d.point((4, 3), fill=(255, 255, 240))
    return img


def mark(kind):
    img = Image.new("RGBA", (12, 12), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    if kind == "new":
        d.ellipse([0, 0, 11, 11], fill=GOLD, outline=BRONZE_DARK)
        d.polygon([(6, 2), (7, 5), (10, 5), (8, 7), (9, 10), (6, 8), (3, 10), (4, 7), (2, 5), (5, 5)], fill=(90, 60, 10))
    elif kind == "taken":
        d.ellipse([0, 0, 11, 11], fill=RED, outline=(80, 20, 20))
        d.line([(3, 3), (8, 8)], fill=(255, 230, 230), width=2)
        d.line([(8, 3), (3, 8)], fill=(255, 230, 230), width=2)
    else:
        d.ellipse([0, 0, 11, 11], fill=GREEN, outline=(30, 80, 30))
        d.line([(3, 6), (5, 9), (9, 3)], fill=(240, 255, 240), width=2)
    return img


PIECES = {
    "panel": (panel(48, 48), [10, 10, 10, 10]),
    "panel_title": (panel(48, 24, title_bar=True), [10, 4, 10, 6]),
    "well": (well(24, 24), [3, 3, 3, 3]),
    "button": (button(32, 24), [6, 6, 6, 6]),
    "button_hot": (button(32, 24, top=(128, 100, 52), bottom=(86, 64, 30)), [6, 6, 6, 6]),
    "button_down": (button(32, 24, top=(60, 46, 22), bottom=(84, 64, 32), sunken=True), [6, 6, 6, 6]),
    "button_off": (button(32, 24, top=(62, 58, 54), bottom=(44, 42, 40), light=(92, 88, 82), dark=(30, 28, 26)), [6, 6, 6, 6]),
    "field": (well(24, 24, rim=BRONZE_DARK), [4, 4, 4, 4]),
    "field_focus": (well(24, 24, rim=GOLD), [4, 4, 4, 4]),
    "slot": (slot(), [3, 3, 3, 3]),
    "slot_hot": (slot(rim=BRONZE_LIGHT), [3, 3, 3, 3]),
    "slot_picked": (slot(rim=GOLD), [3, 3, 3, 3]),
    "slot_worn": (slot(rim=GREEN), [3, 3, 3, 3]),
    "slot_off": (slot(dim=True), [3, 3, 3, 3]),
    "bar_frame": (bar_frame(), [3, 3, 3, 3]),
    "bar_fill": (bar_fill(), [0, 0, 0, 0]),
    "portrait_frame": (portrait_frame(), [8, 8, 8, 8]),
    "hotbar_cell": (slot(40, rim=BRONZE_DARK), [4, 4, 4, 4]),
    "hotbar_key": (button(16, 12, top=(50, 40, 30), bottom=(30, 24, 18)), [3, 3, 3, 3]),
    "tooltip": (tooltip(), [6, 6, 6, 6]),
    "check_off": (check(False), [0, 0, 0, 0]),
    "check_on": (check(True), [0, 0, 0, 0]),
    "slider_rail": (rail(16, 8), [3, 3, 3, 3]),
    "slider_knob": (knob(12, 16), [3, 3, 3, 3]),
    "scroll_rail": (rail(8, 16), [3, 3, 3, 3]),
    "scroll_knob": (knob(8, 16), [2, 3, 2, 3]),
    "cursor": (cursor(), [0, 0, 0, 0]),
    "cursor_drag": (cursor(True), [0, 0, 0, 0]),
    "coin_gold": (coin(GOLD, (140, 100, 20)), [0, 0, 0, 0]),
    "coin_silver": (coin(SILVER, (90, 96, 110)), [0, 0, 0, 0]),
    "mark_new": (mark("new"), [0, 0, 0, 0]),
    "mark_taken": (mark("taken"), [0, 0, 0, 0]),
    "mark_worn": (mark("worn"), [0, 0, 0, 0]),
}

os.makedirs(OUT, exist_ok=True)
lines = [
    "# The skin (LOOK.md 2.2): the faces the tool rasterises and the pieces of the toolkit,",
    "# each a PNG beside this file with its nine-slice insets (left, top, right, bottom).",
    "# scripts/dev/skin-gen.py wrote the first pieces; replace any file with a better one.",
    "",
    "[font.text]",
    'file = "pixelify_sans.ttf"',
    "size = 12",
    "",
    "[font.title]",
    'file = "medievalsharp.ttf"',
    "size = 18",
    "",
]
for name, (img, inset) in PIECES.items():
    img.save(os.path.join(OUT, f"{name}.png"))
    lines.append(f"[piece.{name}]")
    lines.append(f'file = "{name}.png"')
    if any(inset):
        lines.append(f"inset = [{', '.join(str(v) for v in inset)}]")
    lines.append("")
with open(os.path.join(OUT, "skin.toml"), "w") as f:
    f.write("\n".join(lines))
print(f"skin: {len(PIECES)} pieces written to {OUT}")
