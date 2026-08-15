# Scenarios

A **scenario** is a small JSON file: a goal, the rules it is judged under, and
the knowledge the agent may plan with. Full explanation:
[`docs/scenarios.md`](../docs/scenarios.md).

```sh
cargo run --release --example scenario -- check scenarios/leave-the-pokemon-center.json
```

## What is in here, and what is not

`*.json` — scenario files. Checked in, because a scenario is a specification.

`guidance/` — **not checked in** (`.gitignore`). A guidance file is what a run
*learned* by walking through doors on one particular save, and it is only true
of that save: the run recorded here found that the Viridian Pokémon Center's
door leads to Pallet Town, which is true of the save it was played from and of
no other. Learned knowledge belongs to whoever ran it. A scenario naming a
guidance file that does not exist yet is the normal beginning of one, and the
runner says so rather than complaining.

Recordings are not in here either — they go to the run's output directory
(`target/scenario/<name>/` by default). A recording carries the identity of the
cartridge and the start state it came from, so it is safe to hand to somebody
else; it is just not repository furniture.
