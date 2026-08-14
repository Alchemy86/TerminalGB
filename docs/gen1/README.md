# Game mechanics & hacking — Pokémon Generation 1

**How Pokémon Red and Blue keep their state, how to drive them from outside, and what it
cost us to find out.**

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
| [Memory map](memory-map.md) | Where the game keeps the party, the battle, the screen, the map and the dice — with the symbol name and the derivation for every address | **verified on the retail ROM** |
| [Battle control](battle-control.md) | Starting any battle on demand: wild, trainer, custom teams on both sides, and scripting one as an in-game cinematic | **verified on the retail ROM** |
| [Reading the screen](reading-the-screen.md) | Why `wTileMap` reads back as text, and why that is the right way to drive the game | **verified on the retail ROM** |
| [Driving the link cable](link-from-outside.md) | Being the console on the other end: the trade protocol, the role handshake, and how a battle can start down the same wire | **trade verified; link *battle* under investigation** |
| [Two consoles, one wire](../link-cable.md) | The other approach: relay bytes between two real cores instead of impersonating one. Includes the Gen 1 ↔ Gen 2 Time Capsule rules | **Gen 1 ↔ Gen 1 trade verified; Time Capsule needs a Gold save** |
| [Sharp edges](sharp-edges.md) | The traps. Every one of these cost somebody hours | **verified the hard way** |
| [Agent play](agent-play.md) | An AI agent that reads game state and presses buttons, with commentary for a stream | **plays the opening on the retail ROM** |

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
* [`plugins/gen1-link/src/`](../../plugins/gen1-link/src/) — the link partner, the party encoding and the
  cartridge table reader.
* [`plugins/trainer/src/`](../../plugins/trainer/src/) — the read-only live view
  ([`docs/trainer.md`](../trainer.md)).
* [`examples/gen1_battle.rs`](../../examples/gen1_battle.rs),
  [`examples/gen1_trade.rs`](../../examples/gen1_trade.rs) — headless drivers that prove all of
  it against a real cartridge.
