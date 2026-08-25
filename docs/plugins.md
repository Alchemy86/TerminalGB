# Plugins — the attach half of the extension surface

> Source: the contract is [`crates/terminalgb-plugin-api/`](../crates/terminalgb-plugin-api/),
> the plugins are [`plugins/`](../plugins/), the emulator's side is
> [`src/plugin.rs`](../src/plugin.rs) ·
> **Writing your own: [writing-a-plugin.md](writing-a-plugin.md)** · **User guide:
> [trading over the link cable](link-trade.md)** · [link battles](link-battle.md) ·
> Related: [debugging](debugging.md) ·
> [telemetry](telemetry.md) · [serial](components/serial.md) · [conformance](conformance.md)
> · [🗺️ Code map](codemap.md)

---

## 1. Why this exists, and what it is *not*

[`docs/debugging.md`](debugging.md) already designs **one** public extension surface with three
faces — **emit** (subscribe to what the machine does), **control** (drive it) and **transport**
(how a tool in another language reaches both). The render crew built the first half of *emit* as
the NDJSON [telemetry stream](telemetry.md).

This page is the other half: something that can not only *watch* the machine but **answer** it.
It is deliberately **not** a second extension model. The event names, the control verbs and the
JSON encoding are the ones that page specifies, so adding a socket later is a change of *pipe*,
not of contract.

It is also not a plugin marketplace, a scripting engine, or a dynamic-library loader. There is
no registry, no versioned ABI, no sandbox. What there is: a trait, a host that owns a list of
them, and an enforced statement of what each one is allowed to do.

## 2. What a plugin is

**An in-process Rust object implementing
[`Plugin`](../crates/terminalgb-plugin-api/src/host.rs), in its own crate.**

Each plugin is a crate under [`plugins/`](../plugins/) depending on
[`terminalgb-plugin-api`](../crates/terminalgb-plugin-api/) — which has no dependencies of its own and
**does not depend on the emulator**. That direction is load-bearing rather than tidy: this
crate's terminal frontend *draws panels* for the plugins it ships with, so `terminalgb` depends on
those plugin crates. Had the trait lived in `terminalgb`, plugins would have had to depend back on
it and Cargo would have refused the cycle. An API crate that neither side owns breaks the knot,
and the side effect is that a plugin author's dependency is ~400 dependency-free lines instead
of a whole emulator.

The seam that made it possible is `MachineAccess`: the API crate says what a machine must
provide, `src/plugin.rs` implements it for the memory unit, and `Machine` — the capability-
checked handle a plugin actually holds — is defined over the trait rather than over the MMU.

That is the answer to "in-process trait, external process over the wire, or both?", and it is a
narrower answer than "both". The honest reasoning:

- **`wasm32` has no sockets and no `dlopen`.** This crate builds for the browser, and the
  extension surface has to exist there too. An in-process trait is the only mechanism that is
  the same on every target.
- **No new dependencies.** A wire protocol worth shipping needs a server, a framing layer and
  ideally `serde`. The repo's standing rule is that new dependencies need justification, and a
  transport nobody is using yet does not clear that bar.
- **The messages are already wire-shaped.** Every [`Event`](../crates/terminalgb-plugin-api/src/lib.rs) serialises to
  one NDJSON record via `Event::write_json`, the same shape telemetry uses, and every reply a
  plugin gives is a small owned value rather than a borrowed pointer. `GB_PLUGIN_TRACE=<path>`
  already writes that stream to a file — *emit over a pipe*, with a file standing in for the
  socket.

So: **in-process today, wire-shaped by construction, socket transport is an additive
follow-up.** What is missing for a Python script to drive this is a transport and a command
decoder, not a change to the `Plugin` trait. Said plainly so nobody has to discover it: **you
cannot write a plugin in Python today.**

## 3. What a plugin may do

Five capabilities, declared up front by `Plugin::capabilities` and **enforced**, not trusted:

| Capability | Grants | Notes |
|---|---|---|
| `OBSERVE` | receive [`Event`]s, and `Machine::frame_rgba` | `Frame`, `SerialTransfer` |
| `READ_MEMORY` | `Machine::peek` / `peek_range` / `peek_bank` | does not tick the bus, so observing cannot perturb timing |
| `WRITE_MEMORY` | `Machine::poke` | **can corrupt a save** |
| `SERIAL_LINK` | `Plugin::on_serial` — *be the other Game Boy* | at most one plugin may hold it |
| `CONTROL_INPUT` | `Machine::set_button` (raw latch) and `Machine::press` / `tap` / `clear_input` (scheduled in frames) — *play the game* | see below |

### `CONTROL_INPUT`, and why there is exactly one input mechanism

A press is not instantaneous. The game samples the joypad once a frame, a menu needs frames to
settle, and holding a direction walks further than tapping it. So anything driving the game
programmatically has to schedule presses **in frames** and wait for them to complete — and every
such caller writing its own timer is how two mechanisms quietly diverge.

There is therefore one latch and two levels onto it, both under `CONTROL_INPUT`:

* `Machine::set_button(key, down)` — the raw hold/release, for a plugin that is genuinely
  thinking frame by frame (the battle-control cinematic runner is).
* `Machine::press(key, hold, gap)` — a press **scheduled in frames** through
  `crate::input::InputQueue`, owned by the MMU and advanced by `Gameboy::frame` exactly once per
  emulated frame *before* the frame runs, so a press is held for the whole of the frame the game
  samples it in. A frontend reaches the same queue through `Gameboy::queue_press` / `tap`.

**Use these rather than adding another** — an AI driver, a scripted test and a battle-control
plugin all want this, and they must all mean the same thing by "a press".

The queue is **not machine state**. It is external intent, exactly like plugin state, so it is
not serialised and `Gameboy::load_state` clears it (releasing anything held).

`Machine::input_idle()` is readable without the capability — knowing the pad is quiet is an
observation, not an action. It is also the thing that makes a scheduled press composable: a
driver waits for the pad to go quiet before reading state, rather than counting frames itself.

One measured caution from the battle-control crew, worth honouring in anything that navigates a
menu: **Gen 1's `HandleMenuInput` delays several frames after accepting a press**, so two presses
eight frames apart look from outside exactly like the emulator dropping input. Space menu presses
by around half a second — see [AtlasGB's sharp edges](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/sharp-edges.md#the-engine).

`Machine` checks the calling plugin's declared set and refuses — `peek` returns `None`, `poke`
returns `false` — rather than believing the plugin. A plugin that never declares `WRITE_MEMORY`
**cannot** write, even if its code tries. That is proved by a unit test
(`capabilities_are_enforced_not_trusted`), not asserted.

Only one plugin may drive the serial port, because a cable has two ends and the emulated console
is one of them. A second registration fails with `RegisterError::LinkAlreadyClaimed`.

## 4. Safety and honesty

- **Nothing loads implicitly.** `PluginHost` starts empty. A plugin is attached because a name
  appears in `plugins = …` in `config.toml` or in `GB_PLUGINS` — never by scanning a directory.
- **Two things a plugin can do to your game, two markers, both on the name.** `!` = may write
  memory, `!!` = has written; `>` = may press buttons, `>>` = has pressed. They sit on the
  *name* rather than in the detail text precisely because a frontend truncates the detail to fit
  a panel, and the one thing that must never be truncated away is the warning. See
  [`claude-plays.md`](claude-plays.md) for the plugin that uses the second pair.
- **The user can always see it.** The terminal frontend's Status panel gains **one `Plugin` row
  per attached plugin**, showing that plugin's own live status. A plugin holding `WRITE_MEMORY`
  is marked **`!`** ("may write your game") and one that has actually written is marked **`!!`**
  — sticky, and carried on the *name* rather than the detail text, so truncating the row to fit
  the panel can never eat the warning. Attaching (or failing to attach) also raises a toast on
  entering the game. **A plugin cannot be active and invisible.**

  This promise turned out to be false twice, in ways no unit test on the row's *content* could
  see — the Status box was a fixed height that clipped the row off entirely, and before that the
  row joined every plugin into one line and truncated it, hiding the *second* plugin (the link
  partner, the one that can actually change your game) behind an ellipsis. Both were found by
  screenshotting the real frontend. `an_attached_plugin_is_visible_in_the_rendered_status_panel`
  now asserts against **rendered output** at three window sizes; a claim about what a user can
  see has to be tested against pixels, not against a string.
- **Save states do not carry plugin state.** Plugin state is not machine state; a state file
  written with a plugin attached and loaded without one would otherwise desync silently. Loading
  a state calls `Plugin::reset`, so a link partner holding half a trade starts again rather than
  answering the next byte from a protocol position that no longer exists.
- **A failing plugin is switched off, not fatal.** Every call into a plugin runs under
  `catch_unwind`; one that panics is quarantined — never called again, marked `x` on its name,
  reported once — and the emulator and every other plugin carry on. A link partner that panics
  becomes an unplugged cable, a state the game already handles. **The honest caveat:** the
  release profile sets `panic = 'abort'`, and under abort no `catch_unwind` in Rust catches
  anything, so in the shipped release build a panicking plugin still takes the process. Real in
  dev and test; do not panic in release.
- **A plugin from another API revision is refused, not loaded.** `Plugin::api_version` defaults
  to the `API_VERSION` compiled into the plugin's copy of the contract, and `register` returns
  `RegisterError::ApiVersionMismatch` with the rebuild instruction. With build-time linking
  Cargo has already made this impossible; the check is how compatibility is *declared*, and the
  gate that becomes load-bearing if anything is ever loaded at runtime. See
  [writing-a-plugin.md §5](writing-a-plugin.md).
- **Disabled cost.** An empty host is one `is_empty` test per frame and per serial transfer.
  The two *hot* event categories `debugging.md` enumerates — instruction-retired and
  memory-access — are deliberately **not** implemented here, because that page specifies they
  must be ring-buffered and behind the `debug` cargo feature. Adding them later does not change
  any existing variant of `Event`.

## 5. Where it plugs into the core

```text
Cpu ─┬─ memory: MemoryManagementUnit ── Serial ── pending: Option<u8>
     └─ plugins: PluginHost
```

The host lives on the `Cpu`, not inside the MMU, so dispatch can hand a plugin
`&mut MemoryManagementUnit` (two disjoint fields) without a self-borrow.

When the game writes `SC = $81` (start, internal clock), `Serial::wb` offers the byte to the
existing host callback — that is the Blargg capture hook, unchanged — and if the callback
declines, parks it in `pending`. `Cpu::do_cycle` then hands it to the plugin host **before**
`MMU::do_cycle` folds `serial.interrupt` into `IF`, which is exactly where the interrupt was
raised before, so the game observes unchanged timing. An unanswered transfer is always dropped,
never answered late: that is an unplugged cable, which is what it should look like.

## 6. The three consumers the contract has to serve

A contract that can only express its own first plugin is not a plugin system. Three are in
flight, and they need different things:

| | link trade (this crew) | trainer view (another crew) | game hack (design target) |
|---|---|---|---|
| capabilities | `SERIAL_LINK` + `READ_MEMORY` | `READ_MEMORY` only | `READ_MEMORY` + `WRITE_MEMORY` |
| trigger | the game clocks a byte | every frame | *a game state being entered* |
| effect | game-visible, via a real protocol | none | game-visible, by writing memory |
| UI | a status line | **a panel** | **takes over the screen** |

The first two are served today. The third is the interesting one, so it is answered explicitly
below — supported now versus follow-up.

### 6.1 Context detection — "notice we entered the Game Corner"

**Today:** poll. A plugin with `OBSERVE + READ_MEMORY` gets a `Frame` event ~60/s and can read
the map id (or any other state) and compare. For a map change that is entirely adequate: 60
comparisons a second of one byte is free, and it is what the Gen 1 driver in
`examples/gen1_trade.rs` does to know where the player is.

**Follow-up, and it is the right one:** `debugging.md` already specifies the two mechanisms this
wants — **memory watchpoints** in `mmu::rb`/`wb` and PC breakpoints in the driver loop — with
the rule that the watchpoint hook must live behind the `debug` cargo feature so release builds
pay nothing. Two `Event` variants complete it:

```rust
Event::MemoryWrite { address: u16, value: u8, previous: u8 }
Event::Execute     { pc: u16 }
```

with subscription by address range, so the host filters before dispatching. Both are additive:
existing plugins keep compiling. **Not implemented here.** The load-bearing decision is made and
recorded, which is that the answer is a *filtered* hook rather than a firehose — "tell me when
`wCurMap` changes" is a watchpoint, not a per-frame diff, and never an unfiltered stream.

### 6.2 Conditional intervention — and how a plugin undoes what it did

A hack that is active only inside the Game Corner must stop cleanly on the way out, and "stop"
means restoring what it changed. The contract's answer is that **`Machine::poke` is not the
right primitive for a reversible change** — a raw poke has no memory of what it replaced.

The shape this wants, as a follow-up, is a scoped write:

```rust
// Returns a token; dropping it (or leaving the context) restores the old byte.
fn poke_scoped(&mut self, address: u16, value: u8) -> Option<Patch>;
```

`Machine` already reads the previous byte on the way past, so recording it is nearly free, and
the host can restore every outstanding `Patch` when the plugin says the context ended (or when
the plugin is detached, or a save state is loaded). **Not implemented here**, because no
shipping plugin writes memory yet and an unexercised API is a guess. But `poke` deliberately
returns `bool` rather than `()` so a scoped variant can be added beside it without changing the
existing signature.

The honest caveat to record now: **some changes cannot be undone.** If a hack writes coins and
the game then saves, the write is in the player's save file. Reversibility is a property of the
*window*, not a guarantee, and a plugin that writes must say so — which is what the `!!` status
marker is for.

### 6.3 Owning the UI temporarily — and who wins

Three things now want screen space: the stats overlay, the trainer view, and a hack's own panel.
Two plugins wanting the screen at once is a real design question, so here is the resolution:

**A plugin never draws. It publishes a view, and the frontend lays it out.** Concretely, the
follow-up is one method:

```rust
fn view(&self) -> Option<PluginView>;   // a title and some lines, not a Frame or a Rect
```

The frontend owns the layout and the arbitration, exactly as it already owns the arbitration
between the stats overlay and the picture. Three reasons this is the right split and not
laziness:

1. **The picture rectangle is sacred.** [`docs/terminal-render.md`](terminal-render.md) records
   a live bug where the stats overlay was drawn *over* the game image and froze it: a terminal
   image owns whole cells, so text there is not *over* the picture, it is *instead of* it, and
   for Kitty it removes the placeholders that make the picture update at all. A plugin handed a
   raw drawing surface would reintroduce that bug on its first use. There is a test
   (`the_stats_panel_never_overlaps_the_picture_at_any_size`) that exists because of it.
2. **There is more than one frontend.** Desktop GL, terminal and browser canvas cannot share a
   drawing API, but they can all render a title and some lines.
3. **Arbitration needs a single owner.** The rule: *at most one plugin view is shown at a time;
   the frontend picks, the user can cycle, and an explicit user toggle (the stats overlay)
   always outranks a plugin's request.* A plugin **asks** for the screen and may not get it,
   which is the only arrangement in which two plugins wanting it at once is not a bug.

**Not implemented here.** What exists today is `Plugin::status`, the one-line form, which the
Status panel already shows.

### 6.4 Safety for a writing plugin

Covered by §3 and §4, and it is the reason capabilities are enforced rather than advisory: the
trainer view declaring `READ_MEMORY` **cannot** write even if a future edit to it tries, and the
hack class declaring `WRITE_MEMORY` is visible in the status bar from the moment it attaches.

## 7. The first plugin: a virtual link-cable partner (Gen 1)

[`plugins/gen1-link/src/`](../plugins/gen1-link/src/) emulates the second Game Boy on the serial link so
Pokémon Red/Blue/Yellow completes a **real trade** at the Cable Club.

It declares `SERIAL_LINK + OBSERVE + READ_MEMORY` and **not** `WRITE_MEMORY`. It does not patch
the ROM, poke the party, or fabricate anything in memory: the Pokémon arrives because the game
asked for it over the wire and inserted it itself. That is what makes this a genuine test of
link trading rather than a cheat — and it is *why* the offered Pokémon has to be well formed,
because the game will not fix anything up afterwards.

### 7.1 Who drives the clock — the one decision that makes this easy

The Game Boy link is a **clocked synchronous** exchange, and meeting that clock in real time is
the hard part of every hardware or network bridge. An in-process partner does not have that
problem *if it arranges to be the slave*, and Gen 1 lets us choose.

`pret/pokered`'s [`CableClubNPC`](https://github.com/pret/pokered/tree/master/engine/link/cable_club_npc.asm)
alternates two offers in a loop: it arms an externally-clocked transfer with `$02` in `SB`
("you take the clock"), then an internally-clocked one sending `$01` ("I'll take the clock").
Its interrupt handler stores whatever it *received* as the connection status, and the constants
are chosen so the two meanings coincide:

```text
ESTABLISH_CONNECTION_WITH_INTERNAL_CLOCK = $01     USING_EXTERNAL_CLOCK = $01
ESTABLISH_CONNECTION_WITH_EXTERNAL_CLOCK = $02     USING_INTERNAL_CLOCK = $02
```

**Answering `$02` to the game's `$01` makes the game the master for the whole session.** Every
later transfer is then started by the game writing `SC = $81`, which this emulator services
synchronously — so the partner answers inside that write and has **no timing budget to miss**.
No tolerance, no buffering, no clock recovery. The failure mode the brief warned about (the game
simply hangs) is real, but it is a *protocol* risk here, not a timing one.

### 7.2 The exchange

1. **Handshake** — game sends `$01`, we answer `$02`.
2. **Nybble sync** — `Serial_SyncAndExchangeNybble` exchanges `$60 | n`, then ten `$00`s. Used
   at every rendezvous.
3. **Link menu** — `$D0 | item | (button << 2)`; `$D4` is "A on TRADE CENTER". The partner
   mirrors the player's byte, i.e. always agrees.
4. **Section exchange** — three preamble-delimited blocks:

   | block | preamble | data | total |
   |---|---|---|---|
   | random-number list | 7 | 10 | 17 |
   | party data | 6 | 418 | 424 |
   | patch list | 3 | 197 | 200 |

5. **Trade selection** — a nybble round carrying each side's party slot (`$6F` = cancel).
6. **Confirmation** — a nybble round: `1` cancel, `2` confirm.
7. **Post-trade sync**, then back to 4.

**`$6F` is the way off the trade screen, and it must be answered from every phase.** The Trade
Center has no warp events at all (`data/maps/objects/TradeCenter.asm`; `wNumberOfWarps` reads 0),
so backing out of the selection screen is the only way to reach the room's floor again — and
`.cancelMenuItem_APressed` reaches `ReturnToCableClubRoom` only when the round comes back `$F`.
(Reaching the floor is not leaving the *room*; nothing can, short of START → RESET. See
[`docs/gen1/leaving-the-cable-club.md`](gen1/leaving-the-cable-club.md).)
`wSerialExchangeNybbleSendData` is a latch, a round is 11–12 copies of the same byte, and a
wrongly-answered round is retried from whatever phase the partner has moved into, so handling
`$6F` in step 5 alone leaves the player permanently sealed in the room the moment anything
desyncs (a quick-load does, by design — see §5). `LinkPartner::feed_cancel` therefore answers it
uniformly and returns to "connected"; the sole exclusion is **inside a section exchange**, where
`$6F` is party data. Full record: [`conformance-notes.md`](conformance-notes.md) (2026-08-14).

**The re-sync byte is the easiest thing to get wrong.** `Serial_ExchangeBytes` starts each block
with `hSerialIgnoringInitialData` set and **re-sends its first byte** until it receives a
preamble byte, only then storing what follows. A block of `N` bytes therefore costs `N + 1`
transfers, and our stream must lead each block with one extra `$FD`. Total stream: **644 bytes**
(18 + 425 + 201). Off by one and the whole party block shifts and the game receives garbage.

### 7.3 Making the Pokémon well formed

`AddEnemyMonToPlayerParty` copies the 44-byte structure **verbatim** into the player's party.
Nothing is recomputed. So every field has to be right at the moment it goes on the wire:

- **Stats** are computed with the Gen 1 formula for the level and DVs. The HP DV is not stored —
  it is assembled from the low bit of the other four.
- **Experience** is the exact total for that level on that species' growth curve, or the mon
  jumps or stalls the next time it gains any.
- **Types, catch rate and base stats** come from the cartridge.
- **Multi-byte fields are big-endian**, which is a classic source of "why is my traded Pokémon
  level 6400".
- **Names** are Gen 1's own encoding, fixed-width and terminator-padded.
- **`$FE` is the protocol's "no data" byte** and the receiver *skips* it. Real Pokémon data
  contains `$FE` constantly, so the 264-byte Pokémon region is walked, every `$FE` replaced with
  `$FF`, and the 1-based offset recorded in the two-part patch list. Getting this wrong does not
  fail loudly — it silently alters one byte of the Pokémon.

**No game data is embedded in this repository.** Base stats, the internal-index↔Pokédex mapping,
species names and move PP are read out of the player's own cartridge
([`romdata.rs`](../plugins/gen1-link/src/romdata.rs)), located by distinctive byte signature and then
cross-checked against a second known entry, so a mislocated table is an error rather than a
silently wrong Pokémon. It also means Red, Blue and Yellow are each right by construction — and
that Mew, whose base stats live *outside* the main table in Red/Blue, is handled rather than
quietly wrong.

Gen 1's famous **invalid species values** are the 39 unused slots in the 190-entry index table.
`resolve_species` refuses them (`RomError::UnknownSpecies`) rather than offering one.

### 7.4 Choosing what to trade

Config key `gen1_trade = <path>` (or `GB_GEN1_TRADE`), a `key = value` file in the same
hand-rolled shape as the terminal config:

```text
trainer  = ORACLE

species  = Mew           # a name, "#151" for a Pokédex number, or an index
level    = 5
nickname = MEW
ot_name  = GF
ot_id    = 22796
moves    = Pound
dvs      = 15, 15, 15, 15
```

Unknown keys are an **error**, not a silent no-op: a typo'd `speceis =` says so rather than
handing over a default Pokémon and leaving the user wondering. With no file, the partner offers
the level 5 event Mew — chosen because it exercises the two things most likely to be wrong
(Mew's out-of-table base stats, and a legendary at a level no player could have caught, so a
successful trade is unmistakable).

### 7.5 The source is separate from the protocol

`LinkPartner` knows the link cable and nothing else; it is handed a finished `Party`. Everything
about *choosing* what to offer is behind one trait:

```rust
pub trait PokemonSource {
    fn describe(&self) -> String;
    fn party(&self, tables: &RomTables) -> Result<Party, SourceError>;
}
```

This is the load-bearing abstraction, not a tidy one. A virtual link partner is already a
**bridge**: a Pokémon leaves the running game through a genuine trade and something arrives in
its place, and the protocol does not care whether that something came from a text file, another
emulator, a converted save, or a remote peer. Today there is one implementation (`TradeSpec`).
A network partner, a save-file reader, a generation converter or a model picking a Pokémon are
*additional implementations of this trait*, not changes to the protocol.

### 7.6 The user journey, step by step

This is the whole thing end to end, for someone who has never used it.

**1. Write what you want.** Put a `key = value` file anywhere — say `~/mew.trade`:

```text
# ~/mew.trade — the partner's identity, then one block per Pokémon.
trainer  = ORACLE            # who the game will say you traded with

species  = Mew               # a name, "#151" for a dex number, or a raw index
level    = 5
nickname = MEW               # what it is called in your party afterwards
ot_name  = GF                # original trainer; a traded mon keeps its OT
ot_id    = 22796             # 0-65535; with ot_name this is what makes it "traded"
moves    = Pound             # up to four, comma separated
dvs      = 15, 15, 15, 15    # attack, defense, speed, special (0-15 each)
```

Only `species` is required — everything else has a sensible default, and a **second `species`
line starts a second Pokémon** (up to six, which is what lets the partner offer a party rather
than a single mon). A misspelled key is an **error at load**, not a silent default: `speceis =`
says so and refuses to attach, because a partner that quietly offers something other than what
you asked for is worse than one that will not start.

Everything is validated against **your own cartridge's** tables — species names, base stats and
move PP are located in the ROM you are running (§7.3), so `species = Mew` cannot resolve to
something the game does not have.

**2. Arm the partner.** Nothing loads implicitly, so name it:

```bash
# one run
GB_PLUGINS=gen1-link GB_GEN1_TRADE=~/mew.trade cargo run --release --example terminal -- <rom>
```

or permanently, in `~/.config/terminalgb/config.toml`:

```toml
plugins    = gen1-link
gen1_trade = ~/mew.trade
```

**You can tell it worked before you press a button**: the Status panel gains a `Plugin` row
reading `gen1-link: offering MEW (Lv5) — waiting for link`. If the file had a problem, a toast
says what, and no plugin attaches. There is no state in which a link partner is quietly present.

**3. Play to the Cable Club.** You need `EVENT_GOT_POKEDEX` (so, after Oak's parcel errand) and
any Pokémon Center. Talk to the **receptionist at the top-right counter**, choose **TRADE
CENTER**, and let her save. You will walk into the link room.

**4. Use the console — facing RIGHT.** This is the step that catches people, and it is not our
quirk, it is the game's. `CableClubLeftGameboy` (`engine/link/cable_club.asm`) starts the
exchange only for the player standing on the **left** console tile **facing right**; the
right-hand tile is the *other* Game Boy's seat. Walk to the left console, face right, press A.
The player who does this is also the one who ends up **clock master** — which is exactly the
role our partner wants the game to have (§7.1), so this is the supported path, not a
workaround.

**5. What you will see when it lands.** In order: `"Just a moment."` while the two sides
exchange their party blocks → the trade selection screen, your party on top and **the
partner's on the bottom** → pick yours, pick theirs, confirm → the game's trade animation,
naming the Pokémon you sent and the partner's `trainer` → **`"Trade completed!"`**. Back in
your party you now have a
Pokémon whose OT is `GF` and whose ID is not yours, so it is **flagged as traded**: it obeys
only with the right badges, and it earns **boosted experience** (§7.6.1). The Status panel row
changes to `— 1 traded`.

**6. If it hangs.** The failure mode of a link protocol is a freeze, not an error, so here is
the triage in the order worth trying:

| symptom | almost certainly |
|---|---|
| The Status panel has no `Plugin` row at all | The plugin never attached. Check the startup warning — a bad `gen1_trade` path or an unknown key in the trade file. |
| `"Please wait…"` forever at the console | You are on the **right-hand** console tile, or not facing right. Only the left seat drives `CableClubLeftGameboy`. |
| The link room never appears | The save the receptionist takes failed, or the cartridge is not English Red/Blue. The partner refuses unknown builds by header checksum rather than corrupting them. |
| Trade screen appears, then freezes | A desynced exchange. Loading a save state mid-trade does this by design — plugin state is **not** in the save file, so the partner is `reset()` and the game is left mid-protocol. Leave the Cable Club and start the trade again. |
| It worked once and now does nothing | The partner offers its party once per link session. Walk out of the room and back in. |

Nothing here can corrupt your save: the plugin declares **no `WRITE_MEMORY`** (§7.2), so even a
completely desynced exchange can only leave the game waiting for a byte.

#### 7.6.1 The boosted-experience proof

The single most useful piece of evidence that this is a **real** trade and not a clever
imitation is a number we never sent.

The partner puts a level 5 Mew on the wire with **exp 125** — the exact total for level 5 on
Mew's medium-slow growth curve, computed by us. Read the arrived Pokémon out of the party
afterwards and it has **exp 135**.

We did not write 135 anywhere. Generation 1 gives a traded Pokémon boosted experience, and the
game applied it *itself*, on the receiving side, because it believed it had just received a
Pokémon from another trainer. A memory poke that spawned a Mew would have left 125 sitting
there. This is the difference between arriving *through* the trade path and arriving *beside*
it, and it is checked on every run of `examples/gen1_trade.rs`.

### 7.7 How it is verified headlessly

Screenshots are good evidence for a person; they are useless in CI. The driver therefore
verifies the way a test can repeat, and two techniques are worth stealing for any future test
that has to drive a real game:

**Read the game's own screen as text.** `wTileMap` (`$C3A0`, 20×18) holds charmap codes
directly, so decoding it gives back exactly the words a player sees. The driver navigates by
*reading menus*, not by counting frames — it presses A when the screen says `TRADE CENTER`, not
147 frames after the receptionist. That is what makes it robust against emulation-speed changes
and animation length, and it is the same trick `testharness/trainer.rs` uses to check the
trainer panel against the game's own party menu.

**Redirect the room's own warp; do not relocate the player.** The save fixture needs the player
in a Pokémon Center with a Cable Club. The obvious approach — write the player's coordinates
into the Cable Club map — produces a game that *looks* right and then hangs, because map,
tileset, sprite and warp state are all mutually consistent state the game maintains together,
and setting one of them desynchronises the rest. What works is to leave the player exactly
where the game put them and **rewrite the destination of the warp they are standing next to**
(`wWarpEntries`), so the *game* performs its own map transition through its own code path. The
general lesson: when driving a real game, move the world's pointers, not the player.

The fixture is honest about what it edits. `--make-save` boots a **new game**, plays the intro
with real button presses until the game writes its own save, then edits four things in that
save — current map, player position, the Pokédex event flag and the party — and fixes the
checksum the way `engine/menus/save.asm` computes it. Everything else is state the game itself
produced, and **from the receptionist onward nothing is edited at all**.

### 7.8 Verified

`examples/gen1_trade.rs` drives the real ROM headlessly: it walks to the Cable Club, talks to
the link receptionist, takes TRADE CENTER, uses the console, and completes the trade. Observed
against Pokémon Blue (USA/Europe):

```text
party before: SPARK Lv12, RATTY Lv9
party after : RATTY Lv9, MEW Lv5

MEW — species index 0x15, Lv5, HP 26/26, atk/def/spd/spc [16, 16, 16, 16],
      moves [1, 0, 0, 0], pp [35, 0, 0, 0], exp 135, OT GF (id 22796)
```

with the game's own trade animation showing *"PIKACHU went to ORACLE."* Every field matches what
was put on the wire except the experience, which the game raised from 125 to 135 — Gen 1's
**boosted experience for traded Pokémon**, i.e. the game treating it as a genuinely traded mon,
which is the behaviour we wanted to provoke.

Read the example's header comment before running it: the Cable Club needs `EVENT_GOT_POKEDEX`,
so it builds a save fixture first by playing the intro with real button presses and then editing
that save. **Nothing about the trade itself is faked** — from the receptionist onward it is only
button presses and the emulated serial port.

#### Screenshots

`--shots <dir>` writes a PNG at every stage transition, through the same `image`-crate path the
`c` / `C` / `F2` screenshot keys use:

```bash
cargo run --release --example gen1_trade -- --rom <rom> --shots target/trade-shots
```

```text
target/trade-shots/
  00-continue.png             title screen, CONTINUE selected
  01-pokecenter.png           inside the Viridian Pokémon Center
  02-walk-to-receptionist.png crossing to the link counter
  03-trade-room.png           the Cable Club link room, both consoles visible
  04-at-machine.png           at the console: "Just a moment."   ← the handshake
  05-done.png                 "Trade completed!"                 ← the game's own verdict
  06-party-after-trade.png    the Cable Club party list, now showing MEW
```

`--state <path>` additionally writes a save state at the console the instant *before* the trade
starts, which is what makes the whole thing reproducible in the real terminal frontend without
playing to Viridian by hand: drop it into save slot 1, press **F8**, press **A**, and watch the
trade run. That is how the screenshots in [docs/link-trade.md](link-trade.md) were taken — and
it must be a state from *before* the exchange, because plugin state is deliberately not in a
save file, so a state captured mid-trade loads against a `reset()` partner and desyncs.

**Those four frontend screenshots are committed, under `docs/link-trade/`.** That is a
deliberate call against this repo's "never vendor game assets" rule, and the reasoning — that
the rule targets game *data* which substitutes for the cartridge, not pictures of our own UI —
is written out in [docs/link-trade.md §5](link-trade.md#5-on-the-screenshots-and-licensing).

## 8. Interoperating: should we speak someone else's protocol?

Four projects were surveyed. A straight verdict, because this is a decision worth taking
deliberately while the contract is open.

| project | what it actually is | licence |
|---|---|---|
| [pret/pokered](https://github.com/pret/pokered) | disassembly of Red/Blue — the **ground truth** for the protocol | disassembly |
| [Lorenzooone/PokemonGB_Online_Trades](https://github.com/Lorenzooone/PokemonGB_Online_Trades) | Python: Gen 1/2/3 trades over a link-cable adapter or a BGB link, with an online server | MIT |
| [GB-Link](https://github.com/GB-Link) org | **real-hardware bridge**: an RP2040 USB↔link-cable adapter (`GBLink-Firmware`, `GB-Link-USB-DIY`) plus browser apps over WebUSB/WebSerial (`gb-pokemon-web`, `gblink-distributor`) | GPL-3.0 |
| [Poke_Transporter_GB](https://github.com/Striaton-Lab-Team/Poke_Transporter_GB) | homebrew Gen 1/2 → Gen 3 transfer | MIT |

**What GB-Link is.** Not an emulator. The game runs on a real Game Boy with a real cartridge; the
adapter bridges its link port to USB, and the browser is the UI. `gb-pokemon-web` is a JS port
of Lorenzooone's Python. `gblink-distributor` is *literally the captain's idea* — event Pokémon
by a genuine Cable Club trade, "nothing is injected into your save file" — but for hardware.
Ours is the emulator equivalent of the same idea.

**How it solves the clocked-timing problem** — the single most valuable thing to learn from it:
**it keeps the network out of the byte loop entirely.** The microcontroller is the link partner
in hard real time and buffers *whole protocol sections*; only complete sections cross the
network, asynchronously. Lorenzooone's `special_sections_len = [0xA, 0x1A2, 0xC5]` is the same
idea in Python. Our in-process partner sidesteps this a third way (be the slave, answer inside
the write), but the section-buffered shape is what a future **network** partner should use, and
it drops straight in behind `PokemonSource`.

**Verdict: stay standalone; make a network transport one implementation of `PokemonSource`.**

- The GB-Link repos are **GPL-3.0**; this repo is MPL-2.0. Protocol facts and byte layouts are
  freely implementable, the code is not ours to lift, and none of it has been.
- Their wire format is a **USB device protocol** (WebUSB to a specific RP2040), not a network
  link format. An emulator has no use for it as a peer: *we are the Game Boy side.*
- There is no published, stable "GB link over network" spec in that org to implement. The
  closest thing is Lorenzooone's application-level trade protocol, which is for Pokémon trades
  specifically rather than a generic link.
- **If we ever do want emulator-to-emulator linking, the standard to target is the BGB link
  protocol** — a small TCP protocol already spoken by BGB, SameBoy and Emulicious, and
  Lorenzooone's repo ships a `bgb_link_cable_server.py` against it. That is a far better
  interoperability target than a USB device protocol, and it is a `SERIAL_LINK` plugin like any
  other.
- The interoperability that *does* matter, and that we already have, is at the **data** level:
  we implement the same Gen 1 section layout everyone else does, so a Pokémon can move between
  our system and theirs as data.

## 9. Cross-generation transfer — what is and is not achievable

Asked directly, answered directly:

- **Local Gen 1 trade — YES.** Done and verified (§7.8).
- **Gen 2 (Gold/Silver/Crystal) — not done.** Gen 2's Cable Club trade is the same *shape* (a
  handshake, sections, a selection phase) with different constants and an extra mail section, so
  it is a second `LinkPartner` behind the same contract. Gen 2 **Mystery Gift is a different
  mechanism** — infrared, not the link cable — and is a separate piece of work, not a variation
  of this one.
- **Cross-generation conversion as a bridge — YES, as a design, not as code.** A Pokémon can
  leave Blue through a genuine trade, be converted per documented Gen 1 → Gen 3 rules, and be
  delivered elsewhere; and equally arrive from elsewhere into Blue. That is exactly what the
  `PokemonSource` split buys. The conversion rules
  ([Poke_Transporter_GB](https://github.com/Striaton-Lab-Team/Poke_Transporter_GB), MIT) are
  facts: species indices differ between generations, Gen 1 has no natures, abilities or held
  items, stat experience must be mapped onto EVs and DVs onto IVs, and the character encodings
  differ — so an upward conversion has to *invent* the fields that did not exist, which is a
  policy decision a converter must state rather than hide. **Not implemented here.**
- **Running FireRed ourselves — NO.** Generation 3 is Game Boy Advance: a different CPU and a
  different PPU. This emulator is DMG/CGB. Trading "in game between Blue and FireRed" on one
  machine is not something this project can do, and no amount of plugin design changes that.
  What it *can* do is be one honest end of that pipe.

### Why this is worth building now

Pokémon Bank — the official ladder from the older games into the modern ones (Virtual Console and
Gen 3–7 into Bank, then Home) — is **reported to be shutting down on 27 February 2027**. Taken at
face value, that removes the only sanctioned route for moving a Pokémon from an old game forward.
It is recorded here as motivation, and marked as a *reported* date rather than a certainty,
because it is the kind of context that is obvious today and completely opaque in two years.

This is the same instinct as the [conformance tool](conformance.md): official tooling goes away,
and well-made open tools fill the gap. A real trade out of a genuine cartridge, plus a documented
forward conversion, is a route that will not otherwise exist — and the reason to keep the trade
protocol and the *source* of the Pokémon cleanly separated is that it makes a converter, a file,
a remote peer and a live partner interchangeable.

## 10. Reference

### Configuration

| key | env | meaning |
|---|---|---|
| `plugins` | `GB_PLUGINS` | comma-separated plugin names to attach; empty = none |
| `gen1_trade` | `GB_GEN1_TRADE` | trade file for `gen1-link`; unset = built-in event Mew |
| — | `GB_PLUGIN_TRACE` | append the NDJSON event stream to this file |

### Event records

```json
{"ev":"frame","number":7}
{"ev":"serial","sent":1,"received":2}
```

### Sources

- Protocol and data layouts: [`pret/pokered`](https://github.com/pret/pokered) —
  `home/serial.asm`, `engine/link/cable_club.asm`, `engine/link/cable_club_npc.asm`,
  `engine/menus/main_menu.asm`, `constants/serial_constants.asm`, `constants/charmap.asm`,
  `ram/wram.asm`, `macros/ram.asm`, `macros/coords.asm`.
- Stat and experience formulas: [Bulbapedia, "Stat"](https://bulbapedia.bulbagarden.net/wiki/Stat)
  and ["Experience"](https://bulbapedia.bulbagarden.net/wiki/Experience).
- Prior art surveyed, and the licence of each, in §8. No code from any of them is used here.
