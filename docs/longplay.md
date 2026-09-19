<!-- codemap:asof -->
# Long runs and episodes — an agent plays for hours, and reports in

*The runner: [`src/longplay/`](../src/longplay/mod.rs). The front end:
[`examples/longplay.rs`](../examples/longplay.rs). The agent substrate it
drives is [`plugins/agent/`](../plugins/agent/), documented in
[`docs/claude-plays.md`](claude-plays.md).*

Two things live here:

* a **long run** — give it a cartridge, a goal and a budget and walk away; it
  plays for hours at headless speed, keeps durable state so a crash can be
  resumed, tells you when something worth knowing happens, notices when the
  *game* has stopped getting anywhere, and writes an honest scoreboard;
* an **episode** — a named segment (a start save state, a success condition, a
  failure/timeout) that is reset and repeated, thousands of times if you like,
  so *"get out of Viridian Forest"* becomes a measurable number instead of an
  anecdote.

Both produce output that is useful **without watching it**: NDJSON you can
consume, a rendered summary nobody hand-wrote, and — for every attempt that
went wrong — a save state you can reload.

---

## 1. The one number the whole design turns on

Headless, no audio, uncapped, this emulator runs at **0.238 ms/frame — 70x real
time** ([`docs/mcp.md` §6](mcp.md)), about **4,200 frames a second**. A model
round trip is *seconds*. Consult the model every few frames and the run happens
at the speed of the network; the 70x is thrown away.

So the metric that decides whether any of this works is **frames per model
call** — how much game one decision bought. It is:

* the first line of the run report,
* the first field in `report.json` and on every line of `episodes.ndjson`,
* in every heartbeat, so you can watch it move.

Two figures fall out of it and both matter more than raw speed:

| figure | what it says |
|---|---|
| **effective speed-up** (game seconds / wall seconds) | what the *whole system* achieved. The emulator alone is 70x; whatever this reads is what the agent left of it. |
| **`executing_frames_per_call`** | the honest headline: frames the *plan* was driving, think time removed. |
| **`think_share`** | how much of the run was the world moving on while the model decided. |
| **spend per hour of game time** | what an overnight run actually costs. |

Measured here on Pokémon Blue with a canned (free) driver, three runs of
75,705 frames each: **252 frames per model call, 5,410–5,670 emulated fps,
91–95x real time**. With a driver that answers instantly `think_share` is **0**
and `executing_frames_per_call` is 231 — i.e. the headline is very nearly all
real play, which is the control the *next* paragraph needs.

(That is a different workload from the 70x in [`docs/mcp.md`](mcp.md) §6 — this
one is a fresh boot with the agent plugin attached and sampling, theirs is a
save in the overworld through the MCP protocol. Neither contradicts the other;
both are stated with their method.)

**And a warning that came out of a real run: on an uncapped headless run this
number is mostly the model's *think time*, not play.** At ~3,500 frames a
second a 26-second round trip is ninety thousand frames of game running on with
nobody at the controller. So the report also carries
`executing_frames_per_call` (the plan's own frames) and `think_share`, and
`--pace thinking` bounds it. **Never quote `frames_per_call` from an uncapped
run without `think_share` beside it.** §9 has the measurement, and the
counterfactual where the *worse* configuration reports the bigger number.

---

## 2. Quick start

```bash
# free and deterministic — a canned script, no model, no cost
cargo run --release --features longplay --example longplay -- \
    --rom ~/roms/blue.gb --dir target/lp/demo \
    --driver scripted --script script.ndjson --loop-script --frames 40000

# a real model, bounded, telling a phone when something happens
cargo run --release --features longplay --example longplay -- \
    --rom ~/roms/blue.gb --dir ~/runs/blue-night \
    --objective "Leave Pallet Town and reach Viridian City" \
    --until "map=0x01" \
    --hours 8 --max-calls 400 --max-usd 2 \
    --ntfy https://ntfy.sh/my-topic \
    --driver command --command tools/claude_plays/claude_cli_adapter.py

# …died at hour four? Same command plus --resume. That is a REWIND, and the
# report says so.
```

`--dry-run` prints the plan and the budget and starts nothing. Use it before a
long run: **nothing should ever spend money without having printed what it may
spend first.**

`--pace max|thinking|realtime`: `max` (the default) runs flat out; **`thinking`
runs flat out while a plan executes and paces to real time while the model is
deciding**, which is almost always what you want for an episode — see §9;
`realtime` paces everything.

### What a run leaves behind

```
<run dir>/
  events.ndjson     append-only: start, milestones, watch changes, captures,
                    heartbeats, the final scoreboard
  steps.ndjson      ONE LINE PER DECISION: observation → intent → actions →
                    outcome.  This is the dataset.
  agent.ndjson      the agent plugin's own log: the exact request and reply,
                    joined to steps.ndjson on `decision`
  report.md         the human summary, rendered from the same data
  report.json       the same, machine-readable, headline field first
  checkpoint.json   what a restart needs that is not in the save state
  checkpoint.gbss   the save state a restart resumes from
  captures/         a save state per thing that went wrong
  <rom>.gb          the staged cartridge (the run's own copy — never yours)
```

---

## 3. Progress reports: what a person who walked away actually wants

Not a frame counter. The runner watches a handful of quantities that only move
when something real happened, and turns changes into **milestones**
([`progress::MilestoneKind`](../src/longplay/progress.rs)):

| milestone | importance | pushed to a phone by default? |
|---|---|---|
| a badge won | major | yes |
| a town or route reached for the first time | major | yes |
| a first Pokémon | major | yes |
| the run resumed from a checkpoint | major | yes — a rewind is never silent |
| an unnamed interior entered | minor | no |
| a level up, a new species, the party growing | minor | no |
| the party wiped | trouble | yes |
| **a wall** — no progress for a long time | trouble | yes |
| the agent halting (budget, breaker) | trouble | yes |

A **heartbeat** every few minutes says what has happened since the last one,
and says so briefly when the answer is nothing:

```
… nothing new for 5m 0s. In Viridian Forest, 0 badges. 148 calls,
  312 frames/call, $1.21. STUCK — 9 tiles, no progress.
```

Push is [ntfy](https://ntfy.sh) (`--ntfy https://ntfy.sh/<topic>`), because
that is the mechanism already in use here rather than a new one. Transport is
the system `curl` (so no new crate — the bar
[`src/portal/http.rs`](../src/portal/http.rs) already meets), the token travels
on **stdin** and never argv, and the queue is bounded and **drops** rather than
ever blocking a frame — the rule the audio hand-off learnt the hard way
([`docs/audio.md`](audio.md) §4c).

---

## 4. The wall detector, and why the agent's own is not enough

The agent plugin already has one ([`governor::Stuck`](../plugins/agent/src/governor.rs)):
it fingerprints the world across consecutive **decisions** and escalates when
nothing changes. That is the right question at the scale of one call.

It cannot see the failure an unattended run actually dies of. An agent that
walks ten steps north, ten south, and ten north again produces a *different*
fingerprint every decision — it is not repeating itself at all — and at 70x it
can do that for two hours of wall clock, which is six game days.

So [`progress::Wall`](../src/longplay/progress.rs) asks a different question at
a different scale: **has the game got anywhere?** A single score
(badges ≫ species owned > maps visited > party levels > items) that **never
falls when the player merely moves**, measured over a window of *game* time.
Flat for long enough and it says so — on a **doubling** ladder, so a run that is
genuinely dead produces a handful of escalating reports rather than one a
minute for six hours.

And it names *which* kind of stuck, because the two need different fixes:

* **pinned** — a handful of tiles: walking into the same wall, or reopening the
  same menu.
* **wandering** — plenty of ground covered, nothing achieved: the maze failure
  (Viridian Forest, Mt. Moon, Rock Tunnel).

The separator is distinct `(map, x, y)` triples seen in the window, so a
two-tile shuffle repeated a thousand times still reads as pinned.

**A wall also writes a save state**, so the exact machine that got stuck can be
reloaded.

---

## 5. Episodes — the primitive that makes an agent measurable

A whole playthrough is one sample of a very long, very noisy process. An
**episode** is the opposite: the same byte-identical starting point, again and
again.

```json
{
  "schema": "gbepisode/1",
  "name": "out-of-viridian-forest",
  "objective": "Leave Viridian Forest by the north exit",
  "start_state": "states/forest-south.gbss",
  "success": ["map=0x33"],
  "failure": ["party_hp=0"],
  "timeout_frames": 60000,
  "max_calls": 40,
  "watch": ["in_battle", "lead_hp"]
}
```

```bash
cargo run --release --features longplay --example longplay -- \
    --rom ~/roms/blue.gb --dir target/lp/forest \
    --episode episodes/viridian-forest.json --repeat 50 \
    --driver command --command tools/claude_plays/claude_api_adapter.py
```

The reset is a save-state load, and
[`testharness/savestate.rs`](../testharness/savestate.rs) proves a load gives
**byte-identical frames from then on**. So every attempt genuinely starts from
the same machine and the only thing that varies is the agent. That is what
makes a success *rate* mean anything.

**Declarative on purpose** — a small JSON file, not Rust. A person defines a
segment without touching the emulator, and an episode file plus its results is
a reproducible artefact somebody else can re-run.

> **A success rate needs a floor as well as a ceiling.** "The agent reached
> Viridian City" means very little on its own; what it is worth depends on what
> *undirected* play does on the same corridor, and the answer there is often
> less flattering than it looks. [The swarm view](swarm-view.md) is that
> measurement — twenty-four decorrelated random walkers on the same save state,
> published as a baseline — and it found that a random walk drains south through
> Route 1's one-way ledges and **never reaches Viridian at all**. Worth having
> before quoting any episode's rate: the same rig can run the null model.

### The condition grammar

`<thing><op><number>`, and nothing else. `op` is `=`, `!=`, `>=`, `<=`, `>`,
`<`; numbers may be decimal or `0x`/`$` hex. `thing` is one of `map`, `x`, `y`,
`badges`, `party`, `party_hp`, `owned`, `money`, `items`, `play_seconds`, or
`watch:<name>`.

* **`success` is an AND** — every condition must hold at the same sample.
  ("On this map *and* out of battle.")
* **`failure` is an OR** — any one ends the attempt. Failures are alternatives
  by nature, and **failure outranks success** when both hold, because a wipe on
  the target map is not a win.

An episode that could never succeed — no success condition, a zero timeout, a
condition on a watch nobody declared — is **refused before it spends
anything**. A thousand attempts against an impossible condition is a thousand
timeouts and a real bill.

### What an episode series reports

`episode.md` / `episode.json`, plus one `episodes.ndjson` line per attempt:

* **frames per model call** (first, as everywhere),
* success rate, and the **median frames of the successful attempts** — a median
  taken over successes and timeouts together measures nothing, because a
  timeout's frame count is the timeout, not the difficulty,
* how attempts ended, by kind,
* **where the time went** — frames per map, across every attempt,
* the reproducible failures, each with the path to its save state.

`gameboy-battleai-g7` owns the battle domain; a battle is the ideal short
episode and runs on this primitive unchanged.

---

## 6. Consumable output

### `steps.ndjson` — one line per decision, and a dataset

```json
{"schema":"gblongplay-step/1","decision":12,"frame":45231,"frames":361,
 "wall_seconds":118,
 "observation":{"in_game":true,"map":12,"map_name":"Route 1","x":9,"y":4,
                "badges":0,"money":3000,"owned":1,"seen":3,"items":4,
                "play_seconds":812,"party_hp":22,"party":[{"species":1,"level":7}]},
 "intent":{"goal":"Reach Viridian City","say":"Heading north through the grass."},
 "actions":[{"action":"walk up x8","result":"moved 8 steps","changed":true}],
 "outcome":{"moved":true,"score_before":140,"score_after":144,"progressed":true},
 "state_after":{…}}
```

Four things and no more: the **observation** the agent was given, the **intent**
it stated in its own words, the **actions** that ran, and the **outcome** they
produced. Every field is already measured for the report, so the dataset costs
a run nothing. Join it to `agent.ndjson` on `decision` for the exact request and
reply, including the model's own token counts.

Note `observation.in_game`: before a save is loaded, every other field is
uninitialised WRAM, so **they are omitted rather than written as zeros**. A
learner cannot tell a real zero from a not-yet-a-game zero, and a dataset that
makes it guess is worse than one that says nothing.

### Where the time went

Frames per map, in the run report and per episode:

```
| area                     | frames | share |
|--------------------------|-------:|------:|
| Red's house 2F (bedroom) |  39102 |   98% |
| Pallet Town              |    438 |    1% |
```

That is the number that says where the agent is *actually* stuck. It is
attributed at sample time (~10 Hz) from the map id, so it costs nothing.

### Memory watches — recording only, never perception

A watch samples a named address and puts the number in the log and the report.
**It is never shown to the agent**, and that is not tidiness:

> An agent told "the opponent has 4 HP left" is not solving the problem the
> screen poses; it is reading the answer out of RAM. If watches leaked into the
> snapshot, every episode success rate this harness ever produced would be
> uninterpretable — and the leak would be invisible in the results.

The two paths are structurally separate: the agent's perception is its own
observer *inside the plugin*; a watch is read *in the host* and goes only to
`events.ndjson`, `episodes.ndjson` and the report.
[`testharness/longplay.rs`](../testharness/longplay.rs) check [13] asserts it by
declaring a watch called `secret_oracle` and grepping **every request the driver
was actually handed** for the name.

Built-in names resolve to addresses
[`plugins/trainer`](../plugins/trainer/src/gen1.rs) already derived and
verified: `in_battle`, `battle_type`, `cur_opponent`, `enemy_species`,
`enemy_hp`, `enemy_level`, `trainer_class`, `lead_hp`, `lead_max_hp`,
`lead_level`, `lead_status`. A raw address works too
(`{"name":"flag","addr":"0xD747","len":1}`) — but a *bare* `"D747"` is refused,
because decimal-or-hex is a guess and an address read wrong reports a plausible
number about the wrong byte for ever.

### Reproducible failures

**A failing episode you can reload is worth ten you cannot.** Every non-success
attempt, every fault and every declared wall writes the whole machine to
`captures/`, named for what went wrong and when. Hand it to a person, a
debugger, or the next version of the agent.

---

## 7. Guard rails, and what a run declares

Numbers are only worth having if you know what helped. Every report — long run
and episode alike — carries an `assistance` block, printed even when every
field is zero, because **a claim nobody can check is not evidence**:

| declared | how it is kept true |
|---|---|
| `memory_writes` | the agent declares `OBSERVE \| READ_MEMORY \| CONTROL_INPUT` and **not** `WRITE_MEMORY`; `Machine::poke` returns `false` for it. The harness checks the *observed* `has_written` flag on a real run, not the declaration. |
| `memory_informed_decisions` | watches are host-side; see above. |
| `state_resets` | one per episode attempt, by design; and one per `--resume`, which is a **rewind** and is also a `Major` milestone. |
| `seed_controlled` | the RNG is left alone. Gen 1's `Random_` mixes `rDIV` in per call, so a seed cannot be pinned anyway — see [`docs/shiny-hunt.md`](shiny-hunt.md). |

### Determinism

Same start state, same agent, same result. This emulator can genuinely offer
that and few environments can, so it is **asserted** rather than assumed:
`testharness/longplay.rs` check [12] runs the same episode series twice and
requires the outcome, frame count, call count and decision count of every
attempt to match — and requires attempts *within* one series to match too,
which is what says each reset is a true reset.

The caveat, stated rather than buried: this holds for a **deterministic
driver**. A real model is not one, and a run through it is reproducible only in
the sense that the *environment* is.

---

## 8. What the field has already learnt, and what this harness does about it

An LLM playing a full RPG is hard, and it is well-trodden ground. The findings
below are other people's, cited; each is followed by what it implies here.

**The harness dominates the model.** The PokéAgent Challenge reports that
*"without a harness, raw frontier VLMs achieve effectively 0% task
completion"* on its speedrunning track ([arXiv 2603.15563](https://arxiv.org/abs/2603.15563)).
lmgame-Bench measured 40% of un-harnessed runs failing to beat random play
against 86.7% beating it with a harness ([arXiv 2505.15146](https://arxiv.org/abs/2505.15146)).
Joel Z, who built the Gemini Plays Pokémon harness, said plainly that *"the
playing field wasn't level. Gemini had the benefit of my custom harness with all
these extra tools and information."*
→ **So a result is meaningless without knowing what the harness gave the
agent.** That is exactly why the `assistance` block exists and why watches are
kept out of perception.

**Persistent memory is the most dangerous component, not just a helpful one.**
Claude 3.7 wrote a strategy document on a false premise during its Pokémon run
and then deliberately fainted its whole team repeatedly to act on it.
→ **An episode carries no memory between attempts, deliberately.** A reset is a
clean-room measurement of a *policy*, not of a policy plus whatever it happened
to write down. A harness that wants cross-attempt memory should add it
deliberately and declare it.

**The knowing–doing gap is a named, reproducible failure.** VideoGameBench
found models that correctly *state* where the exit is and then *"repeatedly
press 'down' irrespective of where the controlled character is in the room"*.
→ This is what the wall detector's **pinned** verdict catches, and why the
report distinguishes it from wandering: the two need different fixes.

**Published throughput, for scale.** Claude 3.7 took roughly 35,000 actions and
~140 hours for three badges (~250 actions/hour); Gemini's first run took ~813
hours. Those are *wall-clock* hours at roughly real-time play.
→ The interesting comparison is not who won but **how much game each decision
buys**, which is why that is the headline here rather than badges/hour.

*(Sources were gathered for this page; a claim is here only if it was traced to
a primary source. Several widely-repeated figures — per-obstacle stall
durations in Twitch Plays Pokémon, and an input-lag number often quoted for it —
could not be verified and are deliberately absent.)*

---

## 9. A real run — and the finding that came out of it

All of this is Pokémon Blue, the maintainer's own cartridge, on this machine.

### The control: a canned driver, no model, no cost

A two-line script that only presses A/START, so it measures the **harness**.
Three consecutive runs, each ending on the agent's own 300-call budget:

```
75,705 frames in 13.36–13.99 s  ->  5,411–5,668 emulated fps, 90.6–94.9x real time
252.35 frames per model call · executing_frames_per_call 231.09 · think_share 0.00
```

(Nine runs, spread over three successive revisions of the emulator.)

Two things to note. The frames-per-call and executing-frames-per-call figures
are **identical to two decimal places across all nine runs, on all three
revisions**, while the wall clock moved by 5%. That is the determinism §7
claims, stated the useful way round: the wall clock belongs to the host, and
the machine does not. And
`think_share` is 0: with a driver that answers instantly the headline *is* real
play, and the two numbers agree — which is the control that makes the finding
below readable.

Rates here divide by **milliseconds**, not whole seconds. That is not a detail:
an earlier version of this page quoted "4,000 fps" from a three-second run, and
whole-second wall time makes 20,000 frames anywhere between 5,000 and 6,667 fps
depending on which side of the second it landed. `Scoreboard::wall_ms` is what
every rate divides by, pinned by
`a_rate_is_measured_in_milliseconds_not_whole_seconds`.

It reached Pallet Town, went into the bedroom and got pinned, which the wall
detector reported after five minutes of game time — *"going round in 2 tiles"* —
with a save state written to `captures/`. 98% of the run was one room. That is
the report a person who walked away wants: **not "it ran 40,000 frames", but
"it has been stuck in one room for five minutes and here is the machine".**

### A real model, 45 calls, ~$0.95

`claude` CLI adapter (haiku), objective *"Leave Red's house, walk to Oak's lab,
get a starter"*, `--until party>=1`, 20 minutes of wall clock:

```
92654 frames per model call (25m 51s of game per decision) · 45 calls ·
19h 23m of game time in 19m 42s of wall clock (59.1x)
```

It got out of the bedroom, downstairs, into Pallet Town and into Oak's Lab —
four areas — and then spent the rest of its budget stuck. It **did not** get a
starter. 12 wall reports, worst stall 8 hours of game time, 60% of the run in
Oak's Lab, three save states captured.

### The finding: at 70x, `frames_per_call` is mostly think time

**92,654 frames per call is not 92,654 frames of purposeful play.** 45 calls in
19m42s is ~26 s a call; at 3,527 emulated fps that is ~92,000 frames — i.e.
essentially *all* of it was the world running on while the model thought. The
emulator never blocks on the agent, which is right and is the property the
design rests on; but headless and uncapped it means each plan comes back aimed
at a world that moved a game-day ago.

So the scoreboard reports **`executing_frames_per_call`** and **`think_share`**
beside the headline, and `--pace thinking` bounds the effect (real time while
the model decides, flat out while a plan runs). **Never quote
`frames_per_call` from an uncapped headless run without `think_share`.**

### The counterfactual, measured

The long run's third capture — *"stuck in Oak's Lab"* — was turned straight
into an episode (`success: ["map=0x00"]`, 12,000-frame timeout, ≤5 calls),
which is exactly the workflow the captures exist for. The same agent, the same
adapter, the same start state, three attempts each way:

| | success rate | frames/call | median frames to succeed |
|---|---:|---:|---:|
| `--pace thinking` | **3/3 (100%)** | **810** | 804 |
| unpaced | 0/3 | 12,000 | — |

Every unpaced attempt timed out at 12,000 frames **having made exactly one
call, which had not come back**. Note which column is bigger: *the failing
configuration reports the larger frames-per-call*. That is the metric's
inflation failure in one table, and it is why the harness now prints a warning
when an episode's timeout is dwarfed by a call's think time.

And note the substantive result: the agent that spent eight game-hours pinned
in Oak's Lab during the long run got out of it **in a single decision, three
times out of three**, once the segment was isolated and the world was not
running away underneath it. That is the argument for episodes.

### Honest limits

* **The model is the bottleneck, not the emulator, and by a wide margin.** With
  the `claude` CLI adapter a call is 8–26 s. The 70x is only reachable with a
  driver that answers in milliseconds; `tools/claude_plays/claude_api_adapter.py`
  is much faster than the CLI and is the one to use for anything long.
* **The agent plugin reads Pokémon Red/Blue only** (title *and* header
  checksum). Anything else is refused with an explanation rather than played
  blindly.
* **`play_seconds` is the cartridge's own clock**, so it does not advance while
  the game is at a menu the runner is waiting on. Frames are the other clock and
  both are reported.
* **Nothing here has been run for eight hours.** The longest verified session is
  twenty minutes. The checkpoint/resume path *is* tested — by a second process
  reading a first process's checkpoint — but "it survives an overnight run" is a
  prediction, not a measurement.
* **The episode numbers above are three attempts, not thirty.** They demonstrate
  the primitive and the pacing effect; they are not a success rate anybody
  should quote for this agent on this segment.

## 10. Testing

```bash
cargo test --release --features longplay --lib longplay   # 51 unit tests
cargo test --release --features longplay --test longplay  # 55-check harness
```

The harness needs **no ROM**. Its fixture is a 32 KiB cartridge whose entire
program is `jr -2`; the header says `POKEMON BLUE` with the checksum the
trainer crate matches on, purely so the Gen 1 reader engages, and it contains no
game data whatsoever. Because the program writes nothing, the harness *is* the
cartridge: it pokes the bytes a real Gen 1 game would write, on a schedule of
its choosing. What is under test is the observation and reporting layer, and
`testharness/agent.rs` already drives the executor against a moving player.

The checks worth knowing about:

| | |
|---|---|
| [9] | **a driver that never answers does not slow the emulator down** — the property the whole design rests on |
| [12] | the same segment and the same agent give the same result, twice |
| [13] | a declared watch is recorded and **never reaches the agent** |
| [27] | on a real run the agent pressed buttons and wrote no memory (observed, not declared) |
| [30]–[35] | a checkpoint written by one run is read by the next, milestones are not replayed, and the rewind is declared |
| [41] | every failed attempt leaves a save state you can reload |

---

## 11. Where the code is

| file | what |
|---|---|
| [`src/longplay/mod.rs`](../src/longplay/mod.rs) | `Runner` — the loop, sampling, areas, checkpoints, captures, heartbeats |
| [`src/longplay/progress.rs`](../src/longplay/progress.rs) | what progress *is*: the score, the milestones, the wall detector |
| [`src/longplay/episode.rs`](../src/longplay/episode.rs) | the episode file, the condition grammar, the per-attempt and summary formats |
| [`src/longplay/series.rs`](../src/longplay/series.rs) | reset-and-repeat |
| [`src/longplay/watch.rs`](../src/longplay/watch.rs) | memory watches, and the separation guarantee |
| [`src/longplay/journal.rs`](../src/longplay/journal.rs) | the append-only log and the checkpoint |
| [`src/longplay/report.rs`](../src/longplay/report.rs) | the scoreboard |
| [`src/longplay/notify.rs`](../src/longplay/notify.rs) | the ntfy push |
| [`examples/longplay.rs`](../examples/longplay.rs) | the front end |
| [`testharness/longplay.rs`](../testharness/longplay.rs) | the harness |
