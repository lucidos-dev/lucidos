#!/usr/bin/env python3
"""Generate the macOS menu-bar template icon (icons/tray-template.png).

macOS status-bar items are *template images*: a transparent-background
silhouette the system renders monochrome and inverts for light/dark menu bars.
This draws only the Lucidos logo glyph (the three rounded squares + sparkle from
icons/icon-source.svg) on a transparent background in solid black — template
mode keys off the alpha channel, so the RGB is irrelevant but the alpha must be
the silhouette, not the full filled app-icon square.

Run from crates/lucidos-app/:  python3 icons/gen-tray-template.py
Requires Pillow (PIL).
"""
import math

from PIL import Image, ImageDraw

SS = 16          # supersample factor for crisp anti-aliasing
S = 36           # canvas HEIGHT in px (== 18pt @2x, the canonical template size)
FILL = 0.94      # fraction of that height the glyph spans. The template carries
                 # only the tiles and the sparkle, with no blue background frame.
                 # So the glyph nearly fills the canvas. What is left is a thin
                 # margin above and below, clearing the menu bar's own edges.

# Glyph geometry copied verbatim from public/icons/icon-source.svg (the logo <g>,
# pre-translate/scale, background <rect> dropped). Content bbox: x[17,87] y[12,83].
minx, miny, maxx, maxy = 17, 12, 87, 83
cw, ch = maxx - minx, maxy - miny
# The height alone sets the size. macOS scales a status-item image to 18pt
# tall, then takes its width from the canvas aspect ratio. So a canvas wider
# than the glyph is padding the bar still measures.
scale = FILL * S / ch
# AppKit lays the unread count out from the image's right edge, and adds a
# fixed 2pt gap of its own past it. A transparent column here is more daylight
# on top of that, so the canvas is exactly as wide as the glyph. Rounding up to
# a whole pixel leaves a sliver, and it goes on the LEFT, where the bar's own
# padding of about 10pt swallows it.
W = math.ceil(cw * scale)
ox = W - cw * scale
oy = (S - ch * scale) / 2


def mx(x):
    return (ox + (x - minx) * scale) * SS


def my(y):
    return (oy + (y - miny) * scale) * SS


img = Image.new("RGBA", (W * SS, S * SS), (0, 0, 0, 0))
d = ImageDraw.Draw(img)
BLACK = (0, 0, 0, 255)

# three rounded squares
for rx, ry in [(17, 17), (17, 54), (54, 54)]:
    d.rounded_rectangle(
        [mx(rx), my(ry), mx(rx + 29), my(ry + 29)],
        radius=7 * scale * SS,
        fill=BLACK,
    )

# four-pointed sparkle — sample the cubic beziers into a filled polygon
segs = [
    ((68.5, 12), (71, 25), (74, 28.5), (87, 31)),
    ((87, 31), (74, 33.5), (71, 37), (68.5, 50)),
    ((68.5, 50), (66, 37), (63, 33.5), (50, 31)),
    ((50, 31), (63, 28.5), (66, 25), (68.5, 12)),
]


def bez(p0, p1, p2, p3, n=40):
    pts = []
    for i in range(n + 1):
        t = i / n
        u = 1 - t
        bx = u * u * u * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t * t * t * p3[0]
        by = u * u * u * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t * t * t * p3[1]
        pts.append((mx(bx), my(by)))
    return pts


poly = []
for s in segs:
    poly.extend(bez(*s))
d.polygon(poly, fill=BLACK)

img = img.resize((W, S), Image.LANCZOS)
img.save("icons/tray-template.png")
print("wrote icons/tray-template.png", img.size)
