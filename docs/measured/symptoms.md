# Symptoms — "my emulator does X" → what it actually is

← [The measured Game Boy](README.md)

A reverse index. Every row is a defect this project actually shipped, then found, then fixed —
so each is a real thing an emulator does, with the test that catches it and the hardware
behaviour it violates. The working for all of them is in
[`docs/conformance-notes.md`](../conformance-notes.md), newest-first.

The column that matters most is the last one. **Where it says *nothing*, that is a finding**:
a whole class of defect that a ROM-driven runner is structurally unable to see.

---

## On screen

| Symptom | Cause | Caught by |
|---|---|---|
| A game's window never appears, though it sets `WY` correctly | the window's Y condition latched behind `LCDC` bit 5 — Pan Docs' [rendering criteria](https://gbdev.io/pandocs/Window.html) says `WY == LY` latches per scanline regardless of the enable bit | Mealybug |
| A raster split runs at twice its intended rate | STAT fired once **per enabled source** instead of on the ORed line's 0→1 edge ([ppu.md](ppu.md#the-stat-interrupt-is-one-line-not-four-triggers)) | Mooneye `ppu/*`; **affects real games** |
| Everything a program times from the mode-2 interrupt lands one M-cycle late, all frame | the mode-2 STAT strobe not leading its scanline by 4 dots on lines 1–143 | `acceptance/ppu/intr_2_0_timing`, `intr_2_mode0_timing`, `intr_2_mode3_timing` — all three by exactly one loop iteration |
| A CGB game loses one STAT interrupt per frame; everything its handler toggles is on the wrong parity of scanlines | the line-144 OAM strobe missing | `acceptance/ppu/vblank_stat_intr-GS` — **a PPU test ROM catching an interrupt bug** |
| Mid-scanline palette/scroll bands are missing | a whole-scanline renderer. **Expected in the default `standard` [picture mode](../picture-modes.md), by design** — `identical` (`P`, or `GB_PPU=identical`) is the per-dot one | Mealybug (79 rows) |
| Objects on a line cost the wrong amount of mode 3, so mid-line writes land on the wrong pixel; a line full of objects at `X < 8` runs short | the OBJ penalty derived from fetcher phase rather than charged as Pan Docs' formula | `acceptance/ppu/intr_2_mode0_timing_sprites` — 39 arrangements |
| Nothing draws at all after the LCD is switched on, and the program runs off the end of its code | the restart line's mode-3 entry skipped, so no mode-2 STAT interrupt ever arrives | every Mealybug mode-3 ROM — **and nothing in the in-repo harness**, because dmg-acid2 recovers at the first VBlank |
| A CGB game that bulk-loads VRAM with the LCD off gets **nothing copied at all** | an HBlank DMA armed only on the *transition into* mode 0, where with the LCD off there is never another entry | SameSuite `dma/hdma_lcd_off`, `dma/hdma_mode0`. **Nothing in Blargg, Mooneye or acid2** |
| Half a game's tile data never arrives and the other half is duplicated | `HDMA1-4` kept as a shadow copy and re-latched on every `$FF55` write, so a second transfer repeats the first block. They **are** the address counters | SameSuite `dma/gbc_dma_cont` |
| A screenshot-judged suite marks the emulator FAIL across the *whole* frame, while the ROM's own verdict says PASS | the boot ROM's leftover tilemap not written — one shade fewer on screen re-ranks every white pixel under a shade-rank comparison ([boot.md](boot.md)) | both SameSuite `sgb/` ROMs, but **only under the image comparison**, never under their own report |

## Timing and interrupts

| Symptom | Cause | Caught by |
|---|---|---|
| Games freeze or mis-time interrupts after a `HALT` with `IME=0` and one pending | the HALT bug not modelled ([Pan Docs, HALT](https://gbdev.io/pandocs/halt.html)) | Blargg `halt_bug` |
| A run of `EI` instructions never enables interrupts at all | `EI`'s delay implemented as a restartable counter instead of a latch | `acceptance/ei_sequence` (18 in a row) |
| Every instruction-timing test fails and the CPU looks broken | **OAM DMA is instantaneous.** 14 Mooneye instruction-timing tests measure which M-cycle an access lands on by DMAing a byte into OAM — none can pass however correct the CPU is | `acceptance/oam_dma*` + those 14 |
| A game's music tempo drifts, or a timer glitch fires one iteration late | the timer built as "a divider and a counter" rather than one tapped 16-bit counter ([timing.md](timing.md#the-timer-is-the-circuit-not-a-divider-and-a-counter)) | `acceptance/timer/*`; the one-iteration case is `rapid_toggle` **alone** |
| A game that starts an internally clocked serial transfer and counts cycles measures zero — or the transfer never completes with nothing plugged in | an untimed serial port. The console drives the clock, so the transfer runs regardless and the byte is `$FF` | `acceptance/serial/boot_sclk_align-dmgABCmgb` |

## Boot and machine identity

| Symptom | Cause | Caught by |
|---|---|---|
| A game's "random" encounters are identical every launch | the system counter starting at zero rather than the boot ROM's leftover (`$ABCC` DMG / `$2678` CGB) | `acceptance/boot_div-*` — which pins the counter's **low bits**, not just `DIV`'s top byte |
| A game enables VBlank, `EI`s at start-up, and waits a whole frame for an interrupt hardware already had pending | `IF` initialised to `$E0` instead of `$E1` | `acceptance/boot_hwio-dmgABCmgb`, `misc/boot_hwio-C`. **Nothing in the in-repo harness, and nothing would**: Blargg and acid2 write every register they depend on |
| A boot-register test prints `HL: 007C` where a DMG shows `014D`, though every assertion inside it passed | a Color machine booted for a DMG cartridge. **Nothing a player sees; everything a conformance run sees** | 9 Mooneye rows, FAIL→PASS with a DMG boot, zero regressions |
| A cartridge cannot tell an MGB from a DMG, or an AGB from a CGB | `A = $FF` (MGB) and `B` bit 0 (AGB) not modelled — one `inc b` at the end of the AGB boot ROM explains both `B` and its `F` | `boot_regs-mgb`, `boot_regs-A` |
| An SGB plays a start-up ding it should not | `NR14 = $87` copied from the DMG; an SGB's last audio write is `$07`, so channel 1 is set up and never triggered | `boot_regs-sgb`, `boot_regs-sgb2` |
| Channel 1 switches itself off ~4 ms after boot and `NR52` reads `$F0` | writing the *read-back* value `$BF` into `NR11`/`NR14` — that loads a length counter of 1 **with length enabled** ([boot.md](boot.md#1-do-not-write-the-read-back-values-into-nr11--nr14)) | `boot_hwio`, which reads `NR52` ~2.8 ms in and wants `$F1` |

## Audio

| Symptom | Cause | Caught by |
|---|---|---|
| CGB double-speed games play an octave high **and** run at half speed | the APU clocked at CPU speed instead of the base clock. (The second half is emergent: the audio queue paces the emulator) | Blargg `cpu_instrs` measured **33.7 fps / 48 % frames late** before, 59.70 / 0 % after |
| `NR52` reads back "powered off" on a machine that has audio; 57 of 61 APU tests cannot start | **no `AudioPlayer` installed — the APU is absent, not quiet.** Any APU measurement without a player is measuring nothing | the divergence between two of our own harnesses reporting different verdicts for the same emulator |
| A music driver's mid-note volume ramps never resume after a period-0 write | the envelope's period counter implemented as stopping rather than free-running | SameSuite `channel_N_nrx2_speed_change` |
| Prehistorik Man breaks | CPU CGB 0/B's extra-length-clocking silicon bug modelled on a machine that should be revision C or later, or the reverse | SameSuite's five `extra_length_clocking-*` ROMs — **and `-cgb0` and `-cgbB` contradict each other**, so a runner that ignores the filename can never score more than four of five |
| Playback is "crunchy when quiet" | attenuating an integer sample instead of scaling in `f32` before the device-format conversion | nothing automated — this is a listening result |
| The emulator runs at ~5 fps with sound enabled and ~60 without | an `AudioPlayer` that **blocks** inside `Cpu::do_cycle`. Measured 17.93 ms/frame against a 16.743 budget, **97.6 % of it blocked rather than executing** | `make test-audio-perf` — needs the `audio` feature but **no sound card** |

## Cartridges and saves

Full detail in [cartridges.md](cartridges.md); the short list:

| Symptom | Caught by |
|---|---|
| Save in-game, quit, relaunch → "NEW GAME" only | **nothing** — no public suite touches persistence; they run in one process and never look at the filesystem |
| A foreign or real-cartridge `.sav` loads 8 bytes shifted | **nothing** |
| Any rumble game crashes the emulator on its first motor poke | **nothing** — there is no rumble or RAM-less MBC5 ROM in any suite |
| A crashed game corrupts the save file that was fine a second ago | **nothing** |
| A compilation cartridge's menu runs the wrong game | Mooneye `emulator-only/mbc1/multicart_rom_8Mb` — verified by its author against a real MBC1B1 chip |
| A ROM-only or MBC5 game is fine on the desktop and garbage in the browser | **nothing** — every harness passes a save path, which is why the mapper-selection bug survived |

---

## Methodology defects — the harness was wrong, not the emulator

The most expensive bugs on this page were not in the emulator at all. Each of these produced
confident, wrong numbers for a long time.

**A harness that judged itself.** Four ROMs' expected result was an **FNV hash of our own
framebuffer**, and the documented response to a changed hash was to re-bless it. So CI would
have stayed green straight through any regression in those four — and `oam_bug` was recorded as
an opaque number that in fact meant *"3 of 8 sub-tests pass"*. Every expectation is now somebody
else's: the ROM's serial string, the text it writes to cartridge RAM, or its author's reference
image. **Never record a hash of your own output as the expected result.**

**A test machine with no APU is not a real machine.** With `sound` left at `None`,
`interrupt_time` drew only its title and hung — written off for months as "cannot run
headless", when in fact it spins on `NR52` — and `dmg_sound/01` reported *Failed*. Both are
artefacts of the harness. **Never build a test machine without an APU.**

**A stale binary looks exactly like a regression.** The suite scouts are `cargo build` targets.
After breaking source for a teeth test you **must rebuild** before trusting any later
measurement; one that was not reported `timer 11/13` on a clean tree.

**Check which population a number belongs to.** Mooneye `acceptance` is 75 rows here and 66 in
both the community's usual denominator and the Shootout's manifest. A commit that moved eleven
real rows moved the Shootout number by **exactly zero**, and that was correct rather than a
measurement error. ([scoreboard.md](scoreboard.md#three-different-denominators-all-called-mooneye))

**Two verdict methods disagreeing is the finding.** It happened twice, in opposite directions.
A Color STAT/VBlank change made the ROM's own report say PASS and a hardware photograph say
FAIL — the photograph was right. The boot tilemap made two ROMs report PASS while failing an
image comparison — the image was right. **Do not pick the flattering one.**

**When a test synchronises through one subsystem to measure another, its failure names the
wrong one.** Blargg `interrupt_time` was diagnosed as "the CGB double-speed switch does not
take effect". The switch worked; the ROM synchronises through the APU, which was clocked at CPU
speed. And 14 instruction-timing failures named the CPU when the defect was in OAM DMA.

**A catch-all arm over a documented enum is a defect until you enumerate what it swallows.**
`wIsInBattle == $FF` means *the player blacked out*, not "a trainer battle" — the disassembly
documents four values above the label. The symptom was a full battle panel, with confident
stale data, displayed in the overworld. The test that catches it iterates **every** byte
`3..=$FF`, not the named cases.

---

## What no test ROM can see

The blind spot is structural, not a gap somebody forgot to fill: a ROM-driven runner compares
framebuffers and serial output, so anything that leaves the framebuffer *correct* is invisible
to it.

| Class | Example measured here |
|---|---|
| **Persistence** | every save-file defect above. Suites run in one process and never touch the filesystem |
| **Host survival** | four abort-on-guest-input defects. No ROM tries to crash its host |
| **Presentation cost** | the terminal transport sized by the window: the framebuffer is perfect and the player sees ~18 fps. Bytes on the wire fell ~1,000× when it was capped |
| **Startup and capability negotiation** | the frontend refusing to run in a terminal without an optional keyboard extension. Every ROM harness constructs a `Gameboy` directly and never enters the frontend at all |
| **Sub-sample APU timing** | the square frequency-reload latch. `dmg_sound` stays 12/12 with **every captured audio hash unmoved** on both sides |
| **Consumers of correct state** | the blacked-out sentinel above. The cartridge was entirely correct; the defect was in something reading it |

This is the argument for a machine-readable telemetry stream being the reference adapter for a
standalone conformance tool: `speed_pct`, `fps`, `tx_bytes` and `frames_dropped` expose the
presentation class, and a framebuffer hash exposes none of it. See
[`docs/conformance.md`](../conformance.md) and
[`docs/conformance-action.md`](../conformance-action.md), which is the shippable form —
a GitHub Action that scores *foreign* emulators through a two-output adapter contract.

> One design decision from that action worth repeating here, because it is the same failure
> shape as "a harness that judged itself": **a crash during capability probing must mean
> "assume the capability exists"**. Get it backwards and an emulator that segfaults on every
> ROM produces no files, reads as *no capabilities*, and scores **0 out of 0** — which a badge
> renders as a clean slate.
