# Where things live, and why

> **Written on 2026-08-19, after the captain had to give the same correction
> twice.** On 2026-08-18 this repository took 50 commits and AgentGB took 4, and
> everything about what the agent had achieved — reaching Cerulean, beating
> Brock, catching Pokémon, the test rig, move learning — had been written in
> here. This page exists so the next crew does not do it a third time. The
> mirror of it is
> [`docs/where-things-live.md` in AgentGB](https://github.com/Alchemy86/AgentGB/tree/main/docs/where-things-live.md).

## The rule, in one line

**TerminalGB is the terminal tool that carries the harness. AgentGB is the home
of the teacher-and-student story.**

The captain's own wording, 2026-08-17: *"TerminalGB is not the place for it,
AgentGB is. Terminal is just the terminal tool."* This is an emulator that
happens to carry a harness; AgentGB is the project that trains the thing.

## The test that decides it

Ask **what the page is a fact about**:

| the page is a fact about… | it lives in | example |
|---|---|---|
| **the Game Boy**, or Pokémon Blue's own code | here | the capture routine, the PPU, `wIsInBattle`'s values, the ledge tables, everything in `docs/measured/` and `docs/pitfalls/` |
| **our tooling** — the emulator, the harness, the scenario system, the renderers | here | `docs/scenarios.md`, `docs/agent/track-format.md`, `chainrender.md`, `battle-policy.md`, the MCP server |
| **what our agent did** — a route it solved, a rate it achieved, a policy it follows, a failure it had | [AgentGB](https://github.com/Alchemy86/AgentGB) | the Pewter chain, the gym, Cerulean, the test rig's results, everything about the trained student |

A useful corollary: **if the number would change when the agent is retrained or
re-routed, it is AgentGB's. If it would still be true with no agent in the
world, it is ours.**

## Pages that are genuinely both

Some pages document a cartridge mechanic *and* our measured use of it. **Split
them; do not duplicate them.** The mechanics stay here, the results go there,
and each half links to the other at the seam.

The worked example is catching:

| half | where | what it holds |
|---|---|---|
| mechanics | [`docs/gen1/catching.md`](gen1/catching.md) | `ItemUseBall`, the disassembled odds and their 251-throw agreement with the cartridge, the wild/trainer screen signal and its nine-frame window, the two doors the ROM opens, the tables `gen1_tables.py` reads |
| results | [AgentGB `docs/agent/catching.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/catching.md) | the gate measured on the chain's own states, the target list the route needs, `catch::decide`'s ordering, the 24-throw policy run, and the 1,247 benches link 23 has landed |

**No claim is maintained in two places.** The 251-throw odds table is a
validation of the cartridge and appears only here; the 24-throw policy run is a
measurement of the agent and appears only there.

## What moved on 2026-08-19

**Moved to AgentGB**, with a pointer left at every old path so that existing
links — including this repository's own source comments — do not die:

| old path | now |
|---|---|
| `docs/agent/pewter-chain.md` | [AgentGB `docs/agent/pewter-chain.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/pewter-chain.md) |
| `docs/agent/brock.md` | [AgentGB `docs/agent/brock.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/brock.md) |
| `docs/agent/cerulean.md` | [AgentGB `docs/agent/cerulean.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/cerulean.md) |
| `docs/agent/test-rig.md` | [AgentGB `docs/agent/test-rig.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/test-rig.md) |
| `docs/agent/test-rig-results.md` | [AgentGB `docs/agent/test-rig-results.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/test-rig-results.md) |
| `docs/agent/move-learning-result.md` | [AgentGB `docs/agent/move-learning-result.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/move-learning-result.md) |
| the results half of `docs/gen1/catching.md` | [AgentGB `docs/agent/catching.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/catching.md) |
| `docs/agent/chain-stills/1`–`11`, all of `docs/agent/rig-stills/` | AgentGB `docs/agent/` — they are pictures of agent runs |

**Stayed here:**

- `docs/conformance-notes.md`, `docs/measured/`, `docs/pitfalls/` — emulator
  accuracy, the +427 Gambatte rows, the OAM DMA and prohibited-region fixes
- `tools/pewter-chain/`, `src/scenario/`, `chainrender`, `screen_mosaic`,
  `path_arrows`, and the pages that document them —
  [`track-format.md`](agent/track-format.md),
  [`chainrender.md`](agent/chainrender.md),
  [`path-arrows.md`](agent/path-arrows.md),
  [`screen-mosaic.md`](agent/screen-mosaic.md)
- [`docs/agent/battle-policy.md`](agent/battle-policy.md) — it documents
  `plugins/agent/src/battle.rs`, and a page that leaves the code it documents
  is a page that drifts from it
- the cartridge-mechanics half of `docs/gen1/`
- `docs/agent/chain-stills/12-certified-fastest.png` (illustrates
  `track-format.md`) and `camera-before-after.png` (illustrates the renderer's
  camera)

### Three consequences worth knowing before you write anything

1. **Three stills in `docs/gen1/discoveries.md` are now absolute links into
   AgentGB's raw content**, because the pictures are of agent runs and the
   findings are about the cartridge. If you move them back, move the findings
   with them.
2. **The stubs cannot preserve anchors.** Source comments and `AGENTS.md`
   entries that say "`docs/agent/pewter-chain.md` §8.10" still reach the stub,
   which names the new home; the section number is only resolvable at the far
   end. That is the accepted cost of the move.
3. **The agent tooling here is still worth documenting here, in detail.** The
   rule is not "write less about the agent"; it is "write about the *tool* here
   and about *what the agent did* there".
4. **A cross-repo link must use the `/tree/main/` URL form, never `/blob/`, and
   a new stub must be added to `EXEMPT` in
   [`docs/tools/docs_index.py`](tools/docs_index.py).** Both gates are in
   `make docs-check` and in CI, and both fired on the first attempt at this
   move: `codemap_links.py --check` rejects *any* `github.com/<owner>/<repo>/
   blob/…` link that is not ours (it cannot verify the line exists), and
   `docs_index.py --check` requires every page under `docs/` to be indexed or
   exempt — a stub is neither content nor an oversight, so it is exempt with
   its reason stated. An image is the exception and stays a
   `raw.githubusercontent.com` URL, because that is the only form that renders.
