# Music mode — cartridges as albums

> **Keys:** `M` gets you here — from the picker **or from a running game**; then `←`/`→` browse, `Enter` play, `w` export a WAV, `c` next cover, `g` play it properly. [Every control →](controls.md)

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`6c20451`](https://github.com/Alchemy86/TerminalGB/tree/6c20451306663018288d34253c199fad507ae6f6) · regenerate with `make docs-links`, verify with `make docs-check`.

A library view where the ROMs you already own appear like albums: cover art
drawn from the game itself, the cartridge's own title, its own music playing,
and one key that saves what you are hearing as a WAV.

```
terminal music          # the run command
M                       # …or from the ROM picker
```

```
MUSIC MODE · 3 albums

  ┌────────────────────┐   POKEMON BLUE
  │                    │   MBC3+RAM+BATTERY · 1024 KiB
  │   the game's own   │
  │   title screen     │   ♪ title theme
  │                    │     what the cartridge plays when it boots
  │                    │   ▶ playing
  └────────────────────┘     0:37  ▮▮▮▮ ch1-4
                             ▦ cover 1/4 · from 0:19 into the boot · c to change
─────────────────────────────────────────────────────────────────────────────
▶ POKEMON BLUE      POKEMON_GLD       POKEMON_SLV
←/→ browse · ⏎ play · w save WAV · c cover · space pause · g play the game · q leave
```

---

## 1. What is easy here, and what is not

**The title is free.** Every cartridge carries it at header `$0134`–`$0143`
([`romlib::read_title`](../src/screen/romlib.rs)).

**Cover art is very doable.** Boot the ROM headless, let it reach its title
screen, keep the frame. It is the game's own artwork, drawn by the game. §3.

**Track selection is the hard part, and there is no general solution.** A Game
Boy cartridge exposes no "play track 5" interface — no standard sound-driver
entry point, no track table, nothing. There are three realistic approaches, in
order of value:

| # | approach | what it needs | status |
|---|----------|---------------|--------|
| 1 | **Play what the game plays.** Boot it, listen. | nothing game-specific | **implemented** |
| 2 | Drive the game's own **sound test**, where it has one. | per-game knowledge: which screen, which buttons, how many tracks | named, not implemented — this is what a **plugin** is for |
| 3 | Call the game's **sound driver** to enumerate every track. | a reverse-engineered driver, per title | named, not attempted |

Music mode implements (1) and says so. That is the whole reason
[`TrackSource`](../src/music/library.rs) exists as a type rather than a comment:
"title theme" and "full soundtrack" are very different promises, and the shelf
is only allowed to make the one it kept.

> `the_shelf_never_claims_a_soundtrack_it_did_not_get` and
> `the_panel_never_promises_more_than_a_title_theme` render the actual strings
> and grep them. The wording is a tested property, not a convention.

## 2. Copyright — the line

Music mode operates on **ROMs the user supplies**, and renders art and audio
from those files **at run time**. That is what every emulator does, and it is
fine.

**Nothing derived from a commercial cartridge is ever written into this
repository** — not artwork, not audio, not screenshots, not test fixtures. The
fixtures in [`music::testrom`](../src/music/testrom.rs) are a few dozen bytes of
our own machine code precisely so the tests, the demo shelf and CI never need a
game. `examples/music_probe.rs` **refuses** a `--covers` or `--wav` destination
inside the working tree.

This is the same rule as
[the licence boundary in `AGENTS.md`](../AGENTS.md#licence-boundary-format-knowledge-is-ours-game-assets-are-not),
applied to pixels and samples.

## 3. Cover art: scenes, not frames

The first version scored each sampled frame on its own — shade variety, ink
coverage, "how busy does this look". Measured against real cartridges it picked
**a fade for Pokémon Blue and the sea behind the intro for Pokémon Gold**. Both
are perfectly good pictures of the wrong thing.

The property that actually distinguishes a title screen is that it **stays
there**: the game has finished introducing itself and is waiting for the player.
So [`cover::Picker`](../src/music/cover.rs) groups consecutive samples into
*scenes* and scores a scene as **how long it lasted × how much detail its best
frame holds**.

Three decisions inside that are load-bearing:

* **A cut is measured against the frame that opened the scene, never against the
  previous sample.** A slow pan moves the picture by a fraction of a per cent in
  a sixth of a second, so comparing neighbours never sees a cut and swallows a
  whole sixty-second intro into one "scene". Against a fixed anchor the drift
  accumulates and the scene ends where it visibly ends.
* **Detail is the count of distinct 8×8 blocks**, which is the unit a Game Boy
  draws in. A dithered sky or water looks like the busiest thing on screen pixel
  by pixel and is a handful of tiles repeated; an edge-density measure rates it
  as highly detailed and this one does not. That is the difference between
  picking a game's title screen and picking its sea.
* **Both terms are needed.** A long *blank* scene scores zero (duration alone is
  a black screen); a detailed one-frame flash barely registers.

**Several candidates, not one.** Titles animate in, and any single moment is a
guess. The picker keeps the best few *distinct* scenes and `c` cycles them, which
turns "we picked wrong" from a defect into a keypress.

## 4. The picture and the sound agree

A candidate is a **moment**, not just a picture: it carries the frame its scene
started on *and a save state of the machine as it stood there*
([`Album::start`](../src/music/library.rs)). Playing an album and exporting it
both begin from that state.

This is not a nicety. Pokémon Gold plays about **a minute of intro** before its
title screen. Without it the shelf would show the title and play the intro
underneath, and a "sixty second" export would be sixty seconds of the wrong
music. Cycling covers with `c` re-drops the needle for the same reason.

It also means the shelf does not start instantly: an album's music begins when
its probe lands (about two seconds), and the header says `listening… 1/3` while
it does. Starting at reset and seeking later was tried and is worse — the first
seconds of most cartridges are a silent publisher logo, so it buys no music and
then jumps.

## 5. The sleeve is a still, deliberately

The cover is the chosen still frame, not the live picture, though the machine is
running right there and it was written the live way first. A record sleeve does
not animate; `c` would have nothing to cycle; and — the deciding reason — **a
title screen is not where a game stays.** Pokémon Blue rolls into an attract
demo after about twenty seconds, so a shelf showing the live frame quietly turns
into gameplay footage of a game nobody is playing.

`g` is how you get the live picture: it leaves the shelf and starts the game
properly, with a save file this time.

## 6. Music mode never touches your saves

Every machine music mode builds is constructed with `filepath = None`, so the
MBC has nowhere to write cartridge RAM. Browsing a shelf boots every cartridge
on it; a music player that overwrote a real Pokémon save with the pristine RAM
of a thirty-second listen would be the worst bug this repository has shipped —
and it has shipped it once already, which is why
[`src/mbc/battery.rs`](../src/mbc/battery.rs) exists.

The mapper is still chosen from header byte `$0147` (`get_mbc` takes the path
only for persistence), so nothing about the machine is less accurate.

Pinned on **both** paths, and both proven to fail when the path is put back:

* `music::library::tests::probing_a_battery_cartridge_writes_nothing_beside_it`
* `screen::tui::tests::playing_an_album_never_writes_a_save_beside_the_rom`

The fixture is [`testrom::battery_rom`](../src/music/testrom.rs), which *writes*
to its battery-backed RAM. One that merely declares RAM proves nothing: the
battery layer deliberately writes nothing for a session that never touched
cartridge RAM, so a test built on one passes either way.

No plugins are attached either. A trainer panel or a link partner appearing
because you opened a music player would be a surprise.

## 7. Export to WAV

`w` renders `music_export_seconds` (default 60) of the album's own audio and
writes it to `music_dir`. Three properties worth keeping:

* **It re-boots the cartridge rather than tapping the live stream.** The take
  starts at the beginning of the theme instead of wherever the listener happened
  to be, it is deterministic (same ROM + same length ⇒ byte-identical file), and
  it does not perturb playback.
* **It runs on its own thread.** A minute of audio renders in about a second of
  CPU; doing that on the loop thread would drain the audio queue's ~41 ms
  cushion and be heard as a gap. Same rule as everywhere else in this frontend —
  see [`docs/audio.md`](audio.md) §4c.
* **A WAV of silence is reported as such.** "Saved … — but no sound heard" beats
  reporting success and letting the player find out.

The encoder is **not ours**: it is
[`terminalgb_chiptune::wav`](../crates/terminalgb-chiptune/src/wav.rs), which the
chiptune renderer and `testharness/audiocapture.rs` also use. There is
deliberately one WAV writer in this repository, agreed with the chiptune crew
before either of us wrote a line of it.

## 8. Honest verdicts

[`AudioVerdict`](../src/music/library.rs) has four states, not a bool, because
**"silent" and "the sound chip never ran" look identical from the outside and
mean opposite things** — one is a property of the cartridge, the other is a bug
in us. `Sound::do_cycle` returns immediately when no `AudioPlayer` is installed,
so a capture harness without one has *no APU at all* rather than a quiet one.
The recorder counts its own calls, so the two are distinguishable, and the panel
says `sound chip did not run — this is a fault in the emulator, not the game`.

`music::capture::tests::silence_is_reported_as_silence_not_as_an_absent_apu`
pins it.

## 9. Measured

Against the three cartridges on this machine (Pokémon Blue, Gold, Silver),
`cargo run --release --example music_probe -- <dir>`:

| | cover | audio | probe |
|---|---|---|---|
| POKEMON BLUE | ✅ title screen, 4 candidates | ✅ peak 0.91, 15 % silent | 1.9 s |
| POKEMON_GLD | ✅ title screen, 4 candidates | ✅ peak 1.00, 16 % silent | 3.8 s |
| POKEMON_SLV | ✅ title screen, 4 candidates | ✅ peak 1.00, 16 % silent | 2.6 s |

**3/3 usable covers, 3/3 audible title themes, none falling back to best-effort.**
Verified by eye (the three title screens) and by spectrogram (a fanfare, then a
structured arrangement with a phrase change where the theme loops).

The probe window is **75 s** and that is why: Blue reaches its title screen at
~25 s and Gold/Silver at ~55–65 s after a long intro. At 26 s Gold offered the
sea. It costs ~2–4 s of CPU per cartridge, on a worker thread, while the shelf is
already browsable — every album is on the shelf with its header title from the
first frame.

## 10. Configuration

| key | default | meaning |
|---|---|---|
| `rom_dir` | `~/roms` | the shelf is this directory |
| `music_dir` | `$XDG_MUSIC_DIR/gameboy`, else `~/Music/gameboy` | where `w` writes |
| `music_export_seconds` | `60` | length of one export, 5–600 |

## 11. Keys

| key | does |
|---|---|
| `←` `→` `↑` `↓` `h` `j` `k` `l` | browse (the music keeps playing) |
| `⏎` | play the selected album |
| `w` | save what you are hearing as a WAV |
| `c` | next cover candidate — and re-drops the needle, since a candidate is a moment |
| `space` | pause |
| `g` | leave the shelf and play the game properly |
| `,` `.` `m` | volume / mute |
| `r` | rescan the directory |
| `?` | help · `q` `esc` leave |

## 12. Where it lives

Music mode is a **feature, not core emulation**. Everything is behind the `music`
cargo feature, so `--no-default-features` (the embedding build) carries none of
it and `make embed` is unaffected.

| what | where |
|---|---|
| capture a cartridge's own audio | [`src/music/capture.rs`](../src/music/capture.rs) |
| pick cover art out of a boot | [`src/music/cover.rs`](../src/music/cover.rs) |
| the shelf model, probing worker, WAV export | [`src/music/library.rs`](../src/music/library.rs) |
| hand-assembled fixtures (no game needed) | [`src/music/testrom.rs`](../src/music/testrom.rs) |
| layout + the wording | [`src/screen/music_view.rs`](../src/screen/music_view.rs) |
| the screen itself | [`src/screen/tui.rs`](../src/screen/tui.rs) (`Screen::Shelf`) |
| headless evidence tool | [`examples/music_probe.rs`](../examples/music_probe.rs) |

It is a **frontend feature, not a plugin**, for the same reason the MCP server
is one ([`docs/mcp.md`](mcp.md) §7): a `Plugin` is handed a `Machine` during a
frame the emulator has already decided to run, and music mode *drives* the
clock, replaces the cartridge, and owns host resources (a sound device, a music
directory). It uses no plugin surface and attaches none.

## 13. Conformance record

Per the [standing convention](../AGENTS.md#standing-convention-every-emulation-fix-feeds-the-conformance-tool).
Music mode fixed no emulation bug — but it is a *consumer* of emulated state, and
it found the same class of blind spot the render bugs did.

*Symptom* — a library that shows a fade or a patch of sea as a game's cover art,
and plays a minute of intro under a title screen.
*Caught by* — **no test ROM, and none could be.** The emulator was entirely
correct: every frame it produced was right, and every sample it played was right.
The defect was in choosing *which* of them to show. That is the same blind spot
as the terminal-render bugs ([`docs/rendering.md`](rendering.md)) and the
`wIsInBattle == $FF` misread ([`docs/conformance-notes.md`](conformance-notes.md)):
a framebuffer hash exposes none of it.
*Contract* — not a hardware one. The relevant contract is the product's own: the
shelf may only claim what it did.
*New tests* — `a_repeating_pattern_is_not_detail`,
`a_scene_that_lasts_beats_one_that_flashes_past`,
`a_long_scene_with_no_detail_scores_nothing`,
`the_shelf_never_claims_a_soundtrack_it_did_not_get`,
`the_panel_never_promises_more_than_a_title_theme`,
`silence_is_reported_as_silence_not_as_an_absent_apu`, and the two save-safety
tests in §6. Conformance candidate: **a tool that selects from emulator output
must be tested on what it selects, not only on what the emulator produced.**

## 13a. The cover in a terminal with no image protocol

**For as long as music mode existed, the cover was blank outside Kitty and
iTerm2** — and the panel beside it still printed `▦ cover 1/4 · … · c to
change`, so the shelf offered a picture it had no way to draw. Reported as a
regression; it never worked. The captain is on Ptyxis, which is VTE-family:
no kitty graphics, and Fedora builds VTE without sixel.

The cause is one branch. [`App::new_cover_image`](../src/screen/tui.rs) returns
`None` for any protocol that is not Kitty or iTerm2 — correctly, since those are
the two that can place an image over a *cell rectangle* — and
`place_panel_image` returns immediately when it has no image. The **game**
picture has had a fallback since [`screen::halfblocks`](../src/screen/halfblocks.rs)
landed; the cover simply never grew one.

It has one now, and it is the same one: two flat colours per character cell via
the `▀` glyph, sampled nearest-neighbour at pixel centres. So the cover draws in
**every** terminal.

* **Cost was measured rather than assumed, and it is not a reason to weigh.**
  One integer pass over the cell grid, no allocation: **7.8 µs** for a 40×25
  cover, **14.0 µs** at 56×34, **27.9 µs** at 80×48 (release, best of 5,000
  draws each). Against a 16.743 ms frame that is 0.05–0.17 %, it is paid only on
  a frame the shelf actually redraws, and the game picture does the same work
  over a larger rectangle at 59.7275 Hz. **The game's own frame path is
  untouched** — `render_display` and `step_frame` are not changed by this.
* **On a sixel terminal the cover is half-blocks while the game picture is real
  pixels**, which is a visible difference and is deliberate. Sixel has no
  cell-addressed placement, so it would need `ratatui-image`'s resize protocol —
  which writes its whole payload into a single cell symbol, the defect
  [`docs/rendering.md`](rendering.md) §2 records as re-emitting the entire
  screen. For a still sleeve that trade is not worth taking; a crisp,
  lower-resolution cover that always works is the better answer.
* **`c to change` is no longer offered when nothing can be shown.** The one
  remaining case is a window too narrow for a legible sleeve, where
  [`layout`](../src/screen/music_view.rs) drops the cover rather than smear it.
  The panel now says the sleeve is hidden and why, instead of naming a key whose
  effect is invisible — `music_view::cover_line` is the only place that wording
  is decided and `the_panel_never_offers_a_cover_key_it_cannot_honour` renders
  it. Silence was the bug; a smaller silence would still have been one.

## 14. Known, and not fixed here

* **The help overlay used to render clipped**, cut at exactly the cover's edge,
  because a still picture's cells are marked `skip` and a popup drawn across
  them loses whole runs of characters. `render_cover` now takes the cover off
  the terminal while a popup is up and puts it back (re-transmitted) when it
  closes. **Found and verified by screenshotting a real terminal, not by the
  harness**: `ratatui`'s `Clear` calls `Cell::reset`, which clears `skip` over
  the popup's own rectangle, so a `TestBackend` always flushes the popup and
  `the_help_overlay_is_not_eaten_by_the_cover` stays green with the fix removed.
  It is kept as a content/box-integrity guard and its doc comment says so.
* **A cover placed in a cell rectangle whose aspect does not match 160:144 is
  letterboxed with black by the terminal.** Music mode fits the cover with the
  *same* arithmetic as the game picture (`fit_metrics`), and the game picture
  has identical bands on the same display — so this is a pre-existing property
  of the terminal's cell-rect placement, not something music mode introduced. It
  is visible in the game view too; fixing it belongs with the render path.
* **Sound tests and per-game drivers** (§1, approaches 2 and 3) are the way this
  format grows into whole soundtracks, and both belong in a plugin.
* **No cover cache.** Re-entering the shelf re-probes. A cache would have to live
  outside the repository (it is game-derived art) and be invalidated on the ROM's
  checksum; worth doing when a shelf gets large enough to notice.
