# Leaving the Cable Club

**Short answer for a player who is stuck right now: press START and choose RESET.**
The Cable Club rooms in Pokémon Red and Blue have no exit tile. That is the cartridge, not
the emulator, and Generation 1 provides RESET in the START menu precisely because of it.
Your save is safe — the game saved when you walked in, and again the instant any trade
completed.

---

## The report

> *"For trading, one Gen 1 — there is simply no leaving the damn room. Cancelling doesn't
> make you leave. I'm sure if both players choose cancel you SHOULD leave, but Blue is
> trapped in the void trade room. Gold has a door to walk out so… but Blue cannot leave and
> I think it's us."*

Three different claims live inside that, and they have three different answers. All three
were measured on the retail cartridge rather than argued from the state diagram — see
[Reproducing it](#reproducing-it) below.

| Claim | Answer |
|---|---|
| Cancelling alone doesn't let you out | **Correct, and it is the cartridge** — both players must cancel |
| If both cancel you should leave | **Correct — you leave the trade *screen*** |
| Blue is still trapped in the room | **Correct, and it is the cartridge** — the room has no door; RESET is the exit |
| "I think it's us" | **No.** Nothing of ours is in this path |

---

## 1. One player cancelling is not enough, by design

`.cancelMenuItem_APressed` ([`engine/link/cable_club.asm`][cable_club]) is six instructions
and the disassembly comments the decisive one itself:

```asm
.cancelMenuItem_APressed
	ld a, '▷'                          ; hollow cursor: "A was pressed"
	ldcoord_a 1, 16
	ld a, $f
	ld [wSerialExchangeNybbleSendData], a
	call Serial_PrintWaitingTextAndSyncAndExchangeNybble
	ld a, [wSerialSyncAndExchangeNybbleReceiveData]
	cp $f                              ; did the other person choose Cancel too?
	jr nz, .cancelMenuItem_Loop
	; fall through to ReturnToCableClubRoom
```

`.cancelMenuItem_Loop` redraws the filled cursor and goes back to waiting for a button. So a
player whose partner is still browsing presses A on CANCEL for ever, and the game keeps
rendering, the music keeps playing and the clock keeps running the whole time. It is not a
hang and it does not look like one.

**Generation 2 is the same rule, spelled in Gen 2's names.**
`LinkTradeOTPartymonMenuCheckCancel` ([`pokegold/engine/link/link.asm`][gold_link]) writes
`$F` to `wPlayerLinkAction`, and `LinkTransfer` puts `SERIAL_TIMECAPSULE | $F` = `$6F` on the
wire — byte for byte what Gen 1 sends — then requires `$F` back before falling through to
`ExitLinkCommunications`. So a Time Capsule partner is under exactly the same obligation, and
**there is nothing for an emulator in the middle to relay or translate**. Both consoles are
speaking the same protocol correctly; they are simply waiting for their players.

> **Grade, per [this area's standards](README.md#standards-for-this-area):** the Gen 1 half of
> this page is *measured on the retail cartridge*. The Gen 2 half is **read out of the
> disassembly and not separately observed** — a Gold-side cancel has not been driven here,
> because doing so needs a crafted Time Capsule save and the Gen 1 side's requirement does not
> depend on who the partner is. Say "derived" if you quote it.

## 2. Leaving the trade screen is not leaving the room

What the fall-through reaches is `ReturnToCableClubRoom`, which sets `wDestinationWarpID` to
`-1` and calls `LoadMapData`. `wCurMap` does not change: it reloads the **same** map and puts
the player back on its floor.

And that floor has nothing on it. `data/maps/objects/TradeCenter.asm` is, in full:

```asm
TradeCenter_Object:
	db $e ; border block

	def_warp_events        ; <- nothing under it

	def_bg_events

	def_object_events
	object_event  2,  2, SPRITE_RED, STAY, ANY_DIR, TEXT_TRADECENTER_OPPONENT
```

`def_warp_events` with no entries means `wNumberOfWarps` reads **0** at run time — confirmed
by reading it out of the running game. The Colosseum is the same. These are the only maps in
Red/Blue with no way out, and there is no map script that provides one either: the whole of
`scripts/TradeCenter.asm` faces the opponent sprite the right way and returns.

This is the half the report is actually about, and it is why the two cartridges behave
differently. Gold's room really does have a door — `TimeCapsule_MapEvents`
([`pokegold/maps/TimeCapsule.asm`][gold_map]) declares two of them:

```asm
	def_warp_events
	warp_event  4,  7, POKECENTER_2F, 4
	warp_event  5,  7, POKECENTER_2F, 4
```

one behind each seat, back down to the Pokémon Center's first floor. So the Gen 2 player
walks out and the Gen 1 player cannot. **That asymmetry is two cartridges, not a bug.**

## 3. The exit the cartridge does provide: START → RESET

`StartMenu_SaveReset` ([`engine/menus/start_sub_menus.asm`][start_menu]):

```asm
StartMenu_SaveReset::
	ld a, [wStatusFlags4]
	bit BIT_LINK_CONNECTED, a
	jp nz, Init                 ; <- soft reset
	predef SaveMenu
```

`BIT_LINK_CONNECTED` is set for the whole Cable Club visit, which is why the START menu
there reads

```
POKéDEX / POKéMON / ITEM / <name> / RESET / OPTION / EXIT
```

with **RESET** where SAVE normally is. That is not a warning label; it is the door.

It is safe, and deliberately so. The receptionist makes you save on the way in ("you have to
save"), and `TradeCenter_Trade` calls `predef SavePartyAndDexData` immediately after "Trade
completed!" with pokered's own comment beside it: **`; this allows reset into Pokecenter`**.
So a reset returns you to the Pokémon Center holding everything you traded for.

Quitting the emulator and relaunching does the same thing for the same reason.

### What to press, in order

1. If you are on the trade selection screen, **both** players move to CANCEL and press A.
   (Either player can also press Up to go back to the party list; nothing is lost.)
2. Back in the room, press **START**.
3. Choose **RESET**, press A.
4. The console restarts. Choose CONTINUE. You are in the Pokémon Center, with your party.

---

## Reproducing it

```text
cargo run --release --features image,plugin-gen1-link,plugin-trainer \
    --example gen1_cancel_exit -- --rom <blue.gb> --save <cableclub.sav>
```

[`examples/gen1_cancel_exit.rs`](../../examples/gen1_cancel_exit.rs) puts two whole cores on a
[`LinkCable`](../../src/link.rs) — nothing of ours impersonates a console — drives both into
the Trade Center, and then walks the three claims one at a time. Measured output, Blue against
Blue:

```text
1. one player cancelling alone: STUCK, as pokered says it should be
                                (console A pressed A on CANCEL 60 times)
2. both players cancelling:     LEFT THE TRADE SCREEN
3. the room itself:             wNumberOfWarps = 0 — the Trade Center has NO DOOR
4. START menu in the room:      offers RESET in place of SAVE
5. after RESET → CONTINUE:      out of the Cable Club, standing on map 0x29
                                (the Pokémon Center it saved in)
faults: A None B None
```

Four bytes make the middle of that observable, and **two of them were read out of the
cartridge rather than looked up**: the byte string `EA E1 C4 3E 0F EA lo hi` — the hollow
cursor written to `wTileMap(1,16)`, then `ld a,$F` into the send register — occurs exactly
once in the ROM, so its operand is the address.

| Byte | What it says |
|---|---|
| `wLinkState` `$D12B` | `$32` = the trade screen is up; `$01` = back in the room; `$00` = out of the Cable Club |
| `wSerialExchangeNybbleSendData` `$CC42` | `$0F` once the player has pressed A on CANCEL, and it is a **latch** |
| `wSerialSyncAndExchangeNybbleReceiveData` `$CC3D` | `$0F` is the only value that opens the door; `$FE` is "nothing came back" |
| `wTileMap(1,16)` `$C4E1` | `$ED` (`▶`) parked on CANCEL; `$EC` (`▷`) inside the sync round |

Watching that last one is what turns "the player says nothing happens" into an observation:
`▷` with `wLinkState` at `$32` *is* the stuck state, and it is visible from outside the game.

### What this rules out

* **Not the transport.** The same run's cable reports 1,539 transfers, 1 unanswered, 0 torn,
  and the cancel round uses the identical `Serial_SyncAndExchangeNybble` path that the trade
  itself depends on.
* **Not a plugin.** Only the trainer plugin attaches without being asked, and it declares no
  `WRITE_MEMORY`. `gen1-link` (the virtual partner) and `gen1-lobby` need an explicit
  `plugins = …`, and neither was in the reported session.
* **Not the save guard.** Neither console faulted across the cancel or the reset, so no
  battery was poisoned by legitimately resetting out of the room.

---

## What the emulator now says about it

A room with no exit and no explanation reads, from the inside, as "the emulator has trapped
me". So the emulator says so on the way in:

* [`plugins/trainer/src/cableclub.rs`](../../plugins/trainer/src/cableclub.rs) watches
  `wLinkState` with the panel closed — one byte at about 10 Hz, and a second byte only once
  that one is non-zero — and names the room.
* The terminal frontend toasts the advice once on entry (`App::announce_cable_club`),
  covering both obstacles: *both* players must cancel, and the room itself needs START →
  RESET.
* The trainer plugin's Status row shows `Cable Club` / `Cable Club trade` while you are in
  there, outranking the wild-encounter verdict — being unable to leave is the one thing on
  that row a player can act on.

None of it writes to the game. The plugin declares `OBSERVE | READ_MEMORY` and the host
refuses anything else.

---

## Sources

| | |
|---|---|
| `.cancelMenuItem_APressed`, `ReturnToCableClubRoom`, `TradeCenter_Trade` | [`pokered/engine/link/cable_club.asm`][cable_club] |
| `StartMenu_SaveReset` | [`pokered/engine/menus/start_sub_menus.asm`][start_menu] |
| The Trade Center's (absent) warps | [`pokered/data/maps/objects/TradeCenter.asm`][objects] |
| `LINK_STATE_*` | [`pokered/constants/serial_constants.asm`][serial_constants] |
| `LinkTradeOTPartymonMenuCheckCancel`, `LinkTransfer` | [`pokegold/engine/link/link.asm`][gold_link], [`pokegold/home/serial.asm`][gold_serial] |
| The Time Capsule room's two warps | [`pokegold/maps/TimeCapsule.asm`][gold_map] |

Related pages: [Driving the link cable](link-from-outside.md) ·
[Two consoles, one wire](../link-cable.md) · [The Time Capsule](../gen2/time-capsule.md) ·
[Sharp edges](sharp-edges.md)

[cable_club]: https://github.com/pret/pokered/tree/master/engine/link/cable_club.asm
[start_menu]: https://github.com/pret/pokered/tree/master/engine/menus/start_sub_menus.asm
[objects]: https://github.com/pret/pokered/tree/master/data/maps/objects/TradeCenter.asm
[serial_constants]: https://github.com/pret/pokered/tree/master/constants/serial_constants.asm
[gold_link]: https://github.com/pret/pokegold/tree/master/engine/link/link.asm
[gold_serial]: https://github.com/pret/pokegold/tree/master/home/serial.asm
[gold_map]: https://github.com/pret/pokegold/tree/master/maps/TimeCapsule.asm
