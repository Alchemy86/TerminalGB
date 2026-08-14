# Agent play — reading state and pressing buttons

[← Gen 1 area](README.md)

An AI agent watches this emulator and plays Pokémon Blue, and says what it is thinking while it
does. **The full design, the OBS/Twitch recipe and the honest gaps are in
[`docs/claude-plays.md`](../claude-plays.md)**; this page is the Gen 1 half — what it reads, what
it presses, and the answers to the questions this page was left holding.

The code is [`plugins/agent/src/`](../../plugins/agent/src/lib.rs), the panel is
[`src/screen/agent_view.rs`](../../src/screen/agent_view.rs), and the proof is
[`testharness/agent.rs`](../../testharness/agent.rs).

---

## What it reuses, and what it added

**Read.** Party, badges, money and the battle opponent come from the trainer view's verified RAM
map ([memory-map.md](memory-map.md)); the screen comes from the tilemap-as-text trick
([reading the screen](reading-the-screen.md)) — read out of **VRAM** rather than `wTileMap`, so
what the agent is told cannot disagree with the picture a viewer is looking at, and so the
window-layer trap is handled in one place. Three addresses were added, each cited to its
`pret/pokered` symbol in [`plugins/agent/src/observe.rs`](../../plugins/agent/src/observe.rs):

| what | address | why it earns its place |
|---|---|---|
| `wSpriteStateData1 + $9` | `$C109` | which way the player faces — without it, "the press only turned me" is indistinguishable from "a wall" |
| `wNumberOfWarps` / `wWarpEntries` | `$D3AE` / `$D3AF` | **the doors.** A human sees a door; an agent reading coordinates does not know one exists. This turns "leave the house" into a `goto` |
| `wCurMapHeight` / `wCurMapWidth` | `$D368` / `$D369` | see below — the cleanest "am I in control?" test there is |

**Press.** `Capability::CONTROL_INPUT`, as the battle-control crew defined it. The agent uses the
*scheduled* level — `Machine::press(key, hold, gap)` through
[`crate::input::InputQueue`](../../src/input.rs) — rather than `set_button`, because it reasons
above the frame level and every caller keeping its own frame timer is how one mechanism becomes
three. Same capability, same latch; see [plugins.md](../plugins.md#control_input-and-why-there-is-exactly-one-input-mechanism).

**Decide.** [`docs/claude-plays.md`](../claude-plays.md). The short version: the model is slow and
the game is not, so the agent never presses buttons — it returns a goal, one line of commentary
and up to twelve *actions*, and local code carries them out over hundreds of frames.

---

## The seven questions

### 1. What is the goal, and how does it know it reached it?

Two levels, and keeping them apart is what stops the agent drifting.

* The **objective** is the operator's, fixed for the session, and put in every snapshot
  (`--objective`, or `agent_objective` in the config). For this milestone: *get out of the
  bedroom, downstairs, out of the house, and choose a starter.*
* The **goal** is the agent's own next sentence — "get downstairs", "find Oak's lab" — and it is
  what the commentary panel puts at the top for viewers.

Neither is the *termination* test. That is the host's, and it is a fact about the game rather
than a claim by the agent: `--stop-on party>=1` (`wPartyCount`) or `--stop-on map=0x28`. An
agent that says it has finished is not evidence; `wPartyCount` going from 0 to 1 is.

### 2. How does it know it is stuck?

A **fingerprint of the observable world** — map, x, y, badges, party size, and an FNV hash of the
20×18 tilemap — compared across *decisions*, not frames. Two deliberate exclusions:

* **facing is not in it.** Turning on the spot is exactly what a blocked walk produces, so
  counting it as progress would hide the commonest failure.
* **the frame number and the clock are not in it.** They always change, so including either
  would mean nothing was ever stuck.

That answers "an unchanged tilemap for N frames?" — a tilemap hash alone is not enough, because
an animated NPC changes it while the player gets nowhere, and a menu the agent keeps reopening
looks identical to one it opened once.

### 3. What does it do about it?

Escalate, because the right answer depends on how stuck you are
([`governor::Stuck`](../../plugins/agent/src/governor.rs)):

| repeats | response |
|---|---|
| 2–3 | tell the agent nothing has changed |
| 4–6 | insist: do not repeat that plan; back out with B and go somewhere new |
| 7+ | the host takes over for one plan: **B twice**, then a step *away* from whatever was being walked into — and no model call is spent |

B twice rather than "B until something changes", for the reason this page's own sharp-edges
section gives: B can never open a menu, so two are enough to leave a nested one and cannot make
things worse. Then it is the agent's problem again.

Two **situational** notes do more work than the generic ladder, and both were added after
watching a real run flounder in exactly these spots:

* *"You did not move at all last time."* Emitted when every movement in the last plan achieved
  nothing. Walking in all four directions failing almost never means you are surrounded by
  walls; it means the game is not accepting movement.
* *"You are standing ON an exit."* Emitted when the player's coordinates equal a warp's. Gen 1
  fires a warp when you step *through* a door, so arriving sideways along the bottom row of a
  house leaves you on the mat with nothing to show for it — and the agent had been pressing A at
  it, six calls running.

### 4. What does it read, and how often?

Only at **decision points**, never per frame — and a decision point is roughly one per 1,100
emulated frames on this game's opening. Between decisions the executor reads position only on
frames where the input queue is *idle*, which is a handful per action; while a press is in
flight it does nothing at all. The 360-`peek` screen read happens once per decision and inside
`advance_text`, which is the one action that has to watch the screen.

The trainer view's 10 Hz sampling was the right budget for a panel a human reads. An agent's
budget is different: it is bounded by how often it *thinks*, not by how often it looks.

### 5. Is it deterministic, and does that matter?

The emulator is; the model is not. So the substrate is built so that **the agent is the only
non-deterministic part**, and it can be removed:

* `ScriptedDriver` replays fixed replies through the identical parse → clamp → execute path a
  model's answer takes. Every test in `testharness/agent.rs` uses it, and every one is exactly
  reproducible.
* `RandomDriver` is seeded, so a random run reproduces from its seed.
* A run's whole decision stream — request, reply, cost, error — is written as NDJSON
  (`--log`), so a model run can be *replayed* as a scripted one.

For a clip that has to be reproducible, use a save state plus a scripted driver, the same
argument [battle control](battle-control.md#making-a-cinematic-reproducible) makes.

### 6. How is it visible?

`docs/plugins.md` §4's promise, with a second marker for the second thing a plugin can do to your
game: the Status row shows `agent>` for *may press your buttons* and a sticky `agent>>` for *has*,
beside the existing `!` / `!!` for memory. Both markers sit on the plugin's **name**, because the
detail is truncated to fit the panel and the warning must survive that.

Asserted in **rendered output**, not on the row's text —
`a_plugin_that_presses_buttons_is_marked_in_the_rendered_status_panel` draws the real frontend at
several window sizes and looks for the marker. That is the third bug of this shape in this panel:
the height had been hard-coded as "eleven rows plus the plugins" and the next crew to add a row
silently clipped the last plugin away again.

Beyond the marker: the commentary panel says what the agent is doing in words, `G` takes the
controller back at any time, and the [telemetry](../telemetry.md) record carries an `agent`
object so an operator can watch a stream's health without reading the panel.

### 7. Where is the boundary with battle control?

As this page proposed, and it held up. Battle control **sets up** a scenario and can play a fixed
script through it; the agent plays an open-ended one and never writes memory — it does not hold
`WRITE_MEMORY`, so `Machine::poke` refuses it. If an agent wants a specific battle to practise
against, it arms a preset; it cannot write `wCurOpponent` itself even if its code tried.

The one place the two crews had to converge is input, and they have: one capability, one latch,
two levels on it.

---

## Gen 1 facts this cost real model calls to learn

Each of these is now in the agent's prompt or in a host note, and each was found by watching a
real run waste decisions on it. They belong beside the ones in
[sharp edges](sharp-edges.md#driving-the-game).

* **The naming screen is the one place mashing A does not work.** `YOUR NAME?` / `RIVAL NAME?`
  over a grid of letters is the on-screen keyboard: A types another letter, for ever. **START
  confirms.** A run spent eleven calls typing `AAAAA` before this went into the prompt.
* **`wCurMap` is set during the opening cutscene** — this page already says it cannot mean "we
  are in the game", and it is worse than that: during Oak's speech it reads the *bedroom*, with
  plausible coordinates, while the map header has not been loaded. So position, map name and
  mode all look like ordinary play and walking does nothing. **`wCurMapHeight`/`wCurMapWidth`
  are the test**: `0` until `LoadMapHeader` runs, `4`/`4` the instant the bedroom loads, and no
  real map is 0×0. The observer reports `mode: "cutscene"` on that basis, which turned "try
  every direction for six calls" into "press A".
* **A door is not opened with A.** Standing on the mat does nothing; you have to step *through*
  it, which for a house door at the bottom of a room means one more `down`.
* **A press that only turns the player is not a blocked press.** Reported as a wall on the first
  unchanged coordinate, this tells the agent every direction is a wall — the most misleading
  thing the layer could say. Three consecutive failures, not one.
* **`HandleMenuInput`'s post-accept delay is real**, as this area already recorded: the agent's
  `press` action spaces repeats by 24 frames (0.4 s) rather than the default 8.
* **Oak's Lab is not the route to a starter.** "Go to the lab" is the obvious plan and it does
  not work: walking straight in reaches an empty room. Verified with the deterministic driver —
  from power-on into Oak's Lab in 9,034 emulated frames, party still 0, nothing at the tiles the
  balls sit on. The game gates the starter behind trying to **leave town to the north**, which
  fires `PalletTownDefaultScript` and has Oak escort you back; that is the same event
  [sharp edges](sharp-edges.md#driving-the-game) notes can be skipped outright by setting
  `EVENT_FOLLOWED_OAK_INTO_LAB`. An agent given the objective *"choose a starter"* found the
  north route by itself; it is worth knowing that the direct one is a dead end before concluding
  the navigation is broken.
* **A map's width and height are in 32×32 blocks and its coordinates are in 16×16 tiles**, two
  per block — see the table at the top. Reporting the raw header values told the agent Pallet
  Town was 10 wide when its `x` runs 0..19, so it decided it was standing outside the map.
