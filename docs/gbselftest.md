# GBSelfTest — the cartridge that explains itself

[GBSelfTest](https://github.com/Alchemy86/GBSelfTest) is a Game Boy cartridge
that tests the machine it is running on. It boots, runs a suite unattended, and
reports the verdict twice: as text on the screen, and as the same text over the
link port so a host can capture it with nobody watching.

**This page is where its failures point.** Every check carries a stable code,
the code prints with the failure, and the code is a heading below:

```
GB-PPU-08 FAIL objects on a line lengthen mode 3
      got $0000  want $000F
      -> ten objects on a line did not lengthen mode 3. Each one stops the
         background fetcher for at least six cycles while its own row is
         fetched; a renderer that draws the line in one go and charges a fixed
         172 cycles reports no penalty at all
      -> https://github.com/Alchemy86/TerminalGB/blob/main/docs/gbselftest.md#gb-ppu-08
```

That last line is the part nothing else in the ecosystem does. Any suite can
print `FAIL`. What an emulator author needs is a hypothesis about *what
behaviour is missing* and somewhere to read about it — and a stranger running
the cartridge on their own emulator lands here.

## It is somebody else's judgement of us, on purpose

GBSelfTest lives in its **own repository** and is consumed here as a vendored
artefact ([`third_party/gbselftest/`](../third_party/gbselftest/)), never as
source. That is not tidiness. This project's conformance work rests on a single
rule — *no check's expected output may be defined by our emulator*
([`docs/conformance-action.md`](conformance-action.md)) — and a test cartridge
kept inside the emulator it judges would be the easiest possible way to break
it. Keeping it out means other emulator authors can adopt it without taking a
dependency on us, and it means the cartridge is free to disagree with us.

It follows the same rule internally. Every expectation in it is either computed
a second, independent way on the machine — the adder against a counter built
from `INC HL`, the logic operations against De Morgan's law, the rotates
against the fact that eight of them are the identity, `DAA` against decimal
arithmetic done a digit at a time — or is a documented hardware constant. It is
developed against SameBoy and Peanut-GB alongside this emulator, and two of its
checks were wrong and were found that way rather than by reasoning.

## What it scores us

**Our own two rows** are measured with `tools/gbselftest-gate.py` (`make gbselftest`), whose
denominator is every check the ROM prints and whose skips count in it, in both picture modes.
**The four rows below them are not, and `make gbselftest` will not reproduce them**: that script
drives one adapter with `GBCONFORM_MODEL` fixed to `dmg` and defaults to this repository's own
`conformance_adapter`, so it cannot score another emulator per console. Those come from the
GBSelfTest repository's own SameBoy runner and its Peanut-GB adapter.

| machine | result |
|---|---:|
| this emulator, `identical` (per-dot) | **99 / 101**, 1 skipped |
| this emulator, `standard` (whole-scanline) | **90 / 101**, 2 skipped |
| SameBoy, DMG and MGB | 97 / 98, 1 skipped |
| SameBoy, CPU CGB C and CGB E | 93 / 98, 5 skipped |
| SameBoy, AGB | 94 / 98, 4 skipped |
| Peanut-GB | 67 / 98, 2 skipped |

**The two denominators are a cartridge vintage, not two instruments.** Our own rows are
re-measured against the currently pinned 101-check cartridge; the foreign rows below them were
measured before it grew from 98 and have not been re-run, so they are left at the denominator
they were taken at rather than rescaled. Their fail-and-skip counts are what to compare, not
the fractions.

Every row we lose is a gap this project had **already written down**, and that
is the point of running it:

| row | what it is |
|---|---|
| `GB-PPU-07` `GB-PPU-08` `GB-PPU-09` `GB-PPU-18` | the mode-3 fetcher penalties — fine scroll, objects, the window taking over, an object off the left edge. A renderer that draws a whole line at once and charges a fixed 172 cycles cannot show any of them ([`docs/picture-modes.md`](picture-modes.md)) |
| `GB-PPU-10` `GB-PPU-11` `GB-PPU-20` `GB-PPU-21` | mode-based VRAM and OAM blocking — reads on the first pair, writes on the second — deliberately off in the default mode because with a fixed mode 3 it would drop writes a game placed legally in HBlank |
| `GB-PPU-16` | the DMG spurious STAT-write interrupt, which [`docs/components/ppu.md`](components/ppu.md) records as not modelled |

The first eight are the **whole-scanline renderer's documented trade-off** and
the per-dot engine passes them. `GB-PPU-16` is lost by both, and it is the
interesting one: it was a sentence in a component page that **nothing in this
project could measure**. It is a check with a code on it now, and the gate will
say so the day somebody implements it.

A cartridge that separates our two renderers on precisely the rows our own
documentation predicts is a cartridge worth trusting on the rows we did not
predict.

**Nothing here changes emulation.** None of these is a regression; they are
standing trade-offs and one standing gap, restated by an instrument that was
written somewhere else.

## Running it

```sh
tools/fetch-gbselftest.sh --verify      # the vendored ROM is the pinned one
cargo build --release --example conformance_adapter
python3 tools/conformance/gbconform.py \
    --adapter target/release/examples/conformance_adapter --out out
```

It is an ordinary `serial` check in
[`tools/conformance/checks.json`](../tools/conformance/checks.json) and needed
**no change to the adapter contract**: the cartridge ends its report with
`Passed` or `Failed`, which is Blargg's convention, so every runner that
already understands Blargg's ROMs understands this one. The full report is in
the check's `serial` evidence.

A full run is **about eight seconds of emulated time** and the checks are not
what costs it: 77 frames of waiting on the display, and the rest shifting the
report out of the link port at 8192 bits per second. The cartridge prints its
own cost (`cost 77 frames, sent 6678 B`), and a host that only wants the
verdict can stop at `Passed`/`Failed`, which the reference adapter does.

To read the report directly, capture the link port — or run it under an
emulator that is not ours, which is what the cartridge's own
`tools/run-sameboy.sh` does.

## What it does not check

A cartridge cannot see the composited picture: there is no framebuffer to read,
which is why the suites that judge pixels ship photographs and need a host. Use
the [Mealybug Tearoom tests](mealybug.md) for that half, and
[`docs/testing.md`](testing.md) for how this project scores the rest.

What it *can* see is the **rendering fingerprint**: mode 3 lasts exactly as long
as the fetcher takes, so a scene whose cost is documented can be laid out and
the length measured from inside the machine to a resolution of four dots. That
turns out to be the most discriminating part of it.

The cartridge prints its own coverage statement at the end of every run, naming
what it skipped and why. Read that rather than assuming.

---

<!-- checks:begin -->

## CPU: instruction behaviour and flags

### GB-CPU-01

**ADD and ADC, against a counter-built adder**

the sum or its flags disagree with the same addition counted out with INC HL.

### GB-CPU-02

**SUB, SBC and CP**

the difference or its flags disagree with the same subtraction counted out with DEC HL.

### GB-CPU-03

**AND, OR, XOR and CPL by identity**

a law that holds for every byte does not hold here.

### GB-CPU-04

**INC r and DEC r flags**

the half carry or the preserved carry is wrong.

### GB-CPU-05

**rotates and shifts are invertible**

a rotate is losing, duplicating or misplacing a bit.

### GB-CPU-06

**BIT, RES and SET**

a bit operation reached a bit it should not have, or BIT's flags are wrong.

### GB-CPU-07

**POP AF masks the unused flag bits**

F's low four bits are being stored; they do not exist on this CPU.

### GB-CPU-08

**LD HL,SP+e flags come from the low byte**

an eight-bit carry is being taken from a sixteen-bit addition.

### GB-CPU-09

**ADD HL,rr flags**

H must come from bit 11 and Z must be left alone.

### GB-CPU-10

**SCF, CCF and CPL**

a flag instruction is setting where it should complement, or clearing what it should keep.

### GB-CPU-11

**INC rr and DEC rr set no flags**

a sixteen-bit increment is writing to F.

### GB-CPU-12

**DAA against decimal arithmetic**

the decimal adjust disagrees with the same sum done a digit at a time.

### GB-CPU-13

**the one-byte rotates always clear Z**

RLCA and its three relatives never set Z; their prefixed namesakes always do.

### GB-CPU-14

**INC and DEC through HL**

a read-modify-write took its flags from the wrong byte.

### GB-CPU-15

**the pointer forms of LD move HL**

LD [HL+] and its relatives must step the pointer after the access.

## CYC: instruction and memory timing

### GB-CYC-01

**loads and memory accesses**

an instruction took a different number of machine cycles from the published opcode table.

### GB-CYC-02

**arithmetic and sixteen-bit operations**

an instruction took a different number of machine cycles from the published opcode table.

### GB-CYC-03

**PUSH, POP, CALL and RET**

an instruction took a different number of machine cycles from the published opcode table.

### GB-CYC-04

**jumps taken and not taken**

an instruction took a different number of machine cycles from the published opcode table.

### GB-CYC-05

**CB-prefixed operations**

an instruction took a different number of machine cycles from the published opcode table.

### GB-CYC-06

**restarts, returns and the register-addressed load**

an instruction took a different number of machine cycles from the published opcode table.

### GB-CYC-07

**an access lands on its own cycle**

advancing the clocks once per instruction keeps every rate right and still fails this.

### GB-CYC-08

**an interrupt lands on the cycle it was raised**

running a batch of instructions and delivering at the end of it keeps every rate right and still fails this.

## TIM: the divider and the timer

### GB-TIM-01

**the divider and the timer share one counter**

DIV and TIMA are not being clocked from the same chain.

### GB-TIM-02

**writing DIV clears it**

a write to $FF04 must zero the whole sixteen-bit counter, not just the byte you can read.

### GB-TIM-03

**all four TAC rates, measured against DIV**

a TAC rate is not the documented number of cycles per increment.

### GB-TIM-04

**TAC bit 2 stops the timer**

TIMA moved while the timer was switched off.

### GB-TIM-05

**overflow reloads TMA and raises the interrupt**

TIMA wrapping must reload from TMA and set bit 2 of IF.

### GB-TIM-06

**writing DIV can itself clock the timer**

the timer watches a bit of the divider's counter and a write that clears that bit is a falling edge.

### GB-TIM-07

**switching the timer off can clock it once more**

what is counted is the watched bit ANDed with the enable, so clearing the enable is itself a falling edge.

## INT: interrupts

### GB-INT-01

**IF's top three bits read as one**

the three unimplemented bits of IF must read back set.

### GB-INT-02

**EI is delayed and DI cancels it**

an interrupt was taken between EI and the instruction after it.

### GB-INT-03

**EI then NOP does take the interrupt**

interrupts never became enabled at all.

### GB-INT-04

**the lowest vector wins**

with several interrupts pending the lowest-numbered vector must be serviced first.

### GB-INT-05

**HALT with IME clear and a pending interrupt**

the byte after HALT must be executed twice; this is a documented CPU defect, not an option.

### GB-INT-06

**HALT wakes without servicing when IME is clear**

HALT must resume on a pending interrupt even with IME clear, and must not jump to a vector.

### GB-INT-07

**dispatch costs five machine cycles**

taking an interrupt is two idle cycles, two pushes and a vector fetch.

### GB-INT-08

**IE stores all eight bits, IF does not**

the enable register is real storage throughout; the flag register's top three bits are not implemented.

### GB-INT-09

**RETI restores the master enable**

returning from a handler with RETI re-enables interrupts with none of EI's delay.

### GB-INT-10

**clearing IF cancels a pending interrupt**

IF is a register a program can write, and clearing a bit before the interrupt is taken takes the request back.

### GB-INT-11

**clearing IE prevents dispatch and keeps the flag**

IE is consulted at the moment of dispatch, and only dispatch clears a flag.

### GB-INT-12

**a run of EI instructions still enables**

EI arms a latch; an EI that restarts a countdown already running means the enable is never reached at all.

## MEM: the memory map

### GB-MEM-01

**work RAM appears again at $E000**

the echo is the same memory seen through a second door, and games read through it.

### GB-MEM-02

**high RAM is its own 127 bytes**

the bytes at $FF80 are on the processor die, not a window onto work RAM.

### GB-MEM-03

**writes to the ROM area do not stick**

$0000 to $7FFF is where the mapper's registers are decoded; a write there is a command.

### GB-MEM-04

**video RAM is eight kilobytes**

the whole of $8000 to $9FFF must hold what is written with the LCD off.

### GB-MEM-05

**object memory is 160 bytes and stops**

the block above $FE9F is not more object memory.

### GB-MEM-06

**$FEA0-$FEFF answers, per console**

the bytes above object memory are not open bus once the screen is off.

## PPU: timing and the rendering fingerprint

### GB-PPU-01

**a frame is 70224 cycles, measured against DIV**

the PPU and the divider disagree about how long a frame is.

### GB-PPU-02

**a scanline is 456 cycles**

LY is not advancing at one line per 456 cycles.

### GB-PPU-03

**LY counts 0 to 153 and wraps**

the frame is not 154 lines long.

### GB-PPU-04

**STAT reports mode 2, then 3, then 0**

the mode sequence on a visible line is wrong, or VBlank does not report mode 1.

### GB-PPU-05

**the LY equals LYC flag and its interrupt**

the coincidence flag is not latching, or its interrupt does not fire.

### GB-PPU-06

**mode 3 is 172 cycles with nothing to draw**

the shortest possible mode 3 is not the documented length.

### GB-PPU-07

**fine scrolling lengthens mode 3**

SCX modulo 8 must delay the first pixel by that many cycles.

### GB-PPU-08

**objects on a line lengthen mode 3**

each object costs the fetcher at least six cycles; a fixed-length mode 3 cannot show that.

### GB-PPU-09

**the window lengthens mode 3**

starting the window costs at least six cycles.

### GB-PPU-10

**VRAM is unreadable during mode 3**

the CPU must read $FF from VRAM while the fetcher owns it.

### GB-PPU-11

**OAM is unreadable during modes 2 and 3**

the CPU must read $FF from OAM while the PPU is scanning or drawing.

### GB-PPU-12

**only ten objects are drawn per line**

the scan keeps the first ten that cover the line, so beyond ten the cost stops rising.

### GB-PPU-13

**objects switched off in LCDC cost nothing**

the enable bit is read by the scan, not only by whatever draws the pixels.

### GB-PPU-14

**the LCD off means LY 0 and mode 0**

switching the screen off stops the timing chain and resets the line counter.

### GB-PPU-15

**the STAT interrupt is one line, not four**

the selected conditions are ORed together and only the rising edge of the result raises anything.

### GB-PPU-16

**writing STAT raises a spurious interrupt**

on the original silicon the write acts for one cycle as though every condition were selected, and games depend on it.

### GB-PPU-17

**a window past the right edge costs nothing**

the cost is the takeover, not the enable bit.

### GB-PPU-18

**an object off the left edge still costs**

the scan finds it and its row is fetched; only the drawing is thrown away.

### GB-PPU-19

**vertical scrolling costs the fetcher nothing**

only SCX discards fetched pixels; SCY just picks which row of the tile is read.

### GB-PPU-20

**a VRAM write during mode 3 is dropped**

the fetcher owns that bus while it draws, so the write never reaches the memory and the picture never shows it.

### GB-PPU-21

**an OAM write during the scan is dropped**

the PPU owns object memory through modes 2 and 3, which is why every game moves its objects in the blank.

### GB-PPU-22

**object memory corrupts, or does not, per console**

the increment unit drives the address bus with no access behind it, and the original silicon lets that reach the object scan.

### GB-PPU-23

**tile id $19 is unsigned-addressed to $8190**

unsigned tile addressing is $8000 + 16*id; a wrong base or stride is invisible until a tile is identified by number, which every OAM entry and every unsigned BG tilemap byte does.

## DMA: the object transfer

### GB-DMA-01

**the transfer copies 160 bytes**

the object attribute memory does not hold what was sent to it.

### GB-DMA-02

**the transfer is not instantaneous**

160 machine cycles cannot have elapsed one instruction after the transfer began.

### GB-DMA-03

**$FF46 reads back the page written**

the transfer register must read back the last value written to it.

### GB-DMA-04

**the transfer shares the cartridge bus**

the transfer has no bus of its own, so a read on the bus it uses sees its byte.

### GB-DMA-05

**an OAM DMA transfer overrides address bit 12 of a work-RAM access**

on a Color console, $C000 and $D000 can trade places while the transfer's source has that bit set.

### GB-DMA-06

**a transfer stops the scan reading objects**

the controller drives the object address lines, so the scan reads none.

### GB-DMA-07

**a write into the transfer's own source page does not land**

a work-RAM-sourced transfer's own bus answers with its own byte, not the processor's, on any console.

## APU: registers and the length counter

### GB-APU-01

**every register reads back its documented bits**

a sound register is not ORing in the bits that are write-only or unused.

### GB-APU-02

**powering off clears the registers and locks them**

clearing bit 7 of NR52 must zero NR10 to NR51 and ignore writes until the power comes back.

### GB-APU-03

**NR52 reports which channels are on**

a triggered channel must show in NR52, and a channel whose length runs out must stop showing.

### GB-APU-04

**wave RAM is readable while the channel is off**

the sixteen bytes at $FF30 are ordinary memory when channel 3 is not playing.

### GB-APU-05

**a channel with its converter off stays off**

the top five bits of NRx2 drive the converter, and a channel without one cannot be triggered on.

### GB-APU-06

**powering off switches every channel off**

clearing bit 7 of NR52 stops everything, and the status bits go with it.

### GB-APU-07

**wave memory survives a power cycle**

the registers are cleared by a power cycle; the sixteen bytes of wave memory are not.

## MBC: banking and cartridge RAM

### GB-MBC-01

**each ROM bank reads back its own number**

the bank selected at $2000 is not the bank appearing at $4000.

### GB-MBC-02

**bank 0 is translated to bank 1**

writing a bank number of zero to an MBC1 selects bank 1; the mapper cannot map bank 0 twice.

### GB-MBC-03

**cartridge RAM only answers when enabled**

a write to cartridge RAM must be discarded unless $0A was written to $0000.

### GB-MBC-04

**a bank number past the end wraps**

only as many of the register's five bits are connected as the cartridge has banks.

### GB-MBC-05

**the RAM enable decodes four bits only**

any value whose low nibble is $A enables cartridge RAM; anything else locks it.

## BOOT: the state handed over at $0100

### GB-BOOT-01

**the stack pointer starts at $FFFE**

every console's boot ROM leaves SP at the top of high RAM.

### GB-BOOT-02

**A identifies the console**

A must be one of $01, $FF or $11 at handover; software has always used it to tell the machines apart.

### GB-BOOT-03

**the whole register set matches this console**

the boot ROM leaves a documented value in every register, and games read them.

### GB-BOOT-04

**unimplemented I/O reads back as ones**

an address with no register behind it must read $FF, not $00.

### GB-BOOT-05

**the boot ROM leaves its vertical blank pending**

IF reads $E1 at hand-over: the boot ROM waits for the screen before letting go.

### GB-BOOT-06

**the divider is not zero at hand-over**

the counter behind $FF04 has been running since power-on, and games seed their randomness from what it holds.

### GB-BOOT-07

**the cartridge's own header logo decompresses into $8010-$818F**

the monochrome boot ROMs unpack the header's Nintendo-logo bitmap into 24 tiles by a documented algorithm; this recomputes it from the cartridge's own header and checks it against what was actually left in VRAM.

## SER: the link port

### GB-SER-01

**SB is a plain read/write register**

$FF01 holds whatever is written to it while no transfer is running.

### GB-SER-02

**SC's unused bits read as ones**

only bits 7, 1 and 0 exist in SC, and only bit 1 on a Color console.

### GB-SER-03

**an internally clocked transfer completes**

the console drives the clock itself, so a transfer finishes and raises its interrupt whether or not anything is plugged in.

### GB-SER-04

**an externally clocked transfer does not**

with bit 0 of SC clear the other console drives the clock, and with nothing attached the transfer never ends.

<!-- checks:end -->

---

## Refreshing this page

The sections above between the markers are spliced from the check reference the
cartridge ships beside its ROM. Do not edit them here — edit the cartridge, cut
a release, then:

```sh
GBSELFTEST_REF=<tag> tools/fetch-gbselftest.sh
docs/tools/gbselftest_doc.py
```

`docs/tools/gbselftest_doc.py --check` fails if this page has drifted from the
vendored reference, which is what stops a printed URL landing on a heading that
no longer means what the ROM said.
