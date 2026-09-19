# Scenarios — a goal, rules the host enforces, and a run you can replay

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`e0f10b0`](https://github.com/Alchemy86/TerminalGB/tree/e0f10b0f977e73b2aecd4284015c69dccd5327f0) · regenerate with `make docs-links`, verify with `make docs-check`.

A **scenario** is a small JSON file: where to start, what counts as done, and
the rules the run is judged under. An agent plays it against the knowledge base
this repository already has rather than against luck, and what comes out is a
**recording** — a file somebody else can take, replay exactly, and check.

Three things stacked, and only the first two are new:

```text
  scenario.json ──▶ Referee    rules the HOST decides, not the model
                ──▶ Knowledge  the atlas, plus what earlier runs walked
                ──▶ Episode    the goal machinery already in src/longplay
                              │
                              ▼
                          the run ──▶ recording.json ──▶ replay, 0 model calls
```

- **Writing one**: [`docs/writing-a-scenario.md`](writing-a-scenario.md) builds
  a small real scenario end to end — pick a goal, find the byte that proves it,
  write the watch, check it, run it, replay it — and shows the ways it goes
  wrong. Start there if you have not written one before; this page is the
  reference behind it.
- **Code**: [`src/scenario/`](../src/scenario/mod.rs) — `rules.rs`,
  `knowledge.rs`, `record.rs`, `run.rs`.
- **Front end**: [`examples/scenario.rs`](../examples/scenario.rs).
- **Gate**: [`testharness/scenario.rs`](../testharness/scenario.rs), 15 checks,
  no commercial ROM, in a plain `cargo test`.
- **Behind the `scenario` cargo feature** (`longplay` plus this module), in
  `default`, out of `--no-default-features`, and it adds **no third-party
  crate**.

---

## 1. Why a rule has to be enforced rather than requested

A rule written into a prompt is a request. The model may honour it, ignore it,
or honour it for two hours and then not. That is not a foundation for any claim
about a run — and the claim is the whole product.

Consider [`examples/shiny_hunt.rs`](../examples/shiny_hunt.rs)'s `make_state`,
which is the thing this was built to replace. It reaches Route 1 in seconds by
writing `EVENT_FOLLOWED_OAK_INTO_LAB` into `$D747` so Oak stops blocking the
exit, writing a Pokémon into `wPartyMons` so the first wild battle does not
hang, and steering to a hardcoded tile. Its own doc comment calls it *"a
convenience, not the feature"*, and it is right: the save state it produces
satisfies every observable you could name and is not a playthrough.

Those two writes are worth *watching*: `shiny_hunt --show` draws the same walk
at real time with a caption under it, and captions the writes as they happen
precisely because they are the interesting part and are otherwise invisible.
That is the same fact from the other end — it makes the shortcut legible to a
person, and what follows makes it impossible to take without saying so.

From the outside those two files are indistinguishable. So the rule is decided
by the **host**, from evidence the host owns:

| rule | what decides it |
|---|---|
| `no_memory_writes` | [`Gameboy::host_writes`](../src/gameboy.rs) and every plugin's `has_written` |
| `no_state_loads` | the runner counts its own rewinds |
| `require <cond>` | a condition on observed state, tested **at the goal** |
| `forbid <cond>` | a condition on observed state, tested every sample |
| `max_calls` / `max_frames` / `max_wall_seconds` / `max_usd` | the governor's own counters |
| `battle flee` / `fight` / `decide` | whether battles were won or run from |

Nothing in that right-hand column can be talked out of.

**A violation is a forfeit, not a bad score.** The referee runs *before* the
goal is judged, so a run that broke a rule did not reach the goal — it forfeited
— and the report names the rule and the frame:

```text
rule "no_memory_writes" broken at frame 30: 1 byte(s) were written into the
emulated machine by something other than the game itself
```

### The rules are told to the agent, in words

A rule the host enforces but never states is a trap: the agent fails it without
ever having been told it existed. Every rule that constrains *behaviour* (as
opposed to merely bounding cost) is put at the **head** of the snapshot's
`notes`, before the situational ones, and before the agent's first decision —
not on its first map change, because by then it has already decided once.

```text
BATTLE POLICY: flee. Every battle is to be run from — choose RUN from the
battle menu. Do not fight, do not throw a ball.
RULE: reaching the goal only counts if `party>=1` is also true.
```

### `require` is judged at the finish, not throughout

"Must legitimately obtain a Pokémon" is false for the first ten minutes of every
legitimate playthrough. So `require party>=1` is not a condition on every frame;
it is a condition on the *arrival*, and a run that gets there without it forfeits
rather than passing:

```text
rule "require party>=1" broken at frame 162: the goal was reached but
`party>=1` is not true, so it does not count
```

### Allowances are declared

Some things are legitimate and change what a number means: the reset an episode
is made of, a rewind after a crash, a start state somebody built by hand, a
battle recommendation computed from the opponent's true stats. All of them are
printed in every report, and `clean` means "no writes, no rewinds, no privileged
advice". An undeclared allowance makes every number worthless — the same
principle the agent's memory-write ban already encodes.

---

## 2. What a scenario looks like

```json
{
  "schema": "gbscenario/1",
  "name": "leave-the-pokemon-center",
  "objective": "Walk out of the Viridian Pokémon Center. The two exit mats are at the bottom of the room; a door is not opened with A, so step onto a mat and then one more step down to go through it.",
  "goal": ["map!=0x29"],
  "failure": ["party_hp=0"],
  "rules": [
    "no_memory_writes",
    "battle flee",
    "require party>=1",
    "max_calls 12",
    "max_wall_seconds 600"
  ],
  "knowledge": ["atlas", "guidance"],
  "guidance": "guidance/viridian.json",
  "timeout_frames": 12000
}
```

`goal` is an **AND** — every condition at the same sample, which is what a
person means by "done". `failure` is an **OR** — any one of them, which is what
a person means by "gone wrong". Same grammar and same reasoning as
[`src/longplay/episode.rs`](../src/longplay/episode.rs), which supplies the
reset-and-repeat machinery underneath; a scenario converts to an `Episode`
rather than reimplementing one.

The condition language is deliberately tiny: `map`, `x`, `y`, `badges`, `party`,
`party_hp`, `owned`, `money`, `items`, `play_seconds` and `watch:<name>`, with
`= != >= <= > <`. A condition nobody can read at a glance is a condition that
gets misused.

### Everything with a cost is on screen before anything is spent

```console
$ cargo run --release --example scenario -- check scenarios/leave-the-pokemon-center.json --prose
scenario "leave-the-pokemon-center": Walk out of the Viridian Pokémon Center. …
goal: map!=0x29
fails on: party_hp=0
rules: no_memory_writes, battle flee, require party>=1, max_calls 12, max_wall_seconds 600
cost ceiling: 1 attempt(s) × 12 calls = 12 model calls worst case, 12000 emulated frames worst case
knowledge: atlas, guidance
battles: flee
evidence: NONE STATED — nothing in this file says which address defines success, …
  knowledge available: atlas (2891 addresses)
  knowledge available: guidance (new, will be written to scenarios/guidance/viridian.json)

the agent will be told:
  - BATTLE POLICY: flee. …
  - RULE: reaching the goal only counts if `party>=1` is also true.

OK
```

(`--prose` is the paragraph form above; without it `check` prints the same
thing as TOON, which is what an agent driving the tool by shell reads. Every
subcommand's output is TOON by default and every error is on stdout — see the
module docs of [`examples/scenario.rs`](../examples/scenario.rs).)

`check` loads no cartridge and costs nothing. A scenario that could never be
judged — no goal, a condition on a watch nobody declared, advice it could never
act on — is **refused here**, before a thousand attempts turn into a thousand
timeouts and a real bill.

---

## 3. Planning against what we know

> *"leveraging the knowledge and rules so that it can … ref these tools,
> determine location and requirements, knowledge of locations and stuff, and
> BUILD its own rules and AI for guidance"*

Two sources, and the line between them matters.

### The atlas — static, cited, free

[AtlasGB](https://github.com/Alchemy86/AtlasGB)'s pinned snapshot lives at
[`third_party/atlasgb/pokemon-rb/atlas.tsv`](../third_party/atlasgb/README.md):
2,891 evidenced Gen 1 addresses, each carrying its evidence tier. It is what a
*requirement* is expressed in terms of, and it answers questions directly:

```console
$ cargo run --release --example scenario -- knowledge --search wEventFlags
atlas:
  addresses: 2891
  from: third_party/atlasgb/pokemon-rb/atlas.tsv
matches[1]{symbol,addr,bytes,evidence,what it is}:
  wEventFlags,$D747,320,"rom,live","**The story, in 320 bytes.** A flag array of
  2,560 single-bit events: every gift taken, every trainer beaten, every door
  unlocked. …"

$ cargo run --release --example scenario -- knowledge --at 0xD35E
query:
  address: $D35E
covers[1]{symbol,addr,bytes,evidence,what it is}:
  wCurMap,$D35E,1,"rom,live,inv","The map you are standing on. …"
```

It is **loaded from disk, never embedded** — 237 KB of TSV in the published
crate and in the embedding build would break a contract `make embed` measures.
`$GB_ATLAS` overrides the path. An absent atlas is a source that is *reported
missing*, not a failure to start: a run that planned without the reference it
asked for is a different run and must say so.

### Guidance — learned, and that is the point

A full Gen 1 route table exists in `pret/pokered` and could be copied out. Two
reasons not to. The licence boundary this repository keeps says facts and
formats yes, bulk game data no — a warp table is closer to data than to a fact.
And the ask was for the agent to build its *own* guidance: a graph a run
discovered by walking through the doors is evidence the run really went there,
which a transcribed table is not.

So a run records every door it walks through, merges what it found into the
scenario's guidance file, and the next run is handed it:

```console
$ cargo run --release --example scenario -- guidance scenarios/guidance/viridian.json
guidance:
  file: scenarios/guidance/viridian.json
  rom: POKEMON BLUE
links[1]{from,from_id,door_x,door_y,to,to_id,uses}:
  Viridian Pokémon Center,0x29,4,7,Pallet Town,0x00,2
```

`uses` is evidence: a door two runs found is better evidenced than one seen
once. When a scenario names a single destination map, the next run's agent is
handed the route in words, with the door:

```text
A route earlier runs actually walked: Red's house 2F (bedroom) → Red's house 1F.
The next step is Red's house 1F.
The door to Red's house 1F was at (8, 8) — walk onto it and then one more step
in the direction it faces.
```

…and when nothing has walked there, it is told *that*, rather than being handed
a guess:

```text
No earlier run has walked from here to the goal, so there is no route to follow
— explore.
```

### The honest boundary

Reading a *reference* to plan with is legitimate — a player with a wiki open is
still playing. Reading *live memory to decide* is the cheat the agent is barred
from. The two live in different places on purpose: `knowledge.rs` holds only
what is true of the cartridge, never of this playthrough.

The one source that crosses the line is `battle_advice`, which would read the
opponent's true stats out of the live machine. It is **off unless a scenario
asks for it**, asking for it puts a `PRIVILEGED` line in the briefing *before*
the run, and every decision taken under it is counted in the report's assistance
block. A scenario that asks for it and also forbids fighting is refused: the
advice could never be acted on.

---

## 4. A run is an artefact

A decision is expensive to produce and cheap to *repeat*. So a recording is the
driver's **replies**, in order, plus enough identity to prove it is being
replayed into the same world:

```text
cartridge title + global checksum   which game
start-state hash                    which moment in it
scenario hash                       which rules were in force
the replies, in order               what the agent decided
final-state hash                    what it produced
```

Nothing else, because everything else is *derivable*. The frames are not
recorded; the observations are not recorded. That is the difference between
recording an emulator and recording anything else, and it is why 400 decisions
over 21,887 frames is a few hundred kilobytes.

Replay pushes those replies back through
[`ScriptedDriver`](../plugins/agent/src/driver.rs), which goes through the same
parse-clamp-execute path a live model's reply does — that is what it exists for
— so a replay is not an approximation of the run, it *is* the run. And with
nothing to wait for it goes at the emulator's own speed rather than the
network's.

### Refused into the wrong world, by name

Each of the three identities means something different to whoever is holding the
file, so each is checked separately:

- wrong cartridge → *"this recording is of `POKEMON BLUE` and the cartridge is
  `POKEMON RED`"*
- right cartridge, wrong build → *"the global checksum does not match"*
- right game, wrong moment → *"the start state is not the one this was recorded
  from"*

A **scenario edited since** the recording was made is different again: the
decisions are still valid, so the replay runs, but it is a different
measurement and the report says so rather than refusing.

### Faithful means everything agrees

An identical final state reached in a different number of frames is not a
replay. `faithful()` requires the final state hash, the frame count, the ending
*and* the decision count to match.

---

## 5. Measured, on the maintainer's own Pokémon Blue

A start state was made by booting his cartridge and choosing CONTINUE — the
game's own save, entered the game's own way. The scenario is
[`scenarios/leave-the-pokemon-center.json`](../scenarios/leave-the-pokemon-center.json)
above.

**The run** — a three-step plan, only button presses:

```text
GOAL REACHED after 378 frames and 2 decisions (189 frames per decision)
59 ms — 6407 frames/second, 107.3x real time
ended on map Pallet Town (0x00) at (4, 7) — 2 in the party, 0 badge(s)
state resets: 1
rewinds inside an attempt: 0
host memory writes: 0
knowledge consulted: atlas (2891 addresses), guidance (1 links, 0 notes)
learned 1 door(s) between maps
```

**The replay**, from the recording alone:

```text
replaying 2 decisions recorded on "POKEMON BLUE" — 0 model calls
final state MATCHES (771b5c43d6066115 recorded, 771b5c43d6066115 replayed)
frames 378 recorded, 378 replayed (drift 0)
ending goal recorded, goal replayed
2 of 2 recorded decisions used, 0 model calls
FAITHFUL — the recording reproduced exactly
```

**A longer one, because a 378-frame run is mostly setup.** A 400-decision random
walk over the same cartridge, recorded and replayed:

| | frames | decisions | wall | rate |
|---|---:|---:|---:|---:|
| run | 21,887 | 400 | 2,839 ms | 7,709 frames/s (129.1x) |
| replay | 21,887 | 400 | 2,804 ms | 7,806 frames/s (130.7x) |

Final state `f6e2d9485d8c7797` both times, drift 0, **0 model calls**.
Machine: 20-core x86-64, headless, no audio. Quote the method with the number.

---

## 6. What has *not* been done

**Nobody has trained anything on this, and nothing here should be read as
saying otherwise.** What exists is a harness and an honest report: *this
scenario is solved in N decisions, replays deterministically in M seconds, here
is the file*. The recording is shaped so it could feed a learner — one row per
decision, observation in, action out, outcome, joinable to the agent plugin's
own `agent.ndjson` on `decision` — because that is cheap and obviously useful.
Whether it trains anything is a claim nobody here has earned.

Also honestly absent:

- **No scenario has been solved by a live model.** Every run measured above used
  a scripted or random driver, which is still *only pressing buttons* and so
  satisfies `no_memory_writes` — but it is not a measurement of a model. The
  `--driver command` path exists and is the same one
  [`docs/claude-plays.md`](claude-plays.md) documents; it has not been pointed
  at one for a scenario.
- **`battle_advice` is declared but not wired.** The scenario language accepts
  it, the briefing announces it as privileged and the report counts the
  decisions that could have carried it — deliberately over-counting rather than
  under — but no recommendation is computed yet. The battle engine that would
  compute it is [`crates/terminalgb-gen1-battle`](../crates/terminalgb-gen1-battle/src/lib.rs),
  and `ai::greedy::expected_damage` takes two `BattleMon`s directly, which is
  the hook.
- **`no_state_loads` has no runner that rewinds**, so its counter is always
  zero today. The rule is there because a rewind is exactly the assistance that
  must be impossible to give quietly.
- **The legitimate replacement for `shiny_hunt::make_state`** — playing a new
  game to Route 1 with buttons only, through the intro, the naming screen, Oak,
  the starter and the rival battle — is a scenario somebody can now *write*, and
  has not been run. It is a long chain and it costs real model calls.

## 7. Prior art worth reading rather than rediscovering

- **The harness dominates the model.** The PokéAgent Challenge
  ([arXiv 2603.15563](https://arxiv.org/abs/2603.15563)) reports that *without a
  harness, raw frontier VLMs achieve effectively 0% task completion*;
  lmgame-Bench ([arXiv 2505.15146](https://arxiv.org/abs/2505.15146)) measures
  40% of un-harnessed runs failing to beat random play against 86.7% with one.
  Cited already in [`docs/longplay.md`](longplay.md), and it is the reason this
  work is a harness rather than a prompt.
- **Persistent memory is the most dangerous component** — a wrong belief written
  down early and acted on for hours. That is why an episode carries no memory
  between attempts, and why learned guidance is a *separate, declared, per-run
  file* rather than something the agent writes into itself: it can be inspected,
  deleted, and its influence is counted in the report.
- **The Arcade Learning Environment and Gym-style environments** settled the
  shape of the reset-and-repeat primitive long ago; `src/longplay/episode.rs`
  is that shape and this builds on it rather than inventing a second one.
  [`tools/agent-docs/gb_env.py`](../tools/agent-docs/gb_env.py) is the
  Gym-*shaped*, Gym-*free* client. **No reward function is shipped**,
  deliberately — a default one becomes everybody's benchmark by accident.

## 8. Related pages

- [`docs/longplay.md`](longplay.md) — the long-horizon runner and the episode
  primitive this is built on.
- [`docs/claude-plays.md`](claude-plays.md) — the agent substrate: the driver
  contract, the governor, why the emulator never waits.
- [`docs/agent-reference.md`](agent-reference.md) — what an agent receives and
  what it may send back.
- [`docs/gen1/`](gen1/README.md) — the Gen 1 mechanics area, including
  [`memory-map.md`](gen1/memory-map.md) and the atlas pointer page.
- [`docs/writing-a-scenario.md`](writing-a-scenario.md) — the walkthrough: one
  small scenario built from an empty file, and the ways it goes wrong.
- [`docs/mcp.md`](mcp.md) — driving the emulator from an MCP client, which is
  the other way an agent reaches it.
