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

## 3. Frame differencing — and reading these numbers honestly

A frame that is identical to the one before it costs nothing.
[`TermImage::place`](../src/termimage.rs) hashes the pixels it is given and returns
`changed: false` when neither the frame nor the placement moved; the frontend then writes
**zero bytes**. No encode, no transmission.

This was already implemented, but the benchmark reported `frames_identical: 0` and so looked
broken. That was a **reporting bug in the benchmark** — it hard-coded a zero into the
telemetry sample while tracking the real count in a different variable.

### Two numbers, never one

Differencing and compression are separate wins and **must be reported separately**, or the
figure flatters. Quoting an average over a static screen measures the deduplicator and
almost nothing else:

* **bytes per *sent* frame** — what compression and the transport achieved. Roughly constant
  for a given transport.
* **dedup hit rate** — what fraction of frames cost nothing at all. Entirely a property of
  what is on screen.

The average a user experiences is the first multiplied by (1 − the second). A test pattern
gets a 99 % hit rate and a flattering average; a battle animation gets none.

### Measured on real content

Pokémon Blue, 213 × 55 window, Kitty PNG transport, 600 frames each, driven from a save state
of actual gameplay (`rom_script`'s `state` command; `render_bench --state … --walk`):

| Scene | Bytes/frame (avg) | Bytes per *sent* frame | Dedup hit rate |
|---|---:|---:|---:|
| Overworld, standing still | **2** | 1,254 | **99.8 %** |
| Overworld, walking | **652** | 1,392 | **53.2 %** |
| Oak intro (animation + scrolling text) | **430** | 2,877 | **85.0 %** |
| dmg-acid2 (a still test pattern) | 6 | ~1,300 | 99.7 % |
| `--motion` (every frame forced to change) | **1,448** | 1,448 | **0 %** |

Read across the "per sent frame" column: **it barely moves**. Compression delivers ~1.3–2.9 KB
per transmitted frame whether the motion is real or synthetic — so `--motion` is a fair
worst-case stand-in for a scene the deduplicator cannot help with, and every number in
[§2](#2-the-transport-ladder) is a `--motion` number, i.e. **the honest worst case, not the
flattering average**.

Note what the dedup rate is *not*: a constant. Walking halves it; a full-screen battle
animation would take it to zero. Design for the `--motion` row and treat differencing as a
bonus.

> **The static-image trap.** An earlier version of this page led with a dmg-acid2 screenshot
> showing `Sent 0B` and `FPS 57`. dmg-acid2 is a *still image* — nothing changes between
> frames, so that measured the deduplicator at its best and the transport not at all. Any
> figure quoted from a static ROM is labelled as such here. If you add measurements, do the
> same.

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

### Asking a terminal a question is harder than it looks

Three defects were found here by running the real frontend in a real terminal and looking at
the answer, each of which silently cost the session its best transport:

1. **Do not use stdout/stdin.** `crossterm` owns those — raw mode, its own startup queries, a
   buffered event source — and the reply never reached our read. Measured as a permanently
   empty reply against a WezTerm that answers the identical query on `/dev/tty` in
   single-digit milliseconds. The probe now opens the controlling terminal directly.
2. **Drain before asking.** `crossterm` queries the terminal during startup
   (keyboard-enhancement support). A leftover reply of *its* still sitting in the buffer is
   read as the answer to *our* question, so the probe concludes "no" while the real reply
   arrives a moment later and is discarded. Intermittent — it cost several runs out of six
   before the drain went in.
3. **Knock first.** A terminal launched *for* this process is still starting up when we ask,
   and answers nothing at all. Asking immediately came back empty in most cold-start runs. So
   the probe sends a cursor-position request until the terminal answers one, and only then
   asks the capability question.

With all three, six cold starts out of six negotiate shared memory. Without any one of them
the failure is invisible: the picture is still correct, just carried by a transport a
thousand times more expensive.

**The general lesson:** a capability handshake that fails *open* (falls back) and is not
displayed anywhere will silently be wrong forever. That is why the transport is on the status
panel.

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
| `GB_TERM_FENCE=off` | Never fence the terminal: no `Term` figure, no pacing to it ([§11](#11-how-fast-can-the-terminal-actually-present--and-why-fps-used-to-say-59)) |
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

Measured in a real WezTerm window on this machine, 213 × 55 cells.

**Synthetic worst case** (`--motion`, every frame forced to change — the case the captain's
6 fps report actually was, since his overlay showed 91 KB being *sent* every frame, meaning
nothing was being deduplicated):

| Transport | Bytes/frame | Achieved fps | Sync timeouts |
|---|---:|---:|---|
| Kitty shared memory | **87** | 203 | 0 |
| iTerm2 indexed PNG | 2,022 | 151 | 0 |
| *Before:* iTerm2 uncompressed | 93,113 | — | **terminal saturated from frame 4** |

That last row is the bug reproduced: WezTerm stops answering within a handful of frames and
then stops draining its pty altogether — the run does not get slow, it *wedges*. (Hence
`--tty` setting the terminal non-blocking and reporting `terminal_saturated` rather than
hanging: a blocking `write` of a 91 KB frame to a terminal that has stopped reading parks the
process somewhere even `timeout` cannot reach.)

**Real gameplay** — Pokémon Blue, loaded from a save state of the overworld, walking
(`--state … --walk`), 150 frames:

| Transport | Bytes/frame (avg) | Per sent frame | Dedup | Achieved fps |
|---|---:|---:|---:|---:|
| Kitty shared memory | **39** | **87** | 55.3 % | **184** |
| iTerm2 indexed PNG | 811 | 1,816 | 55.3 % | 160 |
| *Before:* iTerm2 uncompressed | 41,590 | 93,112 | 55.3 % | 155 |
| Kitty shm, standing still | 1 | 150 | 99.3 % | 267 |
| Kitty shm, Oak intro (text + animation) | 26 | 89 | 70.7 % | 201 |

**Be careful with that "before" row — it does not show a collapse.** At 150 frames of walking
the old path still managed 155 fps, because differencing skipped 55 % of the frames and the
burst was short. The old path collapses when the picture changes *every* frame and the run is
*sustained*: that is the synthetic table above, and it is the captain's reported case.
So the honest claim is not "6 fps became 184 fps everywhere" — it is:

* **Bytes on the wire fell ~1,000×** (93,112 → 87 per transmitted frame). That is unambiguous
  and scene-independent.
* **The terminal no longer saturates** under sustained full-motion, which is what produced
  6 fps.
* **In lighter scenes the old path was already coping**, and the gain there is headroom
  (155 → 184 fps) rather than a rescue.

**Caveat, stated plainly:** these runs are short (tens to ~150 frames) because the WezTerm
window is spawned unfocused from an automated session, and an unfocused WezTerm throttles
regardless of what we send it. Byte counts are exact; the saturation is unambiguous; a
*sustained* frame-rate figure in normal use still wants a human watching a visible window.

---

## 8. What the captain should see

In the terminal frontend, press **`t`** for the stats overlay, or read the Status panel:

```
Sent      160x144  28B        <-- was "91K"
Proto     kitty shm           <-- transport chosen by handshake
FPS       58 · emu 60         <-- ours: pictures issued / frames emulated
Term      1.8ms ok            <-- the terminal's: its cost per frame, measured
```

That is a real capture of Pokémon Blue's animated intro, not a still image.

`Sent` is a **one-second average over displayed frames**, not the last frame's size. It used
to be the instantaneous value, which reads `0B` on every deduplicated frame — so on a static
screen it was almost always `0B`, and even in motion it flickered between `0B` and the real
figure. A number that is zero half the time cannot tell anyone whether the transport is
working, which is the one thing this panel is for.

- `Sent` should be **tens of bytes** on Kitty with shared memory, or a few hundred to ~2K on
  iTerm2, depending on how much is moving. If it still reads `91K`, the transport did not
  engage.
- `Proto` names the transport: `shm`, `png`, or `legacy`.
- `Term` is the **terminal's** cost per displayed frame, measured on this machine at this
  window size ([§11](#11-how-fast-can-the-terminal-actually-present--and-why-fps-used-to-say-59)).
  `1.8ms ok` means it is not the bottleneck; `30.3ms ≤33fps` means it is, and `FPS` above will
  have settled to that ceiling because the frontend paces to it. `n/a no CPR` means this
  terminal does not answer a cursor report, so the figure is genuinely unavailable — that is
  the honest answer, not a zero.
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

# Real gameplay instead of a title screen: capture a save state with rom_script's
# `state` command, then load it and hold walking directions.
cargo run --release --example rom_script -- ROM.gb script.txt out/   # script ends: state overworld
cargo run --release --example render_bench -- --rom ROM.gb \
    --state out/overworld.gbss --walk --frames 600 --json
```

### Screenshotting the terminal frontend

Contrary to a long-standing note in this repo, the frontend **can** be captured on this
GNOME/Wayland box — the earlier attempt only tried the X root window, which is empty under
Wayland because Wayland clients do not draw there. Run the terminal on **XWayland** and
capture its window by id:

```bash
env -u WAYLAND_DISPLAY DISPLAY=:0 wezterm-gui --config initial_cols=120 \
    start --always-new-process --class gbshot -- ./target/release/examples/terminal ROM.gb &
sleep 8
WID=$(DISPLAY=:0 xprop -root _NET_CLIENT_LIST | grep -o '0x[0-9a-f]*' | tail -1)
DISPLAY=:0 import -window "$WID" shot.png
```

This is how the shared-memory negotiation bugs above were found: they are invisible in every
headless test and obvious in one screenshot of the status panel.

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

---

## 11. How fast can the terminal actually present? — and why `fps` used to say 59

> *"if I fullscreen, still looks GREAT and reports 59 fps, but it's clearly rendering
> slower."*

He was reading a number that could not go down.

### 11.1 What `fps` counted, and why that was not a measurement

Since emulation was decoupled from rendering, the frontend's `fps` counted **display frames
we issued**: one per `terminal.draw` plus image write. With the shared-memory transport a
frame is ~87 bytes on the wire and ~0.2 ms of our CPU, and an 87-byte write never fills a pty
buffer — so nothing on our side of the pipe can make that number fall, however long the
terminal takes to put the picture on the glass. Every earlier fix in this document made our
half faster; none of them measured the other half.

### 11.2 There is no present count, and pretending otherwise would be worse

No terminal protocol reports "I painted your image":

- The **Kitty graphics protocol** response acknowledges the *command* — it is sent once the
  transmission has been received and decoded. The specification defines no display
  acknowledgement.
- **Write back-pressure** on the pty measures nothing here, for the reason above.
- A **DSR/CPR round trip** (`CSI 6 n`) is a *fence*: the terminal answers only after working
  through everything written before it, so the round trip includes its decode, upload and
  composite. That is strictly stronger than "we wrote bytes" — but it still cannot report a
  frame the terminal **coalesced** and never painted, because from its side that frame was
  processed.

So the honest quantity is not a present count, it is **how long the terminal takes to accept
one frame**, which is a hard ceiling on how many it can present. That is what
[`src/screen/termcost.rs`](../src/screen/termcost.rs) measures, and it is labelled as a
ceiling everywhere it is shown.

### 11.3 The measurement: cost scales with composited **area**, not with bytes

WezTerm on a 5120×2880 display, kitty `t=s`, every frame changed
(`render_bench --tty --motion`). The fixed part of a fence — pty latency plus the terminal's
input loop, measured with **nothing** to draw — is 3.35 ms at 80×24 and 3.39 ms at
fullscreen: flat, as a fixed latency should be. Subtracting it leaves the terminal's frame
work:

| window (cells) | picture (physical px) | wire | fence | terminal | ceiling |
|---|---:|---:|---:|---:|---:|
| 40 × 12 | 516 × 563 (0.29 Mpx) | 86 B | 3.44 ms | 0.04 ms | — |
| 80 × 20 | 840 × 939 (0.79 Mpx) | 86 B | 3.77 ms | 0.37 ms | ~2700 fps |
| 140 × 34 | 1452 × 1596 (2.3 Mpx) | 86 B | 4.45 ms | 1.05 ms | ~950 fps |
| 200 × 48 | 2044 × 2253 (4.6 Mpx) | 87 B | 5.25 ms | 1.85 ms | ~540 fps |
| **268 × 60 (fullscreen)** | **2540 × 2816 (7.2 Mpx)** | **87 B** | **6.37 ms** | **2.97 ms** | **~337 fps** |

**≈ 0.42 ms per megapixel of composited picture.** The wire column does not move at all: this
is the terminal scaling and compositing a picture, not parsing bytes, and it is a function of
the *display rectangle* — exactly the quantity [§1](#1-why-the-naive-approach-cannot-work)
worked so hard to keep off the wire, reappearing on the terminal's side where we cannot
optimise it away.

**Practical ceiling on this machine: the terminal is not the bottleneck at any size that fits
the screen.** Reaching a 16.74 ms budget at 0.42 ms/Mpx would take ~40 megapixels of picture,
about 5.6× this display. Free-running at 59.7 fps into a fullscreen window leaves the
terminal 7.8 ms behind (`--free-run --fence-every 30`), not backlogged. So on *this* box the
captain's fullscreen report does not reproduce — said plainly, because the useful outcome is
not a fix here but an instrument that answers the question **on his** box, and paces to the
answer if it is below 59.7.

### 11.4 A trap that produced a wrong answer first — do not repeat it

The first ladder run reported **30 ms/frame at "fullscreen"** and a ceiling of 29 fps. It was
wrong. A window asked for more cells than the screen holds is **clamped**, and a terminal
launched for the process reports the size it was *asked* for until it has done so. Sizing the
placement from that early value asks the terminal to composite an image mostly off the
screen, and the cost then rises with a rectangle nobody can see. `render_bench --tty` now
reads `TIOCGWINSZ` **after** the terminal has answered its first CPR, never before. A
"640×176" window on this display is really 268×60.

**Read the window size after the terminal has spoken, not before.**

### 11.5 What changed in the frontend

- `src/screen/termcost.rs` measures the terminal's per-frame cost with a fence, on a **layout
  change** and every 3 s thereafter — never per frame, because a fence blocks and blocking
  the loop every frame would starve the audio queue (cushion ~41 ms, `docs/audio.md`) and add
  its own 3.4 ms round trip to every displayed frame. Each sample is the **minimum** of three
  fences; a sample over 500 ms is discarded as an occluded window rather than committed.
- The loop paces drawing to `max(FRAME_TIME, our draw cost, the terminal's cost)`. If the
  terminal can take 20, we issue 20 — and `fps` becomes **true** rather than merely lower.
  Emulated frames are never dropped for this; display frames are, as always.
- The Status panel gains a **`Term`** row and the `t` overlay a **`term`** row: `1.8ms ok`
  when the terminal is not the bottleneck, `30.3ms ≤33fps` when it is, and **`n/a no CPR`**
  when it cannot be asked. Telemetry gains `terminal_ms`, `terminal_ceiling_fps` (both
  `null`, never `0`, when unmeasured) and `frames_issued`.
- The fence uses `crossterm::cursor::position()`, which reads through crossterm's own event
  source, so a keypress arriving during the wait is requeued and delivered afterwards.
  Reading `/dev/tty` directly — correct at *startup*, before the loop owns stdin — would race
  crossterm for the same input and silently eat keys.
- `GB_TERM_FENCE=off` opts out entirely: the frontend then paces at the display rate as
  before, and says `n/a` rather than showing a figure it did not measure.

### 11.6 The transmitted image is still 160×144, and `cell_scale` does not change that

Asked directly, because the HiDPI note in `AGENTS.md` reads as though `cell_scale = auto`
doubles something that reaches the wire. It does not. `cell_scale` multiplies the **font
size** fed to the picker, and the fit divides by the same font, so the cell rectangle is
identical and the payload is 160×144 either way — confirmed at fullscreen by `tx_w`/`tx_h` in
telemetry and pinned by `cell_scale_does_not_change_what_the_terminal_is_asked_to_composite`
and `the_transmitted_image_is_the_source_resolution_at_every_window_size`. All `cell_scale`
moves is the pixel figure the status panel reports (and the legacy `ratatui-image` path,
where the encoded image really is sized in pixels).

Kitty offers no cheaper placement for an unchanged-but-rescaled image either: re-transmitting
data under an id deletes that id's placements, and a placement that is *not* re-transmitted
is already free — an unchanged frame writes zero bytes and costs the terminal nothing
([§3](#3-frame-differencing--and-reading-these-numbers-honestly)). There is nothing between
those two cases to optimise.

### 11.7 Reproducing it

```bash
# The ladder. Run each in its own window; sizes must FIT the screen (see 11.4).
cargo run --release --example render_bench -- --tty --motion --frames 240 --json

# Is the terminal falling behind when we do not pace to it?
cargo run --release --example render_bench -- \
    --tty --motion --frames 300 --free-run --fence-every 30 --json

# What the frontend itself measures, live:
GB_TELEMETRY=/tmp/t.ndjson ./target/release/examples/terminal game.gb
jq '{fps, terminal_ms, terminal_ceiling_fps, frames_issued}' /tmp/t.ndjson
```

### 11.8 Conformance record

*Symptom* — the frontend reports a frame rate the user can see is wrong; the picture is
visibly slower than the counter at large window sizes, and no number anywhere disagrees.
*Caught by* — **no test ROM, and no headless test of any kind.** The framebuffer is perfect;
the loss is entirely on the far side of the pty. This is the same blind spot as the three
bugs in [`terminal-render.md`](terminal-render.md), one level further out: there, a
framebuffer hash could not see a frozen picture; here, even *our own* frame counter cannot.
*Contract violated* — none, and that is the finding. No terminal graphics protocol offers a
display acknowledgement ([§11.2](#112-there-is-no-present-count-and-pretending-otherwise-would-be-worse)),
so a frontend that reports "fps" from its own issue rate is reporting something it has not
measured. The honest reading is a *ceiling*, obtained from a round-trip fence.
*New test* — `a_slow_terminal_drops_pictures_not_emulated_frames`,
`the_status_panel_reports_the_terminal_alongside_our_own_frame_rate` (`src/screen/tui.rs`),
`an_unmeasured_terminal_cost_is_null_not_zero` (`src/telemetry.rs`), and the seven cases in
`src/screen/termcost.rs`.
Conformance candidate: **a reported frame rate must be bounded by something measured beyond
the emulator's own output boundary — and where that measurement is impossible, the report
must say so rather than substitute the number it can produce.**
