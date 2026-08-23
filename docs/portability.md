# Portability — what the core actually runs on

The README's "Embedding the core" section makes a claim that used to stop one step short:
`--no-default-features` builds the emulator with **one third-party dependency**, and it
*type-checks* for wasm32, Windows, aarch64 Linux and macOS. A type check cannot tell you
that a 32-bit machine has no 64-bit atomics, that a `usize` leaked into a file format, or
that a shift is architecture-dependent. **Running it can.**

This page records what has been run, on what, and what it cost — and says plainly which
rows are still only a compile.

Everything below was measured on 2026-08-15. Reproduce the Linux rows with
`make test-cross`; the PSP with `tools/psp-build.sh`.

---

## 1. The board

| Target | Status | Evidence |
|---|---|---|
| `x86_64-unknown-linux-gnu` | **runs** — the development platform | everything |
| `aarch64-unknown-linux-musl` | **runs**, frame-identical | `make test-cross`, under `qemu-aarch64` |
| `armv7-unknown-linux-musleabihf` | **runs**, frame-identical | `make test-cross`, under `qemu-arm` |
| `i686-unknown-linux-musl` | **runs**, frame-identical | `make test-cross`, natively |
| `mipsel-sony-psp` (**PlayStation Portable**) | **runs Pokémon Blue at 59.7275 fps** | §3, under PPSSPP |
| `wasm32-unknown-unknown` | **runs** in a browser, with audio | `docs/web-audio.md` |
| `x86_64-pc-windows-msvc` | **builds and smoke-tests** on a real Windows runner | `.github/workflows/release.yml` |
| `x86_64-apple-darwin`, `aarch64-apple-darwin` | **type-checks only** | `cargo check`; nobody has linked or run it |
| `armv7-sony-vita-newlibeabihf` (**PS Vita**) | not attempted — *the cheapest console left*, §5 | — |
| `armv6k-nintendo-3ds`, `aarch64-nintendo-switch-freestanding` | not attempted, §5 | — |
| any big-endian machine | **never run**, §6 | — |
| bare metal / `no_std` | **not possible without a rework**, §7 | — |

**`std` is required and `no_std` is not a flag away.** `Vec`, `Box<dyn …>`,
`std::sync::Mutex` in the FFI shim and `PathBuf` in the battery layer are all load-bearing.
Embedded *Linux* is fine — that is what the aarch64/armv7 rows above are. Bare metal is a
rework of the MBC and battery layers; see §7.

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

## 7. `no_std` and bare metal

Not a flag away, and here is the list:

* `Vec` and `Box<dyn …>` throughout — `alloc` would cover these.
* `std::sync::Mutex` in the FFI shim (`src/lib.rs` parks a `Gameboy` in a `static`).
* `std::path::PathBuf` and `std::fs` in the battery layer (`src/mbc/battery.rs`) — the
  cartridge-RAM persistence that `docs/testing.md` treats as a correctness feature, not a
  convenience.
* One `std::env::var` (`$GB_MODEL`, and only for `Model::Auto` — `Gameboy::new_with_model`
  bypasses it) and one `SystemTime::now` (only when an MBC3 clock cartridge's battery file
  is read or written).

`alloc`-only would need the battery layer restructured behind a storage trait the host
implements, and the FFI shim's `static Mutex` replaced. That is a real piece of design work
with a clear shape; it has not been done and should not be described as close.

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
