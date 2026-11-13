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
- **A hidden panel does no work at all.** `TrainerPlugin::set_active(false)` is set the moment
  the panel is toggled off, and `on_events` returns immediately.
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

## How this was verified

Four checks, in increasing order of what they prove
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

**The save is never the user's.** The ROM and its battery file are copied into `target/`
before anything runs, because the harness presses buttons at menus and the MBC writes saves
next to the ROM path.

### What could not be verified

The cartridge this was developed against **has an empty battery save** — both `.gbsave` files
next to it are entirely zero — so there was no existing game to read a real party out of.
Check 3 reports that honestly rather than showing the title-screen demo's leftovers, and check
4 exists precisely because of it. A human with a save in progress should press `T` and confirm
the panel matches their party; that is the one thing a harness cannot do.

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
3. **Catch odds** — the catch rate is already read; the formula is in `pokered`.
4. **Gender-if-traded** — needs the Gen 2 gender-ratio table.
5. **Yellow, then Gen 2** — see above.
6. **A `peek_bank(bank, addr)` on `DebugView`** so a plugin can read cartridge tables without
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
