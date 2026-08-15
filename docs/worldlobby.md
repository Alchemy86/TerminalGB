# The World Trade Centre — a shared Gen 1 room, proved

**What this is:** a working proof of concept of the room scoped in
`gameboy-worldlobby-w4`'s report §9. Several independent emulator instances drop into one
Cable Club room, four screens by three, with a floor drawn in this repository; each walks
under its own control; each sees the others; two of them completed a real Gen 1 trade and
two more started a real link battle, both from inside that room.

**These were four emulator instances, not four humans, and one human seat.** Nobody can
summon four people on demand. What was run is four *independent processes*, each with its
own staged cartridge, its own save file, its own memory and its own walking — plus, since,
a `--human` mode that puts the captain at one of those seats through the ordinary terminal
frontend. The code cannot tell the two apart: a scripted walk and a person's thumb both
arrive as `Gameboy::keydown`. If four instances can see and pass each other, four people
can; what four instances cannot prove is anything about how it *feels*, and that is what
the human seat is for.

Screenshots: **`~/terminalgb-worldlobby/`** (see §7).

---

## 1. What was built

| Piece | Where | What it is |
|---|---|---|
| The room, the crowd, the speech | [`plugins/gen1-lobby/`](../plugins/gen1-lobby/) | A `Plugin`. Pure memory work, no dependency but the plugin API, 31 unit tests, no ROM needed |
| The driven players | [`examples/worldlobby.rs`](../examples/worldlobby.rs) | One emulator per process, walking to a script |
| The relay, and the seat | [`src/lobby.rs`](../src/lobby.rs) | The room's server and client, and `take_seat` |
| The human seat | **`terminalgb --lobby`** | The ordinary terminal frontend, unchanged |

**The network is deliberately not in the plugin.** A `Plugin` is handed a `Machine`
*during a frame the emulator has already decided to run*; a socket is not that, and a
plugin that blocked on one would be the audio hand-off bug again (`docs/audio.md` §4c). So
the plugin is memory work and the socket is the frontend's. That is also what makes every
rule in the plugin testable at unit speed with no ROM, no network and no cartridge.

Two layers, and §4 of the scout report is why they are separate:

```
  relay  ── presence: position, facing, a phrase, for everybody. Never a game byte.
  cable  ── a trade or a battle. Strictly 1:1. The cartridge's own protocol, unchanged.
```

A Game Boy has one shift register and no addressing, so there is nothing to make a hub out
of; the crowd lives entirely outside the machine.

## 2. The room

`20 × 14` blocks = **40 × 28 walk tiles**, four screens by three, in `wOverworldMap`.
Written at run time, never patched, never distributed.

- **The cartridge's own Trade Center stays at block (0,0).** The two consoles are hidden
  events at walk tiles (4,4) and (5,4), keyed to the map id in ROM, and cannot be moved. So
  the room keeps `maps/TradeCenter.blk` in its corner — consoles, both seats and the walls
  around them byte-identical to vanilla — and grows right and down from it. A side effect
  worth having: the first frame after installation is already correct, because every block
  the player can currently see is the block that was there a moment ago.
- **Ringed with wall on every side.** There is no map boundary in the collision check — one
  tile of `wTileMap` is the whole test — so an unringed room lets the player walk out of the
  world and `wYCoord` underflows with no fault raised.
- **Both dimension pairs are written.** `wCurMapWidth`/`Height` in blocks *and*
  `wCurrentMapWidth2`/`Height2` in walk tiles; the edge-of-map check reads the second pair.
- **Our own art**, three tiles written into `vTileset` at `$9000`: two floor tiles and the
  wall. Collision is by tile *id* against `Club_Coll`, never by pixels, so redefining the
  floor cannot change where anybody may walk — asserted.

## 3. The one thing that was measured rather than argued

Deriving screen tiles from a block map means reimplementing `LoadCurrentMapView` and
`DrawTileBlock`, and the anchor — which screen row the player's walk tile starts on — is
invisible on a checkerboard floor and glaring on anything else.

So the plugin **calibrates against the running engine before it paints anything**: it
reproduces the whole 20×18 buffer from the cartridge's own block map, for each candidate
anchor, and compares cell by cell.

```
screen calibration: anchor Some(8), candidates [(360, 360), (133, 360)]
```

Row 8 reproduces the engine's screen **exactly**; row 9 gets 133 of 360. If neither matched,
`anchor` would be `None` and the plugin would never write `wTileMap` at all — declining to
paint beats painting something plausible and wrong.

## 4. The crowd, and the four numbers

| Number | Value | Fixed by |
|---|---|---|
| connected to one room | no cartridge limit | our relay |
| held as game objects | 15 | `wSpriteStateData` |
| **drawn at once** | **10 characters including you** | 40 OAM entries ÷ 4 per character |
| **on one horizontal band** | **5 characters including you** | the DMG's 10-objects-per-scanline rule |

The streaming layer picks the nearest **9** visible peers each tick and writes them into
sprite slots 1..9, clearing the rest. Nine plus the player is exactly the OAM budget, which
makes the overflow the scout measured (the eleventh character writes into `wTileMap`)
*impossible by construction* rather than merely unlikely.

Two mechanisms worth keeping:

- **`IMAGEBASEOFFSET` stays zero.** `_UpdateSprites` skips a slot whose base offset is zero,
  so no movement AI ever walks our avatars off on their own — while `PrepareOAMData`, which
  gates on `PICTUREID` alone, still draws them. The cost is that the engine's own cull is
  skipped too, so the plugin reimplements `CheckSpriteAvailability`'s window (four up, four
  down, four left, five right) and owes the reader that function.
- **The pixel formula is `InitializeSpriteScreenPosition`'s**, and the check that it is the
  right one is that a peer standing on your own square reduces to `($3C, $40)` — exactly the
  fixed screen position `ResetPlayerSpriteData` gives the player.

## 5. What worked

Measured on the captain's own Pokémon Blue cartridge and Cable Club save, staged into
`target/`, four processes plus a relay:

- **Four consoles into one room.** Each pairs with its partner over the auto-discovered
  unix-socket cable, walks the Cable Club receptionist path, and installs the room. Room
  installed once per console; re-installed automatically after `ReturnToCableClubRoom`
  restores the cartridge's own header (`room installs 2, art blits 5` on the trading pair).
- **Independent walking, and they see each other.** Four screenshots of one relay tick show
  four different views of the same four people — the ring seen from four points in it.
- **Avatars are solid.** `CollisionCheckOnLand` calls `IsSpriteInFrontOfPlayer` before it
  looks at a tile, so you cannot walk through somebody. This is free and it is real: with
  four people converging on two consoles they block each other, exactly as four people
  would.
- **Speech.** A canned phrase drawn over the speaker's head in the cartridge's own font,
  relayed to everybody. Gen 1's charmap has no `@ # & * " _`, so there are no e-mail
  addresses, URLs or `@handles` in this room — a rather good accident.
- **A real trade, in the custom room.** ~1,600 transfers, 0 torn; RED ends holding ROCKY
  (OT BLUE) and BLUE holding GLIMMER (OT RED), every field well-formed.
- **A real link battle, in the custom room** — and this closes an item the scout report
  listed as *inferred and explicitly not measured*. `wCurMap = $F0` (COLOSSEUM) is the whole
  change: `wLinkState` reached `$03` (`LINK_STATE_START_BATTLE`) then `$04`, 36,700 transfers,
  and the two parties ended at 0/28 and 1/25 HP with `FLAP used TACKLE!` and
  `Enemy ATTY used TACKLE!` on film. **The one-byte flip is now measured, not inferred.**
- **No save was touched.** Every write is below `wMainDataStart` except five header bytes,
  and those are gated on `wLinkState == LINK_STATE_IN_CABLE_CLUB` — the state in which
  `LinkMenu` has set `BIT_LINK_CONNECTED`, so `DrawStartMenu` prints RESET where SAVE would
  be and `SaveMainData` cannot run. Every ROM and save is a staged copy in `target/`.

## 6. What went wrong, and the causes

Four defects, each of which looked like something else. They are the useful part of this
report.

1. **A fixed key-hold is 1.5 Gen 1 steps.** A step is 16 frames and the game commits to one
   once begun, so any hold long enough to cover turn-then-walk also starts a second step; a
   walker aiming at an exact square overshoots and oscillates either side of it for ever.
   Fixed by releasing the key the instant the coordinate moves — which is what a person
   does.
2. **`Gameboy::clear_input` does not release a key pressed with `keydown`.** It resets the
   *scheduled-press queue*. Handing over from the walker to the Cable Club bot without
   releasing left a direction physically down for the rest of the run, and every player
   ended up pinned against a wall — which reads exactly like a broken route.
3. **A plugin's `Frame` event is dispatched *after* the picture is drawn**
   (`Gameboy::end_frame`). The bubble was repainted at the top of the tick and redrawn at
   the bottom, which brackets the render exactly, so it was never on screen for a single
   frame and looked as though the text was not being written at all. Draw first, hand back
   only what is no longer wanted.
4. **The two ends of a cable must run at the same speed, and must start together.** The room
   is paced to the real console (16.7427 ms/frame) because a demonstration of what people
   would see has to run at the speed they would see it; the 1,600-transfer exchange is then
   run unpaced, as the shipped `link_two --peer` does. Letting whichever console reached its
   seat first do that *while its partner was still walking back* meant one end offered
   **33,697 unanswered master transfers**, and Gen 1's `.establishConnectionLoop` gives up
   after 90 iterations and sits on `PLEASE WAIT!` for ever. The relay now reports how many
   players are at a console and both ends start on that signal.

Two of these — 2 and 3 — are general facts about this codebase that nothing else had
occasion to discover.

## 7. The screenshots

`~/terminalgb-worldlobby/`. Contact sheets first:

| File | Shows |
|---|---|
| `00-SHEET-four-viewpoints.png` | **one instant, four screens** — the money shot |
| `10-SHEET-trade-red.png`, `10-SHEET-trade-blue.png` | at the console in the custom room → the machine → the offer screen → `MACHOP will be traded.` → `Trade completed!` |
| `20-SHEET-battle-green.png`, `20-SHEET-battle-yellow.png` | the COLOSSEUM entry in the custom room → `PLEASE WAIT!` → the BLUE vs BLUE versus screen → HP falling 21/28 → 13/28 → 5/28 |

And per-stage frames from **both** consoles: `01-room-*` (the empty room, our art),
`02-arrived-*`, `03-walking-*`, `04-passing-*`, `05-crowd-*`, `06-talking-*`,
`10-trade-<who>-{at-console,at-machine,live-NN,traded,finished}`,
`20-battle-<who>-{colosseum-entry,live-NN,hp-NN}`, `08-end-*`.

## 8. Running it

### The one command

```sh
gameboy --lobby
```

That is the whole thing. No `--features` block, no ROM path, no relay address,
no `GB_LOBBY`.

**No launcher change was needed for it**, which is worth knowing rather than
assuming: `~/.local/bin/gameboy`'s argument loop matches its own five flags and
sends anything else through as the ROM path, so `--lobby` arrives at the
frontend intact and is matched there before the path arm. `terminalgb --lobby`,
`terminalgb lobby` and `./target/release/examples/terminal --lobby` are the same
command by other names.

What it does, and why each part is where it is:

| | |
|---|---|
| **the cartridge** | the argument if you gave one, else the top of the recent list — the same machinery every other launch uses (`screen::config::Recent`) |
| **the save** | whatever sits beside that cartridge, as usual. The seat is an *ordinary launch of your own game*, not a copy |
| **the plugins** | your configured list, minus anything that would fight a real second console for the cable, plus `gen1-lobby`. So `plugins = gen1-link, trainer` keeps its trainer panel and loses only `gen1-link` |
| **the room** | hosted here, on `127.0.0.1` and a port the kernel picks |
| **the partner** | a driven console booted for you, so the room works with one person in it |

Variations, all single tokens because the launcher only forwards one argument:

- `--lobby=HOST:PORT` — join somebody else's room instead of opening one. No
  partner is booted; the seats there are already spoken for.
- `--lobby-alone` — open a room and *don't* boot a partner. For two people at
  one machine: both run it, and `link = auto` pairs them.

**`plugin-gen1-lobby` has been in the crate's default features since the room
landed** (`default` → `standard-plugins` → `plugin-gen1-lobby`), so an ordinary
`cargo build --release` already contains it and the long `--features` list on
the older command below was redundant. The only thing that was ever missing from
an ordinary launch was a way to *ask* for it.

### The partner, and where it comes from

The driven console is `examples/worldlobby.rs`'s own `play` subcommand, spawned
as a child process — **not a second implementation of it**. A driven player is a
walker, a phase machine, a screenshot writer and the Cable Club bot; none of
that belongs in the shipped emulator, and two copies of it would let the
captain's partner and the demonstration's players drift apart. The frontend's
job is only to *find* that binary (`lobby::find_partner_bin`: beside this
executable, then `examples/` beside it, `$GB_LOBBY_PARTNER` overriding) and hand
it the room.

If it is not there, that costs the partner and nothing else — the room still
opens, and it says so. Build it with:

```sh
cargo build --release --example worldlobby
```

### The demonstration

Four driven instances, the full capture run, with screenshots:

```sh
cargo run --release \
  --features tui,image,plugin-gen1-lobby,plugin-gen1-link,plugin-trainer \
  --example worldlobby -- \
  --rom "$HOME/roms/Pokemon - Blue Version (USA, Europe) (SGB Enhanced).gb" \
  --save "$HOME/roms/Pokemon - Blue Version (USA, Europe) (SGB Enhanced).sav" \
  --shots ~/terminalgb-worldlobby --players 4 --speed 3
```

**The human seat** — you play one console at your own terminal, a driven player walks the
same room:

```sh
cargo run --release \
  --features tui,image,plugin-gen1-lobby,plugin-gen1-link,plugin-trainer \
  --example worldlobby -- --human \
  --rom "$HOME/roms/Pokemon - Blue Version (USA, Europe) (SGB Enhanced).gb" \
  --save "$HOME/roms/Pokemon - Blue Version (USA, Europe) (SGB Enhanced).sav"
```

Walk to the Cable Club receptionist, say yes, choose TRADE CENTER. Your partner is doing the
same on the other end of the cable. Once you are both in, the room becomes 40 × 28 with our
own floor and you will see each other walk about; the partner then stands at its console and
keeps offering a trade, so walking to yours and pressing A starts one. The partner's output
goes to `target/worldlobby/partner.log`.

Nothing about the human seat is a special path: the terminal frontend attaches the same
`gen1-lobby` plugin (`plugins = gen1-lobby`), joins the same relay over the same wire
(`GB_LOBBY=host:port`, `GB_LOBBY_NAME=…`), and pairs on the cable through the same
auto-discovered peer (`link = auto`, the default).

### What was measured, and how

Every claim in §8 and §9 was run, not reasoned about, on this machine against
the captain's own Pokémon Blue and Cable Club save, staged into `target/`:

- **`terminalgb --lobby` end to end** — resolved the cartridge from the recent
  list, hosted a room on `127.0.0.1`, booted the partner, seated a person, and
  put both of them in the 40x28 room. Screenshotted from inside with `C`.
- **The corruption, before and after** — the scrambled screen reproduced, then
  the trace that explains it (§9.1), then the same walk rendering correctly.
- **Walking about, and seeing each other** — seven consecutive frames of the
  camera scrolling across the room, and a dozen more with both characters on
  screen at once.
- **The cable** — `0 torn` across every run after the timeout fix, against a
  `FAULT` on the first master transfer before it.

Two things were **not** completed and are not claimed: a full trade *from* the
room with a person's own thumb (the driven partner reached the trade screen —
`wLinkState = $02`, "both consoles ready; pressing A" — but the keystroke
harness cannot steer a player onto an exact tile the way `Walker` does, which is
the whole reason `Walker` exists), and anything at all about how a crowd of ten
*feels*, which is §10's first item and still open.

## 9. Two defects the human seat found, and neither was in the room

Both were reported as "the lobby is broken", and neither was the room, the block
ids, the floor art or the crowd — all of which were already right. They are
recorded here in the shape the [standing conformance
convention](../AGENTS.md) asks for; like the render bugs, these are defects in
*consumers* of emulated state rather than in emulation, so point 3 cites the
disassembly and this repository's own measurements rather than Pan Docs.

### 9.1 The room was installed into a map that was not loaded yet

*Symptom* — a screenful of overworld/shop blocks (bikes, tables, chairs,
counters) instead of the room, on the console with `gen1-lobby` attached, while
its partner rendered the vanilla Trade Center perfectly. Then both consoles
wedged: the scrambled one stopped answering the cable and the save guard faulted
its partner. The corruption has *structure* — it is the room's block layout read
through the wrong map — which is what says "wrong moment", not "wrong data".

*Caught by* — **nothing, and nothing could be**: the emulator was entirely
correct throughout, and the four-instance demonstration passes because
`examples/worldlobby.rs` attaches the plugin *after* its console has already
arrived in the Cable Club. The terminal frontend attaches plugins at ROM load,
so only a person walking in at their own speed goes through the window. That is
a coverage gap in the demonstration, not a lucky escape.

*The behaviour being violated* — **`wCurMap` is set long before the map is
loaded.** This repository already records that for the agent
([`docs/gen1/agent-play.md`](gen1/agent-play.md): during the opening cutscene
`wCurMap` reads the bedroom with plausible coordinates while no map header is
loaded), and `pokered`'s `LoadMapHeader` (`home/overworld.asm`) is where the
header, the block pointer, the tileset and the dimensions actually arrive. The
plugin's guard tested `wCurMap == TRADE_CENTER && wLinkState ==
LINK_STATE_IN_CABLE_CLUB` — two bytes that are true several frames early.

*Measured, on the human console, at the frame it matters* (`GB_LOBBY_TRACE`, a
temporary instrument, one line per plugin tick):

```
f2040  map0xEF link0x01  h4 w7   cal[(5,360),(2,360)]    <- Pokémon Center's 7x4: header NOT loaded
f2070  map0xEF link0x01  h4 w5   cal[(360,360),(101,360)] <- Trade Center's 5x4: header loaded
```

Thirty frames — half a second — in which both guard bytes say Cable Club and the
engine is still showing the previous map. The old code installed at `f2040`.

*The fix, and why it is not a new heuristic* — the plugin **already** proves the
engine has loaded this map: `screen::calibrate` reconstructs all 360 cells of
`wTileMap` from the live block map and requires an *exact* match before it will
paint. It computed that proof, used it to decide whether to paint, and installed
regardless. Now nothing is written until the calibration succeeds, and it is
retried every tick until it does. Note that a "non-zero width and height" gate
would **not** have caught this: the width was 7, not 0 — it was the *previous*
map's, restored by the save. The calibration is strictly stronger because it
asks the engine rather than a byte.

*And a second, sharper edge under it* — the first version of that gate still
installed on **frame 1, before the boot logo**, on a calibration that scored
360/360. Both `wTileMap` and `wOverworldMap` are untouched WRAM at that point,
so "an exact reproduction" is two uniform buffers holding the same byte.
`screen::MIN_DISTINCT_TILES` is the answer: a screen with fewer than four
distinct tiles is not evidence of anything. Same rule, same reason, as the
test-ROM harness requiring its reference image to have more than one shade
before it will believe a comparison against it.

*New test* — `screen::tests::a_blank_screen_is_never_a_calibration`, which pins
the second half at unit speed with no machine and no cartridge. **The first
half — that nothing is written until the calibration succeeds — is pinned by
the measurement above and not by a unit test**, and that is worth saying rather
than glossing: this crate deliberately has no dependency on the emulator, so a
test that watches an install would need the `terminalgb` dev-dependency
`plugins/battle-control` carries. It is a real gap, and the honest reproduction
is the one that found it: run `gameboy --lobby`, walk in, and look.

### 9.2 The cable's patience was counted in the wrong clock

*Symptom* — `FAULT — the console on the other end of the link cable stopped
answering mid-exchange, so a byte was lost`, before the player had walked out of
the Pokémon Center, every time. The far console was healthy and running.

*Caught by* — nothing. Every in-repo link test runs two consoles at the same
speed, which is the one case that cannot show it.

*The behaviour being violated* — a master transfer that is waiting for **another
process** is waiting on the wall clock, because that is the only clock the two
processes share. `LinkPeer` counted it in *its own emulated T-cycles*:
`HOLD_TIMEOUT_TICKS = 1_048_576`, a quarter of a second of emulated time. On a
console running unpaced — a scripted player, a fast-forwarding one, a benchmark
— that is about **5 ms of anybody's afternoon**, and the far end services its
socket once per 16.7 ms frame. So every master transfer was declared torn and
the save guard faulted both machines.

The reference implementations do it the other way round and say so:
[`src/bgblink.rs`](../src/bgblink.rs)'s `MASTER_TIMEOUT` is a
`Duration::from_secs(2)`, and [`docs/bgb-link.md`](bgb-link.md) §3a records why
a generous wall-clock timeout is right — the counterparty it was verified
against ([`Lorenzooone/PokemonGB_Online_Trades`](https://github.com/Lorenzooone/PokemonGB_Online_Trades),
MIT, used unchanged by our interop harness) paces at **10 ms per byte from a
Python thread that does not even set `TCP_NODELAY`** and completes real trades.
`LinkPeer::HOLD_TIMEOUT` is now the same two seconds, for the same reason.

*The same mistake twice* — `OFFER_TICKS`, how long an *incoming* byte is held
while this console is given a chance to arm `SC = $80`, was 4096 T-cycles: one
transfer's worth, i.e. **one seventeenth of a frame**. A console that armed on
its very next frame missed it every time. It is now two frames, which is
[`src/link/transport.rs`](../src/link/transport.rs)'s `SLAVE_ARM_WINDOW`, the
same number and the same argument (`docs/bgb-link.md` §3b: *"an incoming byte is
held for up to `SLAVE_ARM_WINDOW` (two frames) while the console keeps running,
and only refused when it plainly is not going to take it"*).

*Conformance candidate* — **a deadline for something another process must do
belongs to the clock both processes share.** Deriving it from emulated time
makes it mean whatever multiple of real time this console happens to be running
at.

### 9.3 …and one that was the demonstration's own rule, unapplied

`worldlobby play`'s phase 0 — boot, walk to the receptionist, get into the Cable
Club — ran **unpaced**, and counted its patience in emulated frames. That is
harmless when the other end is another copy of the same program (they start
together and run at the same multiple of real time) and wrong the moment it is a
person: six emulated minutes at ~50x is seven seconds, and the Cable Club
attendant will not let *either* console through until both are standing at a
desk. Measured: the bot gave up before the player had left the Pokémon Center.

It is paced now, and its patience is wall-clock. That is not a special case for
humans — it is §6.4's rule, that the two ends of a cable must run at the same
speed, applied to the half of the run that was exempt from it. `--speed` scales
it, so the four-instance demonstration is as quick as it ever was.

### Dead ends, recorded so nobody repeats them

- **"Stall the console like `TransportLink` does."** That is the right shape for
  a socket transport and wrong here: `LinkPeer`'s own tests drive two peers
  **in one thread**, alternately, so a peer that stops stepping the CPU until
  the far end answers deadlocks against a far end that only runs when this call
  returns. Holding the shift register (which `LinkPeer` already does) achieves
  the same thing for the *game* without that hazard; only the deadline was
  wrong.
- **"Gate the install on non-zero `wCurMapWidth`/`wCurMapHeight`."** The obvious
  reading of the `wCurMap`-is-early rule, and it does not work here: at the
  moment that matters the width was **7** — the previous map's, restored from
  the save by `LoadSAV` — not 0. Measured before it was believed.
- **"Stage the human's cartridge, as `--human` does."** Right for a capture run,
  wrong for a launch: `terminalgb --lobby` is somebody playing their own game,
  and silently playing a throwaway copy loses whatever they do after they leave
  the Cable Club. `SeatOptions::stage_self` is the one line of difference
  between the two callers.
- **Do not diagnose this from a picture.** Two runs were spent squinting at
  screenshots deciding which console was which. The trace that settled it in one
  run was tagged with the plugin's own name, not with a pid — the two processes
  interleave into one file and pids do not say which console is which.

## 10. What is not done

- **Three or more *humans*.** Everything above is instances. A human-in-the-loop test would
  measure the one thing instances cannot: whether a crowd of ten reads as a busy place or as
  a mess, and whether 8 Hz position sync looks smooth to an eye rather than to arithmetic.
- **The relay is local.** `127.0.0.1`, one process, plain TCP. The Durable Object relay
  (§7 of the scout report) is where this goes to reach people who are not on this machine;
  the wire is deliberately trivial so swapping the transport is a client change.
- **Free text.** Canned phrases only, because free text needs an answer to "what happens
  when a stranger types something unpleasant", and that is a decision rather than an
  implementation.
- **Invite/accept.** The demonstration decides who trades and who battles up front. In the
  product that is a relay message and an overlay, and it is also where the *both consoles
  must agree* rule of §5 gets enforced: `wLinkState` never crosses the cable, so if one side
  thinks trade and the other thinks battle they exchange the same 644 bytes and then diverge.
- **Gen 2.** Everything here is derived from `pret/pokered`. Gold is a different engine with
  a different Cable Club; it needs its own address derivation and is not a small addition to
  this.
