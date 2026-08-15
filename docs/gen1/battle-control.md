# Starting a battle from outside the game

> **Keys:** `B` fires the selected preset · `N` chooses the next one. [Every control →](../controls.md)

[← Gen 1 area](README.md) · implemented in [`plugins/battle-control/src/`](../../plugins/battle-control/src/)

**Pokémon Red and Blue decide to start a battle by reading one byte.** Write that byte and the
engine does everything else: the transition, the sprite, the music, the AI, the animations, the
cries, the experience, the end-of-battle bookkeeping. Nothing here simulates a battle — it asks
for one.

That single fact is what makes an *in-game cinematic* possible: a Charizard using Flamethrower
on a Pidgey, rendered by the actual game, produced on demand and identical every time.

---

## 1. The mechanism

`OverworldLoop` (`home/overworld.asm`) tests one byte on every iteration:

```
	ld a, [wCurOpponent]
	and a
	jp nz, .newBattle
```

`.newBattle` calls `NewBattle`, which (unless a map script has suppressed battles) far-jumps to
`InitBattle` (`engine/battle/core.asm`). `InitBattle` copies `wCurOpponent` into
`wEnemyMonSpecies2` and then branches on one comparison:

```
	ld a, [wEnemyMonSpecies2]
	sub OPP_ID_OFFSET          ; 200
	jp c, InitWildBattle       ; below 200: it is a species
	ld [wTrainerClass], a      ; at or above: it is a trainer class
```

So:

| `wCurOpponent` | Result |
|---|---|
| `0` | nothing; this is the resting value |
| `1 … 199` | a **wild** battle with that species, at `wCurEnemyLevel` |
| `200 + class` | a **trainer** battle of that class, roster `wTrainerNo` |

**This is not a trick discovered by poking around — it is what the game's own debug menu
does.** `engine/debug/debug_menu.asm` gives the player a level 20 Rhydon, writes `RHYDON` to
`wCurOpponent`, and calls `InitOpponent`. Whoever left that in the cartridge documented the
technique for us.

### Where the four bytes come from

| Symbol | Address | What to write |
|---|---|---|
| `wCurOpponent` | `$D059` | the trigger — species index, or `200 + class` |
| `wCurEnemyLevel` | `$D127` | the wild opponent's level |
| `wTrainerNo` | `$D05D` | which of the class's rosters (1-based, the game's own numbering) |
| `wBattleType` | `$D05A` | 0 normal, 1 the old man tutorial, 2 Safari Zone |

Derivations for all four are in [memory-map.md](memory-map.md#2-the-battle).

### Three conditions, or nothing happens

`wCurOpponent` is polled **only** by the overworld loop, so writing it while a menu or a text
box is up simply queues the battle until the player has control again. `NewBattle` also refuses
outright in three cases (`home/overworld.asm`):

* `wStatusFlags3` `BIT_ON_DUNGEON_WARP` is set;
* `IsPlayerCharacterBeingControlledByGame` — a cutscene is moving the player;
* `wStatusFlags4` `BIT_NO_BATTLES` is set, which a handful of map scripts do (Mt Moon B2F,
  Pokémon Tower 5F).

None of these are worth forcing. The right behaviour is to wait, and to say so if the wait goes
on — which is what `Phase::Triggered` does, with a message naming all three.

There is a fourth condition that is not the engine's: **a battle with an empty party is an
instant black-out** and looks exactly like a crash. Battle control refuses to fire when
`wPartyCount` is 0 and the preset does not supply a party.

---

## 2. Legitimacy: the hard half

Writing a species and a level is easy. Producing a battle the *engine* considers valid is the
part that bites, and it bites in ways that look like emulator bugs rather than like bad input.

### The party must be coherent

`AddEnemyMonToPlayerParty` and the battle engine recompute nothing. Stats, experience for the
level, types, catch rate and PP all have to be right in the bytes you write. Battle control
delegates all of it to [`party::build_mon`](../../plugins/gen1-link/src/party.rs) — the *same*
function the link partner uses to put a Pokémon on the wire:

* types, catch rate and base stats read from the cartridge's `BaseStats`;
* the five stats computed with the Gen 1 formula for that level and those DVs;
* experience set to exactly the total for that level on that species' growth curve;
* PP read from the cartridge's move table;
* the HP DV derived from the other four, not stored.

There is deliberately **one** encoder. A second one would drift, and a drifted party structure
does not fail loudly — it produces a Pokémon with impossible stats.

Empty party slots are **zeroed**, not left as whatever the save had. A structure that is half
old data and half new is exactly the incoherent state that glitches.

### The disobedience trap

This one cost an afternoon and is the best illustration of the whole problem.

An injected level 50 Charizard, fully coherent, refused to attack:

> `CHARIZARD won't obey!`

`CheckForDisobedience` (`engine/battle/core.asm`) compares `wPartyMon1OTID` against
`wPlayerID`. If they differ the Pokémon counts as **traded**, and a player with no badges is
only obeyed up to level 10 (`wObtainedBadges` raises the ceiling to 30/50/70/101). The
Charizard had been given a made-up trainer id, so the game — entirely correctly — treated it as
somebody else's Pokémon and wasted the turn.

The fix is to fill the original trainer in from the *running game*: `wPlayerID` (`$D359`,
big-endian) and `wPlayerName` (`$D158`). Only the id is compared, but the name is copied too so
the summary screen agrees with it. A preset can still state an `ot_id` explicitly, which is how
you would deliberately build a battle that *tests* the disobedience path.

**The general lesson, which is the reason this is written down:** state that is merely
*well-formed* is not the same as state the engine *accepts*. The game has cross-checks between
structures — this one between a party entry and the player record — and injecting into one
without the other produces behaviour that is correct for the state you created and wrong for
the state you meant.

### Custom teams: the timing rule

Species and level need no patching at all: the engine generates the opponent from
`wCurOpponent` and `wCurEnemyLevel`, and its generation is more legitimate than anything a
tool could substitute. Only DVs, moves and nicknames — things the engine rolls or looks up —
need overwriting, and only if a preset asked.

Where the override goes depends on the battle:

* **Wild** — `LoadEnemyMonData` writes `wEnemyMon` (the 29-byte `battle_struct`) with two
  `BattleRandom` bytes for DVs and the level-up moveset for the level. Overwrite `wEnemyMon`.
* **Trainer** — `ReadTrainer` builds a whole six-slot party in `wEnemyMons` (the 44-byte
  `party_struct` layout, same as the player's) from `data/trainers/parties.asm`. Overwrite
  `wEnemyPartyCount`, `wEnemyPartySpecies`, `wEnemyMons`, `wEnemyMonOT`, `wEnemyMonNicks`.
  The class still supplies the sprite, the name, the music, the AI and the prize money — only
  the six Pokémon change.

**And here is the rule that matters.** The engine fills those structures inside `InitBattle`,
which is not synchronised to a frame boundary. A tool that patches once, on the first frame it
sees `wIsInBattle` set, can be overwritten by the rest of `LoadEnemyMonData` running
immediately afterwards — intermittently, depending on where the frame landed.

The fix is **not a bigger delay**. It is a read-back: keep writing every frame until the bytes
are still yours at the *start* of a frame, which proves the engine has stopped touching them.
A delay is a guess that gets tuned until the failures stop; a read-back is an observation.
(There is still a cap, at 40 frames, so a game that never settles reports that rather than
fighting forever.)

Observed on a real cartridge, replacing the champion rival's roster:

```
enemy party as ReadTrainer left it: 6 — $97, $95, $01, $14, $0A, $1C
enemy party being fought:           2 — $B4, $95
```

---

## 3. What it guarantees about a save

This writes into live game state, so the guarantees have to be stated plainly.

**1. It never writes cartridge RAM.** A `.sav` is written from cartridge RAM
(`$A000-$BFFF`) by [`src/mbc/battery.rs`](../../src/mbc/battery.rs). Every write goes through
one function that refuses any address outside WRAM (`$C000-$DFFF`) and HRAM
(`$FF80-$FFFE`), and *counts* the refusal so a bug cannot be silent. An injected party lives
in WRAM; it can only reach a save file if the **player** then walks into a Pokémon Center and
chooses SAVE, which is a decision rather than an accident.

**2. Everything it writes, it puts back.** The exact byte ranges are snapshotted before the
first byte moves and restored when the battle ends. A preset with no `player =` line never
touches the party at all, so there is nothing to restore — and that form is the one to use
against a save you care about.

**3. Nothing fires by itself.** The plugin is not attached unless it is named in
`plugins = …` or `GB_PLUGINS`; it does nothing until a preset is armed; and while it is
attached the terminal frontend shows it in the Status panel with the `!` write marker (`!!`
once it has actually written). A plugin cannot be active and invisible.

**4. It refuses cartridges it does not know.** The game is identified by title *and* header
global checksum, and then every address it will write is **re-derived from the cartridge's own
machine code** before the plugin is even constructed — `ld [nn],a` is `EA lo hi`, so the
address is right there in the operand. If a byte disagrees with the constant, the plugin
refuses and names both addresses. Yellow and Generation 2 are refused for the same reason.

**5. The demonstration needs no save at all.**
[`examples/gen1_battle.rs`](../../examples/gen1_battle.rs) starts a **new game** — the player is
standing in their bedroom with no Pokémon, which is exactly why the preset supplies a party —
and copies the ROM into `target/` first, because the MBC writes its `.sav` next to the ROM path.

---

## 4. Presets: one keypress, not a config edit

A preset file is the same hand-rolled `key = value` shape the terminal config and the trade
file use, plus `[section]` headers. `player =`, `wild =` and `enemy =` each start a Pokémon,
and every key after one of those lines describes *that* Pokémon.

```text
[charizard-flamethrower]
description = Charizard flamethrowers a Pidgey

player   = Charizard
level    = 50
moves    = Flamethrower, Slash, Fly, Earthquake
nickname = CHARIZARD
dvs      = 15, 15, 15, 15

wild     = Pidgey
level    = 3
dvs      = 5, 5, 5, 5

seed     = 0x1234

do = await-battle 400
do = press-until b FIGHT 900
do = press-until a FLAMETHROWER 300
do = press-until a "used FLAMETHROWER" 400
do = press-until b fainted 900
do = press-until b gained 900
do = mash b 8
do = await-overworld 900
```

A trainer battle names a class instead — by name, resolved against the cartridge's own
`TrainerNames` (`data/trainers/names.asm`), or by number:

```text
[gym-brock]
player   = Squirtle
level    = 12
trainer  = BROCK
roster   = 1
```

…and `enemy =` lines replace that class's roster outright.

Unknown keys are an error rather than a silent no-op: a typo'd `spceies =` should say so, not
quietly fight the wrong thing.

**In the terminal frontend:** `plugins = gen1-battle` in `config.toml` (or `GB_PLUGINS`), then
**`B`** starts the selected preset and **`N`** chooses the next one. `battles = <path>` /
`GB_BATTLES` points at your own file; with none, the built-in presets are used.

**Headless:**

```bash
cargo run --release --example gen1_battle -- --list
cargo run --release --example gen1_battle -- --check          # every preset resolves
cargo run --release --example gen1_battle -- --preset charizard-flamethrower \
    --shots target/battle-shots
```

`--check` is worth having in CI: a preset naming a Pokémon, a move or a trainer class the
cartridge does not have would otherwise only fail when somebody pressed the key.

---

## 5. The cinematic layer

Setting a battle up is half of it. The other half is playing it out — and the interesting
constraint is that a clip has to be the *same* clip every time.

### Why the script is written against the screen

The obvious script language is a list of `(frame, button)` pairs, and it is the wrong one. It
records *this* run rather than describing the sequence, so it breaks the moment anything
upstream shifts by a frame — a different save, a longer name to print, a Pokémon whose cry is
a few frames longer. It is also unreadable: nobody can tell what `A at 214` is for.

So the language is written in terms of what is **on screen**, which Generation 1 makes easy:
the tilemap reads back as text ([reading the screen](reading-the-screen.md)).

```text
do = wait 30                      # 30 emulated frames
do = press a                      # hold A 4 frames, then 4 released
do = press a 8                    # hold 8, release 8
do = mash b 6                     # six presses
do = await FIGHT                  # until the screen says so
do = await "Enemy PIDGEY" 300     # …with a timeout in frames
do = press-until b FIGHT 900      # press every half second until it does
do = await-battle                 # until wIsInBattle is nonzero
do = await-overworld              # until it is zero again
```

Determinism is not lost by doing this — the emulator is deterministic, so the same starting
state and the same script produce the same frames. What `await` removes is *authoring*
fragility, not reproducibility.

`press-until` is the one genuinely looping step, and Gen 1 needs it: several battle messages
hold until a button is pressed, and *how many* presses it takes depends on the length of the
names involved. Counting presses is exactly the brittleness `await` exists to avoid, so the
press count is driven by the screen too.

Two details that are load-bearing:

* **`press-until` presses at the end of each period, not the start.** Gen 1's
  `HandleMenuInput` delays several frames after accepting a button and does not see one that
  arrives inside that delay — which looks precisely like the emulator dropping input. Waiting
  first also means a condition that is already true costs no press at all.
* **A and B are not interchangeable.** Both advance a message box, but B also closes a menu and
  can never open one. So B is right for messages and wrong before a move has been chosen — a B
  press at the move list backs out to the FIGHT menu. And an A press one frame after a box
  closes opens the START menu, which then eats every direction key for the rest of the run.

A step that times out **fails loudly** and says which step and what it was waiting for. A
cinematic that quietly produces a clip of the wrong thing is the failure mode worth designing
out.

### Making a cinematic reproducible

Generation 1 rolls for damage, criticals, accuracy and turn order. Here is exactly what is and
is not guaranteed.

**What the RNG actually is.** `Random_` (`engine/math/random.asm`) advances a 16-bit seed in
`hRandomAdd`/`hRandomSub` (`$FFD3`/`$FFD4`) and mixes in **`rDIV`**, the hardware divider, on
every call. `BattleRandom` is `Random` except in a link battle, where it draws from a shared
nine-entry list advanced by `x*5+1` with no divider in it.

That has three consequences:

1. **You cannot make one roll come out a chosen way by writing the seed.** The divider is
   folded in per call, so the value depends on the cycle the call happens on.
2. **You can still make the whole run identical**, because emulation is deterministic: `rDIV`
   is a deterministic function of cycles since reset. Same starting state + same inputs ⇒
   bit-identical frames. This is the guarantee the repository already relies on for its
   save-state and audio baselines.
3. **Therefore a cinematic is reproducible when its starting point is.** Boot from the same
   place — a save state, or a scripted new game — drive it from a script, and the clip is the
   same clip. Trigger the *same* battle by hand at a different moment and the divider is in a
   different phase, so the low bits differ.

`seed = 0x1234` writes `hRandomAdd`/`hRandomSub` immediately before the trigger. It does not
make the run deterministic on its own — point 1 — but it removes the seed as a *source* of
variation between runs that start from different places, and it is one honest line in the
preset instead of a paragraph of hope.

**The design rule that actually delivers a clip: make the outcome independent of the dice.**
The built-in example is deliberate. A level 50 Charizard out-speeds a level 3 Pidgey by fifteen
times, so turn order cannot change. Flamethrower does around forty times the Pidgey's entire HP
bar, so the damage roll (217–255 of 255) and a critical hit cannot change the result. Fire on a
Normal/Flying target is neutral, so the type chart is not in play either.

That leaves exactly one thing that *could* still vary, and it is worth knowing about because it
is a famous Generation 1 defect: **accuracy is checked against a random byte, so even a
100 %-accuracy move misses about one time in 256.** The disassembly says so itself, in a
comment above the check (`MoveHitTest`, `engine/battle/core.asm`):

```
; if the random number generated is greater than or equal to the scaled accuracy, the move misses
; note that this means that even the highest accuracy is still just a 255/256 chance, not 100%
```

So: **a cinematic that starts from a fixed state is exactly reproducible, and one triggered by
hand has a ~0.4 % chance of a miss on any 100 %-accuracy move.** That is the honest statement,
and it is why the proof below is "run it three times and compare", not "it should be fine".

**Measured**, three consecutive runs of the built-in preset from a scripted new game:

```
--- reproducibility ---
  run 2: frames 1219 (same), hash dac74ad86f8e1854 (same), log (same)
  run 3: frames 1219 (same), hash dac74ad86f8e1854 (same), log (same)
  run 1: frames 1219, hash dac74ad86f8e1854
  PASS every run produced byte-identical frames
```

The hash is FNV-1a over **every frame's framebuffer**, so it is not "the same outcome" — it is
the same pixels, in the same order, for the whole clip.

```bash
cargo run --release --example gen1_battle -- --preset charizard-flamethrower --repeat 3
```

### Recording it

The cinematic composes with the video path already in the tree
([`docs/video-out.md`](../video-out.md)) — nothing about it needs special handling, because it
is just the emulator running.

```bash
# straight into an MP4
cargo run --release --example gen1_battle -- --preset charizard-flamethrower --video - \
  | ffmpeg -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 -i - \
           -pix_fmt yuv420p target/charizard.mp4
```

`--save-state <file> --save-at <frame>` writes a save state at a chosen frame of the clip.
Drop it in the terminal frontend's save slot 1 and press **F8** and the same moment comes back
on screen — which is how a screenshot of the frontend mid-battle is made reproducible, the same
trick `examples/gen1_trade.rs --state` uses for the link-trade pictures.

`--shots <dir>` writes a PNG at every phase change and every 40 frames of the fight, through
the `image` crate — not through `termimage`'s writer, which is *stored* deflate: right for a
60 Hz wire, wrong for a file someone keeps.

This also closes a gap the video documentation itself names. `examples/video_out.rs` says
"headless means no input: this records a game's attract mode, a test ROM, or a demo". A
scripted cinematic is a headless recording *with* input, which is the thing that was missing.

For a live stream, the terminal frontend's `--video-out <fifo>` and the virtual-camera script
work unchanged: press `B`, and what plays out goes down the same pipe.

---

## 6. What actually happens, step by step

From the real cartridge, with the plugin's own log:

```
frame     1: armed 'charizard-flamethrower': wild PIDGEY Lv3
frame     1: triggered wild PIDGEY Lv3            <- party written, then wCurOpponent
frame     4: battle started                        <- the overworld loop noticed
frame     6: opposition set                        <- override read back unchanged
frame   617:   step 2: press B until 'FIGHT'
frame   647:   step 3: press A until 'FLAMETHROWER'
frame   742:   step 4: press A until 'used FLAMETHROWER'
frame  1068:   step 5: press B until 'fainted'
frame  1142:   step 6: press B until 'gained'
frame  1189: battle over; 5 byte ranges restored
```

Three frames from trigger to battle. Everything after that is the game.

---

## 7. What this suggests next

The generative point of the whole page: `wCurOpponent` is one byte that the overworld loop
polls, and there is nothing special about it. **Anything the game polls is a control surface.**

* **`wCurOpponent` and evolution.** `wEvolutionOccurred` and the evolution path are driven the
  same way; the "Charizard flamethrowers a Pidgey" clip has an obvious sibling in "watch this
  Pokémon evolve".
* **Scripted scenarios rather than clips.** The same arm/snapshot/restore discipline applied to
  the bag (`wNumBagItems`), the badges (`wObtainedBadges`) and the event flags (`wEventFlags`)
  would let a preset stage a whole situation — a gym leader fought with the exact team and
  items a player wants to test — and put it all back afterwards.
* **A battle as a test fixture.** Battle control makes a *specific* battle reproducible from a
  cold boot in about twenty seconds. That is a better test harness for anything battle-shaped
  than a save state, because it is readable and editable.
* **The link path is a second way in.** A link battle takes its enemy party from `wEnemyMons`
  and its dice from a shared list, which means a virtual partner can produce a battle without
  writing memory at all — see [driving the link cable](link-from-outside.md).
* **The same reading applies to any game with a disassembly.** The question that produced all
  of this was "what does the main loop poll?". For Generation 2 (`pret/pokecrystal`) and
  Yellow (`pret/pokeyellow`) it is the same question with different answers.
