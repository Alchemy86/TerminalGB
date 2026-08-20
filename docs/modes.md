# The three modes — `identical`, `standard` and speed mode

> **Short version.** `identical` is the default: exact to the dot, and 2.2 % of a
> frame for somebody playing a game. `standard` is the cheap engine — reach for it on
> small hardware, on a headless collector, or when you want speed mode, which
> `identical` refuses. **Speed mode** is faster still and takes one liberty — turn it
> on to *drive* the machine, never to judge it.

TerminalGB has two picture engines and one machine-timing liberty. They are separate axes and
this page is the one door to all three; [`picture-modes.md`](picture-modes.md) is the deep page
on the two engines.

| | **`identical`** | **`standard`** | **speed mode** |
|---|---|---|---|
| Default | **yes, since 2026-08-20** | no — opt in | no — opt in |
| What it is | the hardware pixel pipeline, dot by dot | a whole scanline drawn at the mode 3 → 0 transition | `standard`, with the peripherals advanced once per **instruction** instead of once per bus **access** |
| Cost, dmg-acid2, same box | 0.326 ms/frame (51× real time) | 0.038 ms/frame (444× real time) | **+27 % (cold boot) to +39 % (an overworld state) over `standard`**, and each shortcut is a separate switch — see 4a |
| Cost, Pokémon Blue, one P-core | 0.373 ms/frame (45× real time) | 0.094 ms/frame (178×) | — |
| …headless, no pixels wanted | 0.382 ms/frame (44×) — the skip does not apply | 0.057 ms/frame (295×) | — |
| Blargg `mem_timing` | pass | pass | **fails, on purpose** |
| Mid-scanline register writes | exact | not modelled | not modelled |
| Select it with | the default | `P`, `ppu = standard`, `$GB_PPU=standard` | `$GB_SPEED=speed`, `--speed-mode`, `Gameboy::set_speed_mode` (needs `standard`) |
| Use it to **judge** | ✅ anything | ✅ anything but mid-scanline timing | ❌ **never** |
| Use it to **drive** | ✅ (slow) | ✅ | ✅ |

---

## 1. What each one guarantees

**`identical`** guarantees the picture and the bus are what the hardware produces, to the dot.
It is what the accurate baselines are scored on and the only mode that can win Mealybug's
mid-scanline rows. Everything else is a departure from it, measured. **It became the default on
2026-08-20**, when the last row it lost to `standard` (`cgb-acid2`) closed: through the GB
Emulator Shootout's own 264-row manifest the two engines are 238 and 223, fifteen gains and no
losses. See [`picture-modes.md` §3](picture-modes.md#3-what-each-mode-scores).

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

> **Since `identical` became the default, `GB_SPEED=speed` does nothing until you also ask
> for `standard`.** Identical mode refuses every flag (§2's last paragraph), in either order,
> so a collector that wants the shortcuts now has to say
> `GB_PPU=standard GB_SPEED=speed`. Read
> [`Gameboy::speed_flags`](../src/gameboy.rs) for what is *in effect* rather than assuming it
> is what you asked for — that accessor exists for exactly this.

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
> certified upstream from a pool with a real attempt budget. **The hunt link
> has NOT yet been A/B'd on its own** — that run is the first thing queued, at
> 60 attempts a side, because a randomness-driven hunt is exactly where moving
> `rDIV`'s read phase would show if it showed anywhere.

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
| **`horizon-batch`** | advances the peripherals once per **horizon** — the distance to the next point at which any of them could do something the CPU can see — instead of once per instruction. **Includes `instruction-bus` by construction**: a horizon computed at the last flush is only valid while nothing advances a peripheral behind its back | *on top of what `instruction-bus` already costs*: nothing measured. On the 91-check GBSelfTest cartridge and on the 18-check conformance action it moves **exactly the same rows as `instruction-bus` alone and no others** (4b). What it *can* affect in principle is any observation not routed through a register: an OAM-DMA bus conflict raised mid-batch, or a VRAM/OAM access timed against a mode boundary finer than the PPU's own horizon | anything read back through a register. Every `$FE00-$FF7F` access, every interrupt dispatch, every halt, every save state and every frame boundary flushes the batch first, so `DIV`, `TIMA`, `LY`, `STAT` and `NR52` are exact **at the instant a program reads them** | **yes** | **+13 %** cold boot, **+21 %** overworld, on top of the preset it joins |
| **`no-oam-bug`** | skips the DMG OAM-corruption bug and its address-bus hook | `blargg/oam_bug`; any DMG-class program that moves a 16-bit register through `$FE00-$FEFF` while the PPU scans OAM | anything on a Colour console or an Advance, where the defect does not exist — a no-op there by construction | **no** — see below | **0.0 %** |
| **`no-apu-state`** | freezes the APU's state machines as well as its output | everything readable back from the sound chip: `NR52`'s channel-enable bits, the length counters, the sweep and envelope units, `PCM12`/`PCM34`. A game that *waits* on any of them does not go quiet, it **stops** | a program that never reads the sound registers back | **no** — see below | **+25.9 %** |

Measured on one P-core, thread CPU clock, Pokémon Blue, interleaved against a
binary built without the flag. `instruction-bus` and `no-apu-state` from a Route
1 state, 9,000 frames, best of three, load 1.3; `horizon-batch` best of eight
25,000-frame rounds at load 4.3-6.9, from a cold boot **and** from that same
state, because a short round measures the frequency governor rather than the
binary. **The preset as a whole is 0.0421 → 0.0331 ms/frame from a cold boot and
0.1152 → 0.0830 on the overworld state**, against the accurate machine.

**And what having the flag costs a machine that does not use it: about 2 %**
(−2.3 % cold boot, −1.6 % overworld, same method). That is the `$FE00-$FF7F`
test that empties the batch, inlined into every memory-accessing opcode along
with `read_at` and `wb`; `docs/measured/speed-ledger.md` §3b has the number and
the cheaper version that was not taken. On a quiet box (load 0.84, 12,000 frames,
three interleaved rounds) the preset reads **+9.2 %** and the preset plus
`no-apu-state` reads **+40.8 %**. All three leave the WRAM hash unchanged on
that workload.

### 4b. The horizon batch, certified

The bar is the captain's: **a speed flag may drop cycle accuracy; what it may
not do is move the Pokémon frames.** `horizon-batch` is measured against the
flag that was already in the preset, because that is the honest comparison — it
*includes* that liberty and the question is what it adds to it.

| what was compared | result |
|---|---|
| **92 Pokémon Blue save states**, 2,000 frames each, frame hash **and** work-RAM hash | **byte-identical to `instruction-bus` on all 92.** Zero differences. (Both differ from the accurate machine on 32 of the 92 — that is `instruction-bus`'s own, already-shipping property, not this flag's.) |
| **the Pewter chain, 26 links × 3 seeds** from a cold cartridge | **identical run-for-run**: same frames, same decisions, same ending, same coordinates on all 78 attempts. Only the wall clock differs. 26/26 links solved either way. |
| **GBSelfTest, 91 checks** on the cartridge that exists to detect this class | 82/91 with `instruction-bus`, **82/91 with the preset** — the same single loss, `GB-CYC-07`, which is the check written to detect that shortcut. **No additional row.** |
| **the conformance action, 18 checks** through the same adapter an external project calls | 15/18 either way. The three lost are `blargg/mem_timing`, `mem_timing-2` and `oam_bug`, and **all three are lost by `instruction-bus` alone**. |
| **`Gameboy::save_state` byte-for-byte**, on a fixture that reads `DIV`, `LY` and `STAT` into work RAM | identical; teeth proven by removing the flush from `read_at`, which makes it fail (`the_horizon_batch_is_invisible_to_a_cartridge_that_reads_its_peripherals`) |

**And the default did not move.** All 18 standard external baselines and all 15
accurate ones are byte-identical with no flag set — Gambatte's 5,320 rows in
each picture mode, GBMicrotest 513, Mooneye 103 + 8, SameSuite 76, Mealybug 79,
`cpp` 4 — plus `testroms` 46/47, `savestate`, `battery`, `audiocapture`,
`ppuengines`, `bootrom`, `gen1atlas` and 891 library tests.

**Why it is in the preset.** The bar is *safe **and** worth having*. It is
exactly as safe as the flag already there — that is what the table above says —
and it is worth about a fifth of the frame on top of it. Nothing in it is
believed rather than measured.

**What it is honestly NOT.** It is not lossless, and the difference from the
lazy PPU advance (`docs/throughput.md` §16, which is in the *default*) is the
whole reason this is a flag. What a batch can miss is any observation that does
not come through a register: an OAM-DMA bus conflict raised mid-batch, or a
VRAM/OAM access timed against a boundary finer than the PPU's own horizon.
Nothing we run measures those; something we have never run might.

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

**And with `horizon-batch` on top, the same ticks are applied once per
*horizon* rather than once per instruction.** The horizon is the distance to the
next point at which any peripheral could do something the CPU can see — the
PPU's next mode edge, the timer's next `TIMA` overflow, the APU's next output —
and the batch is refused outright while a VRAM DMA, an OAM DMA or a link
transfer is in flight. What makes it safe is not the horizon but the **flush
list**: every access in `$FE00-$FF7F`, every interrupt dispatch, every halt,
the OAM-corruption hook, a speed switch, a save state and the frame boundary
empty the batch before anything can observe it. So a program still reads `DIV`,
`TIMA`, `LY`, `STAT` and `NR52` exactly — which is why Gen 1's `Random_`, which
reads `rDIV`, draws the same numbers.

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
