# Scenarios

A **scenario** is a small JSON file: a goal, the rules it is judged under, and
the knowledge the agent may plan with. Full explanation:
[`docs/scenarios.md`](../docs/scenarios.md). Never written one?
[`docs/writing-a-scenario.md`](../docs/writing-a-scenario.md) builds
[`tutorial/town-map.json`](tutorial/town-map.json) from an empty file.

```sh
cargo run --release --example scenario -- check scenarios/leave-the-pokemon-center.json
```

`testrig/` — **test plans**, a different thing: a scenario says what one attempt
must achieve, a plan (`gbtestrig/1`) says which scenario to run, from which
starting states, how many times, and *who plays*. Checked in for the same reason
— a test is a specification — and described in
[`docs/agent/test-rig.md`](../docs/agent/test-rig.md).

```sh
cargo run --release --features scenario --example pewter_chain -- \
    ab --rom <blue.gb> --plan scenarios/testrig/starter-rival-battle.json
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

`tutorial/` — the one [`docs/writing-a-scenario.md`](../docs/writing-a-scenario.md)
builds, plus the two `*.script.jsonl` route scripts it plays with. A script is
one JSON reply per line, the same shape a model returns, and it goes through the
identical parse-clamp-execute path a live model's reply does — so a route that
works scripted works with a model behind it, and costs nothing to iterate on.

Recordings are not in here either — they go to the run's output directory
(`target/scenario/<name>/` by default). A recording carries the identity of the
cartridge and the start state it came from, so it is safe to hand to somebody
else; it is just not repository furniture.
