# The conformance action — score *any* Game Boy emulator from CI

> Add one job to your emulator's workflow and get a **score**, a **badge**, and —
> the part that makes it worth using — an **explanation of every failure** in
> hardware terms. · [← Top-level README](../README.md) · Related:
> [the standalone-tool goal](conformance.md) · [testing](testing.md) ·
> [telemetry](telemetry.md)

This is the first shippable form of the goal recorded in
[`conformance.md`](conformance.md): our accuracy harness, turned into something
other projects can point at their own emulator.

```yaml
jobs:
  conformance:
    uses: Alchemy86/TerminalGB/.github/workflows/conformance.yml@main
    with:
      build: cargo build --release
      adapter: ./ci/gbconform-adapter.sh
```

Anyone can run a test ROM. What this adds is the *why*: a failing check comes
back as *"your `POP AF` is not masking the low nibble of F — bits 0-3 of the
flag register do not exist on the chip"*, with the hardware citation attached.

---

## 1. The adapter contract (`gbconform/1`)

Your emulator and this runner share no API, so they meet at the smallest
interface we could find that still works: **a command that runs one ROM and
writes at most two files.** Print it any time with

```console
$ python3 tools/conformance/gbconform.py --contract
```

The runner executes:

```
<your-adapter-command> <rom-path>
```

with these environment variables set:

| Variable | Meaning |
|---|---|
| `GBCONFORM_CONTRACT` | always `gbconform/1` |
| `GBCONFORM_ROM` | the ROM path (also `argv[1]` — use whichever suits) |
| `GBCONFORM_MODEL` | `dmg` or `cgb`: which model to emulate. **Not advisory** — `oam_bug` is a pass on a DMG and a fail on a Color console, and `interrupt_time`'s cartridge is CGB-only |
| `GBCONFORM_FRAMES` | run at least this many frames, then stop |
| `GBCONFORM_SERIAL_OUT` | **set only when serial output is wanted**: write the raw bytes the ROM sends to the link port (`SB`) here |
| `GBCONFORM_FRAME_OUT` | **set only when a frame is wanted**: write the final 160×144 frame here |
| `GBCONFORM_TIMEOUT_S` | wall-clock budget; the runner kills you after it |

Exit `0` when the run completed. A non-zero exit, a timeout, or an unreadable
output file is reported as a failure with your stderr attached.

**That is the whole contract.** No protocol, no JSON, nothing parsed from
stdout. There is deliberately no way for an adapter to *report a verdict* —
verdicts come from the ROM, not from the emulator under test.

### The two capabilities

`GBCONFORM_SERIAL_OUT` — the raw byte stream, no framing and no
interpretation. Blargg's ROMs print human-readable text there; the runner
decides what it means.

`GBCONFORM_FRAME_OUT` — one frame, in **whatever you already have**:

* PNG (any of colour types 0/2/3/4/6, bit depths 1–16, non-interlaced),
* Netpbm `P6`/`P5` **or the ASCII `P3`/`P2`** — binjgb writes `P3`, and
  rejecting it would have been a pointless demand,
* or a raw dump: 160×144 RGBA, RGB, or 8-bit greyscale/index. An emulator with
  no image encoder answers with a single `write()` of its framebuffer.

**You do not have to support both.** The runner probes each capability once;
checks needing one you do not have are reported **skipped**, not failed, and
are excluded from the score. Declining is explicit: exit `0` and write nothing.

> **Why "exit 0 and write nothing" and not just "write nothing":** an emulator
> that crashes on every ROM also writes nothing. If that read as "has no
> capabilities", it would score 0 out of 0 — which a badge renders as a clean
> slate rather than the total failure it is. A crash or timeout during the
> probe therefore means *assume the capability exists*, so every check runs and
> records a real error. There is a test for exactly this
> (`test_an_adapter_that_crashes_is_an_error_everywhere_not_a_skip`).

### Four things worth copying into your adapter

* **Stop early on a serial verdict.** Blargg's ROMs print `Passed`/`Failed` and
  then loop forever. Running the full frame budget every time turns a
  two-second suite into a two-minute one.
* **Give the cartridge a private copy of the ROM** if your MBC derives a save
  path from it. The runner stages ROMs into a temporary directory, but an
  emulator writing a `.sav` beside the ROM will dirty any local mirror.
* **Honour `GBCONFORM_MODEL`.** Both adapters here got this wrong first time —
  ours ignored it and auto-detected from the cartridge header, binjgb's
  hard-coded `--force-dmg` — and in both cases it cost a check that would
  otherwise have passed.
* **Install your APU even with nothing to play it through.** A machine with no
  sound chip is not a quiet Game Boy: `interrupt_time` synchronises through the
  APU and never leaves its title screen without one. This project made exactly
  that mistake and wrote the ROM off as unable to run headless.

### Example adapters

Ours is a Rust example — read it as documentation of the whole ask:
[`examples/conformance_adapter.rs`](../examples/conformance_adapter.rs) (about
sixty lines of substance).

A ready-to-copy starting point:
[`tools/conformance/adapters/template.sh`](../tools/conformance/adapters/template.sh) — change
two `exec` lines and it is done.

A shell adapter over an existing headless runner is usually this short —
[`tools/conformance/adapters/binjgb.sh`](../tools/conformance/adapters/binjgb.sh):

```sh
#!/bin/sh
set -eu
[ -n "${GBCONFORM_FRAME_OUT:-}" ] || exit 0     # no serial support: decline
exec binjgb-tester -f "$GBCONFORM_FRAMES" --force-dmg \
     -o "$GBCONFORM_FRAME_OUT" "$GBCONFORM_ROM"
```

---

## 2. What runs today

| Check | Method | Verdict comes from |
|---|---|---|
| Blargg `cpu_instrs` 01–11 | serial | the ROM's own printed `Passed`/`Failed #N` |
| Blargg `instr_timing` | serial | as above |
| Blargg `mem_timing` | serial | as above |
| `dmg-acid2` | screen | its author's reference image |
| Blargg `mem_timing-2` | screen-text | the ROM's own printed report, **read off the frame** |
| Blargg `halt_bug` | screen-text | as above |
| Blargg `oam_bug` (DMG) | screen-text | as above |
| Blargg `interrupt_time` (CGB) | screen-text | as above |

Eighteen checks. Narrow on purpose: a working action that runs Blargg via
serial and produces a badge is genuinely useful, and trying to cover every
suite first produces nothing.

**A principle that keeps this honest: no check's expected output is defined by
our emulator.** A serial verdict is the test ROM's own judgement, and the
dmg-acid2 verdict is Matt Currie's own reference image. We are a participant in
this suite, not the authority for it.

### `screen-text` — the ROM's own verdict, read off the picture

Four of Blargg's ROMs never touch the link port; they print their result on
screen and nowhere else. For a long time they were left out, because the only
obvious way to score them was to compare pixels against a reference frame —
and the only reference frame available is one *we* rendered, which would have
made this project the authority for its own conformance suite. That exclusion
is now gone, and the principle above is intact: these checks read the **text**
and let the ROM's own `Passed` / `Failed #N` be the verdict, exactly as the
serial checks do.

The trick is that Blargg's print routine loads a font whose 8x8 glyphs live
**in the ROM under test**, one per character in ASCII order. So every glyph on
the screen is a byte-for-byte copy of eight bytes already in the cartridge we
were handed:

1. cut the frame into 8x8 cells and reduce each to a one-bit ink mask;
2. find each cell's eight bytes in the ROM — every non-blank glyph is there,
   at one place: the font;
3. the font is contiguous and in ASCII order, so `offset // 8` is the
   character code up to one unknown constant, and the constant is recovered by
   keeping the alignment whose decoded words appear **verbatim in the ROM**
   (each of these ROMs carries its own title and the strings `Passed` and
   `Failed` as plain ASCII).

Step 3 is what makes it self-validating: a wrong alignment decodes to noise,
and noise is not in the ROM. When nothing scores, the check reports **error —
"could not read the ROM's report off the frame"**, never a guessed verdict.
Inverse video (`oam_bug` prints a failing sub-test's code white-on-black) and a
console scrolled by a non-multiple of eight are both handled. The whole thing
is `tools/conformance/screentext.py`, and its tests build a synthetic cartridge
rather than needing a real one.

**It costs an adapter nothing.** `screen-text` is a new *method*, not a new
capability: it consumes the same `GBCONFORM_FRAME_OUT` frame the dmg-acid2
check already asks for. Every adapter with a screenshot flag gained four checks
with no change at all — measured on binjgb, which went from one scoreable check
to five.

Two of the four **pin their console** and ignore `--model`, because running
them on the other machine measures nothing: `oam_bug` is a DMG-silicon defect a
real Color console does not have, and `interrupt_time`'s cartridge header says
CGB-only. That makes `GBCONFORM_MODEL` load-bearing for the first time — see
§7, where honouring it turned a binjgb failure into a pass.

### Screen checks compare shade rank, not colour

Every emulator paints the four DMG shades differently, so a literal RGB
comparison would fail everyone for a reason that has nothing to do with
accuracy. Frames are reduced to **shade rank by luminance** before comparing:
only *which of the four shades* each pixel received is judged.

This is not hypothetical — our own frames use `000000/606060/c0c0c0/ffffff`
while the upstream reference image uses `000000/555555/aaaaaa/ffffff`. A
literal comparison would fail on every run.

A failing screen check reports the differing pixel count, the bounding box of
the difference, and writes both the actual frame and a diff image (differing
pixels in red) into the artifact.

---

## 3. ROM licensing — what we may fetch, and what we may not

This shapes the whole design.

| Suite | Licence | How the action gets it |
|---|---|---|
| **dmg-acid2** | MIT (Matt Currie) | **Vendored** in this repository with its reference image, so the action already has it. |
| **Blargg's test ROMs** | Freely distributed by Shay Green, but **no explicit licence file** | **Fetched at run time** from the community mirror `retrio/gb-test-roms`, pinned to commit `c240dd7d…` — the same commit this project's baselines were produced against. Not vendored, not redistributed by us. |
| **Commercial ROMs** | Copyrighted | **Never.** Nothing in this action reads, fetches or accepts a commercial game. |

If your runner has no network access, or you keep a local mirror, pass
`roms-dir:` and nothing is downloaded.

Pinning matters for a reason beyond reproducibility: an unpinned suite would
let a score drift for reasons that have nothing to do with the emulator being
scored.

Fetched ROMs are unpacked into the system temp directory, **never into the
output directory** — the output directory is uploaded as a CI artifact, and
putting the Blargg corpus in it would effectively republish those ROMs from
*your* repository. We fetch them; nobody redistributes them. Override with
`--cache-dir` if you want them somewhere you can cache between runs.

---

## 4. The badge

The action writes **both** forms, and the choice is yours:

**`badge.json` — a [shields.io endpoint badge](https://shields.io/badges/endpoint-badge).**
About 150 bytes. Publish it at any public URL and reference it:

```markdown
![conformance](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/OWNER/REPO/badges/badge.json)
```

**`badge.svg` — self-contained.** No third-party request, works in a private
repository, works offline, renders identically forever.

**What we chose and why.** The endpoint JSON is recommended as the default: it
is the smallest thing to publish, it renders in the same style as the rest of a
README's badges, and it needs no infrastructure of ours — there is no service
to run, which was the deciding factor. The SVG exists because the endpoint
route has two real costs the JSON cannot avoid: it requires the file to be
*publicly* reachable (so private repositories cannot use it), and it puts a
third-party request in your README.

**What we rejected.** A hosted service that stores results and serves badges
would be the smoothest experience and is the one thing here we would have to
operate, monitor and keep online — for a personal project that is a liability,
not a feature. A **check run** posting the score into the PR is genuinely
useful but is not a README badge, so it solves a different problem; the job
summary already covers most of that value (see below) at zero cost.

Both routes still need the file published somewhere. The usual recipe is an
orphan `badges` branch:

```yaml
      - uses: Alchemy86/TerminalGB/.github/actions/conformance@main
        id: conformance
        with: { adapter: ./ci/gbconform-adapter.sh }
      - if: github.ref == 'refs/heads/main'
        run: |
          git checkout --orphan badges || git checkout badges
          cp conformance-out/badge.json badge.json
          git add badge.json
          git -c user.name=ci -c user.email=ci@local commit -m "conformance badge" || exit 0
          git push -f origin badges
```

The badge says something meaningful — `conformance 24/26`, not pass/fail — and
its colour tracks the percentage.

**You do not need a badge at all to get the value.** The action writes the full
report into the **GitHub step summary**, so the score and every failure
explanation are visible in the run page with nothing published anywhere.

---

## 5. Outputs

Everything lands in the output directory (uploaded as an artifact by default):

| File | What it is |
|---|---|
| `results.json` | the full result, schema below |
| `badge.json` | shields.io endpoint badge |
| `badge.svg` | self-contained badge |
| `summary.md` | the human report, also written to the job summary |
| `artifacts/` | captured serial text, actual frames, diff images |

The action also sets step outputs `passed`, `total`, `percent`, `skipped` and
`results-dir`.

### `results.json`

```jsonc
{
  "schema": 1,                    // bumped if a field is renamed or removed
  "contract": "gbconform/1",
  "label": "conformance",
  "model": "dmg",                 // the DEFAULT; a check may pin its own
  "score": { "passed": 18, "total": 18, "percent": 100.0, "skipped": 0 },
  "capabilities": {               // capabilities, not methods: `screen-text`
    "serial": { "available": true,  "reason": "" },   // needs `screen`
    "screen": { "available": false, "reason": "exited cleanly without writing a GBCONFORM_FRAME_OUT file" }
  },
  "checks": [
    {
      "id": "blargg/cpu_instrs/08-misc",
      "suite": "blargg",
      "method": "serial",          // serial | screen | screen-text
      "model": "dmg",              // what this check actually ran on
      "status": "fail",            // pass | fail | skipped | error
      "detail": "the ROM reported Failed #2",
      "behaviour": "The leftovers: PUSH/POP (AF in particular) …",
      "evidence": { "serial": "08-misc instrs\n\nFailed #2\n" },
      "artifacts": ["blargg_cpu_instrs_08-misc.serial.txt"],
      // present on fail/error only — burying it under every pass would defeat it
      "explain": ["`POP AF` must MASK the low nibble of F to zero — …"],
      "hardware": "Pan Docs — CPU Registers (the F register's unused low nibble).",
      "reference": "https://gbdev.io/pandocs/CPU_Registers_and_Flags.html"
    }
  ]
}
```

`status` meanings, and the distinction that matters:

* **`skipped`** — your adapter cannot provide the capability this check needs.
  Not a failure; **excluded from `total`**, so the score stays honest rather
  than being diluted by checks nobody attempted.
* **`error`** — your adapter crashed, timed out, or wrote something unreadable.
  This *is* counted, and it carries the explanation too: an emulator that
  crashes on one ROM is often crashing *because* of the behaviour under test.

---

## 6. Where the explanations come from

Each entry in
[`tools/conformance/checks.json`](../tools/conformance/checks.json) carries the
behaviour under test, the likely causes of a failure in hardware terms, and a
citation. That material is not invented for the tool — it is the standing
convention this project already follows: **every emulation bug we fix is
written up as a conformance check at the moment of the fix** (see
[`AGENTS.md`](../AGENTS.md) and [`conformance-notes.md`](conformance-notes.md)),
with the observable symptom, which ROM catches it, the hardware behaviour
violated, and the test it produced.

`blargg/mem_timing` is the clearest example of what that buys. A bare runner
says "failed". This one says:

> This is a structural test, not a table of numbers. If your emulator executes
> a whole instruction and only then advances the timer/PPU by its cycle count,
> no per-instruction table can pass it… The fix that worked in this project was
> to step the timer one M-cycle **before** each bus access… If `instr_timing`
> passes and this does not, that is the expected signature of exactly this gap.

The registry's completeness is enforced, not hoped for: a check with no
`behaviour`, no `explain` or no `hardware` citation fails the runner's own test
suite.

---

## 7. Proven on

**Ourselves — 18/18.** The [`Conformance (self)`](../.github/workflows/conformance-self.yml)
workflow scores this emulator through the *same composite action* an external
project calls, on every push, and fails below 100%. It also fails if the badge
committed at [`docs/conformance-badge.json`](conformance-badge.json) has drifted
from the real result, so the README badge cannot quietly lie. That is the
credibility requirement from [`conformance.md`](conformance.md) made mechanical:
a suite whose author fails their own checks is worthless.

**A foreign emulator, screen only — [binjgb](https://github.com/binji/binjgb).**
Honest result, because the honest result is the useful one:

* Its shipped headless `binjgb-tester` covers the **screen** capability with a
  short shell adapter ([`adapters/binjgb.sh`](../tools/conformance/adapters/binjgb.sh)),
  and it scores **4/5** through this runner with no changes to binjgb:
  `dmg-acid2` against the upstream reference image, plus `mem_timing-2`,
  `halt_bug` and `interrupt_time` on their own printed reports. Its one failure
  is `oam_bug` — it does not model the DMG OAM-corruption defect, which the
  report says in those words with the ROM's own `01:ok 02:02 …` line attached.
* It has **no serial output**: no flag for it, and `emulator.h` publishes no
  serial hook (the one that exists is inside an `EM_ASM` and reaches only the
  emscripten build). So the thirteen serial checks are reported **skipped**,
  which is exactly what the capability model is for.
* Writing the adapter surfaced one real gap in the runner — binjgb writes
  **ASCII `P3`** Netpbm, which we did not accept at first. That is precisely
  the kind of needless demand that stops projects adopting a contract, and it
  is why the frame formats are now deliberately permissive.
* And a second one, found when `screen-text` arrived: that adapter passed
  `--force-dmg` unconditionally, so the CGB-only `interrupt_time` was run on a
  DMG and reported `Failed`. Honouring `GBCONFORM_MODEL` turned it into a pass.
  **An adapter that hard-codes a console gets a wrong answer, not a missing
  one** — worth checking before believing any single-check failure.

**A foreign emulator, both capabilities — [Peanut-GB](https://github.com/deltabeard/Peanut-GB).**
This is what closes the gap this section used to name. Peanut-GB is a
single-header MIT emulator by Mahyar Koshkouei with no relation to this
project, and `gb_init_serial()` is part of its published API — so it can
exercise the half binjgb cannot. It has no headless runner, no screenshot flag
and no serial dump of its own, so it is also the honest measure of the
worst-case adoption cost below:
[`adapters/peanut-gb.c`](../tools/conformance/adapters/peanut-gb.c), about a
hundred lines against its header, nothing patched or forked.

```console
$ git clone --depth 1 https://github.com/deltabeard/Peanut-GB /tmp/peanut
$ cc -O2 -I/tmp/peanut -o /tmp/peanut-gbconform \
      tools/conformance/adapters/peanut-gb.c
$ python3 tools/conformance/gbconform.py --adapter /tmp/peanut-gbconform \
      --out /tmp/peanut-out --label peanut-gb
peanut-gb: 13/18 (72%), 0 skipped
```

Eleven of the twelve serial checks pass, `dmg-acid2` passes, `mem_timing` fails
over serial, and all four `screen-text` checks report the ROM's own `Failed` —
with the sub-test codes readable in the artifacts (`01:03 02:02 03:ok …` for
`oam_bug`; a `halt_bug` table whose DE column reads `0C04` on every row, which
is the textbook signature of an emulator that gets "HALT does not halt" right
and the PC-rewind half wrong). Peanut-GB is a DMG emulator and does not
implement the Color console, so the CGB-pinned `interrupt_time` measures a DMG
running a CGB cartridge; that is a true statement about the emulator, which is
what the runner is for.

Two things that matter more than the number. First, those four frames were
drawn by an emulator whose shade ramp is **inverted** relative to ours (its
shade 0 is white) — the text decoder never sees a colour, only which cells
carry ink, so it read them straight off. Second, it read them without any
picture of ours being involved anywhere, which is the whole point.

So: **both halves of the contract are now proven against emulators that are not
ours** — screen by binjgb and Peanut-GB, serial by Peanut-GB.

Neither foreign run is in CI, and deliberately: both fetch and build somebody
else's repository, which is a poor thing to put on every push. Re-run them by
hand with the commands above when the runner or an adapter changes. What CI
does enforce is our own 18/18 and the runner's own unit tests, which is the
part that has to be right in *other people's* repositories.

### What adoption actually costs

* **You already have a headless mode with a screenshot flag** — a four-line
  shell script, as above. Minutes, and it now buys five checks rather than one.
* **You have serial output too** — one more line writing the bytes you already
  collect. Minutes.
* **You have neither** — you need a headless entry point that runs N frames, a
  hook on `SB` writes, and a way to get the framebuffer out. Two independent
  data points: about sixty lines in our own Rust core
  ([`examples/conformance_adapter.rs`](../examples/conformance_adapter.rs)) and
  about a hundred in C against Peanut-GB's header
  ([`adapters/peanut-gb.c`](../tools/conformance/adapters/peanut-gb.c)), most
  of both being argument handling and file I/O. In binjgb it would mean
  exposing serial through `emulator.h`, which its public API does not do today
  — a real cost, and worth naming rather than glossing over.

---

## 8. Running it locally

```console
$ cargo build --release --example conformance_adapter
$ python3 tools/conformance/gbconform.py \
      --adapter target/release/examples/conformance_adapter \
      --out target/conformance
$ python3 -m unittest discover -s tools/conformance -p 'test_*.py'
```

Useful flags: `--only blargg/cpu_instrs`, `--roms-dir`, `--no-fetch`,
`--fail-below 100`, `--model cgb`, `--contract`.

The runner is **Python standard library only** — no `pip install`, no
toolchain, nothing to vendor. It hand-rolls its PNG decoder, its PNG writer and
its badge SVG, which is the same dependency-light ethos as this project's own
[test harness](testing.md).

---

## 9. Not covered yet, and honestly so

* ~~**Blargg's screen-only ROMs**~~ — **done**: `mem_timing-2`, `halt_bug`,
  `oam_bug` and `interrupt_time` are scored now, by reading the verdict text off
  the frame rather than comparing it to a picture of ours (§2). The rest of
  Blargg's screen-reporting corpus (`dmg_sound`, `cgb_sound`) is the same
  mechanism away, and is only unwired because its checks want the audio
  capability to be interesting.
* **Sub-test resolution on a `screen-text` failure.** The decoded report is in
  the artifacts and in `evidence.screen`, so `01:ok 02:02 …` is *there* — but
  the `explain` text is still per-ROM, not per-sub-test. Mapping each code to
  the behaviour it measures is the same next step named below for `Failed #N`.
* **Audio.** [`testharness/audiocapture.rs`](../testharness/audiocapture.rs)
  already scores `dmg_sound` 12/12 device-free by reading the ROM's verdict at
  `$A000`; a third capability (`GBCONFORM_AUDIO_OUT`, a WAV or raw samples)
  would extend the contract to it. The analysis exists; only the contract
  extension does not.
* **Mooneye GB.** MIT-licensed, judged by `LD B,B` plus the Fibonacci register
  signature rather than serial — a third *method*, not a third capability.
* **CGB.** `interrupt_time` is the first check to require it (its cartridge is
  CGB-only), so `GBCONFORM_MODEL` is now load-bearing rather than plumbed —
  but `cgb-acid2` and `cgb_sound` are still unwired.
* **Sub-test detail.** A Blargg failure quotes `Failed #N`; mapping every `N`
  to the specific behaviour it measures would be the next big step in
  explanation quality.

Each of these is an extension of the same contract, not a redesign of it —
which was the point of keeping the contract this small.
