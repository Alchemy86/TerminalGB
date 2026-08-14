# Agent play — reading state and pressing buttons

[← Gen 1 area](README.md)

> **Status: placeholder.** A crew is building an agent that reads game state and presses
> buttons. This page is the slot for its findings, with the questions it should answer and the
> pieces that already exist so it does not rebuild them.
>
> Written by the battle-control crew, who own this area's structure but not this page's
> content. Replace everything below the horizontal rule.

---

## What already exists, and should be linked rather than restated

An agent is a read-decide-press loop, and all three parts are in the tree.

**Read.** The [plugin contract](../plugins.md) gives capability-gated reads that cannot perturb
the run — `Machine::peek` deliberately does not tick the bus, and `peek_bank` reaches a ROM bank
the game has not mapped. The addresses are in [memory-map.md](memory-map.md); the *screen* is in
[reading the screen](reading-the-screen.md), and it is the more general sensor: `wTileMap` reads
back as the text a player can see, so an agent can act on what the game *said* rather than on a
structure it has to know about in advance.

**Decide.** Nothing here yet. This is the page's actual subject.

**Press.** `Capability::CONTROL_INPUT` and `Machine::set_button`
([`src/plugin/mod.rs`](../../src/plugin/mod.rs)) let a plugin hold and release the console's
buttons. It is deliberately a *separate* capability from `WRITE_MEMORY`: pressing a button is
something the game sanctions — every byte it changes, it changes through its own code — whereas
a poke is not. An agent should want the first and should not have to ask for the second.

The button-driving mechanics that already cost somebody time are in
[sharp edges](sharp-edges.md#driving-the-game): B closes menus and A opens them; Gen 1's menu
code does not see a press that arrives inside its own post-accept delay; a text box swallows
direction keys.

**A worked example of the whole loop** is the cinematic runner in
[`src/plugin/battle/script.rs`](../../src/plugin/battle/script.rs) — read the screen, decide
whether the condition holds, press or wait. It is a *script*, so its decision is fixed; an agent
replaces that one part.

---

## Questions worth answering here

These are the ones a script does not have to answer, so they are where this page's own words
should go.

1. **What is the goal, and how does the agent know it reached it?** A script ends when its steps
   run out. An agent needs a criterion.
2. **How does it know it is stuck?** The failure mode that matters is not crashing, it is
   pressing buttons forever against a screen that never changes. What is the detector — an
   unchanged tilemap for N frames? An unchanged `wPlayTimeSeconds`? A repeated screen hash?
3. **What does it do about it?** B is the universal escape (it closes menus and can never open
   one), but "press B until something changes" is a heuristic worth measuring rather than
   assuming.
4. **What does it read, and how often?** The trainer view samples every six emulated frames
   (~10 Hz) and only while its panel is visible, because the cost that matters is per-frame.
   360 `peek`s for a screen read is cheap but not free.
5. **Is it deterministic, and does that matter?** Emulation is; the agent's decisions may not be.
   If a run is meant to be reproducible, say what has to be pinned — see
   [battle control](battle-control.md#making-a-cinematic-reproducible) for the same argument
   applied to a clip.
6. **How is it visible?** `docs/plugins.md` §4 promises a plugin cannot be active and invisible,
   and an agent pressing buttons is the strongest case for that promise. Two bugs have already
   been found where a plugin's status row was clipped or truncated away.
7. **Where is the boundary with battle control?** Battle control *sets up* a scenario and can
   play a fixed script through it. An agent plays an open-ended one. If an agent wants a
   specific battle to practise against, it should arm a preset rather than write `wCurOpponent`
   itself.

## What it might unlock

Left here as prompts, since the whole point of this area is that each technique suggests the
next: a self-playing demo for a README; a soak test that plays for an hour and reports what it
got stuck on; a conformance check that no test ROM can express, because it is about a
*consumer* of emulated state rather than the state itself.
