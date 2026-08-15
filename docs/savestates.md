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
| version `u16` | `FORMAT_VERSION`; mismatch → `UnsupportedVersion` |
| ROM title (len-prefixed string) | from the cartridge header |
| ROM global checksum `u16` | header 0x14E-0x14F |
| CPU: `a b c d e h l f` (`u8`), `pc sp` (`u16`), `ime` (`bool`), `setdi setei halt stop` (`u32`) | |
| MMU: WRAM (0x8000), ZRAM (0x7F), HDMA regs, IE/IF, WRAM bank, mode, speed, speed-switch, HDMA status/src/dst/len, undocumented CGB regs | |
| Serial, Timer, Keypad | register-visible state (serial callback is not state) |
| PPU | mode/clock/line/LYC, LCDC + STAT flags, scroll/window, DMG + CGB palettes, VRAM (0x4000), OAM (0xA0), palette RAM, VRAM bank, framebuffer | `bgprio` omitted (rebuilt each scanline) |
| APU block (len-prefixed) | present-flag + logical channel/envelope/length/sweep/LFSR/wave state; the `blip` buffers and host player are runtime plumbing, not state. Present on every target — the APU is emulated on `wasm32` too — and the flag is `false` only when no player has ever been installed. |
| MBC block (len-prefixed) | banks, RAM-enable, cartridge RAM, and the **MBC3 RTC** (counters, latch snapshot, **sub-second divider**). ROM is never stored. |

### Safety rails

- **Versioned.** Any layout change bumps `FORMAT_VERSION`; an old state is
  rejected, never mis-parsed.
- **ROM-checked.** The ROM is never embedded — only its title + global checksum.
  Loading a state against a different cartridge fails with `RomMismatch`.
- **Bounds-checked.** Every read is bounds-checked; a truncated or malformed blob
  returns `Truncated` / `BadField` instead of panicking.
- **wasm-clean.** No filesystem, no host assumptions. The APU section is
  length-prefixed and present-flagged, so a state from a machine that had a sound
  chip loads into one that does not (and now vice versa: `wasm32` builds emulate the
  APU as well — see [`docs/web-audio.md`](web-audio.md)). The format did not change
  when that happened, which is the point of a skippable block.

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
4. Negative checks: bad magic, wrong version, truncation, and wrong-game are all
   rejected.

It runs against the always-present vendored **dmg-acid2** ROM and, when the
gb-test-roms submodule is initialised, against **Blargg cpu_instrs** — whose
screen visibly scrolls, so "frames are identical after load" is a non-trivial
claim (the test reports how many distinct frames it observed).
