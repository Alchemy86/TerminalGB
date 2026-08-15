# Shiny hunting in Gen 1 — many emulators, one DV pattern

> **Generation I has no shiny Pokémon.** Nothing in Red or Blue is shiny and
> nothing here claims otherwise. What this hunts is the DV spread that *becomes*
> shiny the moment the Pokémon is traded forward to Gen 2 — see
> [What we are actually hunting](#1-what-we-are-actually-hunting).

The captain saw a hunter run **four physical Nintendo DS consoles wired to one
controller through a hardware input splitter**, so identical presses hit all four
at once — then drop back to two, controlled independently, because the four-way
rig kept desyncing. This is the software equivalent, for Gen 1, and it is a
better tool than the rig for one reason that has nothing to do with parallelism:

**we never look at a sprite.** The instant the game finishes rolling the
encounter we read `wEnemyMonDVs` out of memory and know the answer. There is
nothing to watch, so there is nothing to miss.

* **Plugin** — [`plugins/gen1-shinyhunt/`](../plugins/gen1-shinyhunt/), installed
  as `gen1-shinyhunt`, feature `plugin-gen1-shinyhunt`.
* **Driver and CLI** — [`examples/shiny_hunt.rs`](../examples/shiny_hunt.rs).
* **Contract it is built on** —
  [`docs/plugins.md`](plugins.md) / [`docs/writing-a-plugin.md`](writing-a-plugin.md).

---

## 1. What we are actually hunting

Shininess is a **Generation II** concept, and Gen 2 derives it from the DVs a
Pokémon already carries. DVs cross the Time Capsule unchanged, so a Red/Blue
Pokémon has a settled answer to "what happens if this is traded forward" long
before Gold/Silver exists to ask it. The rule (`CheckShininess`,
`pokecrystal/engine/gfx/color.asm`) is:

> **defence, speed and special all exactly 10**, and the **attack DV** one of
> 2, 3, 6, 7, 10, 11, 14, 15 (bit 1 set).

Eight spreads out of 65,536 — **1 in 8,192**. Note that a *perfect* 15/15/15/15
is **not** shiny; this is a pattern, not a quality.

Three facts make the answer available at the encounter, and each one is load
bearing:

1. **A wild Pokémon's DVs are rolled when the battle starts.**
   `LoadEnemyMonData` (`pokered/engine/battle/core.asm`) gives a wild Pokémon two
   `BattleRandom` bytes; they land in `wEnemyMonDVs` before the battle is drawn.
2. **Capture copies those exact bytes.** `_AddPartyMon`
   (`pokered/engine/pokemon/add_mon.asm`) takes its `.copyEnemyMonData` branch
   for a Pokémon caught in battle and copies `wEnemyMonDVs` verbatim. Nothing is
   re-rolled, so a verdict read at the encounter is still true of the Pokémon in
   the Ball.
3. **A trainer's Pokémon is not a roll at all.** The same routine gives every
   trainer Pokémon the fixed pair `ATKDEFDV_TRAINER`/`SPDSPCDV_TRAINER`
   (`$98`/`$88`), which is not the shiny pattern — so a trainer battle is never a
   hit, and the plugin skips it rather than reporting it.

**There is exactly one implementation of the rule in this repository**:
[`terminalgb_plugin_trainer::derived::shiny_if_traded`](../plugins/trainer/src/derived.rs),
already used by the trainer panel's `★` and by the wild-encounter banner. The
hunter calls it. `target.rs` carries a test that enumerates all 65,536 spreads
and requires the hunt's `shiny` target to agree with that function on every one
of them, so a second copy of the rule cannot appear without a red test.

---

## 2. Quick start

```bash
# Build a starting position from a bare cartridge: a fresh game walked to the
# grass at the southern end of Route 1.
cargo run --release --features plugin-gen1-shinyhunt --example shiny_hunt -- \
    --rom blue.gb --make-state route1.gbss

# Check it before trusting it — this also proves the instances diverge.
cargo run --release --features plugin-gen1-shinyhunt --example shiny_hunt -- \
    --rom blue.gb --state route1.gbss --probe 3

# Hunt. Ctrl-C whenever; --time-limit and --max-attempts also stop it.
cargo run --release --features plugin-gen1-shinyhunt --example shiny_hunt -- \
    --rom blue.gb --state route1.gbss --out ~/shiny-hit
```

`--help` lists every flag. Exit status is **0** for a hit, **1** for no hit
within the limits, **2** for an error, so it composes with a shell.

### What you have to supply

**A save state positioned for the hunt.** Every attempt restarts from exactly
those bytes, so where they were taken *is* the setup:

| shape | what the state must show | examples |
|---|---|---|
| `grass` (default) | standing **in** tall grass, **out of battle**, on a map with an encounter table | any route, cave or water tile |
| `reset` | in front of the thing that generates the Pokémon, so that pressing A starts it | a static legendary, a gift, an in-game trade |

`--make-state` builds a `grass` one for you from a brand-new game. It is a
convenience for getting started and for the demonstration below; the interesting
hunts start from positions only a player can reach.

**A save state taken during a battle is the classic mistake** — the last step
into grass is quite likely to be the step that rolls an encounter — so the driver
detects it and says so rather than reporting thousands of silent stalls.

---

## 2a. Watching a save state being built (`--show`)

`--make-state` finishes in about a second and prints a dozen lines. Add
**`--show`** and the same walk runs with the picture drawn and the frames paced,
with a caption under it saying what it is doing:

```bash
cargo run --release --features plugin-gen1-shinyhunt --example shiny_hunt -- \
    --rom blue.gb --make-state route1.gbss --show
```

`--show-speed <x>` sets the pace as a multiple of the DMG's own 59.7275 Hz.
The default is **2** — the walk is about three minutes of emulated time, so
twice speed is a ninety-second demo that is still followable. `--show-speed 1`
is real time and is what to record a video at; `--show-speed max` runs flat out
and is what the test harness uses. `q`, `Esc` or `Ctrl-C` aborts, and an aborted
run writes no state.

**The state `--show` produces is byte-identical to the headless one, and that is
enforced rather than promised.** `testharness/watchstate.rs` runs the real
example binary both ways — the watched one on a pty it owns, since `--show`
refuses to start without a terminal — and compares the two files byte for byte.
It also requires the watched run to have actually *drawn* something, because
"identical" would otherwise be satisfied by a `--show` that quietly did nothing,
and requires neither run to leave a `.sav` beside the cartridge. It needs your
own ROM and skips with exit 0 without one:

```bash
cargo build --release --example shiny_hunt
POKE_ROM=blue.gb cargo test --release --test watchstate
```

Two design points make the identity structural rather than a coincidence:

* **Watch mode changes what happens *between* two frames, never inside one.**
  The walk's one primitive is a `Stage` trait with a single `frame` method;
  every helper calls `stage.frame(gb)` where it used to call `gb.frame()`, so
  both modes execute the same code with the same buttons down on the same
  frames. Watch mode adds a sleep, a redraw and three `peek`s around a frame.
* **The viewer is never given the machine.** `screen::watch::Viewer` takes a
  framebuffer (`&[u8]`), so it cannot write emulated memory even by mistake —
  and the machine `--make-state` builds still has no battery, no audio player
  and no plugins. Those are not omissions: an installed audio player makes the
  APU visible to the cartridge (`Sound::do_cycle` returns immediately with no
  player, so a machine without one has *no APU at all*, not a quiet one), and a
  battery would write a `.sav` next to somebody's ROM.

Teeth proven, not assumed: adding a stray `gb.debug_write` to the watched path
turns the harness red at byte 54 with both files still 174,879 bytes long, and
adding one extra emulated frame per displayed frame breaks the walk outright.

---

## 2b. What the walker actually does — and why it is not the agent

Watching `--show` reach Route 1 in three emulated minutes, next to
[the agent](claude-plays.md) taking a dozen model calls to find Oak's lab, invites
an obvious and wrong conclusion. **They are not the same job and the walker is
not the better one.** It is a satnav with the postcode already typed in; the
agent is looking out of the window.

| | the walker (`--make-state`) | the agent (`plugins/agent`) |
|---|---|---|
| how it knows where it is | reads `wCurMap` / `wYCoord` / `wXCoord` out of WRAM | decodes `wTileMap` as **text** and works it out |
| where it is going | a coordinate a human read off `pret/pokered` and hard-coded | whatever it decides the goal is |
| may it write memory? | **yes, and it does — twice** | **no.** `OBSERVE \| READ_MEMORY \| CONTROL_INPUT`, and the host refuses `Machine::poke` |
| what it recognises | nothing at all | the screen, the situation, the consequence |
| what a result means | "the script ran" | "something worked this out" |

The walker's two memory writes are the whole point of the asymmetry, and
`--show` calls them out on screen as they happen because they are otherwise
invisible:

* **a level 50 PIDGEOT into `wPartyMons`** — a brand-new game has no Pokémon,
  and walking into grass without one hangs the first wild battle. Fast and
  tough on purpose (see §4): a run flees dozens of battles and a party that
  faints blacks out to a Pokémon Center, where there is no grass.
* **`EVENT_FOLLOWED_OAK_INTO_LAB`, bit 0 of `$D747`** —
  `PalletTownDefaultScript` (`scripts/PalletTown.asm`) opens
  `CheckEvent EVENT_FOLLOWED_OAK_INTO_LAB / ret nz`, so setting it is what stops
  Oak running out to block the north exit. The agent has to *do* the
  introduction; the walker skips it by decree.

Everything else really is walking. The walker presses buttons at menus, is
stopped by Mum's send-off downstairs, has to step off its own doorstep before it
can head north, and runs from whatever the last step into grass rolls — all
things `--show` makes visible, and all things it gets wrong often enough to need
the retry loops described in §8. **The one thing it never does is teleport**: it
does not edit `wCurMap`, because `LoadSAV` sets `BIT_NO_PREVIOUS_MAP` and
`LoadMapHeader` then returns without loading the header, so a moved map is read
through the old map's data pointers and the game jumps into `rst $38` within a
second.

So: the walker is fast because somebody already did the understanding, once, by
hand, and wrote the answer down. That is a fine way to build a fixture and a
useless way to measure an emulator's usefulness to a player. **The agent's slowness
is the measurement.**

---

## 3. Architecture: a plugin, and the driver beside it

### The plugin observes and can do nothing else

`ShinyHunt` declares `OBSERVE | READ_MEMORY` and **nothing else**. The host
therefore refuses it `Machine::poke` and `Machine::press` — enforced, not
promised (`plugins/gen1-shinyhunt/src/lib.rs`, and
`the_hunter_cannot_write_memory_or_press_buttons`). That matters more here than
in most plugins: **a hunter that could write memory could fabricate its own
hit**, and then no result it ever reported would mean anything.

Once per frame it samples six bytes and, when a wild battle's enemy structure is
populated, latches a `Sighting`: species, level, the DV spread, and how far into
the attempt it appeared.

### What the plugin API could not do

The driver is not the plugin, and that is a finding.

A plugin is a *passenger*: it is handed a borrowed `Machine` inside a callback,
and the callback ends. It cannot press a button on a schedule, step a frame, load
a save state, or start a second emulator. Hunting needs all four.

So the driver sits beside the plugin, over an eight-method seam:

```rust
pub trait HuntMachine: Send {
    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String>;
    fn save_state(&self) -> Vec<u8>;
    fn frame(&mut self);
    fn key(&mut self, key: KeypadKey, down: bool);
    fn peek(&mut self, address: u16) -> u8;
    fn sighting(&self) -> Option<Sighting>;   // the attached hunter's latch
    fn arm(&mut self);
    fn refusal(&self) -> Option<String>;
}
```

The emulator implements it for `Gameboy` in `src/gameboy.rs`, behind the
`plugin-gen1-shinyhunt` feature. Every method is a one-line forward to a public
`Gameboy` method that already existed.

**This is reported as a finding for the plugin-API crew, not worked around behind
its back.** The API is not missing anything a *plugin* should have; what it is
missing is a *host-side* half — a stated way for an extension to drive the clock
and own several machines. The MCP server reached the same conclusion from the
other direction and is a frontend rather than a plugin
([`docs/mcp.md`](mcp.md) §"A frontend, NOT a plugin"). Two independent features
arriving at the same shape is the signal worth acting on. Concretely, three
things would remove the seam:

1. **A way for a plugin to hand a value back to whoever is driving.** Today the
   only route is `Gameboy::plugin::<T>()` — a downcast to the concrete type,
   which a generic host cannot use. This is what `HuntMachine::sighting` exists
   for and it is the smallest of the three.
2. **A declared "host" trait** in the API crate covering step/load/save/press, so
   a driver can be written once against any host rather than against `Gameboy`.
3. **Something to say about `N` machines.** Nothing in the contract acknowledges
   that a plugin type might be instantiated many times over the same cartridge.
   It works today (the plugin holds no global state), but it works by accident
   rather than by promise.

Until then, the trait is the honest way to write it: the plugin crate never names
`gameboy`, so the dependency graph stays acyclic (the emulator depends on its
plugins, never the reverse), and the whole driver loop is unit-tested in CI
against a fake machine with no ROM and no emulator involved.

---

## 4. Decorrelation — the interesting problem

Emulation is deterministic and every instance starts from the same bytes, so
**identical input produces an identical Pokémon**. Four instances doing the same
thing are one instance wearing four hats. That is the DS rig's desync problem
inverted: the splitter's consoles drift *apart* until they stop agreeing, and
ours never drift at all.

The lever is **time**. Gen 1's `Random_` (`pokered/home/random.asm`) folds `rDIV`
— the free-running divider register — into its state on every call, and `rDIV`
advances with cycles. So changing *when* a press lands changes every subsequent
roll. Each attempt gets a `Schedule` drawn from a small deterministic PRNG seeded
by `(instance seed, attempt number)`: a **lead-in** of 0–239 idle frames, then a
**hold** of 8–40 frames and a **gap** of 0–24 drawn *afresh for every step*.

Nothing else feeds the schedule, so **a hit is reproducible from three
numbers** — the base state, the instance seed and the attempt index.

### Divergence is necessary and **not** sufficient — the finding that cost the most

Instances that differ from one another can still, between them, only ever produce
a handful of Pokémon. Measured, on the real cartridge:

> **968 of the 65,536 DV spreads were reachable.** A 97,212-encounter hunt for a
> 1-in-8,192 target found nothing — and never would have, because none of the
> eight spreads it wanted was in that set. Throughout, the tool reported a
> healthy 81 encounters/s, twelve instances all diverging from one another, and
> an ETA of 1.7 minutes.

That is the worst kind of failure this project has: confident, well-instrumented,
and wrong. The cause is a property of the game, not of the driver:

```asm
Random_::
	ld hl, hRandomAdd
	ld a, [rDIV]
	adc [hl]        ; hRandomAdd += rDIV
	ld [hli], a
	ld a, [rDIV]
	adc [hl]        ; hRandomSub += rDIV
	ld [hl], a
```

`LoadEnemyMonData` takes the two DV bytes from two **consecutive** `Random`
calls, microseconds apart, so both read the *same* `rDIV` sample `R`:

```text
hi = A + R          lo = hi + R          (A = hRandomAdd before the roll)
```

So **`lo − hi` is that one `rDIV` sample** — the two bytes are not independent,
and the pair is fixed by `(A, R)` rather than by two free bytes. Now add a
save state that is reloaded before every attempt: `A` and `R` are then both
functions of elapsed frames, the pair traces a single one-dimensional band, and
no amount of input jitter and no number of parallel instances can leave it.
Measured on that band: **2.7 distinct `lo` values per `hi`**, where independence
would give 256.

*(A consequence worth knowing if you ever hand-check this: every shiny-if-traded
spread needs `lo = $AA` and `def = 10`, so `R = $AA − hi` is always a multiple of
16. A wild roll can only be shiny when its `rDIV` sample has a zero low nibble.)*

### The fix: run, don't reset

**A battle is how the RNG gets off the band.** Gen 1's battle engine calls
`Random` constantly — accuracy, damage, the AI, the run-away roll — so the state
after fleeing one is somewhere else entirely, and the next encounter starts from
a different band. This is also, not coincidentally, exactly what a human
hunter's console is doing between encounters.

So an attempt is a **run**: reload once, then meet `--run-length` Pokémon in a
row, running from each that is not a hit. Every one is judged. Purity is kept —
everything inside an attempt is deterministic, so a hit still replays from
`(base state, seed, attempt)` and the report names which encounter of the run it
was.

Reloading between every encounter is still available (`--run-length 1`) and is
the right thing for [`Shape::Reset`], where the reset *is* the method.

### Coverage is measured, not assumed

`--probe` now reports it, because "can this hunt ever win?" is a different
question from "do the instances differ", and only one of them is visible from the
encounter rate:

```text
runs:     72 attempts, mean run 26.2 of 32 encounters  (ended early: 27 stalled, 0 could not flee, 0 blacked out)
coverage: 1883 encounters, 1622 distinct DV spreads -> ~6149 of 65,536 reachable
          expected shiny-if-traded spreads inside a set that size: 0.75  — TOO NARROW …
```

The run-health line is beside it because a run that keeps stopping after five
encounters is not a slow hunt, it is a broken one, and the three causes want
three different fixes — a **stall** means the walk left the grass, a **flee
failure** means the battle would not end, a **blackout** means the party is too
weak for a run of battles. That line is what turned "the encounter rate is fine
but coverage is stuck" into the two walk bugs in §8.

The estimate inverts `distinct = S·(1 − e^(−n/S))`, the expected number of
distinct values after `n` uniform draws from a set of size `S`. It is a coverage
estimate, not a uniformity test: it says how many spreads are reachable, not
whether they are equally likely. **A verdict of `TOO NARROW` means the hunt
should be reconfigured, not left running** — a longer `--run-length`, a different
base state, or a different route.

### Where this leaves a wild shiny hunt, stated plainly

Measured, from `--make-state`'s Route 1 position:

| loop | reachable spreads | expected shiny spreads inside |
|---|---:|---:|
| reload every encounter | ~968 | 0.12 |
| run of 32, fleeing between | ~6,150 | 0.75 |

Six times better, and **still not enough to promise a hit**. Running *longer*
inside an attempt does not push it further — 4.8 encounters per run and 26.2 per
run give the same ~6,000 — so the remaining narrowness looks structural to the
wild-encounter path rather than to the reload.

The honest reading is: **a wild shiny-if-traded hunt from a given base state has
roughly a coin-flip's chance of being possible at all**, and the tool now says
which side of that coin your starting point is on before you leave it running
overnight. What it will not do is report a healthy 81 encounters/s and a 1.7-
minute ETA for something it can never find, which is what it did before this was
measured.

Two things worth trying if a base state comes back `TOO NARROW`, neither of which
is implemented here: **several base states** (the reachable set is a property of
where you started, so their union should be larger), and a **static or gift**
Pokémon, whose DVs are rolled by `_AddPartyMon` with no encounter-rate check in
front of them.

### Proved in both directions

`--probe N` runs the first `N` attempts of every instance and prints what each
one met. Measured on Pokémon Blue, four instances, three attempts each (this
listing predates the run loop, so each attempt is one encounter):

```
instance 0  seed 0xe4028f46d80a1f2e
  attempt 0: RATTATA      lv3   atk 11  def 10  spd  7  spc  1  hp 11  (total 40/75)
  attempt 1: RATTATA      lv3   atk  4  def  0  spd 12  spc  9  hp  1  (total 26/75)
  attempt 2: PIDGEY       lv3   atk 10  def  8  spd  6  spc  6  hp  0  (total 30/75)
instance 1  seed 0xbd3201f3f5092910
  attempt 0: RATTATA      lv3   atk  6  def 15  spd 14  spc  6  hp  4  (total 45/75)
  attempt 1: RATTATA      lv3   atk  0  def 14  spd 15  spc 12  hp  2  (total 43/75)
  attempt 2: RATTATA      lv4   atk  2  def  2  spd  5  spc 15  hp  3  (total 27/75)
instance 2  seed 0xf878a2aa2b7ee01c
  attempt 0: RATTATA      lv4   atk  2  def 13  spd  0  spc 15  hp  5  (total 35/75)
  attempt 1: RATTATA      lv4   atk  5  def  4  spd  7  spc 15  hp 11  (total 42/75)
  attempt 2: PIDGEY       lv3   atk 15  def  4  spd  4  spc  4  hp  8  (total 35/75)
instance 3  seed 0xa3c974eb63f8282c
  attempt 0: PIDGEY       lv2   atk 12  def  7  spd 12  spc 11  hp  5  (total 47/75)
  attempt 1: RATTATA      lv3   atk 13  def  2  spd  2  spc 13  hp  9  (total 39/75)
  attempt 2: PIDGEY       lv3   atk  1  def  0  spd  5  spc  7  hp 11  (total 24/75)

instances diverge: YES — every instance produced a different sequence
```

**The control case matters just as much**, and is the half people skip: a
decorrelation that "worked" because the emulator was non-deterministic would be a
far worse bug than no decorrelation at all. `--no-decorrelate` gives every
instance the same seed, and every instance must then agree *exactly*:

```
instance 0  seed 0x5eedc0ffee15600d
  attempt 0: PIDGEY       lv4   atk 14  def  3  spd  3  spc  3  hp  7  (total 30/75)
  attempt 1: RATTATA      lv3   atk  5  def 10  spd 13  spc 11  hp 11  (total 50/75)
  attempt 2: PIDGEY       lv4   atk  4  def  1  spd 12  spc  8  hp  4  (total 29/75)
instance 1  seed 0x5eedc0ffee15600d
  attempt 0: PIDGEY       lv4   atk 14  def  3  spd  3  spc  3  hp  7  (total 30/75)
  … identical for instances 2 and 3 …

control (decorrelation off) — every instance identical: YES, as it must be:
emulation is deterministic, so the same schedule gives the same Pokémon
```

The same claim is pinned at unit speed, with no ROM, by
`hunt::tests::instances_diverge_when_decorrelated_and_agree_when_they_are_not`.

### What is *not* reproducible, stated plainly

**Which instance reports the hit is a race.** Two runs of the same hunt with the
same seed can stop on different Pokémon, because whichever thread reaches a
matching encounter first wins. What is reproducible is each *attempt* — and that
is the property a hit report needs. `Hunt::run` therefore replays the winning
attempt in a **fresh machine** before claiming anything, and the report says
whether it reproduced. `--replay <instance>:<attempt>` re-runs one attempt on
demand; it is printed in every hit report as the recipe.

---

## 5. Measured performance

Measured on this box (20 logical cores, `--release`, other work running
concurrently), Pokémon Blue, Route 1 grass, `--make-state`'s own base state:

| instances | loop | encounters/s | emulated speed |
|---:|---|---:|---:|
| 8 | reload every encounter | 76–86 | 429–475x realtime |
| 12 | reload every encounter | 81 | 438x realtime (97,212 encounters in 20 min) |
| 12 | **run of 32, fleeing between** (the default) | **8–18** | **176–367x realtime** |

The run-loop range is wide because this box had other work on it — both figures
are real, from the same binary and the same base state twenty minutes apart, and
quoting only the better one would be the kind of number this document exists to
avoid. **The run loop is roughly five times slower per encounter than reloading,
and it is still the right default**, because the encounters it produces are the ones that can
actually contain a hit — see §4. Most of the cost is the flee: working the battle
menu, waiting out "Got away safely!", and the battle's own animations.

Reloading costs ~330 emulated frames per encounter (a load, a lead-in and a short
walk); the run loop costs roughly 1,250, most of it battle.

**The useful instance count is bounded by cores, not by imagination.** Every
instance is a full emulated Game Boy running flat out, so past one per core they
take turns and the aggregate encounter rate stops rising. The default is
`available_parallelism() - 1`, which leaves the box usable while a hunt runs
overnight.

### The honest expectation

At **1 in 8,192**, and at 8–18 encounters/s on the run loop:

* **mean** 8,192 encounters ≈ **8–17 minutes**
* **median** 5,678 encounters ≈ **5–11 minutes**

…**provided the target is inside the reachable set**, which is the caveat §4
exists for and which no rate can tell you. Check `--probe`'s coverage line first;
a `TOO NARROW` verdict means those minutes are infinite.

Both numbers are printed, and the median is printed on purpose: a geometric
distribution's mean sits well above its median, so quoting only the mean reads
pessimistically and makes people give up early. If a target's true expectation is
"days", the tool says days — that is the number that answers *should I leave it
running overnight?*, and it is computed from the **measured** rate, not an
assumed one.

Route 1's encounter rate (25/256 per step) is unusually generous, and its
encounter table has two species. A hunt for a specific species somewhere rarer is
correspondingly slower, which is why the odds line prints a caveat whenever
`species=` is set: **the quoted odds are per encounter *of that species***, and
how many encounters that is depends on the route's table, not on the DV space.

---

## 6. Stopping on a hit, and what is preserved

**A hit that cannot be reproduced is worthless**, so the driver preserves five
things and checks the sixth:

```
target/shinyhunt/hit/
  hit.gbss   the save state at the exact moment of the encounter
  hit.txt    the full report, including the command that reproduces it
  hit.ppm    the frame (convert with `magick hit.ppm hit.png`)
```

`hit.txt` from a real run — this one used a deliberately common target so the
whole path could be demonstrated in seconds (see §7):

```
species          RATTATA (internal #165, dex #19)
level            4
DVs              atk  6  def 10  spd  3  spc 10  hp  2  (total 31/75)
shiny if traded  no
hidden power     GRASS 44

target           spc=10, def=10
odds             1 in 256 encounters

instance         7
attempt          17
instance seed    0xbbdbe2bba0f162f1
root seed        0x5eedc0ffee15600d
shape            wild grass
location         map $0C, x 10, y 33
elapsed          1.3s
replayed         yes — a fresh machine given the same three numbers produced the same Pokémon

To catch it: load hit.gbss into the emulator with this cartridge — the encounter
is on screen and the Ball is unspent.
```

Note `shiny if traded: no` and the report's opening paragraph, which says the
Pokémon matched *the target that was set* and is explicitly not the shiny
spread. **The preamble is chosen from the actual verdict**, because a report that
explained the Time Capsule rule over a Pokémon that does not satisfy it would be
claiming something the run never checked.

---

## 7. Demonstrating it without waiting for 1 in 8,192

Point `--target` at a deliberately common spread. `spc=10,def=10` is 1 in 256 and
exercises the identical code path — the same latch, the same match test, the same
stop, the same replay check, the same artefacts:

```
$ shiny_hunt --rom blue.gb --state route1.gbss --target 'spc=10,def=10' \
             --instances 12 --out /tmp/hit
attempts          25
encounters        642  (8.37/s measured)
emulated          804465 frames, 176x realtime across 12 instances
per instance      [2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3]
elapsed           76.7s

=== HIT ===
PIDGEY (internal #36, dex #16) level 2
  atk 15  def 10  spd  9  spc 10  hp 10  (total 54/75)
  found by instance 4 on attempt 1, encounter 11 of that run (seed 0xad4ba7b8706d727e)
  replayed from (base state, seed, attempt) in a fresh machine: SAME POKÉMON
```

642 encounters against an expected 256 — an ordinary draw at these odds. Then the recipe out of
its own `hit.txt`, run afterwards, which re-rolls that exact Pokémon in a fresh
machine:

```
$ shiny_hunt --rom blue.gb --state route1.gbss --target 'spc=10,def=10' \
             --instances 12 --run-length 32 --replay 4:1
instance 4 attempt 1 (seed 0xad4ba7b8706d727e), replayed in 8.9s:
  #0   RATTATA      lv3   atk 10  def  7  spd  6  spc  0  hp  4  (total 27/75)
  #1   PIDGEY       lv3   atk 15  def 15  spd 15  spc  4  hp 14  (total 63/75)
  …
  #10  PIDGEY       lv2   atk  6  def 13  spd  9  spc  5  hp  7  (total 40/75)
  #11  PIDGEY       lv2   atk 15  def 10  spd  9  spc 10  hp 10  (total 54/75)  <- MATCHES THE TARGET
```

Note the whole run replays, not just the winner: encounters #0 through #10 come
back byte for byte too. That is the check that a hit is a fact about the game and
not about the particular thread that happened to find it.

---

## 8. Sharp edges, all of them found the hard way

These cost real time on this branch. Every one is a fact about the cartridge or
the emulator, not about this code, so the next person to drive Gen 1 from outside
will want them.

* **`wCurOpponent` reads back `0` once a battle has begun.** The first version of
  the latch required `wEnemyMonSpecies == wCurOpponent` as an "independent
  witness" that the structure was populated. It never latched anything at all:
  every attempt was reported as a stall, and the hunt looked completely broken
  while being one comparison away from working. Generalise it — an independent
  witness has to be **observed** to still hold at the moment you read it, not
  assumed from where it was written.
* **There is no half-written window to guard against.** `InitWildBattle` is
  straight-line code: `ld [wIsInBattle], a` then `call LoadEnemyMonData`, with
  the encounter animation only afterwards. So a once-per-frame sampler can never
  see the flag up and the DVs stale. Worth knowing before inventing machinery to
  handle a race that does not exist.
* **A 24-frame hold is one *and a half* Gen 1 steps**, and the game commits to a
  step once it has begun — so a "walk one tile" press moves the player two tiles
  as often as one. A greedy walker aiming at an exact coordinate therefore
  oscillates 34 → 32 → 34 for ever and never lands on 33. Close the last tile
  with a ~10-frame press.
* **A house's front door is a map-**edge** warp.** Red's House 1F exits at
  `warp_event 2, 7` and `3, 7`, both on row 7 — the bottom row of the map. A Gen
  1 warp on a map edge fires when the player walks *into the edge*, not when they
  step onto the tile, so arriving at the door is not leaving through it. Press
  Down again once standing there.
* **Mum has something to say downstairs**, and a text box swallows every
  direction key. A walk that meets one looks exactly like a walk into a wall and
  never recovers. Press **B**, never A: both advance a box, but B can only ever
  *close* a menu, and an A one frame after the box closes opens the START menu,
  which then eats every direction key for the rest of the run.
* **The host's plugin frame counter is not machine state.** `Event::Frame {
  number }` keeps counting across `load_state`, and `PluginHost::reset` does not
  rewind it. Putting that number in a `Sighting` made every hit report
  "DID NOT REPRODUCE" — the replay produced a byte-identical Pokémon at a
  different absolute frame. `Sighting::frames_into_attempt` is relative to the
  last `arm()` for exactly this reason, and
  `the_same_encounter_at_a_different_absolute_frame_is_the_same_sighting` pins
  it.
* **A long press walks more than one tile, and the grass is only four deep.** A
  press is held 20–48 frames and a Gen 1 step is 16, so a long one moves two or
  three tiles. Uncapped, the walker paced 30 → 36 through a band of 32 → 35,
  spent most of its steps on ordinary ground where nothing is ever rolled, and
  **every run ended in a stall after about five encounters**. From the outside
  that looks like a bad encounter rate, not a walk in the wrong place. The press
  is now capped at the room actually left in the band, which costs no variation
  in the middle of one and all of it at the edges — mean run length 4.8 → 26.2 of
  32, measured.
* **The floor on a press is "a turn *and* a step", not "a step".** A press in a
  direction the player is not facing spends itself turning the sprite. A walker
  pacing a narrow band changes direction constantly, so a floor of 8 frames made
  roughly half of all presses move nothing.
* **Anchor the pacing band once per attempt, not per walk.** Fleeing leaves the
  player wherever the encounter happened; re-reading the centre afterwards lets
  the band *walk*, and over a run of thirty it wanders out of the grass.
* **A party that faints ends the run.** `wIsInBattle` reads `$FF` after a
  blackout and the game teleports the player to a Pokémon Center, where there is
  no grass. `--make-state` therefore hands the scout a level 50 Pokémon with
  Speed 250 — not a cheat, but what it takes to flee thirty battles in a row and
  survive them. The driver counts blackouts separately so a weak party is a
  reported cause rather than a mysteriously short run.
* **Do not try to teleport by editing `wCurMap`.** `LoadSAV` sets
  `BIT_NO_PREVIOUS_MAP` and `LoadMapHeader` then returns without loading the
  header, so the new map is read through the old map's data pointers and the game
  jumps into `rst $38` within a second. `--make-state` walks; the game loads
  every header itself.
* **A brand-new game cannot walk into grass** — the battle needs a Pokémon.
  `--make-state` writes one straight into WRAM, with the running game's own
  `wPlayerID`, because `CheckForDisobedience` treats a mismatched OT id as a
  *traded* Pokémon that a badgeless player is only obeyed by up to level 10.

### Never touching a real save

Every instance is built with `Gameboy::new(bytes, None)` — **no file path** — and
`src/mbc/battery.rs` only wires a battery when it is given one. A hunt therefore
opens, creates and writes nothing next to the cartridge, however many instances
it runs. `--make-state` does the same. The only files the tool ever writes are
the state named by `--make-state` and the three artefacts under `--out`.

---

## 9. Testing

* **`cargo test -p terminalgb-plugin-gen1-shinyhunt`** — 32 tests, no ROM, no
  emulator, in CI (`cargo test --release --workspace`; a bare `cargo test` at the
  repo root covers the `gameboy` package only, which is why no plugin crate's
  tests ran in CI until this branch). The interesting ones:
  * `the_shiny_target_is_the_trainer_panels_rule_and_nothing_else` enumerates all
    65,536 spreads against the single shared implementation.
  * `the_hp_dv_is_derived_so_odds_are_enumerated_not_multiplied` pins why the
    probability is counted rather than computed.
  * `instances_diverge_when_decorrelated_and_agree_when_they_are_not` runs the
    real driver loop over a fake machine whose rolls depend on input timing, and
    checks **both** directions.
  * `a_run_of_encounters_reaches_more_spreads_than_reloading_every_time` and
    `coverage_says_too_narrow_when_the_reachable_set_cannot_hold_a_hit` — the
    coverage half of §4, which divergence alone does not test.
  * `a_hunt_stops_on_a_hit_and_preserves_enough_to_reproduce_it`,
    `an_attempt_that_never_produces_an_encounter_is_a_stall_not_a_hit`,
    `an_unrecognised_cartridge_is_refused_rather_than_hunted_on`.
  * `the_same_encounter_at_a_different_absolute_frame_is_the_same_sighting` — the
    regression test for the bug in §8.
* **Against the cartridge**, by hand (no commercial ROM goes in CI):
  `--make-state`, then `--probe 6 --instances 12` (divergence *and* coverage *and*
  run health), then `--probe --no-decorrelate` for the control, then a
  contrived-target hunt with `--out`, then the `--replay` line its report prints.
  That sequence is §4 and §7 above and takes a few minutes.
* **`--diagnose`** walks one attempt with a running trace — position, battle
  flag, the enemy structure, each encounter of the run and where each flee left
  the player. It exists because the four causes of "every attempt stalled" are
  indistinguishable from the outside, and it is what found both walk bugs in §8.

## 10. Limitations, honestly

* **Red and Blue only.** The cartridge is identified by title *and* header global
  checksum, so Yellow, a translation, a randomiser or a hack is refused with an
  explanation rather than decoded wrongly. Yellow (`pret/pokeyellow`, same
  structures at different addresses) is a plausible extension; Gen 2
  (`pret/pokecrystal`, different structures, and *actual* shininess) is a
  different feature.
* **`grass` and `reset` only.** Fishing and surfing are wild encounters and reach
  the same `LoadEnemyMonData`, so `Watch::Encounter` covers them — but the walker
  only knows how to pace up and down, so a fishing hunt would want its own shape.
* **`--watch party:N` is implemented but not demonstrated on hardware.** Gift,
  starter, fossil and in-game-trade Pokémon roll their DVs in `_AddPartyMon`'s
  `; Not wild.` branch at the instant they become yours, which is what that mode
  watches for; it has unit coverage but no cartridge run in this branch.
* **The hunt does not catch anything.** It stops with the encounter on screen and
  hands you the save state. Catching is a decision — Ball choice, status, HP —
  and `plugins/trainer/src/capture.rs` already computes the odds for a human
  making it.
