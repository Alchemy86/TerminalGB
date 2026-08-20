# A bench, and what it is worth — moved to AgentGB

> **This page is now [`docs/agent/catch-and-train.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/catch-and-train.md) in
> [AgentGB](https://github.com/Alchemy86/AgentGB).**

It records **what our solver did with a caught bench** — the certified rates for
links 23b and 26b, the gym at 2996 of 3000, and the Charmander line going from
0 of 775 against Misty to 794 of 795. That is a fact about what *our agent* did,
which belongs with the project that trains the agent. The rule and the test that
decides which repository a page belongs in is
[`docs/where-things-live.md`](../where-things-live.md).

**This file is a pointer and nothing else.** Do not add content here; add it to
the page in AgentGB.

## What stayed here

**The cartridge half is this repository's, and it is a full page rather than a
stub**: which species beats which gym and at what level, where each is
catchable, what a hunt costs in steps, and the three cartridge facts a
catch-and-train capability rests on — all of which would be true with no agent
in the world.

| here | what it is |
|---|---|
| [`docs/gen1/catch-and-train.md`](../gen1/catch-and-train.md) | **choosing a bench** — the type argument, the candidate table, the encounter tables, the party menu, and why an ELECTRIC bench loses a WATER gym |
| [`tools/gen1-tables/duel.py`](../../tools/gen1-tables/duel.py) | the chooser: every catchable candidate against a trainer's own party through Gen 1's `CalculateDamage` |
| [`examples/pewter_chain.rs`](../../examples/pewter_chain.rs) | `catch_wants`, `hunting`, `lead_with`, `ChainDriver::reorder_party`, `ChainDriver::weakening_move` |
| [`scenarios/pewter/23b-hunt-a-paras.json`](../../scenarios/pewter/23b-hunt-a-paras.json) · [`26b-lead-with-the-paras.json`](../../scenarios/pewter/26b-lead-with-the-paras.json) | the two new links |
| [`tools/pewter-chain/`](../../tools/pewter-chain/) | `build_pool.sh`, `replay_check.sh`, `split_by_line.py` — the certification tooling |
