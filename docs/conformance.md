# Goal: a standalone Game Boy emulator conformance tool

> **Status: a project goal, not yet built.** This page captures enough design thinking to be
> actionable later — it is deliberately a reasoned goal, not a specification. · [← Top-level
> README](../README.md) · Related: [testing](testing.md) · [audio](audio.md) ·
> [debugging](debugging.md)

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
   the `Passed`/`Failed N` string. Almost every emulator can expose serial output.
2. **Framebuffer / screenshot comparison** — for acid2-style visual tests. The adapter hands the
   tool one rendered frame (a PNG or a raw RGBA buffer); the tool compares it to our reference
   image and reports the differing region. Any emulator that can screenshot can participate.
3. **Recorded-audio capture** — for APU tests. The adapter records the emulator's produced sample
   stream (or a WAV); the tool runs the same objective analysis we already do (sample count vs
   cycles, dominant frequency, level, silence/step continuity) plus the `dmg_sound` `$A000`
   verdict.

Each adapter is small because it only has to *emit* one of those three streams. The tool owns
the ROMs, the reference data, the analysis, and the reporting.

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

- **13/13** Blargg serial suite,
- **9/12** Blargg `dmg_sound` (01–08 + 11; 09/10/12 are the wave-RAM timing tests, planned next),
- **dmg-acid2** pixel-perfect against the reference.

If any of those regressed and we hid it, the tool would be checking behaviour we don't ourselves
meet — which is exactly the failure mode that makes a benchmark worthless. So the same
baseline-diff discipline that keeps this repo honest ([testing.md](testing.md)) is also a
precondition for the tool having any authority.
