# Sample episodes

An **episode** is a named segment the runner resets and repeats — a start save
state, a success condition, a failure/timeout. See
[`docs/longplay.md` §5](../../docs/longplay.md).

These are templates. Each needs a `start_state` you produce yourself, because a
save state is made from **your** cartridge and is not ours to ship (the same
licence boundary as everywhere else in this repository: addresses and formats
are facts, game data is not).

Make one with the runner itself — every long run checkpoints, and a checkpoint
*is* a save state:

```bash
# play until you are where you want the episode to start…
cargo run --release --features longplay --example longplay -- \
    --rom ~/roms/blue.gb --dir target/lp/scout --until "map=0x33" …
# …then use its checkpoint as the start state
cp target/lp/scout/checkpoint.gbss states/forest-south.gbss
```

`examples/rom_script.rs` and `examples/gen1_battle.rs --save-state` also write
one, and `captures/` in any run directory is full of the moments things went
wrong — which are often exactly the segments worth turning into an episode.

Then:

```bash
cargo run --release --features longplay --example longplay -- \
    --rom ~/roms/blue.gb --dir target/lp/forest \
    --episode examples/episodes/viridian-forest.json --repeat 50 \
    --driver command --command tools/claude_plays/claude_api_adapter.py
```

Check the episode parses and could in principle succeed before spending
anything on it: any `--episode` is validated at load, and `--dry-run` stops
before the first frame.
