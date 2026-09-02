# Every control, and where each one is explained

This is the written list the in-app **Controls** panel and the **`?`** overlay are short
versions of. Every row links to the document that explains that key properly — so if you have
seen `C raw shot` in the corner of your terminal and wondered what a raw shot is, this is the
page that answers it.

It is **generated from [`src/screen/controls.rs`](../src/screen/controls.rs)**, which is the same
table the Controls panel renders from and the same table the `?` overlay is tested against. That
matters more than it sounds: before it existed there were three lists with no relationship
between them, and two of them had already forgotten a key. See
[How this page cannot go stale](#how-this-page-cannot-go-stale) at the bottom.

**This page is the terminal frontend.** The desktop and web frontends are much smaller and are
covered in [The other two frontends](#the-other-two-frontends).

> **One caveat that applies to the whole page.** In a terminal that cannot report a key coming
> **up**, a held button is let go shortly after your terminal stops auto-repeating it. Which
> terminals do it properly, and exactly what you lose, is [What your terminal can and cannot
> tell us](#what-your-terminal-can-and-cannot-tell-us).

---

<!-- controls:begin -->
<!-- generated from src/screen/controls.rs — `UPDATE_CONTROLS_DOC=1 cargo test --lib controls_doc` -->

### The ROM picker

The first screen, and where `L` takes you back to. Opens on the ROM you played last, so <kbd>Enter</kbd> resumes it.

| Key | What it does | Read more |
|---|---|---|
| <kbd>↑ ↓</kbd> <kbd>j k</kbd> | move the cursor | — |
| <kbd>Enter</kbd> | play the selected cartridge | [Quick start](quickstart.md) |
| <kbd>/</kbd> | filter by title — type to narrow the list; <kbd>Esc</kbd> clears it, <kbd>Enter</kbd> plays the top match | — |
| <kbd>r</kbd> | rescan `rom_dir` | [Where your ROMs go](quickstart.md) |
| <kbd>.</kbd> | resume the cartridge you played last — works even when that cartridge lives outside `rom_dir`, so it is not in the list | — |
| <kbd>g G</kbd> <kbd>Home End</kbd> | jump to the top or the bottom | — |
| <kbd>M</kbd> | open music mode — the same key as in game — you do not have to come back here first | [Music mode](music-mode.md) |
| <kbd>@</kbd> | sign in to the trade portal — the same key as in game; do it before or after loading a cartridge | [The trade portal](portal.md) |
| <kbd>?</kbd> | the help overlay | — |
| <kbd>q</kbd> <kbd>Esc</kbd> | quit | — |

### The Game Boy itself

The four buttons are **configurable** (`key_a`, `key_b`, `key_start`, `key_select` in `config.toml`); the defaults are shown. A configured character always wins over an emulator control that wants the same key — `key_a = m` still means A, and mute moves out of the way.

| Key | What it does | Read more |
|---|---|---|
| <kbd>↑ ↓ ← →</kbd> | the d-pad | [Joypad](components/input.md) |
| <kbd>a</kbd> | **A** (`key_a`) | [Joypad](components/input.md) |
| <kbd>s</kbd> | **B** (`key_b`) | [Joypad](components/input.md) |
| <kbd>x</kbd> | **Start** (`key_start`) | [Joypad](components/input.md) |
| <kbd>z</kbd> | **Select** (`key_select`) | [Joypad](components/input.md) |

### Playing

The emulator itself, while a game is running.

| Key | What it does | Read more |
|---|---|---|
| <kbd>p</kbd> | pause and resume | — |
| <kbd>R</kbd> | reset the machine | — |
| <kbd>+</kbd> <kbd>=</kbd> <kbd>-</kbd> <kbd>_</kbd> | faster / slower — 0.5x · 1x · 2x · 4x · MAX. `1x` is the real 59.7275 Hz. Audio is emitted at a fixed rate, so fast-forward **drops samples** rather than raising the pitch | [Fast-forward and audio](audio.md) |
| <kbd>t</kbd> | the stats overlay — it does not pause the game | [Telemetry](telemetry.md) |
| <kbd>,</kbd> <kbd><</kbd> <kbd>.</kbd> <kbd>></kbd> | quieter / louder — **playback only** — the emulated machine's own master volume (`NR50`) is untouched, so no test ROM, save state or capture can see this | [Why this is not NR50](audio.md) |
| <kbd>m</kbd> | mute — remembers the level you were at | [Audio](audio.md) |
| <kbd>L</kbd> | back to the ROM picker — music mode is `M`, from here or from the picker | — |
| <kbd>M</kbd> | open music mode — puts the cartridge down first, exactly as <kbd>L</kbd> does; the same key opens the shelf from the picker | [Music mode](music-mode.md) |
| <kbd>f</kbd> | fullscreen — the picture, without the panels — <kbd>Esc</kbd> leaves it again | [Scaling and fit](display.md) |
| <kbd>Esc</kbd> | leave fullscreen — does nothing otherwise — it is not a second quit key | [Scaling and fit](display.md) |
| <kbd>?</kbd> | the help overlay | — |
| <kbd>q</kbd> | quit — asks first | — |

### Save states

Three slots per cartridge, held outside the game's own save so they cannot damage it.

| Key | What it does | Read more |
|---|---|---|
| <kbd>1 2 3</kbd> | choose the slot `F5`/`F8` will use — there are three; <kbd>4</kbd>-<kbd>9</kbd> say so rather than doing nothing, and a digit you have bound to a button stays that button | [Save states](savestates.md) |
| <kbd>F5</kbd> | quick-save to the active slot — a state is refused by a different cartridge, and by a build whose state format has moved on | [Save states](savestates.md) |
| <kbd>F8</kbd> | quick-load the active slot | [Save states](savestates.md) |

### How it looks

Everything here is cosmetic except `P`, which changes how the hardware is emulated.

| Key | What it does | Read more |
|---|---|---|
| <kbd>[</kbd> | cycle the palette | [Palettes and scaling](display.md) |
| <kbd>]</kbd> | cycle the pixel-art upscaler | [The upscalers](upscaling.md) |
| <kbd>}</kbd> | cycle the upscale factor — each algorithm allows its own factors, so this clamps | [The upscalers](upscaling.md) |
| <kbd>P</kbd> | picture mode: `identical` ⇄ `standard` — **this one is not cosmetic**: `identical` is the default and draws the picture dot by dot as the hardware does; `standard` draws a whole scanline at a time for about an eighth of the PPU cost. Most games look the same either way | [Picture modes](picture-modes.md) |

### Capturing it

Two kinds of still and two kinds of moving picture — see the note under the table.

| Key | What it does | Read more |
|---|---|---|
| <kbd>c</kbd> <kbd>F2</kbd> | screenshot — what you see: the palette and the upscaler are applied. <kbd>F2</kbd> is the same key | [Screenshots](picture.md#4-screenshots--c-and-c) |
| <kbd>C</kbd> | raw 160×144 screenshot — the archival one: exactly what the emulated hardware drew, nothing applied | [Why the raw one matters](picture.md#4-screenshots--c-and-c) |
| <kbd>v</kbd> | live feed to `video_out` (a FIFO, or a webcam device) on / off — starts **off**. The feed carries the **game picture only** — no panels, by design; to stream the panels too, capture this window in OBS | [Video out and the virtual camera](video-out.md) |
| <kbd>V</kbd> | record a timestamped file — a different job from `v`, hence a different key | [Recording](video-out.md) |

### The link cable

Bound **only when there is a cable**: a second cartridge (`link = <rom>`), a discovered local peer (`link = auto`, the default), or a socket (`link_net = …`). A key that does nothing is worse than no key at all.

| Key | What it does | Read more |
|---|---|---|
| <kbd>W</kbd> | swap console / cable on and off — with `link = <rom>` it swaps which of the two consoles takes your keys; with a discovered or network peer it plugs the cable in and out. The panel row says which | [The link cable](link-cable.md) |

### Trainer view

Reads a running Pokémon Red/Blue cartridge. Read-only: it never writes to the game.

| Key | What it does | Read more |
|---|---|---|
| <kbd>T</kbd> | show / hide the trainer panel | [Trainer view](trainer.md) |
| <kbd>d</kbd> | detail for the focused Pokémon — only bound while the panel is up, so it stays free as a game button | [Trainer view](trainer.md) |
| <kbd>Tab</kbd> <kbd>Shift-Tab</kbd> | walk the focus ring — in a battle the ring starts at the opponent, then your party | [Trainer view](trainer.md) |

### The AI agent

An agent presses the buttons; it never writes to the game. Needs `plugins = agent` and a driver.

| Key | What it does | Read more |
|---|---|---|
| <kbd>A</kbd> | show / hide the commentary panel | [Claude plays Pokémon](claude-plays.md) |
| <kbd>G</kbd> | hand the controller to the agent, or take it back — the game keeps running either way — this only stops something else pressing the buttons | [Claude plays Pokémon](claude-plays.md) |

### Battle control

Pokémon Red/Blue, `plugins = gen1-battle`. Writes the party into work RAM for the fight and puts it back afterwards; your `.sav` is untouched unless you save in-game.

| Key | What it does | Read more |
|---|---|---|
| <kbd>B</kbd> | start the selected battle preset | [Battle control](gen1/battle-control.md) |
| <kbd>N</kbd> | choose the next preset | [Battle control](gen1/battle-control.md) |

### The trade portal

Asynchronous trades with friends over HTTPS. Needs `portal = <url>` and `plugins = gen1-link`.

| Key | What it does | Read more |
|---|---|---|
| <kbd>@</kbd> | sign in — opens your browser; the device code is shown in the app too | [The trade portal](portal.md) |
| <kbd>O</kbd> | offer a Pokémon — picked from the party and boxes of the loaded save, **read out of the cartridge, never typed** | [Offering from the emulator](portal.md) |

### Music mode

A separate screen: `M` — from the picker **or from a running game** — or `terminal music` from the shell. It plays cartridges as albums and **never opens a save file**.

| Key | What it does | Read more |
|---|---|---|
| <kbd>← → ↑ ↓</kbd> <kbd>h j k l</kbd> | browse the shelf — the music keeps playing | [Music mode](music-mode.md) |
| <kbd>Home</kbd> <kbd>End</kbd> | first / last album | [Music mode](music-mode.md) |
| <kbd>Enter</kbd> | play the selected album | [Music mode](music-mode.md) |
| <kbd>Space</kbd> | pause | [Music mode](music-mode.md) |
| <kbd>c</kbd> | next cover candidate — a cover is a *moment*, not just a picture — each candidate starts the music where its own scene began | [The cover](music-mode.md) |
| <kbd>w</kbd> <kbd>W</kbd> | save what you are hearing as a WAV | [Music mode](music-mode.md) |
| <kbd>g</kbd> | leave and play this cartridge properly — this time with a save file — music mode never opens one | [Music mode](music-mode.md) |
| <kbd>r</kbd> | rescan the shelf | [Music mode](music-mode.md) |
| <kbd>,</kbd> <kbd>.</kbd> <kbd>m</kbd> | quieter / louder / mute | [Audio](audio.md) |
| <kbd>q</kbd> <kbd>Q</kbd> <kbd>Esc</kbd> | back to the ROM picker | [Music mode](music-mode.md) |

### Dialogs

Keys that only exist while a dialog is up. Every one of them is escapable.

| Key | What it does | Read more |
|---|---|---|
| <kbd>any key</kbd> | close the `?` overlay | — |
| <kbd>y</kbd> <kbd>Enter</kbd> | confirm the quit prompt — any other key cancels — the prompt says so itself | — |
| <kbd>↑ ↓</kbd> <kbd>Enter</kbd> <kbd>Esc</kbd> | the trade-offer picker — type to filter, <kbd>Enter</kbd> arms the highlighted Pokémon and <kbd>Enter</kbd> again submits it | [Offering from the emulator](portal.md) |

<!-- controls:end -->

---

## Two things the table cannot say in a cell

**`c` and `C` are the same picture twice, for two different purposes.** `c` (or <kbd>F2</kbd>)
saves what you are looking at: the palette is applied and the upscaler has run, so it is the
screenshot to put in a message. `C` saves the raw 160×144 framebuffer, exactly what the emulated
hardware drew — the archival one, the one to compare against a reference, and the one a bug
report should carry. At the defaults (`grayscale`, `nearest` ×1) the display path is the
identity and the two files are byte-identical; press `[`, `]` or `}` and they stop being.
[The whole story, with example pictures →](picture.md#4-screenshots--c-and-c)

**`v` and `V` are two different jobs, which is why they are two keys.** `v` is the live feed to
whatever `video_out` names — a FIFO, or a `v4l2loopback` device that appears in Chrome, Meet,
Discord and OBS as a webcam. `V` records a timestamped file. Neither is on at startup, and the
feed carries the **game picture only** — no panels, by design: they are text, and the terminal
that rasterises them is not in that pipeline. To put the panels on a stream, capture this window
in OBS (it is titled `TerminalGB`) and use OBS Virtual Camera.
[Video out →](video-out.md) · [Why not compose the panels →](stream-panel-routes.md)

---

## What your terminal can and cannot tell us

The kitty keyboard protocol reports a key going **down** and a key coming **up**. A plain
terminal only ever tells you a key was pressed; if you hold it, all you get is the operating
system's auto-repeat. So there are two input modes, both playable, and the status panel's
**`Input`** row says which one you are in.

| | `kitty · press+rel` | `basic · autorepeat` |
|---|---|---|
| Hold a direction | exact — down until you let go | one step, a pause of the auto-repeat *delay* (~0.5 s), then continuous |
| Tap a direction | exactly one step | exactly one step |
| Let go | immediate | ~0.15 s later (adapts to your repeat rate, 0.09–0.35 s) |
| Two buttons at once | yes, including diagonals | only briefly — a terminal auto-repeats the *last* key held |
| Hold a control key (`[`, `]`, `v`, `f`) | cycles | fires once, not thirty times a second |

Nothing else differs and the emulation is identical. **kitty**, **Ghostty** and **foot** enable
the protocol by default; **WezTerm** needs `enable_kitty_keyboard = true`; GNOME Terminal,
GNOME Console and Ptyxis are VTE, which does not implement it, so they get the basic mode.

The emulator never refuses to start over this. It detects, adapts and says so once, to the
scrollback — a missing optional capability may cost only that capability.

---

## The other two frontends

**Desktop** (`--features desktop`, an OpenGL window) binds the game pad and nothing else:
<kbd>↑</kbd> <kbd>↓</kbd> <kbd>←</kbd> <kbd>→</kbd>, <kbd>A</kbd>, <kbd>B</kbd>, <kbd>X</kbd>
(Start), <kbd>Z</kbd> (Select) — note that A and B are the literal letter keys there, not the
terminal's configurable `key_a`/`key_b`. Everything else is a command-line flag: `--model`,
`--scale`, `--upscale`, `--upscale-factor`, `--speed`, `--run-unfocused`, `--video-out`. See
[`src/screen/desktop.rs`](../src/screen/desktop.rs) and [the Shootout entry](shootout.md), which
is why those flags exist.

**Web** (the wasm build) binds the same pad — <kbd>A</kbd>, <kbd>S</kbd>, <kbd>X</kbd>,
<kbd>Z</kbd> and the arrows — plus <kbd>]</kbd> to cycle the upscaler and <kbd>\\</kbd> to cycle
its factor. Note the factor key differs from the terminal's <kbd>}</kbd>. Sound needs one key
press or click first: browsers refuse to start audio without a user gesture. See
[`src/screen/web.rs`](../src/screen/web.rs) and [Web audio](web-audio.md).

**Not a frontend, but it drives the same buttons:** the [MCP server](mcp.md) exposes
`press_buttons` and `run_actions` so Claude Desktop, Claude Code or any MCP client can play,
and the [agent plugin](claude-plays.md) does it from inside the emulator.

---

## How this page cannot go stale

A controls list rots the moment somebody adds a key. This one is wired so it cannot:

- **[`src/screen/controls.rs`](../src/screen/controls.rs) is the single table.** The in-app
  Controls panel is rendered from it (`panel_lines`), and the block above is generated from it
  (`markdown`).
- `the_controls_doc_matches_the_registry` fails the build when the two disagree. Re-bless in
  the same commit as the key that moved: `UPDATE_CONTROLS_DOC=1 cargo test --lib controls_doc`.
- `the_help_overlay_names_every_key_in_the_registry` fails when a key is in the registry and the
  `?` overlay does not mention it. It reads the overlay's *text*, not the rendered picture, so a
  line pushed past the bottom of the popup is treated as the separate layout problem it is.
- `every_control_links_to_a_page_that_exists` fails on a link that does not resolve, and
  `make docs-check` checks every relative link on this page too.

**A finding worth recording, since it is the reason this page exists.** Building the registry
meant reading the key handler rather than the panel, and two keys turned out to be bound and
named *nowhere*: <kbd>F2</kbd> (a second screenshot key) and the <kbd>Esc</kbd> that leaves
fullscreen. The quit prompt's <kbd>y</kbd> and the trade-offer picker's keys were only ever
written in their own dialogs. All four are in the `?` overlay now, and none of them could go
missing again without the build going red.

---

## Where the rest of the story lives

| | |
|---|---|
| [Quick start](quickstart.md) | three commands, and where your ROMs go |
| [How the picture reaches you](picture.md) | transports, sharpness, screenshots, upscalers |
| [Picture modes](picture-modes.md) | `standard` versus `identical`, and what it costs |
| [Display and scaling](display.md) | palettes, fit, HiDPI, the three frontends |
| [Audio](audio.md) | pacing, latency, and why the volume keys are not `NR50` |
| [Save states](savestates.md) | the format, and what it deliberately does not store |
| [Video out](video-out.md) | the FIFO, the virtual camera, recording |
| [The link cable](link-cable.md) · [over TCP](bgb-link.md) | two consoles, one wire |
| [Trainer view](trainer.md) | the live Pokémon panel |
| [Claude plays Pokémon](claude-plays.md) | the agent and its commentary panel |
| [The trade portal](portal.md) | signing in, offering, receiving |
| [Music mode](music-mode.md) | cartridges as albums |
| [Plugins](plugins.md) | what exists and how to arm it |
| [The config file](../README.md#configuration) | every key that is not a keystroke |
