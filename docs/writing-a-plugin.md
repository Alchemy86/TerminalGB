# Writing a plugin

> For someone who has never seen this codebase. Zero to a working plugin.
> Design rationale and the plugins we ship: [plugins.md](plugins.md) ·
> [🗺️ Code map](codemap.md)

A plugin can watch the emulated Game Boy, read its memory, press its buttons, and *be the
console on the other end of the link cable*. Ours use all of that: a live Pokémon party panel,
a virtual trade partner, an AI that plays the game.

You do not need to understand the emulator to write one. You need one dependency and one trait.

---

## 1. The shortest complete plugin

```toml
# Cargo.toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2021"

[dependencies]
terminalgb-plugin-api = "0.1"
```

```rust
use terminalgb_plugin_api::{Capability, Event, Machine, Plugin};

pub const NAME: &str = "frame-counter";

pub struct FrameCounter {
    frames: u64,
}

impl Plugin for FrameCounter {
    fn name(&self) -> &str { NAME }

    /// Everything you may do, declared up front. The host enforces this.
    fn capabilities(&self) -> Capability { Capability::OBSERVE }

    /// One line for the status bar.
    fn status(&self) -> String { format!("{} frames", self.frames) }

    fn on_event(&mut self, event: &Event, _machine: &mut Machine) {
        if let Event::Frame { .. } = event {
            self.frames += 1;
        }
    }

    // Boilerplate: how a frontend recovers your concrete type to render it.
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}
```

That is a complete plugin. Everything else on [`Plugin`] has a default.

**The real starting point is [`plugins/example-tracer/`](../plugins/example-tracer/)** — the
same idea, but it reads memory, keeps bounded history, handles a save-state load, and is
tested with no emulator. Copy that directory.

```
cp -r plugins/example-tracer plugins/my-plugin
```

---

## 2. What you can do

Declare only what you need. Each capability is checked on every call: an undeclared operation
returns `None`/`false` and does nothing, so a read-only plugin *provably* cannot write.

| Capability | Gives you | Watch out |
|---|---|---|
| `OBSERVE` | `on_event` (frames, serial bytes), `Machine::frame_rgba`, `frame_png` | |
| `READ_MEMORY` | `peek`, `peek_range`, `peek_into`, `peek_u16`, `peek_be16`, `peek_bank` | reads do not tick the bus, so observing cannot change timing |
| `WRITE_MEMORY` | `poke` | **can corrupt the player's save** |
| `CONTROL_INPUT` | `set_button` (raw latch), `press`/`tap` (scheduled in frames) | the user is holding the same buttons |
| `SERIAL_LINK` | `on_serial` — be the other Game Boy | only one plugin may hold it |

Combine with `|`:

```rust
fn capabilities(&self) -> Capability {
    Capability::OBSERVE | Capability::READ_MEMORY
}
```

Three things worth knowing early:

- **`peek_bank(bank, addr)` reads a numbered ROM bank whatever the game has mapped**, without
  touching the MBC registers. Cartridge tables live in banks the game maps only while it is
  using them; without this you would read whatever happened to be mapped.
- **`press(key, hold, gap)` schedules a press in frames** rather than holding a latch. A frame
  is the granularity games sample input at, so this is the level to work at unless you are
  genuinely thinking frame by frame.
- **`report_fault(reason)` needs no capability.** It stops the emulator writing the battery
  save for the rest of the session. It is ungated deliberately: it only takes a right away, and
  a plugin that knows a specific game often spots a crash before any CPU-level detector.

## 3. What you may *not* do — the trust model, stated

**Plugins are trusted code that the user chose to compile in.** A plugin is a native Rust crate
in the emulator's own process; it can open files, spawn processes and call `std::process::exit`,
and nothing here will stop it.

So be clear about what capabilities are: **an interlock on the emulated machine, not a sandbox.**
They exist so that

- a plugin's declared intent is checked against what it actually does,
- the user can see the difference in the status bar (`!` may write, `!!` has written, `>` may
  press buttons, `>>` has), and
- "which plugin corrupted my save?" has an answer.

They do not protect you from a hostile plugin. Install one the way you install any Rust
dependency: by reading it. We chose this because plugins here are compiled in from source by
the person running them — the threat model is *mistakes*, not malice, and capabilities catch
mistakes. Sandboxing would mean a wasm or subprocess boundary and a serialised protocol on the
hot path; the [`Event`] encoding is already wire-shaped so that stays possible later.

## 4. Errors: a failing plugin must not take the emulator down

Every call into a plugin runs under `catch_unwind`. A plugin that panics is **quarantined** —
switched off, never called again, marked `x` in the status bar, and reported once. The
emulator and every other plugin keep running. A link partner that panics becomes an unplugged
cable, which is a state the game already handles.

```
exploderx: stopped after an internal error
```

**The honest caveat.** That works in any build that unwinds. This repository's *release*
profile sets `panic = 'abort'` (a size and speed decision that predates plugins), and under
abort no `catch_unwind` in Rust catches anything — a panicking plugin takes the process with
it. So: quarantine is real in dev and test builds and in any release profile you configure to
unwind; in the shipped release build, do not panic. Prefer returning a refusal and going inert
(see `Plugin::status`) over `unwrap()`.

Errors you *expect* — an unrecognised cartridge, a missing config file — should not be panics
at all. The convention ours follow is to attach anyway and report through `status()`, so the
user sees "inactive: POKEMON YELLOW is not supported" instead of a plugin that silently does
nothing.

## 5. Compatibility across our refactors

`terminalgb-plugin-api` is versioned with ordinary semver, and [`API_VERSION`] names the revision
of the contract. `Plugin::api_version` has a default body that compiles that constant *from the
copy of the crate you linked*, and `PluginHost::register` refuses a mismatch:

```
plugin foo: built against plugin API v1, but this emulator implements v2 —
rebuild the plugin against terminalgb-plugin-api v2
```

**Do not override `api_version`.** Overriding it is how you lie about compatibility.

Read the guarantee honestly: with plugins linked at build time — the only supported way today —
Cargo has already made a mismatch impossible, because two incompatible versions of the API
crate cannot satisfy one dependency graph. The check is how compatibility is *declared*, it
costs one comparison at registration, and it is the gate that becomes load-bearing if anything
is ever loaded at runtime.

What actually protects you across our refactors is two deliberate choices:

- every `Plugin` method except `name`, `capabilities` and the two `as_any` hooks **has a default
  body**, so we can add callbacks without breaking you;
- `Event` is `#[non_exhaustive]`, so we can add event categories. **Always include a `_ => {}`
  arm** when you match on it.

## 6. Testing without an emulator

Implement `MachineAccess` over anything — an array is fine — and drive `PluginHost` yourself.
No ROM, no CPU, no display. `plugins/example-tracer/src/lib.rs` has a complete fake in ~40
lines, and `crates/terminalgb-plugin-api/src/tests.rs` has a richer one.

```rust
let mut host = PluginHost::new();
host.register(Box::new(MyPlugin::new())).unwrap();
let mut machine = FakeMachine::new();
machine.mem[0xC123] = 0x10;
host.on_frame(&mut machine);
assert_eq!(host.statuses()[0].detail, "watching $C123");
```

When you genuinely need a real cartridge — banked ROM tables, cartridge RAM, an MBC that
actually switches banks — use the emulator as a **dev**-dependency:

```toml
[dev-dependencies]
terminalgb = { version = "0.2", default-features = false, features = ["testing"] }
```

```rust
use terminalgb::testing::TestMachine;
let mut machine = TestMachine::new(my_rom)?;
```

`TestMachine` is a real machine exposed only as the `MachineAccess` your plugin already talks
to, plus `bus_read`/`bus_write`/`advance_input` for playing the part of the game. It hands out
no emulator internals: a plugin that cannot be tested through `MachineAccess` is reaching for
something the contract does not offer, and a test is a good place to find that out.

A dev-dependency is also the *only* direction Cargo permits, because the emulator depends on
the plugin crates it ships — see §8.

## 7. Installing a plugin

Two lines, then a name in config. There is no package manager and no plugin directory to drop
files into; installing is a rebuild, deliberately (§8).

**1. Add the dependency and a feature** in the emulator's `Cargo.toml`:

```toml
[features]
plugin-my-thing = ["my-plugin"]

[dependencies]
my-plugin = { path = "plugins/my-plugin", optional = true }
```

**2. Add one arm** to `attach_named` in [`src/screen/tui.rs`](../src/screen/tui.rs) — this is
the whole integration, and it is what every one of ours looks like:

```rust
#[cfg(feature = "plugin-my-thing")]
if name == my_plugin::NAME {
    return Some(match gb.add_plugin(Box::new(my_plugin::MyPlugin::new())) {
        Ok(()) => "plugin my-thing attached".to_string(),
        Err(e) => format!("plugin my-thing: {e}"),
    });
}
```

Optionally add a row to `REGISTRY` beside it, so that a build *without* your feature says
"plugin my-thing is not built into this binary — rebuild with --features plugin-my-thing"
rather than "unknown plugin".

**3. Build it in, and name it** in `~/.config/terminalgb/config.toml`:

```toml
plugins = my-thing
```

```
cargo build --release --features plugin-my-thing
```

`GB_PLUGINS=my-thing` overrides the config file for one run. **Nothing loads implicitly** — a
plugin runs because you named it, never because a file was found somewhere.

Walk through it end to end with the example, which is wired up exactly this way and is off by
default:

```
cargo run --release --features tui,plugin-example-tracer --example terminal
# then: plugins = example-tracer   (and GB_TRACE_ADDR=C123 to pick the address)
```

## 8. Why compile-time, and not `dlopen`

Three options were on the table. The trade, plainly:

| | verdict |
|---|---|
| **Compile-time (a Cargo dependency)** | **chosen.** Robust, type-checked, no ABI risk, works on `wasm32`. Costs a rebuild. |
| **A manifest of plugin crates** | Same rebuild, plus a build script and a code generator. It buys you editing one file instead of two. Not worth it at this scale. |
| **Runtime `dlopen` of shared libraries** | **rejected, and not half-built.** |

The reason to reject runtime loading is not effort, it is that **Rust has no stable ABI**.
`repr(Rust)` layout is not guaranteed between compiler versions or even builds, so a `dyn
Plugin` handed across a `dlopen` boundary is undefined behaviour unless the whole interface is
`extern "C"` with `repr(C)` types and manual vtables — at which point `Machine`'s borrowed
handle, `String` returns and `Vec` returns all have to be redesigned as raw pointers with an
ownership protocol. That is a real project, it makes the API meaningfully worse for the
compile-time case that everyone actually uses, and it still breaks when the user upgrades their
compiler. `wasm32` has no `dlopen` at all, so the browser build would need a second mechanism
regardless.

If cross-language plugins become the need, the answer is not `dlopen` — it is a **socket or
subprocess transport**, which the design already anticipates: every `Event` serialises to one
NDJSON record (`Event::write_json`, the same shape the [telemetry](telemetry.md) stream uses),
and every reply a plugin gives is a small owned value rather than a borrowed pointer.
`GB_PLUGIN_TRACE=<path>` already writes that stream to a file. What is missing is a transport
and a command decoder, not a change to the trait.

Said plainly so nobody discovers it the hard way: **you cannot write a plugin in Python today.**

## 9. Where things live

```
crates/terminalgb-plugin-api/   the contract. No dependencies, and it does NOT
                             depend on the emulator — that is what lets the
                             emulator depend on plugin crates without a cycle.
plugins/example-tracer/      the one to copy
plugins/trainer/             live Pokémon party/badges/catch-chance panel
plugins/gen1-link/           virtual link-cable partner (trades, battles)
plugins/battle-control/      start any Gen 1 battle from a preset
plugins/agent/               an AI that plays the game
src/plugin.rs                the emulator's side: `impl MachineAccess for …`
```

```text
         terminalgb-plugin-api          (the contract; no dependencies)
          ^                ^
          |                |
 terminalgb (emulator)  terminalgb-plugin-*
          ^                ^
          |                |
          +--- optional ---+          (the frontend draws their panels)
```

The emulator builds and runs with every plugin removed — that is enforced by `make no-plugins`,
not merely intended.

**Ours are reference implementations, and they are meant to be read.** Roughly in order of how
much of the contract they exercise:

| plugin | capabilities | read it for |
|---|---|---|
| `example-tracer` | observe + read | the shape of a plugin, and testing without an emulator |
| `trainer` | observe + read | reading game structures; refusing an unknown cartridge gracefully; sampling cheaply (every 6th frame, and only while visible) |
| `gen1-link` | serial + read | being the other console; a byte-level protocol state machine; deliberately *not* declaring `WRITE_MEMORY` so a traded Pokémon must arrive through the real trade path |
| `battle-control` | observe + read + write + input | a writing plugin done carefully; testing against a real MBC with `TestMachine` |
| `agent` | observe + read + input | driving the buttons under a budget, and a subprocess driver |

## 10. Rules the host keeps, that you can rely on

- **Nothing loads implicitly.** `PluginHost` starts empty.
- **A plugin cannot be active and invisible.** Every attached plugin gets a row in the status
  panel, and the write/input markers live on the *name* so truncation cannot eat the warning.
- **One link partner.** A cable has two ends; a second `SERIAL_LINK` registration is refused.
- **Names are unique.** A duplicate `name()` is refused.
- **A plugin never draws.** You expose state; the frontend lays it out. That is why
  `as_any`/`as_any_mut` exist, and it is what keeps a plugin usable from the terminal frontend,
  the desktop one and a headless harness alike.
- **Plugin state is not machine state.** It is not serialised into save states, and loading one
  calls `reset()` so a half-finished exchange cannot desync silently.
- **`on_attach` runs on the first dispatch**, not at registration — that is the first moment a
  `Machine` exists. Use it to decide whether you understand this cartridge (`rom_title`,
  `global_checksum`).

## 11. Cost

A plugin runs *inside* the emulated frame. Everything you do is charged to the frame budget of
16.743 ms, so the same rule the audio path learned applies here: **never block**.

An empty host costs one `is_empty` test per frame and one per serial transfer. Ours sample
every 6th frame (~10 Hz) and only while their panel is visible; sprites are decompressed once
and cached. If you need to do real work, do it off the frame path — `plugins/agent` runs its
driver in a subprocess and picks up the answer when it arrives.
