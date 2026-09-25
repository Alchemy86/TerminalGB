# Game mechanics & hacking — Pokémon Generation 2

**What this repository has established about Pokémon Gold, how it was established, and — at
least as important — what it has not.**

This area is the younger sibling of [`docs/gen1/`](../gen1/README.md), and it is deliberately
shaped differently, because the knowledge behind it has a different shape. Gen 1 was explored
from many directions over months: a trainer panel, a battle trigger, a link partner, an agent,
a shiny hunter, an authored room. Every one of those probed a different part of the engine, so
the Gen 1 area can be a *map*.

Gen 2 was explored from exactly **one** direction — getting a Gold cartridge to the Time
Capsule and trading a Pokémon back to Red/Blue through it. So this area is a *narrow, deep
core with a large honest border*. Everything on the path from "boot Gold" to "a Pokémon
crossed the wire and Gold drew it in its shiny palette" is nailed down and cited. Everything
off that path — battle structures, the box, the bag, the map system, the RNG — is **not
established here at all**, and this area says so on every page rather than filling the gaps
with plausible-looking numbers.

That asymmetry is the useful thing to know before you read further. A plausible-looking Gen 2
address is worse than no address, because the failure mode is a save that looks right in a hex
dump and hangs, or a write that lands in the middle of somebody else's data and passes the
checksum.

---

## The pages

| Page | What it covers | Status |
|---|---|---|
| [Building the disassembly](building-the-disassembly.md) | The prerequisite for everything else: making a `pokegold` build that is byte-identical to the cartridge, so its symbol table is exact | **done here; one hour to repeat** |
| [Memory map](memory-map.md) | Every Gen 2 address this repository actually relies on, graded — plus a blunt list of what is missing | **drove a real cartridge end to end** |
| [The save file](the-save-file.md) | Three WRAM runs into three SRAM runs, the checksum, and why editing the map in a save does not move the player | **verified: the run map closes exactly on `sGameDataEnd`** |
| [Reading the screen](reading-the-screen.md) | `wTilemap` reads back as text here too — and the three ways Gen 2 differs from Gen 1 in doing it | **drove menus on the retail cartridge** |
| [The Time Capsule](time-capsule.md) | The only supported Gen 1 ↔ Gen 2 link: what it refuses, what it requires, and why the wire needs nothing added | **a Pokémon crossed it, both directions, and Gold saved the result** |
| [A Pokémon out of a Gold cartridge](marill.md) | Locating `BaseData`, `PokemonNames` and `PokemonPicPointers` by signature; Gen 2's picture compression; a species' cry and its battle entrance, both captured off the cartridge | **read out of the cartridge; 500 pictures agree** |
| [Sharp edges](sharp-edges.md) | The traps. Every one of these cost a run, and several cost a day | **found the hard way** |
| [Two consoles, one wire](../link-cable.md) | The cable that carries it, and the measured record of the run itself | **Gen 1 ↔ Gen 2 trade verified** |
| Authoring an area | Building a new room and walking into it — **scoped for Gen 2, not attempted.** The work, the unknowns and the cost estimate are in [the Gen 1 page's §7](../gen1/authoring-an-area.md#7-generation-2--the-work-not-a-guess), where they belong beside the mechanism they would be adapting | **not established** |

---

## Which cartridge

Everything here is **English Pokémon Gold**, and the addresses are exact for it because the
disassembly was built to match it byte for byte ([building the disassembly](building-the-disassembly.md)).

* **Silver** almost certainly shares this WRAM layout — it is the same build with a different
  `-D` — but **nobody has checked it here**, so no page claims it.
* **Crystal** is cited on these pages for *rules* (the shiny test, Hidden Power, the type
  table) and never for *addresses*. Crystal's WRAM moved. Do not carry a number from this area
  into a Crystal tool.
* **Any translation, hack or randomiser** is out of scope for the same reason it is in Gen 1:
  the layout is a property of the build, not of the game.

Gen 1 tooling here guards against this by re-deriving its write addresses out of the cartridge
image at attach time (`plugins/battle-control/src/wram.rs`). **Gen 2 has no equivalent**, which
is the single biggest structural gap in this area — see the grade table below.

---

## Standards for this area

The four rules in [the Gen 1 area's standards](../gen1/README.md#standards-for-this-area) apply
here unchanged: cite by symbol name; separate verified from believed; explain the mechanism;
never put game data in this repository. Two additions, both forced by how Gen 2 knowledge was
acquired.

### The grades, with one added and one missing

Gen 1 grades an address *symbol file* / *walked* / *read out of the cartridge*. Gen 2 uses
four labels, and the difference between the two lists is itself a finding:

* **Symbol file** — `pokegold` was built at RGBDS 1.0.3 and the ROM matched the disassembly's
  own `roms.sha1`, so the label's address is exact for that cartridge. This is where almost
  every address on these pages comes from. **Caveat that must travel with it:** the symbol
  file is a build artifact and is *not* checked into this repository. What is checked in is the
  transcription — a constant, in Rust, with its symbol name in a doc comment. So the grade
  really means "somebody built it once and wrote the number down with its name", and the way
  to audit it is to build it again.
* **Walked** — added up between two symbol-file addresses, and only usable if it *lands* on a
  third known one. The landing check must be written down beside the walk. There is one walk in
  this area and it is two bytes long.
* **Measured** — the address was read or written in a running cartridge and the game behaved as
  the symbol name says it should. This grade does not exist in the Gen 1 area, and it earns its
  place here because most of what Gen 2 work touched was *driven*, not just read: `wMapGroup`
  changing when a warp fired, `hSerialConnectionStatus` picking the correct seat, `wNextWarp`
  actually redirecting a staircase. It is a **confirmation on top of another grade, never a
  substitute for one** — a byte behaving plausibly is not proof it is the byte you named.
* **Read out of the cartridge** — a distinctive run of machine code located in the ROM image
  and the address taken from its operand bytes. **No Gen 2 address in this repository has this
  grade.** That is the missing one, and it matters: it is the only grade that needs no build,
  no transcription and no trust, and it is the grade that lets a tool *refuse* a cartridge it
  does not recognise instead of writing to the wrong place. Building it for Gen 2 is the
  highest-value piece of unglamorous work this area could gain.

### Say what is not established, in the place a reader would look for it

Every page here ends with, or contains, an explicit gap list. This is not modesty. Gen 1's
pages can afford to be silent about what they omit because the area covers enough that a
reader can tell; Gen 2's cannot, because a page about the Time Capsule that says nothing about
battles reads exactly like a page about a game whose battles are understood.

---

## What is *not* established about Gen 2 here

Named honestly, with what each would cost, so nobody re-derives the same list:

| Not established | What it would take |
|---|---|
| **Battle structures** — the Gen 2 equivalents of `wBattleMon` / `wEnemyMon`, and whatever byte the overworld loop polls to start a battle | The Gen 1 method transfers directly: find what the main loop reads, then grade the structures. Gen 1's [battle control](../gen1/battle-control.md) is the recipe; the derivation is a day, the plugin more. |
| **Storage and the bag** — boxes, the item list | Same shape as the party work already done. Cheap once the party struct is fully transcribed rather than partially. |
| **The map system** — map attributes, tileset pointers, block map buffer, collision, warp/bg-event/object tables | Scoped in detail, with the open questions and a cost range, in [the Gen 1 authoring page's §7](../gen1/authoring-an-area.md#7-generation-2--the-work-not-a-guess). A map is `(group, number)` in Gen 2, which alone invalidates a one-for-one translation. |
| **The RNG** — Gen 2's `Random` and whether it mixes `rDIV` the way Gen 1's does | Read the routine. Everything this repository says about reproducibility is a *Gen 1* claim and must not be assumed forward. |
| ~~**A signature-based address check**~~ — done for the species tables, in [a Pokémon out of a Gold cartridge](marill.md): `BaseData`, `PokemonNames` and `PokemonPicPointers` are located by signature and verified against all 500 pictures, and a cartridge that does not match is refused. **Still missing for the WRAM write addresses**, which is where it matters most | The species tables took a day. The write addresses need the same treatment and are the remaining half. |
| **A Gen 2 observer, trainer panel or battle-control plugin** | None exists. The [trainer plugin](../trainer.md) is Gen 1 only and refuses anything else by title *and* header checksum, which is the correct behaviour and also the reason nothing silently half-works. |
| **Silver, and Crystal addresses** | See "Which cartridge" above. |

---

## What this area is built on

* [`examples/gen2_timecapsule.rs`](../../examples/gen2_timecapsule.rs) — everything on these
  pages. Crafts a Gold save that can use the Time Capsule, drives a real cartridge to the
  receptionist, and runs the whole Gen 1 ↔ Gen 2 trade in one process with both cores on
  [`LinkCable`](../link-cable.md).
* [`examples/gen2_marill.rs`](../../examples/gen2_marill.rs) — the species tables, the picture
  decompressor, and the cry and entrance captures. The one Gen 2 code here that needs no
  transcribed address at all.
* [`plugins/trainer/src/derived.rs`](../../plugins/trainer/src/derived.rs) — the Gen 2 *rules*
  applied to Gen 1 data: the shiny test, Hidden Power, and an explicit list of what Gen 1
  genuinely does not have. One copy of the shiny rule, shared by everything that quotes it.
* [`docs/link-cable.md`](../link-cable.md) §8 — the cable-side record of the run, with the
  measured transfer counts and the screenshot evidence.
