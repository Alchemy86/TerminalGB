# Save states

> **Keys:** `1`-`3` choose a slot · `F5` save · `F8` load. [Every control →](controls.md)

The core serialises the **complete** machine to a flat `Vec<u8>` and restores it
from `&[u8]`. It is frontend-agnostic and `wasm32`-clean: the core produces bytes,
each frontend decides where they live (terminal → a file, browser → local storage
or a download, debug surface → memory).

Public API:

```rust
let bytes: Vec<u8> = gameboy.save_state();
gameboy.load_state(&bytes)?;   // Result<(), terminalgb::savestate::StateError>
```

Also available one layer down on `Cpu` (`cpu.save_state()` / `cpu.load_state()`),
which is what the `Gameboy` methods delegate to. Implementation lives in
[`src/savestate.rs`](../src/savestate.rs) (the `Writer`/`Reader`/`StateError`
primitives) plus a `save_state`/`load_state` pair on every stateful component.

## Format: `GBSS`, versioned, native — **not** BESS

**Verdict: ship our own versioned native format now; propose BESS interop as a
follow-up.** Reasoning:

- **BESS** (Best Effort Save State, SameBoy's format, also read by others) is the
  right target for *interoperability* — loading our states in SameBoy and vice
  versa is a genuine prize. But BESS is a "best effort" format: it stores the raw
  memory blocks (RAM/VRAM/HRAM…) and the CPU/IO register file, and expects the
  loading emulator to **reconstruct** its own internal counters from those
  registers. This core keeps internal state that is *not* recoverable from the
  register file alone — the PPU `modeclock` and mid-scanline `mode`, the timer's
  split `internaldiv`/`internalcnt` accumulators, our HDMA bookkeeping, the APU
  frame-sequencer phase. Round-tripping those through BESS would reintroduce
  exactly the subtle desync this feature exists to avoid.
- So the **native** path stores every one of those fields explicitly and proves a
  byte-identical round-trip (below). It is versioned and fails loudly on
  mismatch.
- **BESS is a clean follow-up**, and purely additive: a `bess.rs` that maps our
  fields onto BESS blocks for *export* (lossless, since we are a superset) and
  *import* (reconstructing our counters from BESS's registers, the same "best
  effort" everyone else does). It can sit behind a cargo feature and share the
  `Writer`/`Reader`. Nothing in the current layout blocks it.

### Container layout (v1)

All integers little-endian. Variable / platform-varying sections are
length-prefixed (`u32` length + bytes) so a reader can skip what it can't use.

| Field | Notes |
|---|---|
| magic `"GBSS"` (4 bytes) | rejects non-states |
| version `u16` | outside `MIN_SUPPORTED_VERSION..=FORMAT_VERSION` → `UnsupportedVersion`; inside it, older versions are **upgraded** — see "Backward compatibility" below |
| ROM title (len-prefixed string) | from the cartridge header |
| ROM global checksum `u16` | header 0x14E-0x14F |
| CPU: `a b c d e h l f` (`u8`), `pc sp` (`u16`), `ime` (`bool`), `setdi setei halt stop` (`u32`) | |
| MMU: WRAM (0x8000), ZRAM (0x7F), HDMA regs, IE/IF, WRAM bank, mode, speed, speed-switch, HDMA status/src/dst/len, undocumented CGB regs | |
| Serial, Timer, Keypad | register-visible state (serial callback is not state) |
| PPU | mode/clock/line/LYC, LCDC + STAT flags, scroll/window, DMG + CGB palettes, VRAM (0x4000), OAM (0xA0), palette RAM, VRAM bank, framebuffer | `bgprio` omitted (rebuilt each scanline) |
| APU block (len-prefixed) | present-flag + logical channel/envelope/length/sweep/LFSR/wave state; the `blip` buffers and host player are runtime plumbing, not state. Present on every target — the APU is emulated on `wasm32` too — and the flag is `false` only when no player has ever been installed. |
| MBC block (len-prefixed) | banks, RAM-enable, cartridge RAM, and the **MBC3 RTC** (counters, latch snapshot, **sub-second divider**). ROM is never stored. |

### Safety rails

- **Versioned.** Any layout change bumps `FORMAT_VERSION`. A version outside
  `MIN_SUPPORTED_VERSION..=FORMAT_VERSION` is rejected, never mis-parsed —
  see "Backward compatibility" for what happens *inside* that window.
- **ROM-checked.** The ROM is never embedded — only its title + global checksum.
  Loading a state against a different cartridge fails with `RomMismatch`.
- **Bounds-checked.** Every read is bounds-checked; a truncated or malformed blob
  returns `Truncated` / `BadField` instead of panicking.
- **wasm-clean.** No filesystem, no host assumptions. The APU section is
  length-prefixed and present-flagged, so a state from a machine that had a sound
  chip loads into one that does not (and now vice versa: `wasm32` builds emulate the
  APU as well — see [`docs/web-audio.md`](web-audio.md)). The format did not change
  when that happened, which is the point of a skippable block.

### Backward compatibility — `MIN_SUPPORTED_VERSION..=FORMAT_VERSION`

**Every `FORMAT_VERSION` bump used to orphan every state written before it,
outright** — a real cost, hit for real: an external consumer's whole `.gbss`
corpus (AgentGB's certification start-states) went from loadable to refused
across four PPU bumps in a few days, with no way forward except regenerating
everything or freezing a build. Migrating a corpus is the wrong shape anyway —
it resets the clock until the next bump orphans it again.

So `Cpu::load_state` now accepts an explicit range,
[`savestate::MIN_SUPPORTED_VERSION`](../src/savestate.rs)`..=FORMAT_VERSION`,
and **upgrades** any version inside it as it reads, rather than refusing
outright. A version outside that range — below the floor or above the
ceiling — is still refused with `UnsupportedVersion`, exactly as before this
existed: **the reject-on-mismatch rule is not weakened, its window is just
wider than one version now.**

- **The floor moves only for reviewed, additive bumps.** `MIN_SUPPORTED_VERSION`'s
  own doc comment is the contract: raising `FORMAT_VERSION` for a change that is
  not purely additive (a field removed, reordered, or reinterpreted under the
  same layout) must raise `MIN_SUPPORTED_VERSION` to match, in the same commit.
  Every version between the two constants has been checked against its own
  commit to add fields and nothing else — see the numbered `v22`…`v26` list
  above [`FORMAT_VERSION`](../src/savestate.rs)'s own definition for exactly
  what each one added.
- **The writer never changed.** `save_state` still writes exactly one layout,
  the current one, unconditionally. Only the *reader* gained version
  awareness — `Cpu::load_state` reads the version word and threads it down
  through `MemoryManagementUnit::load_state` → `Gpu::load_state` →
  `Pipeline::load_state`, each with `if version >= N { r.u8()? } else {
  <documented default> }` at exactly the field a later bump added.
- **A default is a bounded cost, stated per field, never silent.** The four
  bumps this window currently spans (v23-v26) are all per-dot PPU state
  defaulted for at most one scanline's worth of drift — see the `if version
  >= N` comments in `Gpu::load_state`/`Pipeline::load_state` for what each
  one actually costs. `Gpu::oam_bus_y`/`oam_bus_x` (v23) are called out on
  their own in `MIN_SUPPORTED_VERSION`'s doc comment: unlike every other
  field defaulted here, this repository's own accuracy notes record them as
  machine state that survives *between* scanlines, so defaulting them can
  misjudge the very first OAM scan after the load — still bounded to one
  wrong scanline, never corruption, but the nearest thing to a real cost this
  path has.
- **Tested at three levels**, all under `cargo test --release --lib` /
  `--test savestate`: unit tests in `gpu::fifo::savestate_compat` and
  `gpu::savestate_compat` construct genuinely *shorter* old-format byte
  streams (via a `#[cfg(test)]`-only `save_state_as_of(version)` mirror of
  each real writer, never by merely stamping current bytes with an old
  version number) and assert both the documented defaults **and** that the
  reader consumes exactly that blob's own length; `cpu::core::savestate_compat`
  does the same at the whole-machine level, splicing a genuinely old-shaped
  GPU block into an otherwise real `Cpu::save_state()` snapshot; and
  `testharness/savestate.rs`'s `negative_checks` proves both edges of the
  window (`MIN_SUPPORTED_VERSION - 1` and `FORMAT_VERSION + 1`) are still
  refused.

### Performance

Serialisation touches no hot path: `save_state`/`load_state` are called only when
the user asks, and the derive-free hand-rolled `Writer`/`Reader` add nothing to
`rb`/`wb`/`do_cycle`. No struct on the emulation hot path grew a field or a trait
bound. (This is also why we did not reach for `serde` + a format crate: it would
have meant derives across every core struct for no measured benefit over a
~200-line hand-rolled codec, matching the project's existing no-dep hashing/PNG
convention.)

## Proving the round-trip

[`testharness/savestate.rs`](../testharness/savestate.rs) (`cargo test --release
--test savestate`) is the proof:

1. Run N frames, `save_state()`, then keep running the reference machine.
2. Load the snapshot into a **fresh** machine and run it forward in lockstep.
3. Assert **byte-identical framebuffers every frame**, *and* byte-identical full
   `save_state()` snapshots every frame — the latter is a strong completeness
   check: any live field left out of the state would make the two runs drift and
   fail here.
4. Negative checks: bad magic, a version *outside* `MIN_SUPPORTED_VERSION..=
   FORMAT_VERSION` (both edges — one below the floor, one above the ceiling),
   truncation, and wrong-game are all rejected. Loading a genuinely old
   (`MIN_SUPPORTED_VERSION`) state and continuing to upgrade the range as new
   versions are added is covered separately — see "Backward compatibility"
   above for where.

It runs against the always-present vendored **dmg-acid2** ROM and, when the
gb-test-roms submodule is initialised, against **Blargg cpu_instrs** — whose
screen visibly scrolls, so "frames are identical after load" is a non-trivial
claim (the test reports how many distinct frames it observed).
