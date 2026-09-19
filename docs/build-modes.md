# Build modes, and every cargo feature

Everything is a cargo feature, and the honest summary is two sentences. **The default build is a
Game Boy emulator and its frontends — the one you want to play on, carrying nothing specific to
any cartridge.** Everything Generation 1 — the trainer panel, the virtual trade partner, the
battle simulator, the agent and scenario runners — is behind one opt-in flag, `gen1`.

---

## What do you want?

| I want… | Command |
|---|---|
| **to play a game** | `cargo build --release` |
| **everything Pokémon (Generation 1)** | `cargo build --release --features gen1` |
| **sound** | `cargo build --release --features audio` |
| **the terminal only** | `cargo build --release --no-default-features --features tui` |
| **the emulator with no plugins** | `cargo build --release --no-default-features --features tui,desktop` |
| **the core, for embedding** | `cargo build --release --no-default-features` |
| **any MCP client to drive it** | `cargo build --release --no-default-features --features mcp` |
| **the browser build** | `cargo build --release --target wasm32-unknown-unknown` |
| **a C / Go static library** | `make ffi-build` |

The embedding build is the interesting one: at `--no-default-features` the whole dependency tree
is **one third-party crate**, and CI enforces it. See
[portability § embedding the core](portability.md).

---

## Two run-time switches

Neither is a compile flag. Both are read once at start-up, and **a typo warns on stderr and is
ignored** rather than silently changing machine.

```bash
GB_PPU=identical   # the per-dot picture mode (or `standard`) — docs/picture-modes.md
GB_MODEL=dmg       # force a console: auto|dmg|dmg0|mgb|cgb|cgb0|agb|sgb|sgb2
```

`GB_MODEL` only overrides `Model::Auto`; an explicit caller choice always wins, and
`Gameboy::new_with_model` bypasses it entirely. The nine console models and what distinguishes
them are on [the accuracy page](accuracy.md).

---

## Every feature, what it pulls in, and what it needs at run time

| feature | in `default` | pulls in | needs at run time |
|---|:---:|---|---|
| *(none)* | | `blip_buf` | nothing |
| `desktop` | ✅ | `glutin`, `gl`, `libc`, `image` | a display server (X11 / Wayland / Windows / macOS) |
| `tui` | ✅ | `ratatui`, `ratatui-image`, `icy_sixel`, `image`, `libc` | a terminal — any terminal; it degrades rather than refusing |
| `audio` | | `cpal` | a sound device (ALSA **dev headers** to *build* on Linux) |
| `mcp` | | `plugin-agent`, `chiptune` — **no third-party crate** | an [MCP](https://modelcontextprotocol.io) client. Builds `terminalgb-mcp`. |
| `chiptune` | ✅ | `terminalgb-chiptune` — first-party, **no dependencies at all** | nothing. Builds `terminalgb-chiptune`. |
| `music` | ✅ | `tui`, `chiptune` | a ROM directory; the shelf reads their own title screens |
| `bgblink` | ✅ | — the whole implementation is `std::net` | a peer speaking BGB's link protocol |
| `testing` | | — | `terminalgb::testing::TestMachine`, for plugin authors' tests |
| `ffi` | | — | drops the Rust frontends; exposes the C ABI |

### Generation 1 — the opt-in layer

Nothing in this block is in `default`. `--features gen1` is all of it at once; every row is also
selectable on its own, and none of them adds a third-party crate.

| feature | in `gen1` | pulls in | needs at run time |
|---|:---:|---|---|
| `gen1` | — | everything below | as each row |
| `standard-plugins` | ✅ | the six plugin crates below | nothing; each still has to be named to attach |
| `plugin-trainer` | ✅ | `terminalgb-plugin-trainer` | a Pokémon R/B cartridge, or it declines |
| `plugin-gen1-link` | ✅ | + `terminalgb-plugin-gen1-link` | as above |
| `plugin-battle-control` | ✅ | + `terminalgb-plugin-battle-control` | as above |
| `plugin-agent` | ✅ | + `terminalgb-plugin-agent` | an `agent_command` driver, or it declines |
| `plugin-gen1-shinyhunt` | ✅ | + `terminalgb-plugin-gen1-shinyhunt` | a Pokémon R/B cartridge, or it declines |
| `plugin-gen1-lobby` | ✅ | + `terminalgb-plugin-gen1-lobby` | a Pokémon R/B cartridge, or it declines |
| `gen1-battle` | ✅ | `terminalgb-gen1-battle`, `plugin-battle-control` | nothing for the model; a cartridge for the differential harness |
| `portal` | ✅ | `plugin-gen1-link`, `plugin-agent`; HTTPS via the system `curl` | a trade portal to sign in to |
| `longplay` | ✅ | `plugin-agent` | a driver for the unattended runner |
| `scenario` | ✅ | `longplay` | as above |
| `plugin-example-tracer` | | `terminalgb-plugin-example-tracer` | nothing — **the one to copy**, and not Pokémon-specific |

`plugin-*` features are additive and imply the plugins they build on
(`plugin-battle-control` needs `plugin-gen1-link`, which needs `plugin-trainer`).
[`Cargo.toml`](../Cargo.toml) is the authority.

**The default build is checked, not merely intended.** CI fails if any cartridge-specific crate
reaches the dependency graph of a plain `cargo build`, and `make no-plugins` separately builds the
frontends with every plugin off and runs the library tests against them.

---

## See also

* **[the quick start](quickstart.md)** — from a clone to a running emulator, including the
  Codespace and the container image
* **[configuration](configuration.md)** — the `config.toml` keys, which are a separate decision
  from what a build contains
* **[when a build goes wrong](pitfalls/builds.md)** — the dual-rlib artefact and the rest of the
  build traps this repository has actually hit
* **[portability](portability.md)** — what the core runs on, and what the embedding build costs
