# The three modes — `identical`, `standard` and speed mode

> **Short version.** `standard` is the default and is what you should be running.
> `identical` is slower and exact to the dot — turn it on to *judge* something.
> **Speed mode** is faster and takes one liberty — turn it on to *drive* the machine,
> never to judge it.

TerminalGB has two picture engines and one machine-timing liberty. They are separate axes and
this page is the one door to all three; [`picture-modes.md`](picture-modes.md) is the deep page
on the two engines.

| | **`identical`** | **`standard`** | **speed mode** |
|---|---|---|---|
| Default | no — opt in | **yes** | no — opt in |
| What it is | the hardware pixel pipeline, dot by dot | a whole scanline drawn at the mode 3 → 0 transition | `standard`, with the peripherals advanced once per **instruction** instead of once per bus **access** |
| Cost, same ROM and box | 0.326 ms/frame (51× real time) | 0.038 ms/frame (444× real time) | **+9.3 % over `standard`** as shipped, and each shortcut is a separate switch — see 4a |
| Blargg `mem_timing` | pass | pass | **fails, on purpose** |
| Mid-scanline register writes | exact | not modelled | not modelled |
| Select it with | `P`, `ppu = identical`, `$GB_PPU=identical` | the default | `$GB_SPEED=speed`, `--speed-mode`, `Gameboy::set_speed_mode` |
| Use it to **judge** | ✅ anything | ✅ anything but mid-scanline timing | ❌ **never** |
| Use it to **drive** | ✅ (slow) | ✅ | ✅ |

---

## 1. What each one guarantees

**`identical`** guarantees the picture and the bus are what the hardware produces, to the dot.
It is what the accurate baselines are scored on and the only mode that can win Mealybug's
mid-scanline rows. Everything else is a departure from it, measured.

**`standard`** guarantees every peripheral is on its exact cycle and the picture is right for
any program that does not rewrite a PPU register part-way along a scanline. It passes the whole
in-repo harness, all of Mooneye, and Blargg `mem_timing`. **Almost every commercial game is
byte-identical between `standard` and `identical`** — proved frame for frame, not assumed.

**Speed mode** guarantees the peripherals run at the *right rate* and drops *where inside an
instruction* an access lands. `DIV`, `TIMA`, the serial shift clock, the APU frame sequencer and
OAM DMA all still advance exactly as many cycles per instruction as before — the ticks it skips
at each access are applied at the instruction's end instead. What is lost is the sub-instruction
M-cycle phase, which is exactly what Blargg `mem_timing` measures, and it fails there on purpose.

---

## 2. What speed mode is NOT safe for

**Do not use it to judge anything.** Concretely, never turn it on for:

- **any test-ROM verdict** — `mem_timing` and `mem_timing-2` fail by design, and any suite whose
  ROM measures sub-instruction access timing will report a wrong answer with total confidence;
- **any picture comparison** — Mealybug, dmg-acid2, cgb-acid2, the Gambatte screenshots, or
  anything else scored against a reference image;
- **any audio capture or hash** — `testharness/audiocapture.rs` and the audio baselines;
- **a published conformance or shootout score**, of ours or anybody's.

**None of the gates can accidentally use it.** It is off by default, no baseline runner sets it,
and `make test-external` / `make test-external-accurate` / `cargo test` never see it. It has to
be typed.

**`identical` refuses it outright.** Identical mode's whole claim is per-dot timing driven by
per-access stepping, so the two are contradictory rather than merely unwise:
`Gameboy::set_speed_mode(true)` under `RenderMode::Accurate` leaves it off, and switching *to*
identical mode turns it off, so the refusal cannot be bypassed by ordering. Ask
`Gameboy::speed_mode()` for what actually happened rather than for what you asked.

---

## 3. What it IS for

Anywhere the emulator is being driven rather than judged, and the observation comes out of
memory or out of the scenario's own verdict:

- certification runs and corpus collection (`pewter_chain corpus`, `chain`, `solve`)
- swarm films and scenario work
- the A/B test rig
- shiny hunting and any other search over many runs

These already declare `ScenarioRun::collecting()`, which switches the pixel path and the audio
resample off. Speed mode is the next declaration along the same line: *nobody is judging this
machine's timing, only its outcome.*

---

## 4. Why this is safe, and it is measured rather than argued

We have a certification harness, so "is this shortcut acceptable?" does not have to be an
argument about what a cartridge can observe. **Run the certified links under it and see whether
they still certify.**

<!--speedmode:begin-->
**The chain from a cold cartridge to Mt Moon B2F, 24 links × 3 seeds: 72 of 72
goals with every shortcut off, and 72 of 72 with speed mode on.** Zero failures
either way.

> **Why 24 links and not the whole 33.** The bench work added
> `23b-hunt-a-paras`, and a PARAS is about one step in 128 — so three attempts
> is not enough to hunt one and `chain --seeds 3` stops there **in both modes
> alike**. That is a property of the link, not of the shortcut; the link is
> certified upstream from a pool with a real attempt budget. It is A/B'd
> separately below, at 60 attempts a side, because a randomness-driven hunt is
> exactly where moving `rDIV`'s read phase would show if it showed anywhere.

**Per link, 200 attempts each from the same entry state and the same seeds,**
on the three links where a shortcut is most likely to show — two gym battles and
the rival fight, all of them dense in the timer-derived randomness Gen 1 uses:

| link | accurate | speed mode |
|---|---:|---:|
| 05 the rival battle | 200/200 | 200/200 |
| 17 Pewter gym | 200/200 | 200/200 |
| 27 train for Misty | 200/200 | 200/200 |
| 29 Cerulean gym | 200/200 | 199/200 |
| **total** | **800/800** | **799/800** |

**The trajectories are not identical and are not expected to be.** Gen 1's
`Random_` mixes `rDIV` in on every call, so moving *where inside an instruction*
that register is read changes the value it returns — the same way a different
seed does. What is being claimed is that the links still certify, not that the
runs are the same runs. Link 29's single miss is inside its own certified rate
for that starter line (99.0–99.7 %, `docs/agent/misty.md`), and the link is
`accepted, not certified` in any case.
<!--speedmode:end-->

Reproduce it:

```bash
pewter_chain chain --rom <blue.gb> --seeds 3 --to 30 --out target/a
GB_SPEED=speed pewter_chain chain --rom <blue.gb> --seeds 3 --to 30 --out target/b
```

and compare the per-link frame counts, decision counts and endings. That is a far stronger
statement than any reasoning about observability, and it generalises: point it at a different
scenario and it answers the question again for that workload.

---

## 4a. Every flag, and what each one affects

**Speed mode is a preset, not a code path.** There is one implementation; the
preset turns a set of named flags on, and its contents are auditable in
`src/speed.rs` (and pinned by a test, so it cannot quietly grow).

| flag | what it actually does | what it affects | what it does **not** affect | in the preset | measured |
|---|---|---|---|---|---:|
| **`instruction-bus`** | peripherals advance once per CPU instruction instead of once per bus access | *which sub-instruction M-cycle an access lands on.* Blargg `mem_timing`, `mem_timing-2`, `dmg_sound` 09/10/12 (the DMG wave-RAM window, timed to 2 T-cycles). GBSelfTest `GB-CYC-07` detects it by design | every peripheral's **rate** (`DIV`, `TIMA`, the serial shift clock, the APU frame sequencer, OAM DMA); the picture; interrupt dispatch, through which the PPU still steps; OAM/VRAM access blocking; the OAM-corruption bug; OAM DMA bus conflicts | **yes** | **+9.3 %** |
| **`no-oam-bug`** | skips the DMG OAM-corruption bug and its address-bus hook | `blargg/oam_bug`; any DMG-class program that moves a 16-bit register through `$FE00-$FEFF` while the PPU scans OAM | anything on a Colour console or an Advance, where the defect does not exist — a no-op there by construction | **no** — see below | **0.0 %** |
| **`no-apu-state`** | freezes the APU's state machines as well as its output | everything readable back from the sound chip: `NR52`'s channel-enable bits, the length counters, the sweep and envelope units, `PCM12`/`PCM34`. A game that *waits* on any of them does not go quiet, it **stops** | a program that never reads the sound registers back | **no** — see below | **+25.9 %** |

Measured on one P-core, Pokémon Blue from a Route 1 state, 9,000 frames,
best of three interleaved, load 1.3. On a quiet box (load 0.84, 12,000 frames,
three interleaved rounds) the preset reads **+9.2 %** and the preset plus
`no-apu-state` reads **+40.8 %**. All three leave the WRAM hash unchanged on
that workload.

**Why `no-oam-bug` is not in the preset, despite being safe.** It buys **nothing
measurable** — the hook only fires when a 16-bit register points into
`$FE00-$FEFF`, which this workload never does. The bar for the preset is *safe
**and** worth having*; trading real accuracy for zero is not a trade. It ships
as a flag because a DMG game that does hammer OAM might make it worth something,
and then the user can say so.

**Why `no-apu-state` is not in the preset, despite certifying.** It is the
largest single gain on the table and it passes our chain — but it reaches far
wider than our workload. Freezing a whole peripheral is exactly the class of
change we cannot vouch for across games we never run: any sound driver that
spins on a length counter or on `NR52` hangs outright. The evidence says it is
safe *for Pokémon Blue on the Pewter chain*, and that is what the flag's
documentation says. Turn it on deliberately, for a workload you have checked.

**Prefer `Gameboy::set_render_audio(false)` first.** It skips the band-limited
resample and leaves every one of those registers exact, so it costs nothing in
accuracy at all. Every headless front end here already asks for it.

### Combining them

```
GB_SPEED=speed                     # the preset
GB_SPEED=speed,-instruction-bus    # the preset, minus one shortcut
GB_SPEED=speed,no-apu-state        # the preset, plus one
GB_SPEED=no-oam-bug                # one shortcut, no preset
```

**Last wins**, so an explicit flag beats the preset in both directions whichever
order they appear. An unrecognised name is reported and ignored, never guessed
at. `SpeedFlags::spec()` prints what is actually on, which is what to log rather
than what was asked for.

---

## 5. The liberty, precisely

`MemoryManagementUnit::bus_tick` normally advances the timer, the serial shift clock, the APU
and OAM DMA by one M-cycle **before every CPU memory access**, so a read of `DIV` or a write to
wave RAM lands on the exact cycle the program put it on. That is what makes Blargg `mem_timing`,
`mem_timing-2` and `dmg_sound` 09/10/12 pass, and it costs about 9 % of a headless frame.

Speed mode leaves `bus_ticked` at zero, so `MemoryManagementUnit::do_cycle` applies the whole
instruction's tick count at its end. **The totals are identical; only the phase is lost.**

Two things it deliberately does **not** skip, both load-bearing:

- **the PPU's per-access step** — for `identical` that step *is* the timing, and for `standard`
  it is the five M-cycles of interrupt dispatch, during which the CPU decides its own vector
  from `IE & IF` at M4. A PPU frozen through a dispatch cannot redirect it, and
  `mealybug/m2_win_en_toggle` photographs the difference;
- **`bus_tick_enabled` itself**, which is the "a CPU instruction is running" predicate and also
  gates VRAM/OAM access blocking, the DMG OAM-corruption bug and OAM DMA bus conflicts. Those
  are unchanged.

That distinction matters and is not cosmetic. The cruder shortcut — return from `bus_tick`
immediately, which is what `profile::Switches::no_bus_tick` does for measurement — **also**
skips the dispatch stepping, and on Pokémon Blue it changes the cartridge's own work RAM
(`core_bench --breakdown` reports `wram MOVED`). Speed mode as built does not: the WRAM hash is
byte-identical to `standard` on the same state. **Do not conflate the two.**

---

## 6. Selecting a mode, everywhere it can be named

| where | picture engine | speed mode |
|---|---|---|
| terminal frontend | `P`, or `ppu = standard\|identical` in `config.toml` | not exposed — it is not a thing to play a game with |
| environment | `$GB_PPU=standard\|identical` | `$GB_SPEED=speed` (and see 4a) |
| `core_bench` | `$GB_PPU` | `--speed-mode`, or `--speed <spec>` |
| scenarios / chain / corpus | `$GB_PPU` | `$GB_SPEED=speed`, or `ScenarioRun::speed_flags(..)` |
| conformance adapter | `$GB_PPU` | `$GB_SPEED` — so a check that claims to detect a shortcut can be *shown* to |
| library | `Gameboy::set_render_mode` | `Gameboy::set_speed_flags`, or `set_speed_mode` for the whole preset |

Speed mode is deliberately **not** on a key in the frontend. A person playing a game gains
nothing from it — the frontend is paced to 59.7275 Hz either way — and a mode that fails
`mem_timing` should not be one keystroke away from someone judging a picture.

---

## 7. Further reading

- [`picture-modes.md`](picture-modes.md) — the two engines in depth, and what each scores
- [`measured/throughput-baseline.md`](measured/throughput-baseline.md) — the headless numbers and the method
- [`throughput.md`](throughput.md) — every optimisation, and every measured negative
- [`measured/against-the-field.md`](measured/against-the-field.md) — SameBoy and PyBoy, built and timed here
