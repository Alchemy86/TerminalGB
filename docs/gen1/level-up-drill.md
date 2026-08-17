# The level-up move drill

[← Gen 1 area](README.md) · [battling](battling.md) · [learning moves](learning-moves.md) · [discoveries](discoveries.md)

**[Learning moves](learning-moves.md) counted the questions the game can ask and stated a rule for
answering them. This is that rule played out on the cartridge, one level-up at a time, and
recorded as training data.** The prompts are real prompts on the real screen; the answer is
chosen, the menu is worked, and the party is read back out of WRAM afterwards so the cartridge is
the judge and not the drill.

Two lines are drilled, Charmander's and Bulbasaur's, and the pair is the point: it is what lets
"the student learned the skill" be told apart from "the student learned one Pokémon".

---

## 1. The two numbers, first

Measured on the captain's retail Pokémon Blue, both lines walked from level 1 with every
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
captain named explicitly. A decline is not a skipped prompt: the game asks
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
   anything the plan ever arms.

**The opponent is a MAGIKARP and not the CHANSEY the trace scenarios use, and that is arithmetic
rather than taste.** CHANSEY is worth 182, which at low level crosses two level boundaries at
once and loses the move belonging to the one it skipped — the
[crossing two levels](discoveries.md#crossing-two-levels-loses-the-move-in-between) trap. With the
experience written directly, 5 points cannot cross a boundary at any level in the game, so the
same arithmetic works for an event at level 9 and an event at level 65 instead of being tuned per
event.

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
§4.

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

## 4. Four things this cost, and what each one teaches

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
  the captain's idea and is explicitly parked. Two things this drill built would serve it: the
  direct-experience trigger of §2 works at any level on any species, and the plan emitter already
  reports MAGIKARP's line. Neither has been pointed at it.
- **TMs and HMs.** Out of scope here exactly as they are in
  [learning moves](learning-moves.md) — 55 machines against any compatible species at any level
  is a different and much larger problem.
- **Refusing an evolution.** The lines are walked with every evolution taken, because
  [nothing is ever lost by evolving](discoveries.md#cancelling-an-evolution-costs-one-level-and-nothing-else)
  and a line that refuses them is a line no player walks. The `cancel` subcommand still measures
  what a refusal does.
- **A trained student.** This produces what a student trains from. Whether one learns it is the
  A/B rig's question — a separate piece of work, landing as `docs/agent/test-rig.md` — and **no
  model has been through this corpus.**

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
- [discoveries](discoveries.md) — the two-level trap and the evolution measurements this rests on.
