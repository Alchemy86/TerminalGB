# Terminal render performance — why a big window cost frames, and what fixed it

> Source: [`src/screen/tui.rs`](../src/screen/tui.rs), [`src/termimage.rs`](../src/termimage.rs),
> [`src/screen/frame.rs`](../src/screen/frame.rs) · Related:
> [How the picture reaches you](picture.md) · [Display scaling](display.md) ·
> [Telemetry](telemetry.md) · [Frontends](components/frontends.md) · [🗺️ Code map](codemap.md)

> **Sequel:** this page fixed the *size* of what we transmit. The transport itself — shared
> memory, indexed PNG, and why WezTerm rendered nothing over the Kitty protocol — is
> [How a frame gets into a terminal](rendering.md).

> The captain, playing on a big window: *"The frame rate on larger screen is diabolical. We
> should not be losing running speed and lag just because our 8-bit game screen is larger.
> We are lightyears ahead with tech and power. It's just a fixed size of square — let's scale
> this fucker better."*

Observed: **Scale 14.4×, Upscale nearest ×4, FPS 18.** He is right, and the cause is not the
emulator.

---

## 1. Where the time actually went

Measured with [`examples/render_bench.rs`](../examples/render_bench.rs), which runs the real
pipeline stages headlessly over a matrix of window sizes. 120 frames per case, forced motion
(every frame differs), 8×16 px cells, `tests/cpu_instrs/cpu_instrs.gb`, release build:

| window (cells) | display px | path | emulate | palette | upscale | **encode** | KB/frame | fps |
|---|---|---|---|---:|---:|---:|---:|---:|
| 80×24 | 392×352 | legacy | 0.29 | 0.07 | 0.00 | **2.25** | 726 | 383 |
| 120×40 | 672×608 | legacy | 0.28 | 0.07 | 0.00 | **6.90** | 2 150 | 138 |
| 213×55 | 944×848 | legacy | 0.28 | 0.07 | 0.00 | **13.48** | 4 211 | 72 |
| 320×90 | 1568×1408 | legacy | 0.28 | 0.07 | 0.00 | **39.87** | 11 612 | **24.8** |
| 320×90 | 1568×1408 | legacy, iTerm2 | 0.29 | 0.07 | 0.00 | **84.83** | 65 | **11.7** |

**The emulator is nowhere in this.** One emulated frame costs 0.28 ms against a 16.743 ms
budget — the core runs at roughly 60× real time. The whole cost is `encode`, and it grows
quadratically with the window because it was *the display resolution being re-encoded every
frame*.

Concretely, the old path called `ratatui-image`'s `Resize::Scale`, which:

1. resizes the 160×144 source up to the display rectangle's pixel size (1568×1408 = 2.2 M px),
2. allocates a background image that size and overlays onto it,
3. hashes it, then
4. **Kitty**: base64s the raw RGBA — 11.6 MB of escape sequence, *per frame*; at 60 Hz that is
   693 MB/s down a pty, which is simply not possible; or
   **iTerm2**: PNG-**deflates** those 2.2 M pixels — 85 ms of CPU, i.e. 11.7 fps before
   anything else happens.

18 fps at the captain's window size falls straight out of that. It also explains why it got
worse as the window grew, and why the *game* felt laggy rather than merely choppy: the old
loop built the image inside `step_frame`, so emulation and encoding were the same piece of
work, and the audio clock is slaved to emulation.

---

## 2. The fix: transmit at the picture's own resolution, not the window's

Both terminal graphics protocols can place an image over a rectangle measured in **character
cells** and scale it themselves:

* **Kitty** — a virtual placement (`U=1`) with `c=<cols>,r=<rows>`, positioned by Unicode
  placeholder cells.
  <https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders>
* **iTerm2** — `width=`/`height=` default to cells (`Npx` would mean pixels).
  <https://iterm2.com/documentation-images.html>

So [`src/termimage.rs`](../src/termimage.rs) emits those sequences itself and transmits the
frame at **the size the emulator computed**, letting the terminal do the final stretch. Same
matrix, same machine, `capped` path:

| window (cells) | display px | sent px | **encode** | KB/frame | fps |
|---|---|---|---:|---:|---:|
| 80×24 | 392×352 | 160×144 | **0.10** | 95 | 2 252 |
| 120×40 | 672×608 | 160×144 | **0.10** | 104 | 2 247 |
| 213×55 | 944×848 | 160×144 | **0.10** | 116 | 2 217 |
| 320×90 | 1568×1408 | 160×144 | **0.10** | 160 | 2 310 |
| 320×90 | 1568×1408 | 320×288 (`mmpx ×2`) | **0.40** | 431 | 1 005 |
| 320×90 | 1568×1408 | 640×576 (`lcd ×4`) | **1.61** | 1 514 | 285 |
| 320×90, iTerm2 | 1568×1408 | 160×144 | **0.21** | 91 | 1 818 |

**Flat.** Cost stops being a function of window size, which was the requirement. At fullscreen
with `nearest` the encode went 39.69 ms → 0.10 ms (**400×**) and the wire went 11.6 MB → 160 KB
(**74×**); on iTerm2, 86.37 ms → 0.21 ms (**410×**) and 11.6 MB → 91 KB.

The residual growth in the `KB/frame` column (95 → 160 KB as the window grows sixteenfold) is
Kitty's own placeholder grid: one 4-byte character per covered cell, which is inherent to the
protocol and linear in *cells*, not pixels. iTerm2 needs no grid at all, hence its flat 91 KB.
A still frame costs **zero** on both.

### How much to send — `transmit_scale`

Defaults to `auto`, which means:

* **`nearest` → ×1 (160×144).** Integer pixel duplication is precisely what the terminal does
  when it stretches the image, so doing it first buys nothing and costs 16× the bytes at ×4.
* **A real filter (`scale2x` / `mmpx` / `lcd`) → its own factor.** There the upscale *is* the
  visual effect and must happen before transmission — but only ever at the algorithm's own
  factor, never at the display scale.

If a particular terminal's own scaler is too soft for taste, `transmit_scale = 2` (or `3`/`4`)
sends more pixels; the status panel's `Sent` row and the telemetry `tx_bytes` field show
exactly what that costs. `tx_mb_per_s` in the run summary is the number to watch — it is what
the terminal's parser must swallow, and no amount of encoder speed removes it.

### PNG without compression

The iTerm2 payload is a hand-rolled PNG using zlib **stored** (uncompressed) blocks. It is
~20× larger than a deflated one and costs *nothing* to produce — the right trade at 60 Hz,
where compressing a frame costs far more time than the extra bytes cost the pipe (85 ms of
deflate to save 1.4 MB is a terrible deal when 92 KB/frame is only 5.3 MB/s).

### Why the image is written outside `ratatui`'s cell buffer

`ratatui-image` puts its escape sequence into a cell's *symbol*. At this payload size that is
actively broken, in two separate ways found while validating this change:

1. **`Buffer::diff` invalidates the rest of the screen.** It computes
   `invalidated = max(symbol.width(), invalidated)` and re-emits every subsequent cell while
   that counter is non-zero. A 92 KB base64 payload has a display width of ~92 000, far more
   than a terminal has cells — so *the whole UI is retransmitted every frame*, and an
   unchanged picture still cost 84 KB. Measured, not inferred.
2. **Anything drawn over that cell stops the picture.** The symbol *is* the image; a widget
   painting there replaces it, and no new image data reaches the terminal until it moves.

So [`TermImage::place`](../src/termimage.rs) returns one absolutely positioned sequence, the
cells the picture covers are marked `skip` so `ratatui` never touches them, and the sequence is
written straight to the terminal after the frame is flushed.

### The stats overlay froze the picture

> The captain: *"when the stats overlay is on, the game does not refresh. I hear things but the
> screen does not change until I close the overlay."*

Three independent causes, all removed:

* **The overlay was drawn on top of the picture** (`Rect::new(area.x, area.y, …)` inside the
  game panel). A terminal graphics image owns whole cells: text drawn into them is not *over*
  the image, it is *instead of* it — and for Kitty's Unicode-placeholder placement it deletes
  the placeholders that make the picture update at all. The stats panel and the toast now get
  rectangles of their own (the sidebar and the footer; a left column and a reserved row in
  fullscreen), asserted non-overlapping across eight window sizes in both layouts by
  `the_stats_panel_never_overlaps_the_picture_at_any_size` — which fails on the old layout.
* **The image lived in a cell symbol**, per the section above, so the overlay could erase it.
* **Every frame allocated a brand-new Kitty image id.** `Picker::new_resize_protocol` picks
  `rand::random()` for the id, and the old `step_frame` built a fresh protocol *per emulated
  frame* — 60 new images per second, 8.8 MB each at fullscreen, none ever deleted. Terminals
  cap image storage (Kitty's default is 320 MB), so the quota was exhausted in well under a
  second, after which new images are refused and the picture stops updating. One stable id is
  now reused for the whole session.

**Verification, stated plainly: I could not watch this myself.** This environment has no
Kitty-capable terminal, so the fix is verified at the wire level instead —
`the_picture_keeps_updating_with_the_stats_overlay_on` asserts that with the overlay up every
changed picture still produces a transmission, for both protocols. The overlap test above
reproduces the geometric cause and fails on the old layout. A human needs to confirm by eye.

### Unchanged frames cost nothing at all

[`TermImage::place`](../src/termimage.rs) hashes the pixels and, when the frame and the
rectangle are unchanged, returns the identical symbols. `ratatui` diffs its cell buffer, so
identical symbols produce **no terminal output whatsoever** — a menu, a dialogue box or a
paused game costs zero encode and zero bytes. The `frames_identical` counter reports how often
this fires.

---

## 3. Emulation is no longer coupled to rendering

This is the part that made the *game* lag rather than just the picture stutter.

`step_frame` now does **emulation only**. Palette mapping, upscaling and encoding happen in
`render_display`, at most once per *displayed* frame. The loop paces the two independently:

* **Emulation** runs on a deadline accumulator (`frames_due`), advanced by exactly one frame
  period per frame run — never reset to `now`, which would silently add every loop iteration's
  overhead to the period. Catch-up is capped at `MAX_CATCHUP` (12 frames ≈ 200 ms); past that
  the deadline resyncs and the backlog is *dropped* rather than replayed, so a stalled or
  suspended machine never fast-forwards the game.
* **Rendering** runs on its own deadline (`next_draw_at`): never more often than one panel
  refresh, and otherwise as often as the draw itself can sustain. A draw costing 50 ms yields a
  20 fps display while emulation carries on at 59.7275 fps in the gaps.

So **display frames are dropped; emulated frames are not.** That keeps the game at true speed
and keeps the audio queue — which is fed from the emulation clock — from starving. Both rules
are unit-tested over a virtual clock (`a_slow_display_drops_pictures_not_emulated_frames`),
and the frontend reports both rates separately: the status panel shows `FPS 58 · emu 60`, and
telemetry carries `fps` and `emulated_fps` with `speed_pct` derived from the latter.

---

## 4. What is *not* fixed

* **Sixel and halfblocks keep the old path.** Neither has a cell-addressed placement — sixel
  carries its own pixel grid, halfblocks *is* text — so their cost genuinely does scale with
  the window. They fall back to `ratatui-image`, now at least with unchanged-frame skipping.
* **`render_path = legacy`** restores the old behaviour wholesale, as an escape hatch for a
  terminal that mis-renders cell-scaled placements. It is 20–400× more expensive on a large
  window; it exists so a rendering problem is a config change rather than a rebuild.
* **Kitty placements are capped at 297 cells** per axis — the size of the protocol's
  row/column diacritic table. The *area* is clamped before fitting, so the picture stays
  centred and aspect-correct on an enormous display.
* **Encoding is still on the main loop.** It no longer needs to move off it (0.1–1.6 ms
  against a 16.7 ms budget), but a one-frame off-thread queue remains the obvious next step if
  a filter at a high factor ever justifies it.
* **Kitty's placeholder grid is re-sent with every changed frame** (4 bytes per covered cell,
  ~68 KB at fullscreen). It could be sent only on a resize — the cells are `skip`, so nothing
  overwrites them — but that relies on a terminal re-rendering existing placeholders when the
  image behind them is replaced, which is not something this environment can verify. The safe
  arrangement is the one shipped; the saving is available to anyone who can test it.
* **None of this was watched by a human.** Everything above is measured — bytes, milliseconds,
  emitted escape sequences asserted in tests against a `TestBackend`. Whether the picture
  *looks* right at fullscreen, and whether a given terminal's scaler is acceptably sharp, needs
  eyes.

---

## 5. Conformance record

Following the [standing convention](../AGENTS.md) that every fix is captured as a
conformance-tool case at the moment of the fix. These are **presentation** bugs rather than
emulation ones, so the "hardware behaviour violated" slot cites the protocol or library
contract instead of Pan Docs — and the "which test catches it" answer is the same for all
three, which is the finding worth recording:

> **No test ROM catches any of these.** All three leave the framebuffer byte-for-byte correct
> while the player watches a frozen or crawling picture. A conformance tool built purely on
> ROM-driven framebuffer comparison is blind to the entire class. The
> [telemetry stream](telemetry.md) is not — `speed_pct`, `fps`, `tx_bytes` and `frames_dropped`
> expose all three — which is the concrete argument for it being the reference adapter in
> [conformance.md](conformance.md).

| # | Symptom a user sees | Contract violated | Test produced |
|---|---|---|---|
| 1 | Game runs slow and stutters, worse the larger the window (~18 fps at fullscreen) | Cell-sized placement exists so the client need not pre-scale — [kitty unicode placeholders](https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders), [iTerm2 `width`/`height` in cells](https://iterm2.com/documentation-images.html). `Resize::Scale` targets the render area, right for a photo, wrong at 60 Hz | `transmitted_bytes_do_not_grow_with_the_window`, `image_payload_is_independent_of_the_display_rectangle` |
| 2 | Whole UI redraws constantly; a *still* screen still costs 84 KB/frame | `ratatui` `Buffer::diff` treats a cell symbol as a glyph: `invalidated = max(symbol.width(), invalidated)` re-emits the rest of the screen, and a 92 KB payload reports width ~92 000 | `an_unchanged_frame_transmits_nothing` |
| 3 | Picture freezes after ~a second while sound and input keep working | Kitty images persist until deleted and the terminal enforces a storage quota (320 MB default), so ids must be reused or deleted; `Picker::new_resize_protocol` uses `rand::random()` | `the_picture_keeps_updating_with_the_stats_overlay_on` |

Carried forward as conformance checks: **cost per displayed frame must not be a function of
output size**; **an unchanged frame must produce zero output**; **a long run must not grow
terminal-side resource use without bound**.

## 6. Reproducing the numbers

```console
# the whole matrix, human table
cargo run --release --example render_bench -- --frames 120 --motion \
    --rom tests/cpu_instrs/cpu_instrs.gb

# one case, one JSON line (CI-friendly)
cargo run --release --example render_bench -- --frames 200 --motion \
    --cols 320 --rows 90 --upscale nearest --factor 1 --path capped --json

# a live session's own numbers
GB_TELEMETRY=/tmp/gb.ndjson cargo run --release --example terminal -- game.gb
```

In-game, `t` toggles the stats overlay (fps displayed/emulated, per-stage milliseconds, bytes
per frame, dropped and identical frame counts, audio queue depth and underruns). See
[telemetry.md](telemetry.md) for the machine-readable schema.
