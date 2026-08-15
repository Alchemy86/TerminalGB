"""Read a test ROM's own printed verdict back out of the frame it drew.

Some of Blargg's ROMs never touch the link port: `mem_timing-2`, `halt_bug`,
`interrupt_time` and `oam_bug` print their result **on screen** and nowhere
else. Scoring those by comparing pixels against a reference frame would mean
shipping a reference frame produced by *this* project — and the guiding rule of
this whole tool is that no check's expected output may be defined by our
emulator (see `docs/conformance-action.md` §2). So they were left out.

This module removes that exclusion without breaking the rule. It reads the
**text** off the frame and lets the ROM's own printed `Passed` / `Failed` be
the verdict, exactly as the serial checks do. Nothing here is compared against
anything of ours.

How it can possibly work without a font
---------------------------------------

Blargg's print routine (gb-test-roms `common/`) loads a font whose glyphs are
8x8 and stored **in the ROM under test**, one glyph per character in ASCII
order. So every glyph on the screen is a byte-for-byte copy of eight bytes that
are already in the cartridge we were handed. That gives us the character table
for free, from the ROM itself:

1. Cut the frame into 8x8 cells and reduce each to a 1-bit mask (ink / not).
2. Search the ROM for each cell's eight bytes. Every non-blank glyph is found,
   and at one place: the font.
3. Because the font is contiguous and in ASCII order, `offset // 8` is the
   character code up to one unknown constant. Recover the constant by trying
   every value that keeps all glyphs printable and keeping the one whose
   decoded words appear **verbatim in the ROM** — every one of these ROMs
   carries its own title and the strings `Passed` / `Failed` as plain ASCII.

Step 3 is what makes this self-validating: a wrong alignment decodes to noise,
and noise does not appear in the ROM. If no candidate scores, we say we could
not read the screen rather than guessing.

Two wrinkles that real frames actually have, both handled:

* **Inverse video.** `oam_bug` prints a failing sub-test's code white-on-black.
  Such a cell is the glyph's exact complement, so a failed lookup is retried
  against `~cell`.
* **A scrolled console.** If the ROM has scrolled the background by a
  non-multiple of 8 the cell grid is offset; the sub-cell origin is searched
  (0..7 in each axis) and the alignment that finds the most glyphs wins.

None of this needs anything new from an adapter: it consumes the same
`GBCONFORM_FRAME_OUT` frame the dmg-acid2 check already asks for, so every
adapter with a screenshot flag gets these checks with no change at all.
"""

from __future__ import annotations

import collections

BLANK = b"\x00" * 8
CELL = 8

# A run of this many adjacent characters that is also present in the ROM is
# what pins the character table's origin. Three is enough to be discriminating
# (`ok`, `IE` and single digits are deliberately too short to vote) while still
# catching the short titles, e.g. `oam_bug`.
MIN_ANCHOR_WORD = 3


class ScreenTextError(Exception):
    """The frame could not be read as text — with the reason why."""


class Reading:
    """A decoded screen, plus what it took to decode it."""

    __slots__ = ("text", "origin", "glyphs", "inverted", "anchor_score")

    def __init__(self, text, origin, glyphs, inverted, anchor_score):
        self.text = text
        self.origin = origin              # (dx, dy) sub-cell scroll offset
        self.glyphs = glyphs              # distinct non-blank glyphs matched
        self.inverted = inverted          # how many matched only when inverted
        self.anchor_score = anchor_score  # characters confirmed against the ROM


def _cells(ranks, width, height, background, dx, dy):
    """Cut the frame into 8x8 one-bit masks, from sub-cell origin (dx, dy)."""
    rows = (height - dy) // CELL
    cols = (width - dx) // CELL
    grid = []
    for cy in range(rows):
        row = []
        for cx in range(cols):
            g = bytearray(CELL)
            for y in range(CELL):
                base = (dy + cy * CELL + y) * width + dx + cx * CELL
                b = 0
                for x in range(CELL):
                    if ranks[base + x] != background:
                        b |= 1 << (7 - x)
                g[y] = b
            row.append(bytes(g))
        grid.append(row)
    return grid


def _locate(glyph: bytes, rom: bytes) -> tuple[int, bool] | None:
    """Where in the ROM this glyph's bitmap lives; `True` if inverse video."""
    at = rom.find(glyph)
    if at >= 0:
        return at, False
    at = rom.find(bytes(b ^ 0xFF for b in glyph))
    if at >= 0:
        return at, True
    return None


def _decode_grid(grid, rom):
    """Turn one candidate grid into (text, matched, inverted, anchor_score).

    Raises `ScreenTextError` when this grid is not readable, so the caller can
    try the next sub-cell origin.
    """
    distinct = {g for row in grid for g in row if g != BLANK}
    if not distinct:
        raise ScreenTextError("the frame is blank — the ROM drew no text")

    placed: dict[bytes, tuple[int, bool]] = {}
    for g in distinct:
        found = _locate(g, rom)
        if found is None:
            raise ScreenTextError(
                f"{len(distinct)} distinct glyphs on screen, and at least one "
                "is not present in the ROM — this frame was not drawn with the "
                "ROM's own font"
            )
        placed[g] = found

    # The font is one contiguous block, so every glyph shares an alignment.
    # A glyph that landed elsewhere by coincidence is re-sought inside the
    # majority class rather than allowed to skew the character codes.
    align = collections.Counter(o % CELL for o, _ in placed.values()).most_common(1)[0][0]
    codes: dict[bytes, int] = {}
    for g, (at, inv) in placed.items():
        if at % CELL != align:
            needle = bytes(b ^ 0xFF for b in g) if inv else g
            at = _find_aligned(rom, needle, align)
            if at is None:
                raise ScreenTextError(
                    "a glyph does not sit on the font's own alignment"
                )
            placed[g] = (at, inv)
        codes[g] = at // CELL

    lo, hi = min(codes.values()), max(codes.values())
    # Anything outside printable ASCII cannot be what the ROM meant to draw.
    candidates = range(0x21 - lo, 0x7E - hi + 1)
    scored = []
    for origin in candidates:
        text = "\n".join(
            "".join(" " if g == BLANK else chr(codes[g] + origin) for g in row).rstrip()
            for row in grid
        )
        score = sum(
            len(w)
            for w in text.split()
            if len(w) >= MIN_ANCHOR_WORD and w.encode("latin-1") in rom
        )
        scored.append((score, origin, text))
    scored.sort(key=lambda s: (-s[0], s[1]))
    if not scored or scored[0][0] == 0:
        raise ScreenTextError(
            "no character-table alignment produced text the ROM itself "
            "contains — the screen is not this ROM's own printed report"
        )
    if len(scored) > 1 and scored[0][0] == scored[1][0]:
        raise ScreenTextError(
            "two character-table alignments are equally plausible; refusing to "
            "guess which one the ROM meant"
        )
    score, _origin, text = scored[0]
    inverted = sum(1 for _, inv in placed.values() if inv)
    return text.rstrip("\n"), len(distinct), inverted, score


def _find_aligned(rom: bytes, needle: bytes, align: int) -> int | None:
    at = rom.find(needle)
    while at >= 0:
        if at % CELL == align:
            return at
        at = rom.find(needle, at + 1)
    return None


def read(img, rom: bytes, quantise) -> Reading:
    """Read the text a Blargg-style ROM printed on `img`.

    `quantise` is `image.quantise_shades` — passed in so this module stays
    independent of how a frame was decoded. Raises `ScreenTextError` with a
    reason a human can act on.
    """
    ranks = quantise(img)
    counts = collections.Counter(ranks)
    background = counts.most_common(1)[0][0]

    # A frame of one shade is the most likely failure mode by far (an adapter
    # that drew nothing, or a run that never enabled the LCD). Say so at once
    # rather than grinding through sixty-four sub-cell origins to find out.
    if len(counts) < 2:
        raise ScreenTextError(
            "the frame is a single flat shade — the ROM drew nothing, so there "
            "is no report to read (too few frames, or the LCD never came on)"
        )

    best = None
    first_error = None
    # Aligned is overwhelmingly the common case, so try it first and stop as
    # soon as it reads: the sub-cell search exists for scrolled consoles.
    for dy in range(CELL):
        for dx in range(CELL):
            grid = _cells(ranks, img.width, img.height, background, dx, dy)
            try:
                text, matched, inverted, score = _decode_grid(grid, rom)
            except ScreenTextError as e:
                first_error = first_error or e
                continue
            reading = Reading(text, (dx, dy), matched, inverted, score)
            if best is None or score > best.anchor_score:
                best = reading
            if (dx, dy) == (0, 0):
                return reading
    if best is None:
        raise first_error or ScreenTextError("the frame could not be read as text")
    return best


def verdict(text: str) -> tuple[str, str]:
    """The ROM's own judgement, and a sentence saying what it was.

    Blargg's ROMs print `Passed` or `Failed` (with `#N` naming the sub-test).
    Anything else means the ROM had not finished.
    """
    if "Passed" in text:
        return "pass", "the ROM printed Passed on screen"
    if "Failed" in text:
        line = next((ln.strip() for ln in text.splitlines() if "Failed" in ln), "Failed")
        return "fail", f"the ROM printed {line!r} on screen"
    return "unfinished", (
        "the ROM printed neither Passed nor Failed before the frame budget ran "
        "out; it is probably still running or wedged"
    )
