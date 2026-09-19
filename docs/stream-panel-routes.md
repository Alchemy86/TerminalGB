# Getting the panel into the feed — three routes, and a recommendation

> The maintainer: *"is it also possible to be able to toggle the feed to INCLUDE the panel's info too,
> or just be the game?"* — and then, on seeing the cost: *"we can't just make the window the content
> of the camera?"*

He is right that window capture gets the game **and** the panel with no rendering work at all. This
page evaluates the three routes so the choice is made before any of them is built.

**Recommendation up front: route 1 (OBS window capture → OBS Virtual Camera). Do not build the text
renderer yet.** It costs nothing to build, it is strictly more capable than anything we would write,
and he already runs OBS for Twitch. Reasons, and the cases where it is the wrong answer, below.

> **Re-checked 2026-08-14, and it stands.** Nothing has changed that weakens it: routes 2 and 3
> cost the same as they did, and route 3 in particular is still a day of work to get *less* than
> OBS gives for free. What *had* gone wrong is that the recommendation lived only on this page —
> the product itself never said the feed was game-only, so the question read as an unanswered
> feature request rather than a settled decision. That is fixed rather than the renderer built:
> the `?` overlay says it, [`docs/video-out.md`](video-out.md) says it in its opening lines, and
> the terminal window is now **titled `TerminalGB`** so an OBS source can find it. The recipe is
> below — it did not exist before, which is most of why "just use OBS" was not actually usable
> advice.

---

## The recipe — OBS scene, once

Five minutes, and then it is a saved scene you switch to.

1. **Name the window.** Nothing to do: the terminal frontend sets the window title to
   `TerminalGB`, plus the cartridge (`TerminalGB — POKEMON BLUE`) once a game is loaded. The
   prefix never changes, so a scene survives switching ROMs.
2. **New scene**, call it `Game Boy`.
3. **Add → Window Capture.**
   * *Wayland* (PipeWire): OBS shows the desktop portal's window picker — choose the terminal
     window once; OBS remembers it via a restore token.
   * *X11/XWayland* (`Window Capture (Xcomposite)`): pick the window whose title starts with
     `TerminalGB`, and set **Window Match Priority** to *Match title, otherwise find window of
     same type*.
4. **Crop to what you want in frame.** Hold <kbd>Alt</kbd> and drag the source's edges. Two
   layouts worth knowing:
   * *everything* — leave it uncropped; the game sits left, the Status/Controls and any plugin
     panel sit right, which is already close to a stream layout.
   * *game + one panel* — crop the Controls box away and keep the game plus the trainer/stats
     panel. Press <kbd>f</kbd> in the emulator first if you want the game alone with no chrome.
5. **Make the text survive the encode.** The panel is rasterised by the terminal at whatever size
   its window is, so legibility is a matter of the *terminal's* font size, not OBS's scaling.
   Make the terminal window big and the font large enough to read at your output resolution, then
   scale the whole source down in OBS if you need to. Scaling *up* a small terminal is what makes
   panel text mushy.
6. **Filters → Scaling/Aspect Ratio → Nearest** on the source, if OBS is resizing it at all. The
   default bilinear filter smears the pixel art; this is the same reason the camera path scales
   by whole pixels.
7. **Start Virtual Camera** (bottom right). Meet, Zoom and Discord now list **OBS Virtual
   Camera** as a camera.

Two things to expect, neither of them a fault:

* **There are now two cameras** — ours (`Game Boy`, the game picture only) and OBS's (whatever
  the scene shows). Pick the right one in the meeting; ours is the one named after the cartridge.
* **Our camera does not need to be running for this.** OBS is capturing the *window*, not the
  feed. Leave `v` off entirely if the OBS route is the one you are using.

For Twitch, skip the virtual camera and hit **Start Streaming** — same scene.

---

## What is actually hard here

The game picture is already a pixel buffer, so sending it is easy — that is the whole of
[`docs/video-out.md`](video-out.md). **The panel is not.** It is text that the *terminal* rasterises,
and the terminal is not in the pipeline to the camera. Nothing we own has ever drawn those glyphs.

So "include the panel" means one of: let something that *is* in front of the terminal capture it
(routes 1 and 2), or draw the text ourselves (route 3).

---

## Route 1 — OBS window capture → OBS Virtual Camera

Add the terminal window as a source in OBS, arrange it on the canvas, click **Start Virtual Camera**.

**Build cost: zero.** No code, no font, no layout pass, no new failure mode in the emulator.

**What he gets that we would not otherwise build:** any layout he likes, scenes, transitions,
overlays, alerts, webcam picture-in-picture, chat, the lot. Text is rasterised by the terminal at
whatever size the window is, so legibility after Twitch's re-encode is a matter of dragging the
window bigger — no glyph work, no re-encode surprises. And it composites the *real* UI, so it can
never drift from what he is looking at.

**Operational cost:**

* OBS must be running. For Twitch it already is; for a quick Meet call it is a second app to start.
* The camera Meet sees is **OBS Virtual Camera**, not our `Game Boy` device. He picks a different
  entry in the camera list, and the device is named after OBS rather than the cartridge.
* Two producers now exist. Our `/dev/video10` and OBS's device are separate cameras; he has to know
  which one is which. (Our persistent-camera work means ours is always there and always showing
  either the game or the idle card, which makes that *less* confusing, not more.)
* On Wayland, OBS window capture itself goes through the desktop portal, so there is a one-time
  "share this window" prompt inside OBS. OBS handles that; we do not.

**Where it is the wrong answer:** streaming from a headless box, or from a machine where installing
OBS is not on. Neither is his situation.

## Route 2 — the emulator captures its own terminal window

**On Wayland this is a poor fit, and it is worth being blunt about why.** An application cannot
simply grab a window — not even its own, and a terminal emulator's window is not ours anyway, it
belongs to WezTerm. The supported path is `xdg-desktop-portal`'s ScreenCast interface over D-Bus,
which hands back a **PipeWire** stream after showing the user a picker dialogue.

That means, to put the panel in the feed, the emulator would acquire: a D-Bus client, a PipeWire
client, a portal session to restore across launches, and a permission prompt every time the token
goes stale. For a program whose entire dependency argument is "no new crate for the hash, the PNG,
the inflater or the font", adding two IPC stacks so it can photograph a window that another program
is drawing is the wrong trade. It also inverts the architecture: the emulator would be reading its
own output back out of the compositor.

There is a narrower version that does work — X11/XWayland, `XGetImage` on the window id, which is
exactly the technique `CLAUDE.md` documents for taking screenshots of the frontend during
development. It is fine as a *developer* tool and it is already how this crew screenshots the TUI.
It is not something to ship as a feature, because it silently does nothing on native Wayland, which
is what he runs.

**Build cost: large, and mostly in dependencies and platform-specific failure.**
**Operational cost: a portal prompt, and a route that breaks differently on every desktop.**

## Route 3 — composite the panel ourselves

Draw the panel into the frame with our own bitmap font, on a 16:9 canvas, before it goes to the sink.

**This is now cheaper than it was when it was briefed**, because the idle card already needed and
already has the hard half: `video::card` is a hand-written 5×7 font with glyph blitting, scaling,
centring and clipping, it is MPL-2.0 (this repository's own code — no third-party font licence to
audit), and it is proven legible at 1280×720 after a real camera round trip. What is left is a
**layout pass over the panel's content model** and a 16:9 canvas mode.

**Estimate for the remainder: about a day.** Roughly: a `Panel` content model the TUI and the
compositor both render from (the trainer plugin already produces structured lines, so this is
re-pointing existing data, not screen-scraping) ~3 h; the 16:9 canvas with the game letterboxed
beside a text column ~2 h; caching so the panel is redrawn only when its content changes rather than
at 60 Hz ~1 h; the public-facing review — never paths, keys, internals — plus tests and a real
consumer check at stream resolution ~2 h.

**What he does *not* get:** scenes, transitions, alerts, chat, his webcam, or the freedom to move
things around. Every layout change is a code change.

**Where it is the right answer:** a headless box, or a machine with no OBS, or if he wants the
composited feed to be *the* camera other apps see without a second producer in the picture.

---

## The recommendation, and what it depends on

**Take route 1.** It is free, it is better, and he already has OBS open for Twitch. Saving the
renderer is a better outcome than building it.

Route 3 becomes worth building if one of these turns out to be true, and it is worth asking him
directly:

1. He wants the panel in the feed **on Meet calls too**, without starting OBS for them.
2. He wants **our** camera — the one named after the cartridge — to carry the panel, rather than a
   second device named OBS.
3. He ever wants this from a machine without a desktop.

If none of those hold, route 1 covers it and the renderer should stay unbuilt. The font is not wasted
either way: it is what draws the idle card.

**One thing that would change the estimate**, noted while fixing the camera's aspect ratio: the
canvas question is now settled. The camera is 16:9 (`1280x720`) by default, and at that geometry
the game occupies an exact ×5 at 800×720, leaving **two 240 px columns** either side. A panel
column is therefore no longer "a 16:9 canvas mode to build" — it is a rectangle that already
exists and is currently black. That takes maybe an hour off the estimate. It does not change the
recommendation: the remaining ~7 hours still buys less than OBS gives for nothing.
