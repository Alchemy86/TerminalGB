# The host boundary — where a correct emulator still gets it wrong

← [The measured Game Boy](README.md)

Every other page here measures the emulated machine. This one measures the seam between that
machine and a real computer — the clock it is paced by, the build it was compiled into, the
device its samples reach, the terminal its pixels reach.

It exists because that seam has produced some of the most expensive defects in this project,
and **a ROM-driven suite is structurally blind to all of them**: the framebuffer is perfect,
the serial verdict is correct, and the emulator is unusable. The list of what no test ROM can
see is on [symptoms.md](symptoms.md#what-no-test-rom-can-see); this page is the measurements
behind it.

There is no hardware reference to cite for most of this. Where a contract *is* being violated
it is a **protocol or platform specification**, and those are cited in place.

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`6c20451`](https://github.com/Alchemy86/TerminalGB/tree/6c20451306663018288d34253c199fad507ae6f6) · regenerate with `make docs-links`, verify with `make docs-check`.

## A frame rate must come from a clock, never from a presentation callback

The browser build stepped one emulated frame per `requestAnimationFrame` callback.

`requestAnimationFrame` is specified as running its callbacks before the next **repaint**
([HTML, *Animation frames*](https://html.spec.whatwg.org/multipage/imagebitmap-and-animations.html#animation-frames)) —
i.e. at the display's rate. It is not a timer and never was. A DMG frame is
70224 cycles / 4.194304 MHz = **59.7275 Hz**.

| Display | Emulated speed |
|---|---|
| 60 Hz | 100.46 % — inaudible on its own, and **exactly enough to make an audio queue shed a cushion's worth of samples on a timer**: an audible skip every few seconds |
| 120 Hz | **200 %** — music, animation and timers all wrong, with no setting to change it |

*Caught by* — no test ROM, and none can: the emulated machine is entirely correct and a
framebuffer hash is identical whatever rate the frames are produced at. Now
`a_fast_display_does_not_speed_the_machine_up`, which reads **1200** emulated frames per 10 s
against the old loop and **~597** against the fixed one.

The fix is the same deadline accumulator the terminal frontend uses — advance by exact periods,
never reset the timer to "now" — capped at 8 frames so a backgrounded tab is *resynchronised*
rather than replayed. Two supporting facts worth knowing before writing one:

- **`std::time::Instant` panics on `wasm32-unknown-unknown`.** The clock is passed in, which is
  also what lets the loop pace on `AudioContext.currentTime` and share **one** clock with the
  audio consumer — zero structural drift, by construction rather than by tuning.
- Resetting a pacing timer to `now()` silently folds draw and poll overhead into the period.
  Both frontends did that once and ran a few per cent **slow**, which drained the audio queue
  and produced machine-gun gaps. Never reset a pacing timer to `now()`.

> **Detectable from outside, by anyone, on any emulator:** run a known number of emulated
> frames and compare against wall time. That is the whole check.

---

### The same rule from the other side: an input read that waits for the display

A PlayStation Portable samples its pad once per VBlank, and
`sceCtrlReadBufferPositive` **blocks until it has been sampled again**. So a loop that
calls it does not merely read the buttons: it waits for the panel, and inherits the panel's
59.94 Hz rather than the DMG's 59.7275.

*Why it took a fast emulator to find.* At 23.9 ms a frame — 1.4x over the budget — a new
sample was always already there and the call returned at once. The moment the core reached
8.3 ms the call started absorbing the other ~7 ms, and because it sat inside the
frame-timing brackets it reported **15.2 ms of "emulation" for 8.3 ms of work**. Both
symptoms — a machine locked to the display and a per-frame cost that is mostly waiting —
came from one line, and neither is visible in a framebuffer.

`sceCtrlPeekBufferPositive` returns the latest sample without waiting, which is what a loop
with its own deadline accumulator wants. *Conformance candidate:* **nothing inside a frame's
timing brackets may block on the display**, which is the input-side twin of the rule above
and of `docs/audio.md` §4c's "an `AudioPlayer` must never block".

Measured in [`docs/portability.md`](../portability.md) §3.8, together with three more
defects of the same shape that the same speed-up exposed: a statistics window that excluded
its own I/O and so reported **61 fps against a 59.7275 Hz pacer it was keeping**, a forced
battery flush every two seconds that cost a **59 ms frame**, and a first launch that refused
to start because that platform's `std` cannot say "no such file".

## A build configuration is part of the machine

The `wasm32` build had **no sound chip** — not a quiet one.

`MemoryManagementUnit::sound` and the whole `sound` module were `#[cfg(not(target_arch =
"wasm32"))]`, so `$FF10-$FF3F` read back `$FF`, `$FF76`/`$FF77` read back `$00`, and every write
was dropped.

*Symptom* — **the game appears to hang, not to be silent.** A cartridge can tell: `NR52` never
reports itself powered on, so a sound-driver initialisation loop spins for ever and the title
screen never advances. A player reads that as "the emulator is broken".

*Caught by* — **nothing here, and nothing could.** Every test-ROM harness, every external-suite
scout and all 646 unit tests run on the *host* target, where the APU was always present.
`make check-wasm` type-checks the build, and **a type-check cannot see a missing peripheral.**
There is still no `wasm32` test runner in this repository; that gap is open and is stated rather
than papered over.

*Root cause worth remembering — a stale dependency pin, not a design decision.* `blip_buf`
0.1.4 binds a C library (`blip_buf-sys` + `libc`, built through a `cc` script) and does not
cross-compile to `wasm32-unknown-unknown`; **0.1.5 is a pure-Rust rewrite with no dependencies
and no build script**, and the lockfile had resolved to it long before. The `cfg` that removed
the sound chip outlived its reason by however long that took.

The argument had already been written down for the *native* case — `sound::SilentPlayer` exists
because a machine always has an APU, sound card or not — and had simply never been applied to
the other target.

> **Two builds of one emulator that differ by a `cfg` are two emulators**, and this one differed
> by a whole peripheral. A conformance adapter must be able to say which target it was built
> for. **Any check that writes `NR52` bit 7 and reads it back is a one-line detector for the
> whole class.**

The same shape, one layer over: `get_mbc` used to force an **MBC3 for any cartridge given no
save path** — always the case for the web build, the FFI build and every embedder — so a
ROM-only or MBC5 game was mis-banked from its first bank switch *in the browser* while the
identical ROM was fine on the desktop. Caught by nothing, **because every harness passes a
path**. See [cartridges.md](cartridges.md#the-mapper-is-chosen-from-the-header-with-or-without-a-save-path).
The check that generalises: **the same cartridge must behave identically however the emulator
was configured.**

---

## ...and so is a host that never listens

The section above is about a build. This one is about the same missing peripheral reached a
different way, five years and four rediscoveries later: **`terminalgb-mcp` — this repository's
own agent interface — installed a sound chip only when given `--audio` *and* handed a real
output device.** Headless, which is how every agent runs it, the machine had none. So did every
example here that drove a real cartridge.

*Symptom* — **losing a battle ends the run, permanently.** Gen 1 does not end a run when your
last Pokémon faints: `HandleBlackOut` (`home/overworld.asm`) fades the screen out, halves your
money, heals the party and warps you to `wLastBlackoutMap`, which before Viridian is Pallet
Town. What was observed instead was an entirely black screen, `wIsInBattle` stuck at `$FF`,
`wCurMap` unchanged, and the CPU still executing — for **3,670-3,900 decisions**, about
thirty-four minutes of game time, under nine different input arms **including pressing nothing
at all**.

*The mechanism, and it is one instruction deep.* `HandleBlackOut`'s second instruction is
`call StopMusic`, and `StopMusic` ends in a spin on `wAudioFadeOutControl` that only the audio
engine can release. `FadeOutAudio` (`home/fade_audio.asm`) releases it by walking the master
volume down to zero — and it walks it down by **reading `rAUDVOL` (`$FF24`, NR50) back**,
decrementing both nibbles and writing it again, stopping only when the read-back is exactly
zero:

```
.counterReachedZero
	ld a, [wAudioFadeOutCounterReloadValue]
	ld [wAudioFadeOutCounter], a
	ldh a, [rAUDVOL]
	and a ; has the volume reached 0?
	jr z, .fadeOutComplete
```

A machine with no APU answers `$FF` to every read in `$FF10-$FF3F` and drops every write, so
the volume never falls, the flag is never cleared, and `StopMusic` spins behind an
already-faded-out screen for as long as anyone is willing to watch. The reported "only the
sound engine's RAM is moving" was not a symptom beside the bug — **it was the bug**: a wait
loop servicing audio and nothing else.

*Measured, three ways, one byte-identical save state.* `examples/blackout_probe.rs` loses a wild
battle with a party of one from a brand-new game and then presses nothing:

| | `wCurMap` | `wIsInBattle` | screen | party | `wAudioFadeOutControl` | `rAUDVOL` | frames |
|---|---|---|---|---|---|---|---|
| **no sound chip** | 38, unchanged | `$FF` | 1 shade, never came back | 0/20 | `$FF` — spinning | `$FF` | never, 3000+ |
| **with a sound chip** | **0 — Pallet Town** | `$00` | 4 shades, back at 114 | **20/20, healed** | `$00` | `$77` | 235 |
| **mGBA** | **0 — Pallet Town** | `$00` | back | — | `$00` | — | 203 |

The mGBA column is [`tools/blackout/`](../../tools/blackout/README.md): the same cartridge, the
same party bytes, the same opponent poked into `wCurOpponent`, on somebody else's emulator, so
the expected answer is not one this project supplied. **All four steps of the whiteout — the
fade, the map change, the party heal, the respawn — complete with a sound chip and none of them
complete without one.**

*Caught by* — **nothing, and nothing ROM-driven could.** The emulator is entirely correct
throughout: every register, every cycle, every pixel. What was wrong is the *machine the
cartridge was handed*, which is a property of the host and invisible to a framebuffer compare or
a serial verdict. It is the same blind spot as everything else on this page.

*What it did and did not cost.* `examples/pewter_chain.rs` installed a sound chip at all three of
its machine-construction sites and always had, so **the certified chain was never affected** —
its 213 blackouts at link 24 recovered correctly, which is why "a blackout is a legal detour" was
measurable at all. What was affected is everything that did not: `terminalgb-mcp`, and
`scenario.rs`, `longplay.rs`, `claude_plays.rs`, `shiny_hunt.rs` and eighteen more. Any death in
a run driven through one of those was unrecoverable when the cartridge says it should not have
been, and a "failure" recorded there may be this and not the agent.

> **A Game Boy has a sound chip, and `MMU.sound` being an `Option` is a performance affordance,
> not a model of a machine that exists.** Measured on Pokémon Blue, one P-core, interleaved:
> the chip costs **+0.3 %** of a frame on the `identical` picture engine and **+4.6 %** on
> `standard` with pixels off. That is what four rediscoveries have been buying.
>
> Changing `Gameboy::new` to install one by default would end the class outright and is a
> deliberate decision nobody has taken: it moves a documented core contract, every figure in
> [throughput.md](../throughput.md), and `examples/headless.rs`'s frame hash and state length.
> Until it is taken, the enforcement is check 1 of `testharness/blackout.rs` — a named list of
> every host that drives a real cartridge, each of which must install one. It is the only one of
> the three checks there that can catch a *new* harness falling in, which is what has happened
> four times.

---

## Audio scheduling: three constants, all wrong on the first try

Native and browser audio are the same problem with a different consumer, and the numbers do
**not** transfer between them. Every value below was measured in a real browser, and the wrong
version is recorded because it is the argument for the right one. Full working:
[`docs/web-audio.md`](../web-audio.md).

| Constant | First value tried | Measured result | Right value |
|---|---|---|---|
| scheduling lead | **2048** samples, by analogy with the desktop cushion | **~2 underruns per second** in Chrome, with the emulator itself at a perfect 59.6 fps | **4096** (93 ms at 44.1 kHz) — 0 underruns |
| queue bounds | the desktop's `start_fill()` / `high_water()` (1792 / 2816) | **~2,560 samples discarded per second** on an otherwise healthy run — and with the lead at 4096 the two **deadlock**, so not one sample ever plays | `TARGET_LEAD` / `4 × TARGET_LEAD` |
| priming | re-prime after every underrun | one stall becomes **self-sustaining**: ~2 underruns and ~2,560 dropped samples per second, indefinitely | prime **once**, never again |

**Why the desktop analogy fails, and it is the transferable part:** on the desktop a device
callback thread keeps pulling while the frame loop stalls, so the cushion only has to cover the
gap between *two* independent clocks. In a browser one animation-frame callback produces the
audio **and** schedules it, so a single stall starves both halves at once and the cushion must
cover the whole stall. **Where the latency lives decides how to size the buffer:** on the
desktop the queue *is* the latency; in the browser the scheduler owns it and the queue is only a
transfer buffer, so clipping the queue destroys exactly the backlog a recovery needs.

Two rules that fall out and are pinned by tests:

- **Never schedule in the past.** A late `start()` plays *immediately* and overlaps — a click,
  not the gap it really is.
- **Never pad a block with substituted samples.** The cursor is the output clock; padding slides
  it for ever. All of `n` samples or none of them.

*Measured in Chrome*, with a `ScriptProcessorNode` tapped onto the destination so the figures
describe what was **played** rather than what was scheduled: **311 s, 0 underruns, 0 drops,
59.6 fps, lead stable at 85–96 ms**, exactly one gap (the 96 ms priming silence at start-up) and
none after. Pitch 439.7 Hz against the ROM's programmed 439.84 — the estimator counts zero
crossings across the whole run including that start-up silence, and
439.84 × (311 − 0.096) / 311 = 439.70 exactly.

**The related native rule is on [apu.md](apu.md#two-rules-that-are-not-about-accuracy-at-all):
an `AudioPlayer` must never block, because it runs inside the emulated frame.** Making the sound
card the master clock measured **17.93 ms/frame against a 16.743 ms budget** with the no-audio
build at 0.29 — and **97.6 % of that was blocked, not executing.** Never put a second clock
inside the emulator's inner loop.

> **When you have a timing figure, separate "blocked" from "executing" before diagnosing it.**
> That measurement was misdiagnosed four times as the cost of the T-cycle-stepped APU, which in
> fact costs ~0.16 ms/frame. A tool that prints wall time beside `CLOCK_THREAD_CPUTIME_ID`
> answers it in one run.

---

## Presentation cost is not a function of output size

The terminal frontend re-encoded the frame at the size of the display rectangle every frame.

| At a 1568×1408 rectangle | |
|---|---|
| Kitty | **39.7 ms / 11.6 MB per frame** — 25 fps |
| iTerm2 | **86.4 ms per frame** — 12 fps |
| transmitting 160×144 and letting the terminal scale | **0.10 ms / 160 KB**, and it does not move when the window grows |

*Symptom* — the game runs slowly and the picture stutters, progressively worse the larger the
window, while the emulator core is sixty times faster than it needs to be.

*Caught by* — no test ROM. A framebuffer hash is identical either way. The invariant that does
catch it is `tx_bytes` independent of `display_px_*`, asserted by
`transmitted_bytes_do_not_grow_with_the_window`.

Two further defects in the same path, both silent, both worth stating as general rules:

- **An unchanged frame must produce zero output.** Putting a large escape sequence in a
  `ratatui` cell symbol makes its reported display *width* ~92,000, and the buffer diff re-emits
  every following cell while the invalidation counter is non-zero — so a **still** game screen
  cost 84 KB a frame.
- **A long run must not grow terminal-side resource use without bound.** Kitty images persist
  until deleted and the terminal enforces a storage quota; picking a fresh random image id per
  frame uploaded sixty undeleted images a second, 8.8 MB each at fullscreen, and the picture
  froze after about a second while sound and input kept working.

And the one that is a *reporting* defect rather than a cost one: **`fps` counted draws we
issued, and nothing we do can make that number fall.** With an 87-byte shared-memory transport,
everything a user experiences as a displayed frame happens after our timers stop. A counter that
cannot go down is not a counter. There is **no present count** in the Kitty graphics protocol —
its response acknowledges the *command*, and the spec defines no display acknowledgement — so
the honest number is a **ceiling** (how long the terminal takes to accept one frame), measured
by a cursor-position round trip and labelled as a ceiling everywhere it is shown.

> **The measurement trap that produced a confident wrong answer here:** read the terminal's size
> *after* it has answered, never before. A window asked for more cells than the screen holds is
> clamped, and a terminal launched for this process reports the size it was asked for until it
> has drawn. Sizing from that early value asks the terminal to composite an image mostly
> off-screen, and the cost then rises with a rectangle nobody can see — reported as
> "30 ms/frame, ceiling 29 fps" when a "640×176" window on this display is really 268×60.

---

## An absent optional capability must cost only that capability

The frontend once refused to start — one line of output, exit 1, nothing drawn — in any terminal
that did not advertise the kitty keyboard protocol. That is GNOME Terminal / Ptyxis, a very
common default. Reproduced: the pre-fix binary exits **1**; the post-fix one runs at **99.98 %
speed, 55.7 display fps**, on half-blocks.

*Contract violated* — the kitty keyboard protocol is documented as **progressive enhancement**:
a terminal that does not understand `CSI > 1 u` ignores it and keeps sending legacy keys
([kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)). `Ok(false)`
means "you get legacy keys", not "input is impossible". **The information was worth having; the
response was wrong. Detect, adapt, inform.**

The same principle, violated the same week in the *graphics* path: `CSI 16 t` is an **optional**
XTWINOPS report ([XTerm Control Sequences](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)),
and nothing licenses treating its absence as "this terminal has no graphics protocol". A
terminal that had already answered the capability questions — primary DA, the kitty `a=q`
handshake — silently lost its protocol for the whole session because one unrelated query went
unanswered, and reported `halfblocks` perfectly honestly, having genuinely chosen it.

*Caught by* — nothing, before this: every ROM-driven check constructs a machine directly and
**never enters the frontend at all**, so the whole entry path — capability handshakes, refusals,
input mapping — is invisible to it. The answer is a **pty harness**
([`testharness/basicterm.rs`](../../testharness/basicterm.rs)) that *is* the terminal on the far
end and drives the real entry point. Two scenarios differing in **one reply**: answer `CSI ? u`
and the frontend must report `kitty`; ignore it and it must report `basic`. Teeth proven by
restoring the old refusal — `basic` goes red, `kitty` stays green, which is the bug's exact
signature.

> **Three things that make a fake terminal faithful, all learned the hard way.**
> **(1) Answer everything a real terminal answers** — silence is not neutral. Each unanswered
> query costs the frontend a multi-second timeout in a blocking path; ignoring `CSI 5 n`,
> `CSI 14 t` and `CSI 16 t` drove the emulator at **1 % of real speed** and made input look lost
> when it was merely queued.
> **(2) Never assert on a word in the raw byte stream** — `ratatui` diffs per *cell*, so
> `grayscale` → `green` puts `een` on the wire. Force a full repaint, then read.
> **(3) Mind the modulus when counting suppressed events** — the palette cycle is three long, so
> ten suppressed repeats land on the same colour whether the suppression works or not. Send
> twelve.

---

## What a wrong emulator looks like

| Symptom | Cause |
|---|---|
| A game runs at double speed on a 120 Hz monitor and 0.46 % fast on a 60 Hz one | emulation paced by `requestAnimationFrame` rather than by a clock |
| A game hangs on its title screen in one build and is fine in another | a peripheral compiled out by a `cfg` — the cartridge polls `NR52` and never sees it power on |
| A cartridge that works on the desktop produces garbage in the browser | mapper selection tied to whether a save path was supplied |
| Audio clicks about twice a second, indefinitely, once anything has stalled once | a scheduler that re-primes after an underrun instead of rebuilding from the backlog |
| The emulator runs at ~5 fps with sound and ~60 without | an audio hand-off that **blocks** inside the emulated frame |
| The picture stutters, and worse the larger the window, while the core is 60× faster than needed | presentation cost scaled with output size instead of with the 160×144 source |
| A still screen still costs tens of kilobytes a frame | an unchanged frame producing output |
| The picture freezes after about a second while sound and input keep working | terminal-side resources allocated per frame and never released |
| `fps` reads a flat 59.7 while the display is visibly slower | counting frames *issued* rather than frames the display accepted — and there is no present count to ask for |
| The emulator refuses to start in an ordinary terminal | an optional capability treated as a requirement |
| A terminal that can draw real pixels is given two-pixels-per-cell blocks, silently, all session | an optional *report* going unanswered, taken as an absent *capability* |
