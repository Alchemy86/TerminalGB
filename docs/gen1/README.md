# Game mechanics & hacking — Pokémon Generation 1

**How Pokémon Red and Blue keep their state, how to drive them from outside, and what it
cost us to find out.**

> **Generation 2 has its own area: [`docs/gen2/`](../gen2/README.md)** — Pokémon Gold, and
> deliberately much narrower. Everything on the path to the [Time Capsule](../gen2/time-capsule.md)
> is established and cited; everything off it is named as a gap rather than guessed at.

This area exists because the same handful of ideas keeps producing new tricks. Jumping
straight into a battle, reading dialogue back as text, answering the link cable as a second
Game Boy, watching a wild encounter's DVs the instant they are rolled — none of these were
planned features. Each one fell out of understanding a piece of the game's state and asking
"what else touches this?". So these pages are written to be *generative*: they explain the
mechanism, not just the recipe, and every one ends with what it suggests next.

It is also a working area, not a finished manual. Pages arrive as crews prove things.
Anything unproven says so.

---

## The pages

| Page | What it covers | Status |
|---|---|---|
| [**The memory atlas**](atlas/README.md) | The whole address space, region by region: what lives where, what is unused, and the evidence for every entry — indexed by address and by name. It lives at [AtlasGB](https://github.com/Alchemy86/AtlasGB) now; this page is how this repository consumes it and publishes the evidence back | **every byte of WRAM and HRAM accounted for; checked against a running cartridge** |
| [Memory map](memory-map.md) | Where the game keeps the party, the battle, the screen, the map and the dice — with the symbol name and the derivation for every address | **verified on the retail ROM** |
| [Authoring an area](authoring-an-area.md) | What a room is made of, how the engine loads one, and the recipe for building a new one in memory and walking into it through an ordinary door — with the ceilings, and Gen 2 scoped | **built and walked on the retail ROM** |
| [Battle control](battle-control.md) | Starting any battle on demand: wild, trainer, custom teams on both sides, and scripting one as an in-game cinematic | **verified on the retail ROM** |
| [The battle engine](battle-engine.md) | A model of the battle *rules* — damage, crits, accuracy, status, the Gen 1 bugs — held to the cartridge by a differential harness, plus agents and an evaluation protocol | **32 checkable observations against the retail ROM, zero disagreements** |
| [Catching](catching.md) | Is this encounter worth a ball: the capability's gate, wild-versus-trainer told apart from memory *and* from the screen, the catch odds held to the cartridge, and a target list derived from the fights ahead | **measured on the retail ROM: 251 throws, 61.0 % caught against 59.8 % predicted** |
| [A paper's Gen 1 claims, checked](paper-claims.md) | Eleven asserted mechanics from an outside state-space derivation, measured on the cartridge: the toxic formula, five duration counters, the screens, and the critical-hit rate | **measured on the retail ROM; most of the claims hold, and one of ours did not** |
| [Reading the screen](reading-the-screen.md) | Why `wTileMap` reads back as text, and why that is the right way to drive the game | **verified on the retail ROM** |
| [Driving the link cable](link-from-outside.md) | Being the console on the other end: the trade protocol, the role handshake, and how a battle can start down the same wire | **trade verified; link *battle* under investigation** |
| [Two consoles, one wire](../link-cable.md) | The other approach: relay bytes between two real cores instead of impersonating one | **Gen 1 ↔ Gen 1 and Gen 1 ↔ Gen 2 trades both verified** |
| [The Time Capsule](../gen2/time-capsule.md) | The one door to Generation 2: what Gold refuses, what it requires, and why the wire needs nothing added | **a Pokémon crossed it, both directions** |
| [Leaving the Cable Club](leaving-the-cable-club.md) | Why cancelling needs *both* players, why the Trade Center has no door, and the button that actually gets you out | **measured on the retail ROM** |
| [**Discoveries**](discoveries.md) | Every fault and quirk this project found in Pokémon Blue, with the reasoning trail for each — including the wrong turns, and including the entries where the wrong belief was ours. Slugs are a permanent contract so [AtlasGB](https://github.com/Alchemy86/AtlasGB) can link back to them | **measured on the retail ROM** |
| [Sharp edges](sharp-edges.md) | The traps. Every one of these cost somebody hours | **verified the hard way** |
| [Agent play](agent-play.md) | An AI agent that reads game state and presses buttons, with commentary for a stream | **plays the opening on the retail ROM** |
| [Shiny hunting](../shiny-hunt.md) | Hunting the DV spread that becomes shiny in Gen 2, across many headless emulators at once — including how to decorrelate machines that all start from the same save state | **hunts on the retail ROM** |

---

## Where a new finding belongs

Two investigations are live as this area is created, and each has a home waiting:

* **Why a link *battle* started against a trade-only virtual partner in the Colosseum.**
  → [`link-from-outside.md`](link-from-outside.md), section *"The Colosseum: a battle we did
  not agree to"*. That page already lays out the role handshake and where the game branches
  between BATTLE and TRADE; the finding belongs where the branch is described, not in a new
  page. If it turns out to be a *timing* fact rather than a protocol one, it also earns a line
  in [`sharp-edges.md`](sharp-edges.md).

* **An agent that reads game state and presses buttons.** Landed:
  → [`agent-play.md`](agent-play.md) answers the seven questions this section left it, and
  [`../claude-plays.md`](../claude-plays.md) is the full design, including the OBS recipe for
  streaming it. New *Gen 1* findings from agent work belong on this area's pages as usual —
  three already went into [`sharp-edges.md`](sharp-edges.md)'s territory and are listed at the
  bottom of `agent-play.md`.

Anything else: put it on the page whose *mechanism* it belongs to. A new page is warranted
when there is a mechanism nobody has written down yet, not when there is a new trick built on
an old one.

**A *fault* or a corrected belief goes on [`discoveries.md`](discoveries.md)** — the reasoning
trail, the wrong turns, the citation and the evidence — and the one-line warning form of it goes
on [`sharp-edges.md`](sharp-edges.md), grouped by what you were doing when it bit. The two are
deliberately different documents: one is how it was found, the other is how to avoid it. A
discovery entry's heading is a **permanent slug**, because AtlasGB links back to it.

---

## Standards for this area

This is public-facing, and its whole value is that a reader can trust it. Four rules.

**1. Cite [`pret/pokered`](https://github.com/pret/pokered) by symbol name, not by hex.**
`wCurOpponent` survives; `$D059` does not — it is wrong for Yellow, wrong for a translation,
and unsearchable. Write the symbol, then the address, then the file the symbol lives in:

> `wCurOpponent` (`$D059`, `ram/wram.asm`) — nonzero makes the overworld loop start a battle
> (`home/overworld.asm`, `.checkForOpponent`).

For a routine, name the label and the file: `LoadEnemyMonData`, `engine/battle/core.asm`.
That is what lets a reviewer check the claim in ten seconds instead of trusting it.

**2. Separate what is verified from what is believed.** Every address in
[`memory-map.md`](memory-map.md) carries how it was established. There are three grades and
they are not interchangeable:

* **Symbol file** — the disassembly was built at its pinned RGBDS version and the resulting
  ROMs matched `roms.sha1`, so a label's address is exact for that cartridge.
* **Walked** — derived by adding up `ram/wram.asm` between two symbol-file addresses. A walk
  is only usable if it *ends* on a second known symbol; that is the check, and it must be
  written down beside the walk.
* **Read out of the cartridge** — a distinctive run of machine code was located in the ROM
  image and the address read straight out of its operand bytes. This is the strongest grade
  because it needs no build, no symbol file and no arithmetic, and it is what
  `plugins/battle-control/src/wram.rs` does at attach time for every address it will *write* to.

If something is believed but not checked, say "believed" and say why nobody has checked it.
An unqualified claim that turns out to be wrong costs more than a gap.

**3. Explain the mechanism, so a reader can invent the next technique.** The captain is
collecting these because each one suggests the next: the battle trigger came from noticing
that the overworld loop polls a byte; the screen reader came from noticing that the font is
loaded so a tile id *is* a character. Both are properties of the engine, not tricks. So write
the property, then the trick, then what else the property reaches.

**4. Never put game data in this repository.** Facts about the game — addresses, structure
layouts, character maps, formats, constants — are ours to document and must be cited. The
game's *assets* are not ours at any price: no sprite data, no tile sheets, no text corpora,
no fixtures built from them, not even as a test hash. Read them out of the user's own
cartridge at run time. See the licence-boundary section in the repository's `CLAUDE.md`.

---

## What this area is built on

The techniques here are all implemented in the repository and all runnable:

* [`crates/terminalgb-plugin-api/`](../../crates/terminalgb-plugin-api/) — the plugin contract ([`docs/plugins.md`](../plugins.md)):
  a plugin declares what it may do and the host enforces it.
* [`plugins/battle-control/src/`](../../plugins/battle-control/src/) — battle control.
* [`crates/terminalgb-gen1-battle/`](../../crates/terminalgb-gen1-battle/) — the battle
  engine, the agents and the evaluation protocol ([`battle-engine.md`](battle-engine.md)).
* [`plugins/gen1-link/src/`](../../plugins/gen1-link/src/) — the link partner, the party encoding and the
  cartridge table reader.
* [`plugins/trainer/src/`](../../plugins/trainer/src/) — the read-only live view
  ([`docs/trainer.md`](../trainer.md)).
* [`plugins/agent/src/catch.rs`](../../plugins/agent/src/catch.rs) and
  [`battlekind.rs`](../../plugins/agent/src/battlekind.rs) — is this encounter worth a ball,
  and is it even a wild one ([`catching.md`](catching.md)).
* [`examples/gen1_battle.rs`](../../examples/gen1_battle.rs),
  [`examples/gen1_trade.rs`](../../examples/gen1_trade.rs),
  [`examples/warproom_probe.rs`](../../examples/warproom_probe.rs) — headless drivers that prove
  all of it against a real cartridge.
