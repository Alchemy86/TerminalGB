#!/usr/bin/env python3
"""Generate the TerminalGB brand SVGs.

Every letterform is hand-drawn here as a stroked skeleton path — no font is
embedded, subset or traced, so there is no third-party licence in these files.
Run from the repo root:  python3 docs/brand/generate.py
It rewrites the SVGs deterministically: the FINAL mark into docs/brand/
(terminalgb-logo.svg + terminalgb-icon.svg — the only files to ship) and the
three retired exploration options into docs/brand/exploration/.
"""

import os

OUT = os.path.dirname(os.path.abspath(__file__))

# ---------------------------------------------------------------------------
# Palette
BG = "#0d1117"        # near-black panel (matches GitHub dark, works on light)
EDGE = "#30363d"      # faint panel border so the card reads on pure black too
FG = "#f0f3f6"        # wordmark white
GREY = "#8b949e"      # tagline grey
GREEN = "#32d46d"     # terminal phosphor green (option A accent)
DMG_LIGHT = "#9bbc0f"  # DMG LCD lit shade   (option B tile / option C accent)
DMG_DARK = "#0f380f"   # DMG LCD dark shade  (option B letters)

TAGLINE = "A GAME BOY IN YOUR TERMINAL"

# ---------------------------------------------------------------------------
# Stroke-skeleton capital letters. Cap height 100, stroke 26 (half-stroke 13);
# every endpoint is inset 13 so round/square caps land on the ink edge.
S = 13
GLYPHS = {
    'T': (72, ["M13 13 H59", "M36 13 V87"]),
    'E': (56, ["M43 13 H13 V87 H43", "M13 50 H36"]),
    'R': (68, ["M13 87 V13 H36 A19 19 0 0 1 36 51 H13", "M37 54 L54 87"]),
    'M': (86, ["M13 87 V13 L43 57 L73 13 V87"]),
    'I': (26, ["M13 13 V87"]),
    'N': (64, ["M13 87 V13 L51 87 V13"]),
    'A': (76, ["M11 87 L38 15 L65 87", "M23 62 H53"]),
    'L': (54, ["M13 13 V87 H41"]),
    'G': (70, ["M57 13 H13 V87 H57 V56 H42"]),
    'B': (66, ["M13 13 V87", "M13 13 H35 A18 18 0 0 1 35 49 H13",
               "M13 49 H37 A19 19 0 0 1 37 87 H13"]),
    'O': (64, ["M32 13 A19 37 0 1 0 32 87 A19 37 0 1 0 32 13"]),
    'U': (64, ["M13 13 V68 A19 19 0 0 0 51 68 V13"]),
    'Y': (64, ["M13 13 L32 47 L51 13", "M32 47 V87"]),
    ' ': (30, []),
}
TRACK = 8  # tight letter spacing


def word_width(text, track=TRACK, widths=None):
    w = 0
    for i, ch in enumerate(text):
        w += (widths.get(ch) if widths and ch in widths else GLYPHS[ch][0])
        if i < len(text) - 1:
            w += track
    return w


def draw_word(text, x, y, scale, stroke, color, style="round", track=TRACK,
              per_letter_color=None):
    """Return SVG for `text` with the baseline grid's top-left at (x, y)."""
    cap, join = ("round", "round") if style == "round" else ("square", "miter")
    parts = []
    cx = 0.0
    for i, ch in enumerate(text):
        w, paths = GLYPHS[ch]
        col = per_letter_color(i, ch) if per_letter_color else color
        for d in paths:
            parts.append(
                f'<path transform="translate({x + cx * scale:.1f} {y:.1f}) '
                f'scale({scale:.4f})" d="{d}" fill="none" stroke="{col}" '
                f'stroke-width="26" stroke-linecap="{cap}" '
                f'stroke-linejoin="{join}"/>')
        cx += w + track
    return "\n".join(parts)


# ---------------------------------------------------------------------------
# 5x7 pixel capitals for option C (the I is 3 columns wide).
PIX = {
    'T': ["11111", "00100", "00100", "00100", "00100", "00100", "00100"],
    'E': ["11111", "10000", "10000", "11110", "10000", "10000", "11111"],
    'R': ["11110", "10001", "10001", "11110", "10100", "10010", "10001"],
    'M': ["10001", "11011", "10101", "10101", "10001", "10001", "10001"],
    'I': ["111", "010", "010", "010", "010", "010", "111"],
    'N': ["10001", "11001", "10101", "10011", "10001", "10001", "10001"],
    'A': ["01110", "10001", "10001", "11111", "10001", "10001", "10001"],
    'L': ["10000", "10000", "10000", "10000", "10000", "10000", "11111"],
    'G': ["01111", "10000", "10000", "10111", "10001", "10001", "01110"],
    'B': ["11110", "10001", "10001", "11110", "10001", "10001", "11110"],
    'O': ["01110", "10001", "10001", "10001", "10001", "10001", "01110"],
    'U': ["10001", "10001", "10001", "10001", "10001", "10001", "01110"],
    'Y': ["10001", "10001", "01010", "00100", "00100", "00100", "00100"],
    ' ': ["000"] * 7,
}


def pix_word_cols(text):
    return sum(len(PIX[ch][0]) for ch in text) + (len(text) - 1)


def draw_pix_word(text, x, y, p, color, per_letter_color=None):
    """Pixel letters at pixel size p, top-left (x, y).

    Each letter is ONE <path> whose subpaths overlap a little: separate rects
    antialiased at fractional coordinates leave hairline seams between rows,
    but subpaths of a single path are filled once as a union, so they cannot.
    """
    parts = []
    col = 0
    for i, ch in enumerate(text):
        rows = PIX[ch]
        c = per_letter_color(i, ch) if per_letter_color else color
        d = []
        for ry, row in enumerate(rows):
            run = None
            for rx, bit in enumerate(row + "0"):
                if bit == "1" and run is None:
                    run = rx
                elif bit != "1" and run is not None:
                    d.append(f'M{x + (col + run) * p:.2f} {y + ry * p:.2f}'
                             f'h{(rx - run) * p:.2f}v{p + 0.3:.2f}'
                             f'h{-(rx - run) * p:.2f}z')
                    run = None
        if d:
            parts.append(f'<path d="{"".join(d)}" fill="{c}"/>')
        col += len(rows[0]) + 1
    return "\n".join(parts)


def svg(width, height, body, comment):
    return (f"<!-- {comment}\n"
            "     Hand-authored for TerminalGB (MPL-2.0). No font embedded, "
            "subset or traced;\n     letterforms are original stroked paths. "
            "Regenerate with docs/brand/generate.py -->\n"
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} '
            f'{height}" width="{width}" height="{height}" role="img">\n'
            f"{body}\n</svg>\n")


def panel(w, h, rx=24):
    return (f'<rect x="1" y="1" width="{w - 2}" height="{h - 2}" rx="{rx}" '
            f'fill="{BG}" stroke="{EDGE}" stroke-width="2"/>')


def tagline(width, y, scale=0.30, color=GREY):
    tw = word_width(TAGLINE, track=14) * scale
    return draw_word(TAGLINE, (width - tw) / 2, y, scale, 26, color,
                     style="round", track=14)


def write(name, content):
    path = os.path.join(OUT, name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(content)
    print(f"wrote {name} ({len(content)} bytes)")


# ---------------------------------------------------------------------------
# THE FINAL MARK — the one to ship. Composition matched to the maintainer's
# chosen reference: one heavy wordmark; the motif REPLACING one letter (a
# solid DMG-green terminal block cursor stands where the I of TERMINAL would
# be — a block cursor hides the glyph beneath it, exactly what a terminal
# shows); a single accent-colour full stop closing the word; the grey
# wide-tracked tagline underneath. The motif is ours outright — a block
# cursor, nothing resembling any Nintendo mark. Accent = the DMG LCD lit
# shade. A knocked-out inverse-video I was tried at several cell ratios and
# always read as a zero or an empty box; solid is what reads as a cursor.
CURSOR_W = 48      # the character cell the cursor fills, wider than the I
CURSOR_RISE = 18   # the cell extends past cap height, top and bottom


def draw_cursor_cell(x, y0, scale, accent=DMG_LIGHT):
    """The solid block cursor filling its cell, ink-left-edge at x."""
    return (
        f'<rect x="{x:.1f}" y="{y0 - CURSOR_RISE * scale:.1f}" '
        f'width="{CURSOR_W * scale:.1f}" '
        f'height="{(100 + 2 * CURSOR_RISE) * scale:.1f}" '
        f'rx="{6 * scale:.1f}" fill="{accent}"/>')


def final():
    W, H = 1200, 380
    text = "TERMINALGB"
    widths = {'I': CURSOR_W}
    DOT_R = 16  # full stop, bottom-aligned with the letter ink
    scale = 1.36
    total = (word_width(text, widths=widths) + TRACK + 2 * DOT_R) * scale
    x0 = (W - total) / 2
    y0 = 96
    parts = [panel(W, H)]
    cx = 0.0
    for ch in text:
        w = widths.get(ch, GLYPHS[ch][0])
        if ch == 'I':
            parts.append(draw_cursor_cell(x0 + cx * scale, y0, scale))
        else:
            parts.append(draw_word(ch, x0 + cx * scale, y0, scale, 26, FG))
        cx += w + TRACK
    # The single coloured accent: a full stop closing the wordmark.
    parts.append(
        f'<circle cx="{x0 + (cx + DOT_R) * scale:.1f}" '
        f'cy="{y0 + (100 - DOT_R) * scale:.1f}" r="{DOT_R * scale:.1f}" '
        f'fill="{DMG_LIGHT}"/>')
    parts.append(tagline(W, 288))
    write("terminalgb-logo.svg",
          svg(W, H, "\n".join(parts),
              "TerminalGB logo — THE final mark (block cursor + full stop)"))

    # Icon: the end of the wordmark distilled — the solid block cursor with
    # the full stop beside it. Two shapes, both solid, so it survives 16px.
    IW = 128
    bw, bh, r = 52, 100, 11          # block ~1:2, a terminal cell's aspect
    total_w = bw + 14 + 2 * r
    bx = (IW - total_w) / 2
    by = (IW - bh) / 2
    body = [panel(IW, IW, rx=28),
            f'<rect x="{bx}" y="{by}" width="{bw}" height="{bh}" rx="8" '
            f'fill="{DMG_LIGHT}"/>',
            f'<circle cx="{bx + bw + 14 + r}" cy="{by + bh - r}" r="{r}" '
            f'fill="{DMG_LIGHT}"/>']
    write("terminalgb-icon.svg",
          svg(IW, IW, "\n".join(body),
              "TerminalGB icon — THE final mark (the block cursor alone)"))


# ---------------------------------------------------------------------------
# RETIRED exploration, option A — block cursor: TERM[I]NALGB where the I is a phosphor-green block
# cursor with the I knocked out in inverse video, exactly as a terminal draws
# a cursor over a glyph.
def option_a():
    W, H = 1200, 380
    text = "TERMINALGB"
    cursor_w = 64  # a cursor covers the whole character cell, wider than 'I'
    widths = {'I': cursor_w}
    scale = 1.40
    total = word_width(text, widths=widths) * scale
    x0 = (W - total) / 2
    y0 = 96
    parts = [panel(W, H)]
    cx = 0.0
    for i, ch in enumerate(text):
        w = widths.get(ch, GLYPHS[ch][0])
        if ch == 'I':
            bx, bw = x0 + cx * scale, w * scale
            parts.append(
                f'<rect x="{bx:.1f}" y="{y0 - 18 * scale:.1f}" '
                f'width="{bw:.1f}" height="{136 * scale:.1f}" rx="5" '
                f'fill="{GREEN}"/>')
            ins = (w - 18) / 2  # centre the knocked-out I inside the cell
            parts.append(
                f'<path transform="translate({bx + ins * scale:.1f} '
                f'{y0:.1f}) scale({scale:.4f})" d="M9 10 V90" fill="none" '
                f'stroke="{BG}" stroke-width="18" stroke-linecap="butt"/>')
        else:
            parts.append(draw_word(ch, x0 + cx * scale, y0, scale, 26, FG))
        cx += w + TRACK
    parts.append(tagline(W, 288))
    write("exploration/terminalgb-logo-a.svg",
          svg(W, H, "\n".join(parts),
              "TerminalGB logo, option A: the block cursor"))

    # Icon: a white G with the solid block cursor waiting after it.
    IW = 128
    s = 0.78
    gw = GLYPHS['G'][0] * s
    cell = 40 * s
    total = gw + 8 * s + cell
    x0 = (IW - total) / 2
    y0 = (IW - 100 * s) / 2
    body = [panel(IW, IW, rx=28)]
    body.append(draw_word("G", x0, y0, s, 26, FG))
    bx = x0 + gw + 8 * s
    body.append(f'<rect x="{bx:.1f}" y="{y0 - 6 * s:.1f}" width="{cell:.1f}" '
                f'height="{112 * s:.1f}" rx="5" fill="{GREEN}"/>')
    write("exploration/terminalgb-icon-a.svg",
          svg(IW, IW, "\n".join(body), "TerminalGB icon, option A"))


# ---------------------------------------------------------------------------
# RETIRED exploration, option B — the screen tile: TERMINAL in white, GB set dark-on-green inside a
# rounded rectangle: a generic dot-matrix LCD tile in the DMG's two end shades.
def option_b():
    W, H = 1200, 380
    scale = 1.40
    left = "TERMINAL"
    lw = word_width(left) * scale
    gap = 26 * scale
    pad_x, pad_y = 22 * scale, 16 * scale
    gbw = word_width("GB") * scale
    tile_w = gbw + 2 * pad_x
    tile_h = 100 * scale + 2 * pad_y
    total = lw + gap + tile_w
    x0 = (W - total) / 2
    y0 = 96
    parts = [panel(W, H)]
    parts.append(draw_word(left, x0, y0, scale, 26, FG))
    tx = x0 + lw + gap
    parts.append(f'<rect x="{tx:.1f}" y="{y0 - pad_y:.1f}" '
                 f'width="{tile_w:.1f}" height="{tile_h:.1f}" rx="16" '
                 f'fill="{DMG_LIGHT}"/>')
    parts.append(draw_word("GB", tx + pad_x, y0, scale, 26, DMG_DARK))
    parts.append(tagline(W, 288))
    write("exploration/terminalgb-logo-b.svg",
          svg(W, H, "\n".join(parts),
              "TerminalGB logo, option B: the screen tile"))

    # Icon: the tile alone — green rounded square, dark GB.
    IW = 128
    body = [panel(IW, IW, rx=28)]
    body.append(f'<rect x="10" y="10" width="108" height="108" rx="20" '
                f'fill="{DMG_LIGHT}"/>')
    s = 0.60
    gbw = word_width("GB") * s
    body.append(draw_word("GB", (IW - gbw) / 2, (IW - 100 * s) / 2, s, 26,
                          DMG_DARK))
    write("exploration/terminalgb-icon-b.svg",
          svg(IW, IW, "\n".join(body), "TerminalGB icon, option B"))


# ---------------------------------------------------------------------------
# RETIRED exploration, option C — dot matrix: the wordmark built from screen pixels, with a green
# underscore cursor as the single accent.
def option_c():
    W, H = 1200, 380
    text = "TERMINALGB"
    cols = pix_word_cols(text) + 1 + 5  # + space + underscore cell
    p = 1020 / cols
    x0 = (W - cols * p) / 2
    y0 = 92
    parts = [panel(W, H)]
    parts.append(draw_pix_word(text, x0, y0, p, FG))
    ux = x0 + (pix_word_cols(text) + 1) * p
    parts.append(f'<rect x="{ux:.1f}" y="{y0 + 6 * p:.1f}" '
                 f'width="{5 * p:.1f}" height="{p:.1f}" fill="{DMG_LIGHT}"/>')
    parts.append(tagline(W, 288))
    write("exploration/terminalgb-logo-c.svg",
          svg(W, H, "\n".join(parts),
              "TerminalGB logo, option C: dot matrix"))

    # Icon: pixel G plus the green underscore cursor. Every coordinate is a
    # multiple of 16 within the 128 box, so a 16px favicon lands each pixel
    # cell on exactly 2 device pixels — crisp with no resampling blur.
    IW = 128
    p = 16
    body = [panel(IW, IW, rx=24)]
    body.append(draw_pix_word("G", 24, 0, p, FG))
    body.append(f'<rect x="48" y="112" width="48" height="16" '
                f'fill="{DMG_LIGHT}"/>')
    write("exploration/terminalgb-icon-c.svg",
          svg(IW, IW, "\n".join(body), "TerminalGB icon, option C"))


if __name__ == "__main__":
    final()
    option_a()
    option_b()
    option_c()
