# A Generation 1 battle engine, and agents that play it

**A faithful model of Pokémon Red/Blue battle mechanics, a harness that holds it
to the real cartridge, agents that play it, and an evaluation protocol that makes
their scores mean something.**

> Status: **the model runs, and it has been checked against the retail cartridge
> — 32 checkable observations, zero disagreements.** Every rule is cited to
> `pret/pokered`. What is *not* modelled says so, in code and in every report.

---

## Why a battle

A whole playthrough is a sprawling, badly-specified problem: no clear state, no
clear objective, no way to say whether one run went better than another. A battle
is the opposite. It is **bounded** — at most nine choices a side, a state that
fits in a few hundred bytes, a definite outcome — and at seventy times real time
it is **repeatable** thousands of times a minute.

That is what makes it the one place in the game where an agent can be *measured*
rather than watched. Everything here follows from that.

---

## The three pieces

| | Where | What it is |
|---|---|---|
| **The engine** | [`crates/terminalgb-gen1-battle/`](../../crates/terminalgb-gen1-battle/) | A dependency-free model of the battle rules, cited line by line to the disassembly |
| **The oracle** | [`examples/gen1_battle_sim.rs`](../../examples/gen1_battle_sim.rs) | Runs real battles on the real cartridge and holds the model to what the game did |
| **The agents** | `crates/terminalgb-gen1-battle/src/ai/` + [`examples/gen1_battle_ai.rs`](../../examples/gen1_battle_ai.rs) | Random, greedy, expectimax, and a subprocess seam an LLM plugs into — all scored through one protocol |

```bash
# Hold the model to the cartridge. Non-zero exit on any disagreement.
cargo run --release --example gen1_battle_sim -- --rom ~/roms/blue.gb

# Measure an agent. Always prints the random baseline beside it.
cargo run --release --example gen1_battle_ai -- --rom ~/roms/blue.gb \
    --agent expectimax --depth 3 --episodes 40

# Drop in your own agent — any program that reads JSON lines and writes them.
cargo run --release --example gen1_battle_ai -- --rom ~/roms/blue.gb \
    --agent 'external:python3 my_adapter.py'
```

The engine's own 136 tests run under a plain `cargo test` with **no ROM on the
machine** — every one uses an invented two-species cartridge. The cartridge is
needed only for the oracle and for the evaluation teams.

---

## 1. The cartridge is the oracle

This is the rule the whole thing rests on. The engine is a transcription of
`pret/pokered`, and **a transcription can be wrong**. So the model is never tuned
until it looks right; it is diffed against the game and the difference is a
defect.

Writing that down is easy. What it actually cost, in order:

### Three table lengths, and none of them is 151

The data loader refused a perfectly good retail Blue image three times running,
and was wrong every time:

* **`PokedexOrder` is 190 entries, 39 of them zero.** Internal index numbers are
  not dense — the holes are the "MissingNo" slots — and the table is indexed by
  internal number, so it marks each hole with a `0` rather than omitting it.
* **`BaseStats` is 150 records, not 151.** Its own line reads
  `assert_table_length NUM_POKEMON - 1 ; discount Mew`; Mew lives at
  `MewBaseStats` in another bank and `GetMonHeader` special-cases it.
* **`MonsterNames` is 190 too**, for the same reason as `PokedexOrder` — and
  reading only 151 loses most species, because internal index 1 is RHYDON, not
  BULBASAUR.

None of these was reasoned out. Each one was a refusal message from a real
cartridge. They are named constants with citations now, precisely so nobody
re-derives `151` from the species count.

### A crit does not read the enemy's stat — it *recomputes* it

`GetEnemyMonStat` (`engine/battle/core.asm`), outside a link battle, does not
read `wEnemyMonAttack`. It calls `GetMonHeader` on `wEnemyMonSpecies` and
recomputes the stat from base stats, `wEnemyMonDVs` and `wEnemyMonLevel` **with
zero stat experience** — the source's own comment is *"this base address makes
CalcStat look in [wLoadedMonSpeedExp] for DVs"*, and that byte pair holds only
the DVs.

For an ordinary wild Pokémon that is the same number, which is why it hides. It
is **not** the same number for a trainer's Pokémon carrying stat experience.
`BattleMon::crit_stats` exists to express the asymmetry: the player's crit reads
the party struct, the enemy's is reconstructed.

### The truncating divide cannot be inverted by dividing back

The oracle's damage check works by asking what random multiplier an observed hit
implies. `RandomizeDamage` computes `damage * r / 255` for `r` in `217..=255` —
**and that division truncates**, so the map is many-to-one.

The first version divided back, and immediately produced a false accusation: a
predicted 6 observed as 5 inverts to `5 * 255 / 6 = 212`, outside the band, while
the forward computation `6 * 217 / 255 = 5` matches exactly. On small figures a
single output covers a dozen rolls, so the naive inverse systematically accuses
the model of being wrong about the hits where it is *least* likely to be. It is a
forward search over the band now.

**The general lesson, and it is not about Pokémon:** when a harness and a model
disagree, suspect the harness first — it is younger, less tested, and nobody has
run a hundred battles through it.

---

## 2. What the oracle can and cannot check

`Random_` (`engine/math/random.asm`) folds in `rDIV`, so the cartridge's dice
cannot be predicted from outside without also modelling the emulated clock. That
splits the checks in two, and the split is stated rather than glossed over.

**Exact** — anything with no die in it: stat recomputation from stat stages, badge
boosts, the burn and paralysis penalties and the order they compose in, poison
and burn residual damage. A disagreement here is a plain defect.

**Bounded** — damage. An observed hit pins the model's pre-random figure to a 15 %
window. That is weaker than exact and it is reported as such, but it is not weak:
on a fifty-point hit the resolution is about one point, and a systematically
biased model is caught by the *mean* implied roll even when every individual hit
passes. The report warns when that mean drifts off centre.

**Undetermined is never counted as agreement.** A harness that scored "could not
tell" as a pass would have no teeth.

### What is deliberately out of reach, and why

Two whole classes of check are refused rather than faked, both for the same
reason: `plugins/battle-control` builds these battles by patching `wEnemyMon`
*after* the engine has already filled `wEnemyMonUnmodified*` from the Pokémon it
generated. From the first stat change onward the cartridge is doing arithmetic on
numbers the harness cannot see.

* **Enemy-side stat recomputation** is not checkable at all in a plugin-built
  battle. Before this exclusion the harness reported four "disagreements", all on
  the enemy, all of the shape *"the cartridge behaved as if the unmodified stat
  were about 8 % lower than the one in `wEnemyMon`"*.
* **Any damage after a stat stage has moved** is undetermined for the same
  reason.

This is a narrowing of what is *determinable*, not a widening of a tolerance. The
damage band stays exactly `217..=255`.

### The measurement

On the retail Blue cartridge, seven purpose-built battles
([`examples/gen1_battle_sim.preset`](../../examples/gen1_battle_sim.preset)):

```
checked 32 observation(s): 2 exact, 30 within the damage band, 51 undetermined
implied damage rolls: mean 236.2 over 30 (the band is 217..=255, its middle is 236)
no disagreements
```

A mean of 236.2 against a band centre of 236 is the number that matters most:
every individual hit passing is consistent with a model that is wrong by a small
constant factor, and the mean is what would expose it.

**51 undetermined against 32 checked is the honest headline.** Most of what
happens in a Gen 1 battle is not observable from outside without addresses this
repository has not derived. Raising the checked fraction is the obvious next
piece of work; §7 says how.

### And here is what the oracle was NOT looking at

The harness above compares stats and damage **inside one battle**, and it never
switches. So the whole switch path — which volatiles survive a Pokémon leaving
the field — had no check on it at all, and the model was wrong about it:
`Volatile::on_switch_out` preserved Reflect and Light Screen, with a confident
comment saying that was a Gen 1 behaviour, and a unit test underneath pinning it.
It is not. `SendOutMon` zeroes all three `BattleStatus` bytes, and the cartridge
says so the moment you ask it.

That was found by checking somebody else's published claims and discovering they
were right and we were not — [`paper-claims.md`](paper-claims.md) §6 —
and it is fixed, with
[`testharness/paperclaims.rs`](../../testharness/paperclaims.rs) now driving a
real voluntary switch on the real cartridge to keep it fixed.

**The evaluation numbers in §5 are unaffected**, and that is checked rather than
assumed: `build` in `examples/gen1_battle_ai.rs` only ever picks moves with
`power > 0`, so Reflect and Light Screen cannot appear in an evaluated team.

---

## 3. Bugs are behaviour

A model that "fixes" Gen 1's bugs is a model of a game nobody played. These are
reproduced deliberately, each with a test that fails if someone tidies it away:

| Quirk | Cited to | What it does here |
|---|---|---|
| **Focus Energy quarters the crit rate** | `CriticalHitTest`: *"bug: … a shift to the right instead of left, resulting in 1/4 the usual crit chance"* | `crit_threshold` shifts right |
| **A crit uses BASE Speed**, not the in-battle stat | `ld a, [wMonHBaseSpeed]` | `base_speed` reads the species table |
| **A crit discards stat stages on both sides**, and doubles the level | `GetDamageVarsForPlayerAttack` | A crit through your own Swords Dance deals *less* — pinned by a test |
| **The 1/256 miss** | `MoveHitTest`: *"even the highest accuracy is still just a 255/256 chance"* | Accuracy is stored as `n * $FF / 100`, compared against a byte |
| **A resisted move can MISS** | `AdjustDamageForMoveType`: *"this only occurs if a move that would do 2 or 3 damage is 0.25x effective"* | Zero damage sets `wMoveMissed` |
| **Badge boosts compound** | `StatModifierUpEffect`: *"badge boosts get reapplied again to every stat"* | `apply_badge_boosts` is not idempotent |
| **A stat change wipes the burn penalty** | *"paralysis and burn penalties, as well as badge boosts are ignored"* | `apply_stat_change` recomputes from unmodified |
| **Hyper Beam skips its recharge on a KO** | `HYPER_BEAM_EFFECT` is not in `AlwaysHappenSideEffects`, and the KO check returns first | Modelled and tested both ways |
| **Full paralysis is 63/256**, not a quarter | `cp 25 percent`, with no `+ 1` | `percent()` is the cartridge's macro |
| **The turn a Pokémon wakes on is lost** | Both branches fall into `.sleepDone` | Tested |
| **Counter has negative priority** | `MainInBattleLoop`'s hard-coded order | Its user moves second |
| **The toxic counter multiplies Leech Seed** | *"hence the Leech Seed glitch"* | One shared counter |

Dice **consumption** is part of the model too. A turn that reaches the right
answer while drawing a different number of random bytes diverges on the *next*
turn, so `TurnLog::rolls` counts them and the tests assert them — a zero-power
move draws no crit die, a decided speed race draws no tie-break die, and the
damage roll is a rejection loop that can eat several.

---

## 4. What is NOT modelled, and how you find out

`effects::support` classifies every one of the 86 move effects, and an effect it
does not fully model carries the reason:

```rust
id::BIDE     => Support::DamageOnly("Bide's storing and release are not modelled"),
id::DISABLE  => Support::DamageOnly("Disable does not lock a move"),
id::MIMIC    => Support::DamageOnly("Mimic does not copy a move"),
```

A battle that touched one records it in `TurnLog::unmodelled`, an episode carries
it into `EpisodeResult::unmodelled`, and the report **refuses to present an
unqualified win rate**:

```
QUALIFIED: only 94.2% of episodes used exclusively modelled effects.
           win rate over the fully modelled episodes: 71.8%
```

A win rate measured through a silently-dropped Disable is not a win rate.

---

## 5. The evaluation protocol

A win rate is easy to fake. Each rule closes one specific way of doing it, and
the report prints enough for a reader to check rather than trust.

1. **The seed is a function of the episode index, not of the agent.** Two agents
   run on *identical* dice, so a difference between them is a difference in play
   — and an agent cannot be re-rolled until it wins.
2. **The opponent set is fixed and declared.** Picking easy opponents is
   legitimate; hiding it is not.
3. **Both sides of every matchup are played.** A team advantage cannot pass for
   an agent advantage.
4. **Unmodelled effects are counted** (§4).
5. **Illegal choices are counted, never silently improved.** An agent that
   returns something the engine would refuse gets the first legal action and the
   substitution is reported — otherwise the harness is playing.
6. **Dice provenance is printed.** A run under `Provenance::Foreknowledge` says
   *"NOT A COMPETITIVE SCORE"* in its own text.
7. **Exploits are surfaced, not hidden.** `EpisodeResult::exploits` names the Gen
   1 bugs a winning line actually used. An agent that wins by the 1/256 miss has
   produced an interesting result; burying it would make the number a lie.

The random baseline is always run and always printed beside the subject, with a
Wilson interval — a bare win rate over fifty episodes is not a measurement, and a
normal approximation degenerates at the ends.

### The measurement, and the result that spoiled the obvious story

720 episodes, `--seed 2654435769 --episodes 40 --level 30`:

| agent | vs random | vs greedy |
|---|---:|---:|
| random | 49.7 % | — |
| **greedy** (one ply) | **72.6 %** | — |
| expectimax, depth 3, 3 samples | 72.2 % | **46.5 %** |

Random against random lands on 49.7 %, which is the sanity check the whole
protocol needs. Both real agents beat it by a wide, non-overlapping margin.

**But expectimax loses its head-to-head against a one-ply damage-maximiser.** The
expectation going in was the opposite, and an earlier 216-episode run reading
52.3 % was noise — which is exactly why the intervals are printed. Three
candidate explanations, none yet measured: the min node models a perfectly
adversarial opponent and is over-pessimistic against a greedy one; `evaluate` is
crude and at depth 3 most leaves are still evaluated rather than decided; and
three dice samples is a noisy expectation on a turn whose damage roll alone has
39 branches.

So the interesting question is not "can a language model beat the search" — it is
whether anything beats **greedy**.

### One agent finding worth keeping

Greedy lost to random, 33 % against 42 %, on the first run. The cause was not the
policy: the team builder was handing every Pokémon **Explosion**, greedy saw the
largest damage number on the board and fainted itself. Two fixes, both real:
`ai::greedy::self_damage` prices what a move costs its own user, and the default
roster excludes `EXPLODE` and `OHKO` because a roster containing either measures
a different game. Greedy went 33 % → 72.6 %.

*A one-ply agent may be naive. It must not be suicidal.*

---

## 6. Dropping in your own agent

`ai::Agent` is one method — see `ai::View` for what an agent may see — and
`ai::ExternalAgent` puts it behind a subprocess: one JSON object per decision on
stdin, one back on stdout.

```json
{"schema":"gb1battle/1","turn":3,"side":"player",
 "me":{"species":25,"name":"PIKACHU","level":20,"hp":41,"max_hp":60,
       "status":"OK","types":["ELECTRIC"],
       "stages":{"attack":0,"defense":0,"speed":0,"special":0},
       "moves":[{"slot":0,"id":85,"name":"THUNDERBOLT","power":95,
                 "type":"ELECTRIC","accuracy":100,"pp":15,
                 "expected_damage":54,"effectiveness":200}]},
 "them":{…},
 "legal":[{"kind":"move","slot":0},{"kind":"switch","slot":1}]}
```

```json
{"kind":"move","slot":0}
```

A subprocess, not an HTTP client, for the same three reasons `plugins/agent`
chose one: it adds **no dependency** to a crate that has none, it is
language-agnostic, and it keeps credentials and rate limits out of the engine.

**What an adapter is deliberately not given:** no random bytes, no seed, and no
hidden member of the opposing party. An agent that could see the dice would win
by manipulation and its score would be worthless. Stat stages are reported the
way a *player* reads them (−6..+6), not the way the cartridge stores them (1..13)
— an adapter is being asked to play the game, not to model it.

**Failure is not a free pass.** A child that crashes, times out or answers
illegally does not get its turn replayed; the first legal action is substituted
and the substitution is counted in the report.

---

## 7. What is not done

Named, not hidden.

* **51 of 83 observations are undetermined.** Deriving `wPlayerMonUnmodified*`,
  `wPlayerSelectedMove` and `wDamage` from the cartridge — the way
  `plugins/battle-control/src/wram.rs` derives its addresses, by matching machine
  code — would make almost all of them exact. That is the single highest-value
  next piece of work.
* **The engine does not model Bide, Disable, Mimic, Metronome, Mirror Move,
  Transform, Conversion or Rage.** It says so, per battle, in the report.
* **Switching is barely exercised.** The default rosters are two Pokémon and
  `Greedy` does not consider switches at all, because a one-ply evaluation cannot
  see what a switch buys.
* **The opposing trainer AI is not modelled.** `SelectEnemyMove` has real
  behaviour; the evaluation uses `Greedy` or `Random` as the opponent instead, on
  the argument that an agent tuned against the cartridge's (weak) trainer AI
  would score well and generalise badly. That argument has not been tested.
* **Nothing here is in CI.** The engine's 136 unit tests are, via
  `cargo test -p terminalgb-gen1-battle`; the oracle and the evaluation need the
  player's own cartridge and are run by hand — the same rule
  `tools/sameboy-oracle/` and `tools/bgb-interop/` follow.
* **No LLM has been measured through it.** The seam exists, is tested against a
  real child process, and has never had a model on the far end.

---

## 8. Where this fits

`plugins/battle-control` *starts* battles on the real cartridge
([battle control](battle-control.md)). This *reasons* about them. They are
deliberately separate: a `Plugin` is handed a machine during a frame the emulator
has already decided to run, and an engine that searches thousands of hypothetical
battles has no machine and no frame — the same conclusion `docs/mcp.md` §7 and
`docs/shiny-hunt.md` reached from their own directions.

The engine crate is dependency-free and does **not** depend on `terminalgb`,
because the oracle and the CLI live there and the edge would otherwise be a
cycle. Same arrangement, same reason, as `terminalgb-plugin-api` and
`terminalgb-chiptune`.

**No game data is embedded.** Base stats, moves, the type chart and names are
read out of the player's own cartridge at run time; the *formats* are documented
facts and are written down with their citations. Every test in the engine crate
runs against an invented two-species cartridge, so `cargo test` needs no ROM.
