# Teacher against student on the move-learning prompts

[← the test rig](test-rig.md) · [the level-up drill](../gen1/level-up-drill.md) ·
[learning moves](../gen1/learning-moves.md) · [the first, battle, result](test-rig-results.md)

**This is the measurement the rig was built for.** A level-up prompt has one
right answer, the cartridge can only ask 840 distinct ones
([learning moves](../gen1/learning-moves.md)), and
[the drill](../gen1/level-up-drill.md) played nine of them out on the retail
cartridge and verified every one against the party the game wrote back. Those
nine are the pool; a teacher and a student answer them from the same save
states; the score is what four moves each left in the party.

**No battle number appears anywhere on this page**, and the battle columns the
rig prints are all zero for every arm — none of these attempts fights anything.

---

## 1. The result

**Plan:** [`scenarios/testrig/move-learning.json`](../../scenarios/testrig/move-learning.json) ·
**scenario:** [`scenarios/learn/answer-a-move-prompt.json`](../../scenarios/learn/answer-a-move-prompt.json) ·
split **`seen`** — the student was distilled from these same nine prompts, so
this measures the distillation and is **not** evidence of generalisation. The
control that is is [§4](#4-the-control-the-seen-split-cannot-give).

| | prompts | correct | of which replace | of which decline |
| --- | ---: | ---: | ---: | ---: |
| **CHARMANDER line** — teacher | 4 | **4** (100.0 % [51.0-100.0]) | 3/3 | 1/1 |
| CHARMANDER line — student | 4 | **3** (75.0 % [30.1-95.4]) | 2/3 | 1/1 |
| CHARMANDER line — mash-a | 4 | 1 (25.0 % [4.6-69.9]) | 0/3 | 1/1 |
| **BULBASAUR line** — teacher | 5 | **5** (100.0 % [56.6-100.0]) | 3/3 | 2/2 |
| BULBASAUR line — student | 5 | **5** (100.0 % [56.6-100.0]) | 3/3 | 2/2 |
| BULBASAUR line — mash-a | 5 | 2 (40.0 % [11.8-76.9]) | 0/3 | 2/2 |

Teacher **9 of 9**, student **8 of 9**, floor **3 of 9**. Every interval is
Wilson; every one of them is enormous, because nine is nine.

**The paired gap, teacher − student:** +25.0 points [-24.0, +74.0] on the
Charmander line over 4 pairs (1 discordant, McNemar p = 1.0000) and
+0.0 points [+0.0, +0.0] on the Bulbasaur line over 5 pairs (0 discordant,
p = 1.0000).

**Are the two lines distinguishable? No, and the rig says so itself:** the
difference of the gaps is +25.0 with a standard error of 25.0, p = 0.317 —
*"cannot be separated at this sample size — consistent with one gap and two
different ceilings"*. **One prompt out of nine is one prompt out of nine.** It
would be wrong to read a Charmander-specific weakness into it, and the task the
student is doing forbids that reading anyway:
[learning moves](../gen1/learning-moves.md) measured that of the 840 decisions
there are 581 distinct (four held, offered) pairs and **the number whose right
answer differs by species is zero**, so a move-learning decision cannot contain
a species-specific skill to be worse at.

### The one it got wrong, and why

The student's single miss is CHARMELEON at level 33, offered SLASH. The right
answer is to forget **GROWL** (slot 2); it forgot **RAGE** (slot 4), leaving
`SCRATCH GROWL EMBER SLASH` where the teacher left `SCRATCH SLASH EMBER RAGE`.

It is not a mystery — the model is four counters and can be read:

| move | dropped | kept | learned drop score |
| --- | ---: | ---: | ---: |
| SCRATCH | 0 | 3 | 0.20 |
| GROWL | 2 | 3 | 0.43 |
| EMBER | 0 | 3 | 0.20 |
| RAGE | 1 | 1 | 0.50 |

RAGE had been dropped once and kept once; GROWL twice and three times. The
student takes the higher rate and drops RAGE. **The teacher does not have a
drop rate — it has a reason** (`GROWL deals no damage the table can price and
SLASH does`), and the reason is invisible on the screen the student is shown.

---

## 2. What the floor arm proves, and why declines are a separate column

`mash-a` is [`tools/agent-docs/hello_adapter.sh`](../../tools/agent-docs/hello_adapter.sh):
it presses A, for ever. It scores **3 of 9 — and every one of the three is a
decline.** It answers no replacement correctly and it never reaches the
scenario's goal on a single attempt (0 of 9; it runs out of model calls).

**A decline's right answer is the move set the prompt arrived with**, so a
player that does nothing at all is scored correct on it. Pooled into one
column, `mash-a` reads as a third of the task; split, it reads as *zero of six
replacements and three of three declines by inaction*, which is the truth. That
split is the captain's own named requirement for this measurement and it earned
itself on the first run: `prompts_split`, pinned by
`declines_are_counted_on_their_own`.

---

## 3. Who the two players are

Both are [`tools/learn-student/policy.py`](../../tools/learn-student/policy.py),
stdlib Python, no cartridge, spoken through the emulator's own adapter contract
— one snapshot on stdin, one reply on stdout — so either is a `command` arm with
no glue.

**The teacher replays what the drill pressed**, matched on the screen with the
digits normalised away (the party's HP is printed beside the box). It is the
teacher *by construction*: the same buttons on the same screens, all nine of
which the drill verified against the cartridge. **It is not a second
implementation of the criterion** — that lives in
[`learn_table.py`](../../tools/gen1-tables/learn_table.py) and only there, and
two implementations of one rule disagree the moment either is improved.

**The student is fitted from the corpus and shown only what a player is shown.**
Three counters, from the 33 corpus rows:

* at `Delete an older move to make room for X?` — P(replace | X), with a base
  rate to fall back on. **What is held is not on that screen**; that is the
  cartridge's doing, not a simplification.
* at `Which move should be forgotten?` — a per-name propensity to be the one
  dropped, plus which slot tends to be chosen as a tie-break.
* at `Abandon learning X?` — what the teacher answered (always YES).

Everything comes from `observation.screen`. The `label` block the drill writes
beside it is ground truth, is never read, and `_forbid_label` raises rather than
quietly scoring a cheat.

---

## 4. The control the `seen` split cannot give

Train the student on **one line only** and ask it the other line's prompts.

| | prompts | correct | replace | decline |
| --- | ---: | ---: | ---: | ---: |
| CHARMANDER prompts, student trained on BULBASAUR | 4 | **0** (0.0 % [0.0-49.0]) | 0/3 | 0/1 |
| BULBASAUR prompts, student trained on CHARMANDER | 5 | **0** (0.0 % [0.0-43.4]) | 0/3 | 0/2 |

**Zero of nine, both directions — worse than doing nothing**, because it gets
the declines wrong too: it replaces where it should refuse, which `mash-a`'s
inertia accidentally gets right.

And the reason is structural and measured, not a shortcoming of the fit: across
the two lines the corpora share **one held move name of eleven** (GROWL) and
**none of the nine offered moves**. The unseen student has never seen a single
move it is being asked about, so every answer falls back to the base rate.

**The finding, stated plainly: nine prompts distil the teacher's answers and
teach nothing about the question.** A screen-only player sees four move names
and one offered name; the thing that generalises — what a move is *worth* — is
not on the screen at all, and is not recoverable from nine examples. That is a
fact about the corpus's size and coverage, and the fix is more of the 840
prompts rather than a better fit to these nine.

---

## 4a. AT SCALE — 432 prompts, 105 species, and the held-out split that works

**The nine-prompt control above said the distillation does not transfer. It was
right about the nine and wrong as a verdict on the method, and scaling settled
which.** The measured cause was coverage — two lines share one held move name of
eleven and none of the nine offered moves — so the test is whether a corpus that
covers the move table transfers. It does.

### The drill, at scale

`tools/learn-student/drill_all.py` emits a plan per species, drills every one in
parallel, and merges the pools. **140 lines, 64.3 s of wall clock on 14 workers
(855 worker-seconds)** — the fan-out the rig's own throughput work bought,
pointed at the drill.

| | |
| --- | ---: |
| species in the cartridge | 151 |
| lines with at least one decision | **140** |
| lines whose walk offers no decision at all | 10 |
| lines that could not be planned | **1** — MEW |
| decisions the plans asked for | **432** (153 declines) |
| decisions drilled and verified against the cartridge | **432** (153 declines) |
| lines that failed | **0** |
| distinct (four held, offered) pairs covered | **277 of the 581** |
| distinct (species, held, offered) decisions covered | **311 of the 840** |
| distinct species asking | **105** |
| corpus rows | **1,469** across 140 files |

**MEW cannot be planned and the reason is the cartridge**: `BaseStats` holds 150
records — its own line is `assert_table_length NUM_POKEMON - 1 ; discount Mew` —
and MEW's live at `MewBaseStats` in another bank, which the planner does not
read. **It is a deliberate exclusion, not a to-do.** MEW has four level-up
learns in its whole life, no move among them is unique to it, and it is
unobtainable in normal play — so a special case for one species buys nothing the
other 150 do not already carry.

**The 581 and the 840 are over every reachable *starting state*** — a Pokémon
can be caught at any level, so `learn_table.py` walks 17,100 trajectories.
`--drill-plan` walks one line per species from level 1, which is a subset: hence
277 of 581 rather than all of them. Covering the rest needs the planner to emit
mid-line starting states, which is a plan-generator change and not a drill one.

### Every planned decision is now drilled — what the 58 lost ones were

An earlier version of this run drilled **370 of the 432** and lost 24 lines. All
four causes were drill limitations rather than anything about the rig or the
criterion, and all four are closed. The A/B is the two binaries on the same box
under the same load, alternating, with everything else identical:

| | lines failed | decisions drilled | wall | worker-s |
| --- | ---: | ---: | ---: | ---: |
| before | 24 | 370 of 432 | 64.0 s / 64.2 s | 854.3 / 857.5 |
| **after** | **0** | **432 of 432** | 64.3 s / 64.7 s | 854.9 / 843.6 |

**Sixty-two more decisions for nothing.** Two rounds each, load average 23-36
throughout, and on the five lines both binaries complete identically (29 events)
the CPU cost is the same to the second: 18.6/15.8/17.7 s before against
16.3/16.1/17.8 s after. The fix is not a trade.

| lines | cause | what it was |
| ---: | --- | --- |
| 14 | **the level-up never happened** | The drill won its battle by pressing A, and A on the move list takes **slot 1** — so a line whose slot 1 is SPLASH, HARDEN or SING could not kill the level 2 MAGIKARP, and one whose slot 1 is TELEPORT *fled*. |
| 9 | **the move to forget was not in the list** | Not a menu bug: `Screen::selected` read `HORN` back for `HORN ATTACK`, so the cursor walk searched for a move that was on the screen the whole time. [The entry](../gen1/discoveries.md#a-menu-row-can-be-truncated-by-the-row-below-it). |
| 5 | **the NO at the delete box did not take** | The same truncation one screen earlier, plus the [off-by-one on the forget list's cursor](../gen1/discoveries.md#the-battle-move-menu-cursor-is-1-based). |
| 1 | **the lead fainted** (`charmeleon`) | It had not. `wIsInBattle` reads `$FF` for an instant on the way out of a battle it *won*, and the drill read that as a blackout — [the entry](../gen1/discoveries.md#a-documented-sentinel-read-as-anything-else). |

**The fix for the first is the shared battle policy, not a second one.**
`terminalgb_plugin_agent::battle::decide` — the same implementation every Pewter
chain link fights through — now chooses the move, reading the move table and the
type chart out of the cartridge; the drill only works the *menu*. Two drill-only
constraints ride on top of it and both are stated where they are applied: it may
never run (running is not a level-up) and it may never use SELFDESTRUCT or
EXPLOSION, because `decide` is right to weigh an explosion for a player with a
bench and it blacks out a rig whose level-up is checked on a survivor. Eight
events on the seven self-destructing lines fainted before that clause existed.

**Five events hold no move that can hurt anything, and they get an opponent that
knocks itself out.** Measured over all 140 lines, exactly five: JIGGLYPUFF at 9
with only SING, KAKUNA and METAPOD at 12 with only HARDEN, MAGIKARP at 15 with
only SPLASH, ABRA at 16 with only TELEPORT. No choice of *move* recovers any of
them — so the preset gives those five a **trainer's** level 2 MAGIKARP holding
SELFDESTRUCT, which faints itself and awards the same 5 experience a kill would.
A trainer battle rather than a wild one is the whole of why ABRA works:
`TryRunningFromBattle` refuses a trainer battle, so the TELEPORT fails and the
turn passes. Gen 1 has no trainer experience bonus, so the arithmetic that makes
5 points unable to cross two level boundaries is unchanged.
`GB_LEARN_DRILL_NO_EXPLODER=1` puts the wild MAGIKARP back and turns all five
red, which is the A/B that says the exploding trainer is doing the work.

**And two of the 140 lines were never being drilled at all.** NIDORAN♂ and
NIDORAN♀ sanitise to the same filename, so one plan overwrote the other and two
workers drilled the survivor into one directory concurrently; underneath that,
[a species cannot be identified by its printed name](../gen1/discoveries.md#two-species-share-one-printable-name).
Both are fixed and both lines drill to 6 of 6.

**One bad event no longer costs the rest of its line.** A drill that gave up
mid-sequence left the machine wherever it stood, and `arm` only lands when the
overworld loop polls `wCurOpponent` — so on CHARMELEON one lost event was
followed by six unreachable ones. The runner rebuilds the machine whenever an
event did not verify or left `wIsInBattle` non-zero; every event arms its own
party from the preset, so nothing is carried between them anyway.

### The result

Both runs use every arm from the same states at the same seeds; `mash-a` is the
press-A floor. The split is by **line**, never by prompt, and it is every fourth
line by name — `tools/learn-student/split_heldout.py`, which needs no seed file
and is the same split whoever runs it.

**Held out — 114 prompts from 35 species lines the student never saw**
(`scenarios/testrig/move-learning-heldout.json`, split `unseen`, the student
fitted to the other 105 lines' 1,085 corpus rows):

| arm | correct | replace | decline |
| --- | ---: | ---: | ---: |
| teacher | **112 of 114** (98.2 % [93.8-99.5]) | 72/72 | 40/42 |
| **student** | **85 of 114** (**74.6 %** [65.9-81.7]) | **50/72** | 35/42 |
| mash-a | 42 of 114 (36.8 % [28.6-46.0]) | **0/72** | 42/42 |

Paired gap teacher − student **+23.7 points [+14.8, +32.6]** over 114 pairs
(27 discordant, all one way, McNemar p = 0.0000); teacher − floor **+61.4
[+46.6, +76.2]**.

**Fitted to all of it — 432 prompts** (`move-learning-all.json`, split `seen`):

| arm | correct | replace | decline |
| --- | ---: | ---: | ---: |
| teacher | 426 of 432 (98.6 % [97.0-99.4]) | 277/279 | 149/153 |
| student | 347 of 432 (80.3 % [76.3-83.8]) | 205/279 | 142/153 |
| mash-a | 153 of 432 (35.4 % [31.1-40.0]) | **0/279** | 153/153 |

Gap teacher − student **+18.3 points [+14.1, +22.5]** over 432 pairs, p =
0.0000. Agreement teacher-student 345 of 432 (79.9 %).

**The floor is the one arm whose figure is not bit-stable, and the reason is
worth knowing.** Two runs of the identical command gave 152 and 153. The teacher
and the student reproduce exactly; `mash-a` ends every attempt by exhausting its
call budget, and the emulator never blocks on a driver — so how many frames have
gone by when the budget runs out depends on how fast the subprocess was, and
whether that lands before or after one more prompt is a coin the box tosses.
It moves the floor by a prompt, never the arms being measured.

**Wall clock:** the whole pipeline is about three minutes — 64.3 s to drill 140
lines, 31 s for the 342 held-out attempts, 1 m 51 s for the 1,296 attempts of
the full pool, at ~860 % CPU on a box at load average 30.

### More coverage did NOT move the held-out rate, and that is the finding

The corpus grew — 370 drilled decisions to **432**, 236 distinct (held, offered)
pairs to **277**, 98 species to **105** — and the held-out student did not move:
**75.3 % [65.8-82.8] on 97 prompts before, 74.6 % [65.9-81.7] on 114 after**,
with replacements 42 of 60 (70.0 %) against 50 of 72 (69.4 %). The intervals sit
on top of each other and the paired gap is unchanged within a point.

**That is worth stating plainly rather than hunting for a better number.** At
nine prompts the binding constraint was coverage, measured: two lines shared one
held move name of eleven and none of the nine offered moves. At a thousand
corpus rows it is no longer coverage — adding 62 decisions and seven species
changed nothing — so what is left is the **policy**, which is three counters and
was never claimed to be more. A network is the next variable to move, not a
bigger corpus.

**The seen rate went the other way**, 83.2 % on 370 to 80.3 % on 432, on
overlapping intervals and a different pool with a differently-fitted model — so
by itself it is not evidence of anything. What it *does* retire is a caveat this
page used to carry: the old pool "dropped its hardest instantiations and nobody
has measured which way it leans". There is nothing dropped to lean now. Splitting
the 432 by whether the old drill could have reached them at all — the 27 whose
slot 1 cannot damage anything — says the newly-reachable ones are if anything
*easier*: the student scores 23 of 27 (85.2 %) on them against 324 of 405
(80.0 %) on the rest, and the teacher 25 of 27 against 401 of 405.

### What changed between 0 of 9 and 42 of 60

**Nine prompts: the held-out student scored 0 of 9 and got the declines wrong
too. 432 prompts: it answers 50 of 72 replacements on lines it has never seen.**
The policy is the same three counters; only the corpus grew. So the nine-prompt
control was measuring **coverage**, exactly as its own overlap figures said, and
the honest reading of a negative held-out result at a tiny sample is *"the
corpus is too small to say"* rather than *"the method does not work"*.

**And the floor is what stops that being read as more than it is.** `mash-a`
scores 36.8 % held out and **0 of 72 replacements** — every one of its correct
answers is a decline it got by inaction. The student's 50 of 72 replacements is
the number that is doing the work.

### The six the teacher cannot get, and why that is a fact about the screen

On the full pool the teacher scores 426 of 432, not 432. It replays a recorded
answer looked up by what is on the screen, and **six screens in the corpus are
character-for-character identical with different right answers**: the
`Delete an older move to make room for X?` box shows the offered move and the
party's HP and *not what is held*, so two prompts on one line offering the same
move to the same Pokémon at the same HP are indistinguishable from inside the
game. The lookup already prefers the exact text and the party's level over the
digit-stripped form — that alone took 18 ambiguous keys of 619 down to 4 of 895
and the teacher from 88 of 97 to 97 of 97 on the held-out pool — and the
remaining six are not resolvable from what a player is shown.

**That is worth more than the six marks.** It is the ceiling on any screen-only
player, teacher included, and it is measured rather than argued.

---

## 5. Reproducing it

Point `$ROM` at your own cartridge; none is distributed here, and neither the
corpus nor the prompt pool is committed — both are the game's own text and
frames, generated into `target/`.

```sh
ROM=~/roms/'Pokemon - Blue Version (USA, Europe) (SGB Enhanced).gb'
FEATS=plugin-battle-control,plugin-trainer,plugin-agent

# 1. the plans, then the drill — with --prompts, which writes the rig's pool
for sp in charmander bulbasaur; do
  python3 tools/gen1-tables/learn_table.py --rom "$ROM" --drill-plan $sp \
      > target/learn-drill/$sp.plan.json
  cargo run --release --features $FEATS --example gen1_learn_drill -- drill \
      --rom "$ROM" --plan target/learn-drill/$sp.plan.json \
      --out target/learn-drill --prompts target/learn-prompts
done

# 2. fit the student on the corpus the drill just wrote
python3 tools/learn-student/policy.py train \
    --corpus target/learn-drill/charmander/agent.ndjson \
    --corpus target/learn-drill/bulbasaur/agent.ndjson \
    --out target/learn-student/both.json

# 3. the comparison
cargo run --release --features scenario --example pewter_chain -- \
    ab --rom "$ROM" --plan scenarios/testrig/move-learning.json
```

The whole thing is **2 s of wall clock** for 27 attempts across three arms.
The controls are `scenarios/testrig/move-learning-unseen-{CHARMANDER,BULBASAUR}.json`,
each wanting a model trained on the other line
(`--corpus target/learn-drill/<other>/agent.ndjson`).

**And at scale** — 140 species lines, the held-out split, about two minutes:

```sh
# every line, in parallel, into one pool
python3 tools/learn-student/drill_all.py --rom "$ROM" --out target/learn-all

# the held-out split: every fourth LINE by name, so it needs no seed file and is
# the same split whoever runs it. Writes corpus-{all,train}.ndjson and pool-{all,test}.
python3 tools/learn-student/split_heldout.py --out target/learn-all

python3 tools/learn-student/policy.py train \
    --corpus target/learn-all/corpus-train.ndjson --out target/learn-student/train.json

cargo run --release --features scenario --example pewter_chain -- \
    ab --rom "$ROM" --plan scenarios/testrig/move-learning-heldout.json

# ...and the seen split, over every prompt, with the student fitted to all of it
python3 tools/learn-student/policy.py train \
    --corpus target/learn-all/corpus-all.ndjson --out target/learn-student/all.json
cargo run --release --features scenario --example pewter_chain -- \
    ab --rom "$ROM" --plan scenarios/testrig/move-learning-all.json
```

**The split is by line, never by prompt.** A prompt whose *line* contributed corpus
rows to the fit is not a held-out prompt, however different its screen looks.

---

## 6. What this cannot tell you

* **The nine-prompt tables are nine prompts.** Every interval on them is wider
  than the effect it brackets, nothing on them separates the two lines, and
  nothing on them is a rate for "move learning". [§4a](#4a-at-scale--432-prompts-105-species-and-the-held-out-split-that-works)
  is where the sample is large enough to say anything, and even there the pool
  is **277 of the 581 distinct pairs** — the rate is for the decisions a line
  walked from level 1 meets, not for the whole table.
* **The split is `seen`** for the headline table. It says the distillation
  carries the teacher's answers on the data it was fitted to. §4 is the only
  generalisation claim on the page and it is negative.
* **No model has been through this.** The student is a counting policy, not a
  network; the rig's `command` arm is the seam a trained one arrives through
  and it is exercised here by a real subprocess, three arms of it. What the
  scale-out establishes is that the *corpus* is large enough to transfer, which
  is the precondition a network would need and the nine did not meet.
* **The pool is every planned decision now — 432 of 432** — so the selection
  effect this bullet used to warn about is gone rather than bounded. What it is
  *not* is every decision the cartridge can ask: `--drill-plan` walks one line
  per species from level 1, which is 277 of the 581 distinct pairs. Reaching the
  rest is a plan-generator change (mid-line starting states), not a drill one.
* **MEW is a deliberate exclusion.** It cannot be planned — its base stats live
  outside the table every other species is in — and it is not worth a special
  case: four level-up learns, none of the moves unique to it, and unobtainable
  in normal play. Named here so the 150 is never read as 151.
* **More corpus is no longer the lever.** 370 decisions to 432 and 98 species to
  105 moved the held-out rate by less than a point in either direction, so the
  binding constraint at this size is the policy rather than the data. That is
  the opposite of what the nine-prompt control was measuring, and both readings
  are correct at their own sample size.
* **The teacher is a replay.** It cannot be wrong on these nine, because these
  nine are what it replays. Its 9 of 9 is a statement about the pool and the
  harness, not about the criterion — the criterion's own justification is in
  [learning moves](../gen1/learning-moves.md), and
  [the drill](../gen1/level-up-drill.md) is where the cartridge agreed with it.
