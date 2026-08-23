# Stable codes — a failure that tells you where to read about it

← [Pitfalls index](README.md)

Every other Game Boy test suite tells you *that* something is wrong. A serial verdict says
`Failed #2`; a screenshot compare says 1,280 pixels differ; a register dump says `B = $48`.
None of them tells a stranger **what behaviour was being asked about, or where to go next.**

This page is the other half. Each row below is a **stable code** — `TGB-PPU-004`, `TGB-DMA-001`
— naming one behaviour, and every code has an anchor on this page. The intent is that
**GBSelfTest — this project's own self-testing cartridge — prints the code beside a failing
check**, so somebody whose emulator fails it can search five characters and land on a
description of the hardware, the public suites that already measure it, and what a wrong
emulator does.

> **Ownership.** This registry is a *contract*, not an implementation: the codes and their
> meanings live here, and the cartridge decides which of them it emits. Adding a code is
> additive and never renumbers an existing one — **a code is permanent once published**, because
> its whole value is that somebody's five-year-old build log still resolves.

> **The per-row counts below are a dated snapshot, deliberately not gated.** They are a
> *triage* of where failures sat on 2026-08-15, not current suite scores, and they must not
> track the baselines — the live figures are on
> [the scoreboard](../measured/scoreboard.md#the-c-sp-aggregation--thirteen-suites-measured-for-the-first-time),
> where `make docs-check` keeps them honest. What is permanent here is the **code and its
> meaning**; the counts are context for choosing what to work on.

**Where the candidates came from.** The thirteen suites in the
[c-sp aggregation](../measured/scoreboard.md#the-c-sp-aggregation--thirteen-suites-measured-for-the-first-time)
were scored here for the first time on 2026-08-15, producing 2,166 Gambatte failures, 175
GBMicrotest failures and 35 wilbertpol-Mooneye failures on the per-dot picture engine. Every
group below is a *named cause* those failures were triaged into, with its row count, so the
denominator is honest.

---

## How to read a row

| column | means |
|---|---|
| **Already covered?** | whether a public suite measures this *well*. **A code marked "well covered" is not a good GBSelfTest check** — duplicating Gambatte's 811 OAM-DMA rows helps nobody. Aim at "thin" and "gap". |
| **From inside a cartridge?** | whether a program running on the console can decide the answer for itself, with no screenshot and no host. Most timing behaviour can: mode-3 duration is a readable fingerprint of what the PPU actually did. |
| **Rows** | how many rows in the suites gated here currently fail for this cause, per the baselines in `testharness/`. |

---

## Picture — the PPU

### `TGB-PPU-001` — mode 3 grows with the objects on the line

**Behaviour.** Every object drawn on a scanline lengthens mode 3, and the amount depends on
where it sits relative to the background's fetch grid: six dots per object, plus
`max(0, 5 - ((X + SCX) % 8))` for the first object on each background tile (Pan Docs,
[Rendering](https://gbdev.io/pandocs/Rendering.html)). Ten objects on one line is the maximum
and the largest signal.

**What a wrong emulator does.** Mode 0 starts too early, so a raster effect written in the
HBlank handler lands on the wrong pixel column, and any program that counts cycles from a
mode-0 STAT interrupt drifts by up to a hundred dots per line.

| | |
|---|---|
| Already covered? | **well** — Gambatte `sprites/10spritesPrLine_*` (476 rows, 235 failing here), Mooneye `acceptance/ppu/intr_2_mode0_timing_sprites` (39 arrangements, passing) |
| From inside a cartridge? | **yes** — arm the mode-0 STAT source and count cycles to the interrupt |
| Our state | Pan Docs' formula is charged explicitly (`Gpu::start_obj_fetch`); Gambatte measures it more finely and we fail 235 of 476 |
| Read next | [measured/ppu.md](../measured/ppu.md), [mealybug.md](../mealybug.md) |

### `TGB-PPU-002` — the window's Y latch, and enabling it mid-frame

**Behaviour.** `WY == LY` latches a per-frame condition **independently of `LCDC` bit 5**, the
window starts on the pixel `WX - 7` exactly (an equality, never a threshold), and its tile grid
is anchored at `WX - 7` even when that is off the left edge.

**What a wrong emulator does.** A game that sets `WY` while the window is off and enables it
later shows no window at all — this was a real defect here, found by a Mealybug ROM.

| | |
|---|---|
| Already covered? | **well** — Gambatte `window/*` (476 rows, 249 failing), Mealybug's `m3_lcdc_win_*` and `m3_wx_*` families, TurtleTests `window_y_trigger` (passing) |
| From inside a cartridge? | **partly** — the latch itself is observable through mode-3 duration; *which pixel* the window starts on is not, and needs a screenshot |
| Our state | the latch and the equality are modelled; 249 Gambatte rows still fail on mid-frame `WY`/enable changes |
| Read next | [mealybug.md §11](../mealybug.md) |

### `TGB-PPU-003` — a register written in the middle of mode 3

**Behaviour.** `SCX`, `SCY`, `BGP`, `LCDC` and the Color palettes take effect part-way through
the line the CPU writes them on, at a dot the fetcher decides.

| | |
|---|---|
| Already covered? | **well, and by the hardest suite there is** — Mealybug Tearoom (79 rows) plus Gambatte `scx_during_m3` (141), `cgbpal_m3` (44), `dmgpalette_during_m3` (17), `vram_m3` (50) |
| From inside a cartridge? | **no** — the evidence is the picture. A cartridge can time mode 3, which is `TGB-PPU-001`, but not see which pixel changed |
| Our state | the per-dot engine reaches <!--score:mealybug_accurate-->29/79<!--/score--> Mealybug; the default engine <!--score:mealybug:pass-->3<!--/score-->/79. **This is the campaign, not a check to add** |
| Read next | [mealybug.md](../mealybug.md), [picture-modes.md](../picture-modes.md) |

### `TGB-PPU-004` — the CPU cannot see OAM or VRAM while the PPU holds them

**Behaviour.** A read of OAM during mode 2 or 3, or of VRAM during mode 3, returns `$FF`; a
write is dropped. **A read is blocked one dot before a write is** — Mooneye ships the same
nineteen offsets read and written (`lcdon_timing-GS` / `lcdon_write_timing-GS`) and the two
tables disagree exactly at the dots the bus changes hands.

**What a wrong emulator does.** It reads the real byte, so a program that uses `$FF` as a
"busy" signal never sees one and loops forever — or, in the other direction, an emulator that
blocks with a *fixed* mode-3 length silently drops writes a game placed legally in HBlank,
which is strictly worse than not blocking at all.

| | |
|---|---|
| Already covered? | **thin** — GBMicrotest `poweron_oam_*` / `poweron_vram_*` (22 rows) and Mooneye's two `lcdon_*` ROMs are the whole of it, and both are boot-relative |
| From inside a cartridge? | **yes, easily** — the cleanest self-check there is: write a known byte, wait for a mode, read it back |
| Our state | deliberately **off** on the default engine (a fixed 172-dot mode 3 would drop legal writes) and **on** in the per-dot one, which is most of its +87 GBMicrotest rows |
| Read next | [measured/ppu.md](../measured/ppu.md) |

### `TGB-PPU-005` — turning the LCD on does not start a normal line

**Behaviour.** The line the PPU restarts on has **no OAM scan**: `STAT` reads mode 0 through
the OAM slot and goes straight to mode 3. The coincidence flag is a latched flip-flop whose
clock stops with the LCD, so a `LYC` write while the LCD is off is invisible and re-enabling
re-latches it against the `LY` the PPU restarts from.

| | |
|---|---|
| Already covered? | **well** — Gambatte `enable_display` (184), `lcd_offset` (62), `display_startstate` (14); GBMicrotest `lcdon_to_stat*`, `lcdon_to_oam_*`; Mooneye `ppu/stat_lyc_onoff` |
| From inside a cartridge? | **yes** — read `STAT` at fixed offsets after the `LCDC` write |
| Our state | 116 of 260 Gambatte rows in those three groups still fail |
| Read next | [measured/ppu.md](../measured/ppu.md) |

### `TGB-PPU-006` — the `LY == LYC` comparator blacks out for the dot `LY` changes

**Behaviour.** It is a suppression, not a hold: at the next line's dot 0 with a stale `LYC` the
flag must read *clear* too. Line 153 is where every emulator differs, because `LY` is 153 for
only a few dots before it reads 0 while still on that line.

| | |
|---|---|
| Already covered? | **well** — GBMicrotest `line_153_*` (26 rows), Gambatte `lycEnable` (225) / `lycint_*` / `lyc153int_m2irq`, wilbertpol Mooneye's fourteen `gpu/ly_lyc*` ROMs |
| From inside a cartridge? | **yes** — poll `LY` and `STAT` bit 2 across the line-153 boundary |
| Our state | 81 `lycEnable` rows and 8 `line_153_*` rows fail on the per-dot engine |
| Read next | [measured/ppu.md](../measured/ppu.md) |

### `TGB-PPU-007` — writing `STAT` can fire an interrupt that was not there

**Behaviour.** The STAT interrupt is **one line**, not four triggers: the enabled sources are
ORed and `IF.1` rises only on the 0→1 transition. A write to `STAT` moves a term of that OR, so
enabling a source whose condition already holds raises the interrupt immediately.

**What a wrong emulator does.** Firing per source makes a raster split driven off both the LYC
and the HBlank source run at twice the intended rate — a real, visible defect here once.

| | |
|---|---|
| Already covered? | **thin, and oddly** — GBMicrotest `stat_write_glitch_l0/l1/l143/l154` (15 rows) and wilbertpol's two `gpu/stat_write_if`; Gekkio's modern suite dropped the glitch ROMs |
| From inside a cartridge? | **yes** — clear `IF`, write `STAT`, read `IF` |
| Our state | 10 of the 15 GBMicrotest rows and both wilbertpol rows fail |
| Read next | [measured/ppu.md](../measured/ppu.md) |

### `TGB-PPU-008` — the mode-0 interrupt moves with `SCX`

**Behaviour.** `SCX & 7` lengthens mode 3 by up to seven dots, so the mode-0 STAT interrupt
arrives later; and the fractional scroll is re-read on every discarded dot rather than latched
when mode 3 began.

| | |
|---|---|
| Already covered? | **well** — GBMicrotest `int_hblank_*_scx` (24 rows, **0 passing**) and `hblank_int_scx*` (40); Mooneye `ppu/hblank_ly_scx_timing-GS` |
| From inside a cartridge? | **yes** — this is the single most discriminating cheap check available: set `SCX`, time the mode-0 interrupt, repeat for all eight values |
| Our state | the whole `int_hblank_*_scx` family fails on both engines |
| Read next | [mealybug.md §11](../mealybug.md) |

---

## Direct memory access

### `TGB-DMA-001` — while OAM DMA runs, the CPU sees the DMA's bus

**Behaviour.** OAM DMA has no bus of its own: it drives the source address onto whichever bus
that address lives on and reads the byte back off it, for 160 M-cycles. A CPU access on the
same bus in the same M-cycle is looking at the same wires — a read gets the transfer's byte, a
write never reaches its own address and is what the transfer copies into OAM instead. **A DMG
has two buses and a Color console has three**, its work RAM being off the cartridge bus, so the
two consoles disagree across most of the group. OAM itself reads `$FF` while writes are dropped.

**What a wrong emulator does.** Games copy their OAM routine into HRAM precisely because of
this, so an emulator that gets it wrong mostly survives — until a game reads a table during a
DMA, at which point it gets a byte that has nothing to do with the address.

| | |
|---|---|
| Already covered? | **exhaustively** — Gambatte `oamdma/` is 811 rows, the largest single group in the suite. 562 failed here when this row was written; 137 do now |
| From inside a cartridge? | **yes** — start a DMA from a known source and read a byte whose real value differs |
| Our state | **modelled** since 2026-08-18 (`MemoryManagementUnit::dma_conflict`), worth +427 Gambatte rows in each picture mode with no regression once the prohibited block `$FEA0-$FEFF` was given its per-console answer too. What is left is `TGB-DMA-002` |
| Read next | [`docs/conformance-notes.md`](../conformance-notes.md), 2026-08-18 |

### `TGB-DMA-002` — an object's position written late relative to a DMA

| | |
|---|---|
| Already covered? | **well** — Gambatte `oamdma/late_sp*` |
| From inside a cartridge? | **no** — the evidence is where the object was drawn |
| Our state | fails; downstream of `TGB-DMA-001` |

### `TGB-CGB-002` — HDMA/GDMA cycle cost, and reads during a VRAM DMA

| | |
|---|---|
| Already covered? | **well** — Gambatte `dma/` (229 rows, 117 failing), SameSuite `dma/` (4/4 passing here) |
| From inside a cartridge? | **yes** — count `DIV` across a GDMA of known length |
| Our state | SameSuite's four rows pass; Gambatte's finer cycle counts do not |
| Read next | AGENTS.md, "CGB VRAM DMA" |

---

## Clocks

### `TGB-TIM-001` — stopping and restarting the timer mid-tick

**Behaviour.** `TIMA` counts falling edges of one bit of the 16-bit system counter ANDed with
`TAC` bit 2, so clearing and re-setting the enable bit at the wrong moment produces an extra
tick that no "increment every N cycles" model has.

| | |
|---|---|
| Already covered? | **well** — Mooneye `acceptance/timer/*` (13/13 passing here), Gambatte `tima/` (232 rows, 29 failing, concentrated in `tc00_1stopstart_offset2_ff_tma`) |
| From inside a cartridge? | **yes** — the classic self-check: write `TAC`, count, read `TIMA` |
| Our state | the falling-edge model is right; the `TMA`-write-during-reload corner is where the 29 sit |
| Read next | [measured/timing.md](../measured/timing.md) |

### `TGB-SER-001` — the serial interrupt's phase against `DIV`

**Behaviour.** An internally-clocked transfer shifts on falling edges of system-counter bit 8,
so a `DIV` write mid-transfer moves when `IF.3` rises — and an unanswered transfer still
completes, because the console drives the clock.

| | |
|---|---|
| Already covered? | **thin** — Gambatte `serial/` (82 rows, 39 failing) is essentially the only suite that measures it; Mooneye has one boot-alignment ROM |
| From inside a cartridge? | **yes** — start a transfer, write `DIV`, poll `IF` |
| Our state | the edge model is in (`Timer::take_serial_edges`); the `DIV`-write interaction is where all 39 sit |
| Read next | [measured/serial.md](../measured/serial.md) |

### `TGB-CGB-001` — the double-speed switch takes a fixed time and resets `DIV`

| | |
|---|---|
| Already covered? | **well** — Gambatte `speedchange/` (244 rows, 82 failing), daid's `speed_switch_*`, SameSuite |
| From inside a cartridge? | **yes** — `DIV` and `TIMA` across the switch |
| Our state | `SPEED_SWITCH_HALT_TICKS` is calibrated; the PPU's phase across the switch is a known open question |
| Read next | AGENTS.md, "`STOP`, the CGB speed switch" |

---

## Sound

### `TGB-APU-001` — a square channel's duty position is a latch, and it starts somewhere

**Behaviour.** The duty output is a **latch** updated only at a frequency-timer expiry, not
`DUTY_PATTERN[duty][pos]` evaluated on demand, so a freshly powered channel reads 0 for its
whole first position. Which position a trigger leaves it in, and what the DAC emits there, is
what Gambatte's `ch<N>_duty<D>_pattern_pos<P>` family measures.

**What a wrong emulator does.** It is silent where hardware is not.

| | |
|---|---|
| Already covered? | **well** — Gambatte `sound/` (300 rows, 73 failing) and SameSuite's APU half (69 rows, 66 passing here) |
| From inside a cartridge? | **yes on a Color console, no on a DMG** — `PCM12`/`PCM34` (`$FF76`/`$FF77`) report the live DAC nibble of each channel, which is exactly how SameSuite reads the APU. A DMG has no such register and the check would need a host listening |
| Our state | **measured, and it is not a runner artefact**: 71 rows where we emit a flat 0.0 for the whole judged frame and hardware does not. Verified by re-running the audio player at the Game Boy's own 2 MiHz rate (`Sound::set_player` honours it) *and* by widening the window from one frame to a whole second — **neither changes a single verdict** — and by confirming the APU does produce non-zero output on 37 other rows in the same group |
| Read next | [measured/apu.md](../measured/apu.md), [components/apu.md](../components/apu.md) |

---

## Cartridges

### `TGB-MBC-001` — a mapper's bank register is as wide as the cartridge needs

**Behaviour.** MBC3's ROM-bank register is seven bits; **MBC30's is eight**, and no
cartridge-type byte tells them apart. A cartridge with more than 128 banks cannot be addressed
by seven bits at all, so its ROM size is what decides. (Pan Docs,
[MBC3](https://gbdev.io/pandocs/MBC3.html) and its MBC30 section: *"the only title to be
shipped with the MBC30 mapper was Pocket Monsters: Crystal Version in Japan"*.)

**What a wrong emulator does.** Half the cartridge is invisible: reads from banks `$80-$FF`
return whatever the corresponding low bank holds. Corrupt graphics, missing text, and code
jumping into the wrong bank.

| | |
|---|---|
| Already covered? | **gap** — [MBC3 Bank Tester](https://github.com/EricKirschenmann/MBC3-Tester-gb) is the *only* ROM anywhere that exercises the eighth line, it is a 4 MiB download, and it is judged by a screenshot |
| From inside a cartridge? | **yes, and this is the ideal GBSelfTest check** — write a bank number, read one byte, compare. It needs a large ROM, which is the only reason nobody has one |
| Our state | **fixed 2026-08-15** (`MBC3::bank_mask`); `mbc3-tester` FAIL → PASS on both consoles |
| Read next | [measured/cartridges.md](../measured/cartridges.md), `docs/conformance-notes.md` (2026-08-15) |

### `TGB-MBC-002` — cartridge-supplied input must never abort the emulator

**Behaviour.** Not a hardware behaviour at all: a mapper register, a ROM-size code or a DMA
destination that hardware simply ignores must not be a panic. Under `panic = 'abort'` a panic
is process death *and* the loss of any cartridge RAM not yet written.

| | |
|---|---|
| Already covered? | **gap, and a permanent one** — no public suite has any reason to ship a ROM that tries to crash its host |
| From inside a cartridge? | **yes** — a check that pokes every mapper register with every value and then reports that it is still running is a perfectly good self-test, and one no existing suite offers |
| Our state | five aborts of this class have been fixed here (MBC divide-by-zero, RAM-less MBC5+RUMBLE, unofficial `$0148` codes, VRAM-DMA destination wrap, undefined opcodes); covered by Rust tests only |
| Read next | AGENTS.md, "Battery saves" and "CGB VRAM DMA" |

### `TGB-MBC-003` — the cartridge clock is driven by emulated time

**Behaviour.** The MBC3 RTC counts the console's own cycles, not the host's wall clock, and its
registers are counters rather than a time: a value loaded above its natural maximum counts up
to the top of its physical width and wraps **without carrying**.

| | |
|---|---|
| Already covered? | **adequately** — [rtc3test](https://github.com/aaaaaa123456789/rtc3test) (3 sub-tests, 6 rows, all passing here) and CasualPokePlayer's `latch-rtc-test` |
| From inside a cartridge? | **yes**, and rtc3test already does it well — **do not duplicate** |
| Our state | 6/6 and 4/4 |
| Read next | [measured/cartridges.md](../measured/cartridges.md) |

---

## Boot

### `TGB-BOOT-001` — where the PPU is when the cartridge starts

**Behaviour.** The boot ROM hands over mid-frame, at a point that is a property of the console
revision. GBMicrotest pins it directly: `poweron_ly_119` is 119 `nop`s and one `LDH A,[$FF44]`.

| | |
|---|---|
| Already covered? | **thin** — GBMicrotest `poweron_ly_*` / `poweron_stat_*` (22 rows) is the only suite that measures it on a plain DMG; Mooneye's `boot_hwio` masks skip `$FF41`/`$FF44` on every console except DMG 0 |
| From inside a cartridge? | **yes** — it is the simplest check in this whole registry, and it is why GBMicrotest can do it in nine bytes |
| Our state | **measured and deliberately not "fixed"**: hardware reads `LY = 0` at 119 M-cycles and `1` at 120 where we roll over at 114. Sweeping the constant gives 237/513 at dot 69900, 251 at line 0 (today), 262 at 70168 and 250 at 70184 — no clean optimum, which is the signature of a constant absorbing an error elsewhere |
| Read next | [measured/boot.md](../measured/boot.md), `src/mmu/mod.rs::boot_ppu_dot` |

---

## Things a cartridge can never check

These are in the registry so that a reader who *has* one of these symptoms is not left hunting
the emulation for it. All three leave the framebuffer perfectly correct.

| Code | Behaviour | Where it is written up |
|---|---|---|
| `TGB-HOST-001` | cost per displayed frame must not grow with the output size | [rendering.md](../rendering.md), conformance record #1 |
| `TGB-HOST-002` | an unchanged frame must produce zero output | [rendering.md](../rendering.md), conformance record #2 |
| `TGB-HOST-003` | a long run must not grow terminal-side resource use without bound | [rendering.md](../rendering.md), conformance record #3 |
| `TGB-HOST-004` | a missing optional capability must cost only that capability | [display.md](../display.md), and the graceful-degradation record in AGENTS.md |
| `TGB-HOST-005` | persistence: an in-game save must survive a kill, and a save that cannot be read is never overwritten | [measured/cartridges.md](../measured/cartridges.md), `testharness/battery.rs` |

---

## For the GBSelfTest crew, in one table

Ranked by *what a new check would buy*, which is not the same as by row count: a behaviour
eleven public ROMs already measure needs no fourteenth.

| Rank | Code | Why |
|---:|---|---|
| 1 | `TGB-MBC-002` | **nothing public covers it and nothing ever will** — no suite ships a ROM designed to crash its host, and this class has cost this project five real aborts |
| 2 | `TGB-MBC-001` | one ROM in the world tests it, it is 4 MiB, and it is screenshot-judged. A register-level check is a few bytes |
| 3 | `TGB-PPU-004` | trivially self-checking, thinly covered, and it *discriminates*: a rough emulator returns the real byte and a careful one returns `$FF` |
| 4 | `TGB-PPU-008` | the single most discriminating cheap timing check available — eight `SCX` values, one interrupt each |
| 5 | `TGB-PPU-007` | covered only by ROMs the modern Mooneye suite dropped, so it is quietly rotting out of the ecosystem |
| 6 | `TGB-SER-001` | one suite measures it at all, and the link port is the half no ROM-driven runner reaches |
| 7 | `TGB-BOOT-001` | nine bytes per check, and it pins a constant most emulators fudge |
| — | `TGB-PPU-001/002/005/006`, `TGB-DMA-001`, `TGB-TIM-001`, `TGB-CGB-001/002`, `TGB-APU-001` | already covered *exhaustively* by Gambatte and Mooneye. Worth a **thin** check each, as a fingerprint, not a reimplementation |
| — | `TGB-PPU-003`, `TGB-DMA-002` | **not diagnosable from inside a cartridge** — the evidence is the picture. Leave them to Mealybug |
| — | `TGB-MBC-003` | rtc3test already does this well. Do not duplicate |
