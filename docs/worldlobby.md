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
| The relay and the driven players | [`examples/worldlobby.rs`](../examples/worldlobby.rs) | A TCP relay and one emulator per process |
| The relay client | [`src/lobby.rs`](../src/lobby.rs) | The frontend's half — a socket and a thread |
| The human seat | `--human`, and `plugins = gen1-lobby` + `GB_LOBBY` in the TUI | The ordinary terminal frontend, unchanged |

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

Four driven instances, the full demonstration, with screenshots:

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

## 9. What is not done

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
