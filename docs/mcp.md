# MCP server — drive the emulator from Claude Desktop, Claude Code, or anything else

This repo ships `terminalgb-mcp`, a [Model Context Protocol](https://modelcontextprotocol.io)
server. Point any MCP client at it and it can load a cartridge, press buttons, read the
game's state and take screenshots.

You do not need to have seen this repo before. Everything you need is below.

---

## 1. Build it

```sh
git clone https://github.com/Alchemy86/TerminalGB
cd TerminalGB
cargo build --release --no-default-features --features mcp
```

That produces `target/release/terminalgb-mcp`. Copy the absolute path — you will paste it into
a config file in a moment.

`--no-default-features` is deliberate: it leaves out the desktop window and the terminal
frontend, which the server never uses. The result needs **no system libraries at all** — no
display server, no ALSA, no `$HOME`. If you would rather have one binary that can also open
a window, drop the flag and use `--features mcp` on its own.

You need a **cartridge image** too. The repo ships one MIT-licensed test ROM you can point
it at right away, `third_party/dmg-acid2/dmg-acid2.gb`; it draws a picture and nothing else.
For a real game, supply your own `.gb`/`.gbc` file. The structured-state tools understand
**Pokémon Red and Blue (English)** specifically; every other cartridge still works through
`screenshot`, `read_memory`, `press_buttons` and `step`.

---

## 2. Install it in Claude Desktop

Edit the config file:

| OS | path |
|---|---|
| macOS | `~/Library/Application Support/Claude/claude_desktop_config.json` |
| Windows | `%APPDATA%\Claude\claude_desktop_config.json` |
| Linux | `~/.config/Claude/claude_desktop_config.json` |

```json
{
  "mcpServers": {
    "terminalgb": {
      "command": "/absolute/path/to/gameboy/target/release/terminalgb-mcp",
      "args": [
        "--rom", "/absolute/path/to/your/game.gb"
      ]
    }
  }
}
```

Restart Claude Desktop. The tools appear under a `terminalgb` server.

**Use absolute paths.** Claude Desktop launches the server with an unpredictable working
directory, so a relative `--rom` will not be found. (`~` *is* expanded by the `load_rom`
tool, but the shell is not involved in `args`, so spell it out here.)

**Leave `--rom` out** if you would rather choose the game from inside the conversation —
then the first thing you say is *"load the ROM at …"* and the `load_rom` tool does it.

---

## 3. Install it in Claude Code

One command, from anywhere:

```sh
claude mcp add terminalgb -- /absolute/path/to/gameboy/target/release/terminalgb-mcp \
  --rom /absolute/path/to/your/game.gb
```

Or, if you prefer a file you can commit next to a project, write `mcp.json`:

```json
{
  "mcpServers": {
    "terminalgb": {
      "command": "/absolute/path/to/gameboy/target/release/terminalgb-mcp",
      "args": ["--rom", "/absolute/path/to/your/game.gb"]
    }
  }
}
```

and start a session with it:

```sh
claude --mcp-config mcp.json
```

Check it is connected with `/mcp` inside the session.

### Trying it without a session

The one-shot form is the quickest way to see it work, and is exactly how the server in this
repo was verified:

```sh
claude -p --mcp-config mcp.json \
  --allowed-tools "mcp__terminalgb__observe,mcp__terminalgb__step,mcp__terminalgb__press_buttons,mcp__terminalgb__screenshot" \
  'Step 1200 frames, press START, step 700 more, then take a screenshot and tell me what is on screen.'
```

---

## 4. The tools

Eleven, plus one that only exists when you ask for it. Each answers a different question,
which is the whole design rule: a tool a model cannot tell when to use costs tokens in every
request and invites the wrong call.

| tool | what it is for |
|---|---|
| `load_rom` | Load a cartridge from a path and start it from cold. |
| `reset` | Reboot the current cartridge. Battery saves survive. |
| `observe` | **Start here.** The game's state as structured data: what it is doing (title / overworld / dialogue / menu / battle / cutscene), the **screen** block (below), whether it is waiting for a button — and for Pokémon Red/Blue the map, coordinates, facing, exits, party, badges, money and battle opponent. A few hundred bytes, and the only way to tell whether an action worked. |
| `screenshot` | The current frame as a PNG. The fallback for when there is no field or address for what you need. Does not advance time. |
| `read_memory` | Bytes of the Game Boy's address space, read exactly as the CPU sees them and **without perturbing timing**. This is what makes an agent fast instead of squinting at pixels. |
| `press_buttons` | Press buttons with exact frame timing, running the machine until the press has finished. Works on any cartridge. |
| `run_actions` | Higher-level Pokémon Red/Blue primitives — `walk`, `goto`, `advance_text`, `wait`, `press` — that one call carries out over hundreds of frames, reporting what each achieved. |
| `step` | Advance N frames and do nothing else. ~60 frames is one second. |
| `save_state` / `load_state` | Snapshot and restore the whole machine, to a numbered slot or a named file. |
| `render_audio` | Render Game Boy sound-chip instructions to a WAV through the emulated APU — notes, a register script, or a `.vgm` log. The one tool here that needs **no cartridge**. See [`docs/chiptune.md`](chiptune.md). |
| `write_memory` | Only when the server was started with `--allow-write-memory`. Otherwise it is not even listed. |

### The `screen` block, and when not to believe it

Gen 1 loads its font so a tile id **is** a character code, which is why the screen reads
back as text at all. `observe` returns it as:

| field | what it promises |
|---|---|
| `text` | the visible text, blank lines dropped, indentation kept down to the common margin so a two-column menu still reads as two columns |
| `kind` | what shape of screen it is, and so what `A` does: `no-text`, `text`, `dialogue`, `menu`, `yes-no`, `naming-keyboard` |
| `selected` | the item the `▶` cursor points at — on the naming keyboard, the single letter `A` would type |
| `trust` | `ok`, or the reason the text was **withheld**: `graphics-not-text`, `still-redrawing`, `screen-off` |
| `waiting_for_input` | the `▼` "press A" arrow is showing |
| `menu_cursor_row` / `menu_cursor_col` | where the cursor is |

Two of those need saying plainly.

**`trust` is not advisory.** When it is anything but `ok`, `text` is the empty string and a
`note` says why — a decode of artwork whose tiles happen to fall in the font range is
alphabet salad delivered with a straight face, and salad in the same field as good data gets
the same trust. Use `screenshot`, or `step` a few frames and observe again. (This server
answers one call at a time and does not drive the clock between reads, so its `trust` is the
single-read verdict; the agent plugin's snapshot additionally reports `still-redrawing`.)

**Sprites are not in `screen.text`, at any trust level.** People, Poké Balls and every other
object are OAM sprites drawn *over* the tilemap. `kind: no-text` on the overworld means
there is no text on that screen — it does not mean the room is empty.

### Why `run_actions` matters

A round trip to a model takes seconds; the Game Boy runs at 59.7275 frames a second. An
agent reasoning about individual frames spends its entire budget on nothing. So the unit of
agency is an *intention* that cheap local code carries out:

```json
{"actions": [
  {"do": "advance_text"},
  {"do": "goto", "x": 4, "y": 7},
  {"do": "walk", "dir": "down", "steps": 2}
]}
```

`advance_text` holds a whole conversation down — one call instead of thirty. `goto` walks
toward a coordinate and goes *round* what blocks it. Every action is bounded (there is no
"repeat until it works") and reports honestly:

```
head for (4,7) — arrived at (4,7) after 11 step(s)
walk down 2 — walked 2 step(s) to (5,5)
331 frames run; now at frame 331.
```

This is not new code. It is [`plugins/agent/src/action.rs`](../plugins/agent/src/action.rs), written so an
agent could play the game; the MCP server is a wrapper over it. Same for perception:
`observe` is [`plugins/agent/src/observe.rs`](../plugins/agent/src/observe.rs). See
[`docs/gen1/agent-play.md`](gen1/agent-play.md) for the reasoning behind each primitive.

### Why `render_audio` reports rather than returns audio

A model cannot listen to a WAV, so returning one would be a tool whose output is opaque to
its only caller. The reply is the feedback loop instead: length, peak level, RMS, which
channels sounded, and — with `"registers": true` — the compiled register writes, which is
the ground truth the audio came from. `Peak 0.000` comes with a warning naming the three
usual causes (APU not powered on, NR51 not routing the channel, the channel's DAC off).
The file path is for a person.

It composes with everything else here for free because it shares the chip: the same
`gameboy::sound::Sound` that plays a cartridge renders the phrase, so what a model hears
about is what the hardware would do. It needs no cartridge and no session — it works on a
server that has never been given a ROM.

### What `write_memory` is not

It is off by default, and the tool is **filtered out of the listing** rather than refusing
when called — a client is never shown a tool that will say no. The reason is the same one
the agent plugin gives: something that can poke RAM is editing the game rather than playing
it, and the save it leaves behind is a corrupted one. With `--allow-write-memory` the server
becomes a debugger, which is a deliberate and different thing.

---

## 5. Configuration — the flags that matter

Everything optional is **off**, because the normal client is a model: it wants the machine
as fast as the host can manage and it never looks at a window or listens to a speaker.

```
CARTRIDGE
  --rom <PATH>            Load this cartridge at startup.
  --model auto|dmg|cgb    Console. Default auto: read the cartridge's CGB flag.
  --no-battery            Run with NO battery: the .sav is neither loaded nor written.
  --state-dir <DIR>       Where save-state slots live.

SPEED
  --speed uncapped|realtime|<N>x   Default uncapped.
  --max-frames <N>                 Ceiling per tool call. Default 60000.

OUTPUT (all off by default)
  --audio                 Attach a host sound device (needs the `audio` feature).
  --video-out <PATH>      Stream raw frames to a file or FIFO so a human can watch.
  --video-format rgba|rgb|yuyv
  --screenshot-scale <N>  Upscale the screenshot tool's PNG (1-6).

PLUGINS (none attached unless named)
  --plugins trainer,gen1-link,gen1-battle

DEBUGGING
  --allow-write-memory    Expose the write_memory tool.
  --log <PATH>            NDJSON log of every request and reply.
```

### The combinations worth knowing

**The fast one — the default, and what an agent should use.** Nothing attached, nothing
paced:

```sh
terminalgb-mcp --rom game.gb
```

**A human is watching.** Pace to real time and open a video stream. `mpv` in another
terminal, or `ffmpeg` into a file:

```sh
mkfifo /tmp/gb.raw
terminalgb-mcp --rom game.gb --speed realtime --video-out /tmp/gb.raw
# elsewhere:
ffplay -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 -i /tmp/gb.raw
```

*Note:* opening a **FIFO** without blocking needs the `libc` feature, so build with
`--features mcp,libc` (or just `--features mcp,tui`, which includes it) if you want that.
A plain file destination works in every build. See [`docs/video-out.md`](video-out.md).

**Sound.** `--audio` only makes sense with `--speed realtime` or slower. The APU emits
samples at a fixed rate against *emulated* time, so at 70x real time the audio queue is
overrun and the oldest samples are discarded — the documented fast-forward policy, but not
something anyone wants to listen to.

```sh
terminalgb-mcp --rom game.gb --speed realtime --audio
```

**Protecting a real save.** `--no-battery` removes the battery *entirely* — the existing
`.sav` is not loaded either, so the game starts fresh, which is usually not what someone
reaching for the flag wants. To play an existing save without risking the file, copy the ROM
**and its `.sav`** into a scratch directory and point `--rom` at the copy. That is the
standing rule for every harness in this repo, and it exists because a save was destroyed
once already.

```sh
mkdir -p /tmp/gb && cp ~/roms/blue.gb ~/roms/blue.sav /tmp/gb/
terminalgb-mcp --rom /tmp/gb/blue.gb
```

**Debugging a game's RAM.**

```sh
terminalgb-mcp --rom game.gb --allow-write-memory --log /tmp/gb-mcp.ndjson
```

### There is deliberately no `--tui`

The MCP transport **is** stdin/stdout. The terminal frontend owns both, so running them
together would interleave escape sequences with JSON-RPC frames and end the session. A human
who wants to watch gets `--video-out`, which is a real stream into `ffmpeg`, `mpv` or a
virtual camera and costs +0.007 ms/frame at native size. Saying so is better than shipping a
flag that corrupts the protocol.

---

## 6. What it actually achieves — measured

Measured on this machine (Linux, release build) by timing a `step` of 20,000–60,000 frames
through the protocol itself, on a real Pokémon Blue save in the overworld. Real time is
59.7275 fps, so 16.743 ms/frame:

| configuration | ms/frame | vs real time |
|---|---:|---:|
| **headless, no audio, uncapped** (the default) | **0.238** | **70x** |
| headless, `--plugins trainer` | 0.239 | 70x |
| `--video-out` to a stream | 0.238 | 70x |
| `--audio` on, uncapped | 0.264 | 64x |
| `--speed realtime` | 16.74 | 1.0x (by definition) |
| `--speed 4x` | 4.19 | 4.0x |

Reproduce any row:

```sh
time (printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"step","arguments":{"frames":20000}}}' \
  | ./target/release/terminalgb-mcp --rom game.gb --no-battery > /dev/null)
```

**A correction worth recording:** the brief for this work quoted host audio at ~8 ms/frame
against ~0.29 without. That figure is **stale**. It predates the audio crew's fix for a
player that *blocked* inside `Cpu::do_cycle` (`docs/audio.md` §4c); with the current
non-blocking hand-off the measured cost here is 0.264 vs 0.238 ms — about 11%, not 27x.
Audio is still off by default, but the honest reason is that nobody is listening at 70x
speed, not that it is ruinous.

---

## 7. How it is built, and where it is not

### It is a thin wrapper, on purpose

The hard half already existed. [`plugins/agent/`](../plugins/agent/) contains structured perception
(`observe.rs`), a bounded action executor (`action.rs`) and a dependency-free JSON codec
(`json.rs`), all written so an AI agent could play the game. `src/mcp/` re-implements none
of it: `observe` calls `Gen1Observer`, `run_actions` drives `Executor`, the wire format is
`agent::json`. If a change here starts re-deriving game state or re-timing button presses,
it is in the wrong file.

### It is a frontend, not a plugin

A [plugin](plugins.md) is handed a `Machine` *during a frame the emulator has already decided
to run*. That is the wrong shape here in three ways, and each one is a tool a client needs:

1. **It must drive the clock, not ride it.** `step`, `press_buttons` and `run_actions` decide
   how many frames run and when the call returns.
2. **Half the surface is above the machine.** `load_rom` replaces the cartridge, `reset`
   rebuilds it, `load_state` swaps the machine's whole contents. A `Machine` handle reaches
   memory and buttons — correctly — and none of those three is expressible through it.
3. **It owns host resources.** stdio, a sound device, a video sink. That is where
   `screen::tui` sits, not where a plugin sits.

So it *uses* the plugin contract rather than joining it. Every read and every press goes
through `Gameboy::with_machine` with an explicit
[`Capability`](../crates/terminalgb-plugin-api/src/capability.rs) grant, which is how `write_memory` can be *provably*
absent unless the operator asked for it — the same enforcement a plugin gets, applied to the
server's own tool handlers. Plugins can still be attached with `--plugins`.

**The agent plugin deliberately cannot be attached.** An MCP client already *is* the agent;
two of them would share one controller, each undoing the other's plan. Asking for it returns
that explanation rather than a silent no-op.

### It is optional

The whole module is behind the `mcp` cargo feature and adds **no third-party crate** — JSON
is `agent::json`, PNG and base64 are `termimage`. Someone embedding this core in their own
tool or in hardware must not inherit an MCP stack they never asked for. `cargo tree
--no-default-features --features mcp` still shows one dependency, `blip_buf`.

---

## 8. Verifying it

```sh
make test-mcp        # cargo test --release --features mcp --test mcp
make lint-mcp        # clippy over the feature, which plain `make lint` never compiles
```

[`testharness/mcp.rs`](../testharness/mcp.rs) spawns the **real binary** and speaks JSON-RPC
down its stdin, because a server that starts is not a server that works. It checks the
handshake and the tool listing, that a notification is never answered, that stdout carries
protocol and nothing else, that a `--rom` is already running before the first request, that
`screenshot` returns a byte sequence which really is a PNG, that a save state round-trips,
that the frame cap is honoured *and reported*, and that garbage on stdin does not end the
session. It needs only the vendored dmg-acid2 ROM.

Two of those checks were proved to have teeth by breaking the server deliberately: dropping
the image block from `screenshot` turns it red, and making `load_state` a no-op turns the
round-trip red. The second one only became true after a fix — the first version of that
check compared `observe` output, and dmg-acid2 draws a *static* picture, so a `load_state`
that did nothing sailed through it. It now probes `$FF04` (DIV), which moves every frame.

### What was actually driven

A real MCP client — Claude Code via `claude -p --mcp-config` — connected to the server,
listed the tools, and on a real Pokémon Blue save:

* `load_state` restored a snapshot taken inside the Viridian Pokémon Center;
* `observe` returned `map_name: "Viridian Pokémon Center"`, `(11,3)` facing down, the two
  exits at `(3,7)` and `(4,7)`, ¥3000, and a party of `SPARK` (level 12) and `MEWTWO`
  (level 70);
* `read_memory` at 54110 (`$D35E`, `wCurMap`) returned `29 08 C7` — map 41, matching
  `observe`;
* `run_actions` with `walk down 1` reported *"walked 1 step(s) to (11,4)"*, and the
  follow-up `observe` independently showed `y` had gone 3 → 4;
* `run_actions` with `goto (3,7)` reported arrival after routing round the room's furniture;
* `screenshot` produced a picture the model described as the Pokémon Center lobby with a
  nurse behind the counter and potted plants either side — which is what is there.

---

## 9. Honest limitations

* **Structured game state is Pokémon Red/Blue (English) only.** The cartridge is identified
  by title *and* header checksum, so a hack, translation or randomiser with a shifted layout
  is refused with an explanation rather than reported wrongly. Yellow and Gen 2 are named as
  extensions in [`docs/trainer.md`](trainer.md), not pretended. Every other cartridge still
  works through `screenshot`, `read_memory`, `press_buttons` and `step`.
* **One cartridge per server.** `load_rom` replaces it; there is no way to run two at once.
* **The server is synchronous.** A tool call runs the emulator and returns; nothing advances
  between calls. That is what makes `observe` after `run_actions` meaningful, and it means
  `--speed realtime` only paces time *inside* a call.
* **Timing sensitivity is the game's, not ours.** `press_buttons` defaults to a 24-frame gap
  because Gen 1's `HandleMenuInput` delays several frames after accepting a press
  ([`docs/gen1/sharp-edges.md`](gen1/sharp-edges.md)). Several presses in one call still land
  faster than a screen transition, so after a transition let the screen settle with `step`
  before pressing again — that is a real property of the game, and `observe` is how you see
  it.
* **`--video-out` to a FIFO needs the `libc` feature**; a plain file works everywhere.
* **`--no-battery` means no battery, not read-only.** See §5 — copy the ROM and its `.sav`
  instead. This was found by driving the finished server with a real client and watching it
  report *"there was no save file"* on a cartridge that certainly had one.
