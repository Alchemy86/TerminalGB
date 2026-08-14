# The link cable — two emulated consoles, one wire

> Code: [`src/link.rs`](../src/link.rs), [`src/mmu/serial.rs`](../src/mmu/serial.rs),
> [`examples/link_two.rs`](../examples/link_two.rs).
> Related: [`docs/plugins.md`](plugins.md) (the *impersonating* link partner),
> [`docs/gen1/link-from-outside.md`](gen1/link-from-outside.md).
> Conformance record: [`docs/conformance-notes.md`](conformance-notes.md), 2026-08-14 —
> including why **no public test suite can see half of the link port**.

There are two ways to be the far end of a Game Boy's link port, and this repository
now has both.

**Impersonation** — [`plugins/gen1-link`](../plugins/gen1-link) speaks Pokémon
Red/Blue's trade protocol from Rust, convincingly enough that the real game
completes a real trade with it. That is the right answer when you own one
cartridge, and it can only ever know the protocols somebody wrote down.

**Relaying** — this page. Two whole cores, joined by a cable that knows nothing
about any game. Whatever the two programs agree on works, because both ends are
the genuine article: a Gen 1 trade, a Gen 1 link battle, the Gen 1 ↔ Gen 2 Time
Capsule, Tetris, or something nobody has looked at.

---

## 1. The hardware, and the half that was missing

From Pan Docs, *Serial Data Transfer (Link Cable)*:

* **`SB` (`$FF01`) is a shift register, not a mailbox.** Both consoles present
  their `SB` to the wire at once; after eight clock edges each holds what the
  other started with. There is no direction — a transfer is always an *exchange*.
* **`SC` (`$FF02`)** bit 7 is the transfer-start flag; bit 0 selects the shift
  clock, **1 = internal** (this console drives it — "master") and **0 = external**
  (the other console drives it — "slave").
* A master's transfer runs on its own clock, 8192 Hz on a DMG, so eight edges is
  **4096 T-cycles ≈ 1 ms**, and it **completes whether or not anything answers** —
  with `$FF` shifted in, because an unplugged cable's data line idles high.
* A slave's transfer **never** completes on its own. With no cable it waits for
  ever.
* On the eighth edge *both* consoles clear `SC` bit 7 and request the serial
  interrupt (`IF` bit 3). Agreeing on that instant is the whole of staying in sync.

Before this work `Serial::wb` acted on `SC = $81` and **ignored `SC = $80`
entirely**. That is not a bug for one console on its own — nothing was ever going
to clock it — and it is exactly the half a second console needs. Adding it was the
substantive change; the cable on top is small.

`Serial` now exposes the port as a *cable* sees it rather than as the game does:
`LinkRole` (`Idle`/`Master`/`Slave`), the byte in the shift register, a
per-transfer sequence number, and two operations — answer a master, clock a slave.

## 2. Staying in sync: one process, one loop

**Both cores run in the same process and are stepped by the same loop.**
`LinkCable::run_frame` pays each core one instruction at a time, always feeding
whichever is behind by cycle count, so the two never differ by more than a single
instruction (~24 T-cycles) against 4096 T-cycles per byte. There is no IPC, no
wall clock and no negotiation, so there is nothing to drift.

Each core is paid exactly `FRAME_TICKS` (70,224 T) per `run_frame` rather than run
to its own PPU frame boundary as `Gameboy::frame` does. Two consoles cannot both
stop on their own boundary without one standing still while the other catches up,
and standing still is precisely when a link drops a byte.

The cable does **not** own the two consoles — it is handed both on every call — so
a driver keeps whatever arrangement suits it.

```rust
let mut cable = LinkCable::new();
loop { cable.run_frame(&mut a, &mut b); }
```

Servicing happens between every instruction:

1. A console starts a master transfer (`SC = $81`); the cable notices.
2. If the other end is a slave, they are **paired**: both bytes are latched and
   the master is told what it will shift in. The master still runs its full eight
   edges, so the game sees hardware timing, not an instantaneous exchange. A slave
   that arms *after* the master started is still in time — it has the rest of the
   ~1 ms window.
3. When the master's shift register runs out the cable clocks the slave: the
   master's byte lands in the slave's `SB`, its start flag clears and its serial
   interrupt is requested. Both consoles now agree.

Cases the cable deliberately does not paper over, because hardware does not
either: **two masters** (both shift in `$FF`, as two consoles fighting over the
clock line would) and **two slaves** (nothing happens, for ever — every link
protocol's handshake exists to break exactly this).

## 3. Two readings of ambiguous hardware, both forced by measurement

### A torn byte keeps its data

When the other console arms `SC = $80` and rewrites `SC` before the eighth edge
the exchange is *torn*. **The master keeps the byte that was latched**; only the
slave half is abandoned, which is what the slave asked for by rewriting the
register.

The first version did the opposite — voided the byte and faulted both machines as
a desync — and then faulted them 898 frames into a Blue-to-Blue trade, before the
two consoles had finished saying hello. Gen 1's `.establishConnectionLoop`
(pokered `engine/link/cable_club_npc.asm`) does this, every frame, for as long as
it takes to find a partner:

```
	ld a, ESTABLISH_CONNECTION_WITH_EXTERNAL_CLOCK  ; $02
	ldh [rSB], a
	ld a, SC_START | SC_EXTERNAL                    ; $80   <- slave, offering $02
	ldh [rSC], a
	...                                             ;        ~100 T-cycles later
	ld a, ESTABLISH_CONNECTION_WITH_INTERNAL_CLOCK  ; $01
	ldh [rSB], a
	ld a, SC_START | SC_INTERNAL                    ; $81   <- master, sending $01
	ldh [rSC], a
	call DelayFrame
```

Reading `$02` out of that hundred-cycle window is the **only** way the two
consoles ever agree on who drives the clock. `USING_EXTERNAL_CLOCK` and
`USING_INTERNAL_CLOCK` share their values with the `ESTABLISH_CONNECTION_WITH_*`
bytes (`$01`/`$02`, and note they are the opposite way round), so the reply *is*
the role assignment: the console that reads `$02` becomes the clock master and
the one that reads back `$01` follows. A cable that voids a torn byte can never
connect two Game Boys at all.

### The fail-safe is fault propagation, not byte accounting

A desynced trade must never reach a save file, and the guard is the one the crash
detector already uses: `Gameboy::report_fault`, which poisons the battery so the
save is not written again this session.

But the *trigger* is not "a byte went missing" — that fires all through a healthy
handshake, as above. It is **one console faulting**, at which point the cable
faults the other too. Half an exchange is exactly the situation where the
*surviving* console's save is the dangerous one: a Gen 1 trade removes a Pokémon
from one party and adds it to the other, and a crash between those two writes
leaves whichever side is still running holding a file the player would rather not
keep.

Byte-level desync detection is left to the games, which are good at it —
`Serial_ExchangeBytes` re-sends its first byte until it sees the `$FD` preamble
come back. The cable counts `transfers` / `unanswered` / `torn` and shows them, so
a link that is quietly doing nothing is visible.

## 4. Phase — why `--skew` is not a workaround

Two real consoles never share a crystal and two real players never press A on the
same frame, so their handshakes drift past each other until they interleave. **Two
emulated cores stepped by one loop have no drift at all.** Started together they
stay in phase for ever: both master at the same moment, each finds the other deaf,
and the handshake can never resolve. That is the price of determinism, not a cable
bug, and the cure is to start one console a little ahead of the other.

Measured, Blue to Blue, 2500-frame budget:

| skew (frames) | outcome |
|---|---|
| 0 | never connects — `hSerialConnectionStatus` stays `$FF` on both |
| 1, 2, 3, 5 | connects; A internal (`$02`), B external (`$01`) |
| 8, 13, 21, 34 | connects; roles swap — B internal, A external |

Any non-zero skew works, and which console ends up driving the clock depends on
it. Nothing downstream should assume a role: read `hSerialConnectionStatus`.

## 5. Running it

```
cargo run --release --features image,plugin-gen1-link,plugin-trainer \
    --example link_two -- \
    --rom  ~/roms/blue.gb \
    --save <cable-club-fixture.sav> \
    --trade --skew 3 --shots /tmp/shots
```

* `--rom` / `--rom-b` — the two cartridges (`--rom-b` defaults to `--rom`).
* `--save` / `--save-b` — save fixtures, **copied** into `target/link-two/{a,b}`
  along with the ROM. The MBC writes `<rom>.sav` beside the ROM it was given, so
  nothing in `~/roms` is ever opened for writing.
* `--trade` — drive both consoles into the Cable Club and trade.
* `--skew N` — start console B N frames ahead. See §4; **1 or more, never 0**.
* `--shots <dir>` / `--verbose` — see below.

### Presentation: side by side, headless

Both consoles are always visible. `--shots` writes a single PNG containing **both**
screens; `--verbose` prints both tilemaps decoded as text, which is what a Gen 1
game's screen really is.

That is a deliberate choice over a two-pane terminal frontend. The terminal render
path transmits **one** picture under one image id, and the whole of
[`docs/terminal-render.md`](terminal-render.md) is about not making that more
expensive; a second live picture is real work with real regression risk. The link
is the deliverable, and a side-by-side capture proves both machines are running
without putting a finger anywhere near the frontend.

### Which seat, and why the bot does not choose

One bot drives both consoles. Which Game Boy on the Trade Center table is yours is
**not a choice** — `CableClubLeftGameboy` returns immediately unless this console
took the internal clock, and wants the player facing **right**;
`CableClubRightGameboy` mirrors it for the external clock and wants **left**
(pokered `engine/pokemon/bills_pc.asm`). The game has already warped the player
next to the correct console (`LoadSpecialWarpData` picks `TradeCenterPlayerWarp` or
`TradeCenterFriendWarp` off the same byte), so the bot reads
`hSerialConnectionStatus` (`$FFAA`, asserted by name in
`engine/link/cable_club_npc.asm`) and faces the right way. Pressing A at the wrong
console is a silent no-op that looks exactly like a broken link.

## 6. What was measured — Gen 1 ↔ Gen 1

Two Pokémon Blue instances, from copies of the captain's cartridge and a Cable
Club save fixture, with distinct parties patched in:

```
cable: 1552 transfers, 0 unanswered, 0 torn

console A party:
  RATTY    species 0xa5 Lv8  DVs (15,15,15,15)  OT RED  (id 4242)
  ROCKY    species 0x6a Lv11 DVs (3,4,5,6)      OT BLUE (id 1717)
console B party:
  FLAP     species 0x24 Lv7  DVs (15,15,15,15)  OT BLUE (id 1717)
  GLIMMER  species 0x94 Lv10 DVs (14,10,10,10)  OT RED  (id 4242)
           <- would be SHINY if traded to Gen 2
```

GLIMMER (an Abra built with the shiny DV pair) crossed the wire and the trainer
plugin's `derived::shiny_if_traded` still calls it shiny on the far side — the
verdict is that plugin's, not a second copy of the rule. Both `.sav` files on disk
pass the game's own `CalcCheckSum`, and `~/roms` is byte-identical.

---

## 7. Gen 1 ↔ Gen 2: the Time Capsule

**Gen 1 ↔ Gen 2 does not use the ordinary link trade.** Gold/Silver's own trade
receptionist *refuses* a Gen 1 partner — `LinkReceptionistScript_Trade` reads
`wOtherPlayerLinkMode` and branches to `.LinkedToFirstGen` → *"You can't link to
the past"* (pokegold `maps/Pokecenter2F.asm`). The Time Capsule receptionist is
the mirror image: it **requires** that same byte to be zero.

### The wire is unchanged — which is the whole argument for a relaying cable

In Time Capsule mode Gen 2 speaks **Gen 1's protocol, byte for byte**
(pokegold `engine/link/link.asm`): the same `Serial_ExchangeBytes` three-block
exchange — random-number preamble, party block, patch lists — and
`Link_PrepPartyData_Gen1` builds the party block in **Gen 1 format**: six `$FD`
preamble bytes, the 11-byte player name, the party count, the species list
terminated by `$FF`, then `REDMON_STRUCT_LENGTH` (44-byte) structs, then OT names
and nicknames. `ConvertMon_2to1` maps species indices on the way out and
`ConvertMon_1to2` / `Link_ConvertPartyStruct1to2` map them back on the way in.

So the cable in this repository needs **no Time Capsule support at all**. That is
the point: an impersonating partner would need a second protocol implementation;
a relay needs nothing.

### What Gen 2 rejects

`CheckTimeCapsuleCompatibility` (pokegold `engine/link/link.asm`) inspects the
**Gen 2 player's own party** before the link opens and returns one of:

| result | meaning | test in source |
|---|---|---|
| 0 | party is fine | — |
| 1 | a Pokémon introduced in Gen 2 | species `>= JOHTO_POKEMON` (#152 Chikorita and up) |
| 2 | a move introduced in Gen 2 | move index `> STRUGGLE` (#165) |
| 3 | a Pokémon is holding mail | `ItemIsMail` |

Note the consequence that trips people up: **all three Johto starters are Gen 2
species**, so a Gold player must leave the starter in the PC and carry only Gen 1
Pokémon with Gen 1 moves.

### The in-game prerequisites, in order

1. **Reach Ecruteak City and enter its Pokémon Center.** Bill's scene
   (`EcruteakPokcenter1FBillActivatesTimeCapsuleScript`, pokegold
   `maps/EcruteakPokecenter1F.asm`) runs on entry, `clearevent EVENT_MET_BILL`,
   `setflag ENGINE_TIME_CAPSULE`. Ecruteak is the fourth gym town — roughly three
   badges and several hours into the game.
2. **Wait one day.** `ENGINE_TIME_CAPSULE` is a *daily* flag
   (`data/events/engine_flags.asm` maps it into `wDailyFlags1`), and the
   receptionist is closed while it is **set**. `CheckDailyResetTimer`
   (`engine/overworld/time.asm`) zeroes `wDailyFlags1` once a day has elapsed on
   the cartridge clock. Until then: *"I'm sorry — the TIME CAPSULE is being
   adjusted."*
   This emulator's MBC3 clock is driven by **emulated** time while running and
   advanced by real elapsed time across sessions from the `.sav` footer
   (`src/mbc/rtc.rs`), so simply coming back the next day works, and so does
   leaving the game running.
3. **Carry a Gen-1-legal party** — see the table above. The Time Capsule
   receptionist is present on **every** Pokémon Center 2F, at (13, 3), with no
   further gating, so once Bill has been met any Center will do.
4. On the **Gen 1 side**, the ordinary Cable Club rules: the player needs the
   Pokédex (`CableClubNPC` refuses without `EVENT_GOT_POKEDEX`) and picks
   TRADE CENTER.

### Status here

The cable is ready and needs nothing added; Gold boots and runs beside Blue with
the wire live (`examples/link_two.rs` with `--rom-b <gold.gbc>`, both screens
rendered in the capture).

**What is missing is a Gold save.** `~/roms` has the Gold and Silver cartridges but
no `.sav` for either, and prerequisite 1 is several hours of play that no
headless bot in this repository can do. A save that has met Bill — plus a day, plus
a Gen-1-legal party — is the one thing the end-to-end shiny check is waiting on.
