# The Pewter chain — moved to AgentGB

> **This page is now [`docs/agent/pewter-chain.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/pewter-chain.md) in
> [AgentGB](https://github.com/Alchemy86/AgentGB).** It moved on 2026-08-19.

It recorded **the fourteen links from a cold cartridge to Pewter City, the certification gate, and the corpus they produced** — a fact about what *our agent* did, which belongs with
the project that trains the agent. TerminalGB is the terminal tool that carries
the harness; AgentGB is the project that trains the thing. The rule, the test
that decides which repository a page belongs in, and the full list of what moved
is [`docs/where-things-live.md`](../where-things-live.md).

**This file is a pointer and nothing else** — it is kept so that existing links,
including the ones in this repository's own source comments, do not die. Do not
add content here; add it to the page in AgentGB.

## What stayed here

Everything this page *drives* is still in this repository, and this is the list
worth following instead of the page:

| here | what it is |
|---|---|
| [`examples/pewter_chain.rs`](../../examples/pewter_chain.rs) | the solver and the chain runner |
| [`src/scenario/`](../../src/scenario/) · [scenarios](../scenarios.md) · [writing a scenario](../writing-a-scenario.md) | the scenario system, its referee and its rules |
| [`scenarios/pewter/`](../../scenarios/pewter/) · [`scenarios/testrig/`](../../scenarios/testrig/) | the scenario files themselves |
| [the battle policy](battle-policy.md) | `plugins/agent/src/battle.rs` — fight, switch, item or run |
| [position tracks](track-format.md) · [`chainrender`](chainrender.md) · [the swarm's paths](path-arrows.md) · [the screen mosaic](screen-mosaic.md) | the film, track and render tooling |
| [`docs/gen1/`](../gen1/README.md) | what the cartridge does, which is a fact about the Game Boy rather than about anybody's agent |
