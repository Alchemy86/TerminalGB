# Goal: a standalone Game Boy emulator conformance tool

> **Status: the first shippable form is BUILT** — a reusable GitHub Action that scores any
> emulator through a small adapter contract, documented in
> **[`conformance-action.md`](conformance-action.md)**. This page remains the design thinking
> behind the *full* standalone tool; read it for the reasoning, and the action page for what
> exists today. · [← Top-level README](../README.md) · Related: [testing](testing.md) ·
> [audio](audio.md) · [debugging](debugging.md)
>
> **What the action already settles from this page:** the adapter interface (serial capture +
> framebuffer comparison, each an independent capability), the explanation-first reporting, and
> the licensing line — dmg-acid2 vendored under MIT, Blargg's fetched from upstream at a pinned
> commit, commercial ROMs never. Both capabilities are now proven against emulators that are
> **not ours** (binjgb for the screen, Peanut-GB for both). **What it does not:** the audio
> capability, Mooneye, CGB beyond one ROM, and extraction into its own repository.

## The idea

The captain's ask: once our own emulator is correct against the standard test suites, turn the
**test harness itself into a tool other people can point at *their* emulator** — to see what
works, what is missing, and *why* — and eventually extract it into its own crate/repo so it can
live independently of this codebase.

In one line: **our accuracy harness becomes a portable Game Boy emulator conformance checker.**

The value is not "does ROM X pass?" — anyone can run a test ROM. The value is a tool that
**explains a failure**: which behaviour is wrong, what the emulator did versus what the hardware
does, and which documented quirk it maps to. That explanation is the product.

## What it would report

Not a bare pass/fail count. For each check, the useful output is:

- **The behaviour under test**, in hardware terms — e.g. "length-counter extra-clock quirk on a
  no-trigger NRx4 write", "mode-3 length grows with sprites on the line", "the DMG HALT bug
  re-reads the byte after `HALT`".
- **Expected vs actual**, concretely — the reference serial string / reference framebuffer /
  reference audio property, next to what the emulator produced, with the first point of
  divergence highlighted (the frame-hash harness already dumps the differing PNG; the audio
  harness already reports the measured pitch/level/continuity).
- **Why it matters and what is likely missing** — a pointer into the same per-component
  "expected behaviours" spec these docs already carry, so a failing check reads as "you are
  probably not modelling the frame-sequencer off-phase length clock" rather than "test 03
  failed".

This is the genuinely useful part, and it is what would make the tool worth adopting over just
downloading the ROMs.

## How it would attach to a foreign emulator (the hard design question)

Other emulators do not have our `Gameboy::new → frame() → image()` API. So the tool cannot call
them directly; it must observe them through interfaces every emulator already has. The realistic
hooks, each needing only a **thin per-emulator adapter**:

1. **Serial-output capture** — for Blargg-style ROMs that print a result. The adapter feeds the
   ROM and streams the bytes the emulator writes to the serial port (`SB`/`SC`); the tool matches
   the `Passed`/`Failed N` string. *Written as "almost every emulator can expose serial output",
   and that turned out optimistic:* binjgb's public API has no serial hook at all, while
   Peanut-GB's does. Two of the two emulators tried needed the capability model, not one.
2. **Framebuffer / screenshot comparison** — for acid2-style visual tests. The adapter hands the
   tool one rendered frame (a PNG or a raw RGBA buffer). *Written as "the tool compares it to our
   reference image", and the shipped action deliberately does not:* it compares against the ROM
   author's own reference, because an expected picture rendered here would make this project the
   authority for its own suite. Any emulator that can screenshot can participate.
3. **Reading the ROM's report off that same frame** — a method this page originally missed, and
   the one that turned "the screen-only Blargg ROMs cannot be scored independently" from a
   compromise into a solved case. Blargg's font glyphs live in the ROM being tested, so the text
   can be decoded with no font, no reference frame and nothing of ours involved. It is a third
   *method* riding the second *capability*, so it cost adapters nothing. See
   [`conformance-action.md` §2](conformance-action.md).
4. **Recorded-audio capture** — for APU tests. The adapter records the emulator's produced sample
   stream (or a WAV); the tool runs the same objective analysis we already do (sample count vs
   cycles, dominant frequency, level, silence/step continuity) plus the `dmg_sound` `$A000`
   verdict.

Each adapter is small because it only has to *emit* one of those streams — three streams, four
methods, and keeping those two counts apart is what let method 3 be added without asking any
adapter for anything new. The tool owns the ROMs, the reference data, the analysis, and the
reporting.

### Our telemetry stream is the reference adapter

**The single most important early design decision:** the machine-readable telemetry stream the
render crew is building right now (aligned to the **EMIT** half of [debugging.md](debugging.md))
is the natural **reference implementation of that adapter interface**. If we design the
conformance tool's adapter contract and our own telemetry emit-format *to match*, then our
emulator is simply the first, best-behaved adapter — and the contract is proven against real
output from day one instead of invented in the abstract. Designing the two together, now, is
what makes the eventual extraction cheap. Designing them apart is the thing to avoid.

## What we already have that carries over

Most of the hard parts exist and are battle-tested in this repo:

- **Reference images** — the vendored MIT dmg-acid2 ROM + `reference-dmg.png`
  (`third_party/dmg-acid2/`), and the deterministic 4-level pixel-diff that reads them.
- **The known-failures baseline format** — `testharness/baseline.txt` (serial `PASS`/`FAIL`,
  frame `HASH:…`) and the audio baseline (`testharness/audio_baseline.txt`), plus the
  diff-against-baseline model that flags a *change*, not a static red. That format is exactly
  what a foreign emulator's results would be recorded and tracked against.
- **The runners** — the serial-string runner and the FNV-1a frame-hash runner
  (`testharness/testroms.rs`), and the device-free audio capture + analysis
  (`testharness/audiocapture.rs`). All hand-rolled, no heavy dependencies.
- **The spec** — the per-component "expected behaviours phrased as tests" in
  [`docs/components/`](components/README.md), which is where each failure's *why* comes from.

### Growing the corpus: the standing convention

The most valuable material for this tool is produced every time we fix an emulation bug — the
observable symptom, which check catches it (or that none does), the hardware behaviour violated,
and any new test the fix created. That knowledge evaporates if not captured at the moment of the
fix, so **[`AGENTS.md`](../AGENTS.md) carries a standing convention** requiring every
emulation-bug fix to record exactly those four things. Follow it: each fix that does so is one
more ready-made conformance check with its explanation already written. The current **seed set**
is the day's accuracy work — the `mem_timing` per-access fix, the HALT bug, the eight `dmg_sound`
fixes, the HiDPI `cell_scale` fix, and the audio pacing/latency work.

## The extraction path

To become a standalone crate/repo, the following would have to be decoupled from this emulator:

- **Separate the runners and analysis from our core.** Today they call `Gameboy` directly; they
  would move behind the adapter interface (serial stream / framebuffer / audio stream in, verdict
  out) so any emulator can drive them.
- **Package the reference data and baselines** as the tool's own assets, versioned with the tool.
- **Keep the hand-rolled, dependency-light ethos** (own hash, own PNG writer) so the tool stays
  trivial to vendor.
- **Licensing caveat — this is a hard constraint.** We can redistribute the freely-redistributable
  test ROMs (Blargg's are widely mirrored; dmg-acid2 is MIT and already vendored) and our own
  reference images, but **not commercial ROMs** — the tool references and downloads permitted test
  ROMs, it never ships copyrighted games. This repo already honours that line (see the test-ROM
  and upscaler-sample notes in `CLAUDE.md`), and the standalone tool must too.

## The credibility requirement (keep our scores honest)

A conformance tool is only trustworthy if **our own emulator passes what the tool claims to
check.** Our scores are the proof, and they must stay honest and visible:

- **<!--score:harness-->46 / 47<!--/score-->** ROM x console rows in
  `testharness/baseline.txt` — the one red row is `oam_bug` on a Color console, which must
  stay red because Color silicon has no such defect,
- **12/12** Blargg `dmg_sound`,
- **dmg-acid2** pixel-perfect — 0 of 23,040 pixels differ from the reference, **asserted in
  CI** rather than spot-checked by a human.

And the part that keeps the rest honest, on the suites that hurt:
Mooneye `acceptance` <!--score:mooneye_acceptance-->67 / 75<!--/score-->, SameSuite's audio
group <!--score:samesuite_apu-->66 / 69<!--/score-->, and Mealybug Tearoom
<!--score:mealybug-->3 / 79<!--/score--> in the default picture mode
(<!--score:mealybug_accurate-->23 / 79<!--/score--> with the per-dot pipeline). Against the
[GB Emulator Shootout](https://gbdev.io/GBEmulatorShootout/)'s own 264-row manifest we score
**223** — see [`docs/shootout.md`](shootout.md); that figure is measured through their runner
and their screenshot comparison, so it is deliberately *not* derived from our baselines and
must be re-measured rather than edited. A tool that only reports the suites its author passes
is marketing; publish the denominator that hurts.

> Every score above except the Shootout row is rewritten from the checked-in baselines by
> `make status` and verified by `make docs-check`. That is not decoration: the figures this
> paragraph used to carry (32/35, and a Shootout score of 92) were stale by a wide margin,
> which is precisely the failure mode this page is arguing against.

If any of those regressed and we hid it, the tool would be checking behaviour we don't ourselves
meet — which is exactly the failure mode that makes a benchmark worthless. So the same
baseline-diff discipline that keeps this repo honest ([testing.md](testing.md)) is also a
precondition for the tool having any authority.

## The same instinct, elsewhere: transfer and the Bank shutdown

The argument for this tool — *official tooling disappears, well-made open tools fill the gap* —
now has a second instance in this repo, and the two are worth reading together.

Pokémon Bank, the official route for moving a Pokémon from the older games into the modern ones,
is **reported to be closing on 27 February 2027**. Taken at face value, that removes the only
sanctioned forward path for a Pokémon on an old cartridge. The [plugin system](plugins.md) is the
other half of the answer: its first plugin completes a **real Generation 1 link trade** through
the emulated serial port, and it deliberately separates the trade *protocol* from the *source* of
the Pokémon, so a file, a converter, a remote peer and a live partner are interchangeable.

That separation matters here for a reason beyond tidiness. A conformance tool needs a
`SERIAL_LINK` adapter to test the two-console half of the machine at all — see the 2026-08-13
entry in [conformance-notes.md](conformance-notes.md), which records that **no test ROM can
cover a link partner**, because a link partner is by definition a second machine. The virtual
partner built for trading *is* that adapter. One piece of work serves both goals.
