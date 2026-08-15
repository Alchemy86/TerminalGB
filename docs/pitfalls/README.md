# Pitfalls — a symptom index for people hunting a bug

You are probably here because something is wrong and the obvious explanation has already
failed you. Every entry below is a trap this project actually fell into: a symptom, the
**wrong conclusion it invites**, the real mechanism, the fix, and — the part most write-ups
leave out — **what else moves when you change it**.

Nothing here is theory. Each entry names the commit, the test or the measurement that settled
it. Where our own earlier documentation said something different, the entry says so and says
which one is right.

**This is not an emulator-behaviour reference.** Three sibling sections already are, and they
are better at it:

| you want | go to |
|---|---|
| "what does the hardware actually do?" | [the measured Game Boy](../measured/README.md) |
| "my emulator draws / times / sounds wrong" | [symptom → cause index](../measured/symptoms.md) |
| "what *is* an emulator, chip by chip?" | [what an emulator actually is](../emulation/README.md) |
| **"the tooling is lying to me"** | **you are in the right place** |

The dividing line: `docs/measured/` is about the *machine*. This section is about everything
between you and the machine — the harness, the build, the terminal, the clock, the cartridge,
and the suite's own denominator.

---

## Symptom index

Search this page for the error text you have. Strings are quoted verbatim.

### Build and toolchain

| what you see | entry |
|---|---|
| `` the trait bound `X: Y` is not satisfied `` — and rustc's own *help* lists `X` as implementing `Y` | [two builds of one crate](builds.md#two-builds-of-one-crate-and-rustc-blames-your-code) |
| `` `A` and `crate::path::A` have similar names, but are actually distinct types `` | [same](builds.md#two-builds-of-one-crate-and-rustc-blames-your-code) |
| ``perhaps two different versions of crate `X` are being used?`` | [same](builds.md#two-builds-of-one-crate-and-rustc-blames-your-code) |
| ``requires panic strategy `abort` which is incompatible with this crate's strategy of `unwind` `` | [same](builds.md#two-builds-of-one-crate-and-rustc-blames-your-code) |
| `undefined reference to` a symbol that plainly exists, only in `--release` | [same](builds.md#two-builds-of-one-crate-and-rustc-blames-your-code) |
| `warning: output filename collision.` | [same](builds.md#two-builds-of-one-crate-and-rustc-blames-your-code) — the early warning for all of the above |
| ``error[E0432]: unresolved import `std::sync::atomic::AtomicU64` `` | [a type check cannot see a missing atomic](builds.md#a-type-check-cannot-see-a-missing-64-bit-atomic) |
| a feature in `default` breaks the `wasm32` build even though its module is `cfg`-ed out | [Cargo cannot target-gate a bin](builds.md#cargo-cannot-target-gate-a-bin) |

### Tests and harnesses

| what you see | entry |
|---|---|
| `cannot read ROM tests/cpu_instrs/individual/01-special.gb: No such file or directory` | [an uninitialised submodule](tests.md#an-uninitialised-submodule-is-not-a-test-failure-it-is-no-test-at-all) |
| a suite prints `SKIP:` and the run still exits 0 | [a skip is not a pass](tests.md#a-skip-is-not-a-pass) |
| a test that passes alone and fails in the full suite, or the reverse | [tests coupled by a stray temp file](tests.md#a-test-that-runs-only-because-another-test-left-a-file-behind) |
| a test whose pass rate tracks how busy the machine is | [a reliability figure that is really a load figure](tests.md#a-reliability-percentage-that-is-really-a-load-measurement) |
| a green suite that would have stayed green through the regression | [a harness that judged itself](tests.md#a-harness-whose-expected-output-is-its-own-output) |
| a ROM "cannot run headless" and hangs on its title screen | [a test machine missing a peripheral](tests.md#a-test-machine-missing-a-peripheral-is-not-a-quiet-one) |
| a measurement that got worse after you reverted the break | [a stale binary](tests.md#a-stale-binary-looks-exactly-like-a-regression) |
| an intermittent failure only under load, in a test that paces itself with `sleep` | [`sleep` is not a clock](tests.md#sleep-is-not-a-clock-on-a-busy-machine) |
| a comparison that scores 100 % against a buffer nobody has written yet | [an exact match against a blank screen](tests.md#an-exact-match-against-a-blank-screen-is-not-evidence) |

### The host, the terminal and the clock

| what you see | entry |
|---|---|
| a link/socket feature that works at 1× and tears every byte under fast-forward | [emulated time is not wall-clock time](environment.md#a-deadline-for-another-process-belongs-to-the-wall-clock) |
| `Unable to spawn /home/<user>/./target/release/examples/terminal because it does not exist` | [a spawned terminal starts in `$HOME`](environment.md#a-spawned-terminal-starts-in-home-not-in-your-project) |
| a terminal test that hangs for ever and ignores `SIGTERM` | [`timeout` breaks terminal tests](environment.md#timeout-stops-a-terminal-program-instead-of-killing-it) |
| a port whose buttons "do nothing" under an emulator of the target console | [the button map is not the key map](environment.md#the-button-map-of-the-console-is-not-the-key-map-of-its-emulator) |
| a save file the emulator refuses to see | [the extension is replaced, not appended](environment.md#a-save-is-rom-stemsav--the-extension-is-replaced-not-appended) |
| state lost on exit despite a `Drop` impl that writes it | [`Drop` is not a shutdown hook](environment.md#drop-is-not-a-shutdown-hook) |
| `pkill`/`pgrep` killing or reporting the wrong process | [a pattern that matches your own command line](environment.md#pgrep--f-matches-your-own-command-line) |

### Accuracy work

| what you see | entry |
|---|---|
| one constant, two suites, no value satisfies both | [a tunable absorbing an error somewhere else](accuracy.md#one-tunable-two-suites-and-no-value-that-satisfies-both) |
| many image rows failing, all by the same offset | [that is one bug, not many](accuracy.md#rows-that-move-together-are-one-bug) |
| a subsystem blamed by a test that only *synchronises* through it | [the failure names the wrong component](accuracy.md#a-test-that-synchronises-through-one-subsystem-to-measure-another-names-the-wrong-one) |
| a defect that produces no wrong verdict anywhere | [masked by the thing that consumes it](accuracy.md#a-defect-masked-by-the-very-thing-that-consumes-it) |

### It is not your bug

| what you see | entry |
|---|---|
| a game that traps the player with no way out | [the cartridge has no exit](not-our-bug.md#the-room-really-has-no-door-checking-the-games-code-first) |
| a suite score that cannot be improved however correct you get | [the denominator is wrong](not-our-bug.md#most-of-a-suites-remaining-rows-can-be-unwinnable) |
| references that disagree with each other for the same ROM | [more than one reference means more than one machine](not-our-bug.md#more-than-one-reference-image-means-more-than-one-machine) |
| a red row somebody keeps trying to fix | [a row that must stay red](not-our-bug.md#an-honest-suite-has-deliberate-reds) |

---

## The shape of every entry

* **Symptom** — the literal text or observation, quoted so it is searchable.
* **What it looks like** — including the wrong conclusion, because that is the trap.
* **Root cause** — the real mechanism.
* **Resolution** — concretely.
* **Side effects and blast radius** — what else moves. Read this before you change anything.
* **How we found it** — the measurement, not the reasoning.

If an entry has no *Side effects* section it is because the change genuinely has none, and
the entry says so.

---

## The pages

| page | what is in it |
|---|---|
| [accuracy.md](accuracy.md) | traps in emulation-accuracy work: contested constants, correlated failures, tests that blame the wrong component |
| [tests.md](tests.md) | traps in harnesses and measurement: silent skips, self-judging expectations, load-dependent flakes, coupled tests |
| [builds.md](builds.md) | traps in the build: two builds of one crate, target-specific missing APIs, features that reach targets they should not |
| [environment.md](environment.md) | traps outside the program: clocks, spawned terminals, signals, filenames, shutdown |
| [not-our-bug.md](not-our-bug.md) | when the cartridge, the suite or the reference is what is wrong |
| [techniques.md](techniques.md) | the instruments that found the entries above — worth stealing whatever you are building |

## Credit

Several entries were settled by reading another emulator rather than guessing. Behaviour was
learned and reimplemented in this codebase's idiom, never copied; each is cited at its
consumer in the source, per [`CONTRIBUTING.md`](../../CONTRIBUTING.md).

| project | licence (checked, not recalled) | what it settled here |
|---|---|---|
| [docboy](https://github.com/Docheinstein/docboy) | MIT | interrupt latency per source, per halt state and per T-cycle of the M-cycle (`INTERRUPTS_TIMINGS`, `src/docboy/docboy/cpu/cpu.cpp`) |
| [SameBoy](https://github.com/LIJI32/SameBoy) | Expat, outside `iOS/` and `HexFiend/` | the same fact from the other side (`GB_cpu_run`, `Core/sm83_cpu.c`); the strobe's placement (`Core/display.c`) |
| [mGBA](https://github.com/mgba-emu/mgba) | MPL-2.0 — the same licence as this repository | VRAM/OAM access blocking (`GBLoad8`/`GBStore8`, `src/gb/memory.c`); the `LY == LYC` one-T-cycle delay |
| [Pan Docs](https://gbdev.io/pandocs/) | **CC0 1.0 — public domain**, not CC BY-SA | the hardware behaviour behind most of `docs/measured/` |
| [Mealybug Tearoom Tests](https://github.com/mattcurrie/mealybug-tearoom-tests) | MIT | the PPU rows, and the reference-provenance correction in [not-our-bug.md](not-our-bug.md#the-reference-images-are-not-hardware-photographs) |
| [pret/pokered](https://github.com/pret/pokered) | disassembly, cited by symbol | every Generation 1 claim, including [the room with no door](not-our-bug.md#the-room-really-has-no-door-checking-the-games-code-first) |
