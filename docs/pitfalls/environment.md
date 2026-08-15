# Host, clock and environment pitfalls

← [Pitfalls index](README.md)

Everything outside the program: the wall clock, the process group, the terminal, the
filesystem, and the way a program ends. None of these is emulation, and all of them have been
misdiagnosed here as emulation.

---

## A deadline for another process belongs to the wall clock

**Symptom.** A link, socket or IPC feature that works perfectly at 1× and tears **every**
message when either side runs unpaced — a scripted run, a benchmark, fast-forward.

**What it looks like.** A protocol race, or the far end being broken. In our case it looked
like the far end had crashed: the save guard faulted **both** consoles and poisoned both
battery files before the player had finished walking across the room.

**Root cause.** The timeout was counted in the *emulated* machine's own T-cycles.

```rust
const HOLD_TIMEOUT_TICKS: u32 = 1_048_576; // a quarter-second… of emulated time
```

A quarter-second of emulated time is a quarter-second of wall clock only if the console runs
at exactly 1×. On a console running at ~50× it is **~5 ms of wall clock** — and the far end
services its socket once per 16.7 ms frame. So a budget that reads as generous gives the peer
a fifth of the time it needs, and the transfer is declared lost every time.

The same mistake in miniature: `OFFER_TICKS` was 4096 T — **one seventeenth of a frame** — so
a console that armed its port on its very next frame missed every offer.

**Resolution.** What the transfer is waiting for is *another process getting round to
servicing its socket*, and the only clock the two processes share is the wall.

```rust
const HOLD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
```

Two seconds is deliberately generous, for the same reason the BGB transport's `MASTER_TIMEOUT`
is: it must comfortably cover a far end that is merely slow, and only catch one that has
actually stopped. The counterparty that route was verified against paces at 10 ms per byte
from a Python thread with Nagle on; a timeout tight enough to feel "responsive" would break
the one implementation it exists to talk to. `OFFER_TICKS` is now two frames — matching
`SLAVE_ARM_WINDOW` — because *that* one really is a question about the local console's own
emulated behaviour.

> **The rule, stated so it transfers to any emulator or VM.** A deadline for something *inside*
> the emulated machine belongs to the emulated clock. A deadline for something *outside* it
> belongs to the wall clock. Getting this backwards produces a bug whose severity is a
> function of your emulation speed, which is why it survives every test run at 1×.

**Side effects and blast radius.** Moving a deadline from emulated to wall time makes it
*non-deterministic by construction* — which is correct here (the thing it waits for is
non-deterministic) but means it must never be used to decide anything the emulated machine
observes. Ours only decides whether to shift in the idle line, which the game already treats
as a legitimate outcome.

**A compelling wrong answer, tested and rejected.** The obvious next step was to apply the same
reasoning to `OFFER_TICKS` in the two-process reliability flake — the story fits perfectly.
**Multiplying it by 100 and A/B-ing the binaries interleaved under the same load: baseline
17/20, patched 15/20. No effect.** The window is not what expires there. See
[tests.md](tests.md#a-reliability-percentage-that-is-really-a-load-measurement).

**How we found it.** Two independently launched emulators, one driven and one played by hand.
[`docs/link-cable.md`](../link-cable.md) §10; the summary is in
[`docs/measured/serial.md`](../measured/serial.md).

---

## A spawned terminal starts in `$HOME`, not in your project

**Symptom.**

```
Unable to spawn /home/<user>/./target/release/examples/terminal because it does not exist
```

Or, much worse, no error at all: the window opens, the program starts, and reports a missing
ROM or save *inside* a terminal you are trying to screenshot.

**What it looks like.** A broken build, a broken binary, or a broken feature. Nothing ever
started.

**Root cause.** A terminal emulator you spawn does not inherit your shell's working directory.
It starts wherever it likes — usually `$HOME`. Every relative path on the command line then
resolves against the wrong directory.

**Resolution.** Absolute paths for the binary **and every argument**, plus an explicit working
directory:

```sh
REPO=$(git rev-parse --show-toplevel)
wezterm-gui --config initial_cols=150 --config initial_rows=45 \
    start --cwd "$REPO" \
    -- "$REPO/target/release/examples/terminal" "$REPO/target/repro/game.gb"
```

Deriving `$REPO` from `git rev-parse --show-toplevel` rather than writing `./target/…` is the
habit that makes this unrepeatable, and it is why every screenshot recipe in these docs does
it.

**Side effects and blast radius.** None. But note the second half: **stage the ROM into
`target/` first**, because the mapper writes `<rom>.sav` next to whatever ROM it was handed —
pointing a test at a real game directory can destroy a real save.

**How we found it.** Repeatedly, by several people, which is why it is written down. See
[`docs/rendering.md`](../rendering.md) for the full screenshot recipe.

---

## `timeout` stops a terminal program instead of killing it

**Symptom.** A terminal test that hangs for ever, renders nothing, and **ignores `SIGTERM`**,
so even the timeout you wrapped it in cannot clear it.

**What it looks like.** A deadlock in the program under test.

**Root cause.** GNU `timeout` runs its child in **its own process group**. A process that is
not in the terminal's foreground process group raises `SIGTTOU` when it calls `tcsetattr` —
which is exactly what putting a terminal into raw mode does — and the default action for
`SIGTTOU` is to **stop** the process. A stopped process ignores `SIGTERM`.

The mirror-image trap: merely ignoring `SIGTTIN` makes reads fail with `EIO`, so every
capability probe reply looks like a timeout, and you conclude the terminal supports nothing.

**Resolution.** `timeout --foreground`, whose own manual page says it exists for this: *"when
not running timeout directly from a shell prompt, allow COMMAND to read from the TTY and get
TTY signals"*. Or drop the timeout. If you are the one *being* the terminal (a pty harness),
ignore `SIGTTOU` and `tcsetpgrp` to claim it, and set the tty non-blocking so a terminal that
has stopped draining cannot park a blocking write beyond any timeout's reach.

**Side effects and blast radius.** `--foreground` disables timing out the child's own
children. That is usually what you want for a single interactive program and usually not what
you want for a build.

---

## The button map of the console is not the key map of its emulator

**Symptom.** A port to a games console appears to ignore input entirely. Nothing responds; a
menu never advances.

**What it looks like.** A dead input path — an uninitialised controller, a missing poll, a
frontend that never reads the pad.

**Root cause.** You are testing under an emulator of the target console, and **its default
keyboard map does not spell the buttons the way you would guess.** PPSSPP's, from its own
`controls.ini`:

| PSP button | PPSSPP default key |
|---|---|
| **Cross (✕)** | **Z** |
| **Circle (○)** | **X** |
| Square | A |
| Triangle | S |
| Start | Space |
| Select | Enter |
| D-pad | arrow keys |

So a prompt that says "press X" sends a tester's press to **Circle**, and the build looks
broken while being entirely correct.

**Resolution.** Two rules, and they are product rules, not testing ones:

* **Name the button glyph, not the letter** — "press ✕" — and put the host key beside it if
  you know the host.
* **Accept several buttons on a menu.** A menu has no reason to be strict, and "nothing
  happened" is the worst thing a first launch can do.

**Side effects and blast radius.** None; accepting more buttons on a confirmation cannot break
a stricter caller.

**Two neighbours worth knowing while you are here.** Initialise the pad before reading it
(`sceCtrlSetSamplingCycle(0)` + `sceCtrlSetSamplingMode(Digital)`, or
`sceCtrlReadBufferPositive` returns whatever the buffer held — observed as a menu scrolling by
itself with nobody touching it). And synthetic X11 key injection (XTEST) does not reach a
flatpak's SDL window; kernel-level injection does, and it goes to **whatever has focus**,
including your own terminal.

**How we found it.** [`docs/portability.md`](../portability.md) §3.6.

---

## A save is `<rom stem>.sav` — the extension is replaced, not appended

**Symptom.** A game that "does not load its save": the menu offers no CONTINUE, and a
screen-driven script sails past it into a new game and then loops for ever in an opening
cutscene.

**What it looks like.** A corrupt save, or a save-format bug.

**Root cause.** `game.gb` saves to `game.sav`, not `game.gb.sav`. The code is
`rom.with_extension("sav")` in every path that does it. Staging a save under the appended name
silently boots a fresh game, and everything downstream is a consequence of that.

**Resolution.** Use the project's own staging helper rather than hand-rolling the path. If you
must construct it, replace the extension.

**Side effects and blast radius.** None from getting it right. Getting it wrong once destroyed
a real save on this machine, which is why every harness copies the ROM **and** its save into
`target/` before touching either.

**Two adjacent format facts, both of which have cost time:** a `.sav` is **raw cartridge RAM
with no header**, so a file offset *is* an SRAM offset (an emulator that prepends an RTC
anchor makes every foreign save load shifted); and the RTC footer, where one exists, is
appended after the RAM, not prepended. See
[`docs/measured/cartridges.md`](../measured/cartridges.md).

---

## `Drop` is not a shutdown hook

**Symptom.** Save in-game, quit, relaunch → NEW GAME only. The code that writes the save is
right there in a `Drop` impl and is plainly correct.

**What it looks like.** A serialisation bug.

**Root cause.** `Drop` did not run. Four separate reasons, any one of which is enough:

* the desktop event loop **never returns** — winit calls `process::exit(0)` on exit, so the
  object moved into that closure is never dropped;
* `[profile.release] panic = 'abort'` skips destructors;
* a closed window, `SIGTERM` and `kill` all skip them;
* and a crash, obviously.

**Resolution.** Persist on a schedule, not at the end. Mark dirty on write; flush after a
quiet interval or a bounded number of dirty frames; offer an explicit `flush()` for the paths
that *do* have a shutdown signal. Keep `Drop` as a last chance only, and never as the
mechanism.

**Side effects and blast radius.** Writing more often means writing *atomically* matters more:
`<file>.tmp` → `fsync` → `rename`, because a truncated save is worse than no save. And a
session that never wrote cartridge RAM must write **nothing** — unconditionally dumping a
pristine all-zero buffer on exit is how one failed launch permanently erases a real save.

**How we found it.** A real Pokémon save, 32,776 bytes, all zeros. The same rule applies to
anything else that must survive a shutdown; see
[`docs/measured/cartridges.md`](../measured/cartridges.md) and, for the same lesson applied to
shared-memory objects, [`docs/rendering.md`](../rendering.md).

---

## `pgrep -f` matches your own command line

**Symptom.** `pgrep -f <pattern>` reports processes that are already dead; `pkill -f <pattern>`
kills the pipeline you ran it from.

**What it looks like.** A process that will not die, or one that lingers after exit.

**Root cause.** `-f` matches against the full command line — including the command line of the
`pgrep`/`pkill` you just typed, and of the shell that ran it.

**Resolution.** Capture the PID at launch and use it. If you must search, `ps -eo pid,args |
grep` and read the result before acting on it.

**Side effects and blast radius.** None. It is one of those traps that costs an hour exactly
once per person, which is the definition of something that belongs in a document like this.
