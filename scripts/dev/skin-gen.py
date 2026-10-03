#!/usr/bin/env python3
"""The skin (LOOK.md 2.2), drawn procedurally: dark leather panels in a bronze frame,
sunken wells and slots, bevelled buttons, bars, coins and marks. Writes every piece
`assets/content/ui/skin.toml` names as PNGs beside it, once per density: `panel.png` at
one texel a dot, `panel@2x.png`, `panel@3x.png` and `panel@4x.png` drawn finer (not
enlarged: a hairline stays a hairline, a curve gets its pixels). A person who draws better
ones replaces the files; the tool does not care where a PNG came from (CONTENT.md 8), and
a density without a file of its own is the one-texel picture enlarged.

A nine-slice's middle is stretched by the client, so nothing in a middle varies along the
way it is stretched: no grain, only what runs straight through.

    scripts/dev/skin-gen.py [assets/content/ui]
"""
import os
import sys

from PIL import Image, ImageDraw

OUT = sys.argv[1] if len(sys.argv) > 1 else "assets/content/ui"
DENSITIES = (1, 2, 3, 4)
# Every picture is drawn this many times larger and averaged down: the curves' edges.
SS = 4

BRONZE = (176, 138, 60)
BRONZE_LIGHT = (226, 196, 120)
BRONZE_DARK = (98, 70, 28)
OUTLINE = (24, 18, 12)
LEATHER = (40, 32, 28)
LEATHER_LIGHT = (58, 47, 40)
LEATHER_DARK = (22, 17, 15)
WELL = (15, 12, 11)
WELL_LIGHT = (30, 25, 22)
GOLD = (245, 204, 66)
SILVER = (205, 212, 224)
GREEN = (96, 190, 90)
RED = (200, 70, 60)
INK = (240, 236, 220)


class Pic:
    """A picture of `w × h` dots at `k` texels a dot, drawn in dots."""

    def __init__(self, w, h, k):
        self.w, self.h, self.k = w, h, k
        self.u = k * SS
        self.img = Image.new("RGBA", (w * self.u, h * self.u), (0, 0, 0, 0))
        self.d = ImageDraw.Draw(self.img)

    def hair(self):
        """A hairline in dots: one texel at one and two texels a dot, two above."""
        return max(1, round(self.k / 2)) / self.k

    def box(self, x0, y0, x1, y1):
        u = self.u
        return [round(x0 * u), round(y0 * u), round(x1 * u) - 1, round(y1 * u) - 1]

    def rect(self, x0, y0, x1, y1, fill, r=0.0):
        b = self.box(x0, y0, x1, y1)
        if b[2] < b[0] or b[3] < b[1]:
            return
        if r > 0:
            self.d.rounded_rectangle(b, radius=round(r * self.u), fill=fill)
        else:
            self.d.rectangle(b, fill=fill)

    def shade(self, x0, y0, x1, y1, top, bottom, r=0.0):
        """A rectangle shaded from `top` to `bottom`, top to bottom."""
        b = self.box(x0, y0, x1, y1)
        w, h = b[2] - b[0] + 1, b[3] - b[1] + 1
        if w <= 0 or h <= 0:
            return
        grad = Image.new("RGBA", (w, h))
        gd = ImageDraw.Draw(grad)
        for y in range(h):
            t = y / max(1, h - 1)
            c = tuple(round(top[i] * (1 - t) + bottom[i] * t) for i in range(3)) + (255,)
            gd.line([(0, y), (w, y)], fill=c)
        mask = Image.new("L", (w, h), 0)
        md = ImageDraw.Draw(mask)
        if r > 0:
            md.rounded_rectangle([0, 0, w - 1, h - 1], radius=round(r * self.u), fill=255)
        else:
            md.rectangle([0, 0, w - 1, h - 1], fill=255)
        self.img.paste(grad, (b[0], b[1]), mask)

    def ellipse(self, x0, y0, x1, y1, fill):
        self.d.ellipse(self.box(x0, y0, x1, y1), fill=fill)

    def line(self, pts, fill, width):
        u = self.u
        self.d.line([(x * u, y * u) for x, y in pts], fill=fill, width=max(1, round(width * u)), joint="curve")

    def polygon(self, pts, fill):
        u = self.u
        self.d.polygon([(x * u, y * u) for x, y in pts], fill=fill)

    def done(self):
        return self.img.resize((self.w * self.k, self.h * self.k), Image.BOX)


def mix(a, b, t):
    return tuple(round(a[i] * (1 - t) + b[i] * t) for i in range(3))


def framed(p, band, r, fill_top, fill_bottom, rim_light=BRONZE_LIGHT, rim=BRONZE, rim_dark=BRONZE_DARK):
    """A bronze frame `band` dots wide round a shaded inside."""
    h = p.hair()
    p.rect(0, 0, p.w, p.h, OUTLINE, r)
    # The band: lit from above, with a bright upper edge.
    p.shade(h, h, p.w - h, p.h - h, rim_light, rim_dark, max(0.0, r - h))
    p.shade(2 * h, 2 * h, p.w - 2 * h, p.h - 2 * h, mix(rim, rim_light, 0.25), mix(rim, rim_dark, 0.35), max(0.0, r - 2 * h))
    # A dark seam, then the inside.
    p.rect(band - h, band - h, p.w - band + h, p.h - band + h, OUTLINE, max(0.0, r - band + h))
    p.shade(band, band, p.w - band, p.h - band, fill_top, fill_bottom, max(0.0, r - band))


def panel(k):
    p = Pic(48, 48, k)
    framed(p, 4, 3, LEATHER_LIGHT, LEATHER)
    return p.done()


def panel_title(k):
    p = Pic(48, 24, k)
    h = p.hair()
    p.shade(0, 0, 48, 24, LEATHER_DARK, mix(LEATHER_DARK, LEATHER, 0.5), 2)
    # The rule under a title.
    p.rect(2, 24 - 3, 46, 24 - 3 + h, BRONZE_DARK)
    p.rect(2, 24 - 3 + h, 46, 24 - 3 + 2 * h, BRONZE)
    return p.done()


def well(k, w=24, h=24, rim=None):
    """Sunken: a shadow under its upper edge, a light lip along its lower one."""
    p = Pic(w, h, k)
    hr = p.hair()
    p.rect(0, 0, w, h, mix(LEATHER, LEATHER_LIGHT, 0.7), 1.5)
    p.rect(0, 0, w, h - hr, OUTLINE, 1.5)
    p.rect(hr, hr, w - hr, h - hr, WELL, 1.0)
    p.rect(hr, hr, w - hr, 2 * hr, (6, 5, 5))
    if rim:
        p.rect(0, 0, w, h, rim, 1.5)
        p.rect(hr, hr, w - hr, h - hr, WELL, 1.0)
    return p.done()


def button(k, w=32, h=24, top=(112, 86, 44), bottom=(70, 52, 26), light=BRONZE_LIGHT, dark=BRONZE_DARK, sunken=False):
    p = Pic(w, h, k)
    hr = p.hair()
    p.rect(0, 0, w, h, OUTLINE, 3)
    if sunken:
        p.shade(hr, hr, w - hr, h - hr, dark, mix(dark, light, 0.4), 3 - hr)
        p.shade(2 * hr, 2 * hr + hr, w - 2 * hr, h - 2 * hr, top, bottom, 3 - 2 * hr)
    else:
        p.shade(hr, hr, w - hr, h - hr, light, dark, 3 - hr)
        p.shade(2 * hr, 2 * hr, w - 2 * hr, h - 2 * hr - hr, top, bottom, 3 - 2 * hr)
    return p.done()


def slot(k, size=36, rim=None, dim=False):
    p = Pic(size, size, k)
    hr = p.hair()
    floor = (26, 22, 20) if dim else WELL
    p.rect(0, 0, size, size, mix(LEATHER, LEATHER_LIGHT, 0.6), 2)
    p.rect(0, 0, size, size - hr, OUTLINE, 2)
    p.shade(hr, hr, size - hr, size - hr, mix(floor, (0, 0, 0), 0.35), mix(floor, WELL_LIGHT, 0.6 if not dim else 0.2), 1.5)
    if rim:
        w = 2 * hr if k > 1 else 2
        p.rect(0, 0, size, size, rim, 2)
        p.shade(w, w, size - w, size - w, mix(floor, (0, 0, 0), 0.35), mix(floor, WELL_LIGHT, 0.6), 1.0)
    return p.done()


def bar_frame(k):
    p = Pic(24, 12, k)
    hr = p.hair()
    p.rect(0, 0, 24, 12, OUTLINE, 2)
    p.shade(hr, hr, 24 - hr, 12 - hr, BRONZE_DARK, mix(BRONZE_DARK, OUTLINE, 0.5), 2 - hr)
    p.rect(2, 2, 22, 10, WELL, 1)
    return p.done()


def bar_fill(k):
    """White, tinted by the client: brighter along its upper third, as a filled tube is."""
    p = Pic(8, 8, k)
    p.shade(0, 0, 8, 3, (236, 236, 236), (255, 255, 255))
    p.shade(0, 3, 8, 8, (240, 240, 240), (150, 150, 150))
    return p.done()


def portrait_frame(k):
    s = 48
    p = Pic(s, s, k)
    framed(p, 5, 4, (46, 42, 52), (30, 27, 34))
    # Corner studs.
    for x, y in [(2.5, 2.5), (s - 2.5, 2.5), (2.5, s - 2.5), (s - 2.5, s - 2.5)]:
        p.ellipse(x - 1.2, y - 1.2, x + 1.2, y + 1.2, BRONZE_DARK)
        p.ellipse(x - 0.8, y - 1.0, x + 0.8, y + 0.6, BRONZE_LIGHT)
    return p.done()


def tooltip(k):
    p = Pic(32, 32, k)
    hr = p.hair()
    p.rect(0, 0, 32, 32, OUTLINE + (245,), 3)
    p.rect(hr, hr, 32 - hr, 32 - hr, BRONZE + (255,), 3 - hr)
    p.rect(2 * hr, 2 * hr, 32 - 2 * hr, 32 - 2 * hr, (16, 13, 13, 242), 3 - 2 * hr)
    return p.done()


def check(k, on):
    p = Pic(16, 16, k)
    hr = p.hair()
    p.rect(0, 0, 16, 16, BRONZE_DARK, 2.5)
    p.rect(hr, hr, 16 - hr, 16 - hr, WELL, 2.5 - hr)
    if on:
        p.line([(3.5, 8.2), (6.8, 11.5), (12.5, 4.5)], GOLD, 2.0)
    return p.done()


def rail(k, w, h):
    p = Pic(w, h, k)
    hr = p.hair()
    p.rect(0, 0, w, h, OUTLINE, 2)
    p.rect(hr, hr, w - hr, h - hr, WELL, 2 - hr)
    return p.done()


def knob(k, w, h):
    return button(k, w, h, top=(150, 118, 58), bottom=(96, 72, 32))


def cursor(k, drag=False):
    p = Pic(16, 24, k)
    if not drag:
        pts = [(1, 1), (1, 17.5), (5, 13.8), (7.8, 20.3), (10.6, 19.1), (7.8, 12.8), (13, 12.8)]
        p.polygon(pts, OUTLINE)
        inner = [(2.2, 3.6), (2.2, 14.8), (5.3, 11.9), (8.4, 18.8), (9.2, 18.4), (6.2, 11.7), (10.3, 11.7)]
        p.polygon(inner, INK)
    else:
        # A closed hand.
        for x in (3, 6, 9):
            p.rect(x, 5, x + 3, 11, OUTLINE, 1.5)
            p.rect(x + 0.6, 5.6, x + 2.4, 11, INK, 0.9)
        p.rect(2, 8, 14, 21, OUTLINE, 3)
        p.rect(2.7, 8.7, 13.3, 20.3, INK, 2.4)
    return p.done()


def coin(k, colour, dark):
    p = Pic(12, 12, k)
    p.ellipse(0, 0, 12, 12, dark)
    p.ellipse(0.9, 0.9, 11.1, 11.1, colour)
    p.ellipse(2.6, 2.6, 9.4, 9.4, dark)
    p.ellipse(3.3, 3.3, 8.7, 8.7, mix(colour, dark, 0.18))
    p.ellipse(3.2, 2.2, 5.6, 4.0, (255, 255, 244))
    return p.done()


def mark(k, kind):
    p = Pic(12, 12, k)
    if kind == "new":
        p.ellipse(0, 0, 12, 12, BRONZE_DARK)
        p.ellipse(0.8, 0.8, 11.2, 11.2, GOLD)
        star = [(6, 1.9), (7.1, 4.7), (10.1, 4.9), (7.8, 6.8), (8.6, 9.8), (6, 8.1), (3.4, 9.8), (4.2, 6.8), (1.9, 4.9), (4.9, 4.7)]
        p.polygon(star, (96, 62, 10))
    elif kind == "taken":
        p.ellipse(0, 0, 12, 12, (86, 22, 22))
        p.ellipse(0.8, 0.8, 11.2, 11.2, RED)
        p.line([(3.6, 3.6), (8.4, 8.4)], (255, 232, 232), 1.6)
        p.line([(8.4, 3.6), (3.6, 8.4)], (255, 232, 232), 1.6)
    else:
        p.ellipse(0, 0, 12, 12, (30, 84, 30))
        p.ellipse(0.8, 0.8, 11.2, 11.2, GREEN)
        p.line([(3.1, 6.2), (5.2, 8.5), (9.0, 3.6)], (242, 255, 242), 1.6)
    return p.done()


PIECES = {
    "panel": (panel, [10, 10, 10, 10]),
    "panel_title": (panel_title, [10, 4, 10, 6]),
    "well": (well, [3, 3, 3, 3]),
    "button": (button, [6, 6, 6, 6]),
    "button_hot": (lambda k: button(k, top=(146, 114, 60), bottom=(94, 70, 34)), [6, 6, 6, 6]),
    "button_down": (lambda k: button(k, top=(62, 47, 23), bottom=(86, 66, 33), sunken=True), [6, 6, 6, 6]),
    "button_off": (lambda k: button(k, top=(64, 60, 56), bottom=(46, 44, 42), light=(96, 92, 86), dark=(32, 30, 28)), [6, 6, 6, 6]),
    "field": (lambda k: well(k, rim=BRONZE_DARK), [4, 4, 4, 4]),
    "field_focus": (lambda k: well(k, rim=GOLD), [4, 4, 4, 4]),
    "slot": (slot, [3, 3, 3, 3]),
    "slot_hot": (lambda k: slot(k, rim=BRONZE_LIGHT), [3, 3, 3, 3]),
    "slot_picked": (lambda k: slot(k, rim=GOLD), [3, 3, 3, 3]),
    "slot_worn": (lambda k: slot(k, rim=GREEN), [3, 3, 3, 3]),
    "slot_off": (lambda k: slot(k, dim=True), [3, 3, 3, 3]),
    "bar_frame": (bar_frame, [3, 3, 3, 3]),
    "bar_fill": (bar_fill, [0, 0, 0, 0]),
    "portrait_frame": (portrait_frame, [8, 8, 8, 8]),
    "hotbar_cell": (lambda k: slot(k, 40, rim=BRONZE_DARK), [4, 4, 4, 4]),
    "hotbar_key": (lambda k: button(k, 16, 12, top=(58, 46, 34), bottom=(34, 27, 20)), [3, 3, 3, 3]),
    "tooltip": (tooltip, [6, 6, 6, 6]),
    "check_off": (lambda k: check(k, False), [0, 0, 0, 0]),
    "check_on": (lambda k: check(k, True), [0, 0, 0, 0]),
    "slider_rail": (lambda k: rail(k, 16, 8), [3, 3, 3, 3]),
    "slider_knob": (lambda k: knob(k, 12, 16), [3, 3, 3, 3]),
    "scroll_rail": (lambda k: rail(k, 8, 16), [3, 3, 3, 3]),
    "scroll_knob": (lambda k: knob(k, 8, 16), [2, 3, 2, 3]),
    "cursor": (cursor, [0, 0, 0, 0]),
    "cursor_drag": (lambda k: cursor(k, True), [0, 0, 0, 0]),
    "coin_gold": (lambda k: coin(k, GOLD, (140, 100, 20)), [0, 0, 0, 0]),
    "coin_silver": (lambda k: coin(k, SILVER, (90, 96, 110)), [0, 0, 0, 0]),
    "mark_new": (lambda k: mark(k, "new"), [0, 0, 0, 0]),
    "mark_taken": (lambda k: mark(k, "taken"), [0, 0, 0, 0]),
    "mark_worn": (lambda k: mark(k, "worn"), [0, 0, 0, 0]),
}

os.makedirs(OUT, exist_ok=True)
lines = [
    "# The skin (LOOK.md 2.2): the faces the tool rasterises and the pieces of the toolkit,",
    "# each a PNG beside this file with its nine-slice insets in dots (left, top, right,",
    "# bottom). `name.png` is the piece at one texel a dot; `name@2x.png`, `@3x` and `@4x`",
    "# are the same piece drawn finer, for the atlases of the larger UI scales (a density",
    "# without a file is the first one enlarged). scripts/dev/skin-gen.py wrote these;",
    "# replace any file with a better one.",
    "",
    "[font.text]",
    'file = "fira_sans_medium.ttf"',
    "size = 15.5",
    "",
    "[font.title]",
    'file = "medievalsharp.ttf"',
    "size = 20",
    "",
]
for name, (draw, inset) in PIECES.items():
    for k in DENSITIES:
        suffix = "" if k == 1 else f"@{k}x"
        draw(k).save(os.path.join(OUT, f"{name}{suffix}.png"), optimize=True)
    lines.append(f"[piece.{name}]")
    lines.append(f'file = "{name}.png"')
    if any(inset):
        lines.append(f"inset = [{', '.join(str(v) for v in inset)}]")
    lines.append("")
with open(os.path.join(OUT, "skin.toml"), "w") as f:
    f.write("\n".join(lines))
print(f"skin: {len(PIECES)} pieces at {len(DENSITIES)} densities written to {OUT}")
