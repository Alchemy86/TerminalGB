# Conformance notes — raw material for the standalone evaluation tool

Standing convention (captain, 2026-08-13): every real emulation bug we fix is a
ready-made test case for the planned standalone Game Boy **conformance tool** —
one that explains *why* a test failed and *what behaviour is missing*, not just
pass/fail. Capture the reasoning at the moment of the fix, per entry:

- **Symptom** — what a player or a test observes, in user terms (a foreign
  emulator with the same bug shows the same thing).
- **Caught by** — which test ROM or harness check catches it, or explicitly
  none (a suite gap is itself a finding).
- **Hardware behaviour** — what the silicon actually does, with a citation.
- **Carries into the tool** — any new test written here that the standalone
  tool should absorb.

Entries are newest-first.

---

## 2026-08-14 — a square channel's frequency reload is latched a few cycles late

**SameSuite 71/76 → 72/76** (`channel_1_freq_change_timing-cgbDE` FAIL → PASS,
APU half 64/69 → 65/69). Measured with `make test-external` — the CI gate, on
the ROMs pinned in `tools/fetch-test-suites.sh` — before and after. Everything
else is byte-identical across the change: Mooneye 103/103 and 8/8 rows,
Mealybug 79/79 rows, `cpp` 4/4, `testroms` 46/47, `audiocapture` (`dmg_sound`
12/12 **and every hash**), `savestate`, `battery` 8/8, the audio-perf gate
(0.253 ms/frame, 66x real time) and 469 library tests. The accurate PPU engine's
gate (`make test-external-accurate`) is unchanged too, as it must be — this is
not a PPU change.

**Symptom.** A game that retunes a playing note by writing NRx3/NRx4 — which is
every vibrato, portamento and pitch bend in every music driver — puts the change
one sample step in the wrong place when the write lands within a few cycles of
the channel's own frequency timer expiring. It is a sub-sample error, so it is
inaudible in isolation; what it corrupts is the *phase* a driver has been
carefully counting, and a driver that retunes every frame accumulates it.

**Caught by.** SameSuite `channel_1_freq_change_timing`, and only there. The
suite ships it as three ROMs — `-A`, `-cgb0BC`, `-cgbDE` — that differ **only**
in their `CorrectResults` tables, because the three machines genuinely disagree.
Nothing else in any suite we run sees it: Blargg `dmg_sound` is 12/12 with every
captured audio hash unmoved on both sides of this fix, which is itself the
finding — **a whole class of sub-sample APU timing is invisible to a suite that
judges by ear or by checksum**, and it took a ROM that reads PCM12 twice around
a single write to expose it.

**Hardware behaviour.** A square channel's frequency timer counts down and, at
each expiry, reloads itself from NRx3/NRx4. The reload is not instantaneous: the
period is sampled a few T-cycles *after* the expiry, so a write landing inside
that window still supplies the period the timer has just reloaded with. The next
sample then arrives one whole **new** period from that expiry, instead of the
old countdown running out first. Outside the window the ordinary documented rule
holds — "a frequency write takes effect at the next expiry", the rule
`channel_1_freq_change`'s own header states as *"changing channel 1's frequency
takes effect after the current sample finishes"*.

The window is **measured, not chosen**. The ROM sweeps the write across the
expiry one `nop` at a time at single *and* double speed and reads PCM12 twice —
immediately after the write, and once more when the channel has frozen on the
sample it will hold — so it constrains the constant from both sides at both
clock rates. The passing band is `5..=6` T-cycles, *uniform across both speeds*;
`≤4` swaps our verdict onto `-A`'s table and `≥7` fails all three.
`SQUARE_RELOAD_LATCH = 6` (`src/sound.rs`), chosen inside that band to match
`SQUARE_TRIGGER_DELAY`, the square channel's other measured start latency.

**Two of the three ROMs are deliberately left red, on evidence.**

- **`-A` is the Game Boy Advance, not a CGB revision.** Every CGB stepping tag
  in this suite is spelled `cgb…` (`-cgb0`, `-cgbB`, `-cgb0B`, `-cgb0BC`,
  `-cgbDE`); `-A` is the gbdev-wide suffix for the AGB, and SameSuite's
  `apu/README.md` names *"the Game Boy Advance line"* as the one device family
  still to be verified. We do not emulate an AGB at all, so passing it would
  have been an accident — and we did briefly, at latch 4, which is exactly how
  the constant's lower bound was found.
- **`-cgb0BC` is blocked on a quirk its own author has not characterised.** Its
  table differs from `-cgbDE` in the *immediate* PCM12 read, not in the frozen
  sample — i.e. in the read-back, not the frequency timer. `apu/README.md`
  records it: *"A quirk in CPU-CGB revisions C and older makes registers PCM12
  and PCM34 report a glitched PCM amplitude … if they're read in the same
  M-cycle they change. This behavior needs to be understood, tested and
  documented."* That is the documented reason C-and-older fail much of the
  suite. There is no reference implementation to calibrate against.

  This also **retires a note that stood in `CLAUDE.md` and in `CgbRevision`'s
  own doc comment**: that these three rows would need `CgbRevision::CgbC` split
  from a `CgbDE`. They would not. Splitting the enum buys nothing, because the
  other two rows are a console we do not emulate and a quirk nobody has
  documented — not a revision knob we are missing.

**The subtlety worth keeping.** "Time since the last expiry" **cannot** be
inferred as `period - delay`, and getting that wrong is not a rounding error —
it is a regression in two other rows. A freshly *triggered* channel walks
through exactly that range with no expiry behind it at all (a trigger loads the
timer itself), and `channel_1_freq_change` / `channel_2_freq_change` write NRx3
precisely there: the first attempt at this fix derived the window arithmetically
and turned both of those FAIL. `SquareChannel::since_expiry` therefore tracks
the real thing, is parked out of range by a trigger, and is live machine state —
`savestate::FORMAT_VERSION` is **15**.

**Carries into the tool.** Three device-free unit assertions in
`src/sound.rs`, each pinned to the rule applied to a known timer rather than to
a recording of our own output, and each proved to fail against the specific
defect it names: `a_frequency_write_just_after_an_expiry_redoes_that_reload`
(teeth: remove the retro-apply), `the_latch_does_not_reach_back_past_a_trigger`
(teeth: derive the window from `period - delay` — the exact bug above) and
`a_frequency_write_outside_the_latch_leaves_the_countdown_alone`. Conformance
candidate for the standalone tool: **a mid-note frequency write must move the
next sample by a whole new period when it lands in the reload window, and not
at all when it does not** — checkable through the PCM registers on any CGB.

---

## 2026-08-14 — the Super Game Boy: a joypad port that is also a command channel

Not a fix to an existing subsystem — a subsystem that was not there. Three of
the GB Emulator Shootout's 264 published rows are SGB tests and all three were
red; the audit that commissioned this work rated it lowest priority and
recommended declaring SGB out of scope, on the grounds that it is "a new
subsystem" and that bgb fails two of the three as well.

**The audit's size estimate was wrong, and the reason is worth recording**: all
three reference screenshots are plain 160×144 DMG-grey screens. No border, no
SGB palette, no SNES side is involved in any of them. Every one of the three
tests measures the same thing — the command-packet receiver on `$FF00` and the
`MLT_REQ` multiplayer joypad index — because that is the *only* part of an SGB a
Game Boy program can read back. Everything else an SGB does (border, palettes,
sound) is one-way traffic to a screen the cartridge cannot see, so no test ROM
can assert it. The whole of it is one 300-line file.

### 1. No SGB machine at all

**Symptom.** A game that sends `MLT_REQ` and then reads `$FF00` to find out
which player it is talking to always sees player 1. On an SGB with two or four
controllers, every player after the first is invisible: a two-player game reads
player 1's buttons for both.

**Caught by.** `samesuite/sgb/command_mlt_req`,
`samesuite/sgb/command_mlt_req_1_incrementing` (the ROM's own `RESULT_CODE` at
`$CFFE`), `cpp/sgb-ext-test` (its author's reference screenshot), and Mooneye
`acceptance/boot_regs-sgb` / `-sgb2`. All five now pass and are gated by
`make test-external`.

**Hardware behaviour.** Pan Docs, ["SGB Command Packet
Transfer"](https://gbdev.io/pandocs/SGB_Command_Packet_Transfer.html) and
["Multiplayer"](https://gbdev.io/pandocs/SGB_Command_MLT_REQ.html). A packet is
16 bytes sent LSB-first by pulsing `P14`/`P15`; `MLT_REQ` sets how many
controllers the SNES side polls, and the joypad register reports `$F - index`
in its low nibble whenever both select lines are released.

**Carries into the tool.** Full write-up in
[`docs/components/sgb.md`](components/sgb.md). Three protocol rules that the
prose documentation leaves ambiguous and the test ROMs settle, each a check in
its own right:

* *A bare `$00` starts nothing.* The opening `$00` must be acknowledged by a
  `$30` before any pulse is read as a bit — `sgb-ext-test`'s
  `SendPacketShortStart` sends a packet without that acknowledgement and expects
  it to be ignored entirely.
* *The bit is the last line held low.* `$20 → $10 → $30` transmits a `1` and
  `$10 → $20 → $30` transmits a `0`; an implementation that latches on the
  *first* falling edge gets both backwards. `sgb-ext-test` sends exactly these
  two sequences for the same bit position to separate the rules.
* *The command runs on the last bit, not on the stop bit.* `SendPacketCorruptStop`
  terminates a packet with a `1` where the stop bit should be a `0` and still
  expects the command to have taken effect.

### 2. `MLT_REQ 2` is three players, not four — and SameBoy gets this wrong

**Symptom.** A game that sends the illegal two-player-code `2` sees the joypad
alternate between player 1 and player **3** rather than cycling 1-2-3-4 or
staying on 1. Nothing ships that does it deliberately, but it is a precise probe
of how the SNES side computes its mask, and it is where implementations diverge.

**Caught by.** `samesuite/sgb/command_mlt_req`. This is the row **SameBoy
fails** — `Core/sgb.c` has `if (player_count == 3) player_count++;`, rounding the
illegal request up to four players, and that one line is the whole difference.
Of the eighteen emulators in the published shootout only GSE passes all three
SGB rows; SameBoy passes one, bgb one, mGBA two, and six decline the SGB model
entirely.

**Hardware behaviour.** `player_count = (param & 3) + 1` with no correction, so
the index is masked with `player_count - 1 = 2` and can only be 0 or 2. Applying
the command then advances the index `player_count` further times — a whole
number of turns for 1, 2 and 4 players and therefore invisible, but not for 3,
which is precisely what sixteen of `command_mlt_req`'s twenty-four samples
measure. (Physically: the SNES polls every controller it now believes is
attached.)

**Carries into the tool.** *"An illegal `MLT_REQ` parameter must not be
normalised."* Also the index rule it depends on: the index advances on the
**rising edge of P15** and on nothing else, so `$10 → $30` moves it and
`$20 → $30` does not — `command_mlt_req_1_incrementing` is eight pulse patterns
that test only this, and it is the cheapest single check of the three.

### 3. The boot ROM's leftover tilemap — a whole-screen difference, and not an SGB bug

**Symptom.** Any ROM that draws on part of the screen and never clears the
tilemap shows blank cells in tile rows 8 and 9 where hardware shows *something*.
Under a screenshot-comparing suite that is not a small difference: with two
shades on screen instead of three, **every white pixel ranks differently**, so
the whole frame mismatches over two rows of glyphs.

**Caught by.** Nothing here, before this. Both SameSuite `sgb/` ROMs pass on
their own `RESULT_CODE` while failing the shootout's image compare for this
reason alone, and the two verdicts disagreeing is exactly the kind of thing a
project that scores itself by one method never notices. The `cpp` group's four
ROMs are now run against their reference PNGs by
[`examples/cpp_suite.rs`](../examples/cpp_suite.rs), which is what made it
visible.

**Hardware behaviour.** The DMG boot ROM's last act is to scroll the Nintendo
logo into place, and it leaves the tilemap entries behind: `$01`-`$0C` across
`$9904`-`$990F`, `$0D`-`$18` across `$9924`-`$992F`, and `$19` (the ® glyph) at
`$9910`. Nothing clears them, so a game that loads its own tiles over those ids
and never touches the map renders its own glyphs there. This emulator runs no
boot ROM (they are copyrighted and cannot be shipped), so every byte of that
state has to be written by hand in `MemoryManagementUnit::set_initial` — and
this part of it was missing. The tile *pixels* are still not written: on
hardware they are the logo decompressed out of the cartridge header, and every
ROM that reaches `$0100` has replaced them long before anything looks.

**Carries into the tool.** *"Post-boot VRAM is not zero."* A conformance tool
comparing frames should say so explicitly, because the failure it produces looks
like a rendering bug and is not one. Pinned here by
`the_boot_rom_leaves_its_logo_in_the_tilemap`.

### Measured

Before and after on the same ROMs, same commands, no estimates:

| suite | before (`main` @ `fba9d73`) | after |
|---|---:|---:|
| Mooneye `acceptance` + `emulator-only` | 86 / 103 | **88 / 103** |
| Mooneye `misc` (CGB) | 4 / 8 | 4 / 8 |
| SameSuite | 69 / 76 | **71 / 76** |
| Mealybug Tearoom | 3 / 79 | 3 / 79 |
| shootout `cpp` group | *not gated* | **4 / 4** |

All three published SGB rows are pixel-exact against the shootout's own
reference screenshots (0 of 23,040 pixels differ on each). No row moved in any
other direction; `testroms`, `savestate`, `battery` and `audiocapture` are
unchanged.

---

## 2026-08-14 — 266 external rows were measured by hand and gated by nothing

Not a bug fix, and it is here because the standing convention's second point —
*"if nothing in the suite catches it, say so explicitly; a coverage gap is itself
a finding"* — applies to the harness as much as to the emulator.

**Symptom.** None a player can see, and that is the point. Mooneye GB, SameSuite
and the Mealybug Tearoom tests were scored only when somebody ran a scout by
hand. In the twenty-four hours before this was written those three suites moved
by **+96 rows**; every one of those rows could have been lost again to an
unrelated commit and nobody would have known until the next manual sweep. The
same is true of any project that publishes a conformance score it does not
re-measure on every push: the number on the README is a claim about a commit
nobody can name.

**Caught by.** Nothing, until now. `make test-external` fetches the three suites
at pinned versions and scores all 266 rows in about two seconds, against
checked-in expectations that are the ROMs' own verdicts
(`testharness/{mooneye,mooneye_cgb,samesuite,mealybug}_baseline.txt`), red on any
change in either direction. Blargg's `cgb_sound` — twelve rows we had measured
12/12 externally and never wired — is now in `testharness/testroms.rs`, taking
the in-repo harness from 35 to 47 rows.

**Hardware behaviour.** Not applicable; this is a harness property. The
behavioural claim it protects is every claim the other entries on this page
make.

**Carries into the tool.** Three design rules, all of which the standalone tool
needs and two of which are already how `tools/conformance/` works:

1. **Fetch the corpus, never vendor it, and pin it.** These ROMs are their
   authors' work under their authors' licences. One file holds every pin
   (`tools/fetch-test-suites.sh`), so a laptop, a CI job and a published score
   are measuring identical bytes — a conformance number whose ROM set is
   unstated is not a number.
2. **A pass that appears is a change too.** Both directions are red. An
   improvement nobody wrote down is indistinguishable from a measurement error,
   and re-blessing is where a suite quietly stops being an expectation.
3. **For a suite you fail almost completely, pin the distance, not the
   verdict.** Mealybug is 3/79; a PASS/FAIL baseline over it is a flat wall that
   cannot show a cycle-stepped-PPU rewrite getting closer. The baseline records
   *how many of the 23040 pixels differ*, so progress and regression are both
   visible from the first commit of the rewrite.

**A false positive found by running the suite this way.** Three Mooneye tests
that **pass** — `acceptance/add_sp_e_timing`, `acceptance/ld_hl_sp_e_timing` and
`acceptance/oam_dma_start` — print `gameboy: FAULT — the CPU is executing from
0xfe00 … the game has crashed` to stderr. They execute code from OAM on purpose,
and `Cpu::operation`'s runaway-PC heuristic treats any `PC` in `$FE00-$FF7F` as a
crashed program. The consequence for a player is not cosmetic: `report_fault`
poisons the battery, so a game that legitimately runs a routine out of OAM
**silently stops saving for the rest of the session**. Executing from OAM is
unusual but legal — the CPU can read OAM outside PPU modes 2 and 3 (Pan Docs,
"VRAM/OAM accessibility"), which is precisely why Mooneye uses it to time an
access onto a known byte. **Fixed in the same change**: the heuristic now covers
`$FEA0-$FF7F` — the unusable block and the I/O registers, neither of which can
ever hold code — and OAM is out of it (`src/cpu/core.rs`, `Cpu::operation`).
The three ROMs still pass, no other row moved, and
`running_from_oam_is_not_a_fault` pins it beside the existing
`running_from_hram_is_not_a_fault`. **Conformance candidate: a crash heuristic
must not fire on any ROM in a passing suite** — which is only checkable once the
suite is wired, and is the second thing wiring it bought.

---

## 2026-08-14 — the DMG OAM-corruption bug: a defect that had to be *added*

Blargg `oam_bug` went **3 of 8 sub-tests → 8 of 8** on a DMG (`blargg/oam_bug@dmg`
FAIL → PASS in `testharness/baseline.txt`; the whole in-repo suite 33/35 → 34/35).
This is the unusual case where conformance means reproducing a hardware *fault*,
so the four points of the standing convention read a little differently.

**Symptom.** On a DMG, a game that increments or decrements a 16-bit register while
that register holds an address in `$FE00-$FEFF` — or reads/writes there at all — while
the PPU is scanning OAM sees sprites visibly garble: whole 8-byte OAM rows are copied
from their neighbour and one word is mangled. An emulator without the bug shows a clean
screen where hardware shows corruption; an emulator with the bug but the wrong *timing*
corrupts the wrong row. Neither is visible in a framebuffer diff of any normal game,
because essentially no commercial game triggers it deliberately — which is exactly why
it needs a test ROM to see at all.

**Caught by.** Blargg `oam_bug`, and nothing else in any suite we run. Its eight
sub-tests separate cleanly and are worth carrying into the tool individually, because
three of them pass on an emulator that models *nothing*:

| sub-test | what it pins | passes with no model? |
|---|---|---|
| 1 `lcd_sync` | enabling the LCD starts at the top of a visible line | yes |
| 2 `causes` | the exact set of triggering instructions | no |
| 3 `non_causes` | `add hl,rr` / `ld hl,sp+n` / `add sp,n` / 8-bit `inc` must **not** trigger | yes (vacuously) |
| 4 `scanline_timing` | the window is 19 M-cycles and where it starts, to one M-cycle | no |
| 5 `timing_bug` | it recurs on every visible line and not in VBlank | no |
| 6 `timing_no_bug` | nothing is corrupted just outside the window, over 10 frames | yes (vacuously) |
| 7 `timing_effect` | the corruption pattern for each of 116 offsets across a line | no |
| 8 `instr_effect` | the pattern differs per instruction: `inc/dec rr`, `pop`, `push`, `ld a,[hl±]` | no |

*That "passes vacuously" column is the finding.* A pass/fail runner reports "3 of 8" for
an emulator that has never heard of the bug, which reads like partial credit for partial
work. The tool should say: sub-tests 3 and 6 are **negative** checks and are worthless
on their own — score them only alongside 2, 4, 5, 7 and 8.

**Hardware behaviour.** Pan Docs, ["OAM Corruption
Bug"](https://gbdev.io/pandocs/OAM_Corruption_Bug.html). Two facts do the work:

1. *The cause is the address bus, not a memory access.* The CPU's 16-bit
   increment/decrement unit (IDU) is wired straight to the address bus and asserts its
   operand as an address even when no read or write is behind it. So `inc de` with
   `DE = $FE00` corrupts OAM while `add hl,bc` with the same value does not — the latter
   goes through the ALU and never reaches the bus. Blargg's `3-non_causes` is precisely
   this distinction, and it is the part an implementation is most likely to get wrong by
   hooking `rb`/`wb` and stopping there.
2. *OAM is 20 rows of 8 bytes and mode 2 walks one row per M-cycle*, so which row is
   corrupted is a pure function of the dot the offending cycle lands on. Row 0 (objects 0
   and 1) has no preceding row to copy from and is never corrupted — which is why the
   window is **19** M-cycles wide, not 20, and why `4-scanline_timing` brackets it at both
   ends to a single M-cycle.

The three patterns (write `((a^c)&(b^c))^c`, read `b|(a&c)`, and the read-and-write-in-one-
M-cycle case that also reaches a row backwards) are transcribed in
[`src/gpu/oambug.rs`](../src/gpu/oambug.rs).

**A CGB is supposed to FAIL this ROM.** Pan Docs is explicit that Game Boy Color and
Advance are not affected, *even running monochrome software*. `blargg/oam_bug@cgb` is
therefore pinned FAIL in the baseline with a note in its header, and a conformance tool
must not treat it as a red row: scoring it against a CGB rewards emulating a machine that
does not exist. **Generalisation worth carrying into the tool: a check needs a
machine-model predicate, not just a ROM.** This is the first check in the corpus whose
*correct* result depends on which console it is run on, and the same will be true of every
DMG-defect and CGB-only test that follows.

**Carries into the tool.**
- `oam_bug` sub-tests 2, 4, 5, 7, 8 as five separately-reported checks, with 3 and 6
  marked as negative controls that must be scored jointly.
- The model predicate above (`dmg` only; `cgb` expected-fail).
- The unit assertions in `gpu::oambug::tests` — the three bitwise patterns, the row-0
  exemption and the first-four-and-last-row exemption of the read-write pattern — which
  are checkable without a ROM at all.
- **A phase-calibration check.** Our PPU is stepped once per instruction, so the dot a
  mid-instruction bus cycle lands on is predicted arithmetically and one constant
  (`mmu::OAM_BUG_PHASE`) absorbs the offset the batched PPU accumulates when `LCDC` bit 7
  is set part-way through an instruction. `4-scanline_timing` pins that constant to a
  single M-cycle: measured here, ±1 M-cycle (±4 dots) flips it, in opposite directions at
  the two ends of the window. An emulator that passes 2 and 5 but fails 4 has the bug and
  the wrong phase, and the tool should say so rather than reporting a bare failure.

---

## 2026-08-14 — the machine booted from nothing: no post-boot state, no serial clock

This emulator does not run a boot ROM (they are copyrighted and cannot be shipped), so
every byte of the state a boot ROM leaves at `PC=$0100` has to be written by hand — and
most of it was not being written at all. Four Mooneye ROMs assert the *whole* of that
state rather than the handful of values usually quoted. All four now pass, plus a fifth
that was not on the work list:

| ROM | before | after |
|---|---|---|
| `acceptance/boot_div-dmgABCmgb` | FAIL | **PASS** |
| `acceptance/boot_hwio-dmgABCmgb` | FAIL | **PASS** |
| `acceptance/serial/boot_sclk_align-dmgABCmgb` | FAIL | **PASS** |
| `misc/boot_div-cgbABCDE` | FAIL | **PASS** |
| `misc/boot_hwio-C` | FAIL | **PASS** |

Measured on the same build, before and after: Mooneye `acceptance` **55/75 → 58/75**,
`misc` (`GB_MODEL=cgb`) **2/8 → 4/8**, `emulator-only` 28/28 unchanged. `testroms`,
`savestate`, `battery` and `trainer` all still match their baselines exactly, and the
Gen 1 link trade still completes (two trades, `exp 135` — the game's own boosted-experience
proof that the byte arrived *through* the trade path).

There are three separate defects here.

### 1. Half the I/O map was never initialised, and the APU's share was dead code

**Symptom** — a game that reads a register before writing it sees a value no real console
ever presents. Concretely: `P1` read `$FF` on a DMG instead of `$CF`; `IF` read `$E0`
instead of `$E1`, so a game that enables the VBlank interrupt and `EI`s at start-up waits a
whole frame for an interrupt hardware already had pending; the whole APU read back powered
**off** (`NR52 = $70`, not `$F1`) with `NR50`/`NR51` at zero; and on a Color console in
compatibility mode `VBK`, `BCPS` and `OCPS` read `$FF` rather than `$FE`/`$C8`/`$D0`.

The APU half was not a wrong value but *dead code*: `MemoryManagementUnit::set_initial`
wrote `NR10`–`NR52` at construction time, when `self.sound` is still `None`, so every one
of those twenty-two writes was silently dropped and the sound chip that gets installed
afterwards started from its own zeroed defaults. It had been dead since the APU became
optional.

**Caught by** — `acceptance/boot_hwio-dmgABCmgb` and `misc/boot_hwio-C`, which walk
`$FF00-$FF7F` plus `IE` byte by byte against a table and name the first address that
disagrees. Nothing in the in-repo harness caught it, and nothing would have: Blargg's ROMs
and dmg-acid2 all write every register they depend on.

**Hardware behaviour** — Pan Docs, "Power-Up Sequence" → *Hardware registers*, and the two
Mooneye ROMs' own tables (`mooneye-test-suite/acceptance/boot_hwio-dmgABCmgb.s`,
`misc/boot_hwio-C.s`), which are the authority here because they were verified against real
DMG/MGB and CGB units. The two consoles genuinely disagree — a DMG boot ROM leaves both
joypad select lines pulled low, a CGB leaves them released — so this is per-model state, not
one table.

**The trap worth writing down:** `NR11` and `NR14` read back `$BF`, but writing `$BF` to
them is *wrong*. Those read-backs are `$80` and `$87` ORed with their write-only bits; write
the read-back value instead and you load a length counter of 1 **with the length counter
enabled**, so channel 1 switches itself off within 4 ms and `NR52` reads `$F0`. `boot_hwio`
reads `NR52` about 2.8 ms in and requires `$F1`, so it catches exactly this. The fix
replays the writes the boot ROM actually makes (`Sound::set_post_boot_state`), which is the
only formulation that gets both the read-back and the channel-enable flip-flop right.

**Deliberately not reproduced:** the sound. The boot ROM's second beep is still ringing at
hand-off on real hardware and its envelope volume is not readable from any register, so how
far it has decayed cannot be derived — and we never played that beep, so emitting its tail
would be inventing audio the machine never produced. The envelope is left fully decayed.

### 2. The 16-bit system counter started at zero

**Symptom** — `DIV` reads `$00` at `PC=$0100` instead of `$AB` (DMG) / `$26` (CGB). Games
seed their RNG from `DIV`, so on a real console the seed depends on how long the boot ROM
took and on when the player pressed a button; from a zeroed counter it is the same every
time and it is a value hardware never shows at that instant.

**Caught by** — `acceptance/boot_div-dmgABCmgb` and `misc/boot_div-cgbABCDE`, which are much
stronger than "`DIV` reads `$AB`": each reads `DIV` six times at known instruction offsets,
chosen so the reads fall either side of an increment, and so pins the counter's **low bits**
as well. That is what makes the value in `src/mmu/mod.rs` a measurement rather than a guess.

**Hardware behaviour** — there is one 16-bit counter on the die and `DIV` is its top byte
(Pan Docs, "Timer and Divider Registers"; Gekkio, *Game Boy: Complete Technical Reference*
§Timer). It is free-running from power-on, so by the time the boot ROM hands over it holds
`$ABCC` on a DMG. `Timer::set_boot_counter` stores that value four T-cycles behind, because
this core's per-access tick (`MemoryManagementUnit::bus_tick`) advances the counter *before*
the access that observes it; the constant in the source is the hardware number and the
adjustment is made in one documented place.

**Carries into the tool** — `mmu::boot_state_tests::the_system_counter_boots_with_the_right_phase`
performs the same 13-M-cycle-then-read that the ROM does. Proved to have teeth by seeding
the counter one M-cycle out, which turns it red.

### 3. A link transfer completed the instant `SC` was written

**Symptom** — a game that starts an internally clocked transfer and counts cycles until the
serial interrupt measures zero. And with nothing plugged in, the transfer never completed
*at all*: no interrupt, `SB` keeping the byte that was written to it. On hardware an
internally clocked transfer always completes — the console is driving the clock; an empty
link port simply shifts in all ones.

**Caught by** — `acceptance/serial/boot_sclk_align-dmgABCmgb`, which starts a transfer from
a cold boot, `EI`s, and counts `inc` instructions until the serial interrupt fires. It
therefore tests three things at once: that the transfer is *not* instantaneous, that it
completes with no partner attached, and that the shift clock's edges are aligned to the
system counter's **reset time** rather than to the `SC` write. Nothing in the in-repo
harness covers any of the three — Blargg's ROMs use the link port purely as a printer and
never wait on the interrupt, which is a real gap in a ROM-driven suite.

**Hardware behaviour** — the shift register is clocked from the same free-running counter as
`TIMA`, at 8192 Hz on a DMG (512 T per bit, i.e. a falling edge of counter bit 8), so a byte
takes ~1 ms and its edges are wherever the counter says they are (Pan Docs, "Serial Data
Transfer (Link Cable)"; Gekkio, *GBCTR* §Serial). Modelled in
`Timer::take_serial_edges` + `Serial::shift_clock`.

**The one-M-cycle subtlety, which only this ROM can see:** the edge has to be taken from the
counter *as a bus access observes it* (`Timer::observed_counter`), not from the raw field.
Take it from the raw field and the serial interrupt arrives one instruction late — the ROM
reports `C: 90!` against an expected `$90`… with `$91`. Verified by deliberately making that
change. No unit test can catch it, because every other measurement of the serial clock is
relative to an `SC` write and a constant phase offset cancels out; only alignment against a
boot-time counter exposes it.

**Carries into the tool** —
`mmu::boot_state_tests::a_link_transfer_takes_eight_shift_clocks_and_completes_unanswered`
(seven shift clocks is not enough, the eighth completes it with `$FF`), and the two
`plugin` tests, which now assert *both* that a partner's reply arrives and that it does not
arrive early.

### What it cost, and the one baseline that moved

Emulation cost, `render_bench --frames 600` on `cpu_instrs`, three runs each: **0.339 →
0.359 ms/frame** (+6 % of a pure-emulation figure, 0.1 % of the 16.743 ms budget, still
~46× real time). Attributed by disabling each half in turn: about half is the serial edge
test in `Timer::do_mcycle`, about half is channel 1 genuinely running from boot.

`testharness/audio_baseline.txt`'s `tone440` hash moved, and **only** that one. The cause is
the boot state itself: with channel 1's enable flip-flop set at hand-off, the tone ROM's own
`NR14` write is a *re-*trigger and takes the shorter `SQUARE_RESTART_SPEEDUP` path. Confirmed
by comparing the two WAVs rather than by assertion — RMS `0.2467` and peak `0.267` identical
to four decimal places, every zero crossing displaced by the same `−0.047` samples (one
M-cycle). The reported dominant moved 440.0 → 438.0 Hz only because the estimator counts
crossings in a fixed 0.5 s window and the shift drops one. `dmg_sound` is untouched: both
its hashes are unchanged and its verdict is still 12/12. `testharness/baseline.txt` was not
touched and matches exactly.

---

## 2026-08-14 — MBC1M multicarts were driven as ordinary MBC1 carts

Mooneye `emulator-only/mbc1/multicart_rom_8Mb` was the suite's single MBC failure
(27/28). Fixed in `src/mbc/mbc1.rs`; **Mooneye `emulator-only` 27/28 → 28/28**, and the
published-264 shootout row flips too (screenshot-judged, `GB_MODEL=dmg`: FAIL → PASS).
Mooneye `acceptance` was re-measured before and after on the same build and is unchanged
at 55/75, and `testroms` still matches its baseline exactly.

**Symptom** — a multi-game compilation cartridge (*Bomberman Collection*, *Mortal Kombat
I & II*, *Genjin Collection*, *Super Chinese 1・2・3*, …) boots its menu, and picking any
game but the first runs the wrong one, or garbage: the menu selects a game by writing
BANK2, and every bank the chosen game then asks for lands 16 banks away from where it
should. Nothing crashes, which is what makes it hard to recognise — the emulator looks
like it is running a corrupt ROM.

**Caught by** — Mooneye GB `emulator-only/mbc1/multicart_rom_8Mb`, whose expected-bank
table was verified by its author against a real MBC1B1 chip. Nothing else in any suite we
run touches MBC1M. Note the ROM's own readme: *"MBC1 multicarts cannot be detected from
the header alone … any emulator that wants to pass this test must have either heuristics
or some configuration to trigger MBC1 multicart mode."* — so the check is as much about
**cartridge identification** as about banking, and a conformance tool must say which half
an emulator failed.

**Hardware** — MBC1M wires the mapper differently: the top bit of the 5-bit BANK1 register
is not connected (a 4-bit register for banking), and the 2-bit BANK2 register drives bits
**4-5** of the bank number instead of bits 5-6. The `00 → 01` translation still uses all
five written bits *before* bit 4 is dropped, so writing `$10` selects bank `$00` while
writing `$00` selects bank `$01`. In mode 1 the `$0000-$3FFF` window therefore shows banks
`$00/$10/$20/$30` rather than `$00/$20/$40/$60` — which is exactly how a compilation cart
swaps games while each game believes it is running from a 256 KiB cartridge. Pan Docs,
["MBC1M": 1 MiB Multi-Game Compilation Carts](https://gbdev.io/pandocs/MBC1.html).
Identification, from the same section: *"These carts can normally be identified by having
a Nintendo copyright header in bank $10."* We require the image to be exactly 1 MiB **and**
to carry the logo at `$40104`; `GB_MBC1_MULTICART=1|0` forces the answer either way, which
is the "some configuration" half the test ROM asks for.

**Carries into the tool** — the 256-entry expected-bank table is now a unit test
(`mbc1::tests::multicart_banking_matches_the_mooneye_table`), transcribed from Mooneye's
`.s` file rather than from our output, so the expectation stays somebody else's. Two
conformance checks worth publishing: (1) *an MBC1M cart must be recognised as one* — a
detection check, separable from banking, that a tool can report on its own; (2) *the
false-positive direction matters more than the false-negative one* — misdetecting an
ordinary 1 MiB MBC1 game as a multicart makes half its ROM unreachable, so the check
should be run in both directions (a plain 1 MiB cart must keep bits 5-6). Both directions
are pinned here (`a_plain_1mb_cart_keeps_the_ordinary_wiring`).

---

## 2026-08-14 — `STOP` never stopped, and the speed switch took no time

`STOP` (`$10`) was three lines: call `switch_speed()` and carry on. It was a
one-byte opcode that never stopped the CPU, never reset `DIV`, never blanked the
screen, and completed the CGB speed switch instantly. Four of the five things
`STOP` does on hardware were missing.

- **Symptom, in user terms:** a game that puts the console into standby keeps
  running instead — on a DMG the screen should go blank and on a Color it should
  go black, and neither happened. More consequentially for games that *do* use
  `STOP` (which, outside speed switching, is none — Pan Docs: *"No licensed rom
  makes use of STOP outside of CGB speed switching"*), the CGB speed switch took
  zero time and left `DIV` running. Real hardware spends **65536 cycles of the
  4.194304 MHz clock** in the switch — 143.7 scanlines, which is why Pan Docs
  warns that *"during speed switch, the display collapses for a short moment"* —
  and comes out of it with `DIV` reset. A game that seeds its RNG from `DIV`
  after a speed switch gets a different number on us than on hardware, and daid's
  own description says the rest: *"Not doing this could cause problems with RNG
  for games, failing this will most certainly desync a TAS."*
- **Caught by:** `daid/stop_instr.gb` (DMG **and** CGB — two different reference
  images), `daid/stop_instr_gbc_mode3.gb`, `daid/speed_switch_timing_div.gbc`.
  All four were red and are now green. Nothing in the in-repo harness
  (`testharness/`) covers `STOP` at all — Blargg, Mooneye and acid2 between them
  never execute one. **That is a coverage gap in its own right**, and the daid
  suite is the only public source of these checks; it is not run in CI either
  (see the audit's item ⑪).
- **Hardware behaviour:** Pan Docs, *"Using the STOP Instruction"*, and the
  decision chart on that page (Lior Halphon). `STOP` has four inputs — a
  selected button already held, a speed switch requested via `KEY1` bit 0, an
  interrupt pending (`IE & IF != 0`), and `IME` — and they select between STOP
  mode, HALT mode, a speed switch and a plain no-op, with the opcode's own
  *length* (1 or 2 bytes) depending on whether an interrupt is pending. The
  chart also gives the switch duration: *"HALT mode will exit automatically
  after about 0x20000 T-cycles"*, counted in the new clock. For the display,
  same page: *"On CGB, leaving the LCD enabled when invoking STOP will result in
  a black screen. Except if the LCD is in Mode 3, where it will keep drawing the
  current screen."*
- **The one rule worth keeping in mind:** the Mode-3 exception is not a special
  case. Entering STOP mode takes the PPU's VRAM access away *unless the PPU
  already holds it* — and Mode 3 is exactly the mode in which it does. One rule
  covers the black screen and the frozen picture both.
- **Carries into the tool:** four ROM-level checks (the three `stop_instr`
  screens and `speed_switch_timing_div`), plus six unit checks in
  `src/cpu/core.rs` `mod stop_tests` that need no ROM at all: the switch lasts
  two wraps of the system counter, `DIV` comes out at ~0, STOP mode ignores
  interrupts and ends only on a `P1` line going low, a held button means `STOP`
  does not stop, and a pending interrupt makes it a one-byte opcode. The
  property a foreign emulator can be tested for without any reference image is
  **"a speed switch advances the PPU by ~143 scanlines"**.

### The two rows that did *not* turn green, and why they are not this bug

`daid/speed_switch_timing_ly` and `_stat` are still red, and the reason is
measured rather than guessed. Sweeping the switch duration over the three ROMs
gives three optima on this emulator:

| ROM | passes for a duration of |
|---|---|
| `speed_switch_timing_div` | `0x1FFF4 .. 0x20000` |
| `speed_switch_timing_stat` | `≈ 0x20030` |
| `speed_switch_timing_ly` | `0x20034 .. 0x20038` |

No single duration satisfies all three: `div` and `ly`/`stat` disagree by about
**21 dots**. They cannot disagree about the switch itself — the CPU, the system
counter and the PPU all advance in exact lockstep through it — so the difference
is a PPU-versus-counter phase error that already exists *before* the `STOP`.
`div` is immune to it (the counter is reset by the `STOP`, so it measures the
duration alone) and `ly`/`stat` are not (they measure the duration *plus* where
the PPU already was). The likeliest source is the phase the PPU starts at when
`LCDC` bit 7 is set, which is what Mooneye's `ppu/lcdon_timing-GS` and
`lcdon_write_timing-GS` measure — also red, also unfixed.

So the honest reading is that these two rows belong to the LCD-on/STAT work, not
to `STOP`. Tuning `SPEED_SWITCH_HALT_TICKS` to `0x20034` would turn them green
and turn `div` red, and would be a lie about a documented hardware constant.

---

## 2026-08-14 — the STAT interrupt was four triggers, not one line

`STAT` has four selectable interrupt sources (HBlank, VBlank, OAM, `LY == LYC`).
On hardware they are wired into **one** signal, and `IF.1` is set on that
signal's **0→1 transition**. We raised `IF.1` independently from each source,
which is a different machine: it produces extra interrupts whenever two sources
overlap, and misses none — so it looks harmless until a game counts them.

Three related defects, all in `src/gpu/mod.rs`, all fixed together because they
are three terms of the same equation.

### 1. No edge detection ("STAT blocking")

- **Symptom, in user terms:** a game that runs its raster-effect handler off
  more than one STAT source advances its effect *twice as fast as intended*.
  The classic shape is a status-bar split driven by both the LYC source and the
  HBlank source: on the split line the two overlap, we fired twice, and the
  handler ran an extra time every frame.
- **Caught by:** Mooneye `acceptance/ppu/stat_irq_blocking`, which arranges
  `LY == LYC` to stay set across mode 3 so the line is *never* released, and
  fails on any further interrupt. Nothing in Blargg, dmg-acid2 or SameSuite
  catches it — a framebuffer comparison cannot see an interrupt that fired.
- **Hardware behaviour:** the four sources are ORed into one internal signal;
  the interrupt is requested on its rising edge only. Pan Docs,
  [STAT](https://gbdev.io/pandocs/STAT.html) ("Spurious STAT interrupts" and the
  STAT-interrupt-line description); AntonioND's TCAGBD §8.7 gives the OR
  explicitly.
- **Carries into the tool:** *count* interrupts over a window rather than
  observing that one arrived. `gpu::tests::a_source_going_high_behind_another_raises_nothing`
  is the shape: step the PPU one M-cycle at a time, clear `IF` between steps,
  and assert the number of raises.

### 2. `STAT` bit 2 was a live comparison, not a latch

- **Symptom, in user terms:** a game that blanks the screen (`LCDC` bit 7 = 0)
  to rebuild VRAM and reads `STAT` back sees the `LY == LYC` bit change under
  it, because with the LCD off we reset `LY` to 0 and recomputed the bit. On
  hardware the comparison is a flip-flop clocked by the PPU: stopping the PPU
  freezes it, and an `LYC` written while the PPU is off is invisible until it
  restarts.
- **Caught by:** Mooneye `acceptance/ppu/stat_lyc_onoff`, whose four rounds
  read `STAT` back after each step and check both the retained bit and the
  presence/absence of the interrupt on re-enable. Nothing else in the suite.
- **Hardware behaviour:** the coincidence flag is latched by the PPU's own
  comparison clock (Pan Docs, [STAT](https://gbdev.io/pandocs/STAT.html), "LYC
  == LY"; the test's own comments state the rule outright: "Changing LYC should
  not have an effect, because the comparison clock is not running").
- **Also fixed here:** switching the LCD off must **not** force the STAT line
  low. The flag keeps feeding the OR gate, so a program that switches the LCD
  off with the flag already set and then switches it back on sees *no* edge and
  *no* interrupt. Forcing the line low manufactures one — `stat_lyc_onoff`
  round 2 fails with exactly that.

### 3. The line-144 OAM strobe was modelled as "all of VBlank"

- **Symptom, in user terms:** a game whose only STAT source is the OAM one
  receives its first interrupt of the frame a whole scanline late — at `LY = 1`
  instead of `LY = 0` — because the line was held high across VBlank by the same
  source and the real mode 2 at `LY = 0` produced no edge. Anything that counts
  scanlines from that interrupt is off by one for the whole frame.
- **Caught by:** Mooneye `acceptance/ppu/intr_1_2_timing-GS`, which times the
  gap between the mode-1 interrupt and the next mode-2 interrupt and reports it
  as a loop count (`$14`/`$15` expected; a one-line delay reads `$31`).
  `vblank_stat_intr-GS` pins the other half — that the line-144 strobe happens
  at all, simultaneously with the VBlank interrupt.
- **Hardware behaviour:** the OAM strobe the mode-2 select bit watches also
  occurs on line 144, where `STAT` already reads mode 1 — and only there; lines
  145-153 have no OAM scan. Pan Docs, [STAT](https://gbdev.io/pandocs/STAT.html)
  ("the STAT OAM interrupt is also triggered by entering VBlank").
- **Carries into the tool:** the two failure modes are *opposites* and a check
  must name which one it saw — "no interrupt at `LY = 144`" and "the `LY = 0`
  interrupt arrived one line late" are both symptoms of getting this term wrong.

### 4. The line the PPU restarts on has no OAM scan

- **Symptom, in user terms:** a game that enables the LCD and immediately polls
  `STAT` for a mode reads 2 where hardware reads 0, so a "wait until we are out
  of OAM scan" loop exits at the wrong moment on the very first line.
- **Caught by:** `stat_lyc_onoff` round 1 step 3 (it reads `STAT` one
  instruction after the enable and demands `$C0`) and, in much more detail,
  `lcdon_timing-GS`.
- **Hardware behaviour:** `lcdon_timing-GS`'s own header states it — "line 0
  starts with mode 0 and goes straight to mode 3".

### What this did NOT fix, and why it is not STAT work

`acceptance/ppu/lcdon_timing-GS` and `lcdon_write_timing-GS` are still red, and
after attempting them the honest verdict is that they are **not** separable from
the cycle-stepped-PPU rewrite (`docs/mealybug.md`), whatever their names
suggest. Both sample `LY`, `STAT`, and OAM/VRAM **accessibility** at 1 M-cycle
resolution across the first three scanlines after the LCD is switched on
(`lcdon_timing-GS`'s expectation table is 24 samples at nop counts 0, 17, 60,
110, 130, 174, 224, 244 in three passes one M-cycle apart). Passing them needs
two things this core does not have:

1. **VRAM/OAM access blocking** by mode. We do not enforce it, deliberately: our
   mode 3 is a fixed 172 dots where hardware varies it 172-289, so enforcing
   blocking on top of the approximation would silently *drop* writes a game
   placed legally in HBlank. Not enforcing loses two test rows; enforcing on a
   wrong mode 3 corrupts real games.
2. **A PPU stepped per bus access**, not once per instruction. Today
   `Gpu::do_cycle` runs after the whole instruction, so a mid-instruction read
   sees the PPU as of the previous instruction — a systematic 1-2 M-cycle skew
   that no amount of phase tuning fixes.

Recorded as a coverage/priority finding rather than a fix: the audit that
scheduled this work marked the separability of these two as an inference, and
the inference was wrong for exactly half the item.

### Measured

Mooneye `acceptance` **53 → 55** of 75 (`stat_irq_blocking`, `stat_lyc_onoff`
FAIL→PASS; no other row moved). GB Emulator Shootout published rows **192 → 194
of 264**, full suite **194 → 196 of 311**, **zero regressions** — every other row
identical, run before and after through the same manifest, against `main` at
`c10da12` (the VRAM DMA work above had already taken it from 188 to 192). In-repo
harnesses unchanged: `testroms` 33/35 matching baseline, `savestate`, `battery`,
`audiocapture` (dmg_sound 12/12), `trainer`, `audioperf` (0.265 ms/frame) all
green. Mealybug 3/79, SameSuite 54/76, Mooneye `emulator-only` 27/28 and `misc`
2/8 all unmoved.

---

## 2026-08-14 — CGB VRAM DMA: two aborts a guest can trigger, and four SameSuite rows

SameSuite `dma/` was **0/4**. Reading the code to size the work turned up two `panic!`s
reachable from ordinary cartridge code — and this project builds with
`[profile.release] panic = 'abort'`, so a panic is process death *and* whatever cartridge
RAM had not been written yet. That is the rule `src/mbc/battery.rs` already established
after the MBC divide-by-zero crashes: **cartridge-supplied input must never be able to
kill the process.** Both were reproduced with a hand-built CGB cartridge before anything
was changed (`mmu::vramdma_tests`, the two `..._does_not_kill_the_process` cases).

Fixed together, in `src/mmu/mod.rs` (`hdma_read`/`hdma_write`/`perform_vramdma_row`) and
`src/gpu/mod.rs` (`in_hblank`, and the write catch-alls). **SameSuite 50/76 → 54/76**;
the published-264 shootout score **188 → 192**, with the whole 311-row suite re-scored and
**zero regressions** anywhere.

### 1. `panic!` on an out-of-range HDMA source

**Symptom** — the emulator dies outright (`HDMA transfer with illegal start address 8000`)
the moment a game points `HDMA1/2` outside `$0000-$7FF0` / `$A000-$DFF0`, taking any
unsaved in-game progress with it. A player sees the process vanish; there is no error
screen, because there is no process left to draw one.
**Caught by** — **nothing in any public suite.** No test ROM tries to crash its host, so
this whole defect class is invisible to a ROM-driven runner. That gap is the same one the
battery work recorded, and it is a finding in its own right.
**Hardware** — the source is a 16-bit address with its low 4 bits dropped; there is no
"illegal address" trap in the DMA unit. It reads whatever the bus returns, exactly like
any other read (Pan Docs, [VRAM DMA transfers](https://gbdev.io/pandocs/CGB_Registers.html#lcd-vram-dma-transfers)).
**Carries into the tool** — *no cartridge-writable register value may terminate the
emulator.* A fuzz-style check that sweeps `$FF51-$FF55` (and every other I/O register)
across all 256 values and asserts the process survives.

### 2. The destination ran off the end of VRAM

**Symptom** — a transfer whose destination reaches `$9FFF` writes past it and the
emulator dies (`Gpu does not handle write A000`). `$8000 | (dst & 0x1FFF)` and a
guest-chosen length is all it takes.
**Caught by** — the crash, by nothing; the *masking* it shares a fix with is exactly what
SameSuite `dma/gdma_addr_mask` measures.
**Hardware** — the destination is a **13-bit** offset into VRAM: `HDMA3` keeps only bits
0-4 and `HDMA4` only bits 4-7, and the counter wraps inside VRAM. It cannot address
cartridge RAM at all (Pan Docs, "LCD VRAM DMA transfers").
**Carries into the tool** — *a VRAM DMA must never write outside VRAM*, checkable by
running `gdma_addr_mask` and by asserting no byte outside `$8000-$9FFF` changed.

### 3. An HBlank transfer armed during mode 0 waited for the *next* HBlank

**Symptom** — a game that sets up an HBlank DMA with the LCD off (which is where games do
their bulk VRAM loading) gets **nothing copied at all**: the tiles never arrive and the
screen is blank or stale when the LCD comes back on. With the LCD on it loses the first
block of every transfer it starts inside HBlank.
**Caught by** — SameSuite `dma/hdma_lcd_off` and `dma/hdma_mode0` (both were FAIL).
Nothing in Blargg, Mooneye or acid2 covers it.
**Hardware** — the transfer moves a block whenever the PPU **is** in mode 0, not only on
the transition into it; with the LCD off `STAT`'s mode bits read 0 permanently, so one
block moves and then it stalls forever, because there is never a mode-0 *entry* again.
`hdma_lcd_off`'s own expected output pins this: `$FF55` reads `$02` after arming with
`$83` — one block gone, three left.
**Carries into the tool** — *arm an HBlank DMA with the LCD off; exactly one block must
move and `$FF55` must read `$02`.*

### 4. `HDMA1-4` were a shadow copy, so a transfer could not be continued

**Symptom** — a game that kicks off a second transfer without re-writing the address
registers copies the **same block again**, over the top of the first: half the tile data
it asked for never arrives, and the half that does is duplicated.
**Caught by** — SameSuite `dma/gbc_dma_cont` (was FAIL).
**Hardware** — `HDMA1-4` are the transfer's own address counters. The DMA advances them,
so they hold the *next* address when it finishes; they are also write-only, and read back
`0xFF` (Pan Docs, "LCD VRAM DMA transfers").
**Carries into the tool** — *two back-to-back `$FF55` writes with no re-write of
`HDMA1-4` must copy two consecutive blocks to two consecutive destinations.*

### 5. Cancelling an HBlank transfer did not write the length register

**Symptom** — invisible to a player; visible to any program that reads `$FF55` back after
stopping a transfer, which is how a game decides whether to resume it.
**Caught by** — SameSuite `dma/hdma_lcd_off`, whose expected bytes are `$02, $80` — the
second is the read *after* a `$00` write, and `$80` means the length register took the
written value.
**Hardware** — bits 0-6 of `$FF55` are a real register and are written whatever bit 7
says; bit 7 only decides start-HBlank / start-general / stop.
**Carries into the tool** — *after cancelling with `$00`, `$FF55` reads `$80`.*

### 6. Undefined opcodes aborted the process (the same class, found by reading)

**Symptom** — a runaway `PC` — a corrupt save, a bad bank switch, a genuine game bug —
reaches one of `$D3 $DB $DD $E3 $E4 $EB $EC $ED $F4 $FC $FD` and the emulator dies. On
hardware the machine just hangs, and a player power-cycles it with their save intact.
**Caught by** — **nothing.** Every test ROM executes only valid opcodes.
**Hardware** — these bytes are not instructions; the CPU locks up and only a reset
recovers it (Pan Docs, ["Unused opcodes"](https://gbdev.io/pandocs/CPU_Instruction_Set.html)).
It is a *CPU* lock-up: the PPU, timer and APU keep running, so the screen holds its last
frame rather than the window disappearing. `$FC` was separately being treated as a `NOP`,
which is neither behaviour; it now locks up like its ten siblings.
**Carries into the tool** — *executing any undefined opcode must leave the emulator
running and the frame clock advancing.* Covered here by `cpu::core::lockup_tests`.

---

## 2026-08-14 — an unanswered link-close byte: "I cannot leave the trade room"

The Gen 1 link partner answered the trade menu's cancel nybble `$6F` in exactly
one of its six phases. Everywhere else the player was sealed in the Cable Club.

- **Symptom, in user terms:** after a link trade the player picks CANCEL on the
  trade-selection screen and presses A, the screen says `Waiting!`, and nothing
  happens. Press A again, and again — the game keeps rendering, music plays, the
  clock runs, buttons respond. It is not a hang; the exit simply never happens.
  And the Trade Center has **no warp events at all**
  ([`data/maps/objects/TradeCenter.asm`](https://github.com/pret/pokered/tree/master/data/maps/objects/TradeCenter.asm)
  — confirmed at run time: `wNumberOfWarps` reads 0), so that screen is the only
  way out of the room. Measured on the retail cartridge: **2,475 CANCEL presses,
  `wLinkState` stuck at `$32` (`LINK_STATE_TRADING`), never left.**
- **Caught by:** **no test ROM, and none can** — the same blind spot as the
  terminal-render bugs and the `wIsInBattle` sentinel above. The cartridge was
  entirely correct; the defect was in a *consumer* of emulated state, in this
  case one that answers the wire. The suite's own unit test for this byte passed
  throughout, because it only ever drove `$6F` from the one phase that worked.
- **Hardware / cartridge behaviour:** `.cancelMenuItem_APressed`
  ([`engine/link/cable_club.asm`](https://github.com/pret/pokered/tree/master/engine/link/cable_club.asm))
  writes `$F` to `wSerialExchangeNybbleSendData`, runs one
  `Serial_SyncAndExchangeNybble` round, and falls through to
  `ReturnToCableClubRoom` **only** if the round comes back `$F`
  (`cp $f / jr nz, .cancelMenuItem_Loop`). Three properties of that make it a
  whole-state-machine obligation rather than one branch:
  1. `wSerialExchangeNybbleSendData` is a **latch** — every later sync round
     keeps sending `$6F` until `TradeCenter_SelectMon` resets it to `1`;
  2. a round is 11–12 transfers of the same byte (`.loop1` plus ten `.loop2`
     iterations, [`home/serial.asm`](https://github.com/pret/pokered/tree/master/home/serial.asm))
     and the game keeps the **last** `$6x` it saw, so the whole run must be
     answered identically;
  3. a wrongly-answered round is simply **retried**, from whatever state the
     partner has since moved into — so answering it in one phase only makes the
     *second* attempt unanswerable. That is the trap: the first cancel appears to
     work, and every one after it is refused.
  Note also what `CloseLinkConnection`
  ([`engine/link/cable_club_npc.asm`](https://github.com/pret/pokered/tree/master/engine/link/cable_club_npc.asm))
  actually does, because it is easy to blame: it puts `$02` in `SB` and starts an
  **externally** clocked transfer (`SC = $80`). Our `Serial::wb` only offers a
  transfer to a plugin when `SC & $81 == $81`, so that byte never reaches the
  partner at all — exactly like a real second console that is not clocking. It is
  not part of this path.
- **How it was reached in a real session:** the partner's phase only has to be
  out of step with the game once. The terminal frontend's **F5 / F8 quick-save
  and quick-load** do it by design — plugin state is deliberately not in a save
  state, so a load calls `PluginHost::reset` and the partner drops to
  `Phase::Handshake` mid-session (`docs/plugins.md`). Backing out of the trade
  screen is precisely the escape hatch from that situation, which is why it has
  to work from a phase the partner did not expect. `examples/gen1_trade.rs
  --quick-load` reproduces it end to end on the retail cartridge.
- **The fix:** `LinkPartner::feed_cancel` answers `$6F` with `$6F` in **every**
  phase except `Exchanging` (inside a block that byte is party data — a stat, a
  name character, a patch offset — and must still come from our stream), clears
  the per-exchange scratch so nothing from the abandoned trade leaks into the
  next session, and lands in `Phase::Connected`, which is where the game is: the
  link is still up and the player can walk back onto the console. A cancel is
  also no longer counted as a trade — arriving in `Phase::Confirm` it used to
  fall into the "accept" arm, answer `$62`, and increment `trades_completed`.
- **New tests — the shape matters more than the cases.** The old test asserted
  the *byte* from one phase. The new ones assert the byte **and the resulting
  phase**, for every phase the machine can be in, plus a repeated cancel, plus a
  fresh session afterwards (`plugins/gen1-link/src/protocol.rs`:
  `the_cancel_byte_is_echoed_from_every_phase`,
  `a_whole_cancel_round_is_echoed_from_every_phase`,
  `a_repeated_cancel_keeps_being_answered`, `a_cancel_never_counts_as_a_trade`,
  `the_cancel_byte_is_data_inside_an_exchange`,
  `a_new_session_works_after_a_cancel_from_any_phase`,
  `the_player_can_leave_after_a_completed_trade`). Five of the seven go red when
  the fix is reverted — verified, not assumed.
- **Carries into the tool.** Two checks, both generalisable well beyond Pokémon:
  1. **A protocol's escape byte must be answered from every state.** Whenever one
     side of a conversation will retry a request until it is answered, answering
     it in only the state you expected turns a recoverable desync into a
     permanent one. Probe it by driving the peer into each of its states and
     sending the escape.
  2. **State-machine coverage is per-(input × state), not per-input.** A test
     that exercises a byte in one state and asserts only the reply is the exact
     shape of test that shipped this bug. A conformance report for a link
     adapter should say *which states* a message was accepted in.
---

## 2026-08-14 — the MBC3 clock ran on the host's wall clock, not the console's

**Symptom.** A cartridge with a real-time clock keeps time at whatever multiple
of real time the *emulator* happens to be running at, because it was never
running on the console's clock at all — the registers were derived from
`SystemTime::now()` at every read. Concretely, and all measured:

* Fast-forward, or any headless/benchmark run, makes the in-game clock crawl:
  the game sees the host's seconds while its own world runs 60× faster, so
  Pokémon Gold's day/night cycle takes a real hour per emulated hour however
  fast you play. Pause the emulator and the clock keeps running.
* The sub-second phase does not exist, so nothing that measures *when* the next
  tick lands can work.
* The counters were re-derived from a `SystemTime`, so an out-of-range value a
  game wrote (say 62 minutes) was normalised away instead of counting on.
* `compute_difftime` **panicked** if the host clock predated the UNIX epoch —
  an abort, under `panic = 'abort'`, on a machine with a wrong RTC battery.

**Caught by.** ax6's [`rtc3test`](https://github.com/aaaaaa123456789/rtc3test)
(all three sub-suites), which is *not* in our harness and is not in the
conformance action either — it is judged by a reference screenshot, and neither
the ROM nor the reference is ours to vendor. Measured through the
GBEmulatorShootout's copies: **0/3 → 3/3**. Nothing in the in-repo suite caught
any of it, and that is itself the finding: `testroms`, `battery` and `savestate`
all exercise the RTC's *persistence* and none of them its *rate*.

The three sub-suites map onto three separate defects:

| `rtc3test` | what it measures | what was wrong |
|---|---|---|
| Basic tests | the tick interval, with the Game Boy's own timer, to 1000 ms ± 1 ms | the clock was not driven by emulated time at all |
| Range tests | out-of-range register values counting on without carrying | registers were derived from a normalised time, so out-of-range values could not survive a tick |
| Sub-second writes | which register writes restart the sub-second divider, to 1.5 ms | there was no divider |

**Hardware behaviour.** The clock is on the *cartridge*: its own 32768 Hz
crystal and its own coin cell, independent of the console. The five registers
are **counters**, not a time — each is a binary counter of its own physical
width (6/6/5/8+1 bits) that carries into the next only when it rolls over from
its natural maximum (59, 59, 23), and simply wraps to zero **without carrying**
when a value loaded above that maximum overflows the bits it has. The day
counter is 9 bits and sets a sticky overflow flag (`DH` bit 7) past 511. `DH`
bit 6 halts the clock, freezing the sub-second divider with it, so the clock
resumes mid-second. Writing the *seconds* register restarts the divider;
writing any other register does not. Pan Docs, "MBC3"
(<https://gbdev.io/pandocs/MBC3.html>); `rtc3test`'s own test descriptions
(<https://github.com/aaaaaa123456789/rtc3test/blob/master/tests.md>).

**A documented "deviation" that turned out to be the correct behaviour.** This
repo recorded "MBC3 latches on any write to `$6000-$7FFF`, not on the
`$00`→`$01` sequence" as a known divergence and a candidate conformance check.
It is not a divergence. CasualPokePlayer's `latch-rtc-test` writes five random
register values and then **one** random byte to `$6000` per row, and its
reference dump has every row reading back what was just written — so any single
write latches, whatever its value. Implementing the strict `$00`→`$01` pair was
tried here and turned that ROM's output into a screen of zeroes (`FAIL`,
measured); reverting to any-write restored it. Pan Docs describes the idiom
games use, not the decode. **Delete the check, do not add it.**

**Carries into the tool.** Two things worth absorbing:

1. `rtc3test` itself, as a screenshot check, with the caveat that its ROM is
   menu-driven — the shootout's `rtc3test-1/2/3` are pre-selected variants.
2. A property no ROM states but every RTC emulator gets wrong the same way:
   **the cartridge clock must advance with emulated time, not host time.** An
   adapter can be asked to run N emulated seconds as fast as it likes and then
   report the clock; a wall-clock implementation reports nearly zero. That is a
   check the telemetry adapter can make without any ROM at all, and it is the
   single question that separates a real RTC from a `SystemTime` wrapper.

---

## 2026-08-14 — the audio player paced the emulator (host-integration, not hardware)

**Symptom.** With sound enabled the game runs at a fraction of speed and the
picture crawls; with sound disabled, full speed. Same binary, same window, same
ROM. Telemetry shows `emulate_ms` at or above the frame period (17.93 ms against
16.743) while every render stage is ~0 ms, and the display falls to ~6 fps —
`59.7275 / MAX_CATCHUP`, the signature of a frame loop that can never repay its
debt. Misdiagnosed four times as APU cost.

**Caught by.** *Nothing, and that is the finding.* No test ROM can see it: the
framebuffer is byte-perfect throughout, and the defect only exists when a **host
audio player is attached**, which no ROM-driven suite does. Our own benchmarks
missed it for the same reason — `render_bench` is headless and reported a
healthy 0.246 ms/frame while the captain's session ran at 6 fps. This is the
same class of gap as the three terminal-render bugs: a ROM-driven suite compares
framebuffers, and everything here leaves the framebuffer correct.

**Behaviour violated.** Not silicon — the *host integration contract*. A real
Game Boy's APU is driven by the machine clock and cannot stall it; an emulator's
audio sink is a consumer, never the emulator's clock source, unless it is *the*
clock. Here `AudioPlayer::play` is called from `Sound::play_samples` inside
`Cpu::do_cycle`, and it waited at the queue's high watermark to make the sound
card the master clock — while the frontends already paced emulation with a
59.7275 Hz deadline accumulator. Near (byuu), *Dynamic Rate Control*
(<https://bsnes.org/articles/dynamic-rate-control/>) states the trilemma
directly: sync to video only and the buffer drifts, sync to audio only and video
stutters, **block on both and you get both artifacts**. Two compounding details
worth carrying: the wait's equilibrium was *slower* than real time (1 ms poll
granularity, resuming only at/below the watermark → 57.85 fps unpaced), and
catch-up made it self-reinforcing (more frames back-to-back → queue fills faster
→ every frame blocks).

**Carries into the tool.** Two checks, neither expressible as a frame hash:

1. **Emulated speed must not depend on whether an audio device is attached.**
   Run the same ROM with and without host audio and compare emulated frames per
   wall-clock second; a difference is this bug. Cheap, and no reference data of
   ours is involved.
2. **`speed_pct` must stay at 100 whatever `fps` does.** Already the stated
   invariant of our telemetry stream (`docs/telemetry.md`) — it was violated
   here, which is a second argument for the telemetry stream being the reference
   adapter (`docs/conformance.md`): `speed_pct` and `emulate_ms` expose this,
   a framebuffer hash exposes nothing.

Our own gate is `testharness/audioperf.rs` (`make test-audio-perf`), which needs
no sound card: it attaches the real producer half with no consumer so the queue
saturates, then fails above 4 ms/frame or below 4x real time. It reports 29.6 ms
and 0.57x against the old policy.

**Adjacent finding, not fixed (recorded as a check).** A frame-paced loop
advances the machine until the GPU raises `updated`, and `Gpu::do_cycle` returns
immediately while the LCD is off — so during a screen-off transition one "frame"
ran **845,836** cycles instead of 70,224 (measured, Pokémon Blue boot) and the
APU emitted the matching burst. Any emulator whose frontend paces on frames has
this. Check: **the emulated cycle count between two consecutive frame deliveries
must be bounded**, or audio and wall-clock pacing diverge whenever a game blanks
the LCD.

---
## 2026-08-14 — no DMG boot mode: every "DMG" test was run on a Game Boy Color

- **Symptom** — a test ROM that prints its boot registers shows `HL: 007C` where a real
  DMG shows `HL: 014D`, and an emulator is marked FAIL for a screenshot difference even
  though every assertion inside the ROM passed. Nothing a *player* sees; everything a
  *conformance run* sees.
- **Caught by** — Mooneye `acceptance/bits/reg_f`, `ei_timing`, `halt_ime1_timing`,
  `reti_intr_timing`, `timer/tim00_div_trigger`, `timer/tim01`, `timer/tim11_div_trigger`
  and two more: **9 tests, FAIL→PASS with a DMG boot, zero regressions.** Blargg and acid2
  catch none of it — a coverage gap in the suite this project had wired. Our own harness
  now asserts the boot registers directly (`self_check_models_differ`) rather than hoping a
  ROM notices.
- **Hardware behaviour** — a DMG starts with `A=0x01 F=0xB0 HL=0x014D`; CGB hardware starts
  `A=0x11 HL=0x007C`, and keeps that state even when running a DMG-only cartridge in
  compatibility mode, where `FF72`/`FF73`/`FF75`-`FF77` also still respond (Pan Docs,
  ["Power Up Sequence"](https://gbdev.io/pandocs/Power_Up_Sequence.html)). Compatibility
  mode is a *third* machine, not a DMG.
- **Carries into the tool** — a **model** axis is not optional: a conformance runner must
  ask the emulator which console to boot and must report which one produced a result. The
  shootout's runner already does (`startProcess(rom, model=…)`); an emulator that cannot
  answer loses the DMG rows outright. Two checks worth shipping: *boot registers match the
  model asked for*, and *the same ROM run under two models produces two distinguishable
  machines* (else the model flag is cosmetic).

## 2026-08-14 — a test harness that judged itself (harness defect, not an emulation bug)

- **Symptom** — CI green through a regression. Four ROMs' expected result was an FNV hash
  of *our own* framebuffer, and the documented response to a changed hash was to re-bless
  it; `oam_bug` was recorded as an opaque number that in fact meant "3 of 8 sub-tests
  pass"; and dmg-acid2's checked-in reference image was **never compared to** — the harness
  printed a suggestion that a human eyeball it.
- **Caught by** — nothing, by construction. Found by re-running the suite against the
  [GB Emulator Shootout](https://gbdev.io/GBEmulatorShootout/)'s ROMs and references. The
  harness now proves it can fail on every run: the reference image must compare equal to
  itself and must report a difference when one pixel is flipped. Both mechanisms were also
  validated by deliberately breaking the emulator — clearing the HALT-bug latch turns
  `halt_bug` red, ignoring sprite X-flip turns dmg-acid2 red at 119/23,040 pixels.
- **Hardware behaviour** — not a hardware question; a *methodology* one. Blargg's ROMs
  publish their own verdict in two places, so no self-reference is ever necessary: the
  memory-mapped result block (status at `$A000`, signature `$DE $B0 $61` at
  `$A001..$A003`, text from `$A004`) and the screen — Blargg's print routine loads its font
  so that a tile id **is** the glyph's ASCII code and writes characters straight into the BG
  tilemap, so the tilemap reads back as text. `halt_bug` and `interrupt_time` are cartridge
  type `0x02` with RAM size `0x00` (**no cartridge RAM at all**), so for those the screen is
  the only channel.
- **Carries into the tool** — three rules. (1) **Never accept an emulator's own prior output
  as the expected result**; every expectation must come from the ROM or its author. (2) A
  conformance tool should **read the verdict text off the frame** for a screen-only ROM
  rather than compare a reference image it produced itself — the tool then works for ROMs
  nobody has a reference for. (3) **Report sub-test detail**, not a single bit: "3 of 8
  oam_bug sub-tests pass, 2/4/5/7/8 fail" is actionable where "FAIL" is not.

## 2026-08-14 — a test machine with no APU changes what test ROMs report

- **Symptom** — `interrupt_time` draws only its title and hangs forever; it was recorded
  for months as "cannot run headless". `dmg_sound/01-registers` reports
  `NR10-NR51 and wave RAM write/read — Failed #2`. Both are artefacts of the *harness*, not
  the emulator: it never called `enable_audio`, so `MMU.sound` stayed `None`.
- **Caught by** — the divergence between two of our own harnesses (`audiocapture` installed
  an APU, `testroms` did not) reporting different verdicts for the same emulator. With an
  APU installed, `interrupt_time` runs to completion and prints `Failed` — a real,
  reportable result — and `dmg_sound/01` passes.
- **Hardware behaviour** — every Game Boy has an APU whether or not anything is listening,
  and its registers are readable: `interrupt_time`'s `sync_apu`/`get_cpu_speed` spin on
  `NR52` (`$FF26`) waiting for a channel-status bit that never changes if the APU is absent
  (Pan Docs, ["Audio Registers"](https://gbdev.io/pandocs/Audio_Registers.html)).
- **Carries into the tool** — a check worth shipping: **"the machine under test must have an
  APU even when nothing is playing"**. It is invisible in almost every test and then
  silently converts a measurable failure into a hang, which a runner reports as a timeout —
  the least informative verdict there is. Any adapter that offers a "headless/no-audio"
  mode should be probed for it. And the counter-argument is answered with a number rather
  than a shrug: emulating the APU with a device-free sink costs **+0.02 ms per frame** here
  (0.245 → 0.265 ms, against a 16.743 ms budget), so "it is too expensive for a test run" is
  not true. What *is* expensive is a real host-audio path that paces the producer against the
  audio clock — a different thing, which a conformance runner should never be inside.
## 2026-08-14 — a documented sentinel read as "anything else" (`wIsInBattle == $FF`)

**Not an emulation bug — a bug in something *reading* emulated memory**, which is
why it belongs here: the conformance tool's reference adapter is exactly such a
reader ([`conformance.md`](conformance.md)), and this is the failure mode that
class of code has.

- **Symptom, in user terms:** the trainer view showed a **battle panel — an
  opponent, its level, HP, stats and moves — while the player was standing in
  the overworld**, immediately after blacking out. Everything in the panel was
  stale garbage from the battle that had just been lost, presented with the same
  confidence as live data. Nothing crashed, nothing looked broken; it was simply
  *wrong*, which is the worst way for a tracker to fail.
- **Cause:** `wIsInBattle` (`$D057`) has **four** documented values, not three.
  `pret/pokered`'s `ram/wram.asm` says so directly, above the label: *"lost
  battle, this is -1 / no battle, this is 0 / wild battle, this is 1 / trainer
  battle, this is 2"*. The reader matched `0 => none, 1 => wild, _ => trainer` —
  a catch-all arm that swallowed the `$FF` sentinel. `$FF` is written by
  `.allPokemonFainted` in `home/overworld.asm`, which is the *end* of a battle,
  not one in progress.
- **Caught by:** **nothing, and nothing in a ROM-driven suite can.** Blargg,
  Mooneye and acid2 all compare a framebuffer or a serial string produced by the
  cartridge; the cartridge was entirely correct here. The defect lived in a
  *consumer* of emulated state, and it is invisible to every check that only
  asks the game what it thinks. Same gap as the three render bugs in
  [`rendering.md`](rendering.md): the emulator was right and the user was still
  shown the wrong thing.
- **Behaviour being violated:** the game's own documented WRAM contract, cited
  above (`pret/pokered`, `ram/wram.asm` `wIsInBattle`; writers in
  `engine/battle/core.asm` `InitWildBattle` / `InitBattleCommon` and
  `home/overworld.asm` `.allPokemonFainted`). For a *hardware*-level analogue
  the same rule applies to any register whose reserved encodings are documented:
  an unlisted value is not a licence to pick the nearest listed one.
- **New test — and it is the shape that matters:**
  `battle_kind_maps_the_flag` now asserts the three real values **and iterates
  every remaining byte 3..=$FF**, requiring "no battle" for all of them. Pinning
  the enumerated cases alone is what let this through; the test that catches it
  is the one that pins *the entire domain*.
  **Conformance candidate: for any documented multi-valued field an adapter
  decodes, the check must cover every input value, not the named ones.** A
  catch-all arm over a documented enum is a defect until the values it swallows
  are enumerated — and, concretely for a telemetry/`SERIAL_LINK`-style adapter,
  a `battle_state` field must be able to say "unknown" rather than round to a
  neighbour.

---

## 2026-08-13 — DMG wave-RAM access window (dmg_sound 09: "wave read while on")

- **Symptom:** reading `$FF30–$FF3F` while channel 3 is playing returns wave
  data on every read. On real DMG hardware almost every such read returns
  `$FF`; games/tests that probe wave RAM mid-playback see wrong data. Audibly
  invisible — this is register-visible only, which is why it survived so long.
- **Caught by:** Blargg `dmg_sound` 09 (`09-wave read while on.gb`). It sweeps
  the read's phase in 2-T-cycle steps across the channel's 4-cycle fetch period
  (69 iterations) and CRCs the byte sequence — an emulator needs *sub-instruction*
  APU/CPU synchronisation to pass; a "CGB-style always readable" or "always $FF"
  model both fail the CRC. Diagnostic tell: all-bytes output = CGB-style model;
  all-`$FF` = blanket block; alternating `$FF`/byte with wrong values = window
  modelled but trigger delay or window phase off by 2 cycles.
- **Hardware behaviour:** while CH3 is enabled, any wave-RAM access maps to the
  byte CH3 is currently reading, and on DMG it only *works* within roughly
  2 T-cycles of the channel's own fetch; otherwise reads return `$FF` and writes
  are dropped (Pan Docs "Audio Details" → "Wave RAM"; gg8 wiki "Gameboy sound
  hardware" → Obscure Behavior). The channel fetches a byte every period tick
  ((2048−freq)×2 T-cycles), and a trigger reloads the fetch timer with one full
  period **plus 4 T-cycles** (matches SameBoy's `(freq ^ 0x7FF) + 3` reload at
  2 MHz; calibrated here against the test's CRC).
- **Carries into the tool:** the harness's single-ROM TDD mode
  (`AUDIO_ROM=… cargo test --release --test audiocapture`) prints the ROM's
  actual byte sequence from `$A004` — the printed pattern classifies the failure
  mode as above. The per-fixture pinned `$A000` verdict (this change) is the
  regression guard.

## 2026-08-13 — DMG wave-RAM retrigger corruption (dmg_sound 10: "wave trigger while on")

- **Symptom:** retriggering CH3 (NR34 bit 7) while it is already playing leaves
  wave RAM intact in a naive emulator. On DMG, if the retrigger lands on the
  cycle the channel is about to fetch, the first bytes of wave RAM are
  *corrupted* — games relying on it (or tests CRC-ing wave RAM afterwards)
  diverge.
- **Caught by:** Blargg `dmg_sound` 10. Same 2-cycle phase sweep; after the
  retrigger it dumps all 16 wave bytes and CRCs them. Corruption appears in the
  dump as byte 0 replaced (fetch position < byte 4) or bytes 0–3 replaced by an
  aligned block.
- **Hardware behaviour:** on DMG (not CGB), triggering CH3 while it is enabled
  and reading a byte on that same cycle rewrites wave RAM: if the byte being
  read is one of the first four, byte 0 is overwritten with it; otherwise the
  aligned four-byte block containing it is copied over bytes 0–3 (Pan Docs
  "Audio Details" → triggering while reading; modelled in `Sound::wb` NR34,
  condition `delay == 0` i.e. fetch imminent on this exact cycle).
- **Carries into the tool:** same verdict pinning as 09; the dump printed by the
  ROM itself is the diagnostic.

## 2026-08-13 — DMG wave-RAM write window (dmg_sound 12: "wave write while on")

- **Symptom:** writes to `$FF30–$FF3F` during playback either always land
  (CGB-style, at the current byte) or are always ignored (this emulator's old
  model). On DMG a write lands **only** inside the same 2-T-cycle fetch window,
  and then at the byte the channel is reading — not at the addressed byte.
- **Caught by:** Blargg `dmg_sound` 12 (DMG-only; CGB "behaves erratically" per
  the test source, so it requires DMG mode — an emulator booting these carts as
  CGB fails it structurally). Phase-sweeps a `$F7` write, dumps wave RAM, CRCs.
- **Hardware behaviour:** as for 09 (same access window, write side): Pan Docs
  "Audio Details"; gg8 wiki Obscure Behavior.
- **Carries into the tool:** same as 09.

## 2026-08-13 — APU clock synchronisation is per-access, not per-instruction (structural)

- **Symptom (of the gap):** any APU behaviour measured at sub-instruction
  resolution is unfixable no matter how correct the APU logic is, because the
  APU's notion of "now" only advances at instruction boundaries. The same class
  of bug as Blargg `mem_timing` for the timer.
- **Caught by:** dmg_sound 09/10/12 collectively (no other wired ROM measures
  sub-instruction APU timing). **Suite gap:** nothing exercises *CGB* wave
  access (`cgb_sound` is unwired) — the CGB always-accessible path and the
  current-byte write path added here are untested by any ROM.
- **Hardware behaviour:** the APU is clocked by the same 4.19 MHz clock as the
  CPU; a memory access happens on a specific M-cycle of its instruction (Pan
  Docs "Audio Details"; same model the timer's per-access stepping already
  implements for `mem_timing`).
- **Implementation shape worth copying:** no per-T-cycle APU loop. The MMU's
  existing per-access hook (`bus_tick`) advances the APU clock 4 T-cycles before
  each access and the instruction's remaining cycles at the end; synthesis stays
  event-driven. Cost measured at ~25 µs/frame with audio on, zero with audio off.
- **Carries into the tool:** `sound::tests::wave_window_state_survives_save_load`
  — proves the new `since_fetch` counter is serialised (deliberately dropping it
  makes the test fail with a windowed-read desync, verified 2026-08-13).

## 2026-08-13 — the serial port had never been driven by a partner (coverage gap)

Recorded per the standing convention. This entry is unusual: **nothing was
broken**, and that is the finding.

- **Symptom, in user terms:** a game that needs a link partner — every Pokémon
  trade, Tetris versus, any two-player cable game — cannot get past its
  handshake, because an internal-clock transfer with nothing on the other end
  never receives a byte and never raises the serial interrupt. The game waits
  for ever. On this emulator that was the *only* possible outcome before now:
  both MMU constructors installed a no-op callback and nothing in the repo ever
  replaced it, so serial output was silently dropped and serial input never
  existed.
- **Caught by:** **no test ROM catches this, and none can.** Blargg's suite
  *uses* the serial port as its results channel but never tests it; Mooneye's
  one serial test (`boot_sclk_align-dmgABCmgb`) checks clock alignment, not
  exchange. A link partner is by definition a second machine, so a single-ROM
  harness has nothing to talk to. That is the gap: **the whole two-console half
  of the Game Boy is invisible to a ROM-driven suite.**
- **Hardware behaviour:** one console supplies the clock and both shift a byte
  simultaneously; with the internal clock the rate is 8192 Hz and `SC` bit 7
  clears with a serial interrupt when eight bits are done (Pan Docs,
  [Serial Data Transfer](https://gbdev.io/pandocs/Serial_Data_Transfer_(Link_Cable).html)).
- **New test — and it is a *protocol* test, not a frame hash.**
  `examples/gen1_trade.rs` completes a Generation 1 Pokémon trade against
  Pokémon Blue and reads the received Pokémon back out of WRAM. That is a
  different *kind* of check from everything else in the suite: no serial
  string, no framebuffer hash, no audio property — a real bidirectional
  protocol conversation with a commercial game, whose verdict is "the game
  accepted what we sent and inserted it into its own data structures".
  **Conformance candidate: drive a documented link protocol to completion and
  verify the far side's state.** It is the strongest kind of evidence a
  conformance tool can offer, because the game itself is the judge, and it is
  the natural home for the `SERIAL_LINK` adapter the tool will need anyway.
- **Also worth carrying:** the emulator's serial model is *instantaneous* (the
  whole byte transfers the moment `SC` is written, and bit 7 is never cleared).
  Gen 1 does not notice, because it drives everything from the serial interrupt
  rather than polling `SC` — but a game that polls bit 7 would hang for ever.
  That is a real divergence with **no ROM coverage**, and a good check for the
  tool: *after an internal-clock transfer, `SC` bit 7 must read back clear.*

---

## 2026-08-14 — Mooneye acceptance 13/66 → 50/66: five core-timing defects

Recorded per the standing convention. All five were found the same way: run
Mooneye's `acceptance` suite under **its own** criterion (`LD B,B` breakpoint +
Fibonacci registers `B=3 C=5 D=8 E=13 H=21 L=34`, per the suite's README), read
the failing test's source to learn what it asserts, then fix. The runner is
[`examples/mooneye_scout.rs`](../examples/mooneye_scout.rs); it never looks at a
reference image, so its verdict cannot drift with our own rendering.

**Note the two criteria differ and both are honest.** The GBEmulatorShootout
screenshot-compares Mooneye's output, and Mooneye prints a boot-register dump
alongside its verdict — so a test we genuinely pass still fails the screenshot
when our CGB boot state differs from the DMG reference. Under that criterion the
same work moved us **6/66 → 32/66**. The register criterion is the one that
measures the behaviour under test, so it is the one these fixes were driven by;
the 18-test gap between them is entirely the missing DMG boot model.

### 1. The timer was two counters, not one

- **Symptom:** anything that reads `DIV` and `TIMA` together drifts. Music tempo
  and any `DIV`-seeded randomness are subtly wrong, and a game that resets `DIV`
  in a loop (a common idiom) never sees the timer interrupt hardware would give
  it.
- **Caught by:** Mooneye `acceptance/timer/*` — **10 of 13 failed**. (Our own
  docs previously claimed this divergence had "no wired test". It does.)
- **Hardware:** there is one 16-bit system counter. `DIV` is its top byte, a
  `DIV` write clears all sixteen bits, and `TIMA` counts **falling edges** of
  `counter[tap] AND TAC.enable` with `tap ∈ {9,3,5,7}` selected by `TAC & 3`.
  Pan Docs, ["Timer and Divider Registers"](https://gbdev.io/pandocs/Timer_and_Divider_Registers.html);
  Gekkio, *Game Boy: Complete Technical Reference* §Timer.
- **New tests:** six unit tests in `src/mmu/timer.rs`, one per behaviour, so a
  regression is named rather than being a count that moved. **Conformance
  candidates:** *writing `DIV` while the tap bit is high must increment `TIMA`*;
  *stopping the timer while the tap bit is high must increment `TIMA`*;
  *`TIMA`'s phase is absolute — it steps 1024 T after a `DIV` write, not after
  the `TAC` write*.

### 2. `TIMA` overflow reloaded instantly

- **Symptom:** a timer interrupt handler that reads `TIMA` to decide how much
  time it lost sees `TMA` where hardware shows `$00`, so it mis-corrects.
- **Caught by:** Mooneye `acceptance/timer/tima_reload`,
  `tima_write_reloading`, `tma_write_reloading`.
- **Hardware:** on overflow `TIMA` reads `$00` for 4 T-cycles; only then is `TMA`
  copied in and `IF` bit 2 set. A `TIMA` write inside that window cancels both;
  a `TIMA` write *on* the reload cycle is ignored; a `TMA` write on the reload
  cycle is picked up by that very reload (Pan Docs, "Timer Obscure Behaviour").
- **New tests:** `tima_reads_zero_for_four_cycles_before_tma_is_loaded`,
  `a_tima_write_in_the_overflow_window_cancels_the_reload`,
  `the_reload_cycle_beats_a_tima_write_and_honours_a_tma_write`.

### 3. Interrupt dispatch charged 16 T-cycles instead of 20, and latched the
vector too early

- **Symptom:** everything an interrupt handler measures against `DIV` is one
  M-cycle out; a raster or audio routine timed from an interrupt drifts.
- **Caught by:** Mooneye `acceptance/intr_timing` (which states the claim
  outright: "serving an interrupt is supposed to take 5 M-cycles") and
  `acceptance/interrupts/ie_push`. Blargg `interrupt_time` would also catch it
  but cannot run headless — it polls `NR52` and needs an APU.
- **Hardware:** dispatch is 5 M-cycles — two idle, `PC`-high pushed on M3,
  `PC`-low on M4, vector fetch on M5 — and the vector is decided from `IE & IF`
  at the start of M4. So a high-byte push that lands on `IE` (`SP = $0000`) can
  cancel the dispatch entirely (`PC` becomes `$0000`, **no** `IF` bit is
  acknowledged, `IME` is still cleared) or redirect it to another vector, while
  the low-byte push's own write to `IE` is too late. Gekkio, *CTR* §Interrupts.
- **Conformance candidate:** *an interrupt whose `PC`-high push overwrites `IE`
  with a value that masks it must jump to `$0000` and leave `IF` untouched.*

### 4. `DI` was delayed and repeated `EI`s never enabled interrupts

- **Symptom:** a `DI` in a tight critical section does not actually close the
  window, so an interrupt fires inside it. And any routine that writes `EI`
  twice in a row — or `EI` inside a loop with no other instruction between —
  runs with interrupts permanently off.
- **Caught by:** Mooneye `acceptance/ei_sequence` (18 consecutive `EI`s),
  `acceptance/rapid_di_ei`, `acceptance/di_timing-GS`.
- **Hardware:** `EI` sets `IME` after the instruction *following* it, and the
  delay is a latch, not a restartable counter — a second `EI` does not push it
  back. `DI` clears `IME` immediately on DMG/MGB/SGB (Pan Docs, "Interrupts";
  `di_timing-GS` says so in its own header, and notes CGB/AGB differ).
- **Conformance candidate:** *a run of N `EI` instructions must service a pending
  interrupt after the second one.*

### 5. OAM DMA was instantaneous — which made a whole class of tests
unmeasurable

- **Symptom on its own:** a game that starts a DMA and then reads OAM before it
  finishes sees the new data early. In practice benign, since real code waits.
- **The real cost:** Mooneye measures *instruction* timing by DMAing a known
  byte into OAM, pointing `SP` there, and seeing which of an instruction's
  pushes survive. With an instantaneous DMA there is nothing to measure against,
  so **14 instruction-timing tests could not pass no matter how correct the CPU
  was**. That is the finding worth carrying: an unmodelled *peripheral* can hide
  a whole dimension of *core* accuracy.
- **Caught by:** `acceptance/oam_dma_start`, `oam_dma_timing`, `oam_dma_restart`,
  `oam_dma/reg_read`, `oam_dma/sources-GS`, plus `rst_timing`, `call_timing`,
  `call_timing2`, `call_cc_timing`, `call_cc_timing2`, `push_timing`,
  `ret_timing`, `ret_cc_timing`, `reti_timing`, `jp_timing`, `jp_cc_timing`,
  `add_sp_e_timing`, `ld_hl_sp_e_timing`, `pop_timing`.
- **Hardware:** the write to `$FF46` is M=0, the transfer begins at M=2 and
  copies one byte per M-cycle for 160 M-cycles; OAM reads return `$FF` and
  writes are dropped meanwhile. A restart lets the previous transfer run through
  M=1 (Pan Docs, ["OAM DMA Transfer"](https://gbdev.io/pandocs/OAM_DMA_Transfer.html);
  the M=0/1/2 schedule is stated verbatim in `oam_dma_start`'s header).
- **Fixing those 14 also required two CPU corrections**, both about *where in an
  instruction* a bus access falls: a 16-bit push writes the **high** byte first,
  and the internal (non-bus) M-cycles of `PUSH`/`RST`/`CALL`/`RET cc` sit
  *before* their memory accesses, not as slack at the end of the instruction.
- **Conformance candidate:** *`LD A,(OAM)` 161 M-cycles after a write to `$FF46`
  must read `$FF`, and 162 M-cycles after it must read the copied byte.*

### What is still red, and why

Of the 16 remaining acceptance failures: **11 are the PPU** (mode-3 timing, STAT
IRQ blocking, LCD-on timing — a documented structural non-goal needing a
cycle-stepped PPU) and **5 are boot state** (`boot_div`, `boot_hwio`,
`boot_regs`, `bits/unused_hwio-GS`, `serial/boot_sclk_align`), which need a DMG
boot mode this emulator does not have — it always boots the CGB core. Neither is
a timer, interrupt or MMU defect. **Nothing in the timer, interrupt-dispatch or
OAM-DMA groups is red.**

### Measured with the DMG boot mode (landed in `9d70c17`)

On the shootout's 66-test acceptance set, measured all three ways so the two
changes can be told apart:

| | register criterion | screenshot criterion |
|---|---:|---:|
| before either | 13/66 | 6/66 |
| DMG mode alone (`origin/main`) | 15/66 | — |
| this work alone | 50/66 | 32/66 |
| **both** | **52/66** | **52/66** |

37 of the 39 tests gained are this work; 2 are DMG mode. No regressions in
either direction. An earlier scout measured "+9 tests from forcing DMG" against
the *old* core — that gain was real but it lands almost entirely in the
screenshot column, on tests this work has since made pass on registers too, so
the two overlap rather than adding.

Two things worth recording:

- **The two criteria converge exactly once we can boot a DMG.** That is the
  scout report's diagnosis confirmed: the 50-vs-32 gap was never a behaviour
  difference, only the boot-register dump Mooneye prints beside its verdict.
  It is also a reminder that a screenshot-compare suite conflates *behaviour*
  with *boot state*, which is worth saying out loud in a conformance tool.
- **The DMG boot flips two of the five boot-state tests, not five**:
  `bits/unused_hwio-GS` and `boot_regs-dmgABC`. `boot_div-dmgABCmgb` and
  `boot_hwio-dmgABCmgb` need the exact hardware state the *boot ROM* leaves
  behind (the value of the system counter, and every I/O register, at the moment
  `PC` reaches `$0100`) — a strictly harder claim than the boot registers, and
  the natural next step. `serial/boot_sclk_align-dmgABCmgb` needs the serial
  clock's phase relative to that same counter. No regressions in either
  direction.

### `blargg/interrupt_time`: 12 → 13 cycles, and the remaining failure is a
different bug

The 5-M-cycle dispatch fix moves this ROM's measured interrupt time from `$0C`
to `$0D`. **`$0D` is the correct answer** — `tests/interrupt_time/interrupt_time.s`
says so in its own header: *"Should take 13 cycles at either speed."* The row in
`testharness/baseline.txt` has been re-blessed and its note in
`testharness/testroms.rs` now names the real remaining cause instead of the
dispatch gap, which is fixed.

The ROM still reports `Failed`, for an unrelated reason that its output makes
plain. It prints four rounds of `speed, d, cycles`:

```
00 00 00   00 08 0D   00 00 00   00 08 0D
```

Rounds 3 and 4 run after `cpu_fast`, so their first column should read `01`, not
`00` — **the CGB double-speed switch is not taking effect**, or `get_cpu_speed`
cannot see it. Both cycle counts are already right. That is a speed-switch
finding, not an interrupt-timing one, and it is a candidate check in its own
right: *after a `KEY1` bit-0 write followed by `STOP`, `KEY1` bit 7 must read
set.* It also explains why `daid/speed_switch_timing_*` are red.

> **Resolved — and by the other half of the same day's work.** The speed switch
> *was* taking effect; the ROM could not see it because it synchronises through
> the APU (`apu.s`, spinning on `NR52`) and the APU was being clocked at CPU
> speed instead of the base clock, so at double speed it ran twice as fast and
> the ROM's sync never landed where it expected. Fixing that (see "APU sample
> accuracy" below) makes rounds 3 and 4 print `01`, and the ROM prints
> **`Passed`** — its baseline row is now `PASS`.
>
> **This is the entry worth keeping.** Neither fix turns the test green alone:
> the dispatch fix supplies the `0D`, the APU clock fix supplies the `01`, and
> they were found independently by two people looking at different suites. It is
> the strongest argument in this file for recording a **verdict** rather than a
> frame hash — a hash would have been re-blessed twice and told nobody that the
> ROM had started passing. It is also a caution for the conformance tool's
> explanations: "the CGB double-speed switch is not taking effect" was a
> reasonable reading of the evidence and was **wrong about the component**. When
> a test synchronises through one subsystem to measure another, the failure
> names the wrong one.

---

## APU sample accuracy — the SameSuite fixes (2026-08-14)

SameSuite's APU sub-suite went from **2/61 to 49/61** on the GB Emulator
Shootout's published set. These are the fixes, each with the observable symptom
a foreign emulator carrying the same bug would show.

**A note on the whole class first:** every one of these is invisible to Blargg's
`dmg_sound`, which we pass 12/12. Blargg reads register *state*; SameSuite reads
the channels' *output*, sample by sample, through the CGB PCM registers. **An
emulator can pass every published DMG sound test and still be wrong about what
the speaker hears on every single frame.** That is the finding, and it is the
argument for the conformance tool carrying an audio-capture adapter rather than
only a serial/framebuffer one.

### 1. No PCM12 / PCM34 registers at all

- **Symptom:** a CGB game that reads `$FF76`/`$FF77` to visualise or gate on its
  own audio output sees a constant value. On our build the MMU returned a
  hard-coded `0x00`. 57 of SameSuite's 61 APU tests cannot even run.
- **Caught by:** every `samesuite/apu/channel_*` test; nothing in Blargg,
  Mooneye or acid2.
- **Hardware:** `PCM12`/`PCM34` are CGB-only read-only registers exposing each
  channel's live 4-bit digital output, CH1/CH3 in the low nibble and CH2/CH4 in
  the high nibble; a channel whose enable or DAC is off reads 0
  ([Pan Docs, Audio Registers](https://gbdev.io/pandocs/Audio_Registers.html)).
- **Carries into the tool:** *a CGB adapter must expose the APU's digital
  output per channel.* This is the cheapest possible audio-conformance probe —
  no waveform comparison, no resampling, no tolerance: an integer per channel
  per cycle, and the ROM does the judging.

### 2. The frame sequencer was a free-running counter, not DIV-driven

- **Symptom:** a game that resets `DIV` (many do, to seed randomness or to
  re-time the timer) gets note lengths, envelopes and sweeps that drift from
  hardware — audibly wrong tempo in tracker-style music that resets DIV.
- **Caught by:** SameSuite `div_write_trigger`, `div_write_trigger_10`,
  `div_write_trigger_volume`, `div_write_trigger_volume_10`,
  `div_trigger_volume_10` — and **note that the first two do not need PCM at
  all: a DMG passes them.** Blargg does not catch it.
- **Hardware:** the DIV-APU is clocked by the *falling edge* of system-counter
  bit 12 (bit 13 at double speed), so zeroing `DIV` while that bit is set is
  itself an event ([Pan Docs, Audio Details → DIV-APU](https://gbdev.io/pandocs/Audio_details.html)).
- **Carries into the tool:** *reset DIV at a known phase and check the length
  counter advanced.* Cheap, model-independent, and observable through `NR52`
  alone — no PCM required, so it works on a DMG adapter too.

### 3. Writing DIV left the sub-DIV counter running (timer, not APU)

- **Symptom:** none, if you only read `DIV` — which is exactly why it survives.
  Everything else clocked off the same 16-bit counter is silently up to 255
  T-cycles out of phase after any DIV write.
- **Caught by:** Mooneye `acceptance/timer/div_write`; and it makes SameSuite's
  whole `div_*` group non-deterministic, which is how we found it.
- **Hardware:** writing `DIV` resets the **whole** 16-bit counter
  ([Pan Docs, Timer and Divider Registers](https://gbdev.io/pandocs/Timer_and_Divider_Registers.html)).
- **Carries into the tool:** a good example of a bug whose *only* symptom is in
  another component. A conformance tool that reports "timer OK, APU broken"
  when the defect is in the timer is not helping; cross-component provenance is
  worth designing for.

### 4. The APU ran at CPU speed in CGB double-speed mode

- **Symptom, and it is a big one:** every sound in a CGB double-speed game plays
  an octave high. Worse on a real frontend — the audio queue paces the emulator
  against the sound card, so producing samples twice as fast throttles the
  *whole emulator*. Measured on Blargg `cpu_instrs` (which switches to double
  speed): **33.7 fps and 48% of frames late before the fix, 59.70 fps and 0%
  after**, same box, same run.
- **Caught by:** no test ROM we run. `examples/audio_latency.rs` shows it
  plainly once you look at the frame rate; SameSuite's double-speed tests
  (`channel_N_align`, `channel_N_duty`, `channel_3_delay`, …) fail on it.
- **Hardware:** the APU is wired to the unmodified 4.194304 MHz clock; only the
  CPU is doubled ([Pan Docs, CGB Registers → KEY1](https://gbdev.io/pandocs/CGB_Registers.html)).
- **Carries into the tool:** *play a tone, switch to double speed, and check the
  pitch does not move.* An audio-capture adapter can check this with a
  zero-crossing count and no reference data at all — the expected answer is
  "the same as before the switch", which is the strongest kind of check because
  nothing has to be blessed.

### 5. Duty output was a function of position, not a latch; and the table was rotated

- **Symptom:** the square channels' waveform is phase-shifted relative to
  hardware and their first pattern position after a power-on is wrong. Inaudible
  on its own — a square wave's phase is not something you can hear — which is
  precisely why it survives in emulators that only listen.
- **Caught by:** SameSuite `channel_N_duty` and `channel_N_align`. **Nothing
  audible catches it, and no framebuffer or serial test can.**
- **Hardware:** the duty bit is latched at each frequency-timer expiry, so a
  freshly powered channel outputs 0 for its whole first position regardless of
  the table; an APU power-off resets the duty counters
  ([Pan Docs, Audio Registers → NRx1](https://gbdev.io/pandocs/Audio_Registers.html);
  gg8 wiki, "Gameboy sound hardware" → Power Control).
- **Carries into the tool:** *a check whose expected value is a phase, not a
  level.* Worth keeping as an example of something only a cycle-resolved probe
  can see.

### 6. Trigger timing: every channel started at the wrong cycle

- **Symptom:** short percussive notes and rapid retriggering (drum tracks, sound
  effects fired every frame) come out with the wrong first sample. Individually
  a few T-cycles; over a fast arpeggio it is a different waveform.
- **Caught by:** SameSuite `channel_N_delay`, `channel_N_align`,
  `channel_N_restart`, `channel_3_first_sample`, `channel_4_delay`,
  `channel_4_lfsr*`.
- **Hardware, per channel:**
  - Square: the trigger reloads the frequency timer, so the first duty step is a
    whole period away — plus 6 T, rounded up to the free-running 1 MHz grid the
    timer counts on. A *re*-trigger of a running channel is 4 T earlier and does
    not reset the duty position (SameSuite `channel_N_restart` states this
    outright).
  - Wave: the position advances *before* the fetch, so a triggered CH3 never
    plays sample 0 — it starts at sample 1 (Pan Docs, Audio Details → Wave
    Channel).
  - Noise: the trigger reloads only the NR43 divisor and leaves the shift stage
    half-way through, so the first step is at `divisor + (period - divisor)/2`.
    `channel_4_delay` measures this at three shift settings and a plain
    full-period reload is late by 0/1/3 M-cycles as the sample grows 2/4/8.
- **Carries into the tool:** *the trigger-to-first-sample delay is a per-channel
  constant worth publishing.* It is the single most useful number an APU
  conformance report could hand an author, because it is one integer per channel
  and it is what every one of these tests is really measuring.

### 7. CH3's sample buffer survived an APU power-off

- **Symptom:** re-triggering the wave channel after a power cycle plays the
  sample it was playing *before* the power-off for a whole period — a click, or
  a wrong first sample on every note in a game that power-cycles the APU between
  notes (which is a common way to silence it).
- **Caught by:** SameSuite `channel_3_first_sample`. Blargg's wave tests
  (`dmg_sound` 09/10/12, all passing here) do not.
- **Hardware:** the sample buffer is inside the powered domain and empties with
  it; a trigger deliberately does *not* refill it (Pan Docs, Audio Details →
  Wave Channel).
- **Carries into the tool:** a good example of *stale state surviving a reset*,
  a bug class worth probing generally: power a component down, power it up, and
  check nothing from before is visible.

### 8. NRx2 reloaded the live volume ("zombie mode")

- **Symptom:** any game that writes NRx2 while a note is sounding — which is how
  a lot of Game Boy music does its volume envelopes by hand — hears a volume
  jump back to the register value instead of hardware's perturbation of the
  running value.
- **Caught by:** SameSuite `channel_N_volume` (all twelve old/new
  direction+period combinations) and `channel_N_nrx2_glitch`.
- **Hardware:** NRx2 sets what the *next trigger* loads; writing it while the
  channel plays perturbs the live volume by a revision-dependent rule. SameSuite's
  own README records that the glitch "behaves differently across CPU-CGB
  revisions" and that only revision E is documented, so the gg8-wiki DMG formula
  is *not* the right answer for a CGB-booting emulator. Derived rule for CGB-E,
  in `VolumeEnvelope::write_nrx2`: an envelope with period 0 is idle, so loading
  a fresh NRx2 restarts its timer and it fires once immediately in the **old**
  direction; switching from decrement to increment adds two; reversing direction
  reverses the counter (`v = 16 - v`).
- **Carries into the tool:** *this check must be revision-qualified.* It is the
  clearest case in the whole suite where "correct" depends on which console you
  mean, and a tool that reports a bare FAIL here is misleading its user. The
  report should say *which* revision the emulator matches.

### 9. Coverage gap worth recording

Blargg `dmg_sound` 12/12 and `cgb_sound` 12/12 were true before any of this, and
stayed true throughout. **Neither moved by a single sub-test across nine
behavioural fixes.** Any conformance tool that treats "passes Blargg sound" as
"the APU is right" will report a clean bill of health on an emulator that is
wrong about the output of every channel on every frame.

---

## APU corner cases — sweep, envelope and noise restart (2026-08-14)

Second pass over the same suite, from the full-suite audit's work item ②.
SameSuite went **50/76 → 57/76** (its APU half **49/69 → 56/69**), measured with
`cargo run --release --example samesuite -- <shootout>/testroms/samesuite` before
and after. Nothing else moved: Mooneye `acceptance` 53/75, Mealybug 3/79,
`testroms` 33/35 all identical, and the audio-perf gate is unchanged
(0.266 ms/frame before, 0.266–0.278 after, three runs each).

### 10. The sweep's overflow check is not instantaneous

- **Symptom:** a channel whose frequency sweep is about to overflow goes silent
  a handful of microseconds too early — audible as a swept note (a laser, an
  explosion, a rising alarm) being clipped a fraction short at its top end. In
  a test that samples the output every M-cycle it is unmistakable; by ear it is
  the difference between a sweep that ends cleanly and one that ends abruptly.
- **Caught by:** SameSuite `channel_1_sweep` (FAIL → PASS),
  `channel_1_sweep_restart` and `channel_1_sweep_restart_2` (still red, see
  below). **Nothing in Blargg catches it** — `dmg_sound` `04-sweep` and
  `05-sweep details` read NR52 milliseconds after the event, by which time both
  the early and the late answer are the same.
- **Hardware:** the sweep unit computes `freq ± (freq >> shift)` with a serial
  shifter, so the recalculation whose overflow disables the channel finishes
  `shift + 1` M-cycles after the frequency write, not with it. `channel_1_sweep`
  states the mechanism in its own source: *"8 cycles after trigger, the APU
  checks if the NEXT trigger overflows the frequency. If it does, stop the
  channel."* — with `shift = 7`. A trigger's own calculation starts one M-cycle
  later still (`channel_1_sweep_restart` round 2). Clearing NR10's shift field
  **parks** a calculation already in flight rather than cancelling it
  (`channel_1_sweep_restart` round 3: the ROM disables the sweep inside the
  window and the channel that was about to die keeps playing).
- **New tests:** `sound::tests::a_sweep_overflow_kills_the_channel_late_not_instantly`
  and `::clearing_the_sweep_shift_parks_a_pending_overflow_check`. Teeth proven
  by forcing the delay to zero — the first goes red.
- **Carries into the tool:** *the delay between a sweep's frequency update and
  its overflow check is observable and must be checked.* It needs the PCM
  registers (or an audio-capture adapter) and per-M-cycle sampling; a
  register-state probe cannot see it.

### 11. The volume envelope's period counter is free-running, and the step it schedules lands on the next 512 Hz event

- **Symptom:** a game that fades a note in or out by writing NRx2 *while the
  note is playing* — the standard trick for a volume ramp that is not one of
  the eight envelope speeds — gets no ramp at all if it enables the envelope
  from a period of 0, and gets every subsequent step one 64 Hz period late.
  Held notes stay at a constant volume where hardware fades them.
- **Caught by:** SameSuite `channel_1_nrx2_glitch`, `channel_2_nrx2_glitch`,
  `channel_1_nrx2_speed_change`, `channel_2_nrx2_speed_change` (all four
  FAIL → PASS). **Nothing in Blargg catches it**; `dmg_sound` stayed 12/12
  across the change and its capture hash moved with every analysed property
  (RMS, peak, silence, dominant pitch, sample count) identical.
- **Hardware:** the 3-bit period counter is a free-running down-counter that
  *reloads* from NRx2 when it reads zero rather than stopping there, so an
  envelope parked at zero — which is where a period-0 NRx2 leaves it — resumes
  the moment a non-zero period is written, with no trigger. The step itself is
  performed by a separate latch, set when the counter reads zero and acted on
  at the next **512 Hz** DIV-APU event, not the next 64 Hz one. That is why
  `channel_N_nrx2_speed_change` tests 3–7 see the first step arrive ~2 ms after
  the write and the rest at 64 Hz. (Model corroborated against SameBoy's
  `envelope_clock` / `volume_countdown` split, `Core/apu.c`; reimplemented, not
  copied.)
- **A latent inconsistency this exposed:** our frame sequencer clocks length on
  even steps and sweep on step 2 of 4, i.e. both one step ahead of the event
  that acts on them — but the envelope counter ran on step 7, the same step as
  the volume change. Moving the count to step 6 puts all three on the same
  phase and keeps the volume change on step 7 where
  `channel_N_volume_div` measures it.
- **New tests:** `sound::tests::enabling_the_envelope_mid_note_steps_within_one_div_apu_event`
  and `::a_triggered_envelope_still_steps_on_the_64hz_boundary`. The second one
  is the regression guard for the phase move and goes red if the counter is put
  back on step 7.
- **Carries into the tool:** *check the envelope through NRx2 writes, not only
  through triggers.* Every published DMG sound suite tests the envelope by
  triggering it; the mid-note write is the case real music drivers use.

### 12. Re-triggering the noise channel costs one whole sample

- **Symptom:** every percussion hit that re-triggers CH4 without letting it stop
  (drum rolls, hi-hats, most Gen-1-era percussion) starts one LFSR sample early,
  so the noise burst is a different sequence of bits from the one hardware
  plays. The pitch and length are right; the grain is wrong.
- **Caught by:** SameSuite `channel_4_lfsr_restart` and
  `channel_4_lfsr_restart_fast` (both FAIL → PASS). They reconstruct 128
  consecutive LFSR outputs after a restart, so a one-sample slip shifts the
  whole reconstruction. **Nothing in Blargg catches it.**
- **Hardware:** the divider that clocks the LFSR is free-running; a trigger
  reloads the countdown to its next tick but not the divider itself, so the
  first post-restart step waits for the divider's next edge instead of landing
  on the reloaded countdown. (This is the same free-running-counter structure
  SameBoy models explicitly as `noise_channel.counter` plus a bit tap — see the
  gap recorded below.)
- **New test:** the two ROMs above; the behaviour is also pinned by the
  SameSuite score, which drops 57 → 55 if the extra sample is removed.
- **Carries into the tool:** *a restart check must reconstruct the sequence,
  not sample one point.* One reading after a restart looks right half the time.

### 13. Gaps left open, deliberately, with the reason

Recording these is the point of this file: an unexplained red row is worth less
than a red row with a reason.

- **`channel_{1,2,4}_extra_length_clocking-cgb0B`, `channel_3_extra_length_clocking-cgb0`,
  `channel_3_extra_length_clocking-cgbB` — 5 ROMs we should NOT pass.**
  *(Superseded — see note 15 below. The conclusion "passing these would mean
  reintroducing a hardware bug" was right about the behaviour and wrong about
  the choice: the revision is selectable per ROM, exactly as `@dmg`/`@cgb` is,
  so all five pass without any game ever seeing the bug. Kept because the
  reasoning below is still the correct description of the hardware.)* They
  assert the length-counter behaviour of **CPU CGB revisions ≤ B**, and the ROM
  source says so outright: *"On revisions <= CPU CGB B, the length counter only
  has to have been disabled before; the current length enable state doesn't
  matter. This breaks at least one game (Prehistorik Man), and was fixed on"*
  later revisions. We model a CGB-E (the zombie-mode rule the previous APU pass
  implemented is revision E's). Passing these would mean reintroducing a
  hardware bug that breaks a real game. The `-cgb0` and `-cgbB` pair also
  disagree with each other, so **no single machine can pass both**.
- **`channel_1_freq_change_timing-{A,cgb0BC,cgbDE}` — at most 1 of 3 winnable.**
  Three revision variants of one measurement; only `-cgbDE` matches the console
  we model. Its first row (single speed) is currently one M-cycle out and its
  second row needs `STOP`-driven double speed, which is the audit's work item
  ③ and another crew's area.
- **`channel_4_equivalent_frequencies`, `channel_4_freq_change`,
  `channel_4_frequency_alignment` — the free-running noise divider.**
  *(Partly superseded — see notes 14 and 16 below. The grouping was right that
  the three share a cause and that it is the two-stage counter; two of the
  three fell to modelling what a trigger leaves each stage holding, without the
  full counter rewrite. `channel_4_freq_change` needs the rewrite and is the
  one row the suite's own author does not pass either.)* These
  three all have one cause: the noise frequency comes from a free-running
  14-bit counter clocked every `(NR43 & 7) * 8` T-cycles (4 if zero), with the
  LFSR stepping on the 0→1 edge of bit `NR43 >> 4`. We model a per-channel
  period timer reloaded at trigger, which gets the *rate* right and the *phase*
  wrong — hence "identical frequencies expressed differently" producing outputs
  a sample apart, which is exactly what `channel_4_equivalent_frequencies`
  exists to catch. Converting to the counter model is the correct fix and is
  scoped, not done: the counter itself is four lines, but the value the trigger
  reloads its countdown with is a table of revision- and instance-specific
  quirks that SameBoy's own source annotates *"This quirk seems way too
  specific"* and *"non-deterministic"*. Attempting it without a real CGB to
  calibrate against risks the eight CH4 rows that pass today.
- **`channel_1_sweep_restart` — a single cell of 144, and it is not a sweep
  bug.** See finding 18 below, which also **corrects the diagnosis recorded here
  for `channel_1_sweep_restart_2`** (it was not a DIV-APU phase error; it now
  passes).

---

## CH4 noise timing, and the CGB revision the APU shipped in (2026-08-14)

Third pass over the same suite, from the full-suite audit's work item ② —
the **CH4 noise frequency/LFSR** sub-cluster, plus the decision the audit
attached to it about the revision-specific `extra_length_clocking` ROMs.

**Measured, before → after** (`make test-external`, the CI gate, on the ROM
versions pinned in `tools/fetch-test-suites.sh`):

| suite | before | after |
|---|---:|---:|
| SameSuite (whole) | 61/76 | **68/76** |
| SameSuite APU half | 56/69 | **63/69** |
| Mooneye `acceptance` + `emulator-only` | 86/103 | 86/103 |
| Mooneye `misc` (`GB_MODEL=cgb`) | 4/8 | 4/8 |
| Mealybug Tearoom | 3 pixel-exact | 3 pixel-exact |
| `testroms` (Blargg + acid2) | 46/47 | 46/47 |

Zero regressions in either direction; `audiocapture` (dmg_sound 12/12, every
hash), `savestate`, `battery` and the 672 library tests are unchanged.

**Do not restate this as a published-shootout gain.** Of the seven rows, only
**two** — `channel_4_equivalent_frequencies` and `channel_4_frequency_alignment`
— appear in the GB Emulator Shootout's 311-row manifest at all; the five
`extra_length_clocking-*` ROMs are not in that corpus. The published-264 delta
from this work is therefore **at most +2**, and it is screenshot-judged, which
this branch has not measured.

### 14. A trigger reloads *neither* noise counter — it halves both

- **Symptom:** noise (percussion, explosions, surf, engine hum) starts a couple
  of microseconds late, and — the audible part — two NR43 encodings of the
  *same* frequency start at measurably different times, so a game that switches
  between them mid-effect gets a click. `$0C`, `$1A`, `$29` and `$38` all
  describe one 16 M-cycle sample; on hardware their first LFSR step lands
  0/0/+2/+1 M-cycles apart, and a naive "reload the whole period" model puts all
  four in the same place.
- **Caught by:** SameSuite `channel_4_frequency_alignment` (nine `(divisor,
  shift)` pairs) and `channel_4_equivalent_frequencies` (the four equivalent
  encodings, over two whole samples). Both were red; both are green now.
  `channel_4_delay` and `channel_4_align`, which were already green, pin the
  shift stage and the grid phase and must stay green — they do.
- **Hardware:** the noise frequency is two counters in series, NR43's divisor
  stage feeding a `>> shift` stage (Pan Docs, *Audio Registers* → NR43). A
  trigger reloads neither to a full period: it leaves both half-way through, so
  the first step lands at `(period - divisor)/2 + divisor/2`, not at `period`.
  The one exception is that the divisor stage cannot be left holding less than
  one of its own ticks — 16 T-cycles, or 8 for code 0, which is a half tick to
  begin with — which is why codes 0 and 1 are floored at a whole tick and code
  3 is not. That floor, and nothing else, is the 0/0/+2/+1 above.
- **New test:** `sound::tests::the_noise_trigger_delay_matches_samesuite` —
  eleven `(NR43, delay)` pairs read off the ROMs' `CorrectResults` tables, not
  off our output. Nothing in `cargo test` runs SameSuite, so this is the gate
  on the calibration. Conformance candidate: **two encodings of one noise
  frequency must trigger at the same instant.**

### 15. The CGB shipped in revisions, and its APU changed between them

- **Symptom:** a game that turns a channel's length counter off by writing
  `NRx4 = $00` behaves differently on an early Game Boy Color than on a late
  one. On CPU CGB 0 and B the write clocks the length counter anyway and can
  silence the channel; from CPU CGB C on it does nothing. Prehistorik Man is
  the game the gbdev community names as broken by it.
- **Caught by:** SameSuite ships the same test five times with the revision in
  the *filename* — `channel_{1,2,4}_extra_length_clocking-cgb0B`,
  `channel_3_extra_length_clocking-cgb0` and `-cgbB`. All five were red. All
  five pass now.
- **Hardware:** extra length clocking is "an NRx4 write while the frame
  sequencer's next step is not a length step, with length previously disabled
  and now enabled and non-zero, decrements it once". On CPU CGB ≤ B the *"and
  now enabled"* half of that condition is simply absent. CH3 differs again
  between the two early steppings: on CPU CGB B the clock that takes CH3's
  counter to zero leaves the channel playing and the *next* one turns it off,
  which is exactly the one-extra-write shift between the two ROMs'
  `CorrectResults` tables. Source for both: the header comment of
  `apu/channel_*/channel_*_extra_length_clocking-cgb0B.asm` (test written by
  Matt Currie).
- **The decision, and why it is not score-gaming.** `-cgb0` and `-cgbB`
  contradict each other, so **no single machine can pass both** — a runner that
  ignores the filename can score at most four of the five however accurate it
  is. The answer is the one the harness already uses for `@dmg`/`@cgb`: read
  the machine off the ROM and run *that* machine. `gameboy::sound::CgbRevision`
  defaults to `CgbC` (C, D, E — the fixed silicon, and what every DMG does),
  `examples/samesuite.rs::revision_for` picks the revision from the filename
  suffix, and `$GB_CGB_REVISION` exposes the same knob to a frontend. The ROM
  still judges itself; the knob only picks which machine it is judging. A DMG
  is never affected — `Sound::extra_length_rule` pins the modern rule whenever
  `dmg_mode` is set, so a mis-set env var cannot change how a DMG game plays.
- **New tests:** `sound::tests::extra_length_clocking_follows_the_cgb_revision`
  (nine cases, expectations read off the ROMs' `CorrectResults`) and
  `sound::tests::the_revision_quirk_never_reaches_a_dmg`. Teeth proven by
  forcing `extra_clock_ignores_new_enable` to `false`, which turns the first
  red. Conformance candidate: **a suite that names hardware in a ROM filename
  must be scored on that hardware, and a tool must say which machine each
  verdict was measured on.**

### 16. `channel_4_freq_change` — evidenced as not worth grinding

This is the one row of the sub-cluster left red, and it is left red
deliberately.

- **The suite's own author does not pass it.** SameSuite's
  `apu/README.md` says: *"SameBoy – when emulating CGB-CPU-E, passes all tests
  except `channel_4_freq_change` and `channel_1_sweep_restart_2`."* SameSuite
  exists to develop SameBoy; a row SameBoy fails is a row with no known
  reference implementation.
- **And the ROM says the behaviour is not understood.** Its own header:
  *"This test tests what happens when changing the frequency of channel 4 while
  it's playing. Unfortunately the logic behind it is still unclear."*
- **What we do know, recorded so the next person starts ahead.** Our model
  collapses the two counters of note 14 into a single `period` countdown, so an
  NR43 write mid-sample changes the *rate* of every step after the next one but
  cannot change the *next* one — it fires at whatever the old countdown had
  left, ≤ 16 T-cycles later. The ROM measures the next step landing ~56–70
  T-cycles after the write, which is what a two-stage counter would do: the
  shift stage keeps its position `p` and counts on to the *new* `2^shift`
  threshold, so `(2^shift_new - p)` prescaler ticks remain. Our eight-row
  measurement is consistent with that reading — the one row we already match
  (`$09`→`$1a`, offset 0) is the one where old and new shift agree. Making it
  exact would mean splitting `NoiseChannel`'s single countdown into prescaler +
  shift-stage counters, a save-state format bump, and re-calibrating the ten
  CH4 rows that pass today, for one row nothing passes. **Recommendation:
  declare it out of scope until someone has a real CGB to calibrate against.**
- **Caught by:** nothing but this ROM, and it is CGB-only (it reads PCM34).

---

## APU sweep — a trigger's shadow reload takes three M-cycles (2026-08-14)

Work item ② of the full-suite audit, CH1-sweep half. **SameSuite 68/76 → 69/76**
(its APU half 63/69 → 64/69), measured with
`./target/release/examples/samesuite target/test-suites/samesuite` on `main`
(`a5eb0af`, with the CH4 work of notes 14-16 already in) and on this branch —
both built from source in the same run, not quoted from a baseline file. Every
other suite is byte-identical across the change: Mooneye
`acceptance`+`emulator-only` 86/103 and `misc` 4/8, Mealybug 79 rows (both PPU
engines), `testroms` 46/47, `audiocapture` (`dmg_sound` 12/12, all hashes),
`savestate`, `battery` 8/8, `trainer` — all still "match the baseline".

**The other half of work item ② needed no work at all.** The audit listed four
NRx2-zombie rows (`channel_{1,2}_nrx2_glitch`, `channel_{1,2}_nrx2_speed_change`)
as failing and "partially implemented"; on current `main` all four **pass**, as
does `channel_1_sweep`. Finding 11 above is what fixed them. The audit's ② was
measured against `db8ba59` and that cluster closed before this branch started —
the CH1-sweep sub-cluster was 2 rows, not 3, and the NRx2 sub-cluster was 0.

### 17. The sweep shadow is not readable for three M-cycles after a trigger

- **Symptom:** a music driver that re-triggers channel 1 on a beat while a
  frequency sweep is running loses the note — the channel goes silent for the
  rest of the phrase — if the re-trigger happens to land within a few
  microseconds *before* a 128 Hz sweep event. On hardware the same note keeps
  playing. It is a lost note roughly twice a second in the worst case, and it
  only bites drivers that sweep *and* re-trigger, which is why it survives
  every other suite.
- **Caught by:** SameSuite `channel_1_sweep_restart_2` (FAIL → PASS). That ROM
  is *entirely* this window: it re-triggers CH1 at `$7FF` with NR10 = `$10`
  (period 1, add, shift 0), so the sweep's next value `$7FF + $7FF` overflows,
  and it walks the re-trigger from 0 to 7 M-cycles before a sweep event, reading
  NR52 0-15 M-cycles later. Hardware kills the channel for exactly **one** of
  the eight positions. **Nothing in Blargg catches it** — `dmg_sound` `04-sweep`
  and `05-sweep details` never re-trigger inside a sweep event's window.
- **Hardware:** a trigger reloads the sweep unit's shadow register from the
  frequency registers, and that reload is not instantaneous. For three M-cycles
  afterwards the overflow check reads the shadow as zero, so a sweep event
  landing inside the window latches no addend and the delayed check it arms
  compares `0 + 0`. The addend is also latched when the check is *armed*, not
  when it fires — which is why writing a different NR10 shift inside the window
  does not change the value the check adds (`channel_1_sweep_restart` rounds 4
  and 5 agree either way; both kill). Measured in this APU's clock phase: a
  sweep event 12 T-cycles after the trigger kills, one at 8 T does not
  (`SWEEP_RESTART_HOLD`).
- **New test:** `sound::tests::a_sweep_event_inside_the_restart_hold_cannot_kill_the_channel`.
  Teeth proven by removing the hold from both the addend latch and the shadow
  read — it goes red. Save-state `FORMAT_VERSION` 12 → 13 (the addend and the
  hold are both live machine state at a frame boundary).
- **Carries into the tool:** *a re-trigger inside a peripheral's own multi-cycle
  reload is a distinct state, not a shortcut to the post-reload state.* The
  general check is "drive the event one M-cycle at a time across the trigger and
  require the boundary to be in the right place", which is the shape
  `channel_1_sweep_restart_2` already has and which no framebuffer or
  register-dump adapter can see.

### 18. Gap left open: `channel_1_sweep_restart`'s last cell is a square-timing question

`channel_1_sweep_restart` now matches in **143 of its 144** cells. The one that
differs is round 1's last subtest (`SubTest $000f`), and it is worth writing
down precisely because the obvious reading of it — "the sweep is wrong" — is
false.

Round 1 sweeps `$7FF` down to `$7F0` in negate mode (which can never overflow,
so nothing is ever disabled), re-triggers the channel, and reads PCM12 at
0-15 M-cycles. Every cell must read `$00`: the first duty step after the
re-trigger has not happened yet. Ours reads `$08` at the last one. The
re-trigger's first duty step lands 66 T-cycles after the write and the read is
at 68; hardware's must be at 69 or later.

That is `SQUARE_TRIGGER_DELAY` / `SQUARE_RESTART_SPEEDUP`, not the sweep unit —
and both are already pinned by four rows that pass. Measured, not assumed:

| change | SameSuite | rows lost |
|---|---:|---|
| (as shipped) `DELAY = 6`, `SPEEDUP = 4` | **62/76** | — |
| `SPEEDUP = 0` (what round 1 wants) | 58/76 | `channel_{1,2}_restart`, `channel_{1,2}_restart_nrx2_glitch` |
| `DELAY = 5` | 56/76 | + more |
| `DELAY = 7` | 62/76 | no change (the grid rounding absorbs it) |
| `DELAY = 8` | 39/76 | most of the square-channel suite |

(That sweep was run before the CH4 work of notes 14-16 landed, so its absolute
column is 7 rows lower than today's; the *differences* are all in the square
channels and are unaffected by it.)

So round 1 wants the re-trigger reload to be one M-cycle *longer* than
`channel_1_restart` measures it, in a case where the two differ only in how far
through its period the frequency timer is (`delay = 62` of a 64 T period here,
`delay = 10` of a 16 T period in `channel_1_restart`). A correct fix is a
*conditional* re-trigger reload rule, and there is no evidence in the suite for
what that condition is. Recorded rather than guessed: one row, and guessing
costs four.
