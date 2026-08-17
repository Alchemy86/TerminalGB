# The test rig

**Point it at a fight, a set of starting states and two or more players, and
get back the rates, the intervals, the paired gap between them and a report a
stranger can re-run.** One command.

```
cargo run --release --features scenario --example pewter_chain -- \
    ab --rom <blue.gb> --plan scenarios/testrig/starter-rival-battle.json
```

Everything below is about that command: what it measures, how to set a test
up, how to read what comes back, and — the section people skip and shouldn't —
[what it cannot tell you](#7-what-it-cannot-tell-you).

---

## 1. Why this exists

Every measurement this project has published came from code written once for
that question and then left behind: the four-emulator conformance score, the
81-84 % damage agreement against the cartridge, the ablations that came back
null, the fourteen certified chain links. None of those numbers is wrong. The
problem is what having no instrument costs:

* **No two claims are strictly comparable.** A win rate measured by
  `ChainDriver`'s own bookkeeping and a win rate measured by a Python harness
  over MCP are two different quantities with the same name.
* **Nothing is re-runnable by someone who was not there.** The command that
  produced a figure lived in a shell history.
* **Every new question costs a new harness**, and the harness is where the
  subtle errors live — the sampling rate, the priming, the pairing, the
  denominator.

So: the harness once, with the question as its input. The first question it was
pointed at is in [§6](#6-the-first-result--a-battle-comparison-not-the-move-learning-one), and it immediately found something
the pass rate alone could not see.

---

## 2. What it measures

An **attempt** is one play of one scenario from one starting state at one seed.
A **cell** is all the attempts one *player* made on one *subject*. The report is
a grid of cells.

| | teacher | student | random |
| --- | --- | --- | --- |
| **CHARMANDER** | cell | cell | cell |
| **BULBASAUR** | cell | cell | cell |

* An **arm** is who is pressing the buttons. Three kinds:
  `expert` (the scripted, memory-derived solver — the ceiling),
  `random` (six buttons uniformly — the floor),
  `command` (a subprocess speaking the agent's JSON protocol — how a trained
  player arrives; see [§5](#5-plugging-a-trained-player-in)).
* A **subject** is the thing held constant while the arms vary. Today that is a
  lead species, because that is what the first question needed; it is a struct
  rather than a number so a different opponent or a different moveset is a
  field rather than a rewrite.

Per cell it reports up to three **outcomes**, each with its own rate, interval
and paired gap, because one measured event is rarely the only one worth having:

| outcome | what it asks | applies when |
| --- | --- | --- |
| `prompt answered correctly` | did the player leave the party holding the four moves the pool says it should? | the entry carried an expected answer ([§5a](#5a-measuring-move-learning--the-unit-is-a-prompt)) |
| `goal` | did the scenario's own goal condition fire? | always |
| `battle won` | of the attempts that fought, did it win every battle? | the attempt fought |

An outcome that does not apply to an attempt puts it in **neither** column
rather than scoring it as a failure — "did not fight" is not "lost", and an
un-annotated entry is not a wrong answer.

Beside them come the numbers a rate cannot carry: battles, won/fled/lost, HP the
lead had left when a battle ended, mean decisions, a histogram of *how* the
failures failed, and — when two arms answered the same prompts — how often they
left **the same** four moves, which is not the same question as both being
right.

### It says how far it has got, by default

A long run prints a progress line to **stderr** every five seconds and appends
the same tick to `<out>/progress.ndjson`:

```
progress 1200/2700 (44.4%) · CHARMANDER/random 300/300 · 6m12s elapsed · ~7m50s left
```

On by default, because a run that prints nothing until it finishes cannot be
told from a wedged one, and because this rig exists to be driven by things that
are not people. `--quiet` silences the human line; the file is written either
way. The estimate is a linear extrapolation and is labelled one: the cells are
not equally expensive, so it moves as the run crosses a cell boundary.

### The three things it is careful about

**Arms are paired.** Every arm plays the same entry states, the same seeds and
the same phase offsets. Which entry state, which driver seed and how far into
the world an attempt starts are all functions of the seed alone, so two arms at
seed *N* play the same fight. That makes the right test McNemar's on the
discordant pairs rather than two independent proportions, and it means far
fewer attempts buy the same resolution. A gap between two arms that did **not**
play the same seeds is refused, not approximated.

**Subjects are never averaged.** "The student is nine points behind" that pools
two fights with different ceilings is a number about the mixture and about
neither fight. Each subject gets its own row, its own gap, and — when there is
more than one — an explicit test of whether the *gaps* differ.

**The seen/unseen split is stated, loudly, at the top of every report**, and a
plan that does not declare one is refused:

* `seen` — the player was trained on this fight. Measures **how well the
  distillation worked**. It is *not* evidence of generalisation.
* `unseen` — the player was trained on a different fight. Measures whether it
  learned to *battle* or memorised one.
* `n/a` — no trained player took part; the arms are the expert and its
  baselines, so the numbers are a ceiling and a floor.

The `unseen` control is the one nobody runs. The competition benchmark everyone
quotes is a fixed route, so a memorising agent and a battling agent score
identically on it and the question never comes up.

---

## 3. Setting a test up

A test is a `gbtestrig/1` JSON file, so it is a committed artefact somebody else
can re-run rather than a shell command in somebody's history. The one that
produced [§6](#6-the-first-result--a-battle-comparison-not-the-move-learning-one) is
[`scenarios/testrig/starter-rival-battle.json`](../../scenarios/testrig/starter-rival-battle.json):

```json
{
  "schema": "gbtestrig/1",
  "name": "starter-rival-battle",
  "question": "The rival battle in Oak's lab, one subject per starter. How much of the expert's rate does a player that cannot see the machine get, and is the shortfall the same on every starter?",
  "split": "n/a",
  "scenario": "scenarios/pewter/05-the-rival-battle.json",
  "entry_pool": "target/abrig-states/04-take-a-starter/exits",
  "runs": 300,
  "seed0": 0,
  "idle_spread": 3001,
  "subjects": [
    { "name": "CHARMANDER", "lead_species": 176 },
    { "name": "BULBASAUR",  "lead_species": 153 },
    { "name": "SQUIRTLE",   "lead_species": 177 }
  ],
  "arms": [
    { "name": "teacher", "player": "expert" },
    { "name": "mash-a",  "player": "command",
      "program": "tools/agent-docs/hello_adapter.sh", "args": [] },
    { "name": "random",  "player": "random", "seed_base": 2654435769 }
  ]
}
```

`question` is required in spirit and printed at the top: a rig that does not
make you write it produces tables nobody can interpret six months later.

Every field is also a flag, and a flag wins, so a committed plan can be re-run
with one thing changed without editing it:

```
ab --rom R [--plan FILE] [--scenario S] [--start-dir D] [--runs N]
   [--lead NAME|N ...] [--split seen|unseen|n/a] [--student "CMD ARGS"]
   [--seed0 N] [--idle-spread N] [--workers N] [--out DIR]
```

`--lead` takes a species **name** — resolved from the cartridge's own
`MonsterNames`, never from a table written down here — or an index, and may be
repeated once per subject.

### Where the entry states come from

The rig needs a pool of starting states, and the pool is the *previous* link's
successful exits. `corpus --exits N` writes them:

```sh
ROM="$HOME/roms/Pokemon - Blue Version (USA, Europe) (SGB Enhanced).gb"
BIN=target/release/examples/pewter_chain
prev=""
for link in 01-new-game 02-leave-the-house \
            03-oak-takes-you-to-the-lab 04-take-a-starter; do
  args=(corpus --rom "$ROM" --scenario scenarios/pewter/$link.json \
        --runs 700 --workers 10 --exits 700 --out target/abrig-states/$link)
  [ -n "$prev" ] && args+=(--start-dir "$prev")
  "$BIN" "${args[@]}"
  prev="target/abrig-states/$link/exits"
done
```

Measured on a twenty-core box at ten workers: **700 exits through four links in
2 m 27 s**, every link 700/700. The starter is a one-tile difference in link 04
(`ball_x = 6 + seed % 3`), so the pool comes out very nearly evenly split —
234 / 233 / 233 for Charmander, Bulbasaur and Squirtle out of 700.

**Reading the lead species out of a state costs one machine and no emulated
frames**: the state carries the party, so the rig loads each one, reads
`wPartyMon1Species` through the `lead_species` named watch, and files it under
a subject. That is the whole of "a different starter is a flag rather than a
rewrite".

---

## 4. Reading what comes back

`report.txt` and `report.json` land in `--out` (default
`target/testrig/<plan name>/`), and the text is also printed. One block per
subject:

```
── CHARMANDER ─────────────────────────────────────────────
234 entry state(s) in the pool

  arm          goal              battles  won    fled   lost   HP left  decisions
  teacher      100.0% [98.7-100.0]     300  100.0%   0.0%   0.0%    47.4%      66.9
  mash-a         0.0% [0.0-1.3]         0    0.0%   0.0%   0.0%     0.0%     250.0

  arm          battles won (of the attempts that fought)
  teacher       300 of 300  100.0% [98.7-100.0]
  mash-a          0 of 0         —

  GAP [goal] teacher − mash-a: +100.0 points [+88.7,+111.3] over 300 pair(s)
    (300 only teacher, 0 only mash-a; McNemar p=0.0000)
```

* **The interval is Wilson, not the normal approximation.** The one everybody
  reaches for is indefensible exactly where this rig lives: at 300 attempts and
  a rate near 1.0 it puts the upper bound above 100 %, and at a rate of 0 it
  returns `[0, 0]` — certainty from no evidence.
* **The gap's standard error is `sqrt(b + c) / n`**, the conventional McNemar
  form, deliberately not the maximum-likelihood `(b + c − (b−c)²/n)/n²`. The
  corrected form degenerates to *zero* when every discordant pair falls the same
  way, so a run of two attempts would print a hundred-point gap with an interval
  of zero width — which looks like certainty and is an artefact. The
  conventional form is slightly conservative and cannot manufacture confidence.
  A gap interval can therefore exceed 100 points; that is the estimator being
  honest at the boundary, not a bug.
* **`only teacher` / `only arm` are the discordant counts.** Only pairs where the
  two arms disagreed carry any information about which is better; a pair both
  won or both lost says nothing. That is why the McNemar `p` can be decisive on
  a handful of pairs.
* **`endings:` is the failure histogram** — `stuck`, `out-of-calls`, `failed`,
  `violated`. A rate says an arm failed; this says how, and the two failure
  modes above are different findings (see `looping()` in
  [`src/scenario/run.rs`](../../src/scenario/run.rs)).

### Where the time went

```
WHERE THE WALL CLOCK WENT
  arm          attempts    frames   worker-s  emulating   setup   deciding/loop
  teacher             6     51165       5.0      89.6%    10.2%            0.2%
  screen              6    371213      38.3      98.1%     1.7%            0.2%
  ALL                12    422378      43.3      97.1%     2.7%            0.2%
```

**A rig like this is worth having only if it is fast**, so it says which part to
attack rather than leaving anyone to guess. `setup` is building the machine,
burning the phase offset and loading the start state — the per-attempt tax an
*episode* pays, and the number that decides whether short episodes are
affordable.

**`deciding/loop` is NOT the player's think time, and reading it as such is the
trap.** The emulator never blocks on a driver, so a subprocess arm's
spawn-and-decide happens while the machine keeps running and is charged to
`emulating`. A slow player shows up as **more frames per attempt** — the world
runs on while it thinks — which is why `frames` is in the table beside the
shares.

When there is more than one subject, a final block asks the question the
subjects were chosen for:

```
IS THE GAP THE SAME ON EVERY SUBJECT?
  [battle won] arm : CHARMANDER vs BULBASAUR — gaps +0.0 and +50.0,
    difference −50.0 (se 7.1, p=0.000)
    THE GAP ITSELF DIFFERS — something subject-specific is happening in this arm
```

Two subjects are independent samples, so the difference of their two paired gaps
has variance equal to the sum. If the gap is the same on two fights whose
absolute rates differ, the distillation is subject-independent and the fights
merely have different ceilings. If the gap moves, something subject-specific is
happening in the player under test. **When the sample cannot separate the two,
it says so** rather than picking the flattering reading.

---

## 5. Plugging a trained player in

An arm of kind `command` is a subprocess speaking the contract in
[`plugins/agent/src/subprocess.rs`](../../plugins/agent/src/subprocess.rs) —
state snapshot on stdin, plan on stdout, exit 0:

```sh
ab --rom R --plan p.json --split unseen \
   --student "python3 -m agentgb.adapter --policy runs/link05/policy.npz"
```

**A player that looks at the screen is given one.** The snapshot carries a
`frame_png` field pointing at the current frame whenever the arm is a
subprocess; the expert and the random baseline keep the pixel skip, which is
worth about half a frame's cost. A machine that is not drawing hands a
screen-only policy whatever was last in the buffer, and that is not a failure a
rate would ever explain — so the skip is conditional on nobody wanting the
picture, and the arm that wants it pays for it.

The smallest possible adapter is
[`tools/agent-docs/hello_adapter.sh`](../../tools/agent-docs/hello_adapter.sh),
eighteen lines, and it is used as a real arm in §6. The one to **copy** is
[`tools/testrig/screen_adapter.py`](../../tools/testrig/screen_adapter.py):
stdlib-only Python that decodes `frame_png` and returns a plan, written so that
swapping its `decide()` for a forward pass makes it a real student arm. It also
documents by example the rule that matters — **a screen-only player must ignore
every other field in the snapshot**, because `screen.text`, the position and the
party are all derived from memory, and reading `game.party[0].hp` is reading the
answer out of RAM.

Verified end to end: run as an arm it decodes a **160×144, 4-shade** frame on
every decision and its plans are executed like any other player's.

### Where the rig belongs, and why the student is not in this repository

**The trained network lives in [AgentGB](https://github.com/Alchemy86/AgentGB)
and must not be copied here.** AgentGB already depends on TerminalGB as its
environment — it drives `terminalgb-mcp` over JSON-RPC on stdin/stdout
(`src/agentgb/mcp.py`) — and it already has the shape of this comparison in
`src/agentgb/baselines.py`: teacher, random, open-loop and student on the same
starts.

The two repositories have **two different harnesses**, and that is the whole
difficulty. AgentGB's `LinkEnv` presses buttons over MCP with its own settle
frames, its own press timing and its own goal judgement; TerminalGB's
`ScenarioRun` has its own sampling clock, referee and budget. A comparison whose
two halves ran through different harnesses is not a comparison. So one of them
has to be the one, and the cheapest way to guarantee both arms ran through the
same code is to have one code path — which is what
[`play_with`](../../examples/pewter_chain.rs) is: an arm is a driver and nothing
else, and the entry state, phase offset, scenario, referee, sampling clock and
battle bookkeeping are literally the same for every player.

That gives two workable arrangements, and they answer slightly different
questions:

1. **The student comes to TerminalGB's harness**, as a `command` arm. Everything
   needed for this is landed and described above. The student runs one process
   per decision, which is a few milliseconds against a network forward pass.
2. **The teacher goes to AgentGB's harness.** AgentGB's own teacher is a
   *navigator* (`src/agentgb/expert.py`, a shortest path over a walked graph);
   it has no move-choosing battle policy, and `src/agentgb/battle.py` only
   *detects* battles and whether they can be fled. TerminalGB owns the battle
   brain ([`plugins/agent/src/battle.rs`](../../plugins/agent/src/battle.rs)),
   and today it is reachable only from a Rust plugin inside the emulator. Making
   it callable from outside — "what would the expert do in this battle?" — is
   the piece TerminalGB would still owe, and it is also what AgentGB would need
   to label battle states for a DAgger round.

Route 1 is what this page documents because it needs nothing from either side
that is not already here. Route 2 is a real and separate piece of work and
belongs to whoever takes the AgentGB half.

---

## 5a. Measuring move learning — the unit is a PROMPT

**This is what the rig is for.** When a Pokémon levels up already knowing four
moves, the game stops and asks which one to forget.
[`docs/gen1/learning-moves.md`](../gen1/learning-moves.md) shows the set of
questions the cartridge can ever ask is finite and enumerable — 840 distinct
decisions across 151 species — so a level-up prompt is a **closed** problem with
one right answer per case, which is exactly what a distillation can be measured
on. The unit of measurement is therefore a prompt, not an episode: *what was
held, what was offered, what the teacher did, what the student did, do they
agree, and across how many.*

### The corpus contract

A producer of prompts — the level-up drill, or anything else — owes this rig
exactly two things, and **nothing about its own training-corpus format needs to
be agreed**:

1. **A save state per prompt**, `<pool>/NNNNN.gbss`.
2. **Beside it, the right answer**, `<pool>/NNNNN.expect.json`:

```json
{
  "schema": "gbtestprompt/1",
  "species": 176,
  "level": 13,
  "held": [10, 45, 52, 0],
  "offered": 43,
  "correct_moves": [10, 45, 52, 43],
  "why": "the offered move goes into the empty slot; nothing is forgotten"
}
```

Only `correct_moves` is load-bearing; the rest is provenance, carried so a row of
the report can be read without opening the producer's notes. **An entry with no
file beside it is not scored on this outcome** — a pool that is half annotated
measures its annotated half rather than marking the rest wrong.

The rig loads each state, reads the lead's species to file it under a subject,
runs every arm from it, and then reads `wPartyMon1Moves` out of the state each
arm left behind. **That is the whole observation**: what a player chose is
legible from the party it produced, so nothing has to be inferred from the
recording and any player is scored identically.

**Slot order matters and the comparison respects it.** Gen 1 replaces a
forgotten move in place, so *which slot* the new move went into is part of the
answer; a set comparison would score a wrong choice as right whenever the
multiset happened to match. Pinned by
`a_prompt_is_answered_in_slot_order_not_as_a_set`.

**The state→action training corpus never touches this rig.** That is the
producer's business and the distillation's; what arrives here is a player, as a
`command` arm.

### What the level-up drill still owes, precisely

[`docs/gen1/level-up-drill.md`](../gen1/level-up-drill.md) already triggers every
level-up, answers every prompt and verifies the resulting party against the
cartridge, and it already writes the training corpus in this emulator's own
agent format (`agent.ndjson`, one record per button, observation verbatim in
`request`) plus an index (`events.csv`:
`species,level,arm_species,offered,kind,answer,reached,verified,rows`). Nothing
about either needs to change.

**The one thing missing is a pool this rig can start from.** A corpus records
what happened; a rig needs to *re-run* the same prompt with a different player,
which means a save state taken at the moment the prompt is on screen. So, per
verified event:

* `gb.save_state()` written as `<pool>/NNNNN.gbss` **at the prompt**, before the
  answer is given;
* `<pool>/NNNNN.expect.json` as above, whose `correct_moves` is the party the
  drill already verifies — `held`, `offered` and `level` are columns
  `events.csv` already has.

That is two `fs::write` calls at a point the drill is already standing on, and
it is the whole join between the two pieces of work. Everything else on this
page then works unchanged: the rig files each state under a subject by reading
its lead species, runs the teacher and the student from it, and compares the
four moves each left.

### What the report says, and why there are two numbers

```
  arm          prompts answered correctly
  teacher         3 of 6     50.0% [18.8-81.2]
  screen          3 of 6     50.0% [18.8-81.2]
  AGREEMENT teacher vs screen: 6 of 6 prompts left the same four moves (100.0%)
```

**Agreeing with the teacher and being right are different questions, and the rig
reports both.** Two players can each be wrong *and wrong in the same way* —
which is what a student that has faithfully learnt its teacher's mistake looks
like, is a success for the distillation and a failure for the criterion, and is
invisible in a correctness column alone. Pinned by
`agreeing_with_the_teacher_is_not_the_same_as_being_right`.

Per-attempt rows land in `<out>/<subject>-<arm>/attempts.ndjson`, carrying the
four moves each attempt left. A summary cannot be debugged: when a prompt scores
wrong the only useful question is *which* four moves the player chose.

### Reporting note: a species difference cannot come from the decision itself

[`docs/gen1/learning-moves.md`](../gen1/learning-moves.md) measured that of the
840 decisions there are **581 distinct (four held, offered) pairs, and the
number whose correct answer differs by species is zero**. So if a student does
better on one starter than another, **that difference cannot be coming from the
move-learning decision**, which is species-independent by construction. The
report must say so rather than leaving a reader to infer a species-specific
skill the underlying task does not contain. Look instead at how much of each
species' training data the distillation saw, and at the level curve that decides
how many prompts each species even asks.

---

## 6. The first result — a BATTLE comparison, not the move-learning one

> **Read this as what it is.** It measures the rival battle, because that is
> what the instrument was first pointed at while the move-learning prompts were
> still being built. It is **not** the move-learning result and must not be
> quoted as one; that measurement waits on a prompt pool
> ([§5a](#5a-measuring-move-learning--the-unit-is-a-prompt)). It is still
> useful: it is what proved the rig, and it produced two findings.

**Plan:** [`scenarios/testrig/starter-rival-battle.json`](../../scenarios/testrig/starter-rival-battle.json).
Pokémon Blue, link 05 (the rival battle in Oak's lab), three subjects, three
arms, 300 attempts per cell — 2,700 attempts. Entry pool: 700 states from link
04's certified exits, split 234/233/233 by starter. Phase spread 3001.

<!-- The measured table is in docs/agent/test-rig-results.md, generated by the
     run and quoted from there so this page and the artefact cannot drift. -->
See [**the measured result**](test-rig-results.md) for the table as the rig
printed it. The findings:

**The pass rate is blind to the fight underneath it, and that is the first thing
the instrument found.** Link 05's goal is that the rival battle *happened* —
`HandlePlayerBlackOut` special-cases this battle so losing it does not black the
player out, and `OaksLabRivalEndBattleScript` sets the flag whoever won. So the
expert passes every attempt with all three starters while *losing* that battle
at very different rates. A rig that reported only the goal would have said the
three starters were identical. This is why the battle outcome is reported and
gapped as a second outcome rather than as a footnote.

**The battle column independently reproduces a figure this project already had.**
Over 900 certified rival-battle exits the loss rates were Squirtle 14.7 %,
Charmander 23.0 %, Bulbasaur 54.0 % — Bulbasaur draws Charmander, which is
faster at every level and so lands the last hit. The rig, measured a different
way on a different pool, lands in the same place. Two independent measurements
agreeing is the check; if they had disagreed, that would have been the finding.

**Both screen-blind arms score zero on all three starters, and for the same
reason.** Neither can navigate: the endings histogram shows `stuck` and
`out-of-calls`, never a lost battle. The goal gap is therefore the maximum on
every subject and the battle gap has **no pairs at all** — the two arms never
both fought on the same seed, which the report says in those words rather than
printing a zero. That is a floor worth having: none of the expert's rate on this
link is available by accident.

---

## 7. What it cannot tell you

* **A `seen` number is not generalisation.** It is stamped on every report for
  that reason. A player trained on this fight and tested on this fight has been
  measured for distillation quality and nothing else.
* **The gap is paired on the seed**, so it is a within-fight difference. It says
  nothing about a different link, a different opponent or a different cartridge.
  Run those as their own subjects.
* **The battle columns are outcomes, not reasoning.** Whether a move was well
  chosen is not in them. `ChainDriver`'s own tally reports the expert's damage
  predictions, flee reasons and stat-move spending — but it is *driver-side*, so
  only the expert can produce it, and driver-side numbers are exactly what
  cannot be compared between players.
* **A cell is only as good as its entry pool.** Every attempt takes
  `seed % pool.len()`, so a pool smaller than the attempt count means entry
  states repeat — the phase offset still makes the attempts distinct, but the
  three independence axes are not all live. The pool size is printed per subject
  for that reason.
* **It does not judge a moveset.** The moveset is whatever the entry states
  carry; choosing one is a separate question with its own criterion.
* **Nothing here has been run against a trained network.** The `command` seam is
  exercised by a real subprocess arm in §6 and by
  `a_command_arm_carries_its_program_and_arguments`, but no model has ever been
  on the far end of it in this repository. Say that plainly rather than
  implying otherwise.

---

## 8. The arithmetic, and where its teeth are

All of it is in [`examples/support/abrig.rs`](../../examples/support/abrig.rs),
which is pure — no Game Boy, no driver, no scenario — so it is unit-tested with
no cartridge. `cargo test --release --features scenario --example pewter_chain`.

| test | what it pins |
| --- | --- |
| `a_perfect_score_still_has_a_lower_bound_below_one` | Wilson behaves at both boundaries |
| `wilson_matches_a_published_rate` | agrees with this project's own 2999/3000 |
| `mcnemar_agrees_with_arithmetic_that_can_be_done_on_paper` | the exact test against hand arithmetic |
| `a_gap_is_refused_when_the_arms_did_not_play_the_same_seeds` | pairing is required, never silently dropped |
| `a_pass_rate_can_be_blind_to_the_battle_under_it` | the §6 finding, as a test with teeth |
| `a_species_difference_is_separated_only_when_the_evidence_separates_it` | and refuses when it cannot |
| `a_plan_must_declare_its_split` | a plan with no split is an error, not a default |

The host-side battle observation the arms are scored by lives in
[`src/scenario/run.rs`](../../src/scenario/run.rs) (`Battles`,
`BattleSummary`), with its own tests — including
`a_blackout_sentinel_is_not_a_battle`, because `wIsInBattle` is a raw byte and
`$FF` means blacked out. That is the third time this repository has been bitten
by a catch-all arm over that byte.

---

## See also

* [`docs/agent/battle-policy.md`](battle-policy.md) — the expert's battle brain,
  and §4 "What a screen-only student would not have".
* [`docs/agent/pewter-chain.md`](pewter-chain.md) — the fourteen links, the
  certification protocol and where the entry states come from.
* [`docs/scenarios.md`](../scenarios.md) — the scenario language, the referee
  and the recording format, and
  [`docs/writing-a-scenario.md`](../writing-a-scenario.md) if you do not have a
  scenario to point this at yet: it builds one end to end and finishes by
  handing it here.
* [`docs/agent-reference.md`](../agent-reference.md) — the snapshot a driver
  receives and the plan it returns.
