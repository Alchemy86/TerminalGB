# Driving the link cable from outside

[← Gen 1 area](README.md) · the journey: [`docs/link-trade.md`](../link-trade.md) ·
implemented in [`plugins/gen1-link/src/`](../../plugins/gen1-link/src/)

**The emulator can be the Game Boy on the other end of the cable.** A plugin answers the serial
port, so the game runs its real link protocol against something that does not exist — and a
Pokémon that arrives through it arrives the way any traded Pokémon does, inserted by the game
itself.

This page is the *mechanism*. [`docs/link-trade.md`](../link-trade.md) is the story of getting
it working and what to do when it hangs; [`docs/plugins.md`](../plugins.md) is the contract.

**There is a second way, and it needs no protocol at all.**
[`docs/link-cable.md`](../link-cable.md) joins *two whole emulated consoles* with a relaying
cable, so both ends are the genuine article. That is the route for anything nobody has written
a partner for — a link battle, or the Gen 1 ↔ Gen 2 Time Capsule, whose in-game prerequisites
are on that page.

---

## 1. The handshake, and the one choice that makes it easy

`ESTABLISH_CONNECTION_WITH_INTERNAL_CLOCK` and `ESTABLISH_CONNECTION_WITH_EXTERNAL_CLOCK` are
`$01` and `$02`; `USING_EXTERNAL_CLOCK` and `USING_INTERNAL_CLOCK` are `$01` and `$02` as well
(`constants/serial_constants.asm`). The values are shared on purpose: **the reply *is* the role
assignment.**

Answer the game's `$01` with `$02` and the game becomes clock **master**. That single decision
is what makes a virtual partner tractable. Every hardware or network bridge has to solve the
clocked-timing problem — two consoles agreeing on when a bit is on the wire. A software partner
that takes the *slave* role sidesteps it entirely: the reply is produced synchronously inside
the `SC = $81` write that started the transfer, so the exchange is as instantaneous as this
core's serial model already is, and there is **no timing tolerance to get wrong**.

`hSerialConnectionStatus` then holds the role, and the game consults it before doing anything
else at a Cable Club console.

---

## 2. `wLinkState` changes the game underneath you

| Symbol | Value | Meaning |
|---|---|---|
| `LINK_STATE_NONE` | `$00` | not using the link |
| `LINK_STATE_IN_CABLE_CLUB` | `$01` | in a Cable Club room |
| `LINK_STATE_START_TRADE` | `$02` | pre-trade initialisation |
| `LINK_STATE_START_BATTLE` | `$03` | pre-battle initialisation |
| `LINK_STATE_BATTLING` | `$04` | **in a link battle** |
| `LINK_STATE_TRADING` | `$32` | in a link trade |

`wLinkState` is at `$D12B` (read out of the cartridge; it is the operand of `ld a, [wLinkState]`
at the top of `ReadTrainer`). It is worth knowing three places it silently changes behaviour:

* **`ReadTrainer` returns immediately** when `wLinkState` is nonzero — so a link battle's enemy
  party is *not* built from the cartridge. It is whatever is already in `wEnemyMons`, which the
  received party block put there.
* **`LoadEnemyMonData` jumps to `LoadEnemyMonFromParty`** at `LINK_STATE_BATTLING`, for the same
  reason.
* **`BattleRandom` stops using the hardware divider** at `LINK_STATE_BATTLING` and draws from
  `wLinkBattleRandomNumberList`, a shared nine-entry list advanced by `x*5+1`. It has to: both
  consoles must roll identically or the battle desyncs. (This is also the only fully
  divider-free RNG in the game — see
  [battle control](battle-control.md#making-a-cinematic-reproducible).)

There is a fourth thing worth knowing about `wLinkState`, and it is what players notice
first: **while it is nonzero the player is in a room with no exit tile.** The Trade Center
and the Colosseum are the only maps in Red/Blue with no warp events at all, so answering the
cancel nybble `$6F` gets a player off the trade *screen* and no further — the way out of the
room is the START menu's RESET, which Gen 1 substitutes for SAVE exactly here. A partner
implementation that answers `$6F` correctly will still be blamed for trapping people if
nobody says that out loud. Measured, with the disassembly and the reproduction:
[**Leaving the Cable Club**](leaving-the-cable-club.md).

---

## 3. The trade protocol, in one table

Three blocks, each preceded by a run of `$FD` the receiver syncs on. Sizes are what
`CableClub_DoBattleOrTradeAgain` (`engine/link/cable_club.asm`) passes to `Serial_ExchangeBytes`:

| Block | Preamble | Data | Total |
|---|---|---|---|
| random-number list | 7 | 10 | 17 |
| party data | 6 | 418 | 424 |
| patch list | 3 | 197 | 200 |

**Each block costs `N + 1` transfers**, because `Serial_ExchangeBytes` re-sends its first byte
until it receives `$FD`. The whole stream is **644 bytes**. Off by one and the party block
shifts, and the game receives garbage without complaining.

The 418-byte party block mirrors the sender's WRAM verbatim — trainer name (11), party count
(1), species list + `$FF` (7), six 44-byte structures (264), six OT names (66), six nicknames
(66), three bytes of whatever followed.

### The patch list, and why it exists

`$FE` is the protocol's "no data" byte and the receiver **skips** it while copying. Real Pokémon
data contains `$FE` constantly (a stat of 254, an experience byte), so before sending, the game
walks the 264-byte Pokémon region, replaces every `$FE` with `$FF`, and records the 1-based
offset of each substitution. The list has **two parts**, split at offset 252, because an offset
is a single byte and `$FD` is reserved.

Getting this wrong does not fail loudly. It silently alters one byte of the Pokémon, which is
how a trade produces a mon with impossible stats. Names cannot contain `$FE` either — that is
the digit `8` — so a name containing one is refused rather than corrupted.

---

## 4. The proof that it is a real trade

The partner puts a level 5 Mew on the wire with **125 experience** — the exact total for level 5
on Mew's growth curve. Read the Pokémon out of the party afterwards and it has **135**.

Nothing in the plugin knows that number. Generation 1 gives a **traded** Pokémon boosted
experience, and the game applied it on the receiving side, of its own accord. A memory poke
would have left 125 sitting there.

That is the difference between arriving *through* the trade path and *beside* it, and it is
checked on every run of [`examples/gen1_trade.rs`](../../examples/gen1_trade.rs).

The link partner also declares **no `WRITE_MEMORY` capability at all**, so the plugin host would
refuse a poke even if the code attempted one. The read-only guarantee is the host's, not a
matter of trust.

---

## 5. The Colosseum: a battle we did not agree to

> **Status: derived from the disassembly, not yet confirmed against our own wire trace.** A
> scout is investigating why a link *battle* started against a trade-only virtual partner. This
> section is what `pret/pokered` says should happen; the finding belongs here, and if it turns
> out to be a timing fact rather than a protocol one it is a fact about the cartridge and earns a
> line on
> [AtlasGB's sharp edges](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/sharp-edges.md)
> instead.

**The disassembly's answer is that the choice is not on the wire at all — it is the map.**
`CableClubLeftGameboy` (`engine/pokemon/bills_pc.asm`) reads:

```
CableClubLeftGameboy::
	ldh a, [hSerialConnectionStatus]
	cp USING_EXTERNAL_CLOCK
	ret z                          ; the other seat's console; do nothing
	ld a, [wSpritePlayerStateData1FacingDirection]
	cp SPRITE_FACING_RIGHT
	ret nz                         ; must be facing the console
	ld a, [wCurMap]
	cp TRADE_CENTER
	ld a, LINK_STATE_START_TRADE
	jr z, .next
	inc a ; LINK_STATE_START_BATTLE
.next
	ld [wLinkState], a
```

`wCurMap` decides. In `TRADE_CENTER` the console starts a trade; on **any other map** — which
in practice means `COLOSSEUM` — it starts a *battle*. The receptionist's menu is what warps the
player to one room or the other, and the partner is never consulted. So a virtual partner that
only implements trading will still be taken into a link battle if the player is standing in the
Colosseum, and the first thing it will be asked for is the same 644-byte stream with a
different meaning attached.

Then, at the bottom of `CableClub_DoBattleOrTradeAgain`:

```
	ld a, [wLinkState]
	cp LINK_STATE_START_BATTLE
	ld a, LINK_STATE_TRADING
	ld [wLinkState], a
	jr nz, .trading
	ld a, LINK_STATE_BATTLING
	ld [wLinkState], a
	ld a, OPP_RIVAL1
	ld [wCurOpponent], a
	predef InitOpponent
```

Two things worth noticing, because they connect this page to the rest of the area:

* **A link battle is started by writing `wCurOpponent`** — the very same byte
  [battle control](battle-control.md) writes. The link path and the injection path converge on
  one trigger.
* **The enemy party for that battle is `wEnemyMons`**, left exactly as the received party block
  put it, because `ReadTrainer` bails on a nonzero `wLinkState`. So a link partner can produce a
  battle against an arbitrary team **without writing memory at all** — which is a strictly
  stronger claim than injection, and a real avenue if a "fight this exact team, legitimately"
  feature is ever wanted.

**What the scout should nail down**, since the above is disassembly-reading and not
observation:

1. Which map the session was actually on (`wCurMap`, `$D35E`) at the moment the console was
   used — the disassembly says that alone decides it.
2. Whether `wLinkState` went to `$03` and then `$04`, which would confirm the branch above and
   rule out anything on the wire.
3. What the partner answered during the 17-byte random-number block, because a link battle
   *uses* those nine seeds as its RNG and a trade ignores them. A partner that sends filler
   there produces a battle whose dice the two sides disagree about.
4. Whether the game reached `HealParty` — the line right after `InitOpponent` — because that is
   a visible, checkable side effect of having gone down the battle branch.

---

## 6. What this suggests next

* **A link *battle* partner.** Everything above says the pieces exist: the role handshake is
  solved, the party block is already encoded, and the enemy team is the one we sent. What is
  missing is the per-turn exchange and agreeing on `wLinkBattleRandomNumberList`.
* **A bridge rather than a partner.** `PokemonSource`
  ([`plugins/gen1-link/src/spec.rs`](../../plugins/gen1-link/src/spec.rs)) deliberately separates *where a
  Pokémon comes from* from *how it is traded*. A file, a converted Generation 3 save, a remote
  peer and a live partner are then interchangeable — which is what would make cross-generation
  and remote trading the same feature rather than two.
* **The messages are already wire-shaped.** Every plugin event serialises to the same NDJSON
  the telemetry stream uses, so an external-process transport is a change of *pipe*, not of
  contract. Be honest about the present, though: today a plugin is an in-process Rust trait, and
  you cannot write one in Python.
