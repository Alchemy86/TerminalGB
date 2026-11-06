# Portability — what the core actually runs on

The README's "Embedding the core" section makes a claim that used to stop one step short:
`--no-default-features` builds the emulator with **one third-party dependency**, and it
*type-checks* for wasm32, Windows, aarch64 Linux and macOS. A type check cannot tell you
that a 32-bit machine has no 64-bit atomics, that a `usize` leaked into a file format, or
that a shift is architecture-dependent. **Running it can.**

This page records what has been run, on what, and what it cost — and says plainly which
rows are still only a compile.

The Linux, PSP, RISC-V, FFI and Windows rows below were measured on 2026-08-15; this revision
re-confirms them against current `main` (post the v0.3.0 release, 2026-11-06) and adds the two
rows that did not exist yet — the `embedded` row and the M5Stack PaperColor underneath it,
both from the week of 2026-09-15. Re-checked for this pass: `i686-unknown-linux-musl` still
builds under `rust-lld` and still runs frame-identical (`make test-cross`, same frame hash as
below); `aarch64`/`armv7` still build under the same linker but could not be *run* here — this
machine has no `qemu-aarch64-static`/`qemu-arm-static` installed, so `make test-cross` skips
them loudly rather than passing silently, same as it would for anyone else without the package;
`make embed` and `cargo tree --no-default-features --features embedded` both still hold (one
third-party crate, either way). Reproduce the Linux rows with `make test-cross`; the PSP with
`tools/psp-build.sh`; the PaperColor with `tools/m5paper-build.sh`.

---

## 1. The board

| Target | Status | Evidence |
|---|---|---|
| `x86_64-unknown-linux-gnu` | **runs** — the development platform | everything |
| `aarch64-unknown-linux-musl` | **runs**, frame-identical | `make test-cross`, under `qemu-aarch64` |
| `armv7-unknown-linux-musleabihf` | **runs**, frame-identical | `make test-cross`, under `qemu-arm` |
| `i686-unknown-linux-musl` | **runs**, frame-identical | `make test-cross`, natively — reconfirmed 2026-11-06 |
| `mipsel-sony-psp` (**PlayStation Portable**) | **runs Pokémon Blue at 59.7275 fps** | §3, under PPSSPP |
| `xtensa-esp32s3-none-elf` (**M5Stack PaperColor**, `embedded` feature, no host at all) | **runs Pokémon Blue on real hardware** — cartridge in flash, 0 bytes of RAM for the megabyte, 12 fps | §4a, on the maintainer's own device |
| `wasm32-unknown-unknown` | **runs** in a browser, with audio | `docs/web-audio.md` |
| `riscv32imafc-esp-espidf` (**32-bit RISC-V**) | **builds clean** at `--no-default-features` with no source changes, and under QEMU produced the same frame hash as x86-64 on **both** picture engines | a one-off run, *not* covered by `make test-cross` — see the note below |
| **C / Go static library** (`ffi` feature) | **builds** — `libterminalgb.a` + `terminalgb.h` | `make ffi-build`; `docs/components/frontends.md` |
| `x86_64-pc-windows-msvc` | **builds and smoke-tests** on a real Windows runner | `.github/workflows/release.yml` |
| `x86_64-apple-darwin`, `aarch64-apple-darwin` | **type-checks only** | `cargo check`; nobody has linked or run it |
| `armv7-sony-vita-newlibeabihf` (**PS Vita**) | not attempted — *the cheapest console left*, §5 | — |
| `armv6k-nintendo-3ds`, `aarch64-nintendo-switch-freestanding` | not attempted, §5 | — |
| any big-endian machine | **never run**, §6 | — |
| bare metal / `no_std` | **runs** — `--features embedded`, §7 | the PaperColor row above |

**`std` is no longer required.** That used to be flat fact: `Vec`, `Box<dyn …>`,
`std::sync::Mutex` in the FFI shim and `PathBuf` in the battery layer were all load-bearing, and
this section said bare metal was a rework away. The rework happened — `--no-default-features
--features embedded` builds `#![no_std]` + `alloc` and it is what runs on the PaperColor above —
so read it as **done**, and §7 as a record of what that cost rather than a prediction. Embedded
*Linux* needs none of it; that is what the aarch64/armv7 rows above already were.

---

## 2. `make test-cross` — running the core on other architectures

`tools/run-cross.sh` builds `--no-default-features --example headless` for each target,
runs it on a real cartridge, and requires the result to equal this machine's **exactly**:

```
== host x86_64: building the reference
   frame hash 0x01acf78fe505f31c
   state bytes 142102

== aarch64-unknown-linux-musl
   PASS: identical to the host
== armv7-unknown-linux-musleabihf
   PASS: identical to the host
== i686-unknown-linux-musl
   PASS: identical to the host
```

Two lines are compared and both earn their place. **`frame hash`** is every pixel of the
frame after 300 frames of dmg-acid2. **`state bytes`** is the save-state length: it is
142,090 on 32-bit and 64-bit alike, which is the observable form of `savestate`'s
`Writer::usize` storing a `u32` — a save state written on a phone loads on a desktop.

*How it needs nothing installed.* The linker is **`rust-lld`**, which every rustup
toolchain already ships, and the targets are all **musl**, for which rustup ships a
prebuilt `libc.a`. No C cross-toolchain is involved. The only external requirement is
`qemu-<arch>-static` to execute a foreign binary; a missing one **skips that target
loudly** rather than passing it silently. Without root:

```sh
dnf download qemu-user-static-aarch64
rpm2cpio qemu-user-static-*.rpm | cpio -idmu
GB_QEMU_DIR=$PWD/usr/bin make test-cross
```

*Teeth, proven.* Changing `examples/headless.rs`'s FNV constant to
`0x0000_0100_0000_01b3usize as u64` — a bug that is invisible on the host and wrong on any
32-bit machine — turns armv7 and i686 red and the script exits 1. The host build does not
notice.

**The RISC-V row is a one-off measurement, not a gate.** `riscv32imafc-esp-espidf` is a
microcontroller target and needs the ESP fork of the toolchain, so it is not in
`tools/run-cross.sh` and CI does not re-run it: nothing here would notice if it stopped
building. It is on the board because it *was* run and the frame hash *did* match — but read
it as "this happened once" rather than as "this is checked", which is the same standard the
macOS row is held to.

*Not in the list, deliberately:* `riscv64gc-unknown-linux-musl`. The emulator compiles for
it, but this package declares `crate-type = ["cdylib", "rlib", "staticlib"]` for the FFI
build, and a musl target that *supports* `cdylib` then tries to link one against a shared
libc that rustup's musl sysroot does not ship. The three targets above escape it only
because rustc drops `cdylib` for them with a warning. That is a packaging quirk of this
repository, not a finding about RISC-V.

---

## 3. The PlayStation Portable

**It runs Pokémon Blue at full speed.** Aspect-correct and full-height on the 480x272
panel, from the memory stick, with the game's own battery save beside it — **59.7275 fps
locked, with 40 % of the frame budget spare**, where it used to manage 42.

That took **one instrument and two changes to the emulator core**, and both changes leave
every frame byte-identical to the desktop build:

* a **per-tile background renderer** in place of the per-pixel one (`Gpu::draw_map_run`),
  which is worth 3.5x on x86-64 as well;
* **idle-cycle batching** for a halted CPU (`MemoryManagementUnit::idle_mcycles`) — Blue
  spends 82 % of its `exec` calls sitting in `HALT`, and each of those idle M-cycles was
  paying for the whole per-instruction pipeline.

It is **not** full speed in every workload, and §3.7 says exactly where the wall is: a
battle text box, where the game busy-polls the joypad and never halts, reaches 50 fps.

![dmg-acid2 on a PSP](shots/psp-dmg-acid2.png)

*dmg-acid2 rendered by TerminalGB running on a PSP. The frame hash printed on every line
is `0x01acf78fe505f31c` — see §3.2.*

### 3.1 What was measured, and on what

Everything in this section is **PPSSPP 1.20.4 on this desktop, not a PSP**. Nobody here has
a PlayStation Portable, so **no number below has been seen on real hardware** and that is a
gap, not a footnote. Three things say the figures are not merely PPSSPP's frame rate:

* The emulator reads `sceKernelGetSystemTimeWide`, and moving the CPU from 222 MHz to
  333 MHz changed the reported cost by **exactly 1.5000x** (32.537 → 21.681 ms/frame).
  A wall-clock number would not scale with the emulated clock like that.
* **The benchmark is bit-reproducible.** Two runs of `--profile` on the same build and the
  same save state produced identical means to the hundredth of a millisecond in every row,
  on a machine whose load average moved between them. That matters here: this box runs
  several agents at once, and `docs/link-cable.md` records a *different* measurement on it
  collapsing from 19/20 to 1/10 as the load went from 5 to 49. A cycle count does not care.
* Which also means it is a **cycle count divided by a clock**, and PPSSPP does not model
  the PSP's caches. Our working set does not fit in a 32 KB / 16 KB cache, so **real
  hardware is more likely to be slower than these numbers than faster.**

**The interactive figures are not in that protected class, and this is worth knowing before
quoting one.** The in-game statistics line measures a loop that makes PSP syscalls —
`sceCtrlPeekBufferPositive`, `sceKernelDelayThread`, `sceGuSync` — whose emulated cost
PPSSPP does not derive purely from cycles. Under host load those windows wander by several
milliseconds while the `--profile` bench does not move at all. **Quote the bench for
component costs; quote the interactive line for sustained fps, and say what the load was.**

Configuration: `mipsel-sony-psp`, CPU at 333 MHz, `standard` picture mode (the cheap
scanline renderer — `identical` was never in play and would be hopeless here at ~8.8x),
no APU installed, `opt-level = 3` with fat LTO and one codegen unit.

**Two workloads, both reproducible, both from a save state beside the cartridge**, because
a benchmark somebody has to hold a button through is a benchmark nobody re-runs:

| | how it is built |
|---|---|
| **overworld** | `shiny_hunt --rom blue.gb --make-state route1.gbss` — a new game walked to Route 1, standing in grass |
| **battle** | `gen1_battle --rom blue.gb --preset charizard-vs-pidgey --save-state battle.gbss --save-at 900` — "Wild PIDGEY appeared!" |

Sustained over real running, load average ~7, uninstrumented shipping build:

| Workload | before | after |
|---|---|---|
| overworld | 42.01 fps, work 23.8 ms (emu 22.3 + present 1.5), worst 24.5 ms | **59.73 fps, work 9.4-9.9 ms (emu 7.9-8.5 + present 1.4-1.5), worst 11.5-12.8 ms, 0 of 120 frames over budget in every window** |
| battle | 40.60 fps, work 24.6 ms (emu 23.2 + present 1.5), worst 25.3 ms | **50.0 fps, work 19.6 ms (emu 18.1 + present 1.5), worst 20.4 ms, every frame over budget** |

The budget is **16.743 ms**. "before" is `main` at `ecee7ef` measured on the same two save
states through the same frontend; the overworld row reproduces the previous crew's
published 41.9 fps / 23.9 ms exactly, which is what says the two workloads are equivalent.

**Say which battle.** That state is a battle *text box*: the game is waiting for A, and
Blue waits by busy-polling the joypad. It never halts — `halt/f` is **0** — and it spends
16,689 of a frame's 17,556 M-cycles on memory accesses. It is the worst case this emulator
has, not the average one, and it is quoted because the worst case is the honest number.

### 3.2 The determinism is the proof the port is exact

The per-frame hash printed by the PSP build is `0x01acf78fe505f31c`. It is:

* **identical on every frame** of a 1,500-frame run (dmg-acid2 is a static picture), and
* **identical to the x86-64 desktop build**, and to aarch64, armv7 and i686.

That is a stronger claim than "it looks right". A Game Boy emulator has a great many ways
to be *nearly* correct — one dot of PPU phase, one M-cycle of timer, one bit of an APU
register — and every one of them changes the frame. Five machines across three
architectures and two pointer widths producing the same 23,040 pixels means the port
changed the *host*, not the *emulation*.

### 3.3 The one portability bug, and why only building for the target found it

The core did not compile for `mipsel-sony-psp`. One error, in one line:

```
error[E0432]: unresolved import `std::sync::atomic::AtomicU64`
```

`sound::host_status` counted audio-device errors in an `AtomicU64`. **A 32-bit target need
not have 64-bit atomics, and MIPS32 does not** — `std::sync::atomic::AtomicU64` does not
exist there at all. It is `AtomicU32` now, widened to `u64` on read so the published
`Snapshot` type is unchanged; a counter of audio-device errors cannot reach 2^32 in a
session.

This is the kind of defect the rest of this page exists to catch, and note what did *not*
catch it: armv7 and i686 are 32-bit and both build fine, because both have 64-bit atomics
(`ldrexd`, `cmpxchg8b`). Neither `cargo check --target` nor a 32-bit run would have found
it. **Only building for the actual target did.** `host_status` is compiled on every build
including the embedding one, so this made the whole emulator unbuildable for a class of
target while every gate stayed green.

### 3.4 What it took to get `std` on a PSP

`mipsel-sony-psp` is **Tier 3 and `no_std` only** in upstream Rust. That is not a
formality — asking stock nightly for a std build fails *inside std itself*, before it ever
reaches our code:

```
error[E0432]: unresolved imports `crate::sys::thread_local::key::LazyKey`, `…::set`
error[E0425]: cannot find function `fill_bytes` in module `sys`
error: could not compile `std` (lib) due to 14 previous errors
```

So `std` comes from a third-party **sysroot overlay**
([AndrewAltimit/rust-psp](https://github.com/AndrewAltimit/rust-psp), MIT, a fork of
[overdrivenpotato/rust-psp](https://github.com/overdrivenpotato/rust-psp)) which
reimplements std's internal platform layer against PSP syscalls —
`sceKernelCreateThread` for `std::thread`, `sceKernelLwMutex*` for `Mutex`, `sceIo*` for
`std::fs`, `sceKernelGetSystemTimeWide` for `Instant` — and rebuilds std from it with
`-Z build-std`. `std::net` and `std::process` return `ErrorKind::Unsupported`; neither is
in `--no-default-features`.

**Those internals are unstable with no compatibility promise, and the overlay rots.** On
nightly-2026-08-14 it does not compile (`sys::fs`'s `set_permissions_nofollow` signature
moved, among 18 errors); on the nightly its own CI pins, `nightly-2026-04-14`, it builds
first time. `tools/psp-build.sh` pins the nightly *and* the overlay commit for that reason,
and bumping either is real work rather than a version bump. That is the honest cost of the
PSP: **not a target you add, a toolchain you maintain.**

What our code needed on top of all that was nothing at all — the emulator crate compiles
unmodified once `AtomicU64` is gone, and `#![feature(restricted_std)]` is required only in
the *binary* crate (`platform/psp/src/main.rs`), never in the library.

### 3.5 The frontend

`platform/psp/` is ~350 lines and uses only the public embedding API. It is **not a
workspace member and must never become one** — it needs nightly, `-Z build-std` and a
third-party std overlay, and none of that may reach `make embed`, `cargo test` or a
`cargo install`. `make embed` still reports exactly one third-party dependency.

* **Ask for 333 MHz.** A PSP boots at 222 MHz. `scePowerSetClockFrequency(333, 333, 166)`
  is one call and is worth exactly 1.5x — the single largest lever there is.
* **Let the GE scale the picture.** The frame is copied row-wise into a 256x256 staging
  texture in *cached* RAM and drawn as one quad. That costs **1.5 ms**; the emulation
  either side of it costs 22.4. Writing per-pixel into uncached VRAM instead was measured
  at the same 1.5 ms *for an unscaled 1:1 blit*, so the hardware path is the one that
  scales for free.
* **Aspect is corrected inside the texture, not by shrinking the quad.** The picture is
  written at column 47 of the staging texture and 254 texels are sampled across the whole
  panel, so the untouched black texels become the pillarbox. Measured: the picture lands on
  screen columns 89..391, which is 160 x 272/144 = 302 px centred, to the pixel.
* **A ROM picker off the memory stick**, `std::fs::read_dir` over
  `ms0:/PSP/GAME/terminalgb/roms` (then `ms0:/roms`, then `host0:/roms`).
* **Save states**: `<rom>.gbss` beside the cartridge is loaded at boot, L saves, R loads.
  That is also what makes a benchmark reproducible — the overworld and a battle are
  different loads and neither is reachable by mashing buttons.
* **Battery saves work.** `Gameboy::try_new_with_model(rom, Some(path), Auto)` wires the
  battery to `<rom>.sav` on the memory stick. There is no `$HOME` on a PSP and the core
  never looks for one — that is the embedding claim paying off. `fs::rename`, which the
  battery's atomic write depends on, *is* implemented by the overlay; the directory
  `sync_all` beside it is not, and is already `let _ =` in `src/mbc/battery.rs`.

### 3.6 Testing under PPSSPP — the keyboard map that reads as a broken build

**PPSSPP's default keyboard mapping does not spell the buttons the way you would guess**,
and the failure looks exactly like a frontend that ignores input. From its own
`controls.ini` (Android keycodes):

| PSP button | PPSSPP default key |
|---|---|
| **Cross (X)** | **Z** |
| **Circle (O)** | **X** |
| Square | A |
| Triangle | S |
| Start | Space |
| Select | Enter |
| D-pad | arrow keys |

So a prompt that says "press X" sends a tester's press to **Circle**. The picker therefore
names the *button glyph* and the PPSSPP key beside it, and accepts **Cross, Circle or
Start** — a menu has no reason to be strict, and "nothing happened" is the worst thing a
first launch can do.

![the PSP ROM picker](shots/psp-picker.png)

Two more traps worth writing down:

* **`Peek` the pad, do not `Read` it.** `sceCtrlReadBufferPositive` blocks until the next
  VBlank sample; `sceCtrlPeekBufferPositive` does not. A loop with its own pacer wants the
  second, and the first cost a whole frame of apparent emulation time — §3.8.
* **Initialise the pad.** Without `sceCtrlSetSamplingCycle(0)` +
  `sceCtrlSetSamplingMode(Digital)`, `sceCtrlReadBufferPositive` returns whatever the
  buffer held — observed here as a ROM picker scrolling by itself with nobody touching it.
* **XTEST cannot drive PPSSPP here.** Synthetic X11 key events (`python-xlib`,
  `xdotool`-style) are accepted by the X server and never reach the flatpak's SDL window.
  Injecting through the kernel does work: `ydotoold` on a short socket path
  (`/dev/uinput` is group `input`, so no root is needed if you are in it), then focus the
  window first — a kernel-level key goes to whatever has focus, including the developer's
  own terminal.

### 3.7 Where the frame goes now, and where the wall is

**A single aggregate cannot be optimised, only guessed at.** The first thing this campaign
built was the instrument: `tools/psp-build.sh --profile` replaces the interactive loop with
a fixed-length benchmark (`platform/psp/src/main.rs::bench`) that runs the same frames under
several configurations and prints the split.

**Why it is built that way.** The PSP's only clock costs **0.785 µs a call** here — measured,
100,000 reads in 78.5 ms — and quantises to 1 µs. One `MemoryManagementUnit::bus_tick` is
smaller than the measurement of it and there are ~17,000 of them a frame, so per-call
timing is out; the Allegrex CP0 counter was tried and does not advance usefully under
PPSSPP. Whole *frames* are ~10,000 µs, where both problems vanish. So every row is a
whole-frame time and a component's cost is the difference between a run that does its work
and one that does not (`crate::profile`, behind a cargo feature that is off in every
shipping build and adds no dependency). Each row also carries the frame's executed-instruction
count — **an A/B whose instruction counts differ is not an A/B**.

400 frames a row, warm 60:

| | overworld | battle |
|---|---:|---:|
| emulation | **8.43 ms** | **18.67 ms** |
| — pixel rendering | 3.42 (41 %) | 3.41 (18 %) |
| — per-access bus (`bus_tick`) | 0.90 (11 %) | 3.90 (21 %) |
| — everything else | 4.10 (49 %) | 11.36 (61 %) |
| present (the GE) | 1.45 | 1.44 |
| *of "everything else": the timer* | 1.04 | 1.56 |
| *of "everything else": the STAT line* | 0.17 | 0.34 |
| an APU, if one were installed | +1.73 | +3.44 |
| instructions / frame | 2,177 | 6,719 |
| halted M-cycles / frame | 13,260 | **0** |

The two "of everything else" rows overlap the one above them — `no timer` removes the
timer from `bus_tick` as well as from `do_cycle` — so they are marginal costs, not a
partition, and they move by a tenth of a millisecond between runs on a shared box.

**The finding that decided the campaign: Pokémon Blue's CPU is halted for 13,260 of a
frame's 17,556 M-cycles in the overworld — 82 % of every `exec` call — and each of those
idle M-cycles paid the whole per-instruction pipeline** (the VRAM-DMA test, the HBlank
latch, the timer, OAM DMA, the serial drain, the PPU, four interrupt folds) to advance the
machine by four dots. On a wide out-of-order desktop that is noise. On an in-order MIPS it
was two thirds of the frame.

`MemoryManagementUnit::idle_mcycles` computes how far the machine can be advanced before
any peripheral could raise an interrupt — the PPU's next mode edge, the timer's next `TIMA`
overflow — and runs that many M-cycles in one step, through the very same code
`MemoryManagementUnit::do_cycle` already runs for a six-M-cycle instruction. Nothing is
skipped; the only thing that moves is when the CPU looks at `IE & IF`, and the batch stops
**on** the edge so it looks at exactly the same M-cycle it always did. Measured: 13,260
idle M-cycles arrive in **349 batches** instead of 13,260.

**The proof that it changed nothing is in the table the benchmark prints.** The
`no idle batch` row steps one M-cycle at a time as before, and it reports the *same frame
hash* as the baseline — `0x93a3ea1446afa04c` — while costing **18.95 ms against 8.43**.
Same picture, 10.52 ms cheaper.

Attribution on the overworld, all through the same instrument:

| | emulation, ms/frame |
|---|---:|
| `main` at `ecee7ef` | 22.3 |
| + per-tile background renderer | 18.95 |
| + idle-cycle batching | **8.43** |

**The wall, stated plainly.** A battle text box never halts, so idle batching buys it
nothing, and 11.36 ms of its 18.67 is the interpreter and the per-instruction MMU tail —
about **560 MIPS cycles per emulated instruction**. Removing the per-access bus stepping
entirely would put that workload at 16.2 ms, inside the budget — and that is precisely the
`mem_timing` accuracy this repository is built on, so it is not available. **50 fps in the
busiest loop Pokémon Blue has is where this stops without a real interpreter campaign, and
the next lever is a dispatch-level one, not another structural trick.**

One shape worth recording for whoever takes it: `bus_tick` need only step the peripherals
before an access that can *observe* them (`$FE00` and above), and could accumulate the rest
to the next such access or to the instruction's end. That would recover most of the
non-timer half of the 3.90 ms — but not the timer's own per-M-cycle edge detection, which
has to run either way, so it is worth ~2 ms and does not on its own close 1.17x.

### 3.8 Four defects the speed uncovered, all invisible while it was slow

Every one of these was harmless at 42 fps and wrong the moment the emulator had a frame
budget to blow. They are the reason the "after" figures above are what they are.

1. **The frontend blocked on the pad.** `sceCtrlReadBufferPositive` *waits* for the next
   pad sample, and with `sceCtrlSetSamplingCycle(0)` that is once per VBlank. At 23.9 ms a
   frame the sample was always already there and the call returned at once; at 8 ms it sat
   in the remaining ~7 ms, **charged the wait to emulation** (the loop reported 15.2 ms for
   8.3 ms of work) and pinned the machine to the panel's 59.94 Hz instead of the DMG's
   59.7275. It is `sceCtrlPeekBufferPositive` now, which is what a self-paced loop wants.
2. **The statistics line excluded its own I/O.** The window's start was re-read *after* the
   report was written and the battery flushed, so the frames those delayed were counted
   against a window that had not paid for them: the loop reported **61 fps against a
   59.7275 Hz pacer it was in fact keeping**. The next window now begins on the same
   reading the last one ended on.
3. **A forced battery flush every two seconds** cost a **59 ms frame** on the memory stick,
   for no durability `Gameboy::frame`'s own autosave was not already providing. Removed.
4. **A cartridge with no save file yet refused to start.** `BLUE.GB: could not read the
   existing save file` — for a file that had never existed. The PSP's `std` overlay reports
   `ErrorKind::Uncategorized` for a missing file from both `read` *and* `metadata`, so
   `Path::exists` is no help either, and `src/mbc/battery.rs`'s deliberate "a save that
   cannot be read is never overwritten" rule caught a save that was not there. It asks the
   **directory listing** now, which is the one question that does work on that platform,
   and fails towards "present" so the safety property is untouched. **This was a first-launch
   bug for every PSP user** and nothing in this repository could have found it: no harness
   runs on a filesystem whose `std` cannot classify a missing file.

Points 1-3 are host-side and cost nothing on a desktop; point 4 is a one-line change in the
core with a unit test that needs no PSP.

## 4. Building and running it yourself

```sh
./tools/psp-build.sh                 # → target/psp/EBOOT.PBP
./tools/psp-build.sh --profile       # → the same, but a fixed-length benchmark (§3.7)
```

Copy `EBOOT.PBP` to `ms0:/PSP/GAME/terminalgb/` and cartridges to
`ms0:/PSP/GAME/terminalgb/roms/`. For PPSSPP, that path is under
`~/.var/app/org.ppsspp.PPSSPP/config/ppsspp/PSP/GAME/` for the flatpak.

Per-second statistics are written to `<rom>.stats.txt` **beside the cartridge**, because
once the GE owns the framebuffer the debug console is gone and a number nobody can read is
a number nobody can check.

---

## 4a. The M5Stack PaperColor — no host at all

The PSP above still has `std`: a filesystem, threads, a clock. The PaperColor doesn't.
`platform/m5paper/` boots the emulator core through the `embedded` cargo feature —
`#![no_std]` + `alloc`, nothing else — on an ESP32-S3 (Xtensa), with a 1 MiB Pokémon Blue
cartridge built into the firmware's flash and a six-colour E Ink panel standing in for a
screen. It is the board's one `bare metal / no_std` row, and it is a real device
(MAC `28:84:85:43:d2:84`), not a simulator.

```sh
espup install --targets esp32s3                       # once; upstream rustc has no Xtensa backend
TERMINALGB_ROM=~/roms/blue.gb tools/m5paper-build.sh flash
```

Measured off the device over USB serial (`docs/m5paper-emulator.md`):

| | Pokémon Blue | dmg-acid2 |
|---|---:|---:|
| cartridge | 1024 KiB, **in flash, 0 bytes of RAM** | 32 KiB |
| emulation | 1000 frames in 81.051 s — **12 fps, 20 % of a real Game Boy** | 30 frames — 10 fps |
| one frame onto the panel | **15.953 s** | 15.953 s |

Two things follow from that table rather than from prose: the cartridge really does cost no
RAM — `include_bytes!` lands it in `.rodata`, the chip memory-maps that out of flash, and the
`embedded` feature holds mappers over a borrowed `&'static [u8]` instead of a reference-counted
one — and **the panel, not the CPU, is the bottleneck**: 15.953 s to draw one E Ink frame
dwarfs the 0.081 s it takes to emulate 1,000 of them, so 12 fps is this build's honest
emulation speed, not a claim about how fast a picture appears. That is the opposite shape from
the PSP, which hits the DMG's own 59.7275 fps and spends its slack on everything *but* the
pixels; the right comparison is "does the target have `std`", not "which one is faster" — see
§5.

Getting a `Gameboy` to exist at all here cost a real debugging campaign, not a flag: the chip's
~335 KiB of internal DRAM could not hold the panel framebuffer, the emulator's heap, *and* the
stack `Gameboy`'s constructor needed in one pass, and a stack-guard fault (`sp=0x3fc8cb90`
against `top=0x3fcdb700`, a 322,928-byte prologue) is what found it before a word reached the
console. The heap and the panel buffer moved to external PSRAM, and one framebuffer allocation
moved from the stack to `vec!` (which reaches `alloc_zeroed` instead of copying). Full account,
including the five ways four Game Boy greys get drawn on a six-ink, no-grey panel:
[`docs/m5paper-emulator.md`](m5paper-emulator.md) · [the bring-up that came before
it](m5paper-first-boot.md) · [the panel-speed campaign](m5paper-refresh.md).

**Not in CI, same standard as the PSP and the RISC-V row above**: it needs the `espup` Xtensa
toolchain fork (upstream rustc has no Xtensa backend at all) and real hardware to flash, so
nothing here would notice if it stopped building. `platform/m5paper/` is not a workspace member
for the same reason `platform/psp/` isn't — see `AGENTS.md`.

---

## 5. The consoles that were not attempted, and what each would cost

The Rust target must exist and it must have `std`; that single question separates these
sharply, and it is not the answer people expect.

| Console | Target | Upstream `std` | Cost |
|---|---|---|---|
| **PS Vita** | `armv7-sony-vita-newlibeabihf` | **yes, Tier 3 with full std** | **the cheapest console left** |
| Nintendo 3DS | `armv6k-nintendo-3ds` | work in progress | needs `ctru-rs` + devkitARM |
| Nintendo Switch | `aarch64-nintendo-switch-freestanding` | `no_std` only | as bad as the PSP, plus §7 |
| PSP | `mipsel-sony-psp` | `no_std` only | done — §3, and it needed a std overlay |

**The cheapest console port is the Vita, not the PSP.** `armv7-sony-vita-newlibeabihf`
carries full std *upstream*, so there is no sysroot overlay to maintain and no pinned
nightly that rots — the thing that makes §3.4 expensive simply is not there. It is also a
600 MHz quad-core ARM with a 960x544 screen, i.e. comfortably more machine than the 1.4x
the PSP is short by, and it plays PSP titles, so a Vita owner gets both. It needs VitaSDK
and `cargo-vita` for packaging, and `platform/psp/`'s shape — pick a ROM, blit a texture,
read a pad, save beside the cartridge — ports across almost unchanged.

The **3DS** is next: `ctru-rs` is a maintained ecosystem and the std story is real, but it
is `?` upstream and needs devkitARM. The **Switch** is the same shape of problem as the PSP
was, with a freestanding `no_std` target, and should not be attempted before §7.

None of these were attempted here and none should be quoted as "nearly working".

---

## 6. Big-endian: audited, never run

**No big-endian machine has run this emulator.** That matters for the GameCube, Wii and
Wii U, which are big-endian PowerPC and which mGBA supports.

It could not be tested here: rustup ships no std for a big-endian *musl* target, and the
big-endian *gnu* targets it does ship (`powerpc-unknown-linux-gnu`, `s390x-unknown-linux-gnu`)
cannot be linked without a glibc sysroot this machine does not have.

What an audit of the embedding build found, which is weaker evidence than a run and is
offered as exactly that: the save-state codec is explicitly little-endian byte by byte
(`src/savestate.rs`), there is no `transmute` and no reinterpreting slice cast anywhere in
the core, and the only native-endian code is `src/upscale/`, where `u32::from_ne_bytes` and
`to_ne_bytes` are used as a **symmetric** pack/unpack of one RGBA pixel that never crosses a
boundary — channels are extracted with `to_ne_bytes`, never with shifts. So big-endian is
*expected* to work. Until something runs, that is a prediction.

---

## 7. `no_std` and bare metal — done, and what it cost

This section used to be a list of what stood in the way: `Vec` and `Box<dyn …>` throughout,
`std::sync::Mutex` in the FFI shim, `std::path::PathBuf` and `std::fs` in the battery layer,
one `std::env::var` and one `SystemTime::now`. **That rework happened** —
`--no-default-features --features embedded` (added 2026-09-15) builds the crate
`#![no_std]` + `alloc`, and it is what the M5Stack PaperColor in §4a actually runs — so what
follows is a record of how each item was closed, not a plan.

* `Vec` and `Box<dyn …>` — covered by `alloc`, no change needed.
* **Battery saves are gone, not stubbed.** `SavePath` is an *uninhabited* type under
  `embedded`, so `Option<SavePath>` can only ever be `None` and the compiler proves every
  mapper's save arm unreachable — a property check, not a promise the host has to keep.
  `Battery` becomes a no-op with the same surface the host side already had for "no save
  file". There is no `PathBuf`/`std::fs` left to need.
* **The FFI shim's `static Mutex` is simply not built** — `embedded` carries no FFI, no
  terminal/desktop/web frontends, no save *states*, and no telemetry, and `crate::plugin`
  becomes a no-op host so no plugin actually runs. `terminalgb-plugin-api` itself stays in the
  tree either way — it is first-party, has no dependencies of its own, and does not count
  against the "one third-party crate" figure, with or without `embedded`: re-checked here,
  `cargo tree --no-default-features --edges normal` and the same command with
  `--features embedded` both print the identical two-crate tree. Cargo.toml's own
  `embedded = [...]` feature comment is the canonical list of what `embedded` leaves.
* `std::env::var` answers `None` under `embedded` and every caller falls back to exactly the
  default it already had; an embedder passes `Model`, `RenderMode` and `SpeedFlags` in as
  arguments instead of `$GB_MODEL` et al. `SystemTime::now` is only reached through the
  battery layer, which no longer exists in this build.
* The ROM itself is borrowed (`&'static [u8]`) rather than reference-counted, so it can live
  in memory-mapped flash at zero RAM cost — see §4a — which an `alloc`-only port needed but
  was not on the original list above.

What it costs a host, stated rather than hidden: no battery saves, no `$GB_*` overrides, no
plugins, no save states, no bundled frontends. A `#[global_allocator]` is the one thing the
binary must supply. See the feature's own doc comment in `Cargo.toml` and
[the M5Paper port](m5paper-emulator.md) for a host that actually supplies it.

---

## 7a. Embedding the core

Take the emulator and leave every frontend behind:

```toml
[dependencies]
terminalgb = { git = "https://github.com/Alchemy86/TerminalGB", default-features = false }
```

That is the **whole** dependency tree at that feature set:

```console
$ cargo tree --no-default-features --edges normal
terminalgb v0.3.0
├── blip_buf v0.1.5
└── terminalgb-plugin-api v0.1.0 (crates/terminalgb-plugin-api)
```

One third-party crate — `blip_buf`, the APU's band-limited resampler — against 170 for the
default build. `terminalgb-plugin-api` is first-party, lives in this repository, has no
dependencies of its own, and exists so plugin crates depend on a contract instead of on the
emulator. CI enforces the whole paragraph.

### The API is small enough to fit here

```rust
use terminalgb::gameboy::Gameboy;
use terminalgb::{mode::Model, KeypadKey};

// You read the ROM. `None` for the path keeps cartridge RAM in memory and the
// emulator never touches the filesystem; `Some(p)` opts into `<rom>.sav`.
let mut gb = Gameboy::new_with_model(std::fs::read("game.gb")?, None, Model::Dmg);

loop {
    gb.frame();                     // one 59.7275 Hz frame
    let rgba: &[u8] = gb.image();   // 160 × 144 × 4, top-left first
    gb.keydown(KeypadKey::A);
    gb.keyup(KeypadKey::A);
}
```

Also there without any feature: `save_state()` / `load_state()`, `flush_battery()`,
`rom_title()`, the debug reads (`peek`, `peek_range`, `debug_pc`, `step_instruction`), the
[plugin API](plugins.md), the [pixel-art upscaler](upscaling.md), and
`enable_audio(Box<dyn AudioPlayer>)` — implement that one trait and the APU's samples are yours,
with no `cpal` anywhere in the build.

### Prove it rather than believing it

`make embed` runs the example below with `HOME`, `DISPLAY` and `WAYLAND_DISPLAY` unset:

```console
$ cargo run --release --no-default-features --example headless -- third_party/dmg-acid2/dmg-acid2.gb 240
rom            DMG-ACID2
checksum       0xa934
frames         240
frame hash     0x01acf78fe505f31c
state bytes    142102          # moves with savestate::FORMAT_VERSION
state re-run   identical after 60 more frames

                ########
            ####........####
          ##................##
        ##..-+++++....+++++-..##
        ##..--++++....++++--..##
      ##........................##
      ##..........-##-..........##
      ##..........-##-..........##
      ##........................##
        ##..##............##..##
        ##....############....##
          ##................##
            ####........####
                ########
```

That frame hash is the same one `make test-cross` requires on aarch64, armv7 and i686 — and the
same one the core produced on 32-bit RISC-V.

### What the core still asks of a host

It needs `std` (§7), reads exactly one environment variable (`GB_MODEL`, and only when the
console is `Model::Auto` — `new_with_model` bypasses it), and reads a wall clock in one place:
writing or reading an **MBC3 clock cartridge's** battery file, to cover the time the console was
switched off. With no save path nothing here reads a host clock at all.

---

## 8. Where this is enforced

* `make test-cross` — builds and **runs** the core on aarch64, armv7 and i686, and fails if
  any of them disagrees with the host by one pixel. In CI's `embed` job, which also keeps
  the existing type-check sweep for the targets that cannot be run there.
* `make embed` — the core alone, one third-party dependency, a cartridge run with no
  `$HOME` and no display.
* `make check-wasm` — the wasm build, which GitHub CI cannot protect on its own because
  merges land locally first.
* `tools/psp-build.sh` — the PSP, pinned. **Not in CI**: it needs a pinned nightly, a
  third-party checkout and `-Z build-std`, and it produces an artefact nothing here can
  execute. Run it by hand when `platform/psp/` or the core's host surface changes.
* `cargo check --no-default-features --features embedded` — **not currently gated anywhere**
  (neither `Makefile` nor CI greps for it); re-checked by hand for this page, 2026-11-06. A
  change to `src/` that quietly broke `no_std` would not be caught until the next such check.
* `tools/m5paper-build.sh` — the PaperColor. **Not in CI**, same reason as the PSP: it needs
  the `espup` Xtensa toolchain fork and a real device to flash. `tools/m5paper-build.sh test`
  does run `platform/m5paper/src/gbscreen.rs`'s dithering tests without `esp-hal`, so that one
  slice is checkable without hardware; the emulator-on-device numbers in §4a are not.
