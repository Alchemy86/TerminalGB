# Display scaling — enlarging the 160×144 screen

> Source: [`src/screen/`](../src/screen), [`src/gameboy.rs`](../src/gameboy.rs), [`web/`](../web) · Related: [Frontends](components/frontends.md) · [PPU](components/ppu.md)
> [← Docs: components index](components/README.md) · [testing](testing.md) · [debugging](debugging.md)

---

The Game Boy panel is **160×144** — a tiny, fixed grid of pixels resolved to 4 grey
shades (DMG) or 15-bit RGB (CGB). Every frontend has to blow that up to fill a real
window/canvas/terminal, and **how** it does the blow-up is the whole question the captain
asked: *"it's just pixel screens — can we enlarge, smoothly, scale like vector?"*

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
| **Desktop** | opens at **160×144** logical px | the **GPU** (textured quad) | **bilinear (`LINEAR`)** — *blurry* | **Yes** (`resizable(true)`) | **not preserved**, and resize is **broken** (no viewport update) |
| **Web** | canvas **160×144**, shown at **160×144 CSS px** | **not scaled at all** (1:1) | n/a (no upscale happens) | **No** | 1:1, so correct but tiny |
| **Terminal** | source **160×144**, ×1–4 | **nearest** at source, then protocol **fits** to a pane | nearest → protocol's fit | **Yes** (bigger terminal/pane) | **preserved** (letterboxed by `Resize::Fit`) |

### Desktop — GPU-scaled, but smoothed *and* the resize is broken

The window opens at exactly the native resolution — `LogicalSize { width: 160, height: 144 }`
([`gameboy.rs:107`](../src/gameboy.rs)) — and is **resizable**
([`gameboy.rs:114`](../src/gameboy.rs) `.with_resizable(true)`). Each frame is uploaded as a
160×144 RGBA texture (`gl::TexImage2D`, [`desktop.rs:306`](../src/screen/desktop.rs)) and
drawn onto a quad whose vertices span the full **normalised device coordinates** `-1..1`
([`desktop.rs:160`](../src/screen/desktop.rs)) — i.e. the image is stretched to fill whatever
the GL viewport is. So there **is** a real GPU scaling pipeline (glutin 0.28 + raw OpenGL).

Two problems live in that pipeline:

- **It smooths.** Both the min and mag texture filters are `gl::LINEAR`
  ([`desktop.rs:266-267`](../src/screen/desktop.rs)) — bilinear. Enlarging the window makes
  the picture **blurry**, not crisp. For pixel art this is the classic accidental-blur
  source, and it is exactly what a purist does *not* want.
- **Resize doesn't actually work.** There is **no `gl::Viewport` call anywhere** in the
  codebase, and the `Resized` window event is a no-op that just returns `Poll`
  ([`desktop.rs:115-118`](../src/screen/desktop.rs)). OpenGL sets the viewport to the window
  size **once**, when the context is first made current (160×144 here), and never again. The
  quad is in NDC, so it maps to that stale viewport. Net effect when you drag the window
  bigger: the 160×144 image stays pinned to a 160×144 patch (bottom-left on most drivers) and
  the rest of the window shows the clear colour — which is set to **blue** (`ClearColor(0,0,1,1)`,
  [`desktop.rs:303`](../src/screen/desktop.rs)), another cosmetic oddity. *(Behaviour is
  driver-dependent and was reasoned from the code, not observed — this environment has no
  display. The definite, code-level fact is that viewport management is absent.)*
- **No aspect-ratio handling.** Even once the viewport is fixed, the quad fills the whole
  viewport, so a non-10:9 window would **stretch/distort** the image. There is no letterbox
  and no integer-scale snapping.

So the honest summary for desktop: *it can enlarge and it uses the GPU, but the result is
blurry and the enlarge path is currently broken by a missing viewport update.*

### Web — pixel-perfect, but stuck at 160×144 (no enlarging)

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

### Terminal — nearest-neighbour, and the best-behaved on aspect ratio

`get_image` ([`tui.rs:235`](../src/screen/tui.rs)) copies the framebuffer to RGB and, when
the scale is >1, upscales with **`FilterType::Nearest`**
([`tui.rs:257-264`](../src/screen/tui.rs)) — crisp, no blur. The scale is toggled with the
**`o`** key, cycling 1→2→3→4→1 (`MAX_SCALE = 4`, [`tui.rs:43`](../src/screen/tui.rs),
[`tui.rs:126-132`](../src/screen/tui.rs)).

The upscaled image is then handed to `ratatui-image`, which renders it with the terminal's
own graphics protocol (kitty/sixel) using **`Resize::Fit(None)`**
([`tui.rs:349`](../src/screen/tui.rs)) into a pane that is 40% of the window width
([`tui.rs:336-346`](../src/screen/tui.rs)). `Resize::Fit` **preserves aspect ratio and
letterboxes** — so the terminal frontend is actually the one that already gets aspect ratio
right. The `o` scale mainly raises the *source* resolution fed to the protocol (sharper
result); the final on-screen size is bounded by the pane and the terminal's cell grid, which
grows when you enlarge the terminal window.

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

- **Verdict: a genuine, worthwhile option** — best offered as a selectable filter, with
  **scale2x/MMPX** as the cheap sweet-spot and **hqx/xBRZ** as the "maximum smoothing" choices.

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
already does the right thing; and the web path does **no** scaling at all, so its fix is the
biggest visible win for the least code.

### 🥇 First thing to do — Desktop: make it crisp and make resize work

The most likely current complaint ("it's blurry / it won't enlarge properly") is entirely in
the desktop GL path, and the pipeline to fix it already exists. Three small, cheap changes:

1. **Switch the texture filter `LINEAR` → `NEAREST`** ([`desktop.rs:266-267`](../src/screen/desktop.rs)).
   One-line-each change; instantly crisp instead of blurry. This is the single highest-value
   lever.
2. **Add `gl::Viewport` on the `Resized` event** ([`desktop.rs:115`](../src/screen/desktop.rs))
   so enlarging the window actually fills it. Without this, (1) alone still won't visibly
   help — which is why this is a *small set* of changes, not a true one-liner, and is left for
   the implementation task rather than done here.
3. **Compute an integer scale + letterbox** — pick the largest N where `160N×144N` fits the
   framebuffer, size the quad to `160N×144N` centred, and clear the border. Preserves 10:9,
   avoids uneven pixels. (A non-integer "fill" mode can be an option, but integer should be
   the default.)

Cost: all three live in the existing GL pipeline — **essentially free per frame**. This is
what I would do first.

> **Why not implemented in this docs pass:** (1) is arguably a safe one-liner, but on its own
> it changes nothing the user can see until (2) fixes the viewport, and I can't visually
> verify GL output in this display-less environment. Per "verify, don't claim," the whole
> desktop fix is scoped as the first implementation task, not slipped into a docs change.

### 🥈 Web: enlarge it at all — one CSS property + an integer CSS scale

Web currently shows a 160×144 postage stamp with **no** way to grow it, so this is the
biggest quality-of-life jump for the least effort, and needs **no** Rust/WASM change:

1. Add **`image-rendering: pixelated`** to the canvas (and `-webkit-optimize-contrast` for
   older WebKit) so any CSS upscale stays crisp instead of browser-smoothed.
2. Give the canvas a **CSS size that is an integer multiple** of 160×144 (e.g.
   `width: 480px; height: 432px` for 3×), or size it to the largest integer multiple that
   fits its container in JS and letterbox the rest. The backing store stays 160×144 (cheap);
   CSS does the scale on the GPU compositor (free).

This keeps the current `put_image_data` blit exactly as-is — only [`web/index.html`](../web/index.html)
CSS (and optionally a few lines of JS) change.

### 🥉 Terminal: already good — leave it

The terminal already uses **nearest** ([`tui.rs:257`](../src/screen/tui.rs)) and already
**preserves aspect ratio** via `Resize::Fit` ([`tui.rs:349`](../src/screen/tui.rs)). Low
priority. Only nice-to-have: make the `o` scale also grow the display pane (today it mostly
raises source resolution), and expose it in the on-screen help.

### Optional follow-ups (all cheap once the above lands)

- **DMG green palette + LCD-grid shader on desktop** — highest "feels like a Game Boy" payoff
  for little cost, on the GL pipeline you'll already have. Adapt a libretro `dmg`/`lcd-grid`
  shader.
- **A selectable pixel-art filter (scale2x → MMPX → hqx → xBRZ)** — offer as a menu, GPU
  shader on desktop/web-WebGL, CPU path viable on the terminal because the source is tiny.

### Constraints honoured

- **No new dependencies are adopted here** — crates above (`hqx`, `scalenx`, xBRZ/Omniscale
  shaders) are **proposals**, not additions.
- Nearest/integer scaling and CSS `pixelated` are **free**; a GPU shader is cheap **because a
  GL pipeline already exists on desktop**; a *CPU* xBRZ-per-frame would normally be the
  expensive path, but is affordable here only because the source is 160×144.
- **Aspect ratio (10:9) and integer-scale letterboxing must be handled regardless of filter.**

---

## 4. As tests / next steps

Phrased like the component pages' test lists, so this slots into the same spec-first flow
(and ties into the deterministic frame-hashing in [debugging](debugging.md)):

1. **Desktop enlarges crisply.** Resizing the window scales the image to fill it with
   `NEAREST` filtering; no blur. *(Not covered — blurry `LINEAR` today.)*
2. **Desktop resize actually fills the window.** A `gl::Viewport` update on `Resized` makes
   the image track the window. **Expected to FAIL today — no viewport call.** *(Not covered.)*
3. **Integer scale + letterbox.** At any window size the image is the largest `160N×144N` that
   fits, centred, 10:9 preserved, border cleared. *(Not covered.)*
4. **Web canvas can be enlarged and stays crisp.** With `image-rendering: pixelated` and an
   integer CSS size, the canvas scales up without browser smoothing. **Expected to FAIL today
   — 1:1, property absent.** *(Not covered.)*
5. **Terminal remains nearest + aspect-correct.** `o` scales with `Nearest`; `Resize::Fit`
   keeps 10:9. *(Partially true today — regression guard.)*

**First step:** the desktop crispness + viewport + integer-letterbox change in §3 🥇 — the
pipeline already exists, the cost is ~zero, and it fixes the most likely current complaint.
