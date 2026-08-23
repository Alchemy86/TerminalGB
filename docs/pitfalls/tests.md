# Testing and measurement pitfalls

← [Pitfalls index](README.md)

Every entry here produced a *number somebody believed*. That is what makes this class
expensive: a wrong test does not announce itself, it quietly supplies confidence.

---

## An uninitialised submodule is not a test failure, it is no test at all

**Symptom.**

```
thread 'main' panicked at testharness/testroms.rs:
cannot read ROM tests/cpu_instrs/individual/01-special.gb: No such file or directory (os error 2)
```

Or — worse, and this is the version that costs you a day — **no message at all**, because the
harness that would have read those ROMs is not part of the target you ran, and the suite is
green.

**What it looks like.** The panic reads as a broken harness or a bad path. The silent version
reads as a passing accuracy suite. In this repository the in-repo accuracy harness
(`cargo test --release --test testroms`, <!--score:harness-->46 / 47<!--/score--> rows) had
been skipped in *every* `cargo test` run in a worktree, and nothing said so.

**Root cause.** The Blargg ROMs are a git submodule (`tests/`, `retrio/gb-test-roms`), and a
submodule is **not** populated by `git clone` or by `git worktree add`. Notes saying it "is
wired into CI" are about CI's checkout, not about yours.

**Resolution.**

```sh
git submodule update --init --depth 1 tests
```

Same class, different fixture: `testharness/audiocapture.rs` wants Blargg's `dmg_sound` ROMs,
and `make fetch-test-suites` fetches Mooneye / SameSuite / Mealybug at pinned versions into
`target/test-suites/`. None of them ships in the repository, and for a licensing reason rather
than an oversight — see [`docs/conformance-action.md`](../conformance-action.md).

**Side effects and blast radius.** **Every before/after comparison made without the fixtures
is worthless**, in both directions: it cannot show a regression and it cannot show a gain. If
you have already quoted a number from such a run, re-measure it rather than adjusting it.

There is a second decision hidden here. `audiocapture.rs` now **skips** a fixture whose ROM
was never fetched, rather than panicking — but only on `ErrorKind::NotFound`. A ROM that is
present and unreadable still fails, and a skipped fixture *keeps its baseline line* so a
re-bless cannot silently delete rows. Both halves matter: a skip that can delete expectations
is worse than a panic.

**How we found it.** `54a0729` (the audiocapture half — `cargo test` was red for a purely
environmental reason and read as an APU regression) and `2e0da0e` (the testroms half, found
while validating an unrelated commit and realising the comparison had proved nothing).

---

## A SKIP is not a PASS

**Symptom.**

```
== aarch64-unknown-linux-musl
   SKIP: qemu-aarch64-static not found (install qemu-user-static, or set $GB_QEMU_DIR)
```

…and the run exits **0**.

**What it looks like.** A green cross-architecture gate. `make test-cross` builds the
embedding core for aarch64, armv7 and i686, *runs* a cartridge on each, and requires a
byte-identical frame hash against the host. When the emulator for a target is missing, that
target is skipped — and if all of them are, the script has verified nothing while returning
success.

**Root cause.** Skipping is the right behaviour (a missing developer tool must not fail a
build). Exiting 0 afterwards is what turns it into a lie of omission.

**Resolution.** `tools/run-cross.sh` prints the skip loudly, and if *nothing* ran it says so
explicitly:

```
run-cross: nothing was RUN (no qemu, no targets) — this proved nothing
```

That message is the honest minimum. If you need the gate to have teeth on your box, get the
emulator — and note that on a machine with no root you still can:

```sh
dnf download qemu-user-static-aarch64 qemu-user-static-arm
rpm2cpio *.rpm | cpio -idmu
GB_QEMU_DIR=<dir>/usr/bin make test-cross
```

With that, all three foreign architectures really do run and really do produce the identical
frame hash.

**Side effects and blast radius.** None from the fix. But be precise about the *claim*: "the
cross gate is green" and "three architectures ran a cartridge and agreed with the host, pixel
for pixel" are different statements, and only the run-count distinguishes them. Quote the
count.

**How we found it.** `2e0da0e`. See [`docs/portability.md`](../portability.md) for what the
gate proves when it does run — and [builds.md](builds.md#a-type-check-cannot-see-a-missing-64-bit-atomic)
for the defect class that only running finds.

> **The general rule.** Every conditional skip in a test suite needs (a) a loud message, (b) a
> distinguishable exit state or a summary count, and (c) somebody who reads it. A suite that
> can report "0 of 0 passed" as success is a suite that will.

---

## A reliability percentage that is really a load measurement

**Symptom.** A test passes 94 % of the time. Or 95 %. Or 30 %. The number will not sit still
and nobody can say why.

**What it looks like.** Flakiness — an intermittent race to be hunted or, more often, retried
around.

**Root cause.** Pass rate was a monotonic function of how busy the machine was. Same binary,
same cartridge, same save, same harness:

| 1-minute load average (20 cores) | rounds passed |
|---:|---:|
| idle | **122 / 130** |
| ~5 | 19 / 20 |
| ~10 | 7 / 10 |
| ~31 | 3 / 10 |
| ~49 | 1 / 10 |

Both processes in that harness run **unpaced**, as fast as the scheduler allows, so a
descheduled process is a peer that has stopped answering for a very long time in its own
clock's terms.

**Resolution.** Two things, and the first is not optional:

1. **Never quote a reliability percentage without saying what else was running.** "The
   two-process link is 94 % reliable" is false. "It is reliable on an idle box and unreliable
   on a loaded one, and when it fails it can write a corrupt save" is true, less comfortable,
   and the one to say.
2. **A/B interleaved, never sequentially.** Two binaries measured one after the other on a
   shared machine are measuring the machine. Alternate them under the same load.

**Side effects and blast radius.** This decides where a test belongs. A CI runner *is* a
loaded machine — the worst end of that table — so a load-correlated harness in CI is a test
people learn to ignore. Ours is deliberately out of CI and behind an explicit opt-in, and the
open bug it exercises is documented rather than hidden. See
[`docs/link-cable.md`](../link-cable.md) §8.

**A compelling wrong answer, tested and rejected — record these too.** The obvious explanation
was `OFFER_TICKS`, a window measured in *emulated* time (two frames, ~0.7 ms of wall clock on a
console running at ~50×), which a descheduled process exceeds trivially. The sister bug on the
same page is exactly that mistake
([environment.md](environment.md#a-deadline-for-another-process-belongs-to-the-wall-clock)).
It is a good story. **Multiplying `OFFER_TICKS` by 100 and A/B-ing the two binaries under the
same load, interleaved: baseline 17/20, patched 15/20. No effect.** The window is not what
expires. The cause is still open, and the next step is instrumentation rather than another
guess.

**How we found it.** `GB_TWOPROCESS_REPEAT` sweeps at four load levels; the corruption mode
was found by the harness's third verdict, which reads both battery files back off disk.

---

## A test that runs only because another test left a file behind

**Symptom.** A harness that skips cleanly when you run it, and *actually executes* when
somebody else runs the full suite — or the reverse. Test order changes the result.

**What it looks like.** An unrelated regression: the suite goes red on a machine where it was
green, days after the last change to anything it touches.

**Root cause.** `testharness/twoprocess.rs` searched for its Cable Club fixture and fell back
to `$TMPDIR/gameboy-gen1-trade/cableclub.sav`. That is the *same* path
`testharness/colosseum.rs` looks for, and it is produced by an example (`gen1_trade
--make-save`) that a developer might have run once, months ago. Its `required-features` were
already satisfied by the default feature set. So: once that file existed on a box, any later
full `cargo test` promoted a clean skip into a real run — launching two OS processes into the
load-correlated flake above, for a bug nobody had asked to reproduce.

**Resolution.** Make the trigger something a person typed on purpose. `GB_TWOPROCESS_RUN=1` is
required, and it is checked **before** either fixture search runs, so an incidental file can no
longer promote a skip into a run. Without it the harness prints why it is opting out and how
to run it deliberately, then exits 0 loudly.

**Side effects and blast radius.** Deliberately none on what the harness *checks*. Gating
whether a test runs must never become an excuse to loosen what it demands: the field-by-field
arrival check, zero torn transfers, zero faults, and both battery files read back off disk and
checksum-verified are all unchanged.

**How we found it.** `24abd0e`. The full reasoning is in the module header of
`testharness/twoprocess.rs` and in [`docs/link-cable.md`](../link-cable.md) §8.

> **The general rule.** A fixture search that reaches into a shared temp directory couples
> your test to every other test that has ever run on that machine. Make the *decision to run*
> depend on an explicit signal, and make the fixture search depend on the decision — not the
> other way round.

---

## A harness whose expected output is its own output

**Symptom.** A suite that is green, and would have stayed green straight through the
regression you are looking for.

**What it looks like.** Coverage.

**Root cause.** Four ROMs' expected result was an **FNV hash of our own framebuffer**, and the
documented response to a changed hash was to re-bless it. One of them (`oam_bug`) was recorded
as an opaque number that in fact meant *"3 of 8 sub-tests pass"* — unreadable, so nobody
noticed it was not 8 of 8.

**Resolution.** Every expectation must be somebody else's: the ROM's own serial string, the
text the ROM writes to cartridge RAM, or its author's reference image. Where a ROM prints its
verdict on screen and nowhere else, read the *text* off the frame rather than pinning a
picture — see [techniques.md](techniques.md#read-the-roms-own-verdict-off-the-frame).

`testharness/baseline.txt` now stores PASS/FAIL **and the ROM's own report text**, so a diff
is human-readable and a change in any sub-test, register dump or checksum line is caught.

**Side effects and blast radius.** The re-bless workflow changes meaning. `UPDATE_BASELINE=1`
still exists, but a diff is now something you can read and argue with, which is the point. The
conformance tool built on the same rule enforces it mechanically:
`test_no_check_expects_a_reference_frame_produced_here` refuses any check that pins an image
unless the image ships with its own ROM under a redistributable licence.

**How we found it.** By reading the harness rather than its output.
[`docs/testing.md`](../testing.md) carries the full policy.

---

## A test machine missing a peripheral is not a quiet one

**Symptom.** A ROM "cannot run headless". It draws its title screen and hangs there for ever.

**What it looks like.** A ROM that needs a display, or a timing bug deep in the CPU.

**Root cause.** The harness built a machine with `sound = None`. `Sound::do_cycle` returns
immediately when no `AudioPlayer` is installed, so that machine has **no APU at all** — not a
silent one. `NR52` never reports itself powered on, and `interrupt_time`, which spins on
`NR52`, waits for ever. `dmg_sound/01` reported *Failed* from the same cause. Both were
written off for months as properties of the ROMs.

**Resolution.** Install a `SilentPlayer`. Measured cost of doing so in the ROM harness: +0.02
ms/frame, whole suite ~2.9 s. **Never build a test machine without an APU.**

The same defect shipped in a *product* configuration and is the sharper version of the
lesson: the `wasm32` build had the whole `sound` module compiled out, so a cartridge's sound
driver spun on `NR52` for ever and the game looked **hung**, not silent. A type check cannot
see a missing peripheral.

**Side effects and blast radius.** Adding the APU changes verdicts — that is what it is for —
so expect baselines to move in the same commit, and be able to say which ones and why. Do not
confuse this cost with the `audio` *cargo feature* (a host sound device), which is a different
path entirely.

**How we found it.** Two of our own harnesses disagreeing about the same emulator.
[`docs/measured/host.md`](../measured/host.md) has the build-configuration form.

> **The general rule.** "Absent" and "quiet" look identical from outside and mean opposite
> things. Any component whose disabled state is indistinguishable from its idle state needs an
> explicit four-state verdict, not a boolean.

---

## A stale binary looks exactly like a regression

**Symptom.** A measurement that is worse than it was, on a tree you have just restored.

**What it looks like.** The revert did not take, or the regression is real and elsewhere.

**Root cause.** The suite scouts here are `cargo build` targets. After deliberately breaking
source to prove a test has teeth, the binary on disk is the broken one until you rebuild. One
such run reported `timer 11/13` on a clean tree.

**Resolution.** Rebuild between the two halves of any teeth test, and prefer a command that
cannot skip the build. If a number surprises you, check the binary's mtime before you check
the source.

**Side effects and blast radius.** None. This one is pure discipline.

---

## `sleep` is not a clock on a busy machine

**Symptom.** A test that fails 2 runs in 3 at load average 8–15 and is green on a quiet box.
Ours: `auto-repeat strobed a one-shot control key`.

**What it looks like.** A product defect under load — and in this case there genuinely was
one, which is what made the other half so hard to see.

**Root cause, both halves.**

* **The product.** A 200 ms lockout that distinguishes auto-repeat from a human press was
  comparing *our own handling* of two keys. One loop iteration is an emulated frame plus a
  draw; on a loaded machine it can outlast the lockout, and a held key strobes exactly as if
  there were no lockout at all.
* **The harness.** It paced a burst with twelve writes 30 ms apart. `sleep(30ms)` on a busy
  box is however long the scheduler feels like — so the harness itself delivered presses far
  enough apart to genuinely *be* presses, and the emulator was right to act on them. No
  product fix can help that.

**Resolution.** The product now also asks whether the event was *already queued behind its
predecessor* — a key that was cannot be a fresh human press however long we then took to reach
it. The harness writes all twelve bytes in **one** `write_all`: a burst on the wire is what
auto-repeat *is*, and how fast the harness can pace one was never part of the claim.

**Side effects and blast radius.** The queued-behind rule is a strict addition — the timed rule
still decides every key that arrives on an idle queue — and it is sampled for *every* event
kind, because a stale flag left by a key and consumed by a resize would swallow a genuine
press.

**How we found it.** `04f0b72`. Honest caveat recorded there: the original failure did not
reproduce under synthetic CPU load, so the diagnosis rests on the mechanism rather than on a
red-to-green reproduction.

> **The general rule for any pty or subprocess harness: never encode the property under test
> in your own `sleep` spacing.** Put it in one write and assert on the response.

---

## An exact match against a blank screen is not evidence

**Symptom.** A consistency check that scores a perfect 360 of 360 cells — on frame 1, before
the boot logo.

**What it looks like.** A gate that works.

**Root cause.** The check reconstructed a screen buffer from a block map and required an exact
match. Both buffers were untouched RAM holding the same byte, so "an exact reproduction" was
two uniform buffers agreeing about nothing.

**Resolution.** Require the thing you are comparing to be *interesting* before you believe a
comparison against it. That gate now demands a minimum number of distinct tiles. The in-repo
ROM harness makes the same demand of its reference image at start-up: it must be 160×144, it
must have **more than one shade**, it must compare equal to itself, and a single flipped pixel
must be *detected*.

**Side effects and blast radius.** None — it can only reject cases that were never evidence.

**How we found it.** The World Trade Centre work; recorded in
[`docs/worldlobby.md`](../worldlobby.md).

---

## An honest suite has deliberate reds

Full entry: [not-our-bug.md](not-our-bug.md#an-honest-suite-has-deliberate-reds). The short
version, because it belongs to this page too: `blargg/oam_bug@cgb` is pinned **FAIL** in
`testharness/baseline.txt`, with the reason written directly above it, because Game Boy Color
silicon does not have the DMG OAM-corruption defect. Turning that row green would mean
emulating a machine nobody has.

---

## A reference screenshot claimed by the wrong ROM

**Symptom.** One row of a screenshot-judged suite reports a large, stable pixel difference
while every neighbouring row is exact — and the frame you dump for it looks *right*.

**What it looks like.** A single hard emulation bug, in the one feature that row exercises.
It is not: the runner scored the picture against a different program.

**Root cause.** Several suites put a ROM and a variant of it in one directory —
`m3-bg-lcdc.gb` beside `m3-bg-lcdc-nocgb.gb`, `statcount.gb` beside `statcount-auto.gb` — and
name the references after the ROM plus a device tag: `m3-bg-lcdc-dmgC.png`,
`m3-bg-lcdc-nocgb-ncmBCE.png`. A runner that pairs a picture with the *first* ROM stem that
prefixes it hands `m3-bg-lcdc-nocgb-ncmBCE.png` to `m3-bg-lcdc.gb`, because `m3-bg-lcdc` is a
prefix of it too. **A wrong reference is scored as a wrong emulator**, and it also silently
produces two rows claiming the same console, which a naive baseline then collapses into one.

**Resolution.** A picture belongs to the ROM with the **longest** matching stem, and the match
must end on a separator or an exact equality. `examples/csp_suite.rs::references` and its
`owns` helper. The runner also asserts that no two rows share a key, so the collapse can never
happen quietly again.

**Related, and the same class:** a suite's own runner may open only *some* of the pictures it
ships. Gambatte's `testrunner.cpp` opens exactly `<stem>_dmg08_cgb04c.png`, `<stem>_cgb04c.png`
and `<stem>_dmg08.png`; the release also contains bare `<stem>.png` and `<stem>_gba.png` files
it never looks at. Scoring those measures expectations upstream does not hold anyone to.

**Side effects and blast radius.** None to the emulator — this is entirely a runner defect. It
does change baseline *keys*, so fixing it after a baseline exists is a re-bless.

**How we found it.** Wiring the thirteen c-sp suites, 2026-08-15: `m3-bg-lcdc@ncm` reported
"reference palette not understood" because the picture it had been given was drawn by the
other cartridge.

---

## A suite's own documentation disagreeing with the files it ships

**Symptom.** A reference screenshot uses a colour that the suite's own readme says it does
not, so a strict comparison reports every pixel of that shade as wrong.

**What it looks like.** A palette bug in your emulator, or a corrupt download.

**Root cause.** Prose drifts and binaries do not. Every `game-boy-test-roms-howto.md` in the
c-sp release states the Color boot ROM's compatibility background palette as `#000000
#0063C6 #7BFF31 #FFFFFF`. Thirty of the thirty-one references that carry that shade use
`#7BFF31` — and MBC3 Tester's uses **`#7BFF4A`**. Both are self-consistent: `#7BFF31` is
`(X << 3) | (X >> 2)` applied to the 5-bit triple `(15, 31, 6)` and `#7BFF4A` to `(15, 31, 9)`.
One of the two is what the boot ROM really loads and the other is a digit somebody typed.

**Resolution.** Accept both as the same shade index, and say in the code *why* there are two —
refusing the one that disagrees with the prose would cost a real row over a typo. The general
rule: **when a suite's documentation and its artefacts disagree, the artefact is the
expectation**, because the artefact is what every other runner is scored against.

**Side effects and blast radius.** None; it only widens what the runner recognises.

**How we found it.** `mbc3-tester@cgb` reported "reference is neither a DMG nor a
compatibility palette" — a message that exists precisely so an unrecognised colour is reported
as a runner limitation rather than as 23,040 wrong pixels.

---

## A reference emulator's boot ROM is not the console's boot ROM

**Symptom.** You want to know what a real console leaves in `DIV` (or `LY`, or anywhere else)
when it hands the cartridge control, you have an oracle that runs a foreign emulator, and its
answer disagrees with a hardware-verified suite you already pass.

**What it looks like.** A boot-state bug in your emulator, or a bug in the suite.

**Root cause.** SameBoy — the oracle this repository ships in
[`tools/sameboy-oracle/`](../../tools/sameboy-oracle/README.md) — ships **its own
reimplemented boot ROMs**, not Nintendo's. They are functionally faithful and are not
cycle-identical, so anything measured *across* the boot is that emulator's number, not
hardware's.

**Measured, 2026-08-18.** A ten-instruction probe cartridge that stores `DIV`, `LY`, `STAT` and
`LCDC` at `$0100` and then spins:

| | our DMG | SameBoy `dmg` | our CGB, DMG cartridge | our CGB, CGB cartridge | SameBoy `cgbE`, DMG cartridge | SameBoy `cgbE`, CGB cartridge |
|---|---|---|---|---|---|---|
| `DIV` | `$AB` | `$BD` | `$26` | `$1E` | `$C0` | `$AC` |
| `LY` | `$00` | `$90` | `$00` | `$00` | `$93` | `$92` |

**Our two CGB columns were one column when this was first measured**, and the split is the
finding rather than a detail: until 2026-08-23 a CGB-flagged cartridge got the *compatibility*
path's `$2678` too, so it also read `$26`. It boots `$1EA0` now — derived from gambatte's
`div/start_inc_{1,2}_cgb04c`, which have a CGB header — so a genuine Color cartridge reads
`$1E`. SameBoy has always distinguished the two headers, which is why its own columns do.

Our DMG value is the one Mooneye's `acceptance/boot_div-dmgABCmgb` pins — six `DIV` reads at
chosen offsets, verified by Gekkio on real DMG and MGB units — and we pass that ROM. SameBoy is
18 `DIV` ticks away from it. The decisive confirmation: **SameBoy fails BullyGB's
`Invalid initial DIV` check on every one of its nine consoles**, including the ones BullyGB's
author verified pass on his own hardware.

**Resolution.** Use the oracle for what it is good at — a ROM's *own* verdict on a machine you
do not own — and never for the boot hand-off. For that, the authorities are Mooneye's
`boot_div`/`boot_hwio`/`boot_regs` family and a real boot ROM, which this project deliberately
neither ships nor downloads (`src/boot.rs`) — plus gambatte's `div/start_inc_{1,2}_cgb04c` for
the one hand-off Mooneye's family cannot reach, a Color console booting a CGB-flagged
cartridge (see the closing note below).

**Side effects and blast radius.** None to the emulator; the cost is entirely in time. The
`peek` mode added to the oracle while chasing this is worth keeping anyway — it is how you ask
a foreign implementation what it leaves at an address, which is the one question a pass/fail
oracle cannot answer.

**How we found it.** BullyGB draws a black screen here, and its single failing check turns out
to be the initial `DIV`. `CGB_BOOT_SYSTEM_COUNTER`'s doc comment already said what was wrong —
the value came from `misc/boot_div-cgbABCDE`, whose cartridge has a **DMG** header, so it pins
the compatibility path, and the genuine Color path was reusing it as a documented
approximation. The detour was trying to settle the CGB-mode value with the oracle.

**Superseded on the "no ROM pins it" half, 2026-08-23.** That comment also claimed no suite ROM
measured the genuine path. Gambatte's `div/start_inc_{1,2}_cgb04c` do: `$0143 = $C0`, a fixed
13/14-`NOP` distance from hand-off and no `LY` wait, so they pin the counter alone rather than
its phase against the PPU — which is what makes them usable where BullyGB's own check is not.
The constant was split on that evidence (`docs/measured/boot.md`). The *oracle* half of this
entry is untouched: SameBoy is still the wrong authority for a boot hand-off, and it still
fails BullyGB's check on all nine of its consoles.
