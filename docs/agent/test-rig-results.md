# The test rig's first result — the rival battle, three starters

> **This is a BATTLE comparison and it is not the move-learning result.** The
> rig's subject is level-up move learning
> ([`test-rig.md` §5a](test-rig.md#5a-measuring-move-learning--the-unit-is-a-prompt));
> this is what it measured while that prompt pool was still being built. It
> proved the instrument and produced two findings, both about the expert. **No
> number on this page belongs to a student or to move learning.**

What [the test rig](test-rig.md) measured the first time it was pointed at
something. **Plan:**
[`scenarios/testrig/starter-rival-battle.json`](../../scenarios/testrig/starter-rival-battle.json).

```sh
cargo run --release --features scenario --example pewter_chain -- \
    ab --rom "$ROM" --plan scenarios/testrig/starter-rival-battle.json \
       --runs 300 --workers 12
```

| | |
| --- | --- |
| cartridge | Pokémon Blue (USA/Europe, SGB Enhanced) |
| scenario | [`05-the-rival-battle`](../../scenarios/pewter/05-the-rival-battle.json) |
| entry pool | 700 states from link 04's exits — 234 / 233 / 233 by starter |
| attempts | 300 per arm per subject; **2,700 total** |
| phase spread | 3001 |
| split | **`n/a`** — no trained player took part |

**The student arm is empty, and that is not an oversight.** The trained network
lives in [AgentGB](https://github.com/Alchemy86/AgentGB) and must not be copied
here; the `command` arm is the seam it arrives through and is exercised below by
a real subprocess. See
[§5 of the rig page](test-rig.md#5-plugging-a-trained-player-in) for where the
rig belongs and what each repository still owes.

---

## The table

**Goal rate** — did the scenario's own goal condition fire:

| subject | teacher | mash-a | random |
| --- | ---: | ---: | ---: |
| CHARMANDER | **100.0 %** [98.7-100.0] | 0.0 % [0.0-1.3] | 0.0 % [0.0-1.3] |
| BULBASAUR | **100.0 %** [98.7-100.0] | 0.0 % [0.0-1.3] | 0.0 % [0.0-1.3] |
| SQUIRTLE | **100.0 %** [98.7-100.0] | 0.0 % [0.0-1.3] | 0.0 % [0.0-1.3] |

**Battle won** — of the attempts that fought, how many won every battle:

| subject | rival takes | teacher | HP left | mash-a | random |
| --- | --- | ---: | ---: | ---: | ---: |
| CHARMANDER | SQUIRTLE | **94.3 %** [91.1-96.4] | 43.6 % | never fought | never fought |
| BULBASAUR | CHARMANDER | **51.7 %** [46.0-57.3] | 18.3 % | never fought | never fought |
| SQUIRTLE | BULBASAUR | **86.0 %** [81.6-89.5] | 36.7 % | never fought | never fought |

**Gaps**, paired on the seed, teacher as reference: `+100.0 points
[+88.7,+111.3]` over 300 pairs against both screen-blind arms on every subject
(300 discordant one way, 0 the other, McNemar p < 0.0001). The battle gap has
**no pairs at all** and the report says so in those words rather than printing a
zero — the two arms never both fought on the same seed.

---

## What it found

### 1. The pass rate is blind to the fight underneath it

The expert passes **300 of 300 with every starter**, and loses the battle
**5.7 %, 48.3 % and 14.0 %** of the time depending on which one it took. Both
statements are true at once because link 05's goal is that the rival battle
*happened*: `HandlePlayerBlackOut` special-cases this fight (`cp OPP_RIVAL1` …
`cp OAKS_LAB / ret z`, whose own comment reads *"starter battle in oak's lab:
don't black out"*) and `OaksLabRivalEndBattleScript` sets the flag whoever won.

**A rig that reported only the goal would have said the three starters were
identical.** That is why the battle is a second outcome with its own rate,
interval and paired gap, rather than a footnote — and it is the general lesson:
when a scenario's goal is coarser than the thing you care about, the pass rate
will look flat and say nothing.

### 2. The floor is zero, and both screen-blind arms fail the same way

Neither `random` nor `mash-a` ever reached the battle. The endings histogram
says why, and it is not "lost a fight":

```
mash-a   endings: stuck 300
random   endings: out-of-calls 294, stuck 6
```

Neither can navigate to row 6 of the laboratory, so the encounter never fires.
That is worth having as a floor — **none of the expert's rate on this link is
available by accident** — and it is also a warning about the subject: link 05
bundles navigation with a fight, so a player that can battle but not walk scores
zero here for reasons that have nothing to do with battling.

### 3. Two of three battle rates reproduce a published figure. One does not

[`pewter-chain.md`](pewter-chain.md#the-rival-battle-is-not-a-coin-flip-and-the-odds-are-in-the-base-stats)
records these loss rates over 900 certified exits. Beside this run:

| player | published | measured here | agrees? |
| --- | ---: | ---: | :---: |
| SQUIRTLE | 14.7 % | **14.0 %** | yes |
| BULBASAUR | 54.0 % | **48.3 %** | close |
| CHARMANDER | 23.0 % | **5.7 %** | **no** |

**The likely cause is measured, not guessed: the expert fights differently now.**
A link-05 corpus run today reports **49 stat-lowering turns across 60 battles** —
the expert spends roughly one turn in eight on GROWL. That rule
([`docs/agent/battle-policy.md`](battle-policy.md)) landed with the shared battle
policy, *after* the certification the published table came from, along with the
crit-aware faint rule and the accuracy-stage scaling. Lowering the opponent's
Attack helps a glass cannon most, and Charmander is the glass cannon: 39 HP and
Defense 43 against Squirtle's Attack 48, with Speed 65 against 43 buying it the
extra turns to spend.

**This is not settled here.** Confirming it means re-certifying link 05 on the
current policy, which is the chain crew's to do and is exactly what
[`pewter-chain.md`](pewter-chain.md)'s own standing rule already requires when
shared driver code changes. What this run establishes is that the published
Charmander figure no longer describes the expert, and that the two figures that
do still agree are evidence the disagreement is real rather than a harness
difference.

### 4. The gaps cannot be separated, and the report says so

```
[goal] mash-a : CHARMANDER vs BULBASAUR — gaps +100.0 and +100.0,
  difference +0.0 (se 8.2, p=1.000)
  cannot be separated at this sample size — consistent with one gap
  and two different ceilings
```

Both screen-blind arms are at the floor on every subject, so the goal gap is the
maximum everywhere and there is nothing for the species test to find. The block
is still worth printing: it is the shape the answer will take when there **is** a
trained player, and it refuses to manufacture a difference.

**When there is one, remember what cannot cause a species difference.** The
level-up move decision is species-independent by construction:
[`docs/gen1/learning-moves.md`](../gen1/learning-moves.md) enumerates 840
distinct decisions across all 151 species, collapsing to 581 distinct
four-held-plus-offered pairs, and the number whose correct answer differs by
species is **zero**. So a
student that does better on one starter than another is not doing so because of
the move-learning decision, and the report must not let a reader infer that it
is.

---

## The picture

`abfilm` plays one attempt per arm from the **same entry state at the same
seed** and lays the panes out left to right:

```sh
cargo run --release --features scenario --example pewter_chain -- \
    abfilm --rom "$ROM" --plan scenarios/testrig/starter-rival-battle.json \
           --seed 3 --lead CHARMANDER --runs 600
```

Ten seconds of it settles what the table cannot: at the sampled frame below the
teacher has finished, `mash-a` is standing in the laboratory pressing A, and
`random` has stumbled into the battle and is sitting on the FIGHT menu. Those
are three completely different failures, and only one of them is about
battling.

The frames are written to `<out>/frames/%06d.png` with the `ffmpeg` line printed;
they are game art rendered from the user's own cartridge and are **not**
committed, in line with the licence boundary in `AGENTS.md`.

---

## Verification

* **The host-side battle summary reproduces the driver-side tally exactly.** On
  the same 60 seeds of the same link, `ab` reports 60 battles / 42 won / 18 lost
  / 28.9 % HP left and `corpus`'s `ChainDriver` tally reports "battles 60: won 42
  (70.0 %), lost 18 (30.0 %) … 28.9 % of max" — **identical on every field**.
  That is what says the new observer measures the same thing while being
  available to a player that is not the expert.
* **A subprocess arm really receives the screen.** The `frame_png` the `mash-a`
  arm was handed decodes as a 160×144, 4-shade PNG with 130 distinct raw bytes —
  a real picture, not the blank buffer a non-drawing machine would leave.
* **The species indices are read from the cartridge**, not recalled: 176
  CHARMANDER, 153 BULBASAUR, 177 SQUIRTLE, resolved through `MonsterNames`.
* **Independence**: every cell's attempts vary entry state, seed and phase
  offset together, and the pool (233-234 per subject) is close to the attempt
  count (300), so entry states repeat at most twice.

## What this run cannot tell you

Everything in [§7 of the rig page](test-rig.md#7-what-it-cannot-tell-you), and
in particular:

* **No trained network has been measured.** The split is `n/a`. The arms here
  are a ceiling and two floors, which is what a first use of an instrument
  should establish before anything is claimed about a player.
* **It says nothing about move learning**, which is the rig's actual subject.
  A battle is not a prompt; the two share a harness and nothing else.

## One thing it does establish about the rig itself

The same plan was run twice, on two different builds — once while the
`command`-arm pixel channel was being added — and the two runs are **identical
to the digit**: 283, 155 and 258 battles won on 300 attempts each, on every
subject. Emulation here is a pure function of (entry state, scenario, seed), and
that is the property every number this rig produces rests on.
