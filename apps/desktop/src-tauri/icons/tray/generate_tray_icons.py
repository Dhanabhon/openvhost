#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Generate the four tray/menu-bar template icons (P1 tray design, spec D5;
brand guidelines Sec 3.1/3.2/7.3 as amended 2026-07-31 on this branch).

Run: `python3 generate_tray_icons.py` from this directory (or anywhere --
output paths are resolved relative to this file, not the cwd).

Committed alongside the four PNGs it produces (stopped/running/starting/
failed) so the assets are reproducible from source rather than mystery
binaries -- re-run this script any time the geometry needs to change instead
of hand-editing the PNGs.

## Why a script instead of hand-drawn art

`tray-icon` scales the asset to 18pt in the menu bar, so the shipped PNGs are
36x36 (exactly 2x, for Retina -- an 18px source would be visibly blurry).
macOS draws a `setTemplate(true)` image as an alpha MASK: every RGB byte is
discarded and only the alpha channel survives, re-tinted for light/dark/
tinted menu bars. That means the four states cannot differ by colour (brand
guidelines Sec 7.3, amended on this branch after `setTemplate(true)` was
verified to discard colour) -- they differ by the SHAPE of the dot at the
bracket's dot position, matching the aggregate precedence
`failed > starting > running > stopped` from `docs/design/README.md`:

  - stopped  -- no dot at all
  - running  -- filled dot
  - starting -- half-filled dot (a flat vertical edge, not a thin ring --
    a thin outline stroke is exactly what disappears under the
    anti-aliased downsample this script performs, so the half-fill uses a
    BOLD flat-edged half-disc instead)
  - failed   -- filled dot PLUS a triangular spike, changing the dot's
    outer SILHOUETTE rather than adding fine interior detail. This is a
    deliberate choice: the task this script was written for explicitly
    requires the failed glyph to read at 18pt, not just at the shipped
    36px, and a fine interior mark (a tiny cut-out "!" a couple of pixels
    wide) is precisely what a small, anti-aliased, downsampled glyph loses.
    A silhouette change survives scaling because it is carried by the
    outer edge, which anti-aliasing preserves as a visibly different
    outline even when small.

## Geometry

An opening square bracket `[` with rounded terminals (brand guidelines Sec
3.2's reference SVG: `M42 10 H26 A16 16 0 0 0 10 26 V38 A16 16 0 0 0 26 54
H42`, stroke-width 9, round caps, on a 64x64 canvas) holding a dot at the
bracket's mouth. The proportions below are RE-TUNED, not a literal rescale
of that reference: the brand doc calls Sec 3.2 "a starting point for final
design", and this asset's dot needs to be proportionally larger than the
logomark's (8/64 ~= 12.5% of canvas) to leave room for the failed-state
spike to read at 18pt -- this script's dot is ~33% of canvas width.

The bracket ring is built the same way a stroke-to-fill operation would:
an outer rounded square MINUS an inner rounded square (inset by the stroke
width on every side, corner radius reduced by the same amount) produces a
constant-width rounded ring; erasing everything to the right of a cut line
opens it into a `[`; two circles at the cut line (radius = stroke/2) round
the two flat-cut tips into `stroke-linecap="round"`-equivalent terminals.
This avoids hand-rolling arc/line stroke joins (Pillow does not do
stroke-to-fill) while producing an exactly constant-width, seamlessly
joined outline.

Rendered at SUPERSAMPLE-times the shipped size and downsampled with LANCZOS
for anti-aliased edges -- a template image's alpha channel is read at full
precision (not just 1-bit), so smooth edges render as smooth edges, not a
staircase.
"""

import math
from pathlib import Path

from PIL import Image, ImageDraw

CANVAS = 36  # shipped size: 18pt x 2 (Retina).
SUPERSAMPLE = 12
WORK = CANVAS * SUPERSAMPLE


def s(v: float) -> float:
    """Scale a 36-unit-canvas coordinate/length into the supersampled
    working space."""
    return v * SUPERSAMPLE


# ---------------------------------------------------------------------------
# Bracket geometry, in 36-unit canvas coordinates.
# ---------------------------------------------------------------------------
OUTER_LEFT, OUTER_TOP, OUTER_RIGHT, OUTER_BOTTOM = 3, 3, 33, 33
OUTER_R = 10
STROKE = 6
INNER_LEFT, INNER_TOP = OUTER_LEFT + STROKE, OUTER_TOP + STROKE
INNER_RIGHT, INNER_BOTTOM = OUTER_RIGHT - STROKE, OUTER_BOTTOM - STROKE
INNER_R = OUTER_R - STROKE

# Where the arms are cut open. Must stay inside the OUTER rounded rect's flat
# top/bottom run -- [OUTER_LEFT + OUTER_R, OUTER_RIGHT - OUTER_R] = [13, 23]
# -- so the cut passes through a straight part of the arm, not a curve.
TIP_X = 20
CAP_R = STROKE / 2

# The dot sits in the bracket's mouth: clear of the vertical spine
# (x in [OUTER_LEFT, INNER_LEFT] = [3, 9]) horizontally, and clear of the
# top/bottom arms (y in [3, 9] and [27, 33]) vertically. Verified by
# rendering and visual inspection, not just by the arithmetic in this
# comment (see the task report for how).
DOT_CX, DOT_CY = 19, 18
DOT_R = 6


def draw_bracket(alpha: Image.Image) -> None:
    """Paint the opening-bracket ring onto `alpha` (mode "L", 0=transparent,
    255=opaque). Outer rounded square, minus an inner rounded square (the
    ring's hole), minus everything right of the cut line (opens the ring
    into a bracket), plus two round caps at the cut (the terminals)."""
    draw = ImageDraw.Draw(alpha)
    draw.rounded_rectangle(
        [s(OUTER_LEFT), s(OUTER_TOP), s(OUTER_RIGHT), s(OUTER_BOTTOM)],
        radius=s(OUTER_R),
        fill=255,
    )
    draw.rounded_rectangle(
        [s(INNER_LEFT), s(INNER_TOP), s(INNER_RIGHT), s(INNER_BOTTOM)],
        radius=s(INNER_R),
        fill=0,
    )
    draw.rectangle([s(TIP_X), 0, WORK, WORK], fill=0)

    top_cap_cy = OUTER_TOP + STROKE / 2
    bottom_cap_cy = OUTER_BOTTOM - STROKE / 2
    for cy in (top_cap_cy, bottom_cap_cy):
        draw.ellipse(
            [s(TIP_X - CAP_R), s(cy - CAP_R), s(TIP_X + CAP_R), s(cy + CAP_R)],
            fill=255,
        )


def _dot_bbox(pad: float = 0.0) -> list[float]:
    return [
        s(DOT_CX - DOT_R - pad),
        s(DOT_CY - DOT_R - pad),
        s(DOT_CX + DOT_R + pad),
        s(DOT_CY + DOT_R + pad),
    ]


def draw_dot_running(alpha: Image.Image) -> None:
    """Filled dot -- all services running."""
    ImageDraw.Draw(alpha).ellipse(_dot_bbox(), fill=255)


def draw_dot_starting(alpha: Image.Image) -> None:
    """Half-filled dot -- at least one service starting, none failed.

    A flat-edged half-disc (left half solid, right half erased), not a
    thin ring outline: a hairline stroke is exactly what an anti-aliased
    downsample to 18pt loses. The flat vertical edge is a bold, robust
    shape distinct from both a full disc (running) and no dot (stopped).
    """
    draw = ImageDraw.Draw(alpha)
    draw.ellipse(_dot_bbox(), fill=255)
    pad = 2
    draw.rectangle(
        [s(DOT_CX), s(DOT_CY - DOT_R - pad), s(DOT_CX + DOT_R + pad), s(DOT_CY + DOT_R + pad)],
        fill=0,
    )


def draw_dot_failed(alpha: Image.Image) -> None:
    """Filled dot with a mark -- at least one service failed.

    The "mark" is a triangular spike fused to the dot's upper-right edge,
    deliberately changing the dot's outer SILHOUETTE (a bump/point) rather
    than adding fine interior detail (e.g. a cut-out "!"), because a
    silhouette change is what survives being small and anti-aliased --
    see this file's module docstring.
    """
    draw_dot_running(alpha)
    draw = ImageDraw.Draw(alpha)
    r = DOT_R
    a1 = math.radians(-80)
    a2 = math.radians(-20)
    a_mid = (a1 + a2) / 2
    ax, ay = DOT_CX + r * math.cos(a1), DOT_CY + r * math.sin(a1)
    bx, by = DOT_CX + r * math.cos(a2), DOT_CY + r * math.sin(a2)
    tip_len = r * 2.3
    tx, ty = DOT_CX + tip_len * math.cos(a_mid), DOT_CY + tip_len * math.sin(a_mid)
    draw.polygon(
        [(s(ax), s(ay)), (s(tx), s(ty)), (s(bx), s(by))],
        fill=255,
    )


STATES = {
    "stopped": None,
    "running": draw_dot_running,
    "starting": draw_dot_starting,
    "failed": draw_dot_failed,
}


def build(dot_fn) -> Image.Image:
    alpha = Image.new("L", (WORK, WORK), 0)
    draw_bracket(alpha)
    if dot_fn is not None:
        dot_fn(alpha)
    alpha = alpha.resize((CANVAS, CANVAS), Image.LANCZOS)
    # Template images: only alpha is read, but RGB is set to solid black
    # (rather than left undefined) so the source PNG is sane if ever viewed
    # directly (e.g. this script's own QA previews, or a regression against
    # a non-template consumer).
    out = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 255))
    out.putalpha(alpha)
    return out


def main() -> None:
    out_dir = Path(__file__).resolve().parent
    for name, dot_fn in STATES.items():
        build(dot_fn).save(out_dir / f"{name}.png")
        print(f"wrote {out_dir / f'{name}.png'}")


if __name__ == "__main__":
    main()
