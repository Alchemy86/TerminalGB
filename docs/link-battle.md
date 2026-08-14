# The Cable Club Colosseum: a link battle, and the crash that destroyed saves

This page covers two things that turned out to be the same story: **a guard that
makes it impossible for a crashed game to overwrite a battery save**, and **the
Gen 1 link partner learning to take part in a link battle** instead of answering
one with trade answers.

Companion pages: [`link-trade.md`](link-trade.md) (the trade the partner already
did), [`plugins.md`](plugins.md) (the contract it implements),
[`components/mbc.md`](components/mbc.md) (the battery).

---

## 1. What happened

The Cable Club has two rooms and **one function drives both of them**.
`CableClub_DoBattleOrTrade` (`pret/pokered`, `engine/link/cable_club.asm`) says
so in its own header comment: it swaps random numbers, trainer names and party
data, and only *then* looks at `wLinkState` to decide whether what follows is a
trade or a battle. Which room you are in was decided much earlier, by which tile
you pressed A on — `CableClubLeftGameboy` (`engine/pokemon/bills_pc.asm`)
compares `wCurMap` and sets `LINK_STATE_START_TRADE` or `LINK_STATE_START_BATTLE`.

**The partner is never told.** The 644-transfer section exchange is byte-for-byte
identical in both rooms, and the question that follows uses the same encoding in
both: `LinkBattleExchangeData` (`engine/battle/core.asm`) and
`Serial_SyncAndExchangeNybble` (`home/serial.asm`) both carry `$60 | n`.

So the trade-only partner walked into a real link battle and kept walking its
trade state machine — one step per battle turn:

| turn | partner phase | what it meant to say | what the battle engine heard |
|---|---|---|---|
| 1 | `SelectMon` | "I offer party slot 0" | use move slot 0 |
| 2 | `Confirm` | "I accept the trade" | **use move slot 2** |
| 3 | `PostTrade` | post-trade sync | use move slot 2 |
| 4+ | `Connected` | rendezvous sync | use move slot 0 |

### The crash

`SelectEnemyMove`'s link branch bounds-checks nothing:

```asm
	ld a, [wSerialExchangeNybbleReceiveData]
	cp 4
	ret nc
	ld [wEnemyMoveListIndex], a
	ld c, a
	ld hl, wEnemyMonMoves
	add hl, bc
	ld a, [hl]        ; = 0 for an empty slot. No zero check on this path.
```

and `GetCurrentMove` then does `dec a` on move id 0 and `AddNTimes Moves, 6`,
i.e. 1,530 bytes past a 165-entry table. Six arbitrary ROM bytes become a move,
and its effect byte is dispatched through `_JumpMoveEffect`
(`engine/battle/effects.asm`), an unchecked jump table ending in `jp hl`. The CPU
leaves the program.

The threshold is exactly "the offered Pokémon has fewer than three moves", and
**the shipped default offer — a level 5 Mew with one move — is on the crashing
side of it.** Reproduced deterministically at frame 4038.

### Why that was a *save* bug

Post-crash the runaway CPU keeps executing, and one of the things it reliably
does is store bytes into cartridge RAM. Measured: **4,378 bytes of cartridge RAM
changed within six frames**, and `Gameboy::frame`'s autosave wrote them to the
`.sav` file. In that reproduction they all landed in SRAM bank 0 — `layout.link`
calls it "Sprite Buffers", scratch — but nothing constrains which bank a runaway
CPU has selected, and banks 1-3 are "Save Data", "Saved Boxes 1" and "Saved
Boxes 2": the party, the badges, the money, the PC boxes.

Two more consequences of the same root cause, both reproduced:

* **A glitch Pokémon.** When our Pokémon fainted, `ReplaceFaintedEnemyMon` asked
  which one came next and `EnemySendOutFirstMon` computed
  `wWhichPokemon = received - 4` — unchecked. A move index of 0 underflows to
  party slot 252, and the game fielded a MissingNo-class Pokémon whose name
  changed every turn.
* **A phantom trade.** Turn 2 landing on `Confirm` incremented
  `trades_completed` during *every* battle, so the status bar announced a trade
  that never happened.

---

## 2. The guard: a crashed game cannot reach the save file

This is separate from, and larger than, the Colosseum. **Any** crash, from any
cause, must not be able to overwrite a good save.

`MemoryManagementUnit::report_fault(reason)` is the single entry point. It
poisons the battery (`MemoryBankController::poison_battery` →
`Battery::poison`), records the reason, and says so on stderr. Sticky for the
session; the first reason wins. It deliberately does **not** stop or reset the
machine — detectors are heuristics, and only the irreversible act is withdrawn.

`Battery` already refused to write after a failed *read*, on the rule that a save
we could not read is a save we must not overwrite. This is the same rule pointed
the other way, and it covers the per-frame autosave, an explicit
`flush_battery()`, and the last-chance flush in `Drop`.

Three detectors feed it, and **the third is why plugins may report faults at
all**:

1. **An undefined opcode** (`$D3 $DB $DD $E3 $E4 $EB $EC $ED $F4 $FC $FD`). This
   used to `panic!` — an abort, under `panic = 'abort'` — which is neither what
   the hardware does nor helpful at the moment the user most needs telling.
   Hardware locks the CPU up (Pan Docs), so now it does that and reports a fault.
   `$FC` was silently treated as a NOP, hiding a crash rather than reporting it.
2. **Fetching an opcode from `$FE00-$FF7F`** — OAM, the unusable block and the
   I/O registers, three regions that cannot hold code. One subtract and one
   compare per instruction, on a branch a working game never takes. HRAM
   (`$FF80-$FFFE`) is explicitly *not* included: real games run their OAM DMA
   routine there.
3. **A plugin that knows the game.** `Machine::report_fault` needs no capability,
   because unlike everything else on `Machine` it only takes a right away.

Point 3 is not theoretical: **neither CPU-level detector fires on this crash.**
Measured over 600 post-crash frames, the runaway code stays inside executable
memory and never reaches an undefined opcode. The Gen 1 partner sees it
immediately — `wLinkState` reads `$7F` and then `$39`, and neither is a
`LINK_STATE_*` value.

Two guards against a false alarm, which would stop a *healthy* game saving: the
partner only watches while a link session is in progress (this emulator boots
with randomised WRAM, so before then the byte is noise), and only reports after
two consecutive frames.

One more thing had to change for the guard to be usable at all. `Gameboy::frame`
looped until the PPU produced a frame, and `Gpu::do_cycle` returns immediately
while the LCD is off — so a crashed game that switches the LCD off made `frame()`
**unbounded**. Six post-crash frames took about a minute; forty never finished.
It is now capped at `FRAME_TICKS_CAP` (a frame's worth plus a small margin), which
with the LCD on is unreachable — the `updated` break always arrives first — so
nothing about normal running changes. The same 600 post-crash frames now take
four seconds.

---

## 3. The fix: the partner takes part in the battle

Two legitimate outcomes were on the table — implement the battle properly, or
decline the Colosseum cleanly. **Declining turned out not to be available.**
`LinkMenu` (`engine/menus/main_menu.asm`) resolves a disagreement between the two
consoles in favour of whichever is driving the clock, and the partner
deliberately made the *game* the clock master (that is what makes a local virtual
partner easy — see [`link-trade.md`](link-trade.md)). So when the player presses
A on COLOSSEUM, the game wins whatever we send. A partner that refused to answer
would hang the game at "PLEASE WAIT!", which is worse than either option.

So the partner participates. The load-bearing fact is that **the wire carries
four bits per turn**:

```text
0-3   use this move slot          core.asm SelectEnemyMove
4+n   send out party slot n       core.asm EnemySendOutFirstMon ("sub 4")
$D    LINKBATTLE_NO_ACTION        cannot act this turn
$E    LINKBATTLE_STRUGGLE         out of usable moves
$F    LINKBATTLE_RUN              leave the battle
```

Nothing about a *result* ever crosses the cable. The player's console computes
damage, accuracy, criticals, status, faints and experience from a random list
both consoles share; the partner is asked only for an action.

### Reading the host rather than simulating it

Answering *legally* needs our own Pokémon's moves, PP and HP. Computing those
independently means reimplementing Gen 1's battle engine, which is both a large
piece of work and the classic source of desyncs between two real cartridges.

It is also unnecessary. The player's console is the single authority and
maintains all of it in memory the plugin already declares the capability to read.
So `src/plugin/gen1/battle.rs` **looks the state up** at `wEnemyMon` — "enemy"
being us, from the host's point of view — and chooses:

* a move slot that is populated **and has PP**, uniformly at random among those;
* `$E` Struggle when moves remain but none has PP;
* `4 + slot` when our active Pokémon has fainted, naming a party member the
  host's own copy of our party shows as still standing;
* `$F` Run when that copy cannot be read — a defined action
  (`ReplaceFaintedEnemyMon` returns straight to `EnemyRan`), and the only honest
  answer when the partner cannot see which of its Pokémon are alive.

**This is not a faithful two-console emulation and does not claim to be.** An
observer with a logic analyser could tell. It is a real opponent, choosing real
actions, arriving through the genuine link path, with the host doing all the
arbitration — which is the right shape for a battle-control plugin and for
giving an agent an opponent to practise against. A true independent second
console (its own battle engine, exact RNG-stream fidelity) remains a project in
its own right; it is the only version that could serve as a conformance oracle
for link battles, and nothing currently in flight needs it.

### Which room am I in?

`wLinkState` is the authority, and it is readable at exactly the right moment:
`cable_club.asm` still holds `LINK_STATE_START_BATTLE` when the section exchange
finishes and promotes it to `LINK_STATE_BATTLING` immediately afterwards. The
link-menu byte (`$D0 | item | button << 2`, item 1 = COLOSSEUM) is kept as a
fallback for a cartridge whose RAM map we do not know — the partner is then
blind, but the move slot it names is still one that holds a move, so the crash
remains impossible.

### Addresses

Derived the same way as [`../src/trainer/gen1.rs`](../src/trainer/gen1.rs): from
`pret/pokered`'s `ram/wram.asm` and `macros/ram.asm`, cross-checked against the
cartridge. `wEnemyMons` (`$D8A4`) was located by **measurement** rather than
faith — a WRAM search for a `party_struct` matching the active Pokémon's species,
level and max HP returned exactly one candidate. `Host::party_hp` re-checks the
species at run time anyway, so a cartridge that disagrees is declined rather than
misread.

### Two other bugs that fell out

* **`seen_party()` had never once worked against a cartridge.** It indexed the
  received stream at fixed offsets, and no fixed offset can be right:
  `Serial_ExchangeBytes` re-sends its block's first byte until it *receives* a
  preamble and then once more, so the number of leading copies depends on when
  the other console's preamble turns up. It now finds each block the way the
  hardware does — skip to the run of `$FD`, skip all of it, take the block's
  length — and tolerates the final block arriving one byte short, which against a
  real cartridge it always does. Verified end to end: the partner now decodes the
  player's actual team out of the exchange.
* **The phantom trade** is gone as a consequence: a battle turn never enters the
  trade state machine's `Confirm` phase, and the status bar says
  `battling with … — turn N, move 2` rather than claiming a trade.

---

## 4. Conformance record

Per the standing convention in [`../CLAUDE.md`](../CLAUDE.md). The coverage
answer is the same for all of these and is itself the finding: **no public test
ROM covers any of it.** Blargg, Mooneye GB and acid2 test one console in
isolation; nothing in the suite plugs a second Game Boy in.

1. **A crashed CPU's writes reaching a battery save.**
   *Symptom* — the player's save file is silently replaced by whatever a runaway
   CPU stored in cartridge RAM. Indistinguishable from "the game forgot my save"
   until it is far too late.
   *Caught by* — nothing before; now `testharness/battery.rs` case 8, which boots
   a hand-assembled cartridge that switches the LCD off, copies a routine into
   OAM and jumps to it. Teeth proven by disabling the guard: 8,192 of 32,768
   bytes change.
   *Behaviour violated* — a battery-backed cartridge holds SRAM from the instant
   the game writes it (Pan Docs, MBC3); an emulator that persists it also
   persists a crash's scribbling, which hardware cannot help but do either. The
   difference is that hardware has no *file* to overwrite, and the emulator does.
   *Conformance candidate* — **a run that leaves the program must not be able to
   modify a previously-saved file.**
2. **An unpopulated move slot chosen over the link.**
   *Symptom* — the game crashes to a black screen mid-battle, the emulator
   appears to hang, and the save file gains thousands of changed bytes.
   *Caught by* — nothing before; now `testharness/colosseum.rs` case 1, driving
   the shipped default offer into a real Colosseum battle.
   *Behaviour violated* — `SelectEnemyMove` (`core.asm`) trusts the received
   nybble with no zero check and `GetCurrentMove` (`core.asm`) dereferences it
   1,530 bytes past the move table. A real partner never names an empty slot.
   *Conformance candidate* — **an opponent action a peer could not legally send
   must not be able to leave the program.**
3. **A replacement index answered as a move index.**
   *Symptom* — after the opponent's Pokémon faints, a glitch Pokémon with a
   changing name enters and the battle continues on nonsense data.
   *Caught by* — nothing before; now `testharness/colosseum.rs` case 2, which
   reads the answer off the game's own screen: the second Pokémon must come out
   *by name*.
   *Behaviour violated* — `EnemySendOutFirstMon` (`core.asm`) computes
   `wWhichPokemon = nybble - 4` unchecked; the protocol requires `4 + slot`.
4. **Trade phases answering battle turns.**
   *Symptom* — the opponent's move choice mirrors the player's own button
   presses, and the status bar reports a trade that never happened.
   *Caught by* — nothing before; now
   `a_colosseum_round_is_answered_as_a_battle_action_not_a_trade_phase` and the
   `trades_completed` assertion in both harness cases.
   *Behaviour violated* — `LinkBattleExchangeData` and
   `Serial_SyncAndExchangeNybble` share one byte encoding; a partner must
   disambiguate them from `wLinkState`, which nothing on the wire carries.
5. **Receive-side framing pinned to a fixed offset.**
   *Symptom* — `seen_party()` silently returns `None` after every real exchange,
   in both rooms, so "the evidence a trade really happened" never existed.
   *Caught by* — nothing; the old unit test fed the partner *our own* framing
   back. Now `a_party_is_decoded_from_the_framing_a_cartridge_actually_sends`,
   which builds the stream the way `Serial_ExchangeBytes` does, across every
   plausible re-sync count.
   *Behaviour violated* — `Serial_ExchangeBytes` (`home/serial.asm`) emits a
   number of leading re-sync copies that depends on the *other* console.

---

## 5. Reproducing and running it

```bash
# The regression harness. Skips (exit 0) without a cartridge and a save.
POKE_ROM=<blue.gb> cargo test --release --test colosseum

# The save fixture it wants, if you have none:
cargo run --release --example gen1_trade -- --rom <blue.gb> --make-save

# Narrate the run, or dump every serial byte with its frame:
COLOSSEUM_VERBOSE=1 COLOSSEUM_WIRE=/tmp/wire cargo test --release --test colosseum
```

**Never point any of this at a real save.** The harness stages the cartridge
*and* the save into `target/` first, because the MBC writes `.sav` next to the
ROM and the harness presses buttons at menus.

---

## 6. Known gaps, stated plainly

* **Multi-turn moves are not modelled.** If the partner's Pokémon is charging,
  thrashing, storing energy or recharging, a real second console would send
  `$D` (`LINKBATTLE_NO_ACTION`). We send a normal move index; the host's
  `CheckEnemyStatusConditions` overrides it, so the observable behaviour is
  right, but the wire is not what hardware would carry. Closing it needs
  `wEnemyBattleStatus1/2`, which are not yet pinned in this repo.
* **The chooser is uniform-random among legal moves.** It is a real opponent, not
  a good one. Move power and type effectiveness are already available from
  `romdata`, so a better policy is additive.
* **The partner can be poisoned by an interrupted exchange.** `feed` routes
  straight into `feed_exchanging` while a section exchange is in progress, so a
  `$01` handshake arriving mid-stream is swallowed as block data, and
  `LinkPartner::reset()` is only called on a save-state load. Inferred from the
  code and not reproduced; left alone rather than risk breaking a working trade.
* **Yellow and Gen 2 are not supported.** `Game::detect` refuses them by title
  *and* header checksum, so the partner declines to read their memory rather than
  reading a shifted layout. Trading still works; battle turns fall back to the
  blind (but still crash-proof) path.
