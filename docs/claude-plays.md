# Claude Plays Pokémon — the agent substrate

> **Keys:** `A` commentary panel · `G` hand the controller to the agent or take it back. [Every control →](controls.md)

*Just like Twitch Plays Pokémon, but the player is a model, and it says what it
is thinking while it plays.*

This page is the design, the honest limits, and the OBS recipe for putting it on
Twitch. The code is [`plugins/agent/src/`](../plugins/agent/src/lib.rs), the panel is
[`src/screen/agent_view.rs`](../src/screen/agent_view.rs), the headless runner is
[`examples/claude_plays.rs`](../examples/claude_plays.rs), and the proof is
[`testharness/agent.rs`](../testharness/agent.rs).

**If you are integrating rather than reading**, two newer pages carry the parts this
one only describes: [`docs/agent-reference.md`](agent-reference.md) is every hook,
field and payload with generated examples, and
[`docs/agent-environment.md`](agent-environment.md) is the headless, at-speed,
run-agents-against-it case.

**Scope of this milestone.** This is the *substrate*, not an agent that finishes
the game. The target it was built against and measured on is: from a fresh save,
get out of the bedroom, downstairs, out of the house, and pick a starter —
driven by the agent, not by a script written for those steps. See
[§Proof](#7-proof) for what actually happened.

---

## 1. The constraint that decides everything

**The model is slow and the game is not.** A round trip takes seconds; the Game
Boy runs at 59.7275 frames a second. The two obvious designs both fail:

| design | why it fails |
|---|---|
| ask the agent every frame | 60 calls a second. Costs a fortune, and the agent has nothing new to say between frames anyway. |
| let the emulator wait for the agent | the stream becomes a slideshow, the audio dies, and a slow or broken agent takes the whole show down with it. |

The shape that works is **plan, then execute locally**:

```
   ┌── snapshot (a few hundred bytes of structured state)
   │
   ▼
 driver  ── "goal + commentary + up to 12 actions" ──▶  executor
 (slow,                                                 (local, one
  seconds)                                               frame at a time)
   ▲                                                        │
   │                                                        ▼
   └──── outcomes: what each action actually achieved ◀── hundreds of
                                                          emulated frames
```

The agent is consulted again only when the plan **finishes**, **fails**, or the
world does something the plan did not account for — a battle starts, the map
changes. One call therefore buys a few hundred to fifteen hundred emulated
frames.

Measured on the real opening of Pokémon Blue: **about one call per 1,100
emulated frames**, i.e. one every ~18 seconds of game time rather than 60 a
second. The single biggest saving is one action:
[`advance_text`](#22-action--how-the-agent-drives) turns thirty `A` presses into one decision, and the
opening of this game is mostly dialogue.

### The emulator never waits

A driver is **polled, never awaited**. [`Driver::submit`] hands over a request
and returns immediately; [`Driver::poll`] is called once per frame and answers
`Pending` until there is something to say. The subprocess driver runs the child
on its own thread; the emulator thread only ever does a `try_recv`.

Measured (`testharness/agent.rs`, in CI): with a driver that never answers,
**0.24 ms per emulated frame** against a 16.74 ms budget, worst frame 0.8 ms.
With a real model taking ten seconds a call and the runner pacing to real time,
the reported rate holds at **60 emulated fps throughout**.

---

## 2. The three pieces

### 2.1 Observation — what the agent can see

Structure is the channel; pixels are the fallback. An agent that has to infer
everything from an image spends its budget on perception and never gets to play.

[`plugins/agent/src/observe.rs`](../plugins/agent/src/observe.rs) exposes exactly what a
decision needs, and each field earns its place:

| field | why it is exposed |
|---|---|
| `mode` — title / overworld / dialogue / menu / battle / transition | pressing A in a menu and in a battle are different acts |
| `game.map` — id, name, `x`, `y`, `facing` | the only thing that makes "did that work?" answerable |
| **`game.exits`** — every door and staircase on this map, and where it leads | a human *sees* a door; an agent reading coordinates has no idea one exists. Read from the game's own warp table, not guessed |
| **`screen.text`** — the screen decoded as 18 lines of text | Gen 1 loads its font so a tile id **is** a character code, so the tilemap *is* the text. An agent that can read dialogue follows the game's own instructions instead of guessing |
| `screen.waiting_for_input` — the ▼ arrow is showing | the difference between "press A" and "wait for the animation" |
| `screen.menu_cursor_row` — where ▶ is | menu navigation without counting frames |
| `game.party`, `badges`, `money`, `battle` | progress, and what a battle decision needs |
| `last_actions` — what the previous plan actually achieved | **the most important field.** The agent cannot see the map; being told "walked 4 steps to (7,1)" or "blocked, a wall that way" is what makes blind navigation viable |
| `notes` — the host talking to the agent | stuck warnings, parse complaints, and the two situational hints in §4 |
| `budget` — calls used, fraction of the cap | so the agent knows it is on a clock |

Party, badges, money and the opponent come from the existing read-only
[trainer plugin's](trainer.md) verified RAM map rather than being re-derived.
Position, facing and the warp table are the three additions, each with its
`pret/pokered` symbol cited in the source.

**Reading the screen has one trap**, the same one
[`examples/gen1_trade.rs`](../examples/gen1_trade.rs) records: Red/Blue draw the
whole screen through the **window** layer, so reading the background map returns
20×18 spaces and every menu looks empty. This module reads whichever layer is
on. It reads **VRAM**, not the game's `wTileMap` shadow buffer, so what it
reports cannot disagree with the picture a viewer is looking at.

**The framebuffer** is available as `frame_png` in the snapshot when
`frame_capture` is configured — a PNG of the current frame, written at decision
points only. It is off by default and described honestly as the expensive
channel: the structured snapshot is ~1.3 KB, an image is orders of magnitude
more for a model to read. It exists for the cases structure cannot cover.

**What is deliberately *not* exposed:** a walkability map. Gen 1's collision
data would have to be reconstructed from the tileset tables, and a *wrong*
walkability map is worse than none — it would make the agent confidently walk
into walls. Instead the agent navigates by coordinates, exits, and honest
blocked/moved feedback. That is a real limitation and it is the first thing to
add next; see [§8](#8-what-is-missing).

### 2.2 Action — how the agent drives

One mechanism drives the buttons from outside the machine, and it is shared
with the battle-control crew, who defined the capability:
`Capability::CONTROL_INPUT`. Two levels onto one latch —

* `Machine::set_button(key, down)`, the raw hold/release, for a plugin that
  really is thinking frame by frame (their cinematic runner is);
* `Machine::press(key, hold, gap)`, a press **scheduled in frames** through
  [`crate::input::InputQueue`](../src/input.rs), advanced by the host exactly
  once per emulated frame *before* the frame runs. A frontend reaches the same
  queue through `Gameboy::queue_press`.

The agent uses the second, because a frame is the granularity the game samples
input at and anything reasoning above that level needs presses scheduled rather
than timed by hand. There is deliberately no third mechanism: a battle-control
script, a scripted test and an AI agent all drive the game, and three private
frame timers would be three subtly different definitions of "a press".

On top of that sit the primitives an agent can actually afford
([`plugins/agent/src/action.rs`](../plugins/agent/src/action.rs)):

```json
{"do":"press","button":"a|b|up|down|left|right|start|select","times":1}
{"do":"walk","dir":"up|down|left|right","steps":1-24}
{"do":"goto","x":N,"y":N}
{"do":"advance_text"}
{"do":"wait","frames":N}
```

* **`advance_text`** — hold a conversation down: press A, follow the ▼ arrow,
  stop when the game stops asking and the screen settles. One decision instead
  of thirty.
* **`walk`** — not "press down": press, wait out the step animation, compare
  coordinates, notice a wall. Reports what it achieved.
* **`goto`** — greedy movement towards a coordinate, alternating axis when
  blocked, which is enough to get round furniture with no collision data. This
  is what makes `game.exits` actionable.
* **`wait`** — let a warp or an animation finish.

**Everything is bounded.** Each action has its own cap, a plan is at most 12
actions, and a plan is abandoned after 1,500 emulated frames. A driver that is
wrong about the world costs one plan, not the session. Counts a driver sends are
clamped, and a malformed action is *reported back to the driver* rather than
silently dropped — a driver whose mistakes vanish cannot correct them.

Two of these rules were written after watching a real run fail:

* **A wall is three failed presses, not one.** A press that finds the player
  facing another way only turns them, and a press that lands while the game is
  still ignoring input does nothing. Declaring a wall on the first unchanged
  reading told the agent that *every* direction was blocked, and it spent six
  calls trying directions in a room it was not even standing in yet.
* **`advance_text` always presses A once.** Gen 1's opening speech spends long
  stretches on a still frame with no ▼ arrow, and an "advance text" that returns
  "read 0 boxes" in those moments burns a whole model call to achieve nothing.

### 2.3 The commentary panel

This is the part that makes it a stream, and the maintainer's brief narrows it: the
destination is **Twitch via OBS**, on a fixed 16:9 canvas, and the audience is
strangers with no context who cannot pause and cannot zoom.

So the panel is **not a scrolling log**. A log fails a cold joiner completely:
by the time they have read enough lines to work out what is going on, the lines
have moved. Instead, in priority order:

```
CLAUDE PLAYING
GOAL  Get out of the house and find Professor Oak
NOW   walk down 3
▸ walk right 4 — walked 4 step(s) to (7,1)
! Arrived: Red's house 1F.
· Downstairs now. The front door should be south.
▸ head for (2,7) — arrived at (2,7) after 6 step(s)
```

*(A real render — `GB_DUMP_PANEL=<file> cargo test --lib the_rendered_panel`.)*

1. **A badge**: `PLAYING` / `THINKING` / `PAUSED` / `WATCHING`, so "why is
   nothing happening" is never a mystery.
2. **`GOAL`** — the standing objective, bright, bold, changes rarely. This is
   the line a nine-second-old viewer reads.
3. **`NOW`** — what is physically happening this second.
4. **A short history** behind it, dimmed, newest nearest the eye. Four to six
   lines, not forty.

`A` toggles the panel in the terminal frontend; it opens automatically whenever
an agent is attached, because it *is* the show. `G` hands the controller back to
a human — the game keeps running either way.

#### Everything shown is treated as public and permanent

Anything the panel renders can be clipped and reposted forever. Two independent
defences, because one is not enough for something that cannot be taken back:

1. **Internal detail never enters the panel.** A driver failure reaches it as a
   *category* — "The agent took too long to answer — trying again." — never the
   exception, the command line, the exit status or the subprocess's stderr.
   Those go to the NDJSON log file.
2. **Everything that does enter is scrubbed.**
   [`commentary::sanitise`](../plugins/agent/src/commentary.rs) strips anything shaped
   like a credential (`sk-…`, `ghp_…`, bearer tokens, long opaque tokens,
   anything containing `api_key`/`secret`/`password`), absolute paths (`/home/…`,
   `~/…`, `C:\…`), source paths, URLs, email addresses and bare IP addresses;
   removes ANSI escapes, control characters and zero-width/bidi overrides; and
   **drops the line entirely** if what is left is not readable prose. Rejecting
   is always allowed: a missing line costs a viewer nothing.

It is a denylist, so it is not a proof — which is why rule 1 does the real work
and this is the net underneath it. `credentials_paths_and_urls_never_reach_the_panel`
pins ten real shapes.

Two smaller rules, both about being readable rather than safe: a line repeated
verbatim is not re-logged (a looping agent would otherwise fill the panel with
one sentence), and every line is capped and wrapped on a word boundary so it
cannot become a wall of text at phone size.

---

## 3. Keeping the agent swappable

Nothing in the emulator knows what Claude is. The contract is one line: **here
is the state as JSON; give me back JSON with some commentary and a short plan.**

```rust
pub trait Driver: Send {
    fn name(&self) -> &str;
    fn submit(&mut self, request: &str) -> bool;   // must not block
    fn poll(&mut self) -> Poll;                    // must not block
    fn cancel(&mut self) {}
}
```

A driver never receives a `Machine`, so it cannot press a button, read memory or
touch a save. The **host** parses the reply, clamps every number in it, and
performs the plan. That is what makes the deterministic test driver a complete
substitute for a model: it goes through the identical code path, including the
leniency for models that wrap JSON in prose and fences.

Four drivers ship:

| driver | what it is for |
|---|---|
| `ScriptedDriver` | canned replies, optional fake latency in frames. **Built first and used throughout.** This is what makes every other part testable without spending a penny |
| `RandomDriver` | deterministic random presses; shakes out executor bugs, and is the "the emulator is fine, the agent is not" control |
| `CommandDriver` | runs a program: state on stdin, plan on stdout. The provider-agnostic escape hatch |
| *(a human)* | `G` switches the agent off and the keyboard still works |

### Why a subprocess rather than an HTTP client

This crate takes no new dependencies, and an HTTPS client is a large one — TLS,
a runtime, a JSON library — that every build would then carry, including wasm
and the FFI static library. Pushing the network out to a small adapter keeps the
emulator dependency-free *and* makes the model genuinely swappable. The contract
is deliberately tiny:

```text
stdin   one line of JSON: the state snapshot
stdout  one JSON object:  {"goal":…, "say":…, "plan":[…], "usage":{…}}
stderr  diagnostics; logged by the host, never shown to viewers
exit 0  success; anything else is a failed call
```

One process per decision. It costs a few milliseconds against a multi-second
model call and buys three things worth more: a hung call is killed by killing a
process rather than by inventing a resynchronisation protocol, an adapter cannot
leak state between decisions, and one-shot CLIs work as adapters unchanged.

Two adapters ship in [`tools/claude_plays/`](../tools/claude_plays/), sharing one
[`prompt.md`](../tools/claude_plays/prompt.md):

* **`claude_api_adapter.py`** — the Anthropic Messages API over `urllib`, Python
  standard library only. The one to use for a real broadcast: one HTTPS request
  per decision, prompt caching on the instructions, and it reports token usage
  and an approximate cost back to the host.
* **`claude_cli_adapter.py`** — drives the local `claude` CLI, so it needs no API
  key and uses whatever login the machine already has. **This is the one the
  demo run below used.** It is convenient and it is expensive: the CLI loads a
  whole coding-agent system prompt every call. Measured: **$0.078 for the first
  call and $0.0086 thereafter** (the 1-hour prompt cache does the work), at
  **9–22 s per call**. The API adapter sends ~1.5 KB of prompt instead of ~18 KB
  and should be several times cheaper and faster; that comparison is *not*
  measured here, because this box has no `ANTHROPIC_API_KEY`.

Both have a strict per-attempt timeout, a hard retry cap and a cheaper
designated fallback model, as the project's guidance on external APIs requires.
The **budget cap and the circuit breaker are not in the adapter** — they are in
the host, where editing the adapter cannot bypass them.

---

## 4. The safety rails

All in [`plugins/agent/src/governor.rs`](../plugins/agent/src/governor.rs), all in the host.

### Budget: four independent ceilings

| ceiling | catches |
|---|---|
| **calls** | a fast loop |
| **tokens** (self-reported) | a slow loop with huge prompts |
| **spend** (self-reported) | the same, in money |
| **wall clock** | everything else — including a driver that reports no usage at all |

The wall clock is the backstop and it is the one that cannot be evaded: usage is
self-reported, so it is only ever a *floor* on the true cost. Reaching any
ceiling is permanent for the session; the panel says which one, in plain
English, and the game keeps running with nobody at the controller.

There is also a per-call timeout (the driver is cancelled and the child killed)
and an optional minimum interval between calls.

### Circuit breaker

`max_consecutive_failures` opens it; it stays open for a cooldown and then lets
exactly **one probe** through. Repeated trips mean the adapter is broken rather
than unlucky, so after three trips the agent is switched off for good rather
than retried forever. A failure reported with **no call in flight** is ignored —
otherwise a driver that reports one spuriously trips the breaker in
milliseconds, which reads as "the agent is broken" when the truth is "the driver
answered a question nobody asked".

### Stuck detection, escalating

A fingerprint of the observable world — map, coordinates, badges, party size and
a hash of the screen — is compared across decisions. Turning on the spot is
deliberately *not* progress; the frame number and the clock are deliberately not
in the fingerprint, because they always change.

| repeats | response |
|---|---|
| 2–3 | **nudge**: a note telling the agent nothing has changed |
| 4–6 | **insist**: "do not repeat your last plan; back out with B and move somewhere new" |
| 7+ | **intervene**: the host runs a local unstick routine (B twice to leave any menu, then a step away from whatever was being walked into) and starts the ladder again — no model call spent |

### Two situational notes, both found by watching a real run

Generic stuck detection notices *that* an agent is going nowhere. These two tell
it *why*, and both were added after watching the model flounder for several
calls in exactly these situations:

* **"You did not move at all."** Walking in every direction failing almost never
  means you are surrounded by walls; it means the game is not accepting movement
  — a cutscene, a text box, a menu. The note says so and says to use
  `advance_text` instead.
* **"You are standing ON an exit."** Gen 1 fires a warp when you step *through*
  a door, so arriving sideways along the bottom row of a house leaves you on the
  mat with nothing to show for it. A coordinate-only agent cannot work that out;
  the note tells it to take one more step in the direction the door faces, and
  that **A does not open doors**.

---

## 5. Running it

### In the terminal frontend (the streaming setup)

```toml
# ~/.config/terminalgb/config.toml
plugins         = agent
agent_command   = /path/to/gameboy/tools/claude_plays/claude_api_adapter.py
agent_objective = Get out of the house and choose a starter Pokemon
agent_max_calls = 200
agent_max_usd   = 5.0
```

or, without touching the config:

```sh
GB_PLUGINS=agent \
GB_AGENT_CMD=tools/claude_plays/claude_api_adapter.py \
GB_AGENT_OBJECTIVE="Get out of the house and choose a starter Pokemon" \
GB_AGENT_LOG=/tmp/agent.ndjson \
  cargo run --release --example terminal -- ~/roms/blue.gb
```

Keys: **`A`** commentary panel, **`G`** agent on/off, plus everything the
frontend already had. The Status panel gains a `Plugin` row reading
`agent>>: <driver> · playing · 12/200 calls · 6% of budget`; the `>>` marker
means *this plugin has pressed your buttons*, the same way `!!` means it has
written memory. A plugin cannot be active and invisible.

**Nothing attaches implicitly**, and with no `agent_command` the plugin refuses
to attach at all with an explanation, rather than sitting in the status bar
doing nothing.

### Headless

```sh
cargo run --release --example claude_plays -- \
  --rom ~/roms/blue.gb \
  --objective "Get out of the house and choose a starter Pokemon" \
  --driver command --command tools/claude_plays/claude_api_adapter.py \
  --max-calls 130 --max-usd 4 --timeout 150 --wall 7200 \
  --pace realtime --stop-on 'party>=1' \
  --shots target/run --log target/run/agent.ndjson --report
```

`--driver scripted --script <ndjson>` replays canned replies and costs nothing;
`--driver random --seed N` is the control. `--command` must come last (it takes
every remaining argument). The ROM is copied into `target/` first — the MBC
writes a `.sav` next to the ROM, and a demo run must never touch the player's
library.

### Headless *at speed* — no display, no terminal, no pacing

The command above has `--pace realtime` because it is a demonstration for a
human. **Drop it and nothing paces the emulator at all** — no window, no
terminal, no sound device, no `$HOME`, and the machine runs as fast as the CPU
allows. That is the configuration for an unattended run, a batch of episodes or
an evaluation:

```sh
cargo run --release --features image,plugin-agent --example claude_plays -- \
  --rom target/blue.gb --frames 200000 --log run.ndjson --report \
  --driver command --command ./my_adapter.py         # --command must be LAST
```

Measured on a real cartridge with a subprocess adapter answering every call:
**4,038 emulated fps — 67.6x real time**, with the adapter in the loop. The
emulator alone does 3,000–4,300 frames a second on a contended box, and sixteen
instances do 31,000. The MCP server is the same story with a different transport
(`terminalgb-mcp` is 880 KiB, links only libc, and runs under `env -i`).

**Which means the model is the bottleneck, not the emulator, and the only lever
that matters is frames per model call.** Measured, one call each:

| | frames | seconds of game |
|---|---:|---:|
| `press A` once | 32 | 0.5 |
| `walk down 1` | 31 | 0.5 |
| `advance_text` through 4 boxes | 291 | 4.9 |
| `goto (3,7)`, 11 steps round furniture | 331 | 5.5 |
| a real model on Blue's opening (§1) | ~1,100 | ~18 |

A deliberately naive adapter that emits single-press plans spent **235 calls on
23,587 frames** — 100 frames per call. Same emulator, same contract, 10x the
cost. Batch intentions; do not micro-step.

The whole case for using this as an evaluation environment — determinism across
architectures, fan-out numbers, save states as episode resets, recording a
dataset, and the ways an agent will cheat your score — is
[`docs/agent-environment.md`](agent-environment.md).

### The concrete payloads

This page describes the shapes. **[`docs/agent-reference.md`](agent-reference.md)
prints them**: three real `gbagent/1` snapshots with the driver replies to them,
every MCP tool's request and reply, and fourteen decoded screens — all generated
by running the real binaries (`tools/agent-docs/capture.py`) and committed under
[`docs/agent/examples/`](agent/examples/).

---

## 6. Streaming it: OBS → Twitch

The pieces already existed ([video-out](video-out.md), the virtual camera,
[telemetry](telemetry.md)); this section is the layout and the settings, not a
new streaming stack.

### What a viewer needs on the canvas

Three things, and nothing else: **the game**, **the commentary**, and **the
party**. Everything else is for the operator and belongs off-canvas.

```
┌────────────────────────────────────────────────────────────┐ 1920x1080
│ TerminalGB · POKEMON BLUE                                     │
│ ┌──────────────────────────┐ ┌───────────────────────────┐ │
│ │                          │ │  Trainer / party panel    │ │
│ │      GAME  PICTURE       │ │  (T)                      │ │
│ │      (terminal image)    │ │                           │ │
│ │                          │ │                           │ │
│ └──────────────────────────┘ └───────────────────────────┘ │
│ ┌ Commentary ──────────────────────────────────────────── ┐ │
│ │ CLAUDE PLAYING                                          │ │
│ │ GOAL  Get out of the house and find Professor Oak       │ │
│ │ NOW   walk down 3                                       │ │
│ │ ▸ walk right 4 — walked 4 step(s) to (7,1)              │ │
│ │ ! Arrived: Red's house 1F.                              │ │
│ └─────────────────────────────────────────────────────────┘ │
└────────────────────────────────────────────────────────────┘
```

The commentary panel is full width along the bottom, which is both what the
maintainer asked for and what gives it enough columns to stay readable at the large
font a stream needs.

### Canvas and encoder (Twitch)

| setting | value | why |
|---|---|---|
| Base (canvas) | **1920×1080** | fixed 16:9; Twitch's native tier |
| Output (scaled) | **1920×1080**, or 1280×720 on a modest uplink | never letterbox — pick one 16:9 and stay on it |
| FPS | **30** | the Game Boy runs at 59.73 Hz, but a terminal-image source is mostly static text; 30 halves the bitrate for no visible loss on this content. Use 60 only if the uplink is comfortable |
| Encoder | x264 `veryfast`, or NVENC if available | |
| Bitrate | **4,500 kbps** at 1080p30, **3,000** at 720p30 | Twitch's non-partner ceiling is ~6,000; text and flat colour compress well, so 4,500 is generous |
| Keyframe interval | **2 s** | Twitch requires it |
| Profile / tuning | `high`, `zerolatency` off | |
| Audio | 160 kbps AAC, 48 kHz stereo | |
| **Mirror / flip** | **off** | correct. OBS does not mirror a source the way a video-call self-view does, so flipping would make the game read backwards to viewers |

### Sources

1. **Picture and commentary — one Window Capture of the terminal.** The game and
   the panel are in the same window, so one source captures both and they can
   never drift out of sync.
   * Terminal: any with the Kitty graphics protocol or iTerm2 inline images
     (WezTerm and Kitty both work; see [rendering.md](rendering.md)).
   * **Font size 26–30 px.** This is the setting that decides whether the stream
     is readable, so set it deliberately: at 26 px a glyph is roughly 18 px tall
     on a 1080p canvas, which survives Twitch's re-encode and is still legible
     when a phone viewer downscales to ~360p. Below ~20 px the commentary turns
     to mush after encoding.
   * Window ≈ **106×30 cells** at 26 px, or **120×34** at 24 px. The layout is
     tested at exactly those grids
     (`the_stream_layout_shows_goal_activity_and_game_together`).
   * Turn the terminal's own scrollbar, tab bar and padding off; set the
     background opaque.
2. **Sound — capture the emulator's audio output.** Build with
   `--features audio` and add a *Application Audio Capture* (or a PulseAudio
   monitor source) for the emulator process. Do **not** capture the whole
   desktop: notifications are not part of the show.
3. **Optional: a "starting soon" scene** and a text source with the objective,
   for the top of a session.

### The operator's side, deliberately off-canvas

Point `GB_AGENT_LOG` at a file and tail it in another window. It carries every
request, every reply, the errors, the stderr and the cost — everything the panel
is not allowed to show. Do not put that window in a scene.

### Checks before going live

* Play a minute and read the panel **on a phone**, at the size Twitch will
  serve. If the `GOAL` line is not comfortable, raise the font and re-check the
  cell grid.
* Confirm the picture is not mirrored and not letterboxed.
* Switch the agent off with `G` and confirm the stream keeps running and stays
  interesting — that is what a viewer sees if the agent's budget runs out.
* Trigger a failure (point `agent_command` at a nonexistent program) and confirm
  the panel says *"The agent hit a problem — trying again"* and nothing else.

---

## 7. Proof

### Deterministic, in CI, costing nothing

`cargo test --release --test agent` — eleven cases against a **hand-assembled
Gen-1-shaped cartridge** (`build_fake_gen1`) that presents the same RAM layout
the real game does and walks a player around a walled room. It contains no game
data at all; that is what makes it runnable in CI, and a test CI skips proves
nothing.

```
  [PASS] self-check: the fixture really does respond to the buttons
  [PASS] a scripted plan walks the player to the exit and presses A
  [PASS] walking into a wall stops there and moving away still works
  [PASS] goto routes around a wall instead of giving up in front of it
  [PASS] the snapshot reports position, facing, exits and the screen
  [PASS] a snapshot carries no host paths unless frame capture is on
  [PASS] prose, fences and bad actions are tolerated; the good ones still run
  [PASS] the emulator runs at full speed while the agent is thinking
         0.247 ms/frame (budget 16.743), worst frame 0.8 ms
  [PASS] a dead agent leaves the game running at full speed
         0.236 ms/frame with no driver answering
  [PASS] the call budget stops the agent and says so publicly
         4 calls, halted Some(Calls), panel says "the agent has used up its
         decision budget"
  [PASS] the circuit breaker stops a broken driver without leaking its error
         12 calls before giving up, halted Some(Broken), leaked=false
  [PASS] an agent repeating itself is detected and interrupted
```

Plus ~60 unit tests across `plugins/agent/src/` and `src/screen/agent_view.rs`, including
the sanitiser's ten leak shapes and the panel's "goal before history" rule.

### A real agent playing the opening

From a cold boot with no save, unaided: out of the bedroom, downstairs, out of
the house, across Pallet Town, and into Oak's Lab as far as the screen asking
*"which POKéMON do you want?"* — 34 model calls, $0.68, 60 emulated fps
throughout. See [§10](#10-the-recorded-run) for both runs, the numbers, what it
did not finish, and where the recording is.

---

## 8. What is missing

Stated plainly, because "it works" would be an overclaim:

* **No spatial sense — this is the one that matters.** The agent has
  coordinates, exits and blocked/moved feedback, and nothing else. It cannot
  see a table, an NPC or a path, and three runs have now stalled on exactly
  that — twice circling Oak's Lab one tile from a Poké Ball nothing in the
  state pointed at. A `game.sprites` observation was **built and withdrawn**
  during the third session: the sprite table's layout is derived and correct
  (`wSpriteStateData1`/`2`, map Y/X at `+4`/`+5` in the player's tile grid
  offset by 4), but the "is this table in a consistent state?" gate was wrong
  — it anchored on the player's own slot-0 map coordinates matching
  `wYCoord`/`wXCoord`+4, and a live run measured that anchor **never true**
  across twenty decisions on four maps, so the feature silently reported
  nothing. The engine does not maintain slot 0's map coordinates during
  ordinary play (grep `pokered` for writes to `StateData2MapY`: only map
  *scripts* touch it). The next attempt needs a different consistency gate —
  per-slot bounds against the map size, verified against a live game before
  it ships — plus the walkability grid **derived** from the game's own
  tileset collision data. Derived, not guessed, because a wrong map is worse
  than none.
* **A partial map-name table.** Only the maps the early game passes through are
  named; everything else reports its number rather than a guess.
* **Navigating a *town* is much weaker than navigating a room.** `goto` is
  greedy with axis alternation, which is enough to get round a bed but not
  round a building: in Pallet Town the agent reached the right column and then
  spent several calls being told "walls in the way" by the fence between it and
  Oak's lab. This is the same gap as the point above — it is what a walkability
  grid would fix.
* **Red/Blue only.** Yellow is the same structures at different addresses (a
  table swap); Gen 2 needs a second decoder. Anything else is refused with an
  explanation rather than shown wrong.
* **Battles are observable but not thought about.** The state carries the
  opponent and the party; nothing here knows type effectiveness, and the demo
  target deliberately stops before Brock.
* **The API adapter is untested against the live API on this box** (no key).
  The CLI adapter is what the recorded run used.
* **For a real broadcast, still missing:** a "starting soon" / "be right back"
  scene, an overlay for donations or chat, any chat interaction at all, and a
  supervisor that restarts the emulator if it dies. None of that is emulator
  work.
* **The OBS recipe above is documented, not photographed.** The layout is
  asserted by rendering the real frontend at the relevant cell grids
  (`the_stream_layout_shows_goal_activity_and_game_together` writes the grid to
  a file with `GB_DUMP_LAYOUT`), but no screenshot of an actual OBS scene was
  taken: the X display on this machine was in use by another crew's live
  frontend work and taking it over risked disrupting them. **A human should do
  one visual pass before going live**, following the checks in §6.

---

## 9. Conformance-tool notes

Per the standing convention in [`AGENTS.md`](../AGENTS.md), the defects fixed
here recorded as candidate checks. All of these are **host/driver** defects
rather than emulation ones, so point 3 cites the contract violated rather than
Pan Docs — and point 2 has the same answer for every one of them, which is
itself the finding: **no test ROM catches any of this.** A ROM-driven suite
compares framebuffers, and every one of these leaves the framebuffer perfectly
correct.

1. **A press reported as a wall on the first unchanged coordinate.**
   *Symptom* — the agent is told every direction is blocked and gives up on a
   room it can walk around freely.
   *Caught by* — nothing before; now `agent.rs` case
   "walking into a wall stops there and moving away still works".
   *Contract* — Gen 1 turns the player in place when a direction they are not
   facing is pressed (`pret/pokered`, `PlayerStep`), and ignores input entirely
   while `wJoyIgnore` is set, so one unchanged reading is not evidence of a wall.
   *Candidate check* — **a movement primitive must distinguish "turned" from
   "blocked".**

2. **`advance_text` returning immediately when the screen is momentarily still.**
   *Symptom* — four consecutive model calls achieve nothing during a cutscene.
   *Caught by* — nothing; the harness now exercises the primed press.
   *Candidate check* — **an "advance dialogue" primitive must make progress in
   the absence of a ▼ prompt.**

3. **A driver failure counted with no call in flight.**
   *Symptom* — the circuit breaker trips within milliseconds and the agent is
   switched off, reported to the operator as "the agent is broken".
   *Caught by* — `a_failure_with_no_call_outstanding_is_ignored`.
   *Candidate check* — **a breaker must count failures of calls it started.**

4. **A bounded history buffer read as a monotonic counter.**
   *Symptom* — the run log stops printing new commentary after the first 40
   lines while the run continues perfectly well; a human watching concludes the
   run has hung. Cost 28,000 frames of a real run before it was noticed.
   *Contract* — a ring buffer's length is not a sequence number.
   *Candidate check* — **any "show me what is new" display must key on a
   monotonic counter.**

5. **The Status box height hard-coded as "eleven rows plus the plugins".**
   *Symptom* — an attached plugin leaves no trace on screen, defeating the
   promise in [`plugins.md`](plugins.md) §4 that a plugin cannot be active and
   invisible. This is the **third** time this exact bug has been fixed here: the
   box was resized to its contents once, and then the next crew added an `Audio`
   row and silently re-clipped the last plugin. It is now measured from the lines
   it is actually given.
   *Caught by* — `an_attached_plugin_is_visible_in_the_rendered_status_panel`,
   which was **red on `main`** when this work started.
   *Candidate check* — **a panel's height must be derived from its content.**

6. **No public suite covers any of this**, the same gap
   [`battery.rs`](../testharness/battery.rs) records for persistence. Blargg,
   Mooneye and acid2 run one process, look at a framebuffer, and never press a
   button. An emulator's *input path* — that a scheduled press is held for the
   frame the game samples, and released before the next — is unverified by every
   public test ROM. **That is a real candidate for the conformance tool**: a ROM
   that reports which frames it saw each button on would test something nothing
   currently does.

---

## 10. The recorded run

Pokémon Blue, **from power-on with no save**, driven by Claude Haiku through
`tools/claude_plays/claude_cli_adapter.py`. Reproduce with:

```sh
cargo run --release --example claude_plays -- \
  --rom ~/roms/blue.gb \
  --objective "Get out of the bedroom, go downstairs, leave the house, and \
               choose a starter Pokemon from Professor Oak" \
  --max-calls 130 --max-usd 4 --timeout 150 --wall 7200 \
  --pace realtime --stop-on 'party>=1' \
  --shots target/run --log target/run/agent.ndjson --report \
  --driver command --command tools/claude_plays/claude_cli_adapter.py
```

`--pace realtime` runs the console at its true 59.7275 Hz, so what is recorded
is what a viewer would have seen. The screenshots become a watchable timelapse
with `tools/claude_plays/timelapse.sh <run-dir>`.

### Two runs, and the fix between them

The first run is written up below because its *failure* is the useful part.
After the navigation fix it produced, a second run went further: the table in
[§Second run](#second-run-after-the-navigation-fix) is the one to read for what
the substrate does now.

### What it did (first run)

| decision | where | what it said |
|---:|---|---|
| 1 | title screen | *"Let's get this adventure started! Powering through the title screen and Oak's opening now."* |
| 4 | (cutscene) | *"Powering through the opening cutscene. We're heading out of the bedroom toward our first encounter with Professor Oak."* |
| 11 | **bedroom** | *"We're in the bedroom now — time to head down to the first floor via that staircase!"* |
| 12 | **Red's house 1F** | *"I'm standing on the staircase — stepping down to go to the main floor and leave the house to find Professor Oak."* |
| 14 | **Pallet Town** | *"Heading up through Pallet Town to find Professor Oak's lab."* |
| 20 | Pallet Town | *"Direct path to the lab is blocked—let me navigate around it by going left first."* |

It handled, unaided and from a cold start: the title screen, the `NEW GAME`
menu, the whole of Professor Oak's introduction, **both naming screens**
(including pressing START rather than typing `AAAAA` for ever), the staircase
out of the bedroom, and the front door of the house. Three of the four
milestones in the demo target.

### The numbers

| | |
|---|---|
| model calls | 23, **0 failed** |
| emulated frames | 45,020 — **1,957 frames per call** (~33 s of game time each) |
| call latency | 8 s min, 23 s median, 55 s max |
| cost | **$0.52**, 419k tokens (of which most is the CLI's own system prompt, re-sent every call) |
| emulated frame rate | **60 fps, held throughout**, including while the agent was thinking |

The per-call latency is the `claude` CLI's, not the substrate's: it starts a
fresh coding-agent session every time. `claude_api_adapter.py` sends ~1.5 KB of
prompt instead of ~18 KB and should be several times faster and cheaper — not
measured here, because this machine has no `ANTHROPIC_API_KEY`.

### Where it stopped, and why

**In Pallet Town, a few tiles from Oak's lab.** It read the lab's door out of
the warp table correctly — `{"x":12,"y":11,"to":"Oak's Lab"}` — reached the
right row, and then could not get round the fence between it and the door. It
said so accurately (*"Direct path to the lab is blocked—let me navigate around
it by going left first"*) and kept trying, which is the honest failure: the
agent's reasoning was right and the **navigation primitive** was not good
enough.

That produced a real fix rather than an excuse. `goto` was greedy with
single-step axis alternation, which gets round a bed and cannot get round a
building: it steps one tile aside, finds the direct route still blocked, steps
back, and repeats. It now **commits to a four-step run** along the other axis
before retrying, alternating the side each time. See
[`GOTO_DETOUR`](../plugins/agent/src/action.rs).

The fix is pinned by a CI case rather than by the next expensive run: the
synthetic cartridge grew a wall down one column with a single doorway three rows
away, and `goto routes around a wall instead of giving up in front of it`
requires the primitive to find it. Teeth proved by shortening the detour back to
one step — the case fails, ending in front of the wall at (3,6).

Two more corrections came out of watching this run and the one before it, and
both were errors in what the agent was *told* rather than in what it decided —
which is the useful kind to find:

* **map size was reported in the wrong unit.** Gen 1 counts a map's width and
  height in 32×32 blocks and the player's coordinates in 16×16 tiles, two per
  block, so Pallet Town was described as 10 wide when its `x` runs 0..19. The
  agent was being told it was standing outside the map.
* **the opening cutscene looked like ordinary play.** `wCurMap` is already the
  bedroom during Oak's speech, with plausible coordinates, so "walk down"
  failing looked like a wall rather than like not being in control.
  `wCurMapHeight`/`wCurMapWidth` are `0` until the header loads, which is now
  reported as `mode: "cutscene"`.

### Second run, after the navigation fix

Same command, same model, same cold start. With `goto` committing to a sideways
run, and with the map size and the cutscene mode reported correctly, it went
**all the way to the starter-selection screen**:

| decision | where | what happened |
|---:|---|---|
| 3 | **bedroom** | through the title, `NEW GAME`, Oak's speech and both naming screens |
| 13 | **Red's house 1F** | down the staircase |
| ~20 | **Pallet Town** | out of the front door |
| 22 | Pallet Town **(10,1)** | reached the town's north edge, which is what triggers Oak — *"Oak's calling me! Let's head to his lab to pick our starter."* |
| 23 | **Oak's Lab** | escorted there by the game's own script |
| 30 | Oak's Lab | read through Oak's speech to the screen that says **"which POKéMON do you want?"** |

| | |
|---|---|
| model calls | 34, 1 failed (a CLI timeout, retried) |
| emulated frames | 73,111 — **2,150 per call** |
| call latency | 6 s min, 20 s median, 120 s max |
| cost | **$0.68** |
| emulated frame rate | **60 fps throughout** |

**What it did not do is press the last button.** It reached the starter menu and
then spent its remaining calls circling inside the lab without finding the
table, while the `claude` CLI degraded to minutes per call under rate limiting;
the session was stopped rather than left running unattended. So: **all four
locations in the demo target were reached unaided, and the final selection is
unconfirmed.**

That last step is the *same* limitation as the Pallet Town one, indoors: the
agent knows where it is and cannot see what is in the room, and a Poké Ball on a
table is not a warp, so nothing in the state points at it. It is the strongest
argument for the missing piece in [§8](#8-what-is-missing) — the agent needs to
be able to see, or to be told, what is in the room with it.

Note what the agent worked out for itself, because it is the part a script could
not: **the lab is not the route to a starter.** Walking straight to Oak's Lab
reaches an empty room (verified separately with the deterministic driver: from
power-on into Oak's Lab in 9,034 emulated frames and 2.1 s of wall clock, party
still 0, nothing to interact with). The game gates the starter behind trying to
*leave town to the north*, and the agent found that route.

### The recording

`target/<run>/timelapse.mp4` — the run's periodic screenshots at 12 fps, about
250× real time. `target/<run>/agent.ndjson` is the full decision stream:
request, reply, cost and error for every call.

**Neither is committed.** A timelapse is mostly game frames, and this
repository's licence boundary allows screenshots of *our own interface* with a
game incidental in them (as `docs/link-trade/` has), not a gallery of the game
itself. The panel render in §2.3 is committed because it is our chrome and
nothing else.

## 11. The third session — watched live, stopped on purpose

Two runs from power-on with no save, **in the TUI in a visible window with the
maintainer watching** — which is itself why this session produced more fixes than
the first two: a watching human reads stalls, wasted presses and blindness off
the screen faster than any log.

**Run 1 — Claude Haiku, the shipped configuration.** Through the title,
`NEW GAME`, Oak's speech, **both naming screens**, down the stairs, out of the
house, wandered into the empty lab, left it, **found the north-edge trigger**
(Oak's escort), and was walked to the lab with the starter selection live. It
then circled the room — at one point standing at (5,3), one tile left of the
first Poké Ball, being told "but you can have one! Choose!" — until stopped.
33 decisions (3 host-timeout failures), reached frame 59,183 (~16.5 min of
game time), **$0.58 reported**, call latency median 14 s / max 60 s (the 60 s
ones were the TUI's own too-tight timeout, fixed below).

**Run 2 — Claude Sonnet, after the fixes.** Same route in a third of the game
time: overworld by frame ~6,800, lab at frame ~12,700, at the table when the
maintainer called the demo. 25 decisions, **0 timeouts**, median 5.1 s / max
8.7 s per call, **$1.22 reported**. Sonnet's play was visibly sharper (it
announced START on the naming screen unprompted, with the enforced rule as a
backstop) — and it was exactly as blind at the table, because the blindness is
observational, not a model property.

**Host-side facts this session produced** (each enforced or fixed, not just
written down): `advance_text` refuses the naming keyboard — the A's typed on
screen came from the *host's* text loop, not the model; the TUI's per-call
timeout was 60 s against an adapter that legitimately needs up to 150 s;
`GB_AGENT_WALL` now sets the one budget ceiling that is not self-reported; and
the sprite-table observation was built, measured inert on the live game, and
withdrawn (see §8 — the layout is right, the consistency gate was wrong).

### Where the agent was flying blind — the list that feeds the screen-text work

Every entry names something a human at the same screen used instantly and the
agent could not, with the cause. `screen.text` itself worked throughout — it
read `YOUR NAME?`, menus and Oak's dialogue correctly. The gaps are everything
that is **not** background-tilemap text:

1. **Sprites are invisible, and the Poké Balls are sprites.** `screen.text`
   decodes the BG/window tilemap; people and objects are OAM sprites, drawn
   over it. So the three balls at (6,3)-(8,3) in Oak's Lab, Oak himself, Mom,
   the rival — none exist in the state. This ended both runs of the previous
   session and both of this one. It is *the* blocker.
2. **The naming keyboard's cursor column is unknown.** `menu_cursor_row` is
   row-only. On the letter grid the agent cannot know which letter the cursor
   sits on, so it cannot type a chosen name — it types blind and confirms.
   (A name of A's or J's is playable; the point is it *cannot do better*.)
3. **A menu's selected item is not decoded.** The `▶` cursor's row is
   reported, but not what text sits on that row, so "is NEW GAME or OPTION
   selected?" takes model inference over raw rows rather than being a fact.
4. **Mid-redraw screens are fed as if they were stable.** Run 1's decision 22
   got `screen.text` of `KU:N:` — a half-drawn transition — and spent a call
   reasoning about noise. Nothing marks a screen as still settling; a
   "same text on two consecutive reads" gate before snapshotting would do.
5. **Furniture and terrain exist only as `blocked` outcomes.** The lab's
   table, Pallet Town's fences: the agent discovers each wall by walking into
   it, one failed plan at a time. The tileset collision data could say it up
   front (§8's walkability grid).
6. **A YES/NO box looks like flowing text.** `advance_text` presses A through
   everything, which answers any confirmation prompt with its default before
   the model ever sees the question. It happened to be harmless this session;
   it will not always be.

## 12. The screen as a first-class output — what it already was, and what it is now

The maintainer, after watching §11: *"we need the game to output its screen text
like it does other stats directly so ai can get a better handle upon whats
happening."*

### What already existed (measure the starting point before building)

**The channel was already there and already worked.** `Screen::read` has read
the live tilemap out of VRAM — through the window layer, which is the trap that
makes every menu look empty if you read the background map — since the agent
plugin's first commit, `Screen::compact_text` has been a `screen.text` field in
every snapshot, and §11's own list opens by saying so: *"`screen.text` itself
worked throughout — it read `YOUR NAME?`, menus and Oak's dialogue correctly."*
Nothing here is a new pipe. **The defects were all in what the decode omitted,
what it invented, and what it never admitted it could not see.**

### The four blind spots closed, and the two that are not

Against §11's list, in its own numbering:

| # | blind spot | now |
|---|---|---|
| 1 | sprites are invisible, and the Poké Balls are sprites | **open** — not a decoding problem: sprites are OAM, drawn over the tilemap, so no tilemap decode can ever reach them. The prompt now *says* so, so an empty screen no longer implies an empty room |
| 2 | the naming keyboard's cursor column is unknown | **closed** — `screen.menu_cursor_col`, and `screen.selected` names the single letter A would type |
| 3 | a menu's selected item is not decoded | **closed** — `screen.selected` |
| 4 | mid-redraw screens are fed as if they were stable | **closed** — the snapshot waits for the tilemap to hold still, and `screen.trust` reports `still-redrawing` with the text withheld if it never does |
| 5 | furniture and terrain exist only as `blocked` outcomes | **open** — collision data, §8's walkability grid |
| 6 | a YES/NO box looks like flowing text | **closed** — `screen.kind` is `yes-no` and `advance_text` refuses to press A there |

### Three decode defects a real cartridge found that no list had

Each was found by running `examples/screen_text` against Pokémon Blue and
*reading the output*, and each was silent — the field was populated, confident
and wrong.

1. **The battle menu's `PKMN` option was invisible.** "PKMN" is the two ligature
   tiles `<PK><MN>` (`$E1 $E2`, `pret/pokered` `constants/charmap.asm`) and both
   decoded to spaces, so the menu read `FIGHT / ITEM RUN`. A model told that has
   no way to know switching Pokémon exists. Contractions (`'s`, `'t`, …) and the
   battle HUD's `<LV>` glyph were dropped the same way.
2. **The two-column battle menu named an item the player does not have.**
   `FIGHT` ends at column 14 and `PKMN` starts at 16, so exactly **one** blank
   separates the columns — while `NEW GAME` contains one. Splitting on a
   two-blank gap answered `"FIGHT PKMN"`. The rule that separates them is that a
   menu's columns line up across *its own rows*: see `Screen::selected`. Taking
   column starts from every row instead truncated `NEW NAME` to `NEW`, because
   the question printed under that list begins a word in the same column.
3. **Quote glyphs were HP-bar graphics.** `charmap.asm` lists `$70`-`$75` twice —
   once as quotes in the main font, once under
   *"Actual characters (from `gfx/font/font_battle_extra.png`)"* — and a tile id
   cannot tell them apart. Decoding them as quotes put `”’` on a line of its own
   under the opponent's name and `153/153”` on the HP row of every battle. They
   are blank now. `$F0` is the same problem with an answer: `¥` normally, the
   `ED` key on the naming screen, which announces itself well enough to choose.

### The evidence: fourteen real screens

`cargo run --release --features plugin-agent --example screen_text -- --rom
<blue.gb> [--save <blue.sav>] (--tour | --menu-tour | --stability | --state
<gbss>)` drives a real cartridge and prints the raw 20x18 grid, the compacted
text a model is sent, and every signal beside it. Measured on Pokémon Blue:

| screen | `kind` | `trust` | `selected` |
|---|---|---|---|
| power-on, blank | `no-text` | `ok` | — |
| Game Freak intro artwork | `text` | **`graphics-not-text`** | — |
| main menu (new game) | `menu` | `ok` | `NEW GAME` |
| main menu (with a save) | `menu` | `ok` | `CONTINUE` |
| continue confirmation | `menu` | `ok` | `CONTINUE` |
| Oak's speech | `dialogue` | `ok` | — |
| name prompt (preset list) | `menu` | `ok` | `NEW NAME` |
| naming keyboard | `naming-keyboard` | `ok` | `A` |
| overworld | `no-text` | `ok` | — |
| START menu | `menu` | `ok` | `POKéDEX` |
| nurse dialogue | `dialogue` | `ok` | — |
| battle announcement | `text` | `ok` | — |
| battle menu | `menu` | `ok` | `FIGHT` |
| move list | `menu` | `ok` | `FLAMETHROWER` |

Two of those rows are the honest ones. The overworld and the title screen are
drawn from map and artwork tiles, so **nothing on them is text** — `kind:
no-text` makes an empty `screen.text` a stated fact rather than something that
looks like a failed read. The intro decodes as `ABCDEFGHIJKLMNOP` (sequential
tiles through the font range), and that text is **withheld**, not sent.

### The settling gate is bounded by measurement, not by hope

Withholding text while the screen is still moving is only safe if screens
actually stop moving. `--stability` samples the tilemap hash every frame on a
real cartridge: the main menu, the overworld and the START menu are **identical
for all 240 frames**; a dialogue box waiting on the blinking `▼` changes 6 times
in 240 frames, and its **longest identical run is 42 frames** — against a
4-frame gate. The blink cannot starve the gate, and the wait is capped at 120
extra frames regardless, after which the text is withheld and `trust` says
`still-redrawing` rather than the run stalling.
