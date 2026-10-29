#!/usr/bin/env python3
"""Draw the WARDEN app icon and emit every rendition macOS and Tauri ask for.

The mark is the product's own hero: a wireframe globe with a white-hot core and
two subagents tethered below it. The chrome stays colourless cold steel, exactly
as the app's design system requires, so the only hue in the tile is the pair of
harness identities burning in the two satellite cores (Claude emerald, Codex
violet). The wireframe is projected from real 3D, so the back of the sphere fades
and it reads as a globe rather than a flat target.

Art is per size, not one drawing downscaled: below 64px the satellites and the
node dots turn to grey mush, so those renditions drop them and thicken what is
left. That is what keeps the menu bar and Spotlight legible.

    python3 scripts/make-icon.py            # writes src-tauri/icons/
    python3 scripts/make-icon.py --out DIR
"""

from __future__ import annotations

import argparse
import math
import shutil
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw

SS = 4  # supersample factor; every rendition is drawn large and reduced once

# Cold steel chrome. No hue anywhere except the two satellite cores.
TILE_TOP = (0x17, 0x1F, 0x30)
TILE_BOTTOM = (0x05, 0x07, 0x0C)
LIFT = (0x8E, 0xA2, 0xC4)
BORDER = (0x39, 0x45, 0x5C)
WIRE_FRONT = (0xC3, 0xCD, 0xDE)
WIRE_BACK = (0x4D, 0x58, 0x6E)
CORE = (0xFF, 0xFF, 0xFF)
BLOOM = (0xDC, 0xE6, 0xF5)
TETHER = (0x3D, 0x47, 0x59)
CLAUDE = (0x76, 0xFF, 0x9D)
CODEX = (0xA7, 0x8B, 0xFA)


# --- geometry -------------------------------------------------------------


def squircle(box, n=5.0, steps=1024):
    """Superellipse polygon. Apple's tile is a continuous-curvature squircle,
    which a rounded rectangle visibly is not at large sizes."""
    x0, y0, x1, y1 = box
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    a, b = (x1 - x0) / 2, (y1 - y0) / 2
    pts = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        pts.append(
            (
                cx + a * math.copysign(abs(c) ** (2 / n), c),
                cy + b * math.copysign(abs(s) ** (2 / n), s),
            )
        )
    return pts


def rot_x(p, t):
    x, y, z = p
    c, s = math.cos(t), math.sin(t)
    return (x, y * c - z * s, y * s + z * c)


def rot_z(p, t):
    x, y, z = p
    c, s = math.cos(t), math.sin(t)
    return (x * c - y * s, x * s + y * c, z)


def project(p, cx, cy, r):
    """Orthographic. +z is toward the viewer, so z carries the depth cue."""
    x, y, z = p
    return (cx + x * r, cy - y * r, z)


def sphere_circle(lat=None, lon=None, steps=160):
    """Unit-sphere latitude or longitude circle as 3D points."""
    pts = []
    for i in range(steps + 1):
        t = 2 * math.pi * i / steps
        if lat is not None:
            r = math.cos(lat)
            pts.append((r * math.sin(t), math.sin(lat), r * math.cos(t)))
        else:
            pts.append((math.sin(lon) * math.sin(t), math.cos(t), math.cos(lon) * math.sin(t)))
    return pts


def plane_circle(tilt, spin, radius, steps=200):
    """A circle lying in a plane tilted out of the screen, for the orbit rings."""
    pts = []
    for i in range(steps + 1):
        t = 2 * math.pi * i / steps
        p = (radius * math.cos(t), radius * math.sin(t), 0.0)
        pts.append(rot_z(rot_x(p, tilt), spin))
    return pts


# --- drawing --------------------------------------------------------------


def depth_line(draw, pts, cx, cy, r, front, back, width, tilt, bump=0, floor=60):
    """Draw a projected 3D polyline, fading each segment by its own depth so the
    far side of the sphere sits behind the near side."""
    scr = [project(rot_x(p, tilt), cx, cy, r) for p in pts]
    for (x0, y0, z0), (x1, y1, z1) in zip(scr, scr[1:]):
        d = (z0 + z1) / 2
        # 0 at the back, 1 at the front. The orbit rings reach past the unit
        # sphere, so clamp before the curve or the exponent goes complex.
        k = min(1.0, max(0.0, (d + 1) / 2)) ** 1.6
        col = tuple(round(b + (f - b) * k) for f, b in zip(front, back))
        a = min(255, round(floor + (255 - floor) * k) + bump)
        draw.line([(x0, y0), (x1, y1)], fill=col + (a,), width=width, joint="curve")


def radial(size, color, peak, power):
    """A soft round falloff as an RGBA patch, shaped by `power`."""
    n = max(8, int(size))
    # radial_gradient hits 255 at the CORNERS, so rescale to the inscribed
    # circle or the patch stops short of zero and shows its own square edge.
    k = 255 * math.sqrt(0.5)
    mask = Image.radial_gradient("L").resize((n, n), Image.LANCZOS)
    mask = mask.point(lambda v: round(peak * max(0.0, 1 - min(1.0, v / k)) ** power))
    patch = Image.new("RGBA", (n, n), color + (0,))
    patch.putalpha(mask)
    return patch


def paste_center(dst, patch, cx, cy):
    dst.alpha_composite(patch, (round(cx - patch.width / 2), round(cy - patch.height / 2)))


def draw_globe(img, cx, cy, r, *, tilt, detail, width, core_scale=1.0, core_hue=None):
    floor = 165 if detail == "micro" else 60
    d = ImageDraw.Draw(img, "RGBA")

    if detail == "micro":
        lats, lons, rings = [0.0], [0.0], [(1.30, math.radians(74), math.radians(-20))]
    elif detail == "small":
        lats = [0.0, math.radians(40), math.radians(-40)]
        lons = [0.0, math.radians(60), math.radians(-60)]
        rings = [
            (1.30, math.radians(72), math.radians(-20)),
            (1.30, math.radians(68), math.radians(38)),
        ]
    else:
        lats = [0.0, math.radians(38), math.radians(-38), math.radians(66), math.radians(-66)]
        lons = [0.0, math.radians(36), math.radians(-36), math.radians(72), math.radians(-72)]
        rings = [
            (1.28, math.radians(72), math.radians(-20)),
            (1.32, math.radians(66), math.radians(38)),
        ]

    # Orbit rings sit outside the sphere and are what make it read as watched.
    for rad, tl, sp in rings:
        depth_line(
            d, plane_circle(tl, sp, rad), cx, cy, r, WIRE_FRONT, WIRE_BACK,
            width, tilt, bump=45, floor=floor,
        )

    for lat in lats:
        depth_line(d, sphere_circle(lat=lat), cx, cy, r, WIRE_FRONT, WIRE_BACK, width, tilt, floor=floor)
    for lon in lons:
        depth_line(d, sphere_circle(lon=lon), cx, cy, r, WIRE_FRONT, WIRE_BACK, width, tilt, floor=floor)

    # The limb: the one stroke that has to survive every size.
    d.ellipse([cx - r, cy - r, cx + r, cy + r], outline=WIRE_FRONT + (215,), width=width)

    if detail == "full":
        for lat in (math.radians(38), math.radians(-38), 0.0):
            for i in range(10):
                lon = 2 * math.pi * i / 10
                p = rot_x(
                    (
                        math.cos(lat) * math.sin(lon),
                        math.sin(lat),
                        math.cos(lat) * math.cos(lon),
                    ),
                    tilt,
                )
                x, y, z = project(p, cx, cy, r)
                k = ((z + 1) / 2) ** 1.6
                s = r * (0.012 + 0.020 * k)
                col = tuple(round(b + (f - b) * k) for f, b in zip(WIRE_FRONT, WIRE_BACK))
                d.ellipse([x - s, y - s, x + s, y + s], fill=col + (round(70 + 185 * k),))

    # The heart, contained well inside the wireframe. On a satellite the harness
    # hue has to lead: a white core at parent strength swallows it whole.
    if core_hue:
        paste_center(img, radial(r * 1.10 * core_scale, core_hue, 185, 2.3), cx, cy)
        paste_center(img, radial(r * 0.46 * core_scale, CORE, 205, 1.6), cx, cy)
        hot = tuple(round(h * 0.45 + w * 0.55) for h, w in zip(core_hue, CORE))
    else:
        paste_center(img, radial(r * 1.15 * core_scale, BLOOM, 68, 2.6), cx, cy)
        paste_center(img, radial(r * 0.62 * core_scale, CORE, 235, 1.7), cx, cy)
        hot = CORE
    hr = r * 0.15 * core_scale
    d.ellipse([cx - hr, cy - hr, cx + hr, cy + hr], fill=hot + (255,))


def render(n: int) -> Image.Image:
    T = n * SS
    detail = "micro" if n <= 64 else ("small" if n < 256 else "full")

    img = Image.new("RGBA", (T, T), (0, 0, 0, 0))

    # Tile: a lit slab of cold steel, not a flat near-black rectangle.
    grad = Image.new("RGBA", (T, T))
    gd = ImageDraw.Draw(grad)
    for y in range(T):
        k = (y / max(1, T - 1)) ** 0.85
        gd.line(
            [(0, y), (T, y)],
            fill=tuple(round(a + (b - a) * k) for a, b in zip(TILE_TOP, TILE_BOTTOM)) + (255,),
        )

    box = (0.098 * T, 0.088 * T, 0.902 * T, 0.912 * T)
    mask = Image.new("L", (T, T), 0)
    ImageDraw.Draw(mask).polygon(squircle(box), fill=255)
    img.paste(grad, (0, 0), mask)

    body = Image.new("RGBA", (T, T), (0, 0, 0, 0))
    bd = ImageDraw.Draw(body, "RGBA")

    sats = []
    if detail == "micro":
        gcx, gcy, gr = T * 0.5, T * 0.5, T * 0.315
        w = max(SS, round(T * 0.0078))
        core_scale = 1.15
    else:
        gcx, gcy, gr = T * 0.5, T * 0.398, T * 0.213
        w = max(SS, round(T * 0.0030))
        core_scale = 1.0

        sr = T * 0.059
        sats = [(T * 0.338, T * 0.726, CLAUDE), (T * 0.662, T * 0.714, CODEX)]
        tw = max(SS, round(T * 0.0022))
        for sx, sy, hue in sats:
            ax = gcx + (sx - gcx) * 0.30
            ay = gcy + gr * 0.90
            bd.line([(ax, ay), (sx, sy)], fill=TETHER + (200,), width=tw)

    draw_globe(
        body, gcx, gcy, gr,
        tilt=math.radians(20), detail=detail, width=w, core_scale=core_scale,
    )

    for sx, sy, hue in sats:
        draw_globe(
            body, sx, sy, sr,
            tilt=math.radians(20), detail="micro",
            width=max(SS, round(T * 0.0022)), core_scale=1.25, core_hue=hue,
        )

    img.alpha_composite(Image.composite(body, Image.new("RGBA", (T, T), (0, 0, 0, 0)), mask))

    # Depth the way the app does it: a hairline border, then one inset top
    # highlight. No wide diffuse glow anywhere.
    edge = Image.new("RGBA", (T, T), (0, 0, 0, 0))
    ed = ImageDraw.Draw(edge, "RGBA")
    bw = max(SS, round(T * 0.0022))
    ring = squircle(box)
    ed.line(ring + [ring[0]], fill=BORDER + (190,), width=bw, joint="curve")
    img.alpha_composite(edge)

    x0, y0, x1, y1 = box
    inset = bw * 1.6
    lift = Image.new("RGBA", (T, T), (0, 0, 0, 0))
    ld = ImageDraw.Draw(lift, "RGBA")
    inner = squircle((x0 + inset, y0 + inset, x1 - inset, y1 - inset))
    ld.line(inner + [inner[0]], fill=LIFT + (46,), width=bw, joint="curve")
    fade = Image.new("L", (T, T))
    fd = ImageDraw.Draw(fade)
    for y in range(T):
        fd.line([(0, y), (T, y)], fill=max(0, round(255 * (1 - (y / T) / 0.42))))
    lift.putalpha(Image.composite(lift.getchannel("A"), Image.new("L", (T, T), 0), fade))
    img.alpha_composite(lift)

    return img.resize((n, n), Image.LANCZOS)


def render_tray(n: int = 44) -> Image.Image:
    """The menu-bar rendition: a macOS TEMPLATE image, so it carries shape in the
    alpha channel only and the system tints it for a light or dark menu bar. The
    colour tile would be wrong here; nothing in the menu bar has a background."""
    T = n * SS
    img = Image.new("RGBA", (T, T), (0, 0, 0, 0))
    draw_globe(
        img, T * 0.5, T * 0.5, T * 0.335,
        tilt=math.radians(20), detail="micro",
        width=max(SS, round(T * 0.0090)), core_scale=1.05,
    )
    # Menu-bar glyphs beside this one are solid, so the depth fade cannot ride
    # into the alpha or the icon reads washed out next to wifi and bluetooth.
    # Lift everything already drawn to full weight, keeping only its coverage.
    img.putalpha(img.getchannel("A").point(lambda v: min(255, round(v * 1.75))))
    # A template is judged on alpha alone, so flatten every hue to white and let
    # the existing depth fade survive as coverage.
    a = img.getchannel("A")
    out = Image.new("RGBA", (T, T), (255, 255, 255, 0))
    out.putalpha(a)
    return out.resize((n, n), Image.LANCZOS)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--out",
        default=str(Path(__file__).resolve().parent.parent / "src-tauri" / "icons"),
    )
    args = ap.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    iconset = out / "warden.iconset"
    if iconset.exists():
        shutil.rmtree(iconset)
    iconset.mkdir()

    cache: dict[int, Image.Image] = {}

    def at(n: int) -> Image.Image:
        if n not in cache:
            cache[n] = render(n)
        return cache[n]

    for base in (16, 32, 128, 256, 512):
        at(base).save(iconset / f"icon_{base}x{base}.png")
        at(base * 2).save(iconset / f"icon_{base}x{base}@2x.png")

    icns = out / "icon.icns"
    subprocess.run(
        ["iconutil", "-c", "icns", str(iconset), "-o", str(icns)], check=True
    )
    shutil.rmtree(iconset)

    at(32).save(out / "32x32.png")
    at(128).save(out / "128x128.png")
    at(256).save(out / "128x128@2x.png")
    at(1024).save(out / "icon.png")
    render_tray(44).save(out / "tray.png")
    render_tray(88).save(out / "tray@2x.png")

    print(f"wrote {icns} and {len(list(out.glob('*.png')))} PNGs to {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
