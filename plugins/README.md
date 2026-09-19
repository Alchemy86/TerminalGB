# Plugins

The emulator is the product. Everything in this directory is an optional extra, **none of it is
in a default build**, and the emulator builds and runs with all of it deleted — which CI checks
on every push rather than taking on trust.

Six of the seven are specific to Pokémon Red/Blue and come on together with
`--features gen1`; [`example-tracer/`](example-tracer/) is the generic one and the one to copy.

**Writing one: [`docs/writing-a-plugin.md`](../docs/writing-a-plugin.md).** Start by copying
[`example-tracer/`](example-tracer/).

Each has its own README — what it does, its history, how it is tested, and the traps it cost
someone to find.

| directory | you type | feature | what it does | capabilities |
|---|---|---|---|---|
| [`example-tracer/`](example-tracer/README.md) | `example-tracer` | `plugin-example-tracer` | watches one address and reports what changes it — **the one to copy** | observe + read |
| [`trainer/`](trainer/README.md) | `trainer` | `plugin-trainer` | live Pokémon R/B party, badges, catch chance, sprites | observe + read |
| [`gen1-link/`](gen1-link/README.md) | `gen1-link` | `plugin-gen1-link` | a virtual link-cable partner: real trades and Colosseum battles | observe + read + link |
| [`battle-control/`](battle-control/README.md) | `gen1-battle` | `plugin-battle-control` | start any Gen 1 battle from a preset, and script it | observe + read + write + input |
| [`agent/`](agent/README.md) | `agent` | `plugin-agent` | an AI that plays the game, under a budget — [full reference](../docs/agent-reference.md), [as an AI environment](../docs/agent-environment.md) | observe + read + input |
| [`gen1-shinyhunt/`](gen1-shinyhunt/README.md) | `gen1-shinyhunt` | `plugin-gen1-shinyhunt` | hunt a Gen 1 DV spread across many headless emulators at once | observe + read |
| [`gen1-lobby/`](gen1-lobby/) | `gen1-lobby` | `plugin-gen1-lobby` | a shared Cable Club room several emulators walk around in together — [the World Trade Centre](../docs/worldlobby.md) | observe + read + write |

Note the middle column: **`battle-control` is the crate, `gen1-battle` is the name you put in
`plugins = …`.** The crate is named for what it does to any game; the plugin is named for the
game it does it to.

The contract they all implement is
[`crates/terminalgb-plugin-api`](../crates/terminalgb-plugin-api/README.md),
which has **no dependencies** and does not depend on the emulator. That direction is what lets
the emulator depend on plugin crates (its frontend draws their panels) without a dependency
cycle.

## Building with and without them

```sh
cargo build                                          # the emulator — NO plugins
cargo build --features gen1                          # + everything Pokémon, all six
cargo build --features plugin-trainer                # pick and choose
cargo build --features plugin-example-tracer         # the generic example
make no-plugins                                      # proves none of them is in the graph
```

`standard-plugins` is the six-plugin bundle, and `gen1` is that bundle plus the rest of the
Generation 1 layer (the battle simulator, the trade portal, the agent and scenario runners).
Neither is in `default`: a plain `cargo build` is a Game Boy emulator and nothing else.

## Separate repositories

These are **folders in this repository, not separate repositories yet**, and that is
deliberate. They already have the boundary that matters — their own crate, their own tests,
and a dependency on the published contract rather than on emulator internals — so splitting one
out later is a `git filter-repo` and a version bump in one `Cargo.toml`, not a redesign. Doing
it now would buy nothing and cost every cross-cutting change four pull requests.

The signal to split one out is when it wants its own release cadence or its own maintainer.
