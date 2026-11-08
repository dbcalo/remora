"""Generate the program's icon into this folder: icon.png (256 px, the window
icon on every OS) and icon.ico (16-256 px, compiled into the Windows .exe).

Build-time only; the program never runs this. Regenerate after changing it:

    py assets/make_icon.py

THE DESIGN: a carry case (portable) with a play triangle (Jellyfin), in a
purple-to-blue gradient that echoes Jellyfin's colours without using its logo.
Drawn at 4x and downscaled, so edges stay clean at 16 px. Never edit the PNG or
ICO by hand: the next run overwrites them.
"""
import os

from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))
SIZE = 256
SCALE = 4
TOP = (170, 92, 195)     # purple
BOTTOM = (0, 164, 220)   # blue
WHITE = (255, 255, 255, 255)
MARGIN = 8  # px of 256 left empty round the drawing


def gradient(w, h):
    """A vertical gradient, top to bottom."""
    g = Image.new("RGBA", (w, h))
    px = g.load()
    for y in range(h):
        t = y / (h - 1)
        c = tuple(round(a + (b - a) * t) for a, b in zip(TOP, BOTTOM)) + (255,)
        for x in range(w):
            px[x, y] = c
    return g


def draw():
    n = SIZE * SCALE
    s = lambda v: round(v * SCALE)  # noqa: E731 - design units are 256 px

    # The case's silhouette: a handle above a rounded body. Everything coloured
    # is cut out of one gradient by this mask.
    mask = Image.new("L", (n, n), 0)
    m = ImageDraw.Draw(mask)
    m.rounded_rectangle([s(24), s(64), s(232), s(232)], radius=s(36), fill=255)          # body
    m.rounded_rectangle([s(88), s(30), s(168), s(84)], radius=s(18), fill=255)           # handle
    m.rounded_rectangle([s(108), s(48), s(148), s(70)], radius=s(8), fill=0)             # handle hole

    icon = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    icon.paste(gradient(n, n), (0, 0), mask)

    # The play triangle, centred optically in the body (shifted right of the
    # geometric centre, as a play symbol always is).
    d = ImageDraw.Draw(icon)
    cx, cy = 136, 148
    d.polygon([(s(cx - 34), s(cy - 46)), (s(cx - 34), s(cy + 46)), (s(cx + 46), s(cy))], fill=WHITE)
    # Fill the square, as other taskbar icons do: the drawing is cropped to what
    # it covers and scaled to leave MARGIN all round (1 px at 32 px). Without
    # this it sat ~10% inside its square and looked smaller than its neighbours.
    icon = icon.crop(icon.getbbox())
    room = n - 2 * MARGIN * SCALE
    k = room / max(icon.size)
    icon = icon.resize((round(icon.width * k), round(icon.height * k)), Image.LANCZOS)
    out = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    out.paste(icon, ((n - icon.width) // 2, (n - icon.height) // 2))
    return out.resize((SIZE, SIZE), Image.LANCZOS)


def main():
    icon = draw()
    png = os.path.join(HERE, "icon.png")
    ico = os.path.join(HERE, "icon.ico")
    icon.save(png)
    icon.save(ico, sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    print("wrote", png, "and", ico)


if __name__ == "__main__":
    main()
