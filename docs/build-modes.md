# Build modes, and every cargo feature

Everything is a cargo feature, and the honest summary is that **the default build is the one you
want to play on** — every other mode exists to take something away.

---

## What do you want?

| I want… | Command |
|---|---|
| **to play a game** | `cargo build --release` |
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
| `standard-plugins` | ✅ | the plugin crates below | nothing; each still has to be named to attach |
| `plugin-trainer` | ✅ | `terminalgb-plugin-trainer` | a Pokémon R/B cartridge, or it declines |
| `plugin-gen1-link` | ✅ | + `terminalgb-plugin-gen1-link` | as above |
| `plugin-battle-control` | ✅ | + `terminalgb-plugin-battle-control` | as above |
| `plugin-agent` | ✅ | + `terminalgb-plugin-agent` | an `agent_command` driver, or it declines |
| `plugin-gen1-shinyhunt` | ✅ | + `terminalgb-plugin-gen1-shinyhunt` | a Pokémon R/B cartridge, or it declines |
| `plugin-gen1-lobby` | ✅ | + `terminalgb-plugin-gen1-lobby` | a Pokémon R/B cartridge, or it declines |
| `plugin-example-tracer` | | `terminalgb-plugin-example-tracer` | nothing — **the one to copy** |
| `testing` | | — | `terminalgb::testing::TestMachine`, for plugin authors' tests |
| `ffi` | | — | drops the Rust frontends; exposes the C ABI |

`plugin-*` features are additive and imply the plugins they build on
(`plugin-battle-control` needs `plugin-gen1-link`, which needs `plugin-trainer`).
[`Cargo.toml`](../Cargo.toml) is the authority.

---

## See also

* **[the quick start](quickstart.md)** — from a clone to a running emulator, including the
  Codespace and the container image
* **[configuration](configuration.md)** — the `config.toml` keys, which are a separate decision
  from what a build contains
* **[when a build goes wrong](pitfalls/builds.md)** — the dual-rlib artefact and the rest of the
  build traps this repository has actually hit
* **[portability](portability.md)** — what the core runs on, and what the embedding build costs
