# A paper's Generation 1 claims, checked against the cartridge

**The checked claims themselves now live on AtlasGB:**
[a paper's Gen 1 claims, checked](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/paper-claims.md).
Fifteen statements about Generation 1 mechanics from Appendix G of *The PokéAgent Challenge:
Competitive and Long-Context Learning at Scale* ([arXiv 2603.15563v2](https://arxiv.org/abs/2603.15563),
Karten, Grigsby, Upaa et al.), each measured on a retail Pokémon Blue cartridge and given a
verdict against `pret/pokered` — those are all facts about the game, so AtlasGB carries them.

What is left here is **this repository's own harness and its own bug** — the tooling that
produced the measurement, and the one place the measurement found a mistake in *our* battle
engine rather than in the paper.

---

## The harness

Everything on the AtlasGB page was produced by
[`examples/gen1_paper_claims.rs`](../../examples/gen1_paper_claims.rs), which starts real
battles on a real cartridge through [`plugins/battle-control`](../../plugins/battle-control/),
plays them by pressing buttons, and reads the game's own work RAM every frame. **It does not
read this project's battle model at all.** That is deliberate: a check that asks our model what
the cartridge does would only prove that we are self-consistent.

```bash
cargo run --release --features plugin-battle-control,plugin-trainer \
  --example gen1_paper_claims -- --rom ~/roms/blue.gb --trials 40
```

### Provenance

* **Cartridge**: `Pokemon - Blue Version (USA, Europe) (SGB Enhanced).gb`,
  sha1 `d7037c83e1ae5b39bde3c30787637ba1d4c48ce2`.
* **Disassembly**: [`pret/pokered`](https://github.com/pret/pokered) at
  `0cd19d3`, built with rgbds 1.0.3. Its `pokeblue.gbc` has **exactly that
  sha1** (`roms.sha1`, line 2), so the symbol file that supplied every address
  below describes the cartridge that was measured, byte for byte.
* **Addresses**: taken from that build's `pokeblue.sym` and cross-checked against
  [the atlas](../../third_party/atlasgb/README.md), where each one carries `rom,live` evidence:
  `wPlayerBattleStatus1..3` `$D062-$D064`, `wEnemyBattleStatus1..3`
  `$D067-$D069`, `wPlayerNumAttacksLeft` `$D06A`, `wPlayerConfusedCounter`
  `$D06B`, `wPlayerToxicCounter` `$D06C`, `wPlayerDisabledMove` `$D06D`,
  `wEnemyNumAttacksLeft` `$D06F`, `wEnemyConfusedCounter` `$D070`,
  `wEnemyToxicCounter` `$D071`, `wEnemyDisabledMove` `$D072`,
  `wCriticalHitOrOHKO` `$D05E`.
* **Battles**: [`examples/gen1_paper_claims.preset`](../../examples/gen1_paper_claims.preset).
  Each preset exists to put one counter into WRAM where it can be watched; none
  of them is a fair fight.

### The one methodological trap worth repeating

**An episode is keyed on the status FLAG, not on the counter.** A trapping move
that *misses* clears `USING_TRAPPING_MOVE` (`MoveHitTest`'s `.moveMissed`) and
leaves `wPlayerNumAttacksLeft` standing, so the next Wrap's fresh roll looks
exactly like a decrement of the old one. The first version of this harness
recorded a counter going `4 -> 2` in one step and would have reported a duration
the game has never rolled. Keyed on the flag, the same data reads correctly and
the early end is recorded as what it is — 73 of the 371 trapping episodes and 39
of the 161 thrash ones ended that way, and both figures are consistent with the
moves' own accuracy and with Gen 1's rule that hurting yourself in confusion
clears a thrash.

### Sample sizes

| claim | episodes / observations | battles |
|---|---:|---:|
| Toxic residual | 240 ticks (200 unclamped) | 40 |
| Toxic against a healing target | 118 ticks | 12 |
| Confusion | 97 | 40 |
| Disable | 136 | 40 |
| Thrash | 161 | 40 |
| Bide | 281 | 40 |
| Partial trapping | 371 | 40 |
| Reflect / Light Screen | 1 each (deterministic) | 1 each |
| Critical hits, no Focus Energy | 960 attacks | 24 |
| Critical hits, Focus Energy up | 588 attacks (72 discarded before it landed) | 24 |

Emulation is deterministic, so every battle idles a different number of frames
before it is armed: `Random_` folds `rDIV` in on each call, so *time* is the only
lever that decorrelates two runs from the same save state. This is the same
technique [`plugins/gen1-shinyhunt`](../shiny-hunt.md) uses, and for the same
reason.

---

## 6. What we got wrong ourselves

**We set out to check their claims and found our own bug. On this point the
paper was right and we were not.** That is the most valuable result this project
took from the exercise, and it is worth more than every row where the
measurement went the other way.

`Volatile::on_switch_out` in
[`crates/terminalgb-gen1-battle/src/state.rs`](../../crates/terminalgb-gen1-battle/src/state.rs)
carried this comment:

> *What `SwitchPlayerMon` clears — everything except the screens, which in Gen 1
> belong to the side rather than the Pokémon and survive a switch.*
>
> *Reflect and Light Screen surviving a switch is a Gen 1 behaviour, not an
> oversight here; `wPlayerBattleStatus3`'s screen bits are cleared only by the
> end of the battle or by Haze.*

Both sentences are wrong. AtlasGB's claim
[**Reflect and Light Screen are per-Pokémon volatiles**](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/paper-claims.md#3-reflect-and-light-screen-are-per-pokémon-volatiles)
carries the measurement that says so: `SendOutMon` zeroes `$D060`-`$D064`, and the bit is gone
the moment the reserve Pokémon comes out. It was a confident, well-commented, cited-sounding
note that nobody had asked the cartridge about — which is exactly the failure this repository's
standing rule about the cartridge being the oracle exists to catch, applied to a corner of the
model the differential harness had never reached.

### The fix, and its blast radius

`on_switch_out` now clears the whole struct, which is what `SendOutMon` does; the
unit test that pinned the old behaviour is rewritten as
`a_switch_clears_the_volatiles_including_the_screens`, and the cartridge-side
check is `a_screen_does_not_survive_a_switch` in the gate.

**What that touches, reasoned out rather than asserted:**

* **The emulator: nothing.** `terminalgb-gen1-battle` is a dependency-free crate
  that models the battle *rules*; no emulation path calls it. Every baseline in
  `testharness/` is byte-identical, and the in-repo test-ROM harness still reports
  46/47 rows matching.
* **The engine's own tests: 136 of 136 still pass**, with one renamed and its
  assertion tightened from four field checks to `v == Volatile::default()`.
* **The published agent evaluation in
  [`battle-engine.md`](battle-engine.md) §5: provably unaffected**, and this is
  the part that made the fix safe to land rather than hold. `build` in
  [`examples/gen1_battle_ai.rs`](../../examples/gen1_battle_ai.rs) selects a
  team's four moves by filtering on `m.power > 0` — in both the typed picks and
  the filler — so a zero-power move can never enter an evaluated team, and Reflect
  and Light Screen are zero-power. No episode in the 720 that produced those
  numbers could have had a screen up, so no episode could have reached the code
  that was wrong. The numbers do not need re-measuring.
* **Who else calls it**: one site, `sim.rs`'s switch handler. Nothing outside the
  crate can reach `Volatile` at all.
* **Anyone who had already built on the old behaviour** would have been building
  on a game that does not exist; there is no compatibility to preserve.

The alternative was to document the bug and keep shipping the wrong engine, which
is worse: a model that is knowingly wrong is worse than one that is unknowingly
wrong, because it has stopped being a claim about the cartridge.

**The general lesson, and it is not about Pokémon**: a claim in a paper you are
checking is also a claim about *you*. Looking at our own code because the paper
disagreed with it is worth more than every row where the measurement went the
other way.

---

## Re-running any of this

```bash
# Everything. ~10 minutes with these sample sizes.
cargo run --release --features plugin-battle-control,plugin-trainer \
  --example gen1_paper_claims -- --rom ~/roms/blue.gb --trials 40 --frames 9000

# One claim, with every episode printed.
cargo run --release --features plugin-battle-control,plugin-trainer \
  --example gen1_paper_claims -- --rom ~/roms/blue.gb --claim trapping --verbose

# `--trace` adds the frame of every counter change, which is how the
# missed-trapping-move behaviour above was found.
```

The **gate** is [`testharness/paperclaims.rs`](../../testharness/paperclaims.rs),
which runs the same measurement code and fails if the cartridge stops behaving
the way this page and AtlasGB's say it does:

```bash
POKE_ROM=~/roms/blue.gb cargo test --release \
  --features plugin-battle-control,plugin-trainer --test paperclaims
```

It skips with exit 0 and a printed reason without `POKE_ROM`, like
[`testharness/colosseum.rs`](../../testharness/colosseum.rs) and the atlas
harness — this repository ships no game data and the cartridge is yours. It is
**not in CI** for that reason; run it by hand when the harness or the AtlasGB
page changes.

Its assertions are chosen so they cannot flake: no roll outside the documented
range, the toxic formula on every unclamped tick, the screens cleared by a
switch, and the two critical-hit rates inside a 99.9 % interval of what
`CriticalHitTest` predicts. A 95 % interval would cry wolf one run in twenty and
would end up loosened until it meant nothing.

**Teeth proven**, and the demonstration is the same claim as above: narrowing the
gate's documented Disable range to the paper's **1–7** turns it red on exactly
that row and prints the observed range beside it —

```
[FAIL] disable_rolls_one_to_eight_turns_and_picks_a_slot_of_four
       — 21 episode(s), rolled 1-8 (documented 1-7)

20 check(s), 1 failure(s)
```

— while all nineteen other checks stay green.

**Build trap, not ours**: this repository's `[profile.release] panic = 'abort'`
plus its three lib crate-types occasionally hands a `harness = false` test the
wrong rlib, and it shows up here as `` the trait bound `BattlePlugin: Plugin` is
not satisfied ``. `touch src/lib.rs` and rebuild; see
[`docs/pitfalls/builds.md`](../pitfalls/builds.md).

---

## See also

* [AtlasGB: a paper's Gen 1 claims, checked](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/paper-claims.md) —
  the fifteen claims themselves, each with the cartridge fact and citation.
* [The Gen 1 battle engine](battle-engine.md) — the model, the cartridge oracle
  that holds it to the game, and the agents that play it.
* [Battle control](battle-control.md) — how a battle is started from outside.
* [The Gen 1 memory map](memory-map.md) and
  [the atlas](../../third_party/atlasgb/README.md) — where every address above
  comes from and what evidence it carries.
* [The measured Game Boy](../measured/README.md) — the same shape of page for
  the hardware itself: what the reference says, what we measured, and the test
  that pins it.
