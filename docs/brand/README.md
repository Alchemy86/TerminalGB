# TerminalGB brand

Three logo options in the same composition family — heavy geometric wordmark, near-black
panel, one motif carried inside a letterform, one accent colour, grey tagline — each with a
square icon-only lockup for avatars and favicons. Everything here is **hand-authored**:
the letterforms are original stroked skeleton paths drawn in
[`generate.py`](generate.py); **no font is embedded, subset or traced**, so there is no
third-party licence in any of these files, and no Nintendo artwork, logotype or trade
dress is used or imitated (the "screen" in option B is a generic green LCD rectangle; the
motifs in A and C are the two shapes a terminal cursor takes).

Regenerate deterministically with `python3 docs/brand/generate.py`; render previews with
`magick <file>.svg out.png`. Every SVG paints its own panel with a faint border, so it
survives GitHub light mode, dark mode and a pure-black page — the `prefers-color-scheme`
trap recorded in the project memory never arises because nothing is theme-conditional.

## Tagline

> **A GAME BOY IN YOUR TERMINAL**

True, and the shortest honest description of the project. Naming the console a product
emulates is nominative use (the standard practice for emulators); if the captain wants
zero trademark inside the mark itself, the drop-in alternative is
`THE HANDHELD CLASSIC, IN YOUR TERMINAL` — one `TAGLINE` constant in `generate.py`.

## The options

| | wordmark | icon | hero preview |
|---|---|---|---|
| A — the block cursor | [`terminalgb-logo-a.svg`](terminalgb-logo-a.svg) | [`terminalgb-icon-a.svg`](terminalgb-icon-a.svg) | [`preview/logo-a.png`](preview/logo-a.png) |
| B — the screen tile | [`terminalgb-logo-b.svg`](terminalgb-logo-b.svg) | [`terminalgb-icon-b.svg`](terminalgb-icon-b.svg) | [`preview/logo-b.png`](preview/logo-b.png) |
| C — dot matrix | [`terminalgb-logo-c.svg`](terminalgb-logo-c.svg) | [`terminalgb-icon-c.svg`](terminalgb-icon-c.svg) | [`preview/logo-c.png`](preview/logo-c.png) |

**A — the block cursor.** Rounded heavy sans; the **I** of TERMINAL sits under a
phosphor-green block cursor drawn the way a terminal actually draws one — inverse video,
the glyph knocked out dark inside the green cell. This is the most literal translation of
the reference's composition (glyph living inside one letterform, a single accent), and the
most terminal-honest: the mark *is* a cursor over text. Its icon is a white G with the
solid cursor waiting after it. Risk: at very small sizes the inverse-video cell can read
as a green "0" before it reads as a cursor.

**B — the screen tile.** The wordmark splits into TERMINAL in white and **GB** set
dark-on-green inside a rounded rectangle: a generic dot-matrix LCD tile using the two end
shades of the DMG's four-shade palette (`#9bbc0f` / `#0f380f`). The GB tile is a complete
mark on its own, so wordmark and avatar share one device — the icon is just the tile. It
says "Game Boy screen" instantly without borrowing anything from Nintendo, and it is the
most legible option at every size.

**C — dot matrix.** The wordmark rendered in an original 5×7 pixel capital set — the
letterforms themselves are the screen-pixels motif — with a DMG-green underscore cursor
as the single accent. Its icon is built on an exact 16-unit grid so a 16px favicon lands
every pixel cell on exactly 2 device pixels: it is the crispest of the three as a favicon.
Risk: pixel type is the least distinctive move in the retro-emulation space.

## Recommendation

**Ship option B.** The GB tile is the strongest mark of the three: it communicates the
subject in one glance, it never asks the reader to infer a hidden letter (option A's one
weakness), it holds up from 16px favicon to README hero, and the wordmark and the avatar
are the same device rather than two designs. Option A is the closest match to the
reference's composition if the captain wants that literal move; option C's icon is the
best pure favicon and could serve as the favicon even alongside B.

## Files

- `terminalgb-logo-{a,b,c}.svg` — wordmark lockups, 1200×380
- `terminalgb-icon-{a,b,c}.svg` — square icon lockups, 128×128
- `preview/logo-{a,b,c}.png` — hero renders
- `preview/icon-{a,b,c}-128.png`, `preview/icon-{a,b,c}-16.png` — icon renders at avatar
  and favicon size (the 16px files are the real favicon test, not a resized illustration)
- `generate.py` — the only source of truth; edit it, never the SVGs by hand
