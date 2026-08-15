# The Time Capsule — the only door between the generations

[← Gen 2 area](README.md)

The Time Capsule is Gold and Silver's one sanctioned link to Red, Blue and Yellow. Every Gen 2
fact in this repository was found on the way through it, so this page is both the reference for
the mechanism and the reason the rest of the area exists.

**This page is the game side.** The cable-side record of the run — transfer counts, screenshots,
what the two cores did to each other — is [`docs/link-cable.md`](../link-cable.md) §8, and is
not repeated here.

---

## 1. Gen 2 refuses a Gen 1 partner at the *ordinary* trade desk

This is worth stating first because it is the thing that sends people looking for a bug.

Gold's normal trade receptionist reads `wOtherPlayerLinkMode` and branches to `.LinkedToFirstGen`
→ *"You can't link to the past"* (`maps/Pokecenter2F.asm`). The Time Capsule receptionist is the
mirror image: she **requires** that same byte to be zero.

So there is no "just link them and see". A Gen 1 console reaching a Gen 2 game has exactly one
legitimate destination, and picking the wrong desk is a refusal, not a failure.

---

## 2. The wire is Gen 1's protocol, byte for byte

**In Time Capsule mode Gen 2 speaks Gen 1's link protocol unchanged** (`engine/link/link.asm`):
the same `Serial_ExchangeBytes` three-block exchange — random-number preamble, party block,
patch lists. `Link_PrepPartyData_Gen1` builds the party block in **Gen 1 format**: six `$FD`
preamble bytes, the 11-byte player name, the party count, the species list terminated by `$FF`,
then `REDMON_STRUCT_LENGTH` (**44**-byte) structs, then OT names and nicknames.
`ConvertMon_2to1` maps species indices on the way out, and `ConvertMon_1to2` /
`Link_ConvertPartyStruct1to2` map them back on the way in.

**This single fact decides an architecture**, and it is the clearest example in the repository of
why understanding a mechanism beats collecting recipes:

| Approach | What the Time Capsule costs it |
|---|---|
| **A relaying cable** — two real cores, bytes passed between them ([`docs/link-cable.md`](../link-cable.md)) | **Nothing at all.** The wire does not know what generation it is carrying. |
| **An impersonating partner** — one core, software pretending to be the other console ([`plugins/gen1-link`](../../plugins/gen1-link/)) | A **second protocol implementation**, plus both conversion tables, plus a Gen 2 party encoder. |

The relay carried a real Gen 1 ↔ Gen 2 trade with **no Time Capsule support written for it**.
That is not luck; it is the direct consequence of Gen 2 choosing to speak the old protocol
rather than translate at the wire.

---

## 3. The in-game prerequisites, in order

`LinkReceptionistScript_TimeCapsule` gates on three things, and **two of the three read the
opposite way from what you would guess.**

### (a) Bill must have been met — which means `EVENT_MET_BILL` **clear**

Bill's scene (`EcruteakPokcenter1FBillActivatesTimeCapsuleScript`,
`maps/EcruteakPokecenter1F.asm`) runs on entering Ecruteak City's Pokémon Center and does
`clearevent EVENT_MET_BILL` / `setflag ENGINE_TIME_CAPSULE`. So the flag named "met Bill" is
**set until you meet him**. The receptionist refuses while it is set.

Ecruteak is the fourth gym town: roughly three badges and several hours of play.

### (b) A day must have passed — which means the daily flag **clear**

`ENGINE_TIME_CAPSULE` is a **daily** flag, mapped into `wDailyFlags1` by
`data/events/engine_flags.asm`. Bill *sets* it, and the receptionist refuses while it is set;
`CheckDailyResetTimer` (`engine/overworld/time.asm`) zeroes `wDailyFlags1` once a day has elapsed
on the cartridge clock. Until then: *"I'm sorry — the TIME CAPSULE is being adjusted."*

So "the flag Bill sets, cleared" is literally the game's way of writing *"and then a day went
by"*.

**This emulator handles the wait correctly without anything special.** The MBC3 clock is driven
by *emulated* time while running and advanced by real elapsed time across sessions from the
`.sav` footer (`src/mbc/rtc.rs`), so coming back the next day works, and so does leaving the game
running.

### (c) A party Gen 1 can represent

`CheckTimeCapsuleCompatibility` (`engine/link/link.asm`) inspects the **Gen 2 player's own**
party before the link opens:

| result | meaning | test in source |
|---|---|---|
| 0 | the party is fine | — |
| 1 | a Pokémon introduced in Gen 2 | species ≥ `JOHTO_POKEMON` (152, Chikorita and up) |
| 2 | a move introduced in Gen 2 | move index > `STRUGGLE` (165) |
| 3 | a Pokémon is holding mail | `ItemIsMail` |

**All three Johto starters are Gen 2 species**, so a Gold player who has played normally *cannot*
use the Time Capsule without leaving their starter in the PC. This is the rule that trips
everybody, and it is why a real save is usually rejected wholesale — see §6.

### (d) On the Gen 1 side: the ordinary Cable Club rules

The Gen 1 player needs the Pokédex (`CableClubNPC` refuses without `EVENT_GOT_POKEDEX`) and picks
**TRADE CENTER**. Nothing about the Time Capsule changes the Gen 1 half; from Red or Blue's point
of view it is a normal trade with a normal partner.

The Time Capsule receptionist stands on **every** Pokémon Center 2F, at (13, 3), with no further
gating of her own — so once Bill has been met, any Center will do
([memory map §2](memory-map.md#map-ids-and-coordinates-that-have-actually-been-used)).

---

## 4. Crafting a save that can use it

Reaching Ecruteak legitimately is several hours of play, so
[`examples/gen2_timecapsule.rs`](../../examples/gen2_timecapsule.rs) builds the state instead —
the same class of work as [Gen 1 battle control](../gen1/battle-control.md), which writes game
state to start a battle.

The mechanics are [the save file](the-save-file.md); the four edits are:

1. `EVENT_MET_BILL` **cleared** — §3(a).
2. `DAILYFLAGS1_TIME_CAPSULE_F` **cleared** — §3(b).
3. A party the compatibility check accepts — one Gen 1 species, Gen 1 moves, **no held item**, in
   Gen 2's [48-byte `party_struct`](memory-map.md#the-48-byte-party_struct-as-far-as-this-repository-has-established-it).
4. The 16-bit `sGameData` checksum recomputed.

**The player is not moved by editing the save**, for the reason in
[the save file §3](the-save-file.md#3-you-cannot-teleport-the-player-by-editing-the-save): a
continue runs `LoadMapAttributes_SkipObjects` and does not load map objects. The bot walks to the
bedroom stairs and the *warp destination* is overridden at run time instead.

**The verdict is the receptionist's own words, not a flag check.** The tool's success condition
is the game saying *"Would you like to trade across time?"* and then waiting for a partner; its
failure conditions are *"the TIME CAPSULE is being adjusted"* (a prerequisite unmet) and the
party rejection message. A save that looks right in a hex dump and is refused in game is worth
nothing, so the check is what the game says.

```
cargo run --release --features image,plugin-gen1-link,plugin-trainer \
    --example gen2_timecapsule -- --rom <gold.gbc> --make-save
cargo run --release --features image,plugin-gen1-link,plugin-trainer \
    --example gen2_timecapsule -- --rom <gold.gbc> \
    --verify target/gen2-timecapsule/timecapsule.sav
```

---

## 5. The four things the completed trade needed

Each was learned by a run failing, and each is a fact about the *game*, not about the emulator.

### (a) The link window is 767 frames, and it opens before the save prompt

`LinkReceptionistScript_TimeCapsule` runs `WaitForLinkedFriend` **before** the save prompt. That
special (`engine/link/link.asm`) arms Gold as an externally clocked slave answering
`SB = USING_INTERNAL_CLOCK` (`$02`) for `wLinkTimeoutFrames` = `$2FF` = **767 frames**, about 13
seconds — and `$02` is exactly the byte Gen 1's master handshake is looking for.

So the entire choreography between the two consoles is **one gate**: *the Gen 1 side may not
initiate until the Gen 2 side's player is at the desk.* Both sides retry on a miss, and Gen 1's
whole receptionist sequence fits inside the window.

**Without the gate, it fails in a way that looks like the cable is broken and is not.** A Gen 2
console answers handshake bytes from its **overworld** serial handler, so an ungated Gen 1
partner completes a handshake against a Gold that is still walking around, then enters the Trade
Center alone and desyncs. Two separate *processes* cannot see each other's game state and so
cannot implement this gate at all; that is why the completed run is one process holding one bot
back.

### (b) A real save fails Gold's RTC check, and the recovery flow eats the joypad

Continuing a genuine save produced *"The clock's time may be wrong."* and then the whole clock-set
screen. See [reading the screen §1](reading-the-screen.md#1-the-overworld-contains-no-letters-at-all--and-that-is-a-better-test-than-a-word-list)
for what that cost and for the test that survives it.

### (c) Which seat is yours is a byte, not geometry

The two consoles are at (4, 4) and (5, 4) and are **direction-restricted** bg events, so the
seats are (3, 4) facing right and (6, 4) facing left (`maps/TimeCapsule.asm`). The partner's
avatar — a `SPRITE_CHRIS` object — occupies one of them, placed by
`TimeCapsuleSetWhichChrisCallback` via `CableClubCheckWhichChris` (`engine/link/link.asm`), which
branches on **`hSerialConnectionStatus` at `$FFCD`**.

**Gen 1's `hSerialConnectionStatus` is `$FFAA`.** Different byte, same name, same purpose — and
Gen 1's Cable Club has exactly the same "which seat" problem
([Gen 1 link](../gen1/link-from-outside.md)). Read the byte:

| `hSerialConnectionStatus` | this console took | partner's avatar | our seat |
|---|---|---|---|
| `$02` `USING_INTERNAL_CLOCK` | the internal clock | (6, 4) | **(3, 4)**, facing right |
| `$01` `USING_EXTERNAL_CLOCK` | the external clock | (3, 4) | **(6, 4)**, facing left |

Guessing from the arrival coordinate instead walked the bot into the NPC, where it stood blocked
for a whole 60,000-frame run while the partner's screen said *"PLEASE WAIT!"* — a failure that
produces no error anywhere and looks exactly like a protocol problem.

### (d) The trade UI is Gen 1's, redrawn

pokegold carries the literal `db "STATS     TRADE@"` and `db "Trade completed!@"`
(`engine/link/link.asm`), and the selection screen is the same two-column layout with CANCEL. So
**one bot shape drives both ends** — which is the second time on this page that Gen 2 reusing
Gen 1's design halves the work.

One consequence to respect: **while the selection screen is up, a direction key moves the
cursor**, not the player. Any seat-facing logic must stop the moment `CANCEL` is on screen, or its
face-the-console presses walk the selection onto the wrong Pokémon.

### And once the trade completes

**Once the receptionist has sent you through, stop navigating.** The player is warped into the
Time Capsule room, so an "am I beside the desk?" test goes false and a bot that keeps steering
walks straight back out of the link. Measured — mid-handshake.

**Leaving afterwards is asymmetric, and the Gen 1 player will think it is the emulator's
fault.** Gold's Time Capsule room has two ordinary warps back to `POKECENTER_2F`
(`TimeCapsule_MapEvents`, `pokegold/maps/TimeCapsule.asm`), so the Gen 2 player walks out down
the stairs. Gen 1's Trade Center has **no warp events at all**: its player has to get both
sides to choose CANCEL and then press START → RESET, which is what Red/Blue substitutes for
SAVE for the whole visit. Neither side is broken. Measured, with the reproduction:
[`docs/gen1/leaving-the-cable-club.md`](../gen1/leaving-the-cable-club.md).

---

## 6. The shiny rule, read out of the cartridge, not inferred

The reason for the whole chain was to check the trainer panel's *"would be shiny if traded to
Gen 2"* verdict. It is settled, and **more strongly than one trade could settle it.**

From the cartridge-identical build, `engine/gfx/color.asm`:

```
DEF SHINY_ATK_MASK EQU %0010
DEF SHINY_DEF_DV   EQU 10
DEF SHINY_SPD_DV   EQU 10
DEF SHINY_SPC_DV   EQU 10

CheckShininess:
; Check if a mon is shiny by DVs at bc.  Return carry if shiny.
```

Defence, speed and special all exactly 10, and the attack DV with bit 1 set — one of
2, 3, 6, 7, 10, 11, 14, 15. Eight attack values out of sixteen with three fixed nybbles is
8 / 65,536 = **1 in 8,192**, the familiar odds. The canonical pair is `ATKDEFDV_SHINY` = `$EA` /
`SPDSPCDV_SHINY` = `$AA`.

**Reading the rule out of the cartridge covers all 65,536 DV pairs by construction; a trade
samples one.** That is the argument, and it is why this is not re-litigated when a new trade
happens to agree.

Three things follow, and they are the whole reason a Gen 1 tool may talk about Gen 2 shininess at
all:

* **A Gen 1 Pokémon's DVs cross the Time Capsule unchanged**, so the answer is available *before*
  the trade. Verified on the wire: the Abra arrived in Gold with DVs byte-identical.
* **Gen 1 has no shiny Pokémon at all.** The verdict is therefore always phrased *"would be
  shiny if traded to Gen 2"*, never *"is shiny"* — a rule enforced by tests in
  [the trainer plugin](../trainer.md) and [the shiny hunter](../shiny-hunt.md), which render every
  string at every width and grep it.
* **There is one implementation of the rule** — `derived::shiny_if_traded` in
  [`plugins/trainer/src/derived.rs`](../../plugins/trainer/src/derived.rs) — shared by the party
  panel's `★`, the wild-encounter banner and the hunter's target. A second copy is how two parts
  of a program come to disagree about the same Pokémon.

**Closed by Gold's own renderer, not only by our reading of DVs.** After the trade the received
Abra's stats screen shows it drawn in **pink**, and the two colours in the captured frame are
exactly the shiny half of `PokemonPalettes` (`data/pokemon/palettes.asm`) through this emulator's
colour correction, with neither normal-palette colour anywhere on screen.
`CheckShininess` ran *inside the cartridge* and picked the shiny table. Full pixel-level record in
[`docs/link-cable.md`](../link-cable.md) §8.

---

## 7. Quirks and honest gaps

**The catch-rate byte becomes a held item.** A Pokémon arriving from Gen 1 holds an item whose id
is its Gen 1 *catch rate* — the Abra arrived holding item 200, and Abra's Gen 1 catch rate is
200. This is the documented Time Capsule item mechanic, not a bug in the conversion, and it falls
out of the two structures overlapping differently
([memory map](memory-map.md#the-48-byte-party_struct-as-far-as-this-repository-has-established-it):
Gen 2 puts the held item at offset 1, Gen 1 puts catch rate at 7).

**A real Gold save is usually rejected for its party, not its flags.** The captain's cartridge
turned out to have *both* Time Capsule gates already open — Bill met, daily flag clear — and a
party of six Johto species, which `CheckTimeCapsuleCompatibility` refuses wholesale. The party
patch on the staged copy is what made it usable; the gate edits were no-ops. Worth remembering
before diagnosing a refusal as a flag problem.

**Not established here:**

* **The reverse gate.** Nothing has checked what Gen 2 does about a Gen 1 party containing
  something *it* cannot represent, or whether one exists.
* **Yellow.** The Time Capsule accepts Yellow in principle; nothing here has tried it.
* **Silver, and Crystal.** See [which cartridge](README.md#which-cartridge).
* **Mail, eggs and the other Gen 2 concepts** the check screens for beyond the four results
  above.

---

## What this suggests next

* **A relay beats an impersonator whenever the far end is a real game.** §2 is the general form:
  before writing a protocol implementation, check whether the thing you are talking to already
  speaks one you have.
* **"The window is 767 frames and both sides retry" is a design pattern**, not a Pokémon fact.
  Any two-console rendezvous reduces to *one* gate as soon as you know how wide the window is —
  and finding the width is one grep of the disassembly.
* **The generation boundary is a data-conversion oracle.** `ConvertMon_1to2` is a mapping somebody
  else wrote, verified, and shipped in a cartridge; anything that needs to translate Gen 1 data
  forward can be checked against it by running a trade, rather than against a table copied off a
  wiki.
