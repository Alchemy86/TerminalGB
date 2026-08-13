# The live trainer view

A read-only plugin that reads a running Pokémon Red/Blue cartridge and shows the party, the
badges and — during a battle — the opponent's stats, beside the game. It is the emulator's
own answer to the [Ironmon Tracker](https://github.com/besteon/Ironmon-Tracker), and the
second consumer of the plugin surface designed in [`debugging.md`](debugging.md).

Press **`T`** in the terminal frontend.

- Source: [`src/trainer/`](../src/trainer), panel layout in
  [`src/screen/trainer_view.rs`](../src/screen/trainer_view.rs), plugin contract in
  [`src/plugin/mod.rs`](../src/plugin/mod.rs).
- Harness: `cargo test --release --test trainer`
  ([`testharness/trainer.rs`](../testharness/trainer.rs)).

---

## What it shows

**Compact (default).** Every party member on two lines: nickname, status, level, an HP bar
coloured by fraction, then species, `cur/max` and types. A `★` marks a Pokémon that would be
shiny in Gen 2. Below that, the badges held and which gym leader is next. During a battle the
opponent gets its own block: species, level, HP bar, types, status, all four stats and its
moves with PP.

**Detail (`d`).** One Pokémon at a time — `Tab`/`Shift-Tab` cycles the party — with the four
moves and their PP (including PP Ups), the DVs and their total out of 75, the invisible stat
experience, Hidden Power's type and base power, and the shiny-if-traded verdict.

**Sprite.** The focused Pokémon's front picture, decompressed from the cartridge. During a
battle the focus is the opponent, because that is what is being watched.

**Wild-encounter banner.** The moment a catchable wild Pokémon appears, a row across the top
of the terminal names it, prints its DVs, and says whether it **would be shiny if traded to
Gen 2** — see [the section below](#the-wild-encounter-banner). It does not need the panel to
be open, and it is the only part of this feature that shows up uninvited.

| Key | |
|---|---|
| `T` | show / hide the panel |
| `d` | detail view for one Pokémon |
| `Tab` / `Shift-Tab` | next / previous party member |

`d` and `Tab` only bind while the panel is up, so they stay available as game buttons
otherwise. `T` is shifted deliberately: the A/B/Start/Select bindings are configurable
lowercase letters and an unshifted key could collide with somebody's.

---

## Where every address came from

**Not a wiki, and not from memory.** [`pret/pokered`](https://github.com/pret/pokered) was
built at its pinned RGBDS version (1.0.3) and the resulting ROMs checked against the
disassembly's own `roms.sha1`:

```
d7037c83e1ae5b39bde3c30787637ba1d4c48ce2  pokeblue.gbc
ea9bcae617fdf159b045185467ae58b2e4a48b9a  pokered.gbc
```

The cartridge this was developed against has **the same SHA-1 as `pokeblue.gbc`**, byte for
byte. Every address is therefore the value that build's own symbol file gives for a named
`pokered` label, not a number someone typed. `pokered.sym` gives the identical address for
every symbol used here, which is why one table serves both games.

Each constant carries its label in
[`src/trainer/gen1.rs`](../src/trainer/gen1.rs) and
[`src/trainer/rom.rs`](../src/trainer/rom.rs); the headline ones:

| What | Symbol | Address |
|---|---|---|
| Party size | `wPartyCount` | `$D163` |
| Party species list | `wPartySpecies` | `$D164` |
| Party structures (6 × 44 bytes) | `wPartyMons` | `$D16B` |
| Nicknames (6 × 11) | `wPartyMonNicks` | `$D2B5` |
| Badges (bitfield) | `wObtainedBadges` | `$D356` |
| Money (3-byte BCD) | `wPlayerMoney` | `$D347` |
| Trainer name / ID | `wPlayerName` / `wPlayerID` | `$D158` / `$D359` |
| Play time | `wPlayTimeHours` … | `$DA41`, `$DA43`, `$DA44` |
| In a battle? | `wIsInBattle` | `$D057` |
| **The opponent** | `wEnemyMon` | `$CFE5` |
| Your active Pokémon | `wBattleMon` | `$D014` |
| Species names | `MonsterNames` | `07:421E` |
| Move names / data | `MoveNames` / `Moves` | `2C:4000` / `0E:4000` |
| Type names | `TypeNames` | `09:7DAE` |
| Base stats + sprite pointers | `BaseStats` | `0E:43DE` |
| Internal index → Pokédex number | `PokedexOrder` | `10:5024` |

Three traps worth naming, because each silently produces plausible-but-wrong data:

1. **Species indices are not Pokédex numbers.** Gen 1's internal index has Rhydon at `$01`
   and Bulbasaur at `$99`. `MonsterNames` is indexed by internal index; `BaseStats` is indexed
   by *Pokédex* number, so the conversion through `PokedexOrder` has to happen in between —
   exactly as `GetMonHeader` (`home/pokemon.asm`) does, Mew's special case included.
2. **Every HP and stat is big-endian**, on a little-endian machine.
3. **The party and battle structures are different.** `party_struct` is 44 bytes with
   experience and an OT ID; `battle_struct` is 28 without them, so level and stats sit at
   different offsets. Reading one with the other's offsets gives numbers that look almost
   right.

### Games other than Red/Blue

The cartridge is identified by **title *and* header global checksum**, so a translation, a ROM
hack or a randomiser with a shifted layout is refused rather than displayed wrongly. The panel
then says which cartridge it saw and what it does support.

- **Yellow** is [`pret/pokeyellow`](https://github.com/pret/pokeyellow) — the same structures
  at different addresses. It needs its own symbol pass and its own `Game` variant: a table
  swap, not new decoding.
- **Gen 2** is [`pret/pokecrystal`](https://github.com/pret/pokecrystal), which changed the
  party structure itself. It needs a second decoder as well as a second table.

Neither is implemented. Saying so is the point — a tracker that guesses is worse than one that
declines.

---

## Sprites: from the ROM, not from VRAM

There were two honest options.

**VRAM** is trivial: the tiles are already decoded and whatever is on screen is by definition
correct. But VRAM only holds what is *on screen*. In the overworld that is the map and the
player — not one party Pokémon. A party panel sourced from VRAM would be blank almost all the
time, which is not the feature that was asked for.

**The cartridge** gives every Pokémon at any moment, in or out of battle, at the cost of
implementing the compression Gen 1 stores its pictures in. That is what
[`src/trainer/sprite.rs`](../src/trainer/sprite.rs) does, reimplemented from the format as
documented by `pokered` (`home/uncompress.asm`, `home/pics.asm`, `tools/pkmncompress.c`):
a square of up to 7×7 tiles, two bitplanes each run-length coded as 2-bit groups with a
biased unary length encoding, then delta-decoded ("a bit means *toggle*"), then combined by
one of three modes the encoder picks by trying all of them.

**It is checked, not asserted.** With `POKERED_GFX=<dir>` pointing at a built `pokered`
checkout, the harness decompresses thirteen species spanning every "Pics" bank and both
special-cased ones and compares them **byte for byte** against the 2bpp tiles the disassembly
itself produces — the exact oracle, since the ROM's compressed data was produced *from* those
bytes. All thirteen match. Without that directory it still decodes all 151 species and checks
each is square, in range, and not a flat colour.

The sprite is a **second terminal image**, with its own image id, its own cell rectangle, and
its cells marked `skip` — all three rules the render work paid for
([`terminal-render.md`](terminal-render.md)). Sharing the game picture's id would have the two
overwrite each other; sharing cells with text would blank the picture and, on Kitty, freeze
it.

---

## Derived values — what the game hides

The layer that makes this a tracker rather than a prettier party menu. Everything here is
already in memory; the game simply never shows it.
[`src/trainer/derived.rs`](../src/trainer/derived.rs).

**Would it be shiny in Gen 2?** Gen 1 has no shininess, but a Pokémon's DVs travel forward
through the Time Capsule unchanged and Gen 2 derives shininess *entirely* from them. So this
can be answered today, for a save that has never left Gen 1. The rule is `CheckShininess`
(`pokecrystal/engine/gfx/color.asm`): defense, speed and special DVs all exactly 10, and the
attack DV with bit 1 set — one of 2, 3, 6, 7, 10, 11, 14, 15. That is 8 of 16 attack values
with three fixed nybbles: **1 in 8192**. Worth knowing that perfect 15/15/15/15 DVs are *not*
shiny.

**Hidden Power.** Also a Gen 2 rule on Gen 1 data (`pokecrystal/engine/battle/hidden_power.asm`):
the type from the low two bits of the attack and defense DVs, the base power from the top bit
of all four plus the low two bits of special. Never Normal; 31 to 70 power.

**Stat experience.** Gen 1's ancestor of EVs, stored in the party structure and completely
invisible in game. Shown raw, with the conversion `CalcStat` (`pokered/home/move_mon.asm`)
applies.

**DV total** out of 75, including the HP DV — which is not stored but assembled from bit 0 of
the other four, again per `CalcStat`.

### Deliberately not derived

- **The Poké Ball.** There is **no ball field anywhere in the Gen 1 party structure** — the
  concept arrives in Gen 2, which stores it in the caught-data word. It cannot be shown for
  Red/Blue at any price, and inferring one would be fabrication. A future Gen 2 mode gains it.
- **Gender.** Gen 2 derives it from the attack DV against the *species' gender ratio*, and
  that ratio is Gen 2 data present nowhere in a Gen 1 cartridge. Deriving it means embedding a
  151-entry table from `pokecrystal` — a bigger and more debatable step than the rules above,
  so it is named rather than half-built.
- **Type effectiveness, catch odds, route encounters, move power/accuracy.** All computable
  from tables already in the ROM, all additive on this data, none implemented.

---

## Cost

The render path was rescued from 18 fps not long ago ([`terminal-render.md`](terminal-render.md))
and does not get the cost handed back.

- **Nothing is read per frame.** The plugin samples every `SAMPLE_EVERY` (6) emulated frames,
  about 10 Hz — a tenth of a second behind the game, which nobody can see.
- **A hidden panel reads one byte.** `TrainerPlugin::set_active(false)` stops the party read
  entirely; what survives is `watch_encounter`, which peeks `wIsInBattle` at the same 10 Hz and
  returns. In the overworld that is **one byte, six times a second** — the price of the banner
  working when nobody has the panel up, which is the whole point of it.
- **The encounter verdict is latched, not sampled.** Once a wild battle's DVs are read, the
  plugin holds the answer until the battle ends: no further reads, and the banner cannot
  flicker.
- **Sprites are decompressed once per species and cached**, so an unchanged party costs
  nothing, and the encoded image is only rebuilt when the focused species or the palette
  changes.
- With no plugin attached, `PluginHost::frame` is one `is_empty` test per frame.

## Read-only, structurally

The plugin is handed a `DebugView`, which borrows `&Gameboy`. It is not a convention that it
cannot write to the machine — it is the borrow checker. `MemoryManagementUnit::peek` is
likewise `&self`, so it cannot tick the bus, catch the APU up, or trip a register's read
side-effects. (The consequence: the I/O range `$FF00-$FF7F` reads back `0xFF` through `peek`.
Plain memory is exact, which is all a tracker needs.)

---

## The wild-encounter banner

> *"if a wild Pokémon is on screen, can we also confirm if it will be SHINY right there and
> THEN so we don't miss out."*

The party `★` answers the question for Pokémon you already own. This answers it **while a Ball
is still unspent**, which is the only moment the answer changes a decision.

### Why the answer exists that early

Three facts, each from the disassembly, are what make it work
([`src/trainer/encounter.rs`](../src/trainer/encounter.rs)):

1. **A wild Pokémon's DVs are rolled when the battle starts.** `LoadEnemyMonData`
   (`pokered/engine/battle/core.asm`) branches on `wIsInBattle`: a wild Pokémon gets two
   `BattleRandom` bytes into `wEnemyMonDVs`, before the battle is even drawn.
2. **Capture copies those exact bytes.** `_AddPartyMon`
   (`pokered/engine/pokemon/add_mon.asm`) takes its `.copyEnemyMonData` branch for a Pokémon
   caught in battle and copies `wEnemyMonDVs` verbatim into the new party entry. Nothing is
   re-rolled, so the verdict shown at the encounter is still true of the Pokémon in the Ball.
3. **The Gen 2 rule reads only DVs.** `CheckShininess` (`pokecrystal/engine/gfx/color.asm`):
   defense, speed and special all exactly 10 and bit 1 of the attack DV set.

### The wording is the feature

**Gen 1 has no shiny Pokémon at all.** Nothing in Red or Blue is shiny, and no string in this
feature says otherwise — every one of them, at every terminal width, reads *would be shiny if
traded to Gen 2*. A unit test asserts exactly that: it renders the banner at every width from
8 to 200 cells and fails if the word "shiny" ever appears without its qualifier. Getting this
wrong would be worse than not shipping it, because the captain would act on it.

### What gets a verdict, and what does not

| Situation | Verdict? | Why |
|---|---|---|
| Wild encounter (grass, water, fishing, cave) | **yes** | `InitWildBattle` sets `wIsInBattle` = 1 |
| Static / scripted (Snorlax, the birds, Mewtwo, Voltorbs, tower ghosts) | **yes** | `InitOpponent` falls through to `InitWildBattle` — an ordinary wild battle with rolled DVs |
| Safari Zone | **yes**, labelled `SAFARI` | catchable; the Ball being weighed is a Safari Ball |
| Trainer battle | **no** | Balls cannot catch a trainer's Pokémon, and its DVs are the *fixed* pair `ATKDEFDV_TRAINER`/`SPDSPCDV_TRAINER` = `$98`/`$88` — the same for every trainer Pokémon in the game, and never shiny. A verdict there is pure noise. |
| The old man's catching demonstration | **no** | `wBattleType` = `BATTLE_TYPE_OLD_MAN`: real rolled DVs, but the Weedle is never added to the player's party |
| Gift, starter, fossil, in-game trade | **no encounter to warn about** | see below |

**Gift, starter and traded Pokémon are the honest gap.** They never enter a battle, so there
is no moment before the decision — `_AddPartyMon` rolls their DVs with `Random` at the instant
they are handed over (its `; Not wild.` branch), by which time they are already yours. Their
verdict *is* shown, by the party panel's `★` reading the same DVs out of `wPartyMon*DVs`; it is
just retrospective rather than a prompt. There is nothing to fix here — the information does
not exist any earlier.

**Two wild Pokémon you are shown but cannot catch:** an unidentified ghost in Pokémon Tower
(no Silph Scope) and the ghost Marowak on 6F are both refused by `ItemUseBall` even though they
are wild battles. They still get a banner. That is a known, small overclaim of *catchability* —
never of shininess.

### How it is surfaced

The banner takes **a row of its own** across the top of the terminal, above the game picture
and never on it (a terminal graphics image owns whole cells — see
[`terminal-render.md`](terminal-render.md); a test asserts the two rectangles never intersect
at any window size, in both the windowed and fullscreen layouts).

- **A shiny verdict** fills the row: black on bright yellow, `★ WILD NIDORAN L7 WOULD BE SHINY
  IF TRADED TO GEN 2 ★ DV 8/14/10/10/10 …`, padded edge to edge so it reads as a bar rather
  than a sentence. It also raises a toast and rings the terminal bell — **once**, on the frame
  the Pokémon appears, which is when the captain is looking at the game and not at a panel.
- **An ordinary encounter** gets the same row, quietly: the species, the DVs, and *why* it is
  not shiny (`0/3 tens, atk bit yes`). A near miss is interesting, and a banner that only ever
  appears for a shiny gives no way to tell "working, and this one is ordinary" from "not
  working".
- The row **persists for the whole encounter**: the verdict is latched when the battle starts
  and held until it ends, so it cannot flash past.
- `shiny_alert` in `config.toml` — `on` (default: banner + toast + bell), `quiet` (no bell),
  `off` (no row; the verdict still appears in the panel's battle block).

**Terminal frontend only.** The trainer plugin is attached by `tui::load_session` and nowhere
else, so the desktop (`glutin`) and web frontends have neither the panel nor the banner. That
is the existing scope of the plugin, not a decision taken here.

### It does not depend on the boot model

The `Model` axis has landed, and for Pokémon Red/Blue it changes the **default**: the header's
CGB flag is clear, so `Model::Auto` now resolves to `GbMode::Classic` — a real DMG boot — where
this feature was originally developed under CGB compatibility mode. **Nothing here reads the
model, and nothing here is laid out differently under one.**

- `wEnemyMon` is at `$CFE5`, inside `$C000-$CFFF` — the *fixed* WRAM bank. There is no banking
  question at all for the DVs.
- `wIsInBattle` `$D057`, `wBattleType` `$D05A` and the party structures are in `$D000-$DFFF`,
  which goes through `wrambank` — initialised to **1 in both MMU constructors**
  (`MemoryManagementUnit::new` and `new_cgb`) and only ever changed by a write to `SVBK`
  (`$FF70`), which a DMG game never makes. Same physical bytes either way.
- The cartridge is identified from the ROM header's title and global checksum, not from the
  running model, so `Game::detect` is unaffected.
- Red/Blue has no CGB code path — it is a DMG game — so its WRAM layout is one layout.

**Checked on both, not assumed.** `cargo test --release --test trainer` was run under
`GB_MODEL=dmg` and `GB_MODEL=cgb`: three encounters each, every verdict agreeing with
`wEnemyMonDVs` read independently.

The one real difference is that `BattleRandom` is seeded from different startup state, so the
*particular* Pokémon and DVs a given button sequence produces are not the same:

| | `GB_MODEL=dmg` | `GB_MODEL=cgb` |
|---|---|---|
| 1st | PIDGEY L3 `$C4CA` @ step 3 | RATTATA L3 `$9F0B` @ step 13 |
| 2nd | RATTATA L3 `$D42D` @ step 5 | RATTATA L3 `$41CA` @ step 17 |
| 3rd | PIDGEY L3 `$4CCE` @ step 11 | PIDGEY L3 `$3870` @ step 34 |

The harness never asserts specific DVs, only that the verdict matches the memory, so it holds
under either model — but **do not pin a DV value in a test**, and do not expect the printed
species and DVs to reproduce. They also move with *emulation timing*: the Mooneye core-timing
work (real timer, 5-cycle interrupt dispatch, timed OAM DMA) changed every one of these numbers
again, because `BattleRandom` is driven from `DIV` and the frame counter. That the values move
while the verdicts keep agreeing is the point — the check is on the agreement, not the values.

The trainer panel's battle block also gained the opponent's DVs on every battle, trainer
battles included — `$98/$88` showing up every single time is itself worth being able to see.

---

## How this was verified

Five checks, in increasing order of what they prove
([`testharness/trainer.rs`](../testharness/trainer.rs)):

1. **All 151 front sprites decode** to a square of 1..=7 tiles, in range, not a flat colour.
2. **Thirteen of them are byte-identical** to `pokered`'s own 2bpp output.
3. **The cartridge's own battery save is read** through the real boot path.
4. **The panel is compared against the game's own screen.** `wTileMap` (`$C3A0`, 20×18) holds
   Gen 1 charmap codes directly, so decoding it reads back exactly what a player sees. The
   harness plays a new game to the overworld, drives the START menu to **SAVE** so the *game*
   writes an internally consistent save, edits a known party into the party block, fixes
   `sMainDataCheckSum`, boots into it, and then opens the game's own POKéMON menu and compares.
   (A hand-built save **hangs** the game — its map, tileset and sprite state are not
   consistent — which is why the game has to write the base save itself. The battery file is
   `<rom>.sav`, raw cartridge RAM with no header, so SRAM bank 1 begins at file offset
   `0x2000`; see [`src/mbc/battery.rs`](../src/mbc/battery.rs).)

That last one is the real proof, because it checks the addresses against the game rather than
against another copy of our own assumptions:

```
    the game's own party menu:
      |   BULBY      15    |
      |    '         22/ 41|
      |   SPARKY     12 PSN|
      |    '         18/ 34|
```

and the panel, from the same moment:

```
      ┌ Trainer ─────────────────────────────┐
      │A       ID 04660  ¥1,234              │
      │12:34:56  Pokémon Blue                │
      │Badges 2/8  ●○●○○○○○                  │
      └──────────────────────────────────────┘
      ┌ Party (d: detail) ───────────────────┐
      │▸1 BULBY         L15 ████░░░░         │
      │    BULBASAUR   22/41  GRASS/POISON   │
      │ 2 SPARKY    PSN L12 ████░░░░★        │
      │    PIKACHU     18/34  ELECTRIC       │
      │badges: Boulder, Thunder              │
      │next: Misty                           │
      └──────────────────────────────────────┘
```

SPARKY is given the canonical shiny DV pair `$EA`/`$AA` on purpose, so the `★` and the
derived layer are covered by the same run. The game's own CONTINUE screen independently
confirms the badge and play-time addresses (`BADGES 2`, `TIME 12:34`).

5. **A real wild encounter is driven, from a brand-new game.** This is the one the shiny
   verdict stands or falls on. The harness plays a new game, has the *game* write a save,
   edits in a party and one event flag, then **walks**: bedroom → stairs → front door → east
   along Pallet Town → north through the fence gap → Route 1, and paces the grass at the
   southern end until `wIsInBattle` reads 1. It then compares the latched verdict against
   `wEnemyMonDVs` read independently, with the Gen 2 rule **re-implemented in the harness**
   from the pokecrystal constants rather than by calling the code under test — and checks the
   verdict clears when the battle ends. Three encounters per run, in about two dozen steps:

   ```
   [5] a real wild encounter
       walked a new game to ROUTE_1, arriving at (34, 10)
         step   3: PIDGEY L3 wEnemyMonDVs $C4CA -> DV 0/12/4/12/10 -> would not be shiny if traded to Gen 2
                  banner:  WILD PIDGEY L3 · DV 0/12/4/12/10 · would NOT be shiny if traded to Gen 2 (1/3 tens, atk bit no)
         step   5: RATTATA L3 wEnemyMonDVs $D42D -> DV 9/13/4/2/13 -> would not be shiny if traded to Gen 2
         step  11: PIDGEY L3 wEnemyMonDVs $4CCE -> DV 0/4/12/12/14 -> would not be shiny if traded to Gen 2
       3 wild encounter(s) in 11 steps, every verdict agrees
       the same encounter with the canonical shiny DVs $EA/$AA:
          ★ WILD PIDGEY L3 WOULD BE SHINY IF TRADED TO GEN 2 ★ DV 0/14/10/10/10 · nothing is shiny in Gen 1
   ```

   **It fails when it under-delivers.** Asking for three encounters and reporting success on one
   is the same defect class the suite was just cleaned of — a check quietly doing less than it
   claims — so a short run is a FAIL, not a pass with a smaller number. That rule earned its
   keep immediately: under CGB the walk wedged after the second battle and spent its whole
   budget pressing a direction key into a menu. Cause: the post-battle "Got away safely!"
   clean-up pressed **A**, and an A one frame after the text closes opens the START menu, which
   eats every direction key thereafter. It presses **B** now — B advances a text box just as A
   does, closes a menu, and can never open one — and the pacing loop detects a position that
   has not moved for eight steps, presses B and re-anchors, reporting how many times it had to.

   **Two traps this ran into, both worth not rediscovering.** (a) *You cannot teleport the
   player by editing `wCurMap`.* `LoadSAV` (`pokered/engine/menus/save.asm`) sets
   `BIT_NO_PREVIOUS_MAP` on `wCurMapTileset` right after restoring the main data, and
   `LoadMapHeader` (`home/overworld.asm`) sees that bit, clears it and **returns without
   loading the header** — the whole `wCurMapHeader` block lives in the save, so on a continue
   the game trusts it. Change the map without the header and the game reads the new map's
   blocks through the old map's `wCurMapDataPtr`; the observed result is a jump into `rst $38`
   or an illegal opcode within a second. Hence walking. (b) *Oak stops a new game at the north
   exit of Pallet Town* — `PalletTownDefaultScript` opens with
   `CheckEvent EVENT_FOLLOWED_OAK_INTO_LAB / ret nz`, so setting that one bit (`wEventFlags`
   `$D747`, bit 0) skips the entire introduction, the starter choice and the rival battle.
   A shiny wild Pokémon is 1 in 8192, so no run will meet one; what the harness can and does
   check is that the same real encounter struct with the canonical `$EA`/`$AA` pair flips the
   verdict and renders the loud banner.

**The save is never the user's.** The ROM and its battery file are copied into `target/`
before anything runs, because the harness presses buttons at menus and the MBC writes saves
next to the ROM path.

### What could not be verified

The cartridge this was developed against **has an empty battery save** — both `.gbsave` files
next to it are entirely zero — so there was no existing game to read a real party out of.
Check 3 reports that honestly rather than showing the title-screen demo's leftovers, and check
4 exists precisely because of it. A human with a save in progress should press `T` and confirm
the panel matches their party; that is the one thing a harness cannot do.

**Nobody has yet seen a shiny-if-traded banner on a real encounter**, because the odds are 1
in 8192 — the harness proves the reads and the rule against the machine, and proves the loud
rendering with substituted DVs, but the two halves have never met in one live encounter. The
wording and the layout of that banner are worth a human's eyes; `shiny_alert = quiet` is there
for anyone who does not want the bell.

---

## What the Ironmon Tracker contributed, and what was done differently

The tracker is MIT-licensed Lua for BizHawk/mGBA and targets Gen 3, so **its addresses do not
apply and its code was not used** — there is nothing to lift even in principle. What transfers
is what it learned from people staring at it for hours:

**Taken.**
- A **compact default with detail on demand**, rather than one dense screen. The tracker shows
  one Pokémon's essentials and hides everything else behind screens; the split here is
  compact party list versus per-Pokémon detail.
- **HP coloured by fraction** — three bands, red under a fifth, amber under a half. It is the
  fastest thing on the panel to read.
- **One consistent "unknown" glyph** (`Constants.HIDDEN_INFO = "?"`). A blank reads as zero
  and a zero is a lie; stat experience is marked `?` during a battle, where the structure
  genuinely does not carry it, and an empty move slot gets a dash because it really is empty.
- **Surfacing derived and hidden values** as a first-class feature rather than a curiosity.

**Done differently, because Gen 1 is not Gen 3.** There are no abilities, natures or held
items to show; "special" is one stat, not two; DVs run to 15 rather than IVs to 31. That space
goes to DVs, stat experience and the derived layer instead.

**Not taken, and named as a follow-up: the tracking itself.** The Ironmon Tracker does not
just read current memory — it *remembers across a run* which moves an enemy species has used,
what levels were seen, what each route produced. That accumulated knowledge is what makes it a
tracker rather than a live readout, and it is a genuinely strong idea. It is not in this
change: it needs a per-run store with a lifecycle (when does a run start? what resets it?
where is it persisted?) and that is a design conversation, not an increment. The plugin
already samples at a fixed cadence with a snapshot in hand, which is the right place to
accumulate from when it happens.

---

## Follow-ups

In rough order of value:

1. **Per-run memory** — enemy moves seen, levels encountered, route data. The thing above.
2. **Type effectiveness against the current opponent** — the type chart is in the ROM.
3. **Catch odds** — the catch rate is already read; the formula is in `pokered`. It belongs
   *next to the shiny banner*: "would be shiny, and a Great Ball has an N% chance" is the
   complete answer to the question the banner half-answers.
4. **The ghost exception.** Two wild Pokémon get a banner and cannot be caught — an
   unidentified Pokémon Tower ghost and the ghost Marowak on 6F (`ItemUseBall`,
   `pokered/engine/items/item_effects.asm`). Suppressing them needs `wCurMap`, the Silph Scope
   flag and `RESTLESS_SOUL`; small, and not done.
5. **Gender-if-traded** — needs the Gen 2 gender-ratio table.
6. **Yellow, then Gen 2** — see above.
7. **A `peek_bank(bank, addr)` on `DebugView`** so a plugin can read cartridge tables without
   the frontend handing it the ROM; see below.

## Feedback on the plugin contract

This was built as the second consumer of the surface in [`debugging.md`](debugging.md), which
is the point of building it — a contract is only general if something other than its author
can use it. Two gaps found:

1. **`DebugSink::on_events(&mut self, batch: &[Event])` has no view parameter.** A plugin that
   reacts to an event by *reading state* — which is what a tracker does, and probably what most
   plugins do — cannot. `Plugin::on_events` here passes the `DebugView` alongside the events.
2. **There is no banked-ROM read.** `peek` sees only the currently-mapped bank, and mapping
   another would be a write, so a plugin that wants cartridge data (graphics, name tables,
   base stats — all of which live in banks the game is not currently using) has no route to it.
   Until the contract grows a `peek_bank`, the frontend hands the plugin the cartridge image
   at construction. That works, but it means every plugin needing ROM data must be built by a
   frontend that has the bytes, which the contract should not require.

Both are small additions, and both were only visible from the outside.
