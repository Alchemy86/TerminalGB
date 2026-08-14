# `example-tracer` — the plugin to copy

**A complete, working plugin in one file.** It watches one address in the emulated machine and
reports when the byte there changes — which is the first plugin most people actually want,
because *"what writes this?"* is the first question you ask about a game you are reverse
engineering.

```
plugins/example-tracer/
├── Cargo.toml     ← 9 lines of metadata and one dependency
└── src/lib.rs     ← the whole plugin, 284 lines including its tests
```

## Why it exists

Two reasons, and the second is the important one.

1. **It is the thing a stranger copies.** `cp -r plugins/example-tracer plugins/my-plugin`,
   change the name, and you have a plugin.
2. **It is the end-to-end proof that the documented install path works.** It is deliberately
   left *out* of the `standard-plugins` bundle, so it is built and attached exactly the way a
   third party's plugin would be — through a cargo feature and a name in `plugins = …`, with
   nothing in the emulator special-casing it. If the install path in
   [`docs/writing-a-plugin.md`](../../docs/writing-a-plugin.md) ever stops working, this is
   what notices.

## Run it

```bash
cargo build --release --features plugin-example-tracer
GB_PLUGINS=example-tracer cargo run --release --example terminal -- game.gb
```

The Status panel gains a row: `watching $D163`, then `$D163: 00->01 (1 changes)`.

## What it demonstrates

Everything a plugin needs, and nothing else:

| line of the contract | what `Tracer` does |
|---|---|
| `capabilities()` | asks for `OBSERVE` + `READ_MEMORY` — the least that will do the job. It *cannot* write the game, and that is the host refusing, not a comment promising. |
| `on_event(&mut self, &Event, &mut Machine)` | reads memory **in response to** an event; the `Machine` arrives alongside the event precisely so it can. |
| `status() -> String` | one short line for the status bar, called at most once per *displayed* frame. |
| `reset()` | drops observations when a save state is loaded — plugin state is not machine state and is not serialised. |
| bounded state | `HISTORY = 16`. A plugin runs inside the emulated frame, so unbounded growth is a leak with a 60 Hz clock on it. |
| `Event` is `#[non_exhaustive]` | it matches the one variant it cares about and ignores the rest, so a newer host sending unknown categories is not an error. |

## Tests, with no emulator at all

The three unit tests implement `MachineAccess` over a flat `[u8; 0x10000]` and drive
`PluginHost` directly — no ROM, no CPU, no display, no `gameboy` dependency:

```bash
cargo test -p gameboy-plugin-example-tracer
```

One of them asserts the negative case: a `poke` from a plugin that never declared
`WRITE_MEMORY` returns `false` and changes nothing. **Test the capability you did not ask
for**, not only the ones you did.

When a plugin genuinely needs a real cartridge — banked ROM tables, an MBC —
`gameboy::testing::TestMachine` is available as a *dev*-dependency behind the `testing`
feature. [`battle-control`](../battle-control/) does exactly that.

## Next

- **[Writing a plugin](../../docs/writing-a-plugin.md)** — zero to working, for someone who
  has never seen this codebase.
- **[The plugin contract](../../docs/plugins.md)** — the trust model, the version boundary,
  what a plugin may and may not do, and why there is one trait rather than two.
- **[`crates/gameboy-plugin-api`](../../crates/gameboy-plugin-api/)** — the dependency. It has
  none of its own and does not depend on the emulator.
