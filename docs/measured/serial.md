# The link port — the half no ROM can test

← [The measured Game Boy](README.md)

Every other page here is measured against somebody's test ROM. This one mostly cannot be,
and **that is the finding**: a link partner is by definition a second machine, so the whole
two-console half of the Game Boy is invisible to every ROM-driven suite in existence. Blargg
*uses* the serial port as its results channel and never tests it; Mooneye's one serial ROM
measures the internal shift clock's phase from a cold boot and nothing else.

So the material below is measured a different way — two whole emulated cores driven against
each other, and one real second implementation over a socket — and the checks it produces are
written out as things a conformance tool could ask of any emulator that can run two instances.

Hardware reference, not restated here:
[Serial Data Transfer (Link Cable)](https://gbdev.io/pandocs/Serial_Data_Transfer_(Link_Cable).html).
Component page: [`docs/components/serial.md`](../components/serial.md). The two-console
design: [`docs/link-cable.md`](../link-cable.md); the network transport:
[`docs/bgb-link.md`](../bgb-link.md).

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`6c20451`](https://github.com/Alchemy86/TerminalGB/tree/6c20451306663018288d34253c199fad507ae6f6) · regenerate with `make docs-links`, verify with `make docs-check`.

## The slave half did not exist, and one console could never tell

`Serial::wb` acted on `SC = $81` and **ignored `SC = $80` entirely.**

That is *correct* for one console on its own — nothing is ever going to clock it — and it is
exactly the half a second console needs. Which is why the defect is invisible from inside:

*Symptom* — every Cable Club, every trade, every link battle hangs at "PLEASE WAIT!", while a
single console running the same cartridge is flawless. The game sets `SC = $80`, waits for the
serial interrupt that says the other console has clocked eight bits in, and waits for ever.

*Caught by* — **nothing in any public suite**, per the argument above. The tests that catch it
([`src/link.rs`](../../src/link.rs) `mod tests`) drive two real `Gameboy`s, each running its own
program, joined only by a cable: no mocks, no harness in the middle.

*Hardware* — `SB` (`$FF01`) is an 8-bit **shift register**, not a mailbox. `SC` (`$FF02`) bit 7
is the transfer-start flag and bit 0 selects the shift clock: 1 internal ("master", ~8192 Hz on
a DMG, so 4096 T-cycles a byte), 0 external ("slave"). Both consoles present `SB` to the wire at
once, and after eight edges each holds what the other started with; on the eighth edge *both*
clear `SC` bit 7 and request `IF` bit 3. **A master completes with `$FF` if nothing answers**,
because an unplugged cable's data line idles high; a slave never completes on its own.

### Four checks that need no commercial ROM

Written the way a standalone tool would ask them, of any emulator exposing two instances:

| Check | Pinned here by |
|---|---|
| **A slave with no partner waits for ever** — `SC = $80`, then bit 7 still set and `SB` unchanged after several frames | `a_slave_with_no_cable_waits_for_ever` |
| **Two cores exchange a byte** — each ends holding what the other started with, and **both** raise `IF` bit 3 | `two_cores_exchange_a_byte_through_the_cable`, `completing_a_transfer_interrupts_both_consoles` |
| **A torn byte keeps its data** (below) | `a_slave_that_disarms_mid_transfer_keeps_the_byte_it_had_offered` |
| **Two masters shift in `$FF`; two slaves deadlock** — a cable must not invent a byte for either | `two_masters_shift_in_an_idle_line`, `two_slaves_deadlock_quietly` |

---

## A torn byte keeps its data — the ambiguity that decides whether a link works at all

When the far console arms `SC = $80` and rewrites `SC` before the eighth edge, the exchange is
**torn**, and no prose source says what the master receives. Both readings are defensible from
the documentation. Only one lets two Game Boys connect.

Pokémon Red/Blue's `.establishConnectionLoop` (pokered `engine/link/cable_club_npc.asm`) offers
`SB = $02` behind `SC = $80` and replaces it with `SB = $01` / `SC = $81` about **100 T-cycles
later**, every frame, for as long as it takes to find a partner. Reading `$02` out of that window
is the **only** way two consoles agree on who drives the clock — `USING_EXTERNAL_CLOCK` is `$01`
and `USING_INTERNAL_CLOCK` is `$02`, sharing values with the `ESTABLISH_CONNECTION_WITH_*` bytes
so the reply *is* the role assignment.

**So tearing a byte is how that handshake works, and an emulator that voids a torn byte can
never connect two Game Boys — while its single-console behaviour stays indistinguishable from a
correct one.** Voiding it was implemented first here and faulted both machines 898 frames into a
real Blue-to-Blue trade.

*Measured, not asserted.* With a slave offering `$22` and disarming mid-transfer, the torn
transfer lands **`SB = $22`** — the byte latched when the two were paired. Only the *next*
transfer, started with nobody listening any more, shifts in the idle-high `$FF`. Both halves are
now in the test, which previously asserted only the second and was named after it; the module's
own prose had drifted to match the name.

> **The wider rule this belongs to: a lost byte is not corruption.** The first version faulted
> both consoles the moment a slave disarmed. The games' own protocols are the byte-level desync
> detectors, and Gen 1's is a good one — `Serial_ExchangeBytes` re-sends its first byte until it
> sees the `$FD` preamble come back. What the cable guards instead is the **save file**: if one
> console faults, the other is faulted too, because half a trade leaves the *survivor* holding
> the dangerous save. See [cartridges.md § the save guard](cartridges.md#the-save-guard-a-crashed-game-must-never-reach-the-battery-file).

---

## Two deterministic cores stepped by one loop never drift

A finding about *harness* design rather than about a Game Boy, and it generalises to any
project that links two emulator instances.

Real consoles have independent crystals and real players press A on different frames, so their
link handshakes slide past each other until they interleave. Two identical cores stepped by one
loop stay in phase **for ever**: both drive the clock at the same instant, each finds the other
deaf, and Gen 1's `.establishConnectionLoop` gives up after 90 iterations.

Measured, Blue to Blue, on a 2500-frame budget:

| start skew (frames) | outcome |
|---|---|
| **0** | **never connects** — `hSerialConnectionStatus` stays `$FF` on both |
| 1, 2, 3, 5 | connects; A takes the internal clock (`$02`), B the external (`$01`) |
| 8, 13, 21, 34 | connects; the roles **swap** — B internal, A external |

Any non-zero skew works within ~700 frames, and **which console ends up driving the clock
depends on it**, so nothing downstream may assume a role — read `hSerialConnectionStatus`
(`$FFAA` on Gen 1, `$FFCD` on Gen 2). The terminal frontend applies a three-frame skew at
startup.

**This is the price of determinism, not a cable bug.** A harness that links two instances has
to introduce a skew and should say so, rather than reporting "the games do not connect".

---

## Deadlines for something *another process* must do belong to the wall clock

The same mistake, made twice, in two different components — which is why it is on this page and
not in a commit message.

A cross-process link cannot promise the far console is even executing when our shift register
runs out: both emulators pace to 59.7275 Hz, and each emulates a whole frame in a fraction of a
millisecond and then sleeps. **Two such bursts need not overlap at all**, so a transfer that
completed on schedule would shift in `$FF` every time and no link would ever work. The transfer
is therefore *held* ([`src/mmu/serial.rs:88`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/serial.rs#L88), `link_hold`), which the
game cannot observe — a program waiting on a transfer is already spinning on `SC` bit 7 or on
the serial interrupt, and the timer, PPU and APU keep running underneath it.

The bug was in how long it was held for:

| Constant | Was | Is | Why the old value was wrong |
|---|---|---|---|
| peer master hold | `1_048_576` **emulated T-cycles** | `2 s` **wall clock** | A quarter of a second only if the console runs at 1×. On an unpaced one (a scripted player, fast-forward, a benchmark) it is **~5 ms of wall clock**, while the far end services its socket once per 16.7 ms frame — so every master transfer was declared torn and [the save guard](cartridges.md#the-save-guard-a-crashed-game-must-never-reach-the-battery-file) faulted both machines before the player left the Pokémon Center. |
| peer slave offer | `4096` T-cycles | 2 frames | **One seventeenth of a frame** — a console that armed `SC = $80` on its very next frame missed every offer. |

**Generalise it:** a timeout measured in emulated cycles is a timeout on *this* machine's speed
knob. If what you are waiting for is another process, the deadline is wall time. Two frames is
also what the network transport uses (`SLAVE_ARM_WINDOW`,
[`src/link/transport.rs:162`](https://github.com/Alchemy86/TerminalGB/blob/main/src/link/transport.rs#L162)), and its 2 s master timeout is
deliberately generous for the same reason: the counterparty it was verified against paces at
10 ms per byte from a Python thread with Nagle on.

### An incoming byte is *held*, not refused

Neither end can know which instruction the other is on. A console armed for an external clock
takes whatever arrives; one that is not armed does not, and the master shifts in an idle line.
So an incoming byte is held for up to two frames while the console keeps running, and refused
only when it plainly is not going to arm.

The first interop run lost `$01` and `$02` off the front of a twelve-byte payload for want of
this, because the console was still booting when they arrived.

> **`missed` climbing while `transfers` stays flat is the signature of both ends settling into
> the same role**, and it is invisible without a counter. It is in the frontend's `Net` status
> row for exactly that reason. A transport that sits silently idle is the failure mode; a
> transport that *counts what it could not carry* is the fix.

---

## Over a socket: what a second implementation caught

[`src/bgblink.rs`](../../src/bgblink.rs) speaks
[BGB's link protocol](https://bgb.bircd.org/bgblink.html) over TCP, with no third-party crate.
Three defects it produced are worth carrying, because none of them is an emulation error and all
three are silent.

**Never answer a status packet with a status packet.** The reference Python server does — its own
comment admits the documentation says not to — so an implementation that does the same
ping-pongs for ever. Send one **in response to version** and nowhere else.

**A link that never came up must not be reported as one that broke.** `wantdisconnect`
(packet 109) is the only thing on the wire that distinguishes "the player quit" from "the
emulator died", and the two have opposite consequences for a save file: a bare EOF *after* the
link went live faults the console (and so poisons the battery), anything before it costs only the
cable. Two mechanical traps sit under that:

- **Closing a socket with unread data in its receive queue sends RST, not FIN**, and an RST
  discards the goodbye just written. Write, drain, half-close, then wait for the peer's FIN.
- **Parse the buffer before acting on the hang-up that came with it.** A polite peer writes its
  `wantdisconnect` and closes, so both land in one `read(2)`; recording the EOF first overwrote
  the goodbye and turned every clean shutdown into a broken link.

**A trait method that is only an inherent method is a silent no-op.** `BgbLink::disconnect`
existed but was never added to `impl LinkTransport for BgbLink`; the driver calls it through
`dyn LinkTransport` and got the trait's empty default body. Result: **every clean shutdown was
reported by the peer as a broken link, so the peer stopped writing its save.** The unit test
passed throughout, because it called the inherent method on the concrete type — it goes through
`Box<dyn LinkTransport>` now, and removing the override turns it red.

> Carries straight into the conformance-adapter contract: **a trait with default method bodies
> will accept an implementation that silently does nothing.** Exercise optional methods through
> the abstraction the product actually uses, never through the concrete type a test happens to
> hold. Only a two-process end-to-end run found this one.

### What was measured, and what was not

- **A Gen 1 trade over TCP, two processes, real sockets:** 1,786 transfers, 0 unanswered,
  0 missed, 0 timeouts, in 22 s. Each console ended holding a Pokémon whose original trainer is
  the *other* console's player — a value neither process wrote.
- **A Gen 1 trade over one cable in one process, Blue to Blue:** 1,552 transfers, 0 unanswered,
  0 torn, both `.sav` files passing the game's own `CalcCheckSum`.
- **Against a second implementation:** `PokemonGB_Online_Trades`' own
  `bgb_link_cable_server.py`, driven unchanged as a library with Python as master. Both verdicts
  are independent because the emulated cartridge answers with the **complement** of what arrived
  — a plain echo cannot tell a working link from a peer reading its own transmission back. Teeth
  proven: making the answer `$00` turns it red.
- **Not tested, and worth saying rather than implying otherwise:** BGB itself (a Windows program
  nobody here has run) and a real Game Boy (no adapter on this machine). The hardware path is
  established *by construction* — that tool binds one trading implementation to two different
  byte channels — and that is an inference, not a measurement.

> **An interoperability harness has to look like a cartridge, not like a protocol.** The
> counterparty sends no data at all until it has seen the client say "I am not doing a passive
> transfer" once. Every real cartridge satisfies that without trying, because it boots and draws
> a title screen before it touches the link port; a test ROM that arms on its first instruction
> never does, and the link comes up and then sits there.

---

## The correction: the serial model used to be instantaneous

This page's own earlier claim, corrected rather than quietly dropped.

The 2026-08-13 corpus entry recorded that the whole byte transferred the moment `SC` was
written and bit 7 was never cleared, noted that Gen 1 does not notice (it drives everything from
the serial interrupt rather than polling `SC`), and flagged *"a game that polls bit 7 would hang
for ever"* as a real divergence with no ROM coverage.

**It is timed now**, and it arrived with the [post-boot state](boot.md) work rather than with the
link cable: eight falling edges of system-counter bit 8 (8192 Hz, ~1 ms a byte), aligned to the
counter and not to the `SC` write. `acceptance/serial/boot_sclk_align-dmgABCmgb` measures both
facts from a cold boot and fails an emulator that completes on the write — and the one-M-cycle
subtlety inside it ([boot.md § an unanswered link transfer still completes](boot.md#2-an-unanswered-link-transfer-still-completes))
is visible to that ROM and to nothing else, because every other measurement of the serial clock
is relative to an `SC` write, where a constant phase offset cancels.

---

## What a wrong emulator looks like

| Symptom | Cause |
|---|---|
| Every two-player feature hangs at the game's link menu, while one console alone is flawless | `SC = $80` unimplemented — the slave half of the port |
| A game that polls `SC` bit 7 rather than taking the interrupt hangs for ever | an instantaneous transfer that never clears the start flag |
| Two emulator instances never connect, however long they are left | two deterministic cores started in phase — introduce a skew |
| A link works perfectly against a second copy of the same emulator and against nothing else | a transport answering a master transfer synchronously, inside a window a socket cannot meet |
| A link works at 1× and tears every byte under fast-forward | a deadline for another process measured in emulated cycles |
| The first bytes of every block are lost | an incoming byte refused instead of held while the console arms |
| A player finishes a trade, the partner quits, and the first player's save silently stops being written | a clean disconnect reported as a broken link |
| Two consoles connect, exchange a few bytes and never agree who drives the clock | a torn byte voided instead of kept |
