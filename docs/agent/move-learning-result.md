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

## 4a. AT SCALE — 370 prompts, 98 species, and the held-out split that works

**The nine-prompt control above said the distillation does not transfer. It was
right about the nine and wrong as a verdict on the method, and scaling settled
which.** The measured cause was coverage — two lines share one held move name of
eleven and none of the nine offered moves — so the test is whether a corpus that
covers the move table transfers. It does.

### The drill, at scale

`tools/learn-student/drill_all.py` emits a plan per species, drills every one in
parallel, and merges the pools. **140 lines, 46.4 s of wall clock on 14 workers
(616 worker-seconds)** — the fan-out the rig's own throughput work bought,
pointed at the drill.

| | |
| --- | ---: |
| species in the cartridge | 151 |
| lines with at least one decision | **140** |
| lines whose walk offers no decision at all | 10 |
| lines that could not be planned | **1** — MEW |
| decisions the plans asked for | **432** (153 declines) |
| decisions drilled and verified against the cartridge | **370** (127 declines) |
| distinct (four held, offered) pairs covered | **236 of the 581** |
| distinct (species, held, offered) decisions covered | **268 of the 840** |
| distinct species asking | **98** |
| corpus rows | **1,360** across 127 files |

**MEW cannot be planned and the reason is the cartridge**: `BaseStats` holds 150
records — its own line is `assert_table_length NUM_POKEMON - 1 ; discount Mew` —
and MEW's live at `MewBaseStats` in another bank, which the planner does not
read. A one-species gap, named rather than swallowed.

**The 581 and the 840 are over every reachable *starting state*** — a Pokémon
can be caught at any level, so `learn_table.py` walks 17,100 trajectories.
`--drill-plan` walks one line per species from level 1, which is a subset: hence
236 of 581 rather than all of them. Covering the rest needs the planner to emit
mid-line starting states, which is a plan-generator change and not a drill one.

### The 24 lines that failed, and why

58 of the 432 decisions were lost. Four causes, every one a **drill**
limitation rather than anything about the rig or the criterion:

| lines | cause |
| ---: | --- |
| 14 | **the level-up never happened.** The drill wins its battle by pressing A, and A on the battle menu takes **move slot 1** — so on a line whose slot 1 is SPLASH, HARDEN, SING or a status move it cannot kill the level-2 MAGIKARP, and where slot 1 is TELEPORT it *flees*. Two sub-cases: a damaging move exists further down the list (KADABRA holds TELEPORT then CONFUSION) and choosing the slot would fix it; or none of the four can damage anything (MAGIKARP with only SPLASH, METAPOD with only HARDEN), which no opponent choice fixes. |
| 9 | **the move to forget was not in the list** when the cursor walk looked for it. |
| 5 | **the NO at the delete box did not take**, and the move list opened on what should have been a decline. |
| 1 | **the lead fainted** and no battle started thereafter (`charmeleon`). |

### The result

Both runs use every arm from the same states at the same seeds; `mash-a` is the
press-A floor.

**Held out — 97 prompts from 31 species lines the student never saw**
(`scenarios/testrig/move-learning-heldout.json`, split `unseen`, the student
fitted to the other 96 lines' 1,028 corpus rows):

| arm | correct | replace | decline |
| --- | ---: | ---: | ---: |
| teacher | **97 of 97** (100.0 % [96.2-100.0]) | 60/60 | 37/37 |
| **student** | **73 of 97** (**75.3 %** [65.8-82.8]) | **42/60** | 31/37 |
| mash-a | 37 of 97 (38.1 % [29.1-48.1]) | **0/60** | 37/37 |

Paired gap teacher − student **+24.7 points [+14.8, +34.6]** over 97 pairs
(24 discordant, all one way, McNemar p = 0.0000); teacher − floor **+61.9
[+46.2, +77.5]**.

**Fitted to all of it — 370 prompts** (`move-learning-all.json`, split `seen`):

| arm | correct | replace | decline |
| --- | ---: | ---: | ---: |
| teacher | 366 of 370 (98.9 % [97.3-99.6]) | 241/243 | 125/127 |
| student | 308 of 370 (83.2 % [79.1-86.7]) | 193/243 | 115/127 |
| mash-a | 127 of 370 (34.3 % [29.7-39.3]) | **0/243** | **127/127** |

Gap teacher − student **+15.7 points [+11.6, +19.7]** over 370 pairs, p =
0.0000. Agreement teacher-student 312 of 370 (84.3 %).

**Wall clock:** the whole pipeline is about two minutes — 46.4 s to drill 140
lines, 15.0 s for the 291 held-out attempts, 57.6 s for the 1,110 attempts of
the full pool, at ~1,650 % CPU.

### What changed between 0 of 9 and 42 of 60

**Nine prompts: the held-out student scored 0 of 9 and got the declines wrong
too. 370 prompts: it answers 42 of 60 replacements on lines it has never seen.**
The policy is the same three counters; only the corpus grew. So the nine-prompt
control was measuring **coverage**, exactly as its own overlap figures said, and
the honest reading of a negative held-out result at a tiny sample is *"the
corpus is too small to say"* rather than *"the method does not work"*.

**And the floor is what stops that being read as more than it is.** `mash-a`
scores 38.1 % held out and **0 of 60 replacements** — every one of its correct
answers is a decline it got by inaction. The student's 42 of 60 replacements is
the number that is doing the work.

### The four the teacher cannot get, and why that is a fact about the screen

On the full pool the teacher scores 366 of 370, not 370. It replays a recorded
answer looked up by what is on the screen, and **four screens in the corpus are
character-for-character identical with different right answers**: the
`Delete an older move to make room for X?` box shows the offered move and the
party's HP and *not what is held*, so two prompts on one line offering the same
move to the same Pokémon at the same HP are indistinguishable from inside the
game. The lookup already prefers the exact text and the party's level over the
digit-stripped form — that alone took 18 ambiguous keys of 619 down to 4 of 895
and the teacher from 88 of 97 to 97 of 97 on the held-out pool — and the
remaining four are not resolvable from what a player is shown.

**That is worth more than the four marks.** It is the ceiling on any screen-only
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

# the held-out split is every fourth line by name, so it needs no seed file;
# see docs/agent/move-learning-result.md §4a for the four commands that build
# target/learn-all/{corpus-all,corpus-train}.ndjson and pool-{all,test}
python3 tools/learn-student/policy.py train \
    --corpus target/learn-all/corpus-train.ndjson --out target/learn-student/train.json

cargo run --release --features scenario --example pewter_chain -- \
    ab --rom "$ROM" --plan scenarios/testrig/move-learning-heldout.json
```

---

## 6. What this cannot tell you

* **The nine-prompt tables are nine prompts.** Every interval on them is wider
  than the effect it brackets, nothing on them separates the two lines, and
  nothing on them is a rate for "move learning". [§4a](#4a-at-scale--370-prompts-98-species-and-the-held-out-split-that-works)
  is where the sample is large enough to say anything, and even there the pool
  is **236 of the 581 distinct pairs** — the rate is for the decisions a line
  walked from level 1 meets, not for the whole table.
* **The split is `seen`** for the headline table. It says the distillation
  carries the teacher's answers on the data it was fitted to. §4 is the only
  generalisation claim on the page and it is negative.
* **No model has been through this.** The student is a counting policy, not a
  network; the rig's `command` arm is the seam a trained one arrives through
  and it is exercised here by a real subprocess, three arms of it. What the
  scale-out establishes is that the *corpus* is large enough to transfer, which
  is the precondition a network would need and the nine did not meet.
* **58 of the 432 planned decisions were never drilled**, so the pool is what
  the drill could reach rather than what the plans asked for. The four causes
  are in [§4a](#the-24-lines-that-failed-and-why) and all four are drill
  robustness, but a rate over a pool that dropped its hardest instantiations is
  a rate with a selection effect in it, and nobody has measured which way it
  leans.
* **The teacher is a replay.** It cannot be wrong on these nine, because these
  nine are what it replays. Its 9 of 9 is a statement about the pool and the
  harness, not about the criterion — the criterion's own justification is in
  [learning moves](../gen1/learning-moves.md), and
  [the drill](../gen1/level-up-drill.md) is where the cartridge agreed with it.
