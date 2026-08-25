# The Time Capsule — driving it from this emulator

[← Gen 2 area](README.md) ·
[AtlasGB: the Time Capsule](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/time-capsule.md)

> **The mechanism itself moved to AtlasGB** — the trade-desk refusal, the fact that Gen 2 speaks
> Gen 1's link protocol byte for byte, the three in-game prerequisites, the 767-frame link
> window, the seat table, the shiny rule and the quirks/honest gaps are all now on
> [AtlasGB's `time-capsule.md`](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/time-capsule.md).
> What stayed here is this repository's own **architecture choice**, its **bot walkthrough**,
> and the **two-process implementation notes** those facts led to.

**This page is the tooling side.** The cable-side record of the run — transfer counts,
screenshots, what the two cores did to each other — is [`docs/link-cable.md`](../link-cable.md)
§8, and is not repeated here.

---

## The architecture choice this mechanism decides

**Gen 2 speaking Gen 1's link protocol unchanged decides an architecture**, and it is the
clearest example in this repository of why understanding a mechanism beats collecting recipes:

| Approach | What the Time Capsule costs it |
|---|---|
| **A relaying cable** — two real cores, bytes passed between them ([`docs/link-cable.md`](../link-cable.md)) | **Nothing at all.** The wire does not know what generation it is carrying. |
| **An impersonating partner** — one core, software pretending to be the other console ([`plugins/gen1-link`](../../plugins/gen1-link/)) | A **second protocol implementation**, plus both conversion tables, plus a Gen 2 party encoder. |

The relay carried a real Gen 1 ↔ Gen 2 trade with **no Time Capsule support written for it**.
That is not luck; it is the direct consequence of Gen 2 choosing to speak the old protocol
rather than translate at the wire.

---

## Crafting a save that can use it

Reaching Ecruteak legitimately is several hours of play, so
[`examples/gen2_timecapsule.rs`](../../examples/gen2_timecapsule.rs) builds the state instead —
the same class of work as [Gen 1 battle control](../gen1/battle-control.md), which writes game
state to start a battle.

The mechanics (the three prerequisites, the party struct) are on
[AtlasGB's `time-capsule.md`](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/time-capsule.md);
the four edits this tool makes are: `EVENT_MET_BILL` cleared, `DAILYFLAGS1_TIME_CAPSULE_F`
cleared, a party the compatibility check accepts (one Gen 1 species, Gen 1 moves, no held item),
and the 16-bit `sGameData` checksum recomputed.

**The player is not moved by editing the save** — a continue runs `LoadMapAttributes_SkipObjects`
and does not load map objects, per
[AtlasGB's save-file page](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/the-save-file.md).
The bot walks to the bedroom stairs and the *warp destination* is overridden at run time instead.

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

## The two-process implementation, and what it needed

**This emulator handles the day-boundary wait correctly without anything special.** The MBC3
clock is driven by *emulated* time while running and advanced by real elapsed time across
sessions from the `.sav` footer (`src/mbc/rtc.rs`), so coming back the next day works, and so
does leaving the game running.

**Without the link-window gate, it fails in a way that looks like the cable is broken and is
not.** A Gen 2 console answers handshake bytes from its **overworld** serial handler, so an
ungated Gen 1 partner completes a handshake against a Gold that is still walking around, then
enters the Trade Center alone and desyncs. **Two separate *processes* cannot see each other's
game state and so cannot implement this gate at all** — that is why the completed run here is
one process holding one bot back until the other side is at its desk, rather than two
independent programs.

**A real save fails Gold's RTC check on continue, and the recovery flow eats the joypad.**
Continuing a genuine save produced *"The clock's time may be wrong."* and then the whole
clock-set screen. See [reading the screen](reading-the-screen.md#a-word-list-is-the-wrong-test-and-the-overworld-proved-it)
for what that cost and for the test that survives it.

**The trade UI is Gen 1's, redrawn**, so one bot shape drives both ends — the second time this
mechanism reusing Gen 1's design halves the work. One consequence to respect: **while the
selection screen is up, a direction key moves the cursor**, not the player. Any seat-facing logic
must stop the moment `CANCEL` is on screen, or its face-the-console presses walk the selection
onto the wrong Pokémon.

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

**Gold saves TWICE around a Time Capsule trade, and knowing which write you are looking at is the
difference between "the trade did not happen" and "the trade happened".** On the way in,
`LinkReceptionistScript_TimeCapsule` (`maps/Pokecenter2F.asm`) runs `Text_MustSaveGame` →
`special TryQuickSave`; on completion, `LinkTrade` (`engine/link/link.asm`) branches
`cp LINK_TIMECAPSULE / jr z, .save` straight into `farcall SaveAfterLinkTrade`
(`engine/menus/save.asm`: `SavePokemonData` + `SaveChecksum` + `SaveBackupPokemonData` +
`SaveBackupChecksum`) **before** it prints "Trade completed!". `Script_LeftTimeCapsule` saves
nothing further. So a Gold `.sav` whose party is the *pre*-trade one after a completed trade means
the second write did not land — not that Gen 2 declines to save. Measured on a real
hand-driven trade, where Gold's file was a consistent pre-link snapshot standing at (13, 4) below
the receptionist and the reason the second write was missing is **not established**:
[`docs/link-cable.md`](../link-cable.md) §8.

---

## What this suggests next

* **A relay beats an impersonator whenever the far end is a real game.** The architecture-choice
  table above is the general form: before writing a protocol implementation, check whether the
  thing you are talking to already speaks one you have.
* **"The window is 767 frames and both sides retry" is a design pattern**, not a Pokémon fact.
  Any two-console rendezvous reduces to *one* gate as soon as you know how wide the window is —
  and finding the width is one grep of the disassembly.
* **The generation boundary is a data-conversion oracle.** `ConvertMon_1to2` is a mapping somebody
  else wrote, verified, and shipped in a cartridge; anything that needs to translate Gen 1 data
  forward can be checked against it by running a trade, rather than against a table copied off a
  wiki.
