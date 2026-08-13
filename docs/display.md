# Display scaling — enlarging the 160×144 screen

> Source: [`src/screen/`](../src/screen), [`src/gameboy.rs`](../src/gameboy.rs), [`web/`](../web) · Related: [Frontends](components/frontends.md) · [PPU](components/ppu.md) · [🗺️ Code map](codemap.md)
> [← Docs: components index](components/README.md) · [testing](testing.md) · [debugging](debugging.md)

---

> ⚠️ **In flux — line hints may lag.** The terminal renderer (`src/screen/tui.rs`) was
> rewritten (in-app ROM picker, centred `fit_metrics` scaling) after this analysis was
> written, so the terminal-specific `tui.rs:NNN` numbers below point at the file rather than
> exact lines and the terminal narrative is provisional. This page is intentionally left out
> of the `make docs-links` rewrite (see the `EXCLUDE` set in `docs/tools/codemap_links.py`)
> until the terminal-scaling story is refreshed; the desktop/web parts remain accurate.

---

The Game Boy panel is **160×144** — a tiny, fixed grid of pixels resolved to 4 grey
shades (DMG) or 15-bit RGB (CGB). Every frontend has to blow that up to fill a real
window/canvas/terminal, and **how** it does the blow-up is the whole question the captain
asked: *"it's just pixel screens — can we enlarge, smoothly, scale like vector?"*

> **Implementation status (2026-08-13).** The 🥇 desktop fix and 🥈 web fix below
> are now **implemented** on the desktop GL path (`src/screen/desktop.rs`,
> `src/gameboy.rs`) and the web frontend (`web/index.html`, `web/index.js`):
> `NEAREST` filtering, a `gl::Viewport` update on resize, integer-scale +
> black letterbox (default, with a documented N=0 clamp-to-1×-and-crop), and a
> `image-rendering: pixelated` + integer-CSS-scale web canvas that fills the
> viewport and re-fits on window resize. The integer-scale maths is unit-tested
> (`integer_scale_viewport`, `src/screen/desktop.rs`), and both paths were
> visually verified headlessly — desktop through a faithful replica of the GL
> pipeline on llvmpipe, web in Chrome via computed-layout + screenshot.
>
> **The pixel-art upscalers (§2 option 3) are now IMPLEMENTED too** — a
> dependency-free `src/upscale/` module with `nearest` / `scale2x` / `mmpx` /
> `lcd`, runtime-switchable and wired into all three frontends (desktop `U`/`I`,
> terminal `]`/`}`, web `]`/`\`). Full design, licensing rationale and measured
> per-frame costs: **[docs/upscaling.md](upscaling.md)**. Only the CRT/full-shader
> pipeline remains a proposal. Per-frontend detail is corrected inline below.
>
> ⚠️ The **terminal** narrative in §1/§3 predates both the `tui.rs` rewrite and
> the upscaler, and `src/screen/tui.rs` is under active development right now — so
> treat terminal specifics below as provisional and prefer the in-app `?` help for
> the current keys.

This page answers three things:

1. **[What each frontend does today](#1-what-happens-today-per-frontend)** — from the code,
   with `path:line`.
2. **[The scaling options and how they differ](#2-the-options-and-how-they-actually-differ)**
   — nearest vs. smooth vs. "like vector" (pixel-art upscalers), plus LCD/CRT shaders.
3. **[What to do, per frontend, honest about cost](#3-recommendation-per-frontend)**.

---

## 1. What happens today, per frontend

| Frontend | Native size | Scaled by | Filter | User can resize? | Aspect ratio |
|---|---|---|---|---|---|
| **Desktop** | opens at **160×144** logical px | the **GPU** (textured quad) | ~~`LINEAR`~~ → **`NEAREST`** — *crisp* ✅ | **Yes** (`resizable(true)`) | **preserved** — integer scale + black letterbox; resize updates the viewport ✅ |
| **Web** | backing store **160×144**, CSS-scaled up | **CSS integer scale** (GPU compositor) | **`pixelated`** — *crisp* ✅ | **Yes** — fills viewport, re-fits on resize ✅ | **preserved** (integer multiple, centred, letterboxed) |
| **Terminal** | source **160×144** (optionally pixel-art upscaled) | **`Resize::Scale`** upscales to fill a centred `fit_metrics` pane | nearest / scale2x / mmpx / lcd, then protocol scale | **Yes** — auto-re-fits on resize | **preserved** (aspect-correct centred rect) |

### Desktop — GPU-scaled, but smoothed *and* the resize is broken

The window opens at exactly the native resolution — `LogicalSize { width: 160, height: 144 }`
([`gameboy.rs:107`](../src/gameboy.rs)) — and is **resizable**
([`gameboy.rs:114`](../src/gameboy.rs) `.with_resizable(true)`). Each frame is uploaded as a
160×144 RGBA texture (`gl::TexImage2D`, [`desktop.rs:306`](../src/screen/desktop.rs)) and
drawn onto a quad whose vertices span the full **normalised device coordinates** `-1..1`
([`desktop.rs:160`](../src/screen/desktop.rs)) — i.e. the image is stretched to fill whatever
the GL viewport is. So there **is** a real GPU scaling pipeline (glutin 0.28 + raw OpenGL).

Two problems used to live in that pipeline — **both are now fixed**; the original
diagnosis is kept here for context, with the fix noted after each:

- **It smoothed.** Both the min and mag texture filters were `gl::LINEAR`
  ([`desktop.rs`](../src/screen/desktop.rs)) — bilinear. Enlarging the window made the
  picture **blurry**, not crisp. → **Fixed:** both filters are now `gl::NEAREST`, so every
  Game Boy pixel stays an exact square. (Verified: a headless llvmpipe render of the pipeline
  shows hard pixel edges and a staircased diagonal instead of a soft ramp.)
- **Resize didn't actually work.** There was **no `gl::Viewport` call anywhere**, and the
  `Resized` window event was a no-op. OpenGL set the viewport once, at context creation
  (160×144), and never again, so dragging the window bigger left the image pinned to a
  160×144 patch with the rest showing the (blue) clear colour. → **Fixed:** the `Resized`
  event now calls `gl_window.resize()` + `Glcx::resize()` (`src/gameboy.rs`), and `draw()`
  re-applies the integer-scaled viewport every frame, so the image tracks the window. The
  clear colour is now **black** for a clean letterbox.
- **No aspect-ratio handling.** Even with a live viewport, the quad fills the whole viewport,
  so a non-10:9 window would **stretch/distort** the image. → **Fixed:** `integer_scale_viewport()`
  picks the largest N where `160N×144N` fits, centres it, and the black clear letterboxes the
  border — preserving 10:9 and keeping pixels even. **N=0** (window smaller than 160×144) is
  clamped to **1× and centre-cropped**, so the image never divides to zero or vanishes.

So the honest summary for desktop: *it enlarges on the GPU, crisply (`NEAREST`), with a
working viewport and integer-scale letterboxing that preserves 10:9.*

### Web — pixel-perfect, and now enlargeable (was stuck at 160×144)

> **Now implemented.** The backing store stays 160×144 (the `put_image_data` blit is
> unchanged — no Rust/WASM change), but the canvas now carries
> `image-rendering: pixelated` (+ `-moz-crisp-edges` / `crisp-edges` /
> `-webkit-optimize-contrast` fallbacks) and `web/index.js`'s `fitCanvas()` sets its CSS
> size to the largest integer multiple of 160×144 that fits the viewport, re-running on
> `resize` and when the wasm renderer appends the canvas (`MutationObserver`). `#game`
> becomes a fixed, flex-centred black backdrop once it holds the canvas (`#game:not(:empty)`),
> giving a centred, aspect-correct, letterboxed, crisp image that grows with the window.
> Verified in Chrome: at 640×480 → 3×, 900×500 → 3× (height-limited), 1280×800 → 5×, each
> centred with 10:9 preserved and `image-rendering` computing to `pixelated`; a live
> window resize re-fit 3×→5× without reload. The original 1:1 analysis follows for context.



The canvas backing store is set to the native size — `canvas.set_width(160)` /
`set_height(144)` ([`web.rs:54-55`](../src/screen/web.rs)) — and the frame is written with
`context.put_image_data(&d, 0.0, 0.0)` ([`web.rs:123`](../src/screen/web.rs)).
`put_image_data` writes **raw device pixels**: it ignores any canvas CSS size and any 2D
context transform, so there is **no scaling step at all** — the image is 1:1.

The CSS then displays the canvas at its intrinsic size. The host `<div id="game">` is fixed
at `width:160px; height:144px` (inline `<style>` in [`web/index.html`](../web/index.html);
the id rule out-specifies the `.glass` class rule), and the canvas inside has no CSS of its
own, so it renders at **160×144 CSS pixels — a ~4 cm postage stamp** on a modern monitor.

- **The user cannot enlarge it.** There is no zoom, no scale control, no resizable element.
- **`image-rendering: pixelated` is *not* set** anywhere (confirmed: no occurrence in `web/`
  or `src/`). Today that is invisible because the canvas is 1:1 — but the moment anyone scales
  the canvas up (via CSS, a zoom control, or a WebGL path), the browser will **bilinear-smooth
  it by default** and blur the pixels. This is the single most common accidental-blur trap on
  the web and it is one property away from biting us.

### Terminal — nearest-neighbour, aspect-correct, and now uncapped

> **Updated after the `tui.rs` rewrite (file under active development — line hints omitted).**
> The old text below described a capped `o`-key pre-scale and `Resize::Fit`; both are gone.

`get_image` in [`tui.rs`](../src/screen/tui.rs) maps the framebuffer through the selected
palette and then the selected **pixel-art upscaler** (`nearest`/`scale2x`/`mmpx`/`lcd`, cycled
with `]`/`}` — see [upscaling.md](upscaling.md)). The result is handed to `ratatui-image` with
**`Resize::Scale`** (not `Fit`): `Fit` refuses to enlarge a source smaller than the area, which
is why a 160×144 image used to render tiny; `Scale` upscales at pixel resolution to fill a
centred, aspect-correct `fit_metrics` rect that **re-fits automatically on resize** — there is no
manual scale key any more. (On scaled-Wayland HiDPI terminals the `cell_scale` config corrects a
logical-vs-physical pixel mismatch — see the top-level README.)

### FFI — no scaling; the embedder owns it

`image()` ([`lib.rs`](../src/lib.rs)) hands back the raw 160×144 RGBA buffer; any scaling is
the host application's job. (`image_base64` even has a commented-out `Nearest` resize.) Out
of scope for this page, but worth stating so the picture is complete.

---

## 2. The options, and how they actually differ

The captain named three things. They are genuinely different techniques with different looks
and different costs. Here they are on a diagonal edge — the case where the difference is
most visible.

```
 source (2×2)      nearest / integer      bilinear (smooth)      pixel-art ("vector")
   ██··              ████ ····              ▓▓▒░ ····              ███▙ ····
   ····      →       ████ ····      →       ▓▒░· ····      →       ██▜· ····
                     (hard square edge)     (soft grey ramp)       (clean anti-aliased curve)
```

### Option 1 — Nearest-neighbour / **integer** scaling  *(crisp, blocky)*

Each source pixel becomes an N×N block of identical pixels. No new colours, no blur — every
Game Boy pixel stays an exact square. This is what most emulator users want as the default.

The catch is in the word **integer**: at a whole-number multiple (2×, 3×, 4×) every source
pixel gets the *same* number of screen pixels, so the grid stays even. At a **non-integer**
scale (say 2.5×) nearest-neighbour has to give some pixels 2 screen-pixels and some 3 —
producing **uneven pixel sizes and shimmer**, which looks visibly wrong. The fix is to scale
by the largest integer that fits the window and **letterbox** the leftover with a border
(black, or a DMG-green bezel). Aspect ratio (10:9) must be preserved either way.

- **Cost: effectively free.** A GPU sampler set to `NEAREST`, or a trivial CPU block copy.
- **Verdict: this should be the default.**

### Option 2 — **Bilinear / smooth**  *(soft, blurry)*

The hardware interpolates each output pixel from its 4 nearest source pixels. Trivial to
enable — it's usually one flag (`gl::LINEAR`, or the browser's default canvas smoothing) —
which is why it so often shows up *by accident*, as it does on desktop today. It looks fine
for photos and terrible for 4-colour pixel art: edges turn into grey ramps. Generally
disliked for retro content.

- **Cost: free** (it's the hardware's default path).
- **Verdict: offer it as an option, never as the default.**

### Option 3 — "Like vector" — **pixel-art upscaling algorithms**  *(the interesting one)*

This is what the captain means by *"scale like vector"*: algorithms that look at each pixel's
neighbourhood, **detect edges**, and synthesise smooth diagonals and curves — so a blocky
sprite comes out looking like it was drawn as vectors, **not** blurred. They are a real middle
ground between blocky and smeary.

| Algorithm | How it looks on a 160×144 4-colour screen | Relative cost | Rust / shader availability |
|---|---|---|---|
| **scale2x / scale3x (AdvMAME / EPX)** | Mild edge-smoothing, stays close to the original; great for a 4-shade DMG image where there's little colour detail. | **Very cheap** (~9 neighbour reads/pixel). Trivial to hand-write or vendor. | Small pure-Rust helpers exist (e.g. `scalenx`); the algorithm is ~40 lines. GLSL versions ship in RetroArch. |
| **MMPX** | Modern successor to scale2x; better with thin lines and 2-px features. Designed exactly for low-res sprites. | **Cheap**, fixed 2×. | Reference is a public GLSL/C shader; no prominent maintained Rust crate — easy to port. |
| **hqx (hq2x/3x/4x)** | Smooth anti-aliased curves; the classic "vector-ish" look. Can look slightly soft. | **Moderate** CPU. | **`hqx`** crate exists (pure-Rust port, 2×/3×/4×). Also a well-known GLSL shader. |
| **xBRZ** | The best-looking of the family — clean curves, preserves sharp corners. | **Heaviest** CPU (large per-pixel kernel + edge analysis). | `xbrz`-style crates exist but are **not actively maintained**; also available as a `.slang`/GLSL shader in RetroArch. |
| **Omniscale** | Very high quality, continuous scale factor. | Moderate–heavy; GPU-oriented. | Distributed as a GLSL shader (not a Rust crate); licensing worth checking before adopting. |

**Cost in context — the 160×144 source is your friend.** The screen is only **23,040
pixels**. Even xBRZ (the most expensive) chews through a source that small in low single-digit
milliseconds on a CPU, and its 4× output (640×576 ≈ 368k px) is still tiny. So — unusually —
**even a CPU-side xBRZ per frame at 60 fps is affordable here**, precisely because the source
is so small. That is *not* true of a full-resolution console; it's a happy accident of the
Game Boy's size. As a **GPU shader** any of these is essentially free once a shader pipeline
exists.

- **Verdict: a genuine, worthwhile option — and now IMPLEMENTED.** `src/upscale/` ships
  **scale2x** (reimplemented from the published rule, not the GPL reference) and **MMPX** (ported
  from the MIT reference), plus an **lcd** dot-matrix grid, as a selectable per-frame filter
  across all frontends. hqx (LGPL tables) and xBRZ (GPLv3) were deliberately **not** vendored —
  a licensing constraint for this MPL-2.0 repo — so the "maximum smoothing" tier is not offered.
  Full rationale and measured costs: **[docs/upscaling.md](upscaling.md)**.

### Also worth naming — **LCD and CRT shaders**  *(different goal: authenticity, not sharpness)*

Many people don't actually want *sharper* — they want it to **look like a Game Boy**. That is
a separate axis from the three above:

- **DMG green tint** — remap the 4 shades to the pea-green DMG LCD palette (the core currently
  hands over already-resolved RGBA; see [PPU](components/ppu.md)).
- **LCD grid / dot-matrix** — a subtle darkening between pixels reproducing the reflective
  panel's sub-pixel gaps. This is the signature DMG look.
- **Ghosting / motion blur** — blend a little of the previous frame, mimicking the slow LCD
  response many games relied on for transparency effects.
- **CRT** (for the Super Game Boy / TV-out fantasy) — scanlines, mask, mild curvature.

These are cosmetic overlays, cheapest as fragment shaders, and compose *on top of* whichever
scaler you pick. They're the highest "feels right" payoff for the least accuracy risk, and
libretro/RetroArch ship a large, battle-tested free shader library (`dmg`, `lcd-grid`,
`crt-*`) that can be adapted rather than written from scratch.

---

## 3. Recommendation, per frontend

Ranked by **value-for-effort**. The decisive fact: **desktop already has a GPU pipeline**
(glutin + OpenGL), so nearest-scaling and shaders are nearly free *there*; the terminal
already does the right thing; and the web path did **no** scaling at all, so its fix was the
biggest visible win for the least code. The 🥇 and 🥈 items below are now **implemented**
(status noted in each heading); 🥉 and the follow-ups remain as before.

### 🥇 Desktop: crisp + working resize + integer letterbox — ✅ DONE

The most likely complaint ("it's blurry / it won't enlarge properly") was entirely in the
desktop GL path. Three changes, all in the existing pipeline, now landed:

1. **Texture filter `LINEAR` → `NEAREST`** ([`desktop.rs`](../src/screen/desktop.rs), the
   `TEXTURE_MIN/MAG_FILTER` params) — instantly crisp instead of blurry. ✅
2. **`gl::Viewport` on the `Resized` event** — `src/gameboy.rs` intercepts `Resized` and calls
   `gl_window.resize()` + `Glcx::resize()`; `draw()` also re-applies the viewport every frame,
   so enlarging the window fills it. ✅
3. **Integer scale + letterbox (default).** `integer_scale_viewport()` picks the largest N
   where `160N×144N` fits the framebuffer, centres it, and the black clear letterboxes the
   border — preserving 10:9 and keeping pixels even. **N=0** clamps to 1× and centre-crops.
   No stretch/fill mode was added (integer is the only mode); a fill mode remains an easy
   opt-in follow-up. ✅

Cost: all three live in the existing GL pipeline — **essentially free per frame**. The scale
maths is unit-tested (`integer_scale_viewport` tests in `desktop.rs`: exact multiples, largest
fitting N, non-integer framebuffers, N=0 clamp, degenerate source) and the pipeline was
rendered headlessly on llvmpipe to confirm crisp `NEAREST` output with a centred black
letterbox.

### 🥈 Web: enlargeable + pixelated — ✅ DONE (no Rust/WASM change)

Web used to show a 160×144 postage stamp with **no** way to grow it. Now, with **no** change
to the `put_image_data` blit:

1. The canvas carries **`image-rendering: pixelated`** (with `-moz-crisp-edges` /
   `crisp-edges` / `-webkit-optimize-contrast` fallbacks) so the CSS upscale stays crisp. ✅
2. **`fitCanvas()`** ([`web/index.js`](../web/index.js)) sets the canvas CSS size to the
   largest integer multiple of 160×144 that fits the viewport, centred (via the flex-centred
   `#game:not(:empty)` black backdrop), re-running on `resize` and when the wasm renderer
   appends the canvas. The backing store stays 160×144 (cheap); CSS scales on the GPU
   compositor (free). ✅

Only [`web/index.html`](../web/index.html) (CSS) and [`web/index.js`](../web/index.js) (JS)
changed. Verified in Chrome across window sizes (3×/5×, centred, 10:9, `pixelated`) plus a
live resize.

### 🥉 Terminal: rebuilt — now fills the pane and upscales

The terminal was the "leave it" frontend, but the `tui.rs` rewrite went further: it swapped
`Resize::Fit` for **`Resize::Scale`** (so a 160×144 source now enlarges to fill a centred,
aspect-correct pane instead of rendering tiny), dropped the capped `o`-key pre-scale for
automatic re-fit on resize, and gained the shared **pixel-art upscaler** (`]`/`}`) and palette
(`[`). The remaining follow-up is the same as everywhere: CRT/LCD shaders (the `lcd` grid filter
already exists). `src/screen/tui.rs` is still under active development.

### Optional follow-ups (all cheap once the above lands)

- **DMG green palette + LCD-grid shader on desktop** — highest "feels like a Game Boy" payoff
  for little cost, on the GL pipeline you'll already have. Adapt a libretro `dmg`/`lcd-grid`
  shader.
- **A selectable pixel-art filter (scale2x → MMPX → hqx → xBRZ)** — offer as a menu, GPU
  shader on desktop/web-WebGL, CPU path viable on the terminal because the source is tiny.

### Constraints honoured

- **No new dependencies were adopted** — the shipped upscalers (`src/upscale/`) are
  dependency-free: scale2x reimplemented from the published rule and MMPX ported from the MIT
  reference. The crates named above (`hqx`, `scalenx`, xBRZ/Omniscale shaders) were **not**
  pulled in (licensing — see [upscaling.md](upscaling.md)).
- Nearest/integer scaling and CSS `pixelated` are **free**; a GPU shader is cheap **because a
  GL pipeline already exists on desktop**; a *CPU* xBRZ-per-frame would normally be the
  expensive path, but is affordable here only because the source is 160×144.
- **Aspect ratio (10:9) and integer-scale letterboxing must be handled regardless of filter.**

---

## 4. As tests / next steps

Phrased like the component pages' test lists, so this slots into the same spec-first flow
(and ties into the deterministic frame-hashing in [debugging](debugging.md)):

1. **Desktop enlarges crisply.** Resizing the window scales the image with `NEAREST`
   filtering; no blur. ✅ **Done** — filters are `NEAREST`; verified by headless llvmpipe render.
2. **Desktop resize actually fills the window.** A `gl::Viewport` update on `Resized` makes
   the image track the window. ✅ **Done** — `Resized` handler + per-frame viewport in `draw()`.
3. **Integer scale + letterbox.** At any window size the image is the largest `160N×144N` that
   fits, centred, 10:9 preserved, border cleared (N=0 → 1× crop). ✅ **Done** — `integer_scale_viewport()`,
   unit-tested including non-integer sizes and the N=0 edge case.
4. **Web canvas can be enlarged and stays crisp.** With `image-rendering: pixelated` and an
   integer CSS size, the canvas scales up without browser smoothing. ✅ **Done** — verified in
   Chrome at several window sizes and across a live resize.
5. **Terminal fills the pane, aspect-correct.** `Resize::Scale` upscales 160×144 to a centred
   `fit_metrics` rect and re-fits on resize; nearest/scale2x/mmpx/lcd selectable via `]`/`}`.
   ✅ **Done** (part of the `tui.rs` rewrite).

**Remaining next steps:** the pixel-art upscalers **have landed** (`src/upscale/`,
[upscaling.md](upscaling.md)); what remains are the **CRT / full-shader** overlays (§2's LCD/CRT
list — the `lcd` grid filter is done) and a non-integer "fill" desktop mode as an easy opt-in.
