# Discoveries — what this campaign found out about Pokémon Blue

[← Gen 1 area](README.md) · [the memory atlas](atlas/README.md) · [sharp edges](sharp-edges.md)

**Every fault, quirk and corrected belief this project turned up while driving a retail Pokémon
Blue cartridge, with the reasoning that found it.** Some are thirty-year-old defects in the game.
Some are mechanics the community states one way and the cartridge states another. And some are
mistakes of *ours* — a wrong number, a wrong name, a goal that measured the wrong thing — which
are here for the same reason: an entry is worth writing when finding it cost a day, and it costs
just as much either way.

This page exists because the findings were scattered. The soft-lock was a section of
[sharp edges](sharp-edges.md), the ledge trap was three paragraphs inside a chain write-up, the
accuracy-scaling bug was a bullet in a page about a battle policy, and several were only ever in
a commit message. A reader who wants to know *what has been discovered here* now has one door.

> **What this is not.** It is not a Gen 1 wiki, and it does not restate mechanics somebody else
> documented first. Everything below was measured on the cartridge by this project, and where a
> claim came from somewhere else and turned out to be wrong, the measurement that settled it is
> shown.

---

## The slugs are a contract

Every entry has a stable heading, and **its GitHub slug is permanent once published** — the same
rule [`TGB-` codes](../pitfalls/codes.md) follow, and for the same reason: the whole value of a
code or a slug is that a five-year-old link still resolves. A parallel project,
**[AtlasGB](https://github.com/Alchemy86/AtlasGB)**, links back into this page by slug from the
addresses these findings involve, so renaming a heading here breaks somebody else's index.

Add entries freely; never renumber, never reword a heading. If an entry turns out to be wrong,
correct its body and say so in it — that is what the "our own belief" rows below are.

| entry | what it is | kind | the memory behind it |
|---|---|---|---|
| [the Oak's Lab soft-lock](#the-oaks-lab-soft-lock) | the game bricks itself if you stand in the wrong two rows | cartridge fault | [`wTileMap`][wTileMap] · [`wShadowOAM`][wShadowOAM] |
| [a missed trapping move leaves its counter standing](#a-missed-trapping-move-leaves-its-counter-standing) | the flag clears and the countdown does not | cartridge fault | [`wPlayerNumAttacksLeft`][wPlayerNumAttacksLeft] |
| [the blackout carve-out](#the-blackout-carve-out) | one battle in the game you are allowed to lose | cartridge mechanic | [`wCurOpponent`][wCurOpponent] · [`wCurMap`][wCurMap] |
| [a critical hit is the worst case](#a-critical-hit-is-the-worst-case) | Gen 1 doubles the *level*, not the damage | cartridge mechanic | [`wBattleMon`][wBattleMon] |
| [accuracy is not the number in the move table](#accuracy-is-not-the-number-in-the-move-table) | stages scale it, and nothing warns you | cartridge mechanic | [`wPlayerMonStatMods`][wPlayerMonStatMods] |
| [TACKLE is 95 per cent and we said 94](#tackle-is-95-per-cent-and-we-said-94) | our own reading of the move table was wrong | **our own belief** | — |
| [the battle move menu cursor is 1-based](#the-battle-move-menu-cursor-is-1-based) | and the main menu's is not | cartridge mechanic | [`wCurrentMenuItem`][wCurrentMenuItem] |
| [an escape is not free](#an-escape-is-not-free) | a failed run costs the same hit as a swing | cartridge mechanic | [`wEnemyMon`][wEnemyMon] |
| [the rival battle is not a coin flip](#the-rival-battle-is-not-a-coin-flip) | base stats decide it, and we had said otherwise | **our own belief** | — |
| [crossing two levels loses the move in between](#crossing-two-levels-loses-the-move-in-between) | one battle, one level-up message, one learnset check | cartridge mechanic | [`wPartyMon1Level`][wPartyMon1Level] |
| [cancelling an evolution costs one level and nothing else](#cancelling-an-evolution-costs-one-level-and-nothing-else) | it is re-offered at the very next level-up | cartridge mechanic | [`wPartyMon1Species`][wPartyMon1Species] |
| [BubbleBeam is not in Mt Moon](#bubblebeam-is-not-in-mt-moon) | a plausible route plan, checked against the item tables | **an outside belief** | — |
| [the one-way ledge trap](#the-one-way-ledge-trap) | a waypoint whose overshoot cannot be undone | route shape | [`wXCoord`][wXCoord] · [`wYCoord`][wYCoord] |
| [the forest is not a scenic route](#the-forest-is-not-a-scenic-route) | Route 2's halves do not connect on foot | route shape | [`wCurMap`][wCurMap] |
| [only one of the forest's three trainers is compulsory](#only-one-of-the-forests-three-trainers-is-compulsory) | we over-read our own grid | **our own belief** | — |
| [no Potion is purchasable before Pewter](#no-potion-is-purchasable-before-pewter) | so three links have no healing but the nurse | cartridge mechanic | — |
| [wCurMap is not a loaded map](#wcurmap-is-not-a-loaded-map) | the trap that has caught four separate pieces of work | cartridge mechanic | [`wCurMap`][wCurMap] · [`wCurMapWidth`][wCurMapWidth] |
| [a warp changes wCurMap before the coordinates](#a-warp-changes-wcurmap-before-the-coordinates) | one frame naming the destination and the departure's door | cartridge mechanic | [`wCurMap`][wCurMap] · [`wXCoord`][wXCoord] |
| [a documented sentinel read as anything else](#a-documented-sentinel-read-as-anything-else) | `$FF` is "blacked out", not "trainer battle" | **our own belief** | [`wIsInBattle`][wIsInBattle] |
| [two map names in our own table were wrong](#two-map-names-in-our-own-table-were-wrong) | an agent was told it was in a Pokémon Center | **our own belief** | [`wCurMap`][wCurMap] |
| [a screen does not survive a switch](#a-screen-does-not-survive-a-switch) | our battle engine had a Gen 3 rule in it | **our own belief** | [`wPlayerBattleStatus3`][wPlayerBattleStatus3] |
| [a cartridge can tell there is no sound chip](#a-cartridge-can-tell-there-is-no-sound-chip) | the nurse's healing sequence never finishes | emulator | — |
| [the learnset table is indexed by internal number, not by dex number](#the-learnset-table-is-indexed-by-internal-number-not-by-dex-number) | reading the first 151 slots loses all three starters | cartridge mechanic | — |
| [a link that scored 100 per cent on the wrong task](#a-link-that-scored-100-per-cent-on-the-wrong-task) | a goal authored from a plausible idea | **our own belief** | — |
| [the silent dead band](#the-silent-dead-band) | two components deciding the same thing | **our own belief** | — |

[wTileMap]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wTileMap
[wShadowOAM]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wShadowOAM
[wCurMap]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wCurMap
[wCurMapWidth]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wCurMapWidth
[wXCoord]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wXCoord
[wYCoord]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wYCoord
[wIsInBattle]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wIsInBattle
[wCurOpponent]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wCurOpponent
[wBattleMon]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wBattleMon
[wPartyMon1Level]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wPartyMon1Level
[wPartyMon1Species]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wPartyMon1Species
[wEnemyMon]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wEnemyMon
[wCurrentMenuItem]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wCurrentMenuItem
[wPlayerMonStatMods]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wPlayerMonStatMods
[wPlayerBattleStatus3]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wPlayerBattleStatus3
[wPlayerNumAttacksLeft]: https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md#s-wPlayerNumAttacksLeft

**Every entry has the same shape**, because the shape is what makes it usable: *what it is*, *how
it showed up* (the messy symptom, not the tidy one), *how it was found* including the wrong turns,
*what is actually happening* with a `pret/pokered` citation by symbol name, *the memory* involved
where there is one, and *the evidence* that settles it. Where a part is missing it is because
there is nothing honest to put there — an entry with no picture says so rather than inventing one.

---

## Faults in the cartridge

### The Oak's Lab soft-lock

**What it is.** Between Oak's "choose one!" speech and taking a starter, walking into rows 5 or
6 of the laboratory can lock Pokémon Blue up permanently. It is the game's own OAM buffer
overflowing into the tilemap.

**How it showed up.** Not as a crash. **53 of 600 scripted attempts at the take-a-starter link
ended on the same tile — `(5,5)` — with the picture still being drawn, the play-time counter
still ticking, and no button doing anything ever again.** One of them, classified `Stuck` rather
than out-of-budget, printed its own diagnosis: *"the last 24 moves covered 4 tile(s): 0x28(5,3)
0x28(5,4) 0x28(5,5) 0x28(5,6)"*. A hundred and eighty decisions of walking with nothing moving.

**How it was found, wrong turns included.**

1. The first hypothesis was ours — a stuck solver. It was not: the same failure landed on one
   tile 53 times, and a solver that is lost does not cluster.
2. `Gameboy::debug_pc` put the CPU in `DelayFrame`'s spin loop. **That proves nothing and is
   worth knowing**: a Game Boy spends most of every frame there, and a *healthy* machine reports
   it too. What separated the cases was the play-time counter — it advanced at exactly the
   healthy rate, so the game was alive and simply refusing input. (`Gameboy::debug_ime` was added
   afterwards for the other case: a game genuinely hung in `DelayFrame` reads `IME` low with the
   VBlank bit set in both `IE` and `IF`.)
3. Reasoning about which flag "should" be wrong got nowhere for an hour.
4. **What worked was a delta-debug.** Snapshot a frozen machine and a healthy one standing on
   the same tile, copy one candidate byte from healthy to frozen, test whether the player can
   move, shrink. Copying the whole of work RAM and high RAM did *not* unfreeze it — that was the
   clue that the snapshot was too narrow, so VRAM and OAM went in. It went from **565 differing
   bytes to one**: `$C434`.

**What is actually happening.** `$C434` is `wTileMap` row 7 column 8 — which is
`GetTileAndCoordsInFrontOfPlayer`'s `lda_coord 8, 7`, the tile the game reads when the player
faces up. A healthy machine has `$11`, floor. This one had `$A0`.

- `wShadowOAM` is 40 entries, 160 bytes, and **`wShadowOAMEnd` *is* `wTileMap`**
  (`ram/wram.asm`; `$C3A0 - $C300 = $A0 = 160`).
- `PrepareOAMData` (`engine/gfx/sprite_oam.asm`) walks all sixteen sprite slots and writes four
  OAM entries for each one that survives `CheckSpriteAvailability`, tracking the write position
  in the 8-bit `hOAMBufferOffset`. **Nothing bounds it at 40.**
- Its clear-unused-OAM loop then walks on from that offset in steps of four and stops **only on
  `cp l / ret z`** against `LOW(wShadowOAMEnd)`. Start past `$A0` and `l` never equals `$A0`
  again until the next page, so it stamps `$A0` — `SCREEN_HEIGHT_PX + OAM_Y_OFS`, the
  hide-a-sprite Y — into every fourth byte of `wTileMap` all the way to `$C4A0`.
- `$A0` is not a walkable tile in any direction, so the player is standing in a box the collision
  check draws around them.

Oak's Lab declares **eleven objects** (measured: its header at map `$28` reads tileset 5, 5x6
blocks, 2 warps, 11 objects), which with the player is one sprite under the limit. Whether it
tips over depends on where the girl NPC has paced to, which is why the same tile is safe most of
the time and fatal some of it.

**The memory.** [`wTileMap` `$C3A0`][wTileMap], [`wShadowOAM` `$C300`][wShadowOAM].

**Evidence — the shape recorded from the frozen machine** (the `xx` and `..` are bytes that vary;
the `$A0` lattice and its two ends are what was measured):

```text
frozen machine, Oak's Lab, player at (5,5) facing up

  $C3A0  A0 xx xx xx   <- a well-formed OAM quad: Y, X, tile, attributes
  $C3A4  A0 .. .. ..
  $C3A8  A0 .. .. ..        every fourth byte, for 256 bytes
   ...
  $C434  A0            <- GetTileAndCoordsInFrontOfPlayer's lda_coord 8,7
   ...                     healthy: $11 (floor)
  $C4A0                <- where the clear loop finally stops
```

**There is no screenshot of this, and that is the finding.** The frame is *correct* — the picture
keeps being drawn, the sprites animate, the whole screen looks like an ordinary Oak's Lab. A
screenshot would show nothing at all. The evidence is the memory and the frozen clock, which is
exactly why it survived so long as "the solver gets stuck sometimes".

**Proof that it is the cartridge and not us.** The four bytes at `$C3A0` are a well-formed OAM
quad, nothing outside the ROM writes OAM-shaped data into work RAM, and **both picture engines
produce it identically**.

**What to do about it.** Keep the player in rows 3-4 until the starter is taken — which is also
the shortest route: Oak's escort ends at `(5,3)`, one step down is `(5,4)`, and the three Poké
Balls are reached along row 4. Full trap-shaped write-up:
[sharp edges](sharp-edges.md#standing-in-the-wrong-two-rows-of-oaks-laboratory-locks-the-game-up-for-good).

---

### A missed trapping move leaves its counter standing

**What it is.** A Wrap or a Bind that *misses* clears its "I am trapping you" flag and leaves the
turn counter behind it untouched.

**How it showed up.** A harness measuring how long Gen 1's trapping moves lock a target read a
counter going **`4 → 2` in one step**, and would have published a lock duration the game has
never rolled.

**How it was found.** By keying the measurement on the counter rather than on the flag, and then
noticing that the numbers were not a distribution. The correct instrument keys an episode on
**the flag**; re-run that way, 73 of 371 measured trapping locks and 39 of 161 thrash locks end
early, which is consistent with Wrap's 85 % accuracy and with Gen 1's rule that hurting yourself
in confusion clears a thrash.

**What is actually happening.** `TrappingEffect` runs *before* the hit test — its own comment
says so — and `MoveHitTest`'s `.moveMissed` does `res USING_TRAPPING_MOVE` and **does not clear
the counter**. So a first-turn miss sets both and clears only one of them.

**The memory.** [`wPlayerNumAttacksLeft` `$D06A`][wPlayerNumAttacksLeft].

**Evidence.** The full measurement is in [a paper's Gen 1 claims, checked](paper-claims.md), and
the standing rule it produced is worth more than the count: **key an episode on the flag, not on
the counter, whenever a mechanic has both.**

---

## The battle, as the cartridge actually plays it

### The blackout carve-out

**What it is.** `HandlePlayerBlackOut` special-cases the starter battle against the rival in
Oak's Lab. Losing it is not a loss: the party is healed, the flag is set, and the game carries on
exactly as if you had won.

**How it showed up.** **216 of 600 attempts at the rival-battle link failed, and 166 of them were
not failures.** They ended `Ending::Failed("party_hp=0")` on the battle tile, about two seconds
before the goal would have fired.

**How it was found.** By reading the scenario against the disassembly instead of reading the
failure count. The scenario declared `party_hp=0` a failure; **its own objective text said the
opposite** — *"the cartridge sets `EVENT_BATTLED_RIVAL_IN_OAKS_LAB` after the battle either way
and heals your party, so the goal is that the battle happened, not that it was won"* — and nobody
had checked which of the two the cartridge agreed with.

**What is actually happening.**

```asm
	ld a, [wCurOpponent]
	cp OPP_RIVAL1
	jr nz, .notRival1Battle
	...
	ld hl, Rival1WinText
	call PrintText
	ld a, [wCurMap]
	cp OAKS_LAB
	ret z            ; starter battle in oak's lab: don't black out
```

`HandlePlayerBlackOut`, `engine/battle/core.asm`. The rival says *"Yeah! Am I great or what?"*,
the player stays in the laboratory, and `OaksLabRivalEndBattleScript` heals the party on the next
map-script tick and sets the flag.

**Note the two conditions**, because it is what makes the rule narrow: it needs `wCurOpponent` to
be `OPP_RIVAL1` **and** `wCurMap` to be `OAKS_LAB`. On Route 1 a wild Pokémon is a species, so
the branch is not taken and the cartridge really does black the player out, warp them to
`wLastBlackoutMap` and halve their money. Scoring *that* as a failed attempt is a scenario's
choice; scoring the Oak's Lab loss as one is contradicting the game.

**The memory.** [`wCurOpponent` `$D059`][wCurOpponent], [`wCurMap` `$D35E`][wCurMap].

**Evidence.** Reproduced end to end on a seed that had been failing: the run reaches the goal at
55 decisions with the party back at 19/19, money untouched at ₽3000 (a blackout halves it), and
`wOaksLabCurScript` at `SCRIPT_OAKSLAB_RIVAL_STARTS_EXIT` — the same state a win leaves.

![600 agents fighting the rival in Oak's Lab at once](../agent/chain-stills/2-the-rival-battle.png)

*Every trainer battle in the first eleven links of the chain is this one fight, which is why all
600 agents are in it together. Roughly a quarter of them are about to lose it, and it makes no
difference to any of them.*

---

### A critical hit is the worst case

**What it is.** Gen 1's critical hit substitutes `2 * Level` for `Level` in the damage formula,
so it is close to **twice** an ordinary hit — not the top of the ordinary roll range.

**How it showed up.** A battle policy that decided "am I about to faint?" from the maximum
*ordinary* roll **lost 10 of 600 attempts**, and every one of them was comfortably above its own
threshold when the killing turn came.

**How it was found.** By reading the traces of the deaths rather than the aggregate. Each one had
a last exchange that did roughly double what the model predicted, and at the rate the deaths
occurred — base-speed/2 out of 256, about one turn in eleven for a Route 1 Pokémon — that is a
critical hit and not a bad roll.

**What is actually happening.** `CriticalHitTest` sets the flag; `CalculateDamage`
(`engine/battle/core.asm`) doubles the level when it is set. A crit therefore ignores the roll
range entirely as a bound: the *worst* critical outcome is above the *best* ordinary one.

**The memory.** [`wBattleMon` `$D014`][wBattleMon] — the level and stats the engine is fighting
with, which is where the doubling is applied, not the party copy.

**Evidence.** The rate is measurable as an absolute prediction rather than a ratio: for a level 5
Electrode (base Speed 140, the game's highest, which is what makes a few hundred attacks enough)
`CriticalHitTest` gives 70/256 without Focus Energy and 17/256 with. Measured **0.2604**
(250/960) and **0.0612** (36/588), both inside their 95 % intervals — which incidentally confirms
Gen 1's own bug that Focus Energy *quarters* the rate. Pricing the same move with the attacker at
twice its level and using that for the faint test took the link from 98.3 % to 99.0 %.

---

### Accuracy is not the number in the move table

**What it is.** `MoveHitTest` scales a move's accuracy byte by the attacker's accuracy stage and
by the inverse of the defender's evasion stage. A policy reading the ROM value is reading a
number that has not been true since the first SAND-ATTACK landed.

**How it showed up.** Three of 3,000 attempts at one link died, each in the same shape: at 2 HP
against a Pokémon on 1 HP, the policy chose to swing rather than escape because *"a good roll
ends it first"* — a coin flip taken in place of an escape that would also have removed the
incoming hit.

**How it was found.** **Only at 3,000 attempts. Two separate 600-attempt runs were clean.** That
is the certification bar earning its keep rather than being a formality, and it is the strongest
argument on this page for large N: the bug was not rare in the *mechanic*, it was rare in the
*route*, because it needs a Pidgey that has had time to use SAND-ATTACK twice.

**What is actually happening.** Both ratios come out of `StatModifierRatios`, which is **read
from the cartridge, not transcribed** — `0F:76CB`, found by searching for its own opening bytes
(`25,100,28,100,…`, which occur exactly once) and re-checked on every read: the first pair must
be 0.25 and the middle one exactly 1:1, because a table read from the wrong place almost never
has a neutral stage in its middle.

A Pidgey's SAND-ATTACK therefore takes a 95 % TACKLE to **62 %** and then **47 %**. Both the
accuracy byte and the two ratios below were read off the retail cartridge for this page — the
table's signature occurs exactly once, at `0F:76CB`, and its middle pair is `1/1`:

```text
  TACKLE accuracy byte                      242 / 255 = 94.9%
  after one accuracy stage down  (66/100)   159 / 255 = 62.4%
  after two                      (50/100)   121 / 255 = 47.5%

  StatModifierRatios, 0F:76CB, thirteen numerator/denominator pairs:
    -6  25/100   -5  28/100   -4  33/100   -3  40/100   -2  50/100
    -1  66/100    0   1/1     +1  15/10    +2   2/1     ...   +6  4/1
```

**The memory.** [`wPlayerMonStatMods` `$CD1A`][wPlayerMonStatMods] — and note the atlas's own
warning on it, which is a separate trap: **7 is neutral, not 0**, and the block is zeroed work
RAM until `InitBattleVariables` writes the sevens, so a sample on the first frames of a battle
reads `0` everywhere and looks like −6 on everything.

**Evidence.** Effective accuracy is what the snapshot carries now, and a swing has to be **more
likely than not** to land. Link 10 went 99.9 % → 99.93 % on it.

---

### TACKLE is 95 per cent and we said 94

**Kind: a correction of our own belief.**

**What it is.** This project recorded, in a section literally headed *"two things the cartridge
settled"*, that **`TACKLE`'s accuracy byte is 240, not 242 — 94 %, where the community usually
quotes 95 %**. Re-measured on the retail cartridge: **the byte is 242, which is 94.9 %, and the
community's 95 % is right.**

**How it showed up.** It did not. Nothing failed, no run was affected, and the claim sat in
`docs/agent/battle-policy.md`, in `plugins/agent/src/battle.rs`'s doc comments and in `AGENTS.md`
for as long as they existed. It was found by re-reading the move table while writing this page,
because a page about discoveries that repeats a number without checking it is not worth having.

**How it was found.** Read the `Moves` table straight out of the cartridge at `0E:4000`, six
bytes per move (`animation, effect, power, type, accuracy, PP`), and check the reading against
moves whose values are not in dispute before believing anything about the one that is:

| move | id | power | accuracy byte | as a percentage |
|---|---:|---:|---:|---:|
| POUND | 1 | 40 | 255 | 100 % |
| SCRATCH | 10 | 40 | 255 | 100 % |
| VINE WHIP | 22 | 35 | 255 | 100 % |
| **TACKLE** | **33** | **35** | **242** | **94.9 %** |
| HYPER BEAM | 63 | 150 | 229 | 89.8 % |
| HYDRO PUMP | 56 | 120 | 204 | 80.0 % |

Every row but the one under investigation matches the values Gen 1 is known for, which is what
says the table is being read at the right offset with the right stride.

**Where the wrong number probably came from.** 94 % of 255 is 239.7, which rounds to **240** —
the byte you get by *computing it from a percentage you already believed*, rather than reading
it. The code was never wrong:
[`rom::move_accuracy_percent`](../../plugins/trainer/src/rom.rs) rounds to nearest
(`(242 * 100 + 127) / 255 = 95`) and has always reported 95. Only the prose was.

**Evidence, reproducible in one command:**

```sh
python3 - "$ROM" <<'PY'
import sys
rom = open(sys.argv[1], 'rb').read()
e = 0x0E * 0x4000 + (33 - 1) * 6            # Moves, 0E:4000, TACKLE is move 33
print("power %d  accuracy %d  (%.1f%%)" % (rom[e+2], rom[e+4], rom[e+4] * 100 / 255))
PY
# power 35  accuracy 242  (94.9%)
```

**The lesson, and it is the same one this project keeps relearning.** A confident, cited-sounding
sentence with no command behind it is a guess wearing a citation's clothes — the same shape as
[a screen does not survive a switch](#a-screen-does-not-survive-a-switch) below, and as
[`src/link.rs`](../../src/link.rs)'s torn-byte comment, which agreed with its own test and
disagreed with the cartridge.
**When you write down a number the cartridge holds, write down the command that reads it.**

---

### The battle move menu cursor is 1-based

**What it is.** `wCurrentMenuItem` reads **1** when `▶` is drawn on the first move, and 0 when it
is on the first item of every other menu in the game.

**How it showed up.** It did not — and that is the entry. **Getting it wrong is silent**: the
cursor already equals the target the driver was aiming at, A fires on the wrong move, and the
policy's choice is thrown away with nothing anywhere to say so.

**How it was found.** By measuring it before believing it, rather than by a failure. The driver
reads `menu_item - 1` for the move list and `menu_item` for everything else.

**What is actually happening.** `MoveSelectionMenu` (`engine/battle/core.asm`) opens with:

```asm
	ld a, [wPlayerMoveListIndex]
	inc a
	ld [hli], a          ; wCurrentMenuItem
```

**The memory.** [`wCurrentMenuItem` `$CC26`][wCurrentMenuItem],
[`wPlayerMoveListIndex` `$CC2E`][wPlayerMoveListIndex].

**Related, from the same investigation:** Gen 1's menu code does not see a button that arrives
too soon. `HandleMenuInput` delays several frames after accepting a press, so **a burst of
presses is routinely swallowed at the first one and lands out of phase for the rest**. A
`down, right, A` run policy fired blind opened the party list instead — for 2,500 decisions.
Reading the `▶` cursor out of the screen and pressing one key per decision cannot go out of
phase, because a swallowed press simply repeats.

---

### An escape is not free

**What it is.** Running from a Gen 1 wild battle can fail, and a failed escape costs exactly the
same incoming hit as swinging would have. Only one of the two removes the opponent.

**How it showed up.** As **the single failure in 3,000 attempts** of the training link:

```text
lv5 BULBASAUR 10/20  vs  lv3 RATTATA 2/14 (faster)
  -> RUN  "too hurt to pick a fight"     -> Can't escape!  -> took 5, now 5/20
  -> RUN  "one more hit and we faint"    -> Can't escape!  -> took 5, dead
```

The Rattata was on **2 of 14** and our TACKLE does 4-5. It was one press from over.

**How it was found, wrong turns included.** It took three goes to state the rule correctly, and
both wrong versions are worth more than the right one because each was plausible:

| the rule | 3,000 attempts |
|---|---|
| no rule at all | 2,999, and the trace above |
| `max >= their HP` — *a good roll ends it* | **2,995**, three faints |
| `min >= their HP`, any speed | 2,999, but two runs that had walked away from slower Pidgeys safely now died |
| `min >= their HP` **and they are at least as fast as us** | 2,999, **zero faints** |

- **The worst roll has to kill, not the best.** `max >= hp` fires on a *maybe*-kill: at 10 of 20
  against a Rattata on 6 of 16 with a 5-6 TACKLE, the faster Rattata hit first, our roll came up
  5, and it survived on 1. A maybe-kill bought nothing and cost the hit.
- **Only when the escape is the unreliable half.** `TryRunningFromBattle` scales the odds by our
  Speed against theirs, steeply, and always succeeds once ours over four overflows a byte.
  Against something slower, running is very nearly free and the argument does not apply.

**What is actually happening.** `TryRunningFromBattle`, `engine/battle/core.asm`. **162 escapes
failed across 3,000 attempts** of that link, which is Gen 1's own run check and not a defect —
but it is a cost that is invisible without a counter, so it is counted.

**The memory.** [`wEnemyMon` `$CFE5`][wEnemyMon] — Speed and HP both come from the battle struct,
not the party copy.

**Evidence.** The general rule it produced, which outlives the specific numbers: **when two
choices cost the same, prefer the one that ends the situation — and check that it really does end
it.** Full measurement: [the battle policy](../agent/battle-policy.md).

---

### The rival battle is not a coin flip

**Kind: a correction of our own belief.**

**What it is.** This project wrote that the Oak's Lab starter battle was *"close to a coin flip by
the game's design"*. It is not. Which starter you take decides it, and the margin is large.

**How it showed up.** Measured over 900 certified exits of the link, by which ball was taken:

| player | rival takes | lost |
|---|---|---:|
| SQUIRTLE | BULBASAUR | **14.7 %** |
| CHARMANDER | SQUIRTLE | **23.0 %** |
| BULBASAUR | CHARMANDER | **54.0 %** |

**How it was found.** The type chart is irrelevant — at level 5 every starter's damaging move is
Normal — so the answer had to be in the base stats, which are one query away from a script that
reads them out of the cartridge. It had been sitting behind a sentence of plausible reasoning for
months.

```text
             HP  Atk  Def  Spd  Spc
BULBASAUR    45   49   49   45   65
CHARMANDER   39   52   43   65   50
SQUIRTLE     44   48   65   43   50
```

Bulbasaur draws Charmander, which is **faster at every level** (65 against 45, so it moves first
every turn and lands the last hit) and hits harder (52) into a softer target (49). Squirtle draws
Bulbasaur and simply tanks it: Defense 65 against Attack 49, at a near speed tie.

**Evidence.** Read live from `BaseStats` at `0x0383DE`, 28 bytes per record indexed by Pokédex
number. And a consequence worth stating: **the chain's exit pool is not one population** — 30.6 %
of that link's exits carry a level-5 lead that lost, against level 6 for one that won, and every
link after it inherits the mix. That is the honest population; a real playthrough loses
sometimes.

---

### Crossing two levels loses the move in between

**Kind: a cartridge mechanic.**

**What it is.** Gen 1 announces **one** level per battle and checks the learnset for the level it
*ended* on. So a Pokémon that gains enough experience in a single battle to cross two level
boundaries is never offered the move belonging to the one it skipped — and there is no second
chance, because the check is keyed on the level reached, not on the levels passed.

**How it showed up.** A level 6 BULBASAUR was pointed at one level 5 CHANSEY — 182 experience, the
highest base-experience Pokémon in the cartridge — to make it learn LEECH SEED at level 7. It
reported `LEARNER grew to level 8!` and no learn prompt of any kind. The first version of
`gen1_learn_drill` read that as "the prompt does not fire" and went looking for a bug in itself.

**How it was fixed.** Not by fixing anything — by choosing the arithmetic so exactly one boundary
falls inside the run. The drill starts its BULBASAUR at level 12 and feeds it two CHANSEY: 973
experience for level 12, +182 → 1155 (still 12), +182 → 1337, which is level 13 and VINE WHIP.

**The memory.** [`wPartyMon1Level` `$D18C`][wPartyMon1Level], and the experience triple below it.

**Evidence.** The measurement and the full prompt trace:
[learning moves](learning-moves.md). The general rule it earns: **when a harness reports that
something never happens, check that the harness put the game in the state where it could.**

---

### Cancelling an evolution costs one level and nothing else

**Kind: a cartridge mechanic.**

**What it is.** The game offers a B press to cancel an evolution, and refusing it is not a
decision with a permanent cost. Two measurements together:

- **Nothing is ever lost by evolving.** Across all **52** level evolutions in the cartridge,
  the number that lose a move the evolved form never learns is **zero**. **44 of 52** *delay* one,
  and the longest delay any of them costs is **11 levels** — SOLARBEAM, 54 → 65, when an IVYSAUR
  becomes a VENUSAUR.
- **And a cancelled evolution is offered again at the very next level-up.** Measured on the
  cartridge: a level 15 BULBASAUR levelled to 16 with B pulsed through the animation prints
  `Huh? LEARNER stopped evolving!` and stays a BULBASAUR; levelled once more, pressing nothing
  but A, it prints `What? LEARNER is evolving!` and becomes an IVYSAUR at 17.

So the whole cost of one cancel is one level of the evolved form's base stats.

**How it was found.** By trying to cancel one and failing twice. **B has to be pulsed through the
evolution animation, not tapped once and not held**: Gen 1 reads some buttons through a routine
that only sees a transition, so a held button registers exactly once, and a four-frame pulse aimed
at the *text* is long gone by the time the animation asks. Both wrong versions reported
"cancelling does not work" with complete confidence — the Pokémon evolved anyway.

**Related, from the same trace.** Four places in the cartridge have an evolution and a level-up
move on the same level (ABRA → KADABRA at 16 with CONFUSION at 16, MAGIKARP → GYARADOS at 20 with
BITE at 20, SLOWPOKE → SLOWBRO at 37, CHARMELEON → CHARIZARD at 36). Measured on the ABRA: **the
evolution runs first and the evolved form's move for that level is then offered.** Evolving does
not cost you the move.

**The memory.** [`wPartyMon1Species` `$D16B`][wPartyMon1Species] — the species byte is what says
whether the B landed.

**Evidence.** [learning moves](learning-moves.md), which turns both measurements into the simplest
rule this campaign has produced: never press B on that prompt.

---

### BubbleBeam is not in Mt Moon

**Kind: a correction of an outside belief.**

**What it is.** A route plan arrived from outside this project: *a Magikarp is obtainable around
Mt Moon, it evolves into Gyarados at level 20, and a BubbleBeam TM is available there.* Three of
the four parts hold and the TM does not.

**How it showed up.** Sweeping every one of the 248 maps' object lists and every mart's stock
list, **TM11 BUBBLEBEAM lies on no map and is sold in no shop.** What Mt Moon holds is

| map | items |
|---|---|
| MT_MOON_1F `$3B` | POTION, MOON STONE, RARE CANDY, ESCAPE ROPE, POTION, **TM12 WATER GUN** |
| MT_MOON_B2F `$3D` | HP UP, **TM01 MEGA PUNCH** |

and the cartridge's own line for TM11 — *"TM11 teaches BUBBLEBEAM! Use it on an aquatic
POKéMON!"* — sits immediately before *"The CASCADEBADGE makes all POKéMON up to L30 obey!"*, which
is the Cerulean Gym guide's. So it is Misty's gift, **after** Mt Moon.

**What did hold.** MAGIKARP's `EvosMoves` record is `01 14 16` — evolve at level 20 into internal
index 22, GYARADOS. GYARADOS can be taught 23 machines including both TM11 and TM12. And a
Magikarp really is buyable near there: the text at file offset `0x08A977` reads *"MAN: Hello,
there! … I'll let you have a swell MAGIKARP for just ¥500!"*

**What the plan did not price.** MAGIKARP is on the **slow** experience curve, so level 5 to 20 is
**9,844 experience** — and it cannot earn a point of it by fighting. Its only move is SPLASH
(power 0) until TACKLE at 15, and it **can be taught none of the 55 machines**, checked against
its own `BaseStats` bitfield. Every one of those 9,844 has to come from switching it into battles
something else wins.

**Evidence.** [learning moves §6a](learning-moves.md), with the commands. Two grades worth
keeping honest: which script prints the TM11 text, and which map sells the Magikarp, were **not**
established here — only that both texts are in the cartridge and that the item tables do not put
TM11 anywhere.

---

## The map, measured rather than recalled

### The one-way ledge trap

**What it is.** A waypoint sitting one tile above a one-way ledge. Overshooting it by two tiles
is unrecoverable, and the walker then slides along the wall below it until its budget runs out.
**This shape has now appeared three times**, which is what makes it a rule rather than three
anecdotes.

**How it showed up.** One link lost **17 of 600 attempts**, every one of them on Viridian City's
row 28, sliding between x=26 and x=32, with the leg's first waypoint at `(29,26)` unreachable
above it.

**How it was found, wrong turns included.** The first model of the trap was **topological and it
was wrong**: a flood fill says `(29,26)` is fine — there is a 13-step path back to it crossing no
warp tile — and reports nothing. Two things had to be modelled before a check could catch it:

- **The ledge, exactly.** Stepping *down* off row 26 does not land on row 27, it **jumps** to row
  28. That is `LedgeTiles` in the cartridge (`06:66CF`, found by its own shape), and the
  one-wayness is in the data: **eight entries, for facing down, left and right, and none for up.**
- **The walker, exactly.** `Action::GoTo` is not a path-finder — it is greedy on the larger
  remaining delta with committed sideways detours — and below a one-way ledge it slides along the
  wall, which is precisely what the 17 failing runs did.

**Evidence, measured now with the repository's own audit tool over the retail cartridge.** Row 27
is a wall across the entire city with exactly **two** gaps, at x=15 and x=19:

```text
       0123456789012345678901234567890123456789
 y=25  ####....######........#.##..........####
 y=26  ####....######......................####   <- the waypoint was (29,26)
 y=27  ###############.###.####################   <- ledge; gaps only at x=15, 19
 y=28  ...#................................####   <- where 17 runs slid, x=26..32
 y=29  ...#.................#..............####
 y=30  ...#................................####
```

```text
(29,26) walkable=True   step down -> (29,28)      a two-tile jump over row 27
(29,28) walkable=True   step up   -> None         nothing comes back
LedgeTiles: 8 entries, facings down/left/right, none for up

GoTo simulated below the ledge, target (29,26):
    from (29,28) -> stops at (32,28)
    from (30,28) -> stops at (27,28)
    from (26,28) -> stops at (31,28)
```

That last block is the failure reproduced from the cartridge in three lines, with no emulator
run: the walker never reaches the waypoint from below, and where it stops is the x=26..32 slide
the 600-attempt corpus reported.

> **Correction while measuring this.** [`docs/agent/pewter-chain.md`](../agent/pewter-chain.md)
> §8.12 and the comment in [`examples/pewter_chain.rs`](../../examples/pewter_chain.rs) both said
> *"walking down from `(29,27)` lands on `(29,29)`"*. Row 27 is the ledge and is not standable;
> the jump is **`(29,26)` → `(29,28)`**, which is what the same section says three paragraphs further down (*"stepping down
> off row 26 does not land on row 27, it jumps to row 28"*) and what the waypoint coordinate
> implies. Both have been corrected.

**The repair is not a bigger budget.** Row 20 — the row the mart door opens onto — crosses the
city cleanly from x=29 to x=19 (measured: every tile walkable), sits six rows above the ledge, and
the rest of the chain finishes even from *below* the ledge. So the leg turns west first, and an
overshoot costs nothing.

**The three occurrences.**

| link | the waypoint | what was one tile away |
|---|---|---|
| take a starter | `(4,4)` | the rival's sprite, and a detour south into [the soft-lock](#the-oaks-lab-soft-lock) |
| the rival battle | approach `(8,4)` | the rival parks there after taking his ball |
| carry it back to Pallet | `(29,26)` | a one-way ledge |

> **A waypoint whose overshoot is unrecoverable is a waypoint pointing at a trap.** Prefer the
> turning point with slack around it, and prefer a route whose later waypoints still work if an
> earlier one is missed.

[`tools/pewter-chain/route_audit.py`](../../tools/pewter-chain/route_audit.py) is that rule made
checkable, and **it is a design aid rather than a gate, which is worth stating rather than
glossing**: it cannot know whether an overshoot is *reachable* (that needs the map's NPCs, and an
NPC in the way is what pushed the walker over the ledge in the first place), and it has no model
of the driver's stuck-sidestep, so it over-reports.

![the swarm crossing Route 1](../agent/chain-stills/4-route-1-pulled-back.png)

*Route 1 is the same shape at scale: 42 one-way ledge drops, so the climb is not the descent
reversed and the chain carries two waypoint lists for one map. A decorrelated random walk over
those ledges drains south and never arrives — the [swarm view](../swarm-view.md) is that null
model.*

---

### The forest is not a scenic route

**What it is.** Route 2's two halves are **separate components of the walkable graph**. Viridian
Forest is not a shortcut through it; it is the only connection.

**How it showed up.** As an assertion in a scenario's objective text, believed and unmeasured,
until the walkability grid existed and could settle it.

**How it was found.** Build Route 2's grid from the cartridge — block map, the tileset's
blockset, its `$FF`-terminated walkable-tile list, probing each 16x16 walk tile at its
**bottom-left** 8x8 (block indices 4, 6, 12, 14) — and flood-fill from each forest gate's warp
tile.

**Evidence, measured now:**

```text
Route 2, map $0D, 20x72 walk tiles
  warps:  (3,43) -> $32  Viridian Forest south gate
          (3,11) -> $2F  Viridian Forest north gate
  connected components, largest first:
     245 tiles, rows 43-71     <- holds the south gate, reaches Viridian
     189 tiles, rows 39-68
     146 tiles, rows  2-21
      86 tiles, rows  0-11     <- holds the north gate, reaches Pewter
```

Neither component contains the other's gate. The forest is the only way between them, which is
why the chain has to cross it — and therefore why the training link exists at all.

**The memory.** [`wCurMap` `$D35E`][wCurMap].

---

### Only one of the forest's three trainers is compulsory

**Kind: a correction of our own belief.**

**What it is.** This project recorded that *"the forest's three bug catchers are unavoidable"*.
Measured per trainer, **only the one at `(2,18)` severs the map on its own.** The other two are
each avoidable; between them they cover every route, which is why blocking all three leaves no
path — and that is what was over-read.

**How it showed up.** A reconnaissance run of the forest link reported **exactly one trainer
battle per attempt**, which contradicted what two scenario files said.

**How it was found.** By re-running the same walkability grid **per trainer** instead of with all
three cones blocked at once, at every sight range from 1 to 8, with and without line of sight
modelled. All three stand facing left (object-data range byte `$D2`, the `STAY` direction
encoding) at `(30,33)`, `(30,19)` and `(2,18)`.

**What it changes and what it does not.** The compulsory fight is roster 3 — a single level 9
Weedle — and **the level-10 precondition the training link exists to satisfy is unchanged**,
because the threshold is that trainer's level plus one. So the repair was right for a reason that
was partly wrong, which is worth saying out loud.

**Evidence.** 3,000 certified attempts: 9,885 battles, **exactly 3,000 won** (one trainer each),
6,885 fled, 94.4 % of the lead's HP left at the exit, no faints. A count that lands exactly on
the attempt count is the strongest form this claim can take.

---

### No Potion is purchasable before Pewter

**What it is.** The first mart in the game that stocks a Potion is Pewter's — which is where the
chain is going. So three consecutive links have no healing but the Pokémon Center, an item branch
with nothing to reach for, and (with a one-Pokémon party) nothing to switch to either.

**How it showed up.** As the explanation for a residual failure that could not be engineered
away: a lead refuses a fight, sets off for the nurse, and meets one or two more encounters before
it clears the grass.

**How it was found.** Read the mart inventory table out of the cartridge rather than looking it
up. It is at `$02442`, `$FE`-framed: a marker byte, a count, the item ids, then `$FF`.

**Evidence, measured now:**

```text
mart 0 (Viridian): POKE_BALL ANTIDOTE PARLYZ_HEAL BURN_HEAL          <- no POTION
mart 1 (Pewter):   POKE_BALL POTION ESCAPE_ROPE ANTIDOTE BURN_HEAL
                   AWAKENING PARLYZ_HEAL
mart 3:            BICYCLE
```

Route 1 and Viridian City carry no item balls either (map object data), and there is no grass in
Viridian City, so training cannot be moved next to the nurse. Between them those three facts say
the residual is a property of the game at that point in it, not of the policy — which is why the
link is
[recorded as accepted rather than certified](../agent/pewter-chain.md#7b-link-11--accepted-at-one-failure-and-why-that-is-not-a-pass)
rather than tuned until it looked green.

![the training grind](../agent/chain-stills/5-the-training-grind.png)

*396 of 597 agents in a wild battle at once, in one patch of Route 1's grass, with a thin stream
walking north to heal. There is nothing else for them to do: the nurse is the only healing in the
game at this point.*

---

## Reading emulated state, which is where our own bugs live

Everything in this section is a defect in something *reading* the cartridge rather than in the
cartridge itself. **No test ROM catches any of them and none can** — the emulator was entirely
correct in every case — which is the same structural blind spot the
[render conformance record](../rendering.md) describes, and the argument for the reference
adapter in [the conformance tool](../conformance.md) being telemetry rather than a framebuffer
hash.

### wCurMap is not a loaded map

**What it is.** `wCurMap` holds a map id long before that map's header has been loaded, and for a
while after a save has been restored. **It has now caught four separate pieces of work in this
repository**, which is why it has an entry of its own rather than a line in a list.

**How it showed up, four times.**

1. **A scripted run during the opening cutscene.** `wCurMap` already reads the bedroom, with
   plausible coordinates, while no map header is loaded at all.
2. **The World Trade Centre work**, installing a room into `wOverworldMap`. Its guard was
   `wCurMap == TRADE_CENTER && wLinkState == IN_CABLE_CLUB` — two bytes that are true for about
   **thirty frames** before `LoadMapHeader` has run. Installing in that window drew the room
   through the *previous* map's header: a screenful of bikes, tables, chairs and counters, and a
   console that then wedged and dragged its cable partner down with it.
3. **A chain film**, drawn from the position tracks. Through the whole of the opening link — 825
   frames, fourteen seconds of a 1× cut — the position bytes are whatever work RAM held: map
   `$FC` at (26,8) for a few frames, then **map 0 at (0,0)**. Drawn literally, all six hundred
   agents stand in Pallet Town's top-left corner and the camera parks on it.
4. **A save-editing attempt.** `TryLoadSaveFile` populates `wCurMap` at the top of the main menu,
   long before CONTINUE is chosen, so it cannot mean "we are in the game".

**How it was found.** Case 2 is the instructive one, because the *obvious* guard does not work
either: a "non-zero width and height" test **does not catch it** — the width read 7, not 0,
restored from the save. What does catch it is asking the engine rather than a byte: reconstruct
all 360 cells of `wTileMap` from the live block map and require an exact match.

**And an exact match against a blank screen is not evidence.** The first version of that check
installed on **frame 1, before the boot logo**, scoring 360/360 — `wTileMap` and `wOverworldMap`
are both untouched work RAM there, so "an exact reproduction" is two uniform buffers holding the
same byte. A minimum-distinct-tiles floor is the fix, and it is the same rule the test-ROM
harness follows when it requires its reference image to have more than one shade before it will
believe a comparison against it.

**What is actually happening.** `LoadMapData` opens with `call DisableLCD`, which spins until
VBlank — so `wCurMap` has changed a whole frame boundary before `LoadMapHeader` runs. And
`LoadSAV` sets `BIT_NO_PREVIOUS_MAP` on `wCurMapTileset` right after restoring the main data,
which makes `LoadMapHeader` clear the bit and **return without loading anything**.

**The memory.** [`wCurMap` `$D35E`][wCurMap] and [`wCurMapWidth` `$D369`][wCurMapWidth] — the
atlas records the trap on `wCurMap` itself, and `wCurMapWidth` is what the chain's first link uses
as its "am I in control?" test.

---

### A warp changes wCurMap before the coordinates

**What it is.** For one frame after a warp, `wCurMap` names the **destination** and
`wXCoord`/`wYCoord` name the **departure's door tile**.

**How it showed up.** Only after a guess was replaced by real data. A film compositor had been
placing maps with a hand-written layout; swapped onto a published atlas with real gaps between
maps, **every one of 600 agents carried exactly one row reading `OAKS_LAB (12,11)`** — and
`(12,11)` is Pallet Town's own `warp_event 12,11, OAKS_LAB, 2`, while Oak's Lab is ten tiles
wide. An agent stood in a void, visibly, in the finished film.

**How it was found.** The old layout **hid it**: the stray point landed on top of a neighbouring
sheet. That is the finding as much as the timing is — *a guess that is roughly right conceals the
data that would correct it.*

**What is actually happening.** It is [the same trap as above](#wcurmap-is-not-a-loaded-map)
arriving mid-track rather than at boot: the map id moves first, the position follows when the
header loads.

**The memory.** [`wCurMap` `$D35E`][wCurMap], [`wXCoord` `$D362`][wXCoord],
[`wYCoord` `$D361`][wYCoord].

**Evidence, and the rule for any consumer of a position track.** Drop any sample whose coordinate
is not on the map it names, count the drops, print the count, and hold the last real tile until
the destination arrives — which is what a warp *is*. Never place it at the origin: that is the
failure that once put six hundred agents on Pallet Town's corner.

---

### A documented sentinel read as anything else

**Kind: a correction of our own belief.**

**What it is.** `wIsInBattle` has **four** documented values and a reader of ours matched three
of them with a catch-all arm.

**How it showed up.** The live trainer panel showed **a battle — an opponent, its level, HP,
stats and moves — while the player was standing in the overworld**, immediately after blacking
out. Everything in it was stale garbage from the battle that had just been lost, presented with
exactly the same confidence as live data. Nothing crashed. Nothing looked broken.

**How it was found.** By reading `ram/wram.asm` above the label, which says it directly: *"lost
battle, this is -1 / no battle, this is 0 / wild battle, this is 1 / trainer battle, this is 2"*.
The reader was `0 => none, 1 => wild, _ => trainer`, and `$FF` is written by `.allPokemonFainted`
in `home/overworld.asm` — the *end* of a battle, not one in progress.

**The memory.** [`wIsInBattle` `$D057`][wIsInBattle] — the atlas carries the warning on the byte.

**Evidence, and the shape of the test that catches it.** The regression test iterates **every
byte `3..=$FF`** and requires "no battle" for all of them. Pinning the enumerated cases alone is
what let it through.

> **A catch-all arm over a documented enum is a defect until the values it swallows are
> enumerated.** This has bitten three times: this one, a film compositor's `if battle:` (truthy
> for wild, trainer *and* blacked-out alike), and the same reader's `_ => Trainer`.

---

### Two map names in our own table were wrong

**Kind: a correction of our own belief.**

**What it is.** `$33` was labelled "Pewter Pokémon Center" and is **`VIRIDIAN_FOREST`**; `$3B`
was labelled "Viridian Forest" and is **`MT_MOON_1F`**.

**How it showed up.** The name goes to an agent in its snapshot and into every report, so **a run
that had just crossed Viridian Forest said it was in a Pokémon Center.** Found by driving the
chain through it and reading the traces.

**How it was found.** The values are the const-list indices `constants/map_constants.asm`
assigns, and nothing in the table may be added without deriving it the same way. Independently
checkable from the cartridge, which is what settles it:

```text
$33  tileset  3   17x24 blocks   6 warps   8 objects   <- Viridian Forest
$3A  tileset  6    7x4  blocks   2 warps   4 objects   <- a Pokemon Center
$3B  tileset 17   20x18 blocks   5 warps  13 objects   <- Mt Moon 1F (a cave)
```

A Pokémon Center is a 7x4 room with two warps. A 17x24 map with eight objects is not one.

**The memory.** [`wCurMap` `$D35E`][wCurMap].

**Evidence.** `every_named_map_is_the_map_the_cartridge_says_it_is` in
[`plugins/agent/src/observe.rs`](../../plugins/agent/src/observe.rs) pins the whole table against
those indices, and asserts that **`$3B` has no name at all** rather than a plausible one:

> **An unnamed map is honest; a wrongly named one is not.**

---

### A screen does not survive a switch

**Kind: a correction of our own belief.**

**What it is.** This project's Gen 1 battle engine kept Reflect and Light Screen standing across a
switch. That is a **Generation 3** behaviour. On the cartridge the screen is gone.

**How it showed up.** It was found while checking *somebody else's* claims — the mechanics
appendix of an outside paper — and on this point the paper was right and we were not.

**How it was found.** The differential oracle that holds the engine to the cartridge compares
stats and damage **inside one battle and never switches**, so a whole corner of the volatile model
had no check on it at all. **That coverage gap is the finding as much as the bug is.** It is now
covered by a harness that drives the real battle menu to a real voluntary switch on the real
cartridge.

**What is actually happening.** `HAS_LIGHT_SCREEN_UP` and `HAS_REFLECT_UP` are bits 1 and 2 of
`wPlayerBattleStatus3` (`constants/battle_constants.asm`), the per-active volatile block;
`SendOutMon`, `EnemySendOut` and `EnemySendOutFirstMon` each write zero to the five bytes from
`wPlayerStatsToDouble` / `wEnemyStatsToDouble` — `$D060`-`$D064` and `$D065`-`$D069`, **all three
status bytes included**.

**The memory.** [`wPlayerBattleStatus3` `$D064`][wPlayerBattleStatus3].

**Evidence.** Measured on the cartridge: `wPlayerBattleStatus3` reads `$04` with Reflect up and
`$00` immediately after a switch, with the opponent's copy at `$00` throughout. The published
agent evaluation was **provably** unaffected — the team builder filters moves on `power > 0` and
both screens are zero-power — which is what made the fix safe to land immediately rather than
re-measure 720 episodes first.

> **A comment that asserts a behaviour is a claim, and a claim with no test is a guess.** The
> wrong note was confident, cited-sounding, and had a passing unit test underneath it pinning the
> wrong value. **When you check somebody else's claims, check them against your own code too** —
> the row where they were right was worth more than the rows where they were not.

---

### A cartridge can tell there is no sound chip

**What it is.** `Gameboy::new` leaves the APU at `None`, which is a machine with **no sound chip**
rather than a quiet one — and Pokémon Blue can tell.

**How it showed up.** The nurse's healing sequence never finishes: the party heals, *"OK. We'll
need your POKéMON."* stays on screen, and the game accepts no input again. Measured: **700
decisions parked at (3,3)**, every walk refused.

**How it was found.** It cost a day, because the symptom looks exactly like a routing bug — the
solver is standing in a Pokémon Center pressing buttons at a nurse and nothing happens.

**What is actually happening.** `Sound::do_cycle` returns immediately when no player is installed,
so a harness that never calls `enable_audio` has no APU at all, and a driver that reads `NR52`
back spins for ever. `sound::SilentPlayer` is the fix and it costs nothing.

**Evidence.** This is the same rule
[`testharness/testroms.rs`](../../testharness/testroms.rs) already records — *never build a test
machine without an APU* — restated by an entirely different consumer. The emulator's own test-ROM
harness had learned it long before the Gen 1 work rediscovered it, which is worth noting on its
own: **a rule written down in one harness does not reach the next one.**

---

## Reading the cartridge's own tables

### The learnset table is indexed by internal number, not by dex number

**Kind: a cartridge mechanic, and the trap it sets for every reader of it.**

**What it is.** `EvosMovesPointerTable` has **190 slots**, and they are indexed by a species'
*internal* index — the number the engine uses on the bus — not by its Pokédex number. Internal
index 1 is **RHYDON**, not BULBASAUR. 39 of the 190 slots belong to no species at all; the table
that says which is `PokedexOrder`, and a zero there is what everybody calls MissingNo.

So there are two ways to get this wrong and they are not the same mistake. Reading *190* slots is
fine. Reading *the first 151* is not — and it is the one that looks right.

**How it showed up.** It has now bitten more than once, most recently firstmate itself, reading
the first 151 pointers and reporting **580 level-up learn entries and 56 evolutions** for the
cartridge. Both numbers parse cleanly, both are plausible, and both are wrong: the true figures
are **728** and **72**. Nothing about the shape of the answer says so.

**What is actually happening**, measured on the retail cartridge:

| read | learn entries | evolutions | by level | by stone | by trade |
|---|---:|---:|---:|---:|---:|
| the first 151 slots | 580 | 56 | 40 | 12 | 4 |
| `PokedexOrder`-filtered, the 151 real species | **728** | **72** | **52** | **16** | **4** |
| all 190 slots, unfiltered | 728 | 72 | 52 | 16 | 4 |

The third row is the part that names the mistake precisely. **Every one of the 39 MissingNo slots
has its own distinct pointer and every one of those records is empty** — no evolution bytes, no
learnset — so walking all 190 gives exactly the right totals. The error is not reading too many
slots. It is reading *the wrong 151*.

**And what the wrong 151 loses is not a fringe.** 27 real species sit at an internal index above
151, and the first 151 slots contain 27 MissingNo in their place. Among the 27 lost:
**BULBASAUR (internal 153), CHARMANDER (176) and SQUIRTLE (177)** — all three starters and their
whole evolution lines, plus ODDISH, BELLSPROUT, RATTATA, GEODUDE, PONYTA, AERODACTYL and PORYGON.
A drill built on the naive read would report that Charmander learns nothing at all, ever.

**The check that catches it.** Gen 1 has **16 stone evolutions**. The naive read produces 12, and
that is the discriminating number. **Trade evolutions are not a check** — there are 4, and the
wrong read also produces 4, because all four (KADABRA, MACHOKE, GRAVELER, HAUNTER) happen to sit
below internal index 152. A check built on the trade count alone would have passed.

**The memory.** None — this is ROM. `EvosMovesPointerTable` is located by property rather than by
a remembered address (`tools/gen1-tables/gen1_tables.py`, `EvosMoves`): the only offset whose 190
consecutive little-endian words all land in `$4000-$7FFF` *and* whose BULBASAUR slot dereferences
to that species' known first evolution bytes. `PokedexOrder` is found the same way and is at
`$41024` on this cartridge.

**Evidence.** Reproduce both rows of the table in one command — the tool prints the correct
figures and the constants it used, so a disagreement is visible rather than silent:

```sh
python3 tools/gen1-tables/learn_table.py --rom "$ROM"
#   level-up learn entries, all told   728
#   evolution entries                  72   52 by level, 16 by stone, 4 by trade
```

The same trap is recorded from the consuming side in
[learning moves](learning-moves.md) and [the battle engine](battle-engine.md); this entry is here
because those are pages you read once you already know the subject, and this is a number somebody
computes on their way in.

**An open question, not work in progress.** Nobody here has looked at what is *in* the 39 zero
slots. They are empty in the evolution and learnset table, but that is one table of many, and the
same 39 indices are what the community's MissingNo folklore is built on. **What those records
contain across the cartridge's other tables, and whether any of it is reachable or interesting,
is its own subject and is deliberately not started.** Recording it here so it is a question with
a home rather than a rumour.

---

## Method, and two mistakes about measurement

### A link that scored 100 per cent on the wrong task

**Kind: a correction of our own belief, and the most valuable entry on this page.**

**What it is.** A scenario whose goal was `map=0x28 AND watch:oak_appeared_in_pallet!=0` — a
position, plus the flag that *arms* Oak's encounter rather than the flag that records it
happening. It solved 6 of 6.

**How it showed up.** It did not. It was caught **by a human reading the results**, not by any
check we had.

**What is actually happening.** `EVENT_OAK_APPEARED_IN_PALLET` is set the instant the player steps
onto row 1; `map=0x28` would be satisfied by walking to the laboratory door under your own steam,
which is *not what the game asks for at that point*. `PalletTownDefaultScript` fires on
`wYCoord == 1` — its own comment reads *"is player near north exit?"* — Oak then **escorts** the
player, and `OaksLabFollowedOakScript` sets `EVENT_FOLLOWED_OAK_INTO_LAB` when the escort's
simulated walk finishes. That flag is the milestone the cartridge records.

**Evidence, and what it changed.** The link was re-authored against the flag and every other link
audited the same way — nine have no flag behind the milestone at all and legitimately use
`wCurMap`; the five that do already used it. Scenarios gained an **`evidence` field** so the next
person can check a goal without re-deriving it, and the checker reports `evidence: NONE STATED`
when it is missing, because a goal nobody can check is back to being a proxy.

**The frozen artefacts of the wrong version are kept.** A wrong version is evidence.

> This is precisely the failure this project criticises in guide-derived work — a goal authored
> from a plausible idea of how the game goes — and the atlas was right there.

The same shape produced [the blackout carve-out](#the-blackout-carve-out) with the roles
reversed: not a *goal* authored from a plausible idea but a *failure* authored from one. Both
times the giveaway was identical — **the scenario contradicted itself, and nobody had read it
against the disassembly.**

---

### The silent dead band

**Kind: a correction of our own belief.**

**What it is.** Two components each deciding whether an action was safe, from their own
arithmetic, with a gap between them that nothing could see.

**How it showed up, twice.** First with a fight threshold at half health and a heal threshold at
a third: the lead settled at **8 of 23 HP** — too hurt to pick a fight, not hurt enough to go and
get healed — and ran from everything for 2,402 decisions. Making the two numbers equal closed it.

Then it **re-opened**, and that is the interesting half. The battle policy's "too hurt" stopped
being a fraction and became **threat-relative** — it weighs the lead's HP against what the Pokémon
in front of it hits for on a critical — and nothing outside a battle can compute that, because
outside a battle there is no opponent. The walk-to-the-nurse rule was still a flat half. So:

```text
seed 98: 100 battles, won 6, fled 94 — 94 of them "too hurt to pick a fight"
         2,500 decisions, lead still level 7, ended walking into the Pokémon
         Center it should have walked into 2,000 decisions earlier
```

**Nothing failed.** No rule was broken, no assertion fired, the party never fainted, and the run
looked healthy right up to the budget. **That is what makes this class expensive: a dead band is
silent, and the only symptom is that the number the link exists to move stops moving.**

**How it was fixed, and why the fix is not a better threshold.** The next improvement to the
policy would break a better threshold too. **The second threshold is removed**: the policy's own
refusal is the signal — when it returns `Run { why: TOO_HURT }` the solver's next overworld
decision walks to the nurse — and the flat half survives only as a floor, for a lead that is hurt
and has not been attacked yet. The link went 99.3 % → 100.0 % on it, and its win rate 59.4 % →
74.4 %.

> **When one component decides whether an action is safe, no other component may decide the same
> thing from its own arithmetic.** Two thresholds that agree today are two thresholds that will
> disagree the next time either is improved, and the gap between them is silent.

**One more thing it changed.** Reading the reason back had been `why.contains("too hurt")`, which
is tolerable for a report and stops being tolerable the moment a reason drives behaviour. The four
run reasons are named constants now, matched by name.

---

## Reproducing the cartridge measurements on this page

Everything above that says *"measured now"* was read off the captain's retail Pokémon Blue while
this page was being written, with the repository's own tools and no emulator run. **No ROM is
distributed here; point `$ROM` at your own.**

```sh
ROM=~/roms/'Pokemon - Blue Version (USA, Europe) (SGB Enhanced).gb'

# the type chart, wild encounters, base stats and species tables, all located
# in the cartridge by signature rather than by a remembered address
python3 tools/gen1-tables/gen1_tables.py --rom "$ROM" --report

# every route the solver walks, replayed over a walkability grid built from the
# cartridge, with LedgeTiles supplying the one-way jumps
python3 tools/pewter-chain/route_audit.py --rom "$ROM"
```

`route_audit.py`'s `Map` is importable, which is how the grids above were printed: it takes a map
id and gives `walkable`, `step` (ledges included and one-way), `reach` and the map's warps and
objects. Three of the figures on this page — the Viridian City ledge, Route 2's two components,
and Viridian Forest's identity by header — are a dozen lines each on top of it.

## Related pages

- [sharp edges](sharp-edges.md) — the same material where it is a *trap*: grouped by what you were
  trying to do when it bit you. This page is the reasoning; that one is the warning.
- [the memory map](memory-map.md) and [the memory atlas](atlas/README.md) — where these addresses
  live and how each was graded. AtlasGB links back into this page by slug.
- [the Pewter chain](../agent/pewter-chain.md) — the campaign most of these came out of, with the
  per-link measurements and the certification bar.
- [the battle policy](../agent/battle-policy.md) — the battle findings in the order a policy needs
  them, with the report that goes beside a pass rate.
- [a paper's Gen 1 claims, checked](paper-claims.md) — eleven outside claims measured on the
  cartridge, and the one of ours that did not survive the checking.
- [the swarm view](../swarm-view.md) — the null model: a decorrelated random walk over the same
  ledges, and where it does not get to.
