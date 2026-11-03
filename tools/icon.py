"""Draws the Eridu icon: a branch of three nodes, in the colors of what a commit does in the app
(green: added, blue: modified, red: deleted), linked in slate grey. Fluent style: no tile, soft
gradients, the shape fills the frame, so it stays readable at 16 px.

Writes assets/icon.png (256 px, for the window), assets/icon.ico (16 to 256 px, for the
executable) and assets/icon.svg. Run from anywhere: python tools/icon.py (needs Pillow).
"""
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
# Drawn at this size, then scaled down for each size of the icon.
K = 1024
# Positions on a 16-unit grid (the 16 px icon is drawn with these units as pixels).
TRUNK = ((4.2, 12.2), (4.2, 3.8))  # bottom node, top node
SIDE = (11.8, 5.4)
CURVE = ((4.2, 10.0), (11.8, 10.0), SIDE)  # quadratic: from the trunk to the side node
NODE_RADIUS = 2.9
LINK_WIDTH = 1.9
# (light, dark) gradients; the nodes are the app's action colors.
LINK = ("#a3afc6", "#5d6a85")
ADDED = ("#9df0bd", "#2fb862")
MODIFIED = ("#9fd0ff", "#3d84e8")
DELETED = ("#ffa8a0", "#e04848")
NODES = ((TRUNK[0], ADDED, "added"), (TRUNK[1], MODIFIED, "modified"), (SIDE, DELETED, "deleted"))
# Each shape's gradient runs across its own box, from the upper left corner to this point of it
# (in fractions of the box, y down), like SVG's default objectBoundingBox gradients.
SLANT = (0.34, 0.94)
# The links' box (the stroke included). SVG can't use a line's own box: a vertical line's has no
# width, so its gradient, and the line with it, would vanish. Both drawings use this one.
LINK_BOX = (3.25, 2.85, 12.75, 13.15)


def rgb(color: str) -> tuple:
    return tuple(int(color[i:i + 2], 16) for i in (1, 3, 5))


def gradient(colors: tuple, box: tuple) -> Image.Image:
    """A K×K image holding the gradient of `colors` across `box` (in 16ths), as SVG draws it."""
    (a, b), (sx, sy) = map(rgb, colors), SLANT
    u = K / 16
    x0, y0, x1, y1 = (round(v * u) for v in box)
    w, h = x1 - x0, y1 - y0
    # Computed at a quarter of the size, then smoothed up: a gradient has no detail to lose.
    small = Image.new("RGB", (max(w // 4, 1), max(h // 4, 1)))
    norm = sx * sx + sy * sy
    for y in range(small.height):
        for x in range(small.width):
            t = ((x + 0.5) / small.width * sx + (y + 0.5) / small.height * sy) / norm
            t = min(max(t, 0.0), 1.0)
            small.putpixel((x, y), tuple(round(a[c] + (b[c] - a[c]) * t) for c in range(3)))
    img = Image.new("RGBA", (K, K), (0, 0, 0, 0))
    img.paste(small.resize((w, h), Image.BILINEAR), (x0, y0))
    return img


def curve_points(steps: int = 100) -> list:
    (x0, y0), (x1, y1), (x2, y2) = CURVE
    return [
        ((1 - t) ** 2 * x0 + 2 * (1 - t) * t * x1 + t * t * x2, (1 - t) ** 2 * y0 + 2 * (1 - t) * t * y1 + t * t * y2)
        for t in (i / steps for i in range(steps + 1))
    ]


def png() -> Image.Image:
    u = K / 16
    img = Image.new("RGBA", (K, K), (0, 0, 0, 0))
    links = Image.new("L", (K, K), 0)
    d = ImageDraw.Draw(links)
    w = LINK_WIDTH * u
    d.line([(x * u, y * u) for x, y in TRUNK], fill=255, width=round(w))
    points = [(x * u, y * u) for x, y in curve_points()]
    d.line(points, fill=255, width=round(w), joint="curve")
    # Round caps and joints, like the SVG's.
    for x, y in points[::5] + [(x * u, y * u) for x, y in TRUNK]:
        d.ellipse([x - w / 2, y - w / 2, x + w / 2, y + w / 2], fill=255)
    img.paste(gradient(LINK, LINK_BOX), (0, 0), links)
    for (x, y), colors, _ in NODES:
        mask = Image.new("L", (K, K), 0)
        r = NODE_RADIUS * u
        ImageDraw.Draw(mask).ellipse([x * u - r, y * u - r, x * u + r, y * u + r], fill=255)
        node = Image.new("RGBA", (K, K), (0, 0, 0, 0))
        box = (x - NODE_RADIUS, y - NODE_RADIUS, x + NODE_RADIUS, y + NODE_RADIUS)
        node.paste(gradient(colors, box), (0, 0), mask)
        img.alpha_composite(node)
    return img


def svg() -> str:
    def stops(colors: tuple) -> str:
        return f'<stop offset="0" stop-color="{colors[0]}"/><stop offset="1" stop-color="{colors[1]}"/></linearGradient>'

    x0, y0, x1, y1 = LINK_BOX
    x2, y2 = x0 + (x1 - x0) * SLANT[0], y0 + (y1 - y0) * SLANT[1]
    lines = [
        "<!-- Eridu: a branch in the colors of added, modified and deleted files -->",
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">',
        "  <defs>",
        f'    <linearGradient id="link" gradientUnits="userSpaceOnUse" x1="{x0}" y1="{y0}" x2="{x2:.2f}" y2="{y2:.2f}">'
        + stops(LINK),
    ]
    for _, colors, name in NODES:
        lines.append(f'    <linearGradient id="{name}" x1="0" y1="0" x2="{SLANT[0]}" y2="{SLANT[1]}">' + stops(colors))
    (cx0, cy0), (cx1, cy1), (cx2, cy2) = CURVE
    lines += [
        "  </defs>",
        f'  <g fill="none" stroke="url(#link)" stroke-width="{LINK_WIDTH}" stroke-linecap="round">',
        f'    <path d="M{TRUNK[0][0]} {TRUNK[0][1]} L{TRUNK[1][0]} {TRUNK[1][1]}"/>',
        f'    <path d="M{cx0} {cy0} Q{cx1} {cy1} {cx2} {cy2}"/>',
        "  </g>",
    ]
    for (x, y), _, name in NODES:
        lines.append(f'  <circle cx="{x}" cy="{y}" r="{NODE_RADIUS}" fill="url(#{name})"/>')
    lines.append("</svg>")
    return "\n".join(lines) + "\n"


def main() -> None:
    assets = ROOT / "assets"
    assets.mkdir(exist_ok=True)
    big = png()
    big.resize((256, 256), Image.LANCZOS).save(assets / "icon.png")
    # Each size scaled down from the big drawing (Pillow would otherwise resize the 256 px one).
    sizes = (16, 20, 24, 32, 40, 48, 64, 128, 256)
    frames = [big.resize((s, s), Image.LANCZOS) for s in sizes]
    frames[-1].save(assets / "icon.ico", sizes=[(s, s) for s in sizes], append_images=frames[:-1])
    (assets / "icon.svg").write_text(svg(), encoding="utf-8")


if __name__ == "__main__":
    main()
