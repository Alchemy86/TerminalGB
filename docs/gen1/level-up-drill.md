# The level-up move drill

[← Gen 1 area](README.md) · [battling](battling.md) · [learning moves](learning-moves.md) ·
[discoveries](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md)

**[Learning moves](learning-moves.md) counted the questions the game can ask and stated a rule for
answering them. This is that rule played out on the cartridge, one level-up at a time, and
recorded as training data.** The prompts are real prompts on the real screen; the answer is
chosen, the menu is worked, and the party is read back out of WRAM afterwards so the cartridge is
the judge and not the drill.

Two lines are drilled, Charmander's and Bulbasaur's, and the pair is the point: it is what lets
"the student learned the skill" be told apart from "the student learned one Pokémon".

---

## 1. The two numbers, first

Measured on the maintainer's retail Pokémon Blue, both lines walked from level 1 with every
evolution taken.

| | CHARMANDER → CHARMELEON → CHARIZARD | BULBASAUR → IVYSAUR → VENUSAUR |
|---|---:|---:|
| learnset entries in the table, all three forms | 18 | 21 |
| learn events on **one walked line** | **7** | **7** |
| of those, slots filled for free | 2 | 2 |
| of those, a move already held | 1 | 0 |
| of those, genuine **decisions** | **4** | **5** |
| decisions that are a **decline** | **1** | **2** |
| decisions that are a replacement | 3 | 3 |
| events reached, played out, and the party verified | **7 of 7** | **7 of 7** |
| corpus rows produced | **16** | **17** |
| wall clock for the whole line | 6.3 s | 6.5 s |

**The table count and the walked count are different questions and both are worth having.** Each
form in a line repeats its predecessor's moves at shifted levels — CHARMELEON's learnset is
CHARMANDER's with RAGE at 24 instead of 22 — so the 18 and 21 above are what the *cartridge*
holds, and the 7 and 7 are what one Pokémon actually meets on the way to level 100. Nothing is
skipped and nothing is double-counted: an entry whose level has already gone past when the line
evolves into that form never fires.

**A prompt only appears once four slots are full**, which is why 7 events collapse to 4 and 5
decisions. Both species start with two moves and fill the third and fourth for free.

### Every decision, and what the rule said

| line | level | form | offered | answer | why |
|---|---:|---|---|---|---|
| Charmander | 24 | CHARMELEON | RAGE | forget **LEER** | LEER deals no damage the table can price and RAGE does |
| Charmander | 33 | CHARMELEON | SLASH | forget **GROWL** | GROWL deals no damage the table can price and SLASH does |
| Charmander | 46 | CHARIZARD | FLAMETHROWER | forget **RAGE** | RAGE is the worst priceable move held, 20.0 expected damage a turn against 142.5 |
| Charmander | 55 | CHARIZARD | FIRE SPIN | **decline** | the offered move is worth less than all four held |
| Bulbasaur | 22 | IVYSAUR | POISONPOWDER | forget **LEECH SEED** | LEECH SEED deals no damage the table can price and neither does POISONPOWDER; the tie falls through to PP |
| Bulbasaur | 30 | IVYSAUR | RAZOR LEAF | forget **POISONPOWDER** | POISONPOWDER deals no damage and RAZOR LEAF does |
| Bulbasaur | 43 | VENUSAUR | GROWTH | **decline** | the offered move is worth less than all four held |
| Bulbasaur | 55 | VENUSAUR | SLEEP POWDER | **decline** | the offered move is worth less than all four held |
| Bulbasaur | 65 | VENUSAUR | SOLARBEAM | forget **GROWL** | GROWL deals no damage the table can price and SOLARBEAM does |

**Three of the nine are declines and every one of them was exercised**, which was the case the
maintainer named explicitly. A decline is not a skipped prompt: the game asks
`Delete an older move to make room for X?`, the answer is NO, it then asks
`Abandon learning X?`, the answer is YES, and it prints `LEARNER did not learn FIRE SPIN!`. Two
button decisions, both recorded.

---

## 2. Triggering a level-up directly

Grinding to level 65 is not a test of anything. Each event is reached in one battle:

1. **Arm the form the line holds at `level - 1`, with the four moves it would be holding**, via a
   `plugins/battle-control` preset generated from the plan. For the CHARIZARD at 46 that is a
   CHARIZARD at 45 holding SCRATCH, SLASH, EMBER, RAGE — slot order preserved, because the move
   list the prompt shows is in slot order.
2. **Set the experience to one point short of the target level.** `wPartyMon1Exp` (`$D179`, three
   bytes big-endian) is written with
   `terminalgb_plugin_gen1_link::romdata::experience_for_level(growth, level) - 1` — the same
   function the preset itself used to give the Pokémon its experience, so there is no second
   formula to drift.
3. **Kill one level 2 MAGIKARP.** Worth 5 experience, knows nothing but SPLASH, and cannot hurt
   anything the plan ever arms. **Which move does the killing is
   [`terminalgb_plugin_agent::battle::decide`]'s answer, not slot 1** — see §3a.

**The opponent is a MAGIKARP and not the CHANSEY the trace scenarios use, and that is arithmetic
rather than taste.** CHANSEY is worth 182, which at low level crosses two level boundaries at
once and loses the move belonging to the one it skipped — the
[crossing two levels](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md#crossing-two-levels-loses-the-move-in-between) trap. With the
experience written directly, 5 points cannot cross a boundary at any level in the game, so the
same arithmetic works for an event at level 9 and an event at level 65 instead of being tuned per
event.

### 2a. Five events cannot fight, and their opponent knocks itself out

**Measured over all 140 drillable lines, exactly five events hold no move that can damage
anything**: JIGGLYPUFF at 9 with only SING, KAKUNA and METAPOD at 12 with only HARDEN, MAGIKARP
at 15 with only SPLASH, ABRA at 16 with only TELEPORT. Every one is a `free` slot-filling event,
so no *decision* is at stake — but a line that cannot play out its first event used to lose every
event after it, so they had to be got through.

**No choice of move recovers any of them, so the opponent ends the battle instead.** Those five
presets arm a **trainer's** level 2 MAGIKARP holding SELFDESTRUCT, which faints itself and awards
the same 5 experience a kill would.

**A trainer battle rather than a wild one is the whole of why ABRA works.** TELEPORT ends a wild
battle — the ABRA escapes and earns nothing — and `TryRunningFromBattle` refuses a trainer battle
outright, so the TELEPORT fails and the turn passes. SPLASH and HARDEN do nothing either way;
SING costs a few turns of sleep and no more. **Gen 1 has no trainer experience bonus** (that
arrives in Gen 2), so the 5 points and the "cannot cross a boundary at any level" property above
are both unchanged.

**Teeth.** `GB_LEARN_DRILL_NO_EXPLODER=1` answers "this one can fight" for everything, which puts
the plain wild MAGIKARP back in front of the five: all five go red and their lines with them.

**Declared assistance.** Writing the experience is `Gameboy::debug_write`, which
`Gameboy::host_writes()` counts; the drill prints the count at the end (3,017 for a full line,
almost all of it the party write-back below). It is a drill, not a certified run — but an
undeclared advantage makes every number worthless, so it is declared.

---

## 3. Working the prompt

The drill settles the screen, classifies it, acts once, and looks again. Three states, and every
phrase in them was read off a traced screen rather than recalled:

| on screen | what the drill does |
|---|---|
| `Delete an older move to make room for X?` | walk the cursor to **YES** (replace) or **NO** (decline), press A |
| `Which move should be forgotten?` | walk the cursor to the move the rule chose, press A |
| `Abandon learning X?` | walk the cursor to **YES** (decline) or **NO**, press A |

**"Walk the cursor" means read it back, not count presses**, and that distinction cost a run. See
§4. **What is read back is the cursor's *index*, not the text it is sitting on** — see §4 again,
because reading the text cost six lines.

### 3a. The move is the battle policy's, and the menu is the drill's

**The drill used to win its battle by pressing A, and A on the move list takes slot 1.** So a
line whose slot 1 is SPLASH, HARDEN or SING could not kill the MAGIKARP that supplies the
experience, and one whose slot 1 is TELEPORT ran away from it — 14 species lines lost every one
of their decisions to that.

**The choice is [`terminalgb_plugin_agent::battle::decide`], the one battle policy this
repository has**, the same implementation every Pewter chain link fights through: it reads the
move table and the type chart out of the cartridge and both sides out of the two battle structs.
What the drill adds is the *menu* — turning a slot into a keypress without going out of phase,
which is the same split `examples/pewter_chain.rs::fight` makes.

Two drill-only constraints ride on top, and both are stated where they are applied:

* **It may never run.** Running is not a level-up. `decide`'s escape and stat-move branches are
  right for somebody playing the game and unavailable to a rig whose whole purpose is to cross
  one experience boundary, so anything that is not `Fight` falls back to the most damage on offer.
* **It may never use SELFDESTRUCT or EXPLOSION.** `decide` is right to weigh an explosion for a
  player with a bench — it ends the battle, which is usually the point — and it is exactly wrong
  here, because the level-up is checked on a Pokémon that *survived*. Measured: eight events
  across the seven self-destructing lines blacked the party out, and each one took the rest of
  its line with it. `EXPLODE_EFFECT` is 7 (`constants/move_effect_constants.asm`), confirmed
  against this cartridge — those two moves are the only ones carrying it.

[`terminalgb_plugin_agent::battle::decide`]: ../../plugins/agent/src/battle.rs

### The verdict is the cartridge's

After every event the party's level, species and four move slots are read out of WRAM and
compared with what the plan said they would be. A prompt that was answered but produced the wrong
move set is a **failure**, not a pass — which is how the swallowed-press bug in §4 was caught, and
nothing else would have caught it: the screen looked entirely correct while the wrong move was
being forgotten.

The drill also cross-checks its own arithmetic: the plan says which slot the move to forget is in,
and the cursor says which slot it was found at. A disagreement is reported.

**Teeth, proven rather than claimed.** Editing one plan entry to name the wrong move to forget —
EMBER where the rule chose LEER — turns the run red on the party read-back
(`moves [SCRATCH, GROWL, EMBER, LEER] not [SCRATCH, GROWL, EMBER, RAGE]`) and the binary exits 1.
The screens along the way look entirely normal.

---

## 4. Seven things this cost, and what each one teaches

**A tap shorter than `MENU_GAP` is swallowed, and on a move list that forgets the wrong move.**
Gen 1's `HandleMenuInput` delays several frames after accepting a press. Three 12-frame taps moved
the cursor **two** slots, so RAGE replaced EMBER where the rule had chosen LEER — and the
cartridge accepted it without a murmur. The fix is not a longer tap: it is to **read the cursor
back and press Down only while it is not on the move the rule chose**. That is also what a
screen-reading player does, and it is self-verifying.

**The YES/NO box scrolls its own question off the screen.** Gen 1 holds two lines of text and
`Delete an older move to make room for FIRE SPIN?` is three, so by the frame the box is actually
up the only words left are `move to make room / for FIRE SPIN?`. Matching on the opening words
made the box invisible; the drill fell through to "press A"; and **A on that box means YES** — so
a decline silently became a replacement and the move list opened on a decision that had already
been made. Found frame by frame with `GB_LEARN_DRILL_DUMP=1`.

> The general form is worth keeping: **a phrase that identifies a screen must be a phrase that is
> still on it when the screen is asking you something.** The opening words of a Gen 1 sentence are
> usually gone by then.

**A menu row can be truncated by the row below it, and the cursor walk searched for a move that
was on the screen the whole time.** `Screen::selected` ends an item at a blank run followed by a
column where another row of the same list starts a word — right for `FIGHT` over `ITEM`, and on
four left-aligned move names it cuts `HORN ATTACK` down to `HORN`, because `TAIL WHIP`'s `WHIP`
begins where `ATTACK` does. Six lines failed with *"never found TAIL WHIP in the move list after
8 step(s)"*. **Identify a menu row by its index, not by its text**; the text is a cross-check that
must tolerate exactly that truncation and nothing else. The
[discovery](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md#a-menu-row-can-be-truncated-by-the-row-below-it).

**And the two move lists disagree about where their index starts.** The battle move list's
`wCurrentMenuItem` is 1-based — `MoveSelectionMenu` opens `ld a, [wPlayerMoveListIndex] / inc a`
— and the `Which move should be forgotten?` list's is **0**. Measured on RHYDON at level 64 with
`▶` on the first move of each. Subtracting one on the forget list walked to the slot *below* the
move the rule chose and forgot that instead, silently, which is this section's first lesson
arriving a second time.

**`wIsInBattle == $FF` is not a blackout on its own.** The cartridge passes through it on the way
*out* of a battle it won: CHARMELEON at level 8 kills the MAGIKARP, grows to level 9, and the
byte reads `$FF` for an instant while the stat screen is up. The drill read that as a faint and
threw away an event it had just completed. **A blackout is a lead on zero HP** — the same
evidence `longplay::Progress::wiped` uses — and requiring both is the fix.

**One bad event must not cost the rest of the line.** A drill that gives up mid-sequence leaves
the machine wherever it stood — a stat screen, a text box, a battle nobody finished — and `arm`
only ever lands when the **overworld loop** polls `wCurOpponent`. So on CHARMELEON one lost event
was followed by six reporting *"no battle started"*. The runner rebuilds the machine whenever an
event did not verify **or left `wIsInBattle` non-zero**; every event arms its own party from the
preset, so nothing was ever carried between them.

**`BattlePlugin` restores the party the moment `wIsInBattle` reads 0**, which is *after* the
prompts, so a snapshot taken on "there is a party" captures the restored one and writes the
level-up straight back out. Measured: every event after the first read back the previous event's
state. The drill keeps the **best-scoring** snapshot — scored on the three things the verdict
checks — rather than the last one.

**And that same restore lands between an evolution's animation and its commit.** On CHARMELEON at
level 36 the screen prints `LEARNER evolved into CHARIZARD!`, `wPartyMon1Species` reads 178 on
every frame from the level-up to the restore, and with the drill's own write-back disabled the
party ends **empty**. So through an injected party the *form* cannot be verified. The drill says
so and counts it (`ok (form unchecked)`) rather than passing quietly or failing loudly.

**This is a limitation of driving an evolution from a preset, not a finding about the cartridge**,
and it costs this drill nothing: no decision on either line falls on an evolution level
(Charmander evolves at 16 and 36, decides at 24/33/46/55; Bulbasaur evolves at 16 and 32, decides
at 22/30/43/55/65). What evolution *does* on a level-up is measured separately and properly in
[learning moves §6](learning-moves.md#6-evolution-never-cancel-and-here-is-exactly-what-that-costs).

---

## 5. The corpus

**One NDJSON record per button the teacher pressed, in the emulator's own agent format** — the
observation verbatim in `request`, the plan verbatim in `reply`. That is what
`plugins/agent/src/lib.rs` writes for a live agent run, so
[`tools/agent-docs/log_to_dataset.py`](../../tools/agent-docs/log_to_dataset.py) flattens it
unmodified into `(observation, action, outcome)` rows, and a `command` arm of the teacher-versus-
student **A/B test rig** — landing separately as `docs/agent/test-rig.md`, with Charmander and
Bulbasaur as its required pair — speaks that same protocol from the other side. **No new format
was invented and no second comparison harness was built:** the corpus is observation → plan, which
is exactly what [`plugins/agent/src/subprocess.rs`](../../plugins/agent/src/subprocess.rs) defines
as the contract a trained player is plugged in through.

```sh
python3 tools/agent-docs/log_to_dataset.py target/learn-drill/charmander/agent.ndjson --stats
#   decisions            16
#   emulated frames      17550  (293.8 s of game)
```

A row, trimmed:

```json
{"observation": {
   "screen": {"trust": "ok", "kind": "menu",
              "text": "     SCRATCH\n     GROWL\n     EMBER\n    ▶LEER\nWhich move should\nbe forgotten?",
              "selected": "LEER", "menu_cursor_row": 11},
   "game": {"in_game": true, "party": [{"name": "LEARNER", "level": 24, "hp": 69, "max_hp": 69}]},
   "label": {"species": "CHARMELEON", "level": 24, "offered": "RAGE", "answer": "forget",
             "forget": "LEER", "held": ["SCRATCH","GROWL","EMBER","LEER"],
             "flags": ["status-tie","status-victim"],
             "why": "LEER deals no damage the table can price and RAGE does"}},
 "action": {"goal": "Learn RAGE, forgetting LEER.",
            "say": "LEER is the weakest of the four - …",
            "plan": [{"do": "press", "button": "a", "times": 1}]}}
```

Three things about it that are deliberate:

- **`screen` is built by the same `observe` code a live agent is handed**, not by a second reader
  of the same tilemap. It is a *subset* of the agent's full snapshot — the fields a move-learn
  decision can act on — and the schema string says `gen1-learn-drill/1` rather than pretending to
  be the whole thing.
- **`label` is not part of the observation a player receives.** It is the ground truth, kept in
  the same record so a row is self-contained; a pipeline that wants a strictly observable input
  drops it.
- **The teacher's reasoning rides in `say` and `goal`**, which is where an agent's own words go.
  It is the rule's sentence, generated from the criterion, not a hand-written gloss.

`events.csv` beside it is one line per event with the verdict, for a glance.

**The corpus is NOT committed, and must not be.** Its `screen.text` is the game's own text decoded
out of the tilemap — `Which move should be forgotten?`, `SCRATCH`, `GROWL` — which is game data,
and game data is not ours to redistribute whatever the size. It is generated from the reader's own
cartridge into `target/`, by two commands, exactly as
[`docs/gen1/README.md`](README.md)'s licence line requires and for the same reason
`docs/upscale-samples/` is MIT-ROM-only. **The plan is derived from the cartridge too and is not
committed either.**

**It is byte-for-byte reproducible.** Emulation is deterministic, the plan is a pure function of
the cartridge, and the drill takes no randomness: two runs of each line produce identical
`agent.ndjson`. So "regenerate it" is a real instruction and not a hope.

---

## 6. What the drill does **not** decide

**The criterion lives in [`tools/gen1-tables/learn_table.py`](../../tools/gen1-tables/learn_table.py)
and only there.** The plan arrives with the answer already applied; the emulator side triggers
level-ups and works menus. That split is the same one the
[battle policy](../agent/battle-policy.md) records for the battle brain, and for the same reason:
**when one component decides something, no other component may decide the same thing from its own
arithmetic**, because two implementations that agree today will disagree the next time either is
improved, and the gap is silent.

Consequences worth knowing:

- Improving the rule changes the corpus and requires re-running the drill. That is correct — the
  corpus *is* the rule, played out.
- The drill cannot be used to argue the rule is right. It can only show the rule was carried out
  and the cartridge agreed about the result.

---

## 7. What is not here

- **Magikarp.** Training one by switching it in from the box until it learns BITE at level 20 is
  the maintainer's idea and is explicitly parked. Two things this drill built would serve it: the
  direct-experience trigger of §2 works at any level on any species, and the plan emitter already
  reports MAGIKARP's line. Neither has been pointed at it.
- **TMs and HMs.** Out of scope here exactly as they are in
  [learning moves](learning-moves.md) — 55 machines against any compatible species at any level
  is a different and much larger problem.
- **Refusing an evolution.** The lines are walked with every evolution taken, because
  [nothing is ever lost by evolving](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md#cancelling-an-evolution-costs-one-level-and-nothing-else)
  and a line that refuses them is a line no player walks. The `cancel` subcommand still measures
  what a refusal does.
- **MEW.** It cannot be planned — `BaseStats` holds 150 records
  (`assert_table_length NUM_POKEMON - 1 ; discount Mew`) and MEW's live at `MewBaseStats` in
  another bank the planner does not read. **A deliberate exclusion rather than a to-do**: four
  level-up learns in its whole life, no move among them unique to it, and unobtainable in normal
  play, so a special case for one species buys nothing the other 150 do not already carry.
- **Starting states other than level 1.** `--drill-plan` walks one line per species from level 1,
  which reaches 277 of the 581 distinct (four held, offered) pairs the cartridge can ask. The
  rest need the *planner* to emit mid-line starting states — a Pokémon can be caught at any level
  — which is a plan-generator change and not a drill one.
- **A trained student that is a model.** This produces what a student trains from, and
  `--prompts <dir>` now also writes the [A/B rig](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/test-rig.md)'s entry pool: one save
  state per verified decision, taken with the question on the screen, and a `gbtestprompt/1`
  file beside it. A teacher and a counting student have been run against each other on it —
  [the result](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/move-learning-result.md) — and the held-out control there is worth
  reading before anyone plans a bigger corpus. **No model has been through this corpus.**

---

## 8. Reproducing it

No ROM is distributed here; point `$ROM` at your own.

```sh
ROM=~/roms/'Pokemon - Blue Version (USA, Europe) (SGB Enhanced).gb'
FEATS=plugin-battle-control,plugin-trainer,plugin-agent

# 1. the plan: the walked line, with the criterion's answer already in it
python3 tools/gen1-tables/learn_table.py --rom "$ROM" --drill-plan charmander \
    > target/learn-drill/charmander.plan.json

# 2. the drill: trigger every level-up, answer every prompt, check the party
cargo run --release --features $FEATS --example gen1_learn_drill -- drill \
    --rom "$ROM" --plan target/learn-drill/charmander.plan.json \
    --out target/learn-drill

# 3. the corpus, in the shape a training pipeline already consumes
python3 tools/agent-docs/log_to_dataset.py \
    target/learn-drill/charmander/agent.ndjson > charmander.dataset.jsonl
```

`--prompts <dir>` additionally writes the A/B rig's entry pool — see
[the corpus contract](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/test-rig.md#the-corpus-contract). Numbering continues from what
is already in the directory, so drilling both lines into one pool is the ordinary use.

`--level N` runs one level-up on its own, which is how to iterate on a single prompt.
`GB_LEARN_DRILL_DUMP=1` prints the decoded screen, the cursor, what it points at and the live
party at every settle — that is how all four findings in §4 were made, and it is the first thing
to reach for when a prompt is not being answered the way you expect.

The drill **starts a new game**, so it needs no save file, and it stages the ROM into `target/`
first because the MBC writes its `.sav` next to whatever path it is handed.

## See also

- [learning moves, and evolving](learning-moves.md) — the closed table, the rule, and what the
  prompts look like frame by frame.
- [battle control](battle-control.md) — the preset machinery this drives, and the save-safety
  restore that §4 runs into.
- the A/B test rig (`docs/agent/test-rig.md`, landing separately) — what measures a student
  trained on this; its `command` arm speaks the same protocol these rows are written in.
- [discoveries](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md) — the two-level trap and the evolution measurements this rests on.
