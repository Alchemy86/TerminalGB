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
