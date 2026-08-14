# `gameboy-plugin-api` — the plugin contract

**Everything a plugin needs, and nothing from the emulator's internals.** This crate has
**no dependencies at all**, so writing a plugin costs you one `Cargo.toml` line and no build
system archaeology.

```toml
[dependencies]
gameboy-plugin-api = "0.1"
```

```rust
use gameboy_plugin_api::{Capability, Event, Machine, Plugin};

struct FrameCounter { frames: u64 }

impl Plugin for FrameCounter {
    fn name(&self) -> &str { "frame-counter" }
    fn capabilities(&self) -> Capability { Capability::OBSERVE }
    fn status(&self) -> String { format!("{} frames", self.frames) }
    fn on_event(&mut self, event: &Event, _m: &mut Machine) {
        if let Event::Frame { .. } = event { self.frames += 1; }
    }
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}
```

## Why this is a separate crate

**A plugin crate depends on this contract, never on the emulator.** That direction is what lets
the emulator depend on plugin crates — its frontend draws their panels — while keeping the
dependency graph acyclic. It is also what makes a plugin publishable from someone else's
repository: you are not compiling an emulator to write one.

It is why `cargo tree --no-default-features` shows two entries rather than one. That is not a
loosening of the "one third-party dependency" claim: this crate is first-party, in this
repository, and has no dependencies of its own, so an embedder gains no third-party code from
it. CI enforces exactly that wording.

## What a plugin *is*

An **in-process Rust object implementing `Plugin`**. Not a dynamic library, not a script
engine, not a marketplace. That is the honest choice for this project's constraints: it works
identically on `wasm32` (no sockets, no `dlopen`), it costs a null check per frame when nothing
is attached, and Rust has no stable ABI — a `dlopen`-style plugin would have to be `extern "C"`
end to end and would still break on a compiler upgrade.

**Be honest about the consequence: you cannot write a plugin in Python today.** The *messages*
are wire-shaped anyway — every `Event` serialises to the same NDJSON the emulator's telemetry
uses (`Event::write_json`), and every reply is a small value rather than a borrowed pointer —
so an external-process transport is an additive follow-up that does not reshape the trait.

## Capabilities are an interlock, not a sandbox

| capability | what it grants |
|---|---|
| `OBSERVE` | receive `Event`s (frames, serial transfers), look at the picture (`Machine::frame_rgba`) |
| `READ_MEMORY` | `Machine::peek`, `peek_range`, `peek_bank` |
| `WRITE_MEMORY` | `Machine::poke` — **can corrupt saves** |
| `SERIAL_LINK` | be the console on the other end of the link cable (`Plugin::on_serial`) |
| `CONTROL_INPUT` | drive the buttons — `Machine::set_button` (raw latch) or `Machine::press` (scheduled in frames) |

Declaration is **enforced**: `Machine` checks the calling plugin's declared capabilities and
returns `None` / `false` rather than trusting it. A plugin that never declared `WRITE_MEMORY`
*cannot* write.

**But read the trust model plainly.** A plugin is native Rust in the same address space as the
emulator; it could call `std::process::exit` and nothing here would stop it. Capabilities are
an interlock on the *emulated machine* — so a read-only plugin provably cannot write the game,
a plugin that starts pressing buttons is marked the moment it does, and *"which plugin corrupted
my save?"* has an answer. They are not protection from a hostile plugin. Install plugins the way
you install any other Rust dependency: by reading them.

## Rules the host keeps

- **Nothing loads implicitly.** `PluginHost` starts empty. A plugin is registered from explicit
  configuration (`plugins = …`, `GB_PLUGINS`), never by scanning a directory.
- **A plugin cannot be active and invisible.** Attached plugins get a row in the terminal
  Status panel; `WRITE_MEMORY` and `CONTROL_INPUT` holders are flagged there.
- **One link partner.** A cable has two ends, so a second `SERIAL_LINK` registration is
  rejected.
- **A panicking plugin is switched off, not fatal** (with an honest caveat about
  `panic = 'abort'` in the crate docs).
- **Plugin state is not machine state** and is not serialised. Loading a save state *resets*
  attached plugins rather than silently desyncing a half-finished trade.

## Compatibility

`API_VERSION` is the revision of this contract, declared implicitly through
`Plugin::api_version` — whose default body compiles the constant *from the copy of this crate
the plugin linked* — and checked by `PluginHost::register`.

Read that honestly: while plugins are linked at build time, Cargo has already made a mismatch
impossible, because two incompatible versions of this crate cannot satisfy one dependency
graph. The check states how compatibility is declared, costs one `u32` comparison, and becomes
load-bearing the day anything is loaded at run time. It is not pretending to catch a bug you
can hit today.

What actually protects a third-party plugin across our refactors is ordinary semver plus two
deliberate choices: **every `Plugin` method except `name`, `capabilities` and the two `as_any`
hooks has a default body**, and **`Event` is `#[non_exhaustive]`**. So events and callbacks can
be added without breaking you.

## Next

- **[`plugins/example-tracer`](../../plugins/example-tracer/)** — the same idea with tests and
  a README. Copy that directory to start.
- **[Writing a plugin](../../docs/writing-a-plugin.md)** — zero to working.
- **[The full contract](../../docs/plugins.md)** — including why runtime loading was rejected.
