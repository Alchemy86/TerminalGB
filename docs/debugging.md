# Debug tooling — inspect, manipulate, capture

> How to see and steer what the emulator is doing, designed to serve the
> **[accuracy testing](testing.md)** work rather than sit beside it. This page is a
> **design**, not shipped code — nothing here changes emulation behaviour yet. It names the
> smallest core surface that lets *any* frontend (desktop, terminal, web) debug the machine
> **without paying for debug hooks in release builds**.
> · [← Component index](components/README.md) · [← Accuracy testing](testing.md) · [← Top-level README](../README.md)

---

## Why this page exists

The captain's ask: *"see if we can have debug modes etc so we can see or manipulate things,
screenshots, video stream etc."* — with a pointer to a modern Rust emulator
([ioncodes/ayyboy-advance](https://github.com/ioncodes/ayyboy-advance)) for hints, and one
firm instruction: **keep Rust and modern**, take ideas that transfer, and **do not design a
wishlist**. A tile viewer and a step-through debugger find real bugs; a waveform display
mostly does not. So this page **ranks by bug-finding value**, states what is cheap versus
genuinely hard *given this codebase*, and ends with a phased order and a clear first step.

The single most important question — **what must the core expose so any frontend can debug
it without bolting debug concerns into the hot emulation path?** — is answered in
[The core/frontend boundary](#the-corefrontend-boundary). The short version: this
architecture is unusually friendly to debugging, because the core already steps one
instruction at a time and already hands out most of its state. Only **one** feature
(memory-access breakpoints) needs to touch the hot path, and that one is feature-gated off
in release.

---

## What ayyboy-advance actually does — taken vs rejected

[ayyboy-advance](https://github.com/ioncodes/ayyboy-advance) is a modern Rust **Game Boy
Advance** emulator by ioncodes. It is a Cargo workspace: `gba-core` (emulation), a separate
`debugger` binary crate (`ayydbg`), and a `rom-db` headless screenshot harness with a
Next.js gallery. It was read for this design as *architecture and ideas, not code*.

> ⚠️ **Licence: none — the repository ships no `LICENSE` file and the GitHub API reports
> `license: null` (all-rights-reserved).** That is *stronger* than a permissive licence for
> our purposes: **we may take no code at all, only ideas re-implemented from scratch.**
> Everything below is a design lesson, not a snippet. Our crate stays **MPL-2.0**.

A GBA is not a Game Boy — it is an ARM7TDMI with a different PPU, DMA model, and a 256 MB
address space — so anything whose *value* depends on GBA hardware is explicitly out.

### Taken (ideas that transfer — re-implemented, not copied)

| From ayyboy-advance | What we take | Why it transfers |
|---|---|---|
| **Debugger as a snapshot consumer, never touching core state** (its `debugger` crate ships plain snapshot structs — `Cpu{registers,cpsr,dma,timers}`, `PpuRegisters`, decoded instructions, rendered tile/palette/OAM pixel buffers — over `crossbeam-channel`, with the core on its own thread) | The *principle*: the debugger reads **snapshot structs the core builds**, and only when asked. We express it as a borrowing `DebugView` for our in-loop frontends, and note the same snapshot structs serialise over a channel/FFI for the threaded/web case. See [boundary](#the-corefrontend-boundary). | The decoupling is hardware-independent and is the whole point — debug reads never sit in the execution path. |
| **`egui`/`eframe` immediate-mode debugger** (`eframe`/`egui`/`egui_extras` 0.31, `egui-phosphor` icons, `egui-toast` notifications) with a register grid, a **virtualised** hex viewer (`ScrollArea::show_rows`), and palette/tile/sprite texture panes with rich OAM hover-dumps | The widget *layout*, and `egui` as the proposed desktop debug-UI crate. Immediate-mode fits an emulator's "re-read everything each frame" model. | UI-framework choice is hardware-independent; `egui` is the modern-Rust default. |
| **`TrackedValue<T>` dirty-highlighting** (`set()` flips a `changed` flag on inequality; the UI red-tints what changed since last refresh) | The same trick to tint changed registers/bytes in our viewers — cheap, and it makes step-debugging read at a glance. | A pure UI convenience, trivially portable. |
| **Per-subsystem `tracing` targets with runtime filtering** (`--targets cpu,ppu,…`, no recompile) | A `tracing` target per component (`cpu,ppu,mmu,mbc,apu,timer,serial,joypad`), verbosity chosen at runtime. Replaces this repo's commented-out `println!`/`eprintln!` (e.g. [`src/cpu/core.rs:51`](../src/cpu/core.rs)). | Logging discipline is stack-agnostic and cheap. |
| **One `decode()` + `Display`, reused by the UI *and* scripts** (theirs uses the `bitmatch` proc-macro over ARM/THUMB) | The factoring, not the code: a single SM83 disassembler used by the debugger view *and* the Game Boy Doctor log. | The reuse pattern transfers; the ISA does not (see [Disassembler](#the-disassembler-the-one-real-build)). |
| **Scripted breakpoints / patches (`rhai`)** — a user script maps `address → handler`, and a proxy exposes `read/write_u8/16/32`, `read/write_register` so you can express conditional breakpoints, pokes, and traces **without recompiling** | The *UX* — an address→handler scripting layer is the single most powerful debug idea here — listed as an optional later phase. | Scriptable debugging is hardware-independent and high-leverage. |
| **Deterministic framebuffer capture** for the renderer | A stable frame hash + PNG dump keyed off our existing `Gameboy::image()`. | Frame capture is renderer-agnostic; the feature that pays for itself (see [Capture](#3-capture)). |

### Rejected / improved (GBA-specific, unsafe, or simply absent there)

| From ayyboy-advance | Why rejected or changed here |
|---|---|
| **The 256 MB full-address-space memory snapshot** it copies on every memory refresh (`Box<[u8; 0x1000_0000]>`) | Brute-force; fine at GBA desktop scale, wasteful. Our address space is **64 KB** — snapshot the *selected region* via `peek_range`, never the whole map. |
| **The `rhai` proxy's `unsafe` raw-pointer core access** (`*mut Cpu` + `unsafe impl Send/Sync`, dereferenced unsynchronised) | A memory-safety footgun. The handler runs synchronously inside the tick, so if we adopt scripting we pass a **safe `&mut` borrow** through instead of stashing a raw pointer. |
| **ARM/Thumb disassembly and instruction tracing** | We need an **SM83 (LR35902)** disassembler — a different ISA, written for this core. The GBA one is useless to us. |
| **GBA-specific viewers** (mode 3/4/5 bitmap layers, affine/rotation-scaling backgrounds, 4bpp/8bpp OBJ, 512-entry palette RAM) | The DMG/CGB PPU has none of this. Our viewers are DMG/CGB tile-map + OAM + BG/window map + DMG/CGB palettes only. |
| **Its threaded-crate windowing scaffold** | Our desktop frontend is raw **glutin + OpenGL** ([`src/screen/desktop.rs`](../src/screen/desktop.rs)) and we already have a **ratatui terminal** frontend it lacks. We reuse what we have (see [Where it lives](#4-where-it-lives)). |
| **Its headless harness's frame handling** — `rom-db` dumps PNGs every 500 frames **for human eyeballing in a web gallery; there is no frame hashing and no golden-image diff/assert anywhere in the repo** | We are already *ahead* here: [testing.md](testing.md) specs a reference-PNG compare for acid2. There is nothing to copy — **build the hash/compare layer fresh** (it is the [first thing](#the-first-thing-to-build)). Their CPU oracle (watch R15 hit hard-coded ROM addresses for pass/fail) is, however, exactly analogous to our Mooneye `LD B,B`/Fibonacci detection — reassuring that our plan matches practice. |

**One deliberate divergence worth stating.** ayyboy-advance keeps debug code out of the core
by **crate separation, not `#[cfg]` flags** — its hot loop lives in the `debugger` crate and
the core has no debug feature at all (`[profile.dev] opt-level = 3` keeps debug builds fast).
That is clean and we borrow its spirit (Tiers 1–2 below add no hot-path cost and need no
gate). But our frame loop lives *in the core* ([`Gameboy::frame`](../src/gameboy.rs)), and
the one feature that must hook `mmu::rb/wb` (watchpoints) sits in the core's hot path no
matter how the crates are split — so for that one feature we use a cargo feature gate. Where
their model needs no gate, neither do we; where our topology forces a hot-path hook, we gate
it off in release.

**Also studied — the tools emulator developers actually use.** SameBoy and BGB are the
battle-tested references; their debugger feature sets shaped the ranking below more than
ayyboy-advance did, because they target the *same* hardware:

- **BGB** — the de-facto DMG/CGB debugger: CPU + disassembly + breakpoints (execution,
  read, write, and access to I/O), a VRAM viewer (tiles, both BG maps with the screen
  rectangle overlaid, OAM sprites, palettes), an I/O-register map view, and conditional
  breakpoints. This is the concrete target for our inspection + manipulation panels.
- **SameBoy** — a command-line debugger (`b`/`breakpoint`, `w`/`watchpoint`, `s`/step,
  `p`/print-expression, `x`/examine-memory), plus a Cocoa VRAM viewer. Its **text-command
  debugger** is the model for our terminal frontend, which is the cheapest interactive
  debugger we can ship because ratatui is already a dependency.

The lesson from both: the features that get used every day are **the step-through debugger,
breakpoints, the VRAM/tile viewer, and the OAM viewer.** Everything else is convenience.

---

## The current architecture, from a debugger's point of view

Three facts about this codebase decide the whole design.

**1. `do_cycle()` already executes exactly one instruction.**
[`Cpu::do_cycle()`](../src/cpu/core.rs) runs one opcode (or one interrupt dispatch) and
advances every peripheral by that instruction's ticks. [`Gameboy::frame()`](../src/gameboy.rs)
is nothing more than a loop over `do_cycle()` until the PPU signals a finished frame:

```rust
// src/gameboy.rs — the entire frame loop
loop {
    ticks += self.cpu.do_cycle();
    if self.check_and_reset_gpu_updated() { break; }
}
```

That means **stepping is already implemented** — the core just never exposed the finer
granularities. Step-instruction, step-scanline, step-frame, and run-to-breakpoint are all
*driver-loop* constructs: check a condition between `do_cycle()` calls. **None of them cost
anything in the hot path, and none require restructuring the core.**

**2. Decode and execute are fused.** [`Cpu::operation()`](../src/cpu/core.rs) is one large
`match op { … }` that reads the opcode byte and immediately executes it, returning M-cycles.
There is **no standalone decoder**. So a **disassembler must be written from scratch** — it
cannot be salvaged from the execution path. This is the one genuine build in the whole
design (see [below](#the-disassembler-the-one-real-build)), and it pays double because
[Game Boy Doctor](testing.md#4-game-boy-doctor--a-cpu-on-ramp-before-the-ppuserial-work)
also wants a per-instruction CPU-state log.

**3. State is already almost entirely reachable.** Most of what a debugger wants to read is
already `pub`, or already readable through an existing method:

| Want to inspect | Already reachable via | Gap |
|---|---|---|
| CPU registers, flags, `IME`, `halt` | `cpu.registers` (all fields `pub`), `cpu.ime`, `cpu.halt` | `Gameboy.cpu` is **private** — needs one public entry point |
| Any byte across the memory map | `mmu.rb(addr)` routes to every region incl. all I/O registers | `rb` takes `&mut self` and *could* have read side-effects — want a side-effect-free `peek` |
| All I/O registers (LCDC, STAT, LY, SCX/SCY, palettes, timer, `IF`/`IE`) | `mmu.rb(0xFFxx)` → `gpu.rb` / `timer.rb` / fields `inte`,`intf` | none — already works through `peek` |
| VRAM, OAM bytes | `gpu.vram`, `gpu.voam` are `pub` | none |
| Framebuffer | `Gameboy::image()` → RGBA `&[u8]` | none |
| Decoded PPU state (mode, line, scroll, win, tile-map select, palettes) | `gpu.rb(reg)` gives raw register values; palettes `palb/pal0/pal1`,`cbgpal`,`csprit` are **private** | a few read accessors for a clean tile/palette viewer |
| Current MBC bank | `mmu.mbc` is `pub` but the trait exposes no bank number | one accessor on the `MemoryBankController` trait |

The upshot: **~90% of inspection is exposing a read-only view over state that already
exists.** The work is a public entry point on `Gameboy`, a handful of accessors, and the
disassembler — not a rearchitecture.

---

## The core/frontend boundary

The rule: **the core exposes state and single-stepping; the frontend builds the debugger.**
Debug logic never lives in the hot emulation path, and no frontend reaches into private
fields. Concretely, three tiers with three different cost profiles:

### Tier 1 — Read-only inspector (always compiled in, zero cost when unused)

A new `debug` module in the core with a borrowing view:

```rust
impl Gameboy {
    /// Read-only snapshot access. Pure reads — safe to call between steps/frames.
    pub fn debug(&self) -> DebugView<'_> { DebugView { gb: self } }
}

pub struct DebugView<'a> { gb: &'a Gameboy }

impl DebugView<'_> {
    pub fn cpu(&self) -> CpuState;                 // regs, flags, IME, halt, SP, PC
    pub fn peek(&self, addr: u16) -> u8;           // side-effect-free bus read
    pub fn peek_range(&self, base: u16, len: usize) -> Vec<u8>;
    pub fn disassemble(&self, pc: u16, count: usize) -> Vec<Instr>;
    pub fn io(&self) -> IoRegs;                     // LCDC/STAT/LY/scroll/timer/IF/IE …
    pub fn vram_tiles(&self) -> TileBank<'_>;       // decode 2bpp → indices for a viewer
    pub fn bg_map(&self, which: BgMap) -> MapView;  // 32×32 tile indices + attributes
    pub fn oam(&self) -> [Sprite; 40];              // decoded OAM entries
    pub fn palettes(&self) -> Palettes;             // DMG + CGB
    pub fn mbc(&self) -> MbcState;                  // type + current ROM/RAM bank
    pub fn timing(&self) -> Timing;                 // frame count, mode, modeclock, line
}
```

Because it is `&self` and only reads, it costs **nothing** unless a frontend calls it —
there is no reason to feature-gate it. This is the single entry point that lets *any*
frontend debug without touching internals.

**Single-thread borrow vs. cross-thread snapshot.** ayyboy-advance runs its core on a
separate thread and ships snapshot structs to the UI over `crossbeam-channel`; the UI never
holds a reference to core state. Our frontends (terminal, and a desktop overlay sharing the
GL loop) drive the emulator *synchronously* in the same thread, so a borrowing `DebugView`
is simpler and allocation-free — no channel, no copy. The design keeps both doors open: the
returned structs (`CpuState`, `IoRegs`, decoded tiles, …) are **plain owned data**, so the
*same* accessors serialise cleanly (a) over a channel if we later run the core on its own
thread, and (b) as JSON/FFI for the **web** debug pages. We take ayyboy's *decoupling
principle* (debugger consumes snapshots the core builds) without paying for its threading
model where we don't need it.

### Tier 2 — Control (always compiled in, zero cost when unused)

Re-expose the existing loop at finer granularity. These are thin `pub fn`s over
`do_cycle()`; the frontend decides when to call them:

```rust
impl Gameboy {
    pub fn step(&mut self) -> u32;          // one instruction (existing do_cycle)
    pub fn step_scanline(&mut self);        // do_cycle until gpu.line changes
    pub fn step_frame(&mut self);           // = today's frame()
    pub fn run_until<F: Fn(&Gameboy) -> bool>(&mut self, stop: F, budget: u64);
}
```

`run_until` is how **PC breakpoints, conditional breakpoints, and "run to cursor"** are
built — the predicate is checked *between* instructions, in the driver, so it never burdens
the hot path. Pause is simply the frontend not calling `step_frame()`.

### Tier 3 — Manipulation writes + access breakpoints

Live editing of registers and memory is cheap and always available:

```rust
impl Gameboy {
    pub fn poke(&mut self, addr: u16, val: u8);   // routes through mmu.wb
    pub fn set_reg(&mut self, reg: Reg, val: u16); // registers are already pub
}
```

(Honest caveat: `poke` through `wb` respects hardware write semantics — writing an I/O
register triggers its side-effects, and writing ROM space hits the MBC, not RAM. That is
usually what you want, but a "raw VRAM/WRAM poke" that bypasses routing is a separate,
clearly-labelled method.)

**The one hot-path feature: memory-access breakpoints (watchpoints).** Breaking on *read of*
or *write to* an address cannot be done from the driver loop — it needs a check inside
[`mmu::rb`/`wb`](../src/mmu/mod.rs), the two hottest functions in the emulator. This is the
**only** part of the design that touches the hot path, so it is the **only** part behind a
cargo feature:

```toml
[features]
debug = []   # off by default; compiles watchpoint checks into mmu::rb/wb
```

```rust
// src/mmu/mod.rs
pub fn wb(&mut self, addr: u16, val: u8) {
    #[cfg(feature = "debug")]
    self.watchpoints.on_write(addr, val);   // compiled out entirely in release
    // … existing routing …
}
```

A release build contains no watchpoint code and pays not a single branch; a `--features
debug` build pays one predictable, well-predicted branch per access. This is precisely the
constraint the captain set: *"a debug build that runs at half speed is fine; a release build
paying for debug hooks it never uses is not."* In practice the branch is far cheaper than
half speed — but the guarantee is what matters: **release is untouched.**

### Boundary summary

```
              ┌───────────────────────── core (src/) ─────────────────────────┐
frontend ───► │  Gameboy::debug()  → DebugView  (Tier 1, pure reads)           │
(egui / TUI / │  Gameboy::step*/run_until/poke/set_reg  (Tier 2/3, driver API) │
 web pages)   │  #[cfg(feature="debug")] watchpoints in mmu::rb/wb  (Tier 3)   │
              └───────────────────────────────────────────────────────────────┘
   Hot path (do_cycle → operation → mmu::rb/wb) is untouched except for the
   feature-gated watchpoint check. No frontend sees a private field.
```

---

## The three jobs

### 1. Inspect

Ranked by how often it actually finds an emulation bug (BGB/SameBoy experience):

| Panel | Reads | Cost | Bug-finding value |
|---|---|---|---|
| **CPU registers + flags + IME/halt** | `cpu.*` | trivial | **High** — the anchor of every CPU/timing investigation |
| **Disassembly around PC** | `disassemble()` | *build the disassembler* | **High** — pairs with stepping; see what's about to run |
| **Memory viewer (hexdump, any region)** | `peek_range()` | trivial | **High** — "what's actually at `0xC0xx`?" |
| **VRAM tile viewer** | `vram_tiles()` | low (2bpp decode) | **High** — *the* PPU-bug tool; acid2 says the face is wrong, this says why |
| **BG + window map viewer** (with screen rectangle) | `bg_map()` | low | **High** — scroll, tile-map select, window-line bugs |
| **OAM / sprite table** | `oam()` | low | **High** — priority, flip, 10-per-line, off-screen sprites |
| **Palette viewer** (DMG + CGB) | `palettes()` | low | Medium — colour/attribute bugs |
| **I/O register map** (LCDC, STAT, timer, `IF`/`IE`) | `io()` | trivial | Medium-High — one screen of "is the hardware configured how I think?" |
| **Interrupt + timer state** | `io()` + `cpu.ime` | trivial | Medium — IRQ-timing bugs (our 16-vs-20 T-cycle dispatch) |
| **Current MBC bank** | `mbc()` | one accessor | Medium — banking bugs, the MBC3-for-everything web/FFI bug |
| **APU channel state** (registers/enabled) | `io()` | trivial | Low-Medium |
| **APU waveform display** | — | high | **Low — explicitly not worth building.** Looks impressive, rarely fixes anything; the channel *registers* are what you actually read. |
| **Cycle/frame timing readout** | `timing()` | trivial | Medium — pairs with stepping |

### 2. Manipulate — the harder half the captain asked for

What is **cheap** here, given the architecture:

- **Pause** — frontend stops calling `step_frame()`. Free.
- **Step one instruction** — expose `do_cycle()`. Free.
- **Step one frame** — already `frame()`. Free.
- **Step one scanline** — loop `do_cycle()` until `gpu.line` changes. Cheap; needs `line`
  readable (already via `gpu.rb(0xFF44)`).
- **Run to PC breakpoint / conditional breakpoint / run-to-cursor** — `run_until` with a
  predicate over `DebugView`. Cheap; **driver-loop only, zero hot-path cost.**
- **Break on interrupt** — check `cpu.ime`/pending `IF&IE` between steps, or trap the
  dispatch in `run_until`. Cheap.
- **Edit registers** — fields are already `pub`. Trivial.
- **Edit memory** — `poke()` via `wb`, or a raw-write variant. Trivial (with the routing
  caveat above).

What is **genuinely hard** (and why):

- **Memory read/write watchpoints** — the *only* feature needing a hot-path hook. Solved by
  the `debug` feature gate (Tier 3). Not hard to write; the care is entirely about keeping
  it out of release, which the gate does.
- **Step *backwards* / rewind** — needs save-state snapshotting, which the core does **not**
  have today (README lists save states as unimplemented). This is a real restructuring item
  (see [What needs core restructuring](#what-would-need-core-restructuring)). Deferred — it
  is high-effort and BGB-style forward stepping + breakpoints already find the overwhelming
  majority of bugs.
- **Mid-instruction / T-cycle stepping** — impossible without breaking up the fused
  decode-execute in `operation()` into a cycle-stepped microarchitecture. That is a *core
  rewrite*, justified only if the project commits to cycle-accurate PPU/DMA timing. Not
  worth it for a debugger alone. Instruction-granularity stepping is what BGB and SameBoy
  give you and it is enough.

### 3. Capture

| Feature | How | Value |
|---|---|---|
| **Deterministic frame hash + reference compare** | hash `Gameboy::image()` (or, better, a palette-independent 2-bit index buffer) → compare to a checked-in reference | **Highest — build this first.** It *completes* [testing.md step 5](testing.md#how-a-headless-ci-harness-would-run-them): the dmg-acid2/cgb-acid2 harness needs exactly this to assert a frame in CI. It is a dozen lines and it turns the whole PPU test plan from "eyeball the face" into a green/red CI gate. |
| **Screenshot (PNG)** | encode `image()` with the `image` crate — already demonstrated in [`src/lib.rs`](../src/lib.rs) `image_base64` | High utility, trivial cost |
| **Raw frame callback** | `Gameboy::on_frame(Box<dyn FnMut(&[u8])>)`, mirroring the existing [serial callback](../src/mmu/serial.rs) pattern | Medium — the general primitive GIF/video/streaming all build on |
| **GIF / video capture** | feed frames from the callback to the `image` crate's GIF encoder (or pipe to `ffmpeg`) | Low-Medium — good for bug reports and the README demo GIFs, not a bug-*finder* |
| **Stream frames to the browser** | the WASM build already pushes `image()` to a `<canvas>`; a debug stream adds state alongside it | Phase 3, larger lift — see [Where it lives](#4-where-it-lives) |

**Determinism note (matters for frame hashing).** Two nondeterminism sources exist: WRAM is
filled at init by an LCG with a **fixed seed** ([`src/mmu/mod.rs`](../src/mmu/mod.rs),
`fill_random(&mut res.wram, 42)`) — so that is *reproducible*, good; and **MBC3's RTC uses
wall-clock time** — that must be frozen (or the test ROM must not read it) for a stable hash.
Test ROMs like acid2 don't, so frame hashing is safe today; note it so it stays that way.

**Why hash an index buffer, not raw RGBA?** dmg-acid2 checks *compositing*, not the exact
grey shades. Hashing a 2-bit-per-pixel palette-index frame (or a normalised 4-shade image)
makes the check robust to palette representation while still catching every compositing bug
the reference is designed to expose. Offer both; default the acid2 harness to the
index-buffer hash.

### The disassembler — the one real build

Everything else is accessors and driver-loop logic; the disassembler is the one component
that must be *written*. It is a self-contained, table-driven pure function:

```rust
pub fn disassemble(mem: &dyn Fn(u16) -> u8, pc: u16) -> (String, u8); // text, length
```

- ~256 base opcodes + 256 `CB` opcodes, but they collapse into a handful of format families
  (`LD r,r`, `ALU r`, `JR cc`, …). Bounded, mechanical, ~a day of work.
- **No external crate.** There is no well-maintained SM83 disassembler crate worth a
  dependency, and writing it in-tree keeps the opcode truth in one repo. Reject the dep.
- **It pays double:** the same per-instruction formatter drives the
  [Game Boy Doctor](testing.md#4-game-boy-doctor--a-cpu-on-ramp-before-the-ppuserial-work)
  state-log format (`A:.. F:.. … PC:.. PCMEM:..`), which the testing plan already wants for
  pinpointing the exact instruction where CPU state first diverges.

### 4. Where it lives

The core exposes the API once; frontends consume it. Recommended order, cheapest useful
first:

1. **Terminal (ratatui) debugger — build the interactive UI here first.** `ratatui` is
   **already a dependency** ([`Cargo.toml`](../Cargo.toml)); the terminal frontend already
   exists ([`src/screen/tui.rs`](../src/screen/tui.rs)). A panel layout — registers +
   disassembly + hexdump + a command line (`b`, `s`, `x`, à la SameBoy) — is the
   lowest-cost *interactive* debugger we can ship, with **zero new dependencies.** This is
   the recommended home for the first interactive debugger.
2. **Desktop (egui overlay).** The modern-Rust choice and what an ayyboy-class emulator
   uses. Honest cost: the desktop frontend is raw **glutin + OpenGL**
   ([`src/screen/desktop.rs`](../src/screen/desktop.rs)), so adding `egui`/`eframe` is a
   real integration (either an `egui_glow` layer over the existing GL context, or migrating
   the window loop). **Propose `egui`; do not adopt it yet** — it is the biggest single
   dependency decision here and belongs to a later phase.
3. **Web debug pages (Phase 3).** The roadmap already put web debug in Phase 3. The WASM
   build already ships `image()` to a canvas; debug pages add JSON-serialised `DebugView`
   snapshots (registers, hexdump, tile/OAM canvases) over the existing FFI-style surface.
   Largest lift, lowest urgency — do it once the API and the native debugger have proven the
   shape.

The **CI harness** (frame hashing, serial capture) is a *headless* consumer of the same core
API and needs no UI at all — which is why it can, and should, come first.

---

## What would need core restructuring

Only two items force a change deeper than "add an accessor," and both are optional:

1. **Save states / rewind.** No snapshot mechanism exists (README: save states
   unimplemented). Adding `serde`-based serialisation of the whole machine (`Cpu` + `Mmu` +
   `Gpu` + `Mbc` + `Timer`) would unlock save/load, **rewind (step-backwards) debugging**,
   and deterministic test fixtures ("boot to this exact state, then run the test"). This is
   worth doing **on its own merits** — players want save states and the test harness wants
   reproducible start points — so the debugger should be a beneficiary of that work, not the
   justification for it. Medium-large effort (every component must serialise; the `Box<dyn
   MemoryBankController>` and the serial callback need care).
2. **Cycle-stepped CPU (T-cycle granularity).** Would require decomposing the fused
   decode-execute `operation()` match into a micro-stepped core. This is a *large* rewrite
   whose real payoff is **cycle-accurate PPU/DMA/timer timing** (the Mealybug-Tearoom-class
   accuracy the component pages flag as not-yet-modelled), not debugging per se.
   **Not worth doing for the debugger** — instruction-granularity stepping is the industry
   norm and is enough. List it only so the boundary is honest: T-cycle stepping is the one
   debug feature this architecture cannot cheaply provide.

Everything else in this document is additive and leaves the hot path alone.

---

## Phased plan, ranked by value

Effort is rough (S ≈ hours, M ≈ a day or two, L ≈ a week+). Ordered so each phase produces
something that finds bugs immediately and feeds the [testing plan](testing.md).

### Phase 0 — the read-only + control API and the first CI win *(highest value)*
- **[S]** `Gameboy::debug() -> DebugView` with `cpu()`, `peek/peek_range()`, `io()`,
  `timing()`; the `Reg`/`CpuState`/`IoRegs` types. *(Public entry point + accessors only.)*
- **[S]** Tier-2 control: `step()`, `step_scanline()`, `step_frame()`, `run_until()`.
- **[S]** **Deterministic frame hash + reference-image compare**, wired into the
  **dmg-acid2** harness from [testing.md](testing.md). ← **Build this first.**
- **[S]** Screenshot-to-PNG helper (reuse the `image_base64` code path).

**Why first:** turns the PPU test plan into a green/red CI gate, exposes the entire state
surface every later panel needs, and gives instruction-stepping + PC-breakpoints — all
without a UI, a new dependency, or a hot-path change. This is the maximum bug-finding power
per unit effort.

### Phase 1 — the SM83 disassembler + the terminal debugger *(the interactive workhorse)*
- **[M]** In-tree SM83 disassembler (`disassemble(pc, n)`), shared with Game Boy Doctor
  logging.
- **[M]** ratatui debugger: registers + disassembly-around-PC + hexdump + a
  `b`/`s`/`x`/`run` command line. Pause/step/breakpoint on PC. **Zero new dependencies.**

**Why second:** this is the step-through debugger that, with the register view, finds the
most CPU/timing bugs — and it reuses a frontend and a dependency we already have.

### Phase 2 — the PPU viewers + watchpoints *(the PPU-bug tools)*
- **[M]** `vram_tiles()`, `bg_map()`, `oam()`, `palettes()` accessors + a few `pub` PPU
  read methods; render them in the terminal (and behind egui when it lands).
- **[S]** `#[cfg(feature="debug")]` memory read/write watchpoints in `mmu::rb/wb`, surfaced
  through `run_until`.
- **[S]** `poke()` / `set_reg()` live editing.

**Why third:** once acid2 flags a broken frame (Phase 0), these tell you *why*. Watchpoints
answer "what corrupted this byte?" — the classic hard bug — and are the only hot-path item,
safely gated.

### Phase 3 — desktop egui overlay + capture extras *(polish & reach)*
- **[L]** `egui`/`eframe` desktop debugger overlay (the dependency decision; integrate with
  the glutin/GL window).
- **[M]** Raw frame callback → GIF/video capture.
- **[L]** Web debug pages consuming JSON `DebugView` snapshots.

### Later / optional
- **[M]** **Scripted breakpoints (`rhai` or similar)** — ayyboy-advance's standout idea: a
  user script maps `address → handler`, and a safe `&mut` proxy (`peek`/`poke`/`set_reg`,
  read/write registers) lets you express conditional breakpoints, live patches, and traces
  **without recompiling**. High-leverage once the `DebugView`/control API and watchpoints
  exist — it is mostly glue over them. Re-implement with a *safe* borrow, not ayyboy's raw
  pointer. New dependency, so propose-don't-adopt.
- **[L]** `serde` save states → load/rewind debugging + reproducible test fixtures
  *(restructuring-gated — see [above](#what-would-need-core-restructuring))*. Do it for save
  states; the debugger benefits for free.
- **[XL]** Cycle-stepped CPU for T-cycle debugging *(restructuring-gated)* — only if the
  project commits to cycle-accurate timing; not for debugging alone.

---

## The first thing to build

**A deterministic frame hash over `Gameboy::image()`, wired into a dmg-acid2 CI check,
alongside the read-only `DebugView` and instruction-step API.** It is a few hours of work,
adds no dependency, touches no hot path, and immediately converts the highest-value item in
[testing.md](testing.md) — the acid2 "face test" — from a manual eyeball into an automated
regression gate. Everything else (disassembler, terminal debugger, tile/OAM viewers)
sharpens the debugging loop; this one closes the loop with the accuracy work that is already
the project's stated top priority.

---

## Sources

- ayyboy-advance (architecture/ideas studied, MIT-licensed, no code taken) —
  https://github.com/ioncodes/ayyboy-advance
- BGB debugger feature reference — https://bgb.bircd.org/manual.html
- SameBoy debugger — https://github.com/LIJI32/SameBoy (`Documentation/`)
- Accuracy yardstick this design serves — [docs/testing.md](testing.md)
- Component reference (state each viewer surfaces) — [docs/components/README.md](components/README.md)
- egui / eframe (proposed desktop debug UI, not yet adopted) — https://github.com/emilk/egui
- Pan Docs (I/O register and PPU semantics the viewers decode) — https://gbdev.io/pandocs/
