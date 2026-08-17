# Writing a scenario — from an empty file to a run you can replay

[`docs/scenarios.md`](scenarios.md) explains what a scenario *is*: how a rule is
enforced rather than requested, how a goal is judged, what a recording contains.
This page is the other half. It builds **one small, real scenario in front of
you**, from an empty file to a run that reaches its goal and replays exactly —
and then shows the ways it goes wrong, with the output each one actually
prints.

Every command below is one to run, in the order given. Steps 0 to 5 need **no
cartridge and cost nothing** — that is most of the authoring loop. Steps 6 to 8
need a Game Boy ROM, and this repository ships none.

The finished article is [`scenarios/tutorial/town-map.json`](../scenarios/tutorial/town-map.json)
— read it afterwards, not now.

---

## 0. Look at what is already here

The front end with no arguments tells you what it is and what it has:

```console
$ cargo run --release --example scenario --
bin:
  path: ~/gameboy/target/release/examples/scenario
  description: Author, check, play and replay scenarios — a goal, rules the host enforces rather than asks for, and a run recorded as a file that replays with no model calls
atlas:
  addresses: 2891
  from: /home/…/third_party/atlasgb/pokemon-rb/atlas.tsv
commands[5]{command,what it does}:
  check,validate a scenario and print its cost ceiling — no cartridge
  knowledge,ask the memory atlas which address records something
  guidance,print the map graph earlier runs walked
  run,play a scenario and record it
  replay,replay a recording — no model calls
scenarios[16]{file,name,goal,rules,evidence}:
  scenarios/leave-the-pokemon-center.json,leave-the-pokemon-center,map!=0x29,5,NONE
  scenarios/pewter/01-new-game.json,01-new-game,"map=0x26 AND watch:map_width>0",4,2
  …
help[6]:
  Run `cargo run --release --example scenario -- check <file>` to validate one and see its cost ceiling — no cartridge, no cost
  …
```

Two things to notice, because you are about to write a row in that table. Every
scenario's whole identity is **a goal and a count of rules**; and the last
column is how many lines of *evidence* it carries — which addresses define its
success and where the cartridge sets them. `leave-the-pokemon-center` says
`NONE`, and that is a real weakness of that file, not a formatting quirk.

`help <command>` answers about one command rather than printing a manual:

```console
$ cargo run --release --example scenario -- help check
```

---

## 1. Pick a goal somebody could argue about

A good first scenario is one sentence, has a definite answer, and is small
enough that when it fails you can see why.

> **Get the Town Map from your rival's sister.**

She sits in Blue's house in Pallet Town and hands over a Town Map. It is one
short walk and one conversation.

Now the question that decides whether this is a scenario or a wish: **what byte
in the machine is different afterwards?** Not "the screen said so" — a run is
judged by the host from the machine's own state, so if you cannot name the
memory, you cannot write the goal.

Three candidates, and the difference between them is the whole lesson:

| candidate | what it really says |
|---|---|
| `map=0x27` (Blue's house) | you walked in. Says nothing about the map. |
| `items>=1` (the bag has something) | true of every player who has ever picked anything up. |
| an event flag | the game's own record that *this* happened. |

Gen 1 keeps its story in **event flags**, and that is what the goal wants.

---

## 2. Find the memory that proves it

Start with the tool, not with a wiki. `knowledge` reads the vendored
[AtlasGB](https://github.com/Alchemy86/AtlasGB) snapshot — 2,891 Gen 1
addresses, each carrying the evidence tier it earned:

```console
$ cargo run --release --example scenario -- knowledge --search wEventFlags
atlas:
  addresses: 2891
  from: /home/…/third_party/atlasgb/pokemon-rb/atlas.tsv
matches[1]{symbol,addr,bytes,evidence,what it is}:
  wEventFlags,$D747,320,"rom,live","**The story, in 320 bytes.** A flag array of 2,560 single-bit events: every gift taken, every trainer beaten, every door unlocked. Bit 0 of byte 0 is the flag that Pallet Town's script checks before it lets you leave, which is why setting o… (276 chars total)"
```

So the array starts at **`$D747`** and holds 2,560 single-bit events in 320
bytes. What the atlas does *not* hold is which bit is which — that is a fact
about the cartridge's source, and it comes from
[pret/pokered](https://github.com/pret/pokered)'s
`constants/event_constants.asm`, which is a `const` list:

```asm
	const_def
	const EVENT_FOLLOWED_OAK_INTO_LAB   ; 0
	const_skip 2
	const EVENT_HALL_OF_FAME_DEX_RATING ; 3
	const_skip 2
	const EVENT_PALLET_AFTER_GETTING_POKEBALLS ; 6
	const_skip 17
	const EVENT_GOT_TOWN_MAP            ; 24
	const EVENT_ENTERED_BLUES_HOUSE     ; 25
```

Counting through the `const_skip`s gives `EVENT_GOT_TOWN_MAP` **index 24**, so:

```
byte = $D747 + 24/8 = $D747 + 3 = $D74A
bit  = 24 % 8       = 0          → mask $01
```

**Cross-check the arithmetic before you trust it.** Run the same count for a
flag somebody has already derived independently: `EVENT_GOT_POKEDEX` comes out
at index 37 → `$D74B` bit 5 → mask `$20`, which is exactly what the shipped
[`scenarios/pewter/09-receive-the-pokedex.json`](../scenarios/pewter/09-receive-the-pokedex.json)
declares. Agreeing with a file you did not write is what says your counting is
right.

And confirm the address lands where you think:

```console
$ cargo run --release --example scenario -- knowledge --at 0xD74A
query:
  address: $D74A
covers[1]{symbol,addr,bytes,evidence,what it is}:
  wEventFlags,$D747,320,"rom,live",…
```

### Read the script that sets it

An address is not enough; you need to know *when* the game writes it.
`scripts/BluesHouse.asm`:

```asm
BluesHouseDefaultScript:
	SetEvent EVENT_ENTERED_BLUES_HOUSE      ; ← on the frame the map loads

BluesHouseDaisySittingText:
	CheckEvent EVENT_GOT_TOWN_MAP
	jr nz, .got_town_map
	CheckEvent EVENT_GOT_POKEDEX            ; ← the precondition
	jr nz, .give_town_map
	…                                       ; otherwise: "RED is out at Grandpa's lab."
.give_town_map
	lb bc, TOWN_MAP, 1
	call GiveItem
	jr nc, .bag_full                        ; a full bag gives nothing…
	…
	SetEvent EVENT_GOT_TOWN_MAP             ; …and the flag stays clear
```

Three facts fall out, and each one changes the scenario:

1. **`EVENT_ENTERED_BLUES_HOUSE` is index 25 — the *next bit of the same byte*.**
   `$D74A` therefore goes non-zero the instant you step through the door. Any
   goal written on the whole byte fires for walking in. This is what the `mask`
   field exists for, and §9 shows it firing.
2. **The flag is set after `GiveItem` returns carry**, so it records the map
   actually changing hands and not the conversation happening.
3. **Daisy checks `EVENT_GOT_POKEDEX` first.** A run that starts before the
   Pokédex can *never* reach this goal, however well it plays. That is a
   precondition, and §9 shows what it looks like when you miss it.

---

## 3. Write the watch

`map`, `x`, `y`, `badges`, `party`, `party_hp`, `owned`, `money`, `items` and
`play_seconds` are built in and need no declaration. Anything else is a
**watch**: a named address the host samples.

```json
"watch": [
  { "name": "got_town_map", "addr": "0xD74A", "len": 1, "mask": "0x01" }
]
```

and the goal refers to it by name:

```json
"goal": ["watch:got_town_map!=0"]
```

**`mask` is the field that makes an event flag expressible.** Without it the
condition is about a byte that eight unrelated milestones share; with it the
condition is about one of them.

> A watch is **for the host only**. It goes into the report and the journal and
> is never shown to the agent — an agent told the answer is not solving the
> problem the screen poses. `src/longplay/watch.rs` keeps the two paths
> structurally separate and `testharness/longplay.rs` greps every request the
> driver was handed to prove it.

---

## 4. Write the rules

A rule is decided by the host from evidence the host owns, so nothing here can
be talked out of. Four are worth having in a first scenario:

```json
"rules": [
  "no_memory_writes",
  "battle decide",
  "require party>=1",
  "max_calls 60",
  "max_wall_seconds 600"
]
```

- **`no_memory_writes`** is the load-bearing one: nothing but button presses may
  reach the emulated machine. Without it, "reach the goal" is satisfied by
  writing `$01` to `$D74A`.
- **`require party>=1`** is the cartridge's own precondition stated as something
  the host checks, and it is judged **at the goal**, not throughout — see §9.
- **`max_calls` and `max_wall_seconds` are the bill.** A scenario with a live
  model behind it costs money per call, so the ceiling belongs in the file.

Then the two things that make the file *auditable*:

```json
"failure": ["party_hp=0"],
"evidence": [
  "watch:got_town_map!=0 — EVENT_GOT_TOWN_MAP, event index 24 in pokered's constants/event_constants.asm const list, so wEventFlags ($D747, atlas rom+live) byte 24/8 = 3 and bit 24%8 = 0: $D74A, mask $01. …"
]
```

`evidence` is one line per goal condition saying **which address defines success
and where the cartridge sets it**. It is optional and the file hashes the same
without it — and it is the difference between a goal a reader can check and a
goal they have to take on trust. `check` says so loudly when it is missing.

---

## 5. Check it — this is where the mistakes are cheap

```console
$ cargo run --release --example scenario -- check scenarios/tutorial/town-map.json
scenario:
  name: town-map
  file: scenarios/tutorial/town-map.json
  objective: Get the Town Map from your rival's sister. …
  goal: watch:got_town_map!=0
  failure: party_hp=0
  start_state: none — every attempt boots the cartridge cold
  timeout_frames: 40000
  battles: decide
cost:
  attempts: 1
  max_calls_each: 60
  model_calls_worst_case: 60
  frames_worst_case: 40000
rules[5]{rule,as written}:
  no_memory_writes,no_memory_writes
  battle,battle decide
  require,require party>=1
  max_calls,max_calls 60
  max_wall_seconds,max_wall_seconds 600
watches[1]{name,addr,len,mask}:
  got_town_map,0xD74A,1,0x01
knowledge[2]{source,state}:
  atlas (2891 addresses),available
  "guidance (new, will be written to scenarios/tutorial/guidance/town-map.json)",available
evidence[4]{for,citation}:
  "watch:got_town_map!=0","EVENT_GOT_TOWN_MAP, event index 24 …"
  …
briefing[1]{the agent is told}:
  "RULE: reaching the goal only counts if `party>=1` is also true."
verdict: OK
```

Read three lines of that before anything else.

**`cost:`** is the bill, printed before a penny is spent. **`briefing:`** is
what the agent will be *told* — a rule the host enforces and never states is a
trap, so check that anything constraining behaviour appears there.
**`evidence:`** is your own argument, read back to you.

`check` loads no cartridge and costs nothing, so run it after every edit. It
takes a directory too, which is what you want after touching a shared field:

```console
$ cargo run --release --example scenario -- check scenarios
checked[16]{file,name,verdict,evidence}:
  scenarios/leave-the-pokemon-center.json,leave-the-pokemon-center,OK,NONE STATED
  scenarios/tutorial/town-map.json,town-map,OK,4 line(s)
  …
summary:
  checked: 16
  ok: 16
  refused: 0
```

---

## 6. Get a cartridge and a start state

**This repository ships no game data.** For the run you need your own Pokémon
Red or Blue cartridge image.

A scenario also usually needs a **start state** — the moment every attempt
begins from. Ours needs a player who already has the Pokédex, because that is
the cartridge's precondition. There are two honest ways to get one:

**Play the chain.** The expert solver plays from a cold cartridge to any link
you name and writes the state it finished on. Nine links takes about four
seconds:

```console
$ cargo run --release --example pewter_chain -- chain \
    --rom <your-blue.gb> --seeds 1 --to 9 --out target/tutorial
…
=== link 9: 09-receive-the-pokedex
seed 1: GOAL — 5562 frames, 24 decisions, 694 ms, ended on map 0x28 (Oak's Lab) at (5, 3)
  link 9 solved; state carried forward
```

That leaves `target/tutorial/states/09.gbss`: standing in Oak's Laboratory in
Pallet Town, Pokédex in hand, one Pokémon in the party.

**Or use your own save.** Play to the same point in the terminal frontend and
press `F5`; the state lands under `${XDG_STATE_HOME:-~/.local/state}/terminalgb/states/`.
The route in §7 assumes you are starting in Oak's Laboratory.

---

## 7. Run it

A run needs a driver. Three exist — `random` (a local walker, no cost),
`command` (your model, behind a subprocess), and `scripted`, which reads one
JSON reply per line. Scripted is the one to learn on: **it goes through the
identical parse-clamp-execute path a live model's reply does**, so a route that
works scripted works with a model behind it, and it costs nothing to iterate.

[`scenarios/tutorial/town-map.script.jsonl`](../scenarios/tutorial/town-map.script.jsonl):

```json
{"goal":"Walk to the laboratory door","plan":[{"do":"goto","x":5,"y":10}]}
{"goal":"Step out of the laboratory","plan":[{"do":"walk","dir":"down","steps":2}]}
{"goal":"Let Pallet Town finish loading","plan":[{"do":"wait","frames":120}]}
{"goal":"Get off the laboratory doorstep","plan":[{"do":"goto","x":16,"y":12}]}
{"goal":"Walk up the east side of town","plan":[{"do":"goto","x":16,"y":7}]}
{"goal":"Walk to Blue's front door","plan":[{"do":"goto","x":13,"y":6}]}
{"goal":"Step into Blue's house","plan":[{"do":"walk","dir":"up","steps":2}]}
{"goal":"Let the house finish loading","plan":[{"do":"wait","frames":120}]}
{"goal":"Stand beside Daisy","plan":[{"do":"goto","x":2,"y":4}]}
{"goal":"Ask Daisy for the Town Map","plan":[{"do":"press","button":"up"},{"do":"advance_text"}]}
…
```

```console
$ cargo run --release --example scenario -- run \
    --rom <your-blue.gb> \
    --scenario scenarios/tutorial/town-map.json \
    --start target/tutorial/states/09.gbss \
    --out target/tutorial/town-map \
    --driver scripted --script scenarios/tutorial/town-map.script.jsonl
plan:
  scenario: town-map
  goal: watch:got_town_map!=0
  attempts: 1
  model_calls_worst_case: 60
  frames_worst_case: 40000
  driver: scripted
  start_state: target/tutorial/states/09.gbss
  out: target/tutorial/town-map

  GOAL REACHED after 1662 frames and 10 decisions (166 frames per decision)
  233 ms — 7133 frames/second, 119.4x real time
  ended on map Blue's house (0x27) at (2, 4) — 1 in the party, 0 badge(s)
  state resets: 1
  rewinds inside an attempt: 0
  host memory writes: 0
  knowledge consulted: atlas (2891 addresses), guidance (new, will be written to scenarios/tutorial/guidance/town-map.json)
  learned 2 door(s) between maps
  recorded: target/tutorial/town-map/run-1.json
```

**Read the allowances, not just the verdict.** `host memory writes: 0` and
`rewinds inside an attempt: 0` are what make "GOAL REACHED" mean anything; a
report that reached the goal with a memory write in it reached a different goal.

That route took five attempts to get right, and §9 has all five.

The run also *learned* something. Every door it walked through is recorded:

```console
$ cargo run --release --example scenario -- guidance scenarios/tutorial/guidance/town-map.json
guidance:
  file: scenarios/tutorial/guidance/town-map.json
  rom: POKEMON BLUE
links[2]{from,from_id,door_x,door_y,to,to_id,uses}:
  Oak's Lab,0x28,5,11,Pallet Town,0x00,1
  Pallet Town,0x00,13,6,Blue's house,0x27,1
notes: 0 — nothing has been written down about a map yet
```

The next run is handed that in words. `uses` is evidence: a door two runs found
is better evidenced than one seen once.

---

## 8. Replay it, for nothing

A recording is the driver's replies plus enough identity to prove it is being
replayed into the same world. It is not a video and not a log:

```console
$ cargo run --release --example scenario -- replay \
    --rom <your-blue.gb> \
    --scenario scenarios/tutorial/town-map.json \
    --start target/tutorial/states/09.gbss \
    --recording target/tutorial/town-map/run-1.json \
    --out target/tutorial/town-map
recording: 10 decisions recorded on "POKEMON BLUE" — 0 model calls

replay:
  final_state: MATCHES (b0287ce77b8ebb7a recorded, b0287ce77b8ebb7a replayed)
  frames_recorded: 1662
  frames_replayed: 1662
  drift: 0
  ending_recorded: Goal
  ending_replayed: Goal
  decisions: 10 of 10 used
  model_calls: 0
  wall_ms: 176
  frames_per_second: 9443
  scenario_drifted: false
verdict: FAITHFUL — the recording reproduced exactly
```

Exit code 0 when it is faithful, 1 when it is not. That is the artefact you hand
somebody: they can check your claim without spending a model call.

---

## 9. When it goes wrong

These are the five that actually happen, in the order a newcomer meets them.
Every block below is real output.

### The goal is empty, or names a watch nobody declared

Both are refused by `check`, before a cartridge is loaded:

```console
$ cargo run --release --example scenario -- check draft.json
error: draft.json: scenario "town-map" has no goal, so no run could ever reach one
help: Run `cargo run --release --example scenario -- help check` for what a scenario must carry
help: Read docs/writing-a-scenario.md for the field-by-field walkthrough
```

```console
$ cargo run --release --example scenario -- check draft.json
error: draft.json: condition "watch:got_map!=0" needs a watch called "got_map", which this scenario does not declare
```

A thousand attempts at a goal nothing could satisfy is a thousand timeouts and a
real bill. That is why `check` exists and why it is free.

### The goal fires far too early

Now the one `check` **cannot** catch. Take the mask out of the watch:

```json
{ "name": "got_town_map", "addr": "0xD74A", "len": 1 }
```

`check` says `verdict: OK` — it has no way to know that `$D74A` bit 1 is a
different milestone. The run tells you:

```console
  GOAL REACHED after 1116 frames and 8 decisions (140 frames per decision)
  ended on map Blue's house (0x27) at (2, 7) — 1 in the party, 0 badge(s)
```

against the masked scenario's

```console
  GOAL REACHED after 1662 frames and 10 decisions (166 frames per decision)
  ended on map Blue's house (0x27) at (2, 4) — 1 in the party, 0 badge(s)
```

**(2, 7) is the doormat.** The unmasked run declared victory on the frame the
map loaded, 546 frames and two decisions before anybody spoke to Daisy, because
`BluesHouseDefaultScript` sets `EVENT_ENTERED_BLUES_HOUSE` — bit 1 of the same
byte — the moment you walk in.

**How to spot it without knowing the answer:** a goal that fires suspiciously
early, or in the wrong place. The report prints where the run ended, so compare
it against where the milestone actually happens. If they differ, the condition
is about something else.

### The goal never fires, and no amount of playing helps

Run the *same* scenario from a start state one link earlier — before the Pokédex:

```console
  ran out of model calls after 1570 frames and 12 decisions (131 frames per decision)
  ended on map Blue's house (0x27) at (2, 4) — 1 in the party, 0 badge(s)
```

It walked the whole route, stood in exactly the right place, talked to exactly
the right person, and the flag never moved. The agent's own snapshot says why:

```
decision 9 | screen: "RED is out at Grandpa's lab."
```

That is `BluesHouseDaisyRivalAtLabText` — the branch `CheckEvent
EVENT_GOT_POKEDEX / jr nz` takes when the flag is clear. **The cartridge has a
precondition the scenario did not encode**, so the run was impossible from the
first frame.

**How to spot it:** the run reaches the right place and stalls. Read the screen
text in `<out>/agent-1.ndjson`, then read the script that would have set your
flag and look at what it checks *before* it gets there. Then either fix the
start state, or write the precondition as a `require` so the forfeit names it.

### A rule that forfeits every run

`require` is judged **at the goal**, so a requirement nothing satisfies turns
every successful run into a forfeit:

```console
  FORFEIT — require badges>=1 after 1662 frames and 10 decisions
  ended on map Blue's house (0x27) at (2, 4) — 1 in the party, 0 badge(s)
```

Identical frame count to the passing run — it did all the work, then failed the
requirement at the finish line. `forbid` is the opposite and ends the attempt
the first time it holds:

```console
  FORFEIT — forbid map=0x00 after 438 frames and 2 decisions
  ended on map Pallet Town (0x00) at (5, 11) — 1 in the party, 0 badge(s)
```

**A violation is a forfeit, not a bad score**, and the report names the rule and
the frame. If every attempt forfeits on the same rule at roughly the same frame,
the rule is wrong, not the agent.

### The route, which took five goes

The first four attempts at §7's script are worth having, because three of them
are properties of the machine rather than of that particular room:

| attempt | what happened | why |
|---|---|---|
| 1 | never left the laboratory | a plan with a `wait` after a door: **a map change cancels the rest of the plan**, so the wait never ran and the next decision was taken mid-warp |
| 2 | walked out and straight back in | the observation after a warp had `wCurMap` already changed while the map header was still the old room's. `wCurMap` is not "the map is loaded" |
| 3 | bounced between lab and town | `goto` issued from that mid-warp observation, against the wrong map's geometry |
| 4 | stalled at (16, 7) in Pallet Town | `goto` is greedy on the larger remaining delta and does not route around buildings — it needs the turning points |
| 5 | **goal reached** | one door per decision, a bare `wait` decision after each, and a `goto` per leg of the route |

Three rules for a route, and they are not specific to Pallet Town:

1. **Give a door its own decision, and give the settling its own decision after
   it.** A `wait` in the same plan as the walk that triggers the warp will not
   run.
2. **Never plan from an observation taken during a warp.** If the map's `width`
   and `height` do not match the map its `id` names, the world is mid-change.
3. **Break a route at its turning points.** `goto` walks, it does not navigate.

---

## 10. Where to go next

- [`docs/scenarios.md`](scenarios.md) — the reference: every rule, the condition
  grammar, what a recording contains, and what has *not* been done.
- [`docs/agent/pewter-chain.md`](agent/pewter-chain.md) — fourteen scenarios
  chained from a cold cartridge to Pewter City, each one's goal read out of the
  cartridge, and the link that scored 100% on the wrong task.
- [`docs/gen1/memory-map.md`](gen1/memory-map.md) and the
  [memory atlas](gen1/atlas/README.md) — where the addresses come from and how
  each one was graded.
- [`docs/agent-reference.md`](agent-reference.md) — the action grammar in full:
  what a driver is handed and what it may send back.
- [`docs/claude-plays.md`](claude-plays.md) — putting a real model behind
  `--driver command`, and the budget and breaker that stop it running away.
