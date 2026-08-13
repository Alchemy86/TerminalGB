# Getting a frame into a terminal

> Source: [`src/termimage.rs`](../src/termimage.rs), [`src/shmframe.rs`](../src/shmframe.rs),
> [`src/deflate.rs`](../src/deflate.rs), [`src/screen/tui.rs`](../src/screen/tui.rs) ·
> Related: [Terminal render performance](terminal-render.md) · [Display scaling](display.md) ·
> [Telemetry](telemetry.md) · [Frontends](components/frontends.md) · [🗺️ Code map](codemap.md)

> The captain, playing in WezTerm with the stats overlay up:
>
> ```
> Scale     8.4x
> Sent      160x144  91K      <-- 91 KB per frame
> Upscale   nearest x1        <-- no upscaling at all
> FPS       6 · emu 60        <-- emulation fine, DISPLAY starved
> ```

The emulator was holding a perfect 59.73 frames per second. The picture was moving at six.
This page explains why, what replaced it, and what each option costs — measured, not
estimated.

[`terminal-render.md`](terminal-render.md) covers the *previous* round of this fight (sizing
the transmitted image by the window). That fix was correct and is still in place. This page
is about what was left: the **transport**.

---

## 1. Why the naive approach cannot work

A Game Boy frame is 160 × 144 pixels. In the RGBA framebuffer the emulator already has, that
is:

```
160 × 144 × 4 bytes  =  92,160 bytes
```

A terminal cannot be handed raw bytes. Everything it receives is an **escape sequence** on
the same stream as the text, so binary has to be encoded — base64, which is the encoding both
graphics protocols use, costs four bytes for every three:

```
92,160 bytes  →  ~123,000 bytes of base64
```

And that is *per frame*. At the Game Boy's 59.7275 Hz:

```
123 KB × 59.7275  =  5.5 MB per second
```

Five and a half megabytes a second of escape sequence, which the terminal must read from a
pipe, scan byte by byte through an escape-sequence parser, base64-decode, and upload to the
GPU as a texture — sixty times a second, forever, for a picture containing **four colours**.

WezTerm managed about six frames a second of that. Nothing was wrong with the emulator, and
nothing was wrong with our encoder: `render_bench` measured our whole render path at 2,395
fps of headroom. The bottleneck was entirely on the far side of the pipe, which is why it
stayed invisible for so long — see [§7](#7-how-this-hid-for-so-long).

**The lesson in one line:** the cost that matters is not what it costs *us* to produce a
frame, it is what it costs the *terminal* to consume one.

---

## 2. The transport ladder

There are four ways to get a frame to a terminal, and they differ by more than three orders
of magnitude. All numbers below are measured with
`examples/render_bench.rs` at a 213 × 55-cell window (a 122 × 55-cell picture,
6.1× scale), with every frame artificially changed (`--motion`) so nothing is skipped — the
worst case.

| # | Transport | Bytes/frame | MB/s at 60 Hz | Encode | Needs |
|---|-----------|------------:|--------------:|-------:|-------|
| 1 | **Kitty shared memory** (`t=s`) + direct placement | **81** | 0.005 | 0.057 ms | Kitty protocol with `t=s`; terminal on the same machine; a writable `/dev/shm` |
| 2 | **Kitty indexed PNG** (`f=100`) + direct placement | **1,448** | 0.082 | 0.183 ms | Kitty protocol |
| 3 | **iTerm2 indexed PNG** inline image | **2,022** | 0.115 | 0.184 ms | iTerm2 inline-image protocol |
| — | Kitty indexed PNG + *placeholder* grid | 27,725 | 1.579 | 0.184 ms | (see [§5](#5-the-wezterm-placeholder-trap)) |
| — | *Before:* iTerm2 uncompressed truecolour PNG | 93,093 | 5.303 | 0.518 ms | — |
| — | *Before:* Kitty raw RGB + placeholder grid | 118,788 | 6.766 | 0.103 ms | — |

Read the top and bottom rows together: **118,788 bytes per frame became 81**, a **1,466×**
reduction, and the encode got *cheaper* rather than more expensive (0.103 ms → 0.057 ms),
because a memcpy into shared memory is less work than base64-ing 92 KB.

Against a 16.7 ms frame budget, every row in that table is encode-noise. **Compression here
is free, and it always was.** The old code chose zlib *stored* blocks — no compression at all
— reasoning that "compression is not free at 60 Hz". That reasoning measured the wrong side
of the pipe.

### Rung 1 — shared memory: don't send the picture at all

The Kitty graphics protocol has a transmission medium that sidesteps the wire completely.
With `t=s` the emulator writes the raw pixels into a POSIX shared-memory object and sends the
terminal only its **name**:

```
ESC _ G q=2,i=6775297,a=T,f=32,t=s,s=160,v=144,c=122,r=55 ; L2diLTM3MTI3MjUtMg== ESC \
└──────────────────────────── about eighty bytes, whatever the picture is ─────────────────┘
```

The terminal `shm_open`s that name, `mmap`s it, and uploads the texture straight out of the
mapping. The two processes are looking at the same physical pages. There is no base64, no
decode, and no parser work proportional to the image.

`f=32` is RGBA — exactly the layout of our framebuffer — so there is not even a colour
conversion: the frame is one `memcpy` into the mapping.

Two rules shape [`src/shmframe.rs`](../src/shmframe.rs):

- **The terminal deletes the object after reading it.** ("The terminal emulator will delete
  the shared memory object after reading the data from it" —
  [kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/#transferring-pixel-data).)
  So an object cannot be reused across frames; each frame creates a fresh one. Three syscalls
  and a memcpy, measured at a few tens of microseconds.
- **A `Drop` impl is not a cleanup guarantee.** `panic = 'abort'`, `SIGKILL`, a crash or a
  closed window all skip destructors, and each skipped destructor leaves 92 KB in `/dev/shm`
  forever. (This repo already learned that lesson the expensive way with battery saves — see
  `CLAUDE.md`.) So every process **sweeps up after dead ones** when it first uses shared
  memory: parse the pid out of each `gb-<pid>-<n>` name and unlink it if no such process
  exists. Note that `kill(pid, 0)` failing is *not* proof the process is gone — `EPERM` means
  it is alive and owned by someone else; only `ESRCH` means gone.

**Shared memory is unavailable over SSH**, in a container whose `/dev/shm` the terminal
cannot see, on non-Unix, and on `wasm32`. Every one of those falls back to rung 2 or 3, which
is why the ladder exists.

### Rungs 2 and 3 — indexed PNG: stop sending 24-bit colour

Where shared memory is not available, the picture does have to go over the wire, so the
question becomes how few bytes it can be.

**A DMG frame has four colours.** Carrying it as 24-bit RGB spends three bytes per pixel to
express two bits of information. Palette-indexed PNG (colour type 3) says it properly:

| Representation | Bytes before compression |
|---|---:|
| RGBA, as the framebuffer holds it | 92,160 |
| RGB, alpha dropped | 69,120 |
| 8-bit palette indices | 23,040 |
| **2-bit palette indices** (4 colours) | **5,904** including PNG filter bytes |

That is a 15× reduction before a compressor has seen a single byte, and it is pure structure
— no information is lost, because there was never more than two bits per pixel to begin with.

[`PngEncoder`](../src/termimage.rs) picks the representation from the frame itself: it scans
for distinct colours and uses 1, 2, 4 or 8 bits per pixel for up to 256 of them, falling back
to truecolour beyond that. Game Boy content never needs the fallback — `nearest`, `scale2x`
and `mmpx` preserve the source's four shades exactly, and the `lcd` grid upscaler tops out
around twelve — but a frame is never *assumed* to be four-colour, so nothing breaks if it
isn't.

Then the indexed scanlines are deflated by [`src/deflate.rs`](../src/deflate.rs) — greedy
LZ77 against a hash chain, one fixed-Huffman block. Game Boy frames are made of 8 × 8 tiles
and large flat regions, so LZ77 finds enormous matches; the 5,904 indexed bytes come out at
roughly a kilobyte. Both protocols accept PNG (Kitty as `f=100`, iTerm2 natively), so one
encoder serves both.

The compressor is hand-rolled and dependency-free, in the same spirit as `savestate.rs` and
the test harness's hash and PNG writer. Fixed Huffman rather than dynamic is a deliberate
stopping point: on this content it is within a few percent of a full-strength encoder for a
fraction of the code.

### Rung 4 — half-blocks: not implemented, and here is what it would cost

For a terminal with **no image protocol at all**, the universal fallback is to draw the
picture with text: the upper-half-block glyph `▀` with a 24-bit foreground colour for the top
pixel and a 24-bit background colour for the bottom one, so **one cell carries two vertical
pixels**. This also fixes aspect ratio for free, because terminal cells are about twice as
tall as they are wide.

**This is not implemented.** Scoping it honestly, so the next person starts from arithmetic
rather than a guess:

- A 160 × 144 frame becomes 160 × 72 cells. A worst-case cell is
  `ESC[38;2;r;g;bm ESC[48;2;r;g;bm ▀` — about 42 bytes — giving ~484 KB per frame, which is
  **worse than what we started with**. Naive half-blocks are not a win.
- Every practical saving comes from *not* repeating state. A Game Boy frame has four colours,
  so a whole row of identical cells needs one colour pair and then bare glyphs: run-length
  behaviour brings a typical frame to the low tens of kilobytes.
- Getting to the ~1.5 KB figure requires **frame differencing** (see [§3](#3-frame-differencing)):
  emit only the cells that changed, each preceded by a cursor move. On a typical frame a
  small fraction of cells change, and that — not the glyph encoding — is where the number
  comes from. So half-blocks are really "cell-diffing with colour", and the diff is the
  feature.
- It cannot reuse [`TermImage`](../src/termimage.rs) as it stands, because that hashes the
  *whole* frame and re-sends all of it; half-blocks need a retained back buffer of previous
  cell contents.

It is worth building for terminals with no graphics support. It is not on the critical path
for the captain, whose terminal answers `OK` to both rungs above it.

---

## 3. Frame differencing

A frame that is identical to the one before it should cost nothing, and it does.
[`TermImage::place`](../src/termimage.rs) hashes the pixels it is given and returns
`changed: false` when neither the frame nor the placement moved; the frontend then writes
**zero bytes**. No encode, no transmission.

This was already implemented, but the benchmark reported `frames_identical: 0` and so looked
broken. That was a **reporting bug in the benchmark** — it hard-coded a zero into the
telemetry sample while tracking the real count in a different variable. Fixed, and the truth
is emphatic:

| ROM (300 frames, 213 × 55 window) | Identical frames | Bytes/frame, averaged |
|---|---:|---:|
| dmg-acid2 (a still test pattern) | **299 / 300** | **6** |
| Blargg `cpu_instrs` (scrolling text) | **292 / 300** | **26** |

Six bytes per frame averaged over a static screen. In real play — menus, dialogue boxes,
pause screens, anything not scrolling — differencing does most of the work, and the transport
underneath it only matters while the picture is actually moving.

The `--motion` flag in `render_bench` exists to *defeat* this deliberately (it pokes one
pixel per frame), because a transport must be measured on the frames it actually has to send.
Every number in [§2](#2-the-transport-ladder) is a `--motion` number.

---

## 4. How the transport is chosen at runtime

**Ask the terminal. Never infer from `$TERM`.**

At startup, after raw mode is enabled but before the event loop takes stdin,
[`src/screen/tui.rs`](../src/screen/tui.rs) sends the Kitty protocol's own capability
handshake — `a=q`, "could you have done this transfer?" — naming a real shared-memory object:

```
ESC _ G a=q,i=6775305,f=32,t=s,s=1,v=1 ; <base64 name> ESC \
```

A terminal that can answers `ESC _ G i=6775305;OK ESC \`; one that cannot answers an error
code; one that has never heard of the protocol says nothing. A **cursor-position request is
appended as a fence** (`CSI 6 n`, which every terminal answers, and answers *after* anything
the query itself produced) so that silence is a conclusive "no" rather than us not having
waited long enough.

Anything that is not an explicit `OK` means "no", and the frontend uses indexed PNG. That
default is the safe one: guessing "yes" wrongly shows the user a blank rectangle.

The chosen transport is **visible in the status panel** — `Proto  kitty shm` / `kitty png` /
`iterm2 png` — because a transport picked at runtime that nobody can see is a transport
nobody can debug. The `Sent` and `tx` figures now also print small numbers honestly
(`87B`, `2.0K`) instead of rounding everything under a kilobyte to `0K`.

Overrides, for debugging and for people who know their terminal:

| Variable | Effect |
|---|---|
| `GB_TERM_KITTY=shm` | Force shared memory without asking |
| `GB_TERM_KITTY=noshm` | Never use shared memory |
| `GB_TERM_KITTY=placeholder` | Use the Unicode-placeholder grid instead of direct placement |
| `GB_TERM_IMAGE=rgb` | Send the **pre-compression** payload — the "before" of every measurement here |
| `render_path = legacy` (config) | The `ratatui-image` path, unchanged |

---

## 5. The WezTerm placeholder trap

This one cost real time and should never be rediscovered.

The Kitty graphics protocol has two ways to say *where* an image goes:

- **Direct placement** — move the cursor, then `a=T` with `c=`/`r=` cell counts. The original
  mode.
- **Unicode placeholders** (`U=1`) — transmit a *virtual* placement, then draw a grid of
  `U+10EEEE` placeholder characters carrying row/column diacritics. A later, optional
  extension, so the terminal's own text layout moves the image around.

`ratatui-image` uses placeholders, so this frontend did too. **WezTerm implements the Kitty
graphics protocol but not the placeholders**, so the grid renders as rows of missing-glyph
boxes and no picture at all. The natural conclusion — "WezTerm can't do Kitty graphics" — is
wrong, and it pushed this project onto the iTerm2 protocol as a fallback, which is where the
captain still was when the frame-rate bug was reported.

Asking WezTerm directly settles it in one command:

```
$ cargo run --release --example render_bench -- --probe
terminal: 213x55 cells, cell 8x16px
TERM = xterm-256color
TERM_PROGRAM = WezTerm
TERM_PROGRAM_VERSION = 20240203-110809-5046fc22
DA1        : ESC[?65;4;6;18;22cESC[1;1R
kitty t=d  : OK  [ESC_Gi=91;OKESC\ESC[1;1R]
kitty t=s  : OK  [ESC_Gi=92;OKESC\ESC[1;1R]
placeholders: no protocol handshake exists — must be eyeballed
```

WezTerm supports both Kitty transmission media. It was only ever the *placement* mode that
failed.

**Direct placement is therefore now the default** for Kitty. It is also cheaper: the
placeholder grid costs 4 UTF-8 bytes per covered cell **every single frame**, because
re-transmitting image data under an id deletes all of that id's placements
("when re-transmitting image data for a specific id, the existing image and all its
placements must be deleted"), so the grid cannot be left on screen and reused. At 122 × 55
cells that is 25 KB per frame — after compression, seventeen times the picture itself. That
is the 27,725-byte row in the table.

`GB_TERM_KITTY=placeholder` restores the old behaviour for a terminal that needs it.

---

## 6. Terminal compatibility

Confirmed here by handshake (`render_bench --probe`) or by the protocol's own documentation;
the third column says which.

| Terminal | Kitty `t=d` | Kitty `t=s` | Unicode placeholders | iTerm2 inline | Evidence |
|---|---|---|---|---|---|
| **WezTerm** 20240203 | ✅ | ✅ | ❌ *(missing glyphs — see [§5](#5-the-wezterm-placeholder-trap))* | ✅ | Handshake, on this machine |
| **kitty** | ✅ | ✅ | ✅ | ❌ | Its own protocol documentation |
| **Ghostty** | ✅ | ✅ | ✅ | ❌ | Documentation; not tested here |
| **Konsole** | ✅ | ? | ? | ❌ | Documentation; not tested here |
| **iTerm2** (macOS) | ❌ | ❌ | ❌ | ✅ | Its own documentation |
| **Alacritty**, **GNOME Terminal**, **xterm** | ❌ | ❌ | ❌ | ❌ | No image protocol — needs half-blocks ([§2](#rung-4--half-blocks-not-implemented-and-here-is-what-it-would-cost)) |
| **tmux / screen** | ✗ passthrough | ✗ | ✗ | ✗ | Multiplexers swallow graphics sequences; run outside them |
| Any terminal **over SSH** | ✅ | ❌ | — | ✅ | Shared memory is machine-local by construction |

Rows marked "not tested here" are documentation claims, not measurements. Run
`render_bench --probe` in the terminal in question and the answer is authoritative in one
line — that is what the mode is for.

---

## 7. How this hid for so long

The headless benchmark reported **2,395 fps** while the real thing did **6**. The reason is
one line of the old benchmark: it wrote its bytes to `/dev/null`. `write_ms_avg` was
`0.000` because nothing was on the other end. Everything it measured — emulate, palette,
upscale, encode — was genuinely fast, and none of it was the problem.

`render_bench --tty` closes that hole. It drives the **real controlling terminal** and, after
each frame, asks for a cursor-position report and waits for the reply. A terminal answers
that only once it has consumed everything queued ahead of it, so the round-trip is a genuine
barrier: the measured frame rate includes the terminal's own parse and texture upload.

```bash
# from a graphics-capable terminal, NOT inside tmux
cargo run --release --example render_bench -- --tty --frames 120 --motion --json
GB_TERM_IMAGE=rgb cargo run --release --example render_bench -- --tty --frames 120 --motion --json
```

Measured in a real WezTerm window on this machine, 213 × 55 cells:

| Transport | Bytes/frame | Achieved fps | Sync timeouts |
|---|---:|---:|---|
| Kitty shared memory | **87** | 203 | 0 |
| iTerm2 indexed PNG | 2,022 | 151 | 0 |
| *Before:* iTerm2 uncompressed | 93,113 | — | **terminal saturated from frame 4** |

The last row is the bug, reproduced. With the old payload WezTerm stops answering within a
handful of frames and then stops draining its pty altogether — the run does not merely get
slow, it wedges. (That is also why `--tty` sets the terminal non-blocking and reports
`terminal_saturated` rather than hanging: a blocking `write` of a 91 KB frame to a terminal
that has stopped reading parks the process somewhere even `timeout` cannot reach.)

**Caveat, stated plainly:** these runs are short (tens of frames) because the WezTerm window
was spawned unfocused on a headless-ish session, and an unfocused WezTerm throttles and
eventually stalls regardless of what we send it. The byte counts are exact and the "before"
saturation is unambiguous; a *sustained* frame-rate figure needs a human with a visible
window. The captain has the stats overlay and can read it in one glance — see below.

---

## 8. What the captain should see

In the terminal frontend, press **`t`** for the stats overlay, or read the Status panel:

```
Sent      160x144  87B        <-- was "91K"
Proto     kitty shm           <-- transport chosen by handshake
FPS       60 · emu 60         <-- was "6 · emu 60"
```

- `Sent` should be **tens of bytes** on Kitty with shared memory, or **~2K** on iTerm2.
  If it still reads `91K`, the transport did not engage.
- `Proto` names the transport: `shm`, `png`, or `legacy`.
- `same` in the `t` overlay counts skipped identical frames; on a menu or a pause screen it
  should climb steadily.
- If `Proto` says `iterm2`, try setting `protocol = kitty` in the config — with direct
  placement now the default, WezTerm renders the Kitty protocol correctly, and that unlocks
  the shared-memory rung.

---

## 9. Reproducing every number here

```bash
# The full transport ladder, headless (add --protocol iterm2 for that half)
cargo run --release --example render_bench -- \
    --frames 120 --cols 213 --rows 55 --upscale nearest --factor 1 \
    --path capped --protocol kitty --motion --json

# Frame differencing on real ROM content (drop --motion)
cargo run --release --example render_bench -- \
    --rom third_party/dmg-acid2/dmg-acid2.gb --frames 300 --json

# What does this terminal actually support?
cargo run --release --example render_bench -- --probe

# What frame rate does this terminal actually achieve?
cargo run --release --example render_bench -- --tty --frames 120 --motion --json
```

---

## 10. Conformance record

Per the [standing convention](../CLAUDE.md), each fix gets a symptom, the test that catches
it, the contract violated, and the new test it produced. As with the three render bugs in
[`terminal-render.md`](terminal-render.md), these are **presentation** bugs: the framebuffer
stays perfectly correct throughout, so point 2 has the same uncomfortable answer every time —
**no test ROM catches any of them.** A conformance tool built only on ROM-driven framebuffer
comparison is blind to the entire class.

That is worth stating positively for [`conformance.md`](conformance.md): **how an emulator
transmits its frames is a conformance question of its own.** An emulator that renders a
byte-perfect framebuffer at 6 fps has failed its user. The telemetry stream
([`telemetry.md`](telemetry.md)) exposes it where a frame hash cannot — `tx_bytes`,
`speed_pct`, `fps` and `frames_identical` name all four of the findings below.

1. **Truecolour transmission of a four-colour picture.**
   *Symptom* — the game runs at 6 fps while the emulator core holds 60; the terminal is
   pinned; a still screen costs as much as a moving one.
   *Caught by* — no test ROM. `render_bench --json` measures it; `render_bench --tty`
   measures the consequence.
   *Contract violated* — none, strictly: it is legal and wasteful. The relevant fact is
   hardware, not protocol — the DMG has a 2-bit-per-pixel framebuffer
   ([Pan Docs, *Video Display*](https://gbdev.io/pandocs/Graphics.html)), so 24-bit
   transmission spends 12× the bytes on information that does not exist.
   *New test* — `a_four_colour_frame_costs_a_couple_of_kilobytes_not_ninety`,
   `palette_bit_depth_follows_the_colour_count` (`src/termimage.rs`).
   Conformance candidate: **bytes per displayed frame must be bounded by the source's colour
   depth, not its pixel format.**

2. **Uncompressed payload where compression is free.**
   *Symptom* — as above; additionally the encoder was *slower* uncompressed (0.518 ms) than
   compressed (0.184 ms), because base64 of 92 KB costs more than deflate of 6 KB.
   *Caught by* — no test ROM.
   *Contract violated* — none; a prior deliberate choice ("no compression, hence no
   compression cost") made against an unmeasured assumption.
   *New test* — `four_colour_scanlines_compress_by_more_than_ten_times`,
   `round_trips_the_shapes_the_encoder_actually_sees` (`src/deflate.rs`), plus
   `png_decodes_back_to_the_same_pixels` round-tripping every bit depth through the `image`
   crate's decoder — the same decoder WezTerm uses.
   Conformance candidate: **an optimisation's cost must be measured on the side of the pipe
   that pays it.**

3. **A Unicode-placeholder grid re-sent every frame.**
   *Symptom* — on WezTerm, no picture at all (missing-glyph boxes); everywhere else, 25 KB
   per frame of placeholder text that, after the payload was compressed, dwarfed the picture.
   *Caught by* — no test ROM.
   *Contract violated* —
   [kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/): Unicode
   placeholders are an optional extension, and re-transmitting image data under an id deletes
   all of that id's placements, so the grid is inherently per-frame.
   *New test* — `kitty_direct_placement_needs_no_placeholder_grid` (`src/termimage.rs`).
   Conformance candidate: **do not assume a protocol is implemented in full — handshake for
   the specific feature.**

4. **Shared-memory objects outliving their process.**
   *Symptom* — `/dev/shm` fills with 92 KB objects, one per abnormally-terminated run, until
   the machine runs out of shared memory.
   *Caught by* — no test ROM, and nothing in any public suite: this is filesystem state, the
   same gap already recorded for battery saves.
   *Contract violated* — the protocol makes the *terminal* responsible for unlinking, which
   leaves the client's own crash path uncovered; and `Drop` is not a shutdown hook under
   `panic = 'abort'` or `SIGKILL`.
   *New test* — `a_dead_processs_frames_are_swept_up_but_a_live_one_is_left_alone`,
   `a_frame_is_visible_under_its_name_and_unlinked_on_drop` (`src/shmframe.rs`).
   Conformance candidate: **a long run must not grow host-side resource use without bound** —
   the same candidate the per-frame Kitty image-id bug produced, arrived at from a different
   direction, which is a sign it is a real one.
