# Accuracy — what it scores, and how you can check

← [Top-level README](../README.md) · [the yardstick](testing.md) · [the measured Game Boy](measured/README.md)

Every number on this page comes out of a baseline file this repository checks in, and every one
of those baselines is a CI gate that goes red **in either direction** — a row that starts
passing is a change too, and has to be re-blessed deliberately. That is the difference between
a gate and a scoreboard.

The ROMs are not vendored. They are fetched at run time from their authors' own upstreams at the
commits pinned in [`tools/fetch-test-suites.sh`](../tools/fetch-test-suites.sh), so a laptop, a
CI job and a published score all measure identical bytes. The one exception is
[dmg-acid2](../third_party/dmg-acid2/), which is MIT and ships here with its licence.

---

## The standing

Both [picture engines](picture-modes.md) are scored, each against its own baselines.
`identical` has been the default since 20 August 2026.

| Suite | `identical` (default) | `standard` | What it measures |
|---|---:|---:|---|
| **In-repo harness** (Blargg + dmg-acid2, both consoles) | <!--score:harness-->46 / 47<!--/score--> | <!--score:harness-->46 / 47<!--/score--> | instructions, timing, HALT, OAM corruption, the PPU's output |
| **Mooneye GB** `acceptance` | **<!--score:mooneye_acceptance_accurate-->75 / 75<!--/score-->** | <!--score:mooneye_acceptance-->67 / 75<!--/score--> | instruction and interrupt timing, the timer circuit, OAM DMA, PPU mode timing, boot state |
| **Mooneye GB** `emulator-only` | <!--score:mooneye_emulator_only_accurate-->28 / 28<!--/score--> | <!--score:mooneye_emulator_only-->28 / 28<!--/score--> | MBC1/2/5 banking |
| **Mooneye GB** `misc` (CGB) | <!--score:mooneye_cgb-->8 / 8<!--/score--> | <!--score:mooneye_cgb-->8 / 8<!--/score--> | Color-specific boot and timing |
| **SameSuite** | <!--score:samesuite-->73 / 76<!--/score--> | <!--score:samesuite-->73 / 76<!--/score--> | APU <!--score:samesuite_apu-->66 / 69<!--/score--> · DMA <!--score:samesuite_dma-->4 / 4<!--/score--> · PPU <!--score:samesuite_ppu-->1 / 1<!--/score--> · SGB <!--score:samesuite_sgb-->2 / 2<!--/score--> |
| **CasualPokePlayer** (`cpp`) | <!--score:cpp-->4 / 4<!--/score--> | <!--score:cpp-->4 / 4<!--/score--> | MBC3 RTC edge cases, and `sgb-ext-test` |
| **Gambatte** (c-sp) | **<!--score:csp_gambatte_accurate-->3622 / 5225<!--/score-->** | <!--score:csp_gambatte-->3176 / 5225<!--/score--> | the largest suite here — mid-scanline PPU, DMA, interrupt precedence |
| **GBMicrotest** (c-sp) | **<!--score:csp_gbmicrotest_accurate-->339 / 513<!--/score-->** | <!--score:csp_gbmicrotest-->252 / 513<!--/score--> | single-behaviour probes at a fixed cycle offset from reset |
| **Mealybug Tearoom** | **<!--score:mealybug_accurate-->29 / 79<!--/score-->** | <!--score:mealybug-->3 / 79<!--/score--> | mid-scanline PPU effects, against the author's own reference frames |

```bash
make fetch-test-suites
make test-external-accurate   # `identical`, the default engine
make test-external            # the same suites, `standard`
```

Thirteen more c-sp suites are gated beside those two and are listed on
[the scoreboard](measured/scoreboard.md), which gives a named reason for every red row.

---

## Nothing is judged against our own output

That rule is the reason to believe any of the table above. Every verdict belongs to somebody
else:

- **A serial string.** Blargg's ROMs print `Passed` or `Failed` to the link port.
- **The ROM's own printed report.** The ones that never touch the link port still say what they
  found — read out of cartridge RAM, or decoded off the screen, because Blargg's font loads so
  that a tile id *is* the ASCII code of the glyph it draws.
- **The author's reference image.** dmg-acid2 and Mealybug ship photographs and renders from
  hardware; we compare against those.
- **Fibonacci registers.** Mooneye signals a pass with a specific register state after `LD B,B`.

The harness once pinned four ROMs as hashes of our *own* framebuffer, and the documented
response to a changed hash was to re-bless it — so CI would have stayed green straight through
any regression in those four. Every expectation is somebody else's now.

The harness also proves it can fail, at startup: it decodes the reference image, requires it to
be 160×144 with more than one shade, requires it to compare equal to itself, and requires a
single flipped pixel to be *detected*. Both mechanisms were checked by deliberately breaking the
emulator — clearing the HALT-bug latch turns `halt_bug` red, and ignoring sprite X-flip turns
dmg-acid2 red at 119 of 23,040 pixels.

---

## The in-repo harness

`cargo test --release --test testroms` runs every ROM on **both consoles** — a DMG and a Game
Boy Color are different machines, and a boot-register difference alone changes a verdict. A row
is one ROM on one machine.

| Suite | Result |
|---|---|
| Blargg `cpu_instrs` (11) | 11/11, DMG and CGB — 22 rows |
| Blargg `instr_timing` | pass, DMG and CGB |
| Blargg `mem_timing` / `mem_timing-2` | pass, DMG and CGB — the timer is stepped per memory access |
| Blargg `halt_bug` | pass, DMG and CGB |
| Blargg `interrupt_time` | pass, DMG and CGB — dispatch measures the correct 13 cycles |
| `dmg-acid2` | **0 of 23,040 pixels differ** from the reference, DMG and CGB, on both engines |
| Blargg `dmg_sound` (12) | 12/12 |
| Blargg `cgb_sound` (12) | 12/12 |
| Blargg `oam_bug` | 8/8 sub-tests on DMG |

That is <!--score:harness-->46 / 47<!--/score--> rows. **The one red is `oam_bug` on a Game Boy
Color, and it has to stay red** — that silicon has no OAM-corruption defect, so a real Color
console fails the ROM too. Checked against SameBoy, which prints the identical sub-test-by-sub-test
report on a CGB and gates the defect the same way we do.

Per-ROM breakdown: [`testharness/README.md`](../testharness/README.md). How each verdict is
obtained: [`docs/testing.md`](testing.md).

---

## Mealybug, and why the denominator is not 79

Mealybug at <!--score:mealybug_accurate-->29 / 79<!--/score--> is the real remaining gap, and it
is the one number here most easily misread.

**45 of the 79 rows cannot be won by picture work at all.** 27 need a tile that only a real boot
ROM leaves in video RAM — a glyph out of somebody else's ROM, which this project will not ship
at any size. 8 compare our greyscale against a colour reference. 7 are a ROM/reference mismatch
inside the suite itself. So the honest denominator is 34, and
[`docs/mealybug.md` §11](mealybug.md) does the arithmetic.

**Quote the pixel distance, not the row count.** The baseline records how many of the 23,040
pixels differ on every row, so progress shows up even when no row flips. It is a *rank* distance:
a frame with one extra distinct shade reports ~23,040 whatever else is right, and a four-pixel
error can move the total by 60,000.

For scale, from the GB Emulator Shootout's own published per-test data on the 24 DMG Mealybug
rows: SameBoy 15/24, **us on `identical` 11/24**, bgb 2/24, us on `standard` 1/24, PyBoy 0/24. This is
not a suite where a competent cycle-stepped PPU scores 24 — it is the suite that separates the
two or three most accurate emulators in the world from everyone else, and the top scorer on the
board still fails nine rows.

---

## The GB Emulator Shootout

The [GB Emulator Shootout](https://gbdev.io/GBEmulatorShootout/) is the community's public
board: 264 ROMs, 18 emulators, every verdict a screenshot comparison. Scored against **its**
ROMs, **its** reference images and **its** pass criterion:

> **243 / 264 — 92.0 %**, 5th of 19 by pass count: behind docboy (258), Emulicious (254),
> SameBoy (253) and Beaten Dying Moon (247); ahead of GSE (218), bgb (205), GambatteSpeedrun
> (169) and mGBA (150).

Fifteen of those came from making `identical` the default on 20 August 2026 — fifteen gains,
zero losses, 223 → 238. The five rows since are later `identical`-only picture work.

All 21 failures are documented open areas, and 9 of them are Mealybug rows nobody can win by
picture work. Re-derive the number in under a minute — no Windows, no display:

```bash
git clone https://github.com/gbdev/GBEmulatorShootout
python3 tools/shootout/manifest.py GBEmulatorShootout > manifest.tsv
cargo run --release --features image --example shootout_scout -- manifest.tsv
```

This figure is **not** derived from a checked-in baseline, so it is not gated the way the table
above is; it is re-measured through that command. We are not listed on the board yet —
[`docs/shootout.md`](shootout.md) is the prepared submission.

---

## Nine consoles, because a boot ROM is part of the silicon

The emulator picks the console from the cartridge header: a CGB flag (`0x0143` bit 7) boots a
Game Boy Color, anything else boots a DMG — what a person with both on the shelf would do.
`GB_MODEL` overrides it, and every frontend honours it.

```bash
GB_MODEL=dmg  cargo run --release --example desktop -- game.gbc   # force an original Game Boy
GB_MODEL=sgb  cargo run --release --example desktop -- game.gb    # a Super Game Boy
GB_MODEL=agb  cargo run --release --example desktop -- game.gbc   # a Game Boy Advance
```

The full list is `auto`, `dmg`, `dmg0`, `mgb`, `cgb`, `cgb0`, `agb`, `sgb`, `sgb2`. A DMG 0, a
Game Boy Pocket, a CGB 0 and a Game Boy Advance present the same hardware to a cartridge from
`PC=$0100` onwards and hand it a different machine to start from: `A = $FF` is how a game detects
a Pocket, and `B` bit 0 is how it detects an Advance and offers the brighter palette that screen
needs. Mooneye ships a `boot_regs` / `boot_div` / `boot_hwio` ROM per machine, no two of which can
pass at once. All eleven are green.

A typo warns and is ignored rather than silently booting the wrong console, and **`auto` never
picks a revision** — inferring one would change every game's boot registers for no benefit.

Before this existed *everything* booted a Color, so a DMG-only game always ran in the Color's
compatibility mode — a different machine, with different boot registers (`A=0x11 HL=0x007C`
rather than `A=0x01 HL=0x014D`) and the undocumented CGB registers still responding. Nine Mooneye
acceptance tests print those registers and can tell.

### What the Super Game Boy support actually scores

All three of the shootout's published SGB rows pass, **0 of 23,040 pixels different** from their
authors' references: `cpp/sgb-ext-test`, `samesuite/sgb/command_mlt_req` and
`samesuite/sgb/command_mlt_req_1_incrementing`. Of the eighteen emulators in the shootout's own
table, only GSE passes all three — SameBoy passes one, bgb one, mGBA two, and six decline the SGB
model entirely.

**Read that precisely, because it is narrower than it sounds.** All three of those ROMs are plain
160×144 DMG-grey screens. No border, no palette, no SNES side is involved in any of them, because
each measures the same thing: the ICD2 command-packet receiver and the `MLT_REQ` joypad index,
which is the *only* part of an SGB a Game Boy program can read back. Everything else an SGB does
is one-way traffic to a screen the cartridge cannot see, so no test ROM can assert it and this
emulator has nowhere to put it. The border, the SGB palettes and SGB sound are a documented
non-goal. Full reasoning and the packet protocol:
[`docs/components/sgb.md`](components/sgb.md).

A Super Game Boy is never chosen automatically. A cartridge's SGB flag (`0x0146 == 0x03`) says the
game *offers* a border, not that you own the peripheral.

### CGB CPU revision — a conformance knob, not a game setting

The Color shipped in several CPU steppings and its APU changed between them: on CPU CGB 0 and B,
writing `NRx4` with the length counter previously disabled clocks that counter even when the write
leaves it disabled. That is a silicon bug — it breaks *Prehistorik Man* — and Nintendo fixed it in
CPU CGB C. The emulator models CPU CGB C and later, which is what every later console and every
DMG does.

```bash
GB_CGB_REVISION=cgb0 …   # or cgbB — reintroduce the pre-C length-clocking bug
```

It exists because SameSuite ships five ROMs that name a revision in their *filename*, and `-cgb0`
and `-cgbB` contradict each other, so no single machine can pass both. The runner reads the
revision off the name the way it reads `@dmg`/`@cgb` off a header. A DMG ignores it entirely.

### Multi-game compilation cartridges (MBC1M)

Carts like *Bomberman Collection* or *Mortal Kombat I & II* hold several games behind one menu and
wire their MBC1 differently — and they cannot be told apart from an ordinary cartridge by the
header, so every emulator guesses. We treat a cartridge as a multicart when it is exactly 1 MiB
and carries a second Nintendo header in bank `$10`, which is the identification Pan Docs
documents. `GB_MBC1_MULTICART=1` forces multicart wiring and `=0` forces the ordinary wiring.

---

## Known limitations

Each is documented on the relevant component page.

- **No boot ROM, no logo scroll.** The post-boot state is written directly, per console
  *revision* — and, for the system counter on a Color console, per *cartridge header* as well,
  since the CGB boot ROM runs a different program for a CGB-flagged cartridge than for a DMG one.
  All eleven Mooneye `boot_*` rows pass, and gambatte's `div/start_inc_{1,2}_cgb04c` pin the
  Color's other path. What is not reproduced is the logo animation, the header-checksum lock-up,
  and the boot beep — its envelope volume is not readable from any register, so how far it has
  decayed cannot be derived. You can [supply your own dumped boot
  ROM](picture-modes.md); with one, Mealybug's whole-suite pixel distance falls
  693,023 → 483,005.
- **Mode 3 is fixed-length in `standard`.** `identical` derives it instead and is structurally
  right but not fully calibrated — the sprite penalty, the window's WX penalty and the exact dot a
  CPU write becomes visible are the open work. This is what Mealybug is waiting on.
- **`identical` is not strictly dominant.** It does not lose a row to `standard` anywhere the
  shootout looks, but across all 6,210 gated rows four small Gambatte groups are net-negative on
  it (`sprites` −3, `lyc153int_m2irq` −2, `lycint_lycflag` −2, `m2int_m0stat` −1) — eight rows
  between them, against 891 gains.
- **The DMG OAM-corruption bug is modelled**, as is the HALT bug. In `standard` the PPU is stepped
  once per instruction, so which OAM row a mid-instruction bus cycle hits is predicted
  arithmetically rather than observed; one calibrated constant absorbs the phase the batched PPU
  picks up when the LCD is switched on mid-instruction.
- **Battery saves need a filesystem.** The web and FFI builds have none, so in-game saves are not
  persisted there. That is the *only* thing the save path decides — the mapper comes from
  cartridge header byte `$0147` on every path.

---

## Saves

**Battery saves go to `<rom>.sav`** — raw cartridge RAM with no header, the format every other
emulator uses and the one real-cartridge dumpers produce, so saves carry in either direction. For
MBC3 cartridges with a real-time clock, the clock is appended to the same file as the standard
48-byte footer.

That clock runs on **emulated** time — one second per second of Game Boy time, so it speeds up
when you fast-forward and stops when you pause, exactly as the console's own clock would — and
gains the real elapsed time while the emulator is closed, as the cartridge's coin cell does. It
passes all three of ax6's [`rtc3test`](https://github.com/aaaaaa123456789/rtc3test) suites.

`.srm` files are read too, as are saves from older builds of this emulator (`<rom>.gbsave`), which
are migrated automatically with the old file left in place as a backup.

Saves are written **while the emulator runs** — about a second after the game finishes writing
cartridge RAM — and atomically, so closing the window, a crash or a `kill` cannot lose or truncate
them. Nothing is written in a destructor, because a destructor is not a shutdown hook: the desktop
event loop never returns, and release builds abort rather than unwind.

**Save states** are a separate, versioned `GBSS` format that checks the cartridge it is loaded
against — [`docs/savestates.md`](savestates.md).

---

## The gates

Everything in this table runs in CI on every push and pull request.

| command | what it proves |
|---|---|
| `cargo test --release --test testroms` | Blargg + dmg-acid2, every ROM on both consoles, judged by the ROM's own verdict |
| `cargo test --release --test ppuengines` | the two picture modes against each other, against dmg-acid2's reference, and against the clock |
| `cargo test --release --test savestate` | byte-identical save-state round trip, every frame, plus the negative cases |
| `cargo test --release --test battery` | an in-game save reaches disk **while the emulator runs** — no destructor involved |
| `cargo test --release --test audiocapture` | the APU's sample stream with no audio device: rate, pitch, level, continuity, and Blargg `dmg_sound`'s own verdict |
| `cargo test --release --test blackout` | every host that drives a real cartridge installs a sound chip — a machine without one hangs Gen 1's whiteout |
| `cargo test --release --test basicterm` | the frontend runs, draws, takes input and quits in a terminal with **no** kitty keyboard protocol |
| `cargo test --release --test agent` | the agent plugin's budget cap, circuit breaker and stuck detection, against a hand-assembled cartridge |
| `make test-audio-perf` | emulation stays inside the frame budget with a host audio player attached (needs `audio`, **not** a sound card) |
| `make test-external` / `make test-external-accurate` | Mooneye + SameSuite + Mealybug + the c-sp suites, both picture modes, against pinned baselines |
| `make test-cross` | the core built **and run** on aarch64, armv7 and i686, requiring a byte-identical frame and save-state length |
| `make embed` | the core alone: one third-party dependency, and a ROM runs with `HOME` and `DISPLAY` unset |
| `make no-plugins` | no plugin crate is reachable from the emulator or either frontend |
| `make gbselftest` | [GBSelfTest](gbselftest.md)'s 102 checks, both picture modes, against a per-mode list of known gaps |
| `make docs-check` | every generated `path:line` doc link still points at a real line, and every score in prose still matches its baseline |

Re-bless a baseline deliberately, never to make a number look better:

```bash
UPDATE_EXTERNAL_BASELINE=1 make test-external
```

---

## Scoring *your* emulator

The same machinery runs against somebody else's emulator, as a reusable GitHub Action. Add one
job:

```yaml
jobs:
  conformance:
    uses: Alchemy86/TerminalGB/.github/workflows/conformance.yml@main
    with:
      build: cargo build --release          # optional: build your emulator first
      adapter: ./ci/gbconform-adapter.sh    # the only thing you have to write
```

Anyone can run a test ROM. The point of this one is the **why**: a failure comes back as *"`POP
AF` must mask the low nibble of F — bits 0-3 of the flag register do not exist on the chip"*, with
the hardware citation attached, drawn from the same per-fix notes this project has been keeping
all along.

**Your emulator only has to do two things**, and not even both — write the bytes a ROM sends to
the link port, and/or write one frame. No API, no protocol, nothing parsed from stdout. The
adapter is usually four lines of shell over a headless mode you already have:

```sh
#!/bin/sh
[ -n "${GBCONFORM_FRAME_OUT:-}" ] || exit 0     # decline the capability you lack
exec my-emulator --headless --frames "$GBCONFORM_FRAMES" \
     --screenshot "$GBCONFORM_FRAME_OUT" "$GBCONFORM_ROM"
```

| | |
|---|---|
| **Runs today** | Blargg `cpu_instrs` 01–11, `instr_timing`, `mem_timing` (serial) + `dmg-acid2` (screen) + `mem_timing-2`, `halt_bug`, `oam_bug`, `interrupt_time` (the ROM's own printed report, read off the frame) — 18 checks |
| **Badge** | shields.io endpoint JSON **and** a self-contained SVG; `conformance 24/26`, not pass/fail |
| **Report** | `results.json`, a Markdown summary in the job page, captured serial text, actual + diff frames |
| **Cost** | Python standard library only — no `pip install`, no toolchain, seconds to run |
| **ROMs** | dmg-acid2 is MIT and vendored here; Blargg's are fetched from their own upstream at a pinned commit, never redistributed by us |
| **Proven on** | ourselves (18/18, enforced in CI) and on two unrelated emulators, unmodified — [binjgb](https://github.com/binji/binjgb) (screen only, 4/5) and [Peanut-GB](https://github.com/deltabeard/Peanut-GB) (both capabilities, 13/18) |

Screen checks compare **shade rank, not colour**, so your palette can be green, grey or anything
else. Checks needing a capability your adapter does not have are reported *skipped* and excluded
from the score, rather than failed.

📖 **[The full contract, adapter examples, badge recipes and the licensing position →
`docs/conformance-action.md`](conformance-action.md)**

---

## Where to go next

| page | what it is |
|---|---|
| [where it stands](status.md) | the same standing as a field report, with the abilities and the roadmap |
| [the scoreboard](measured/scoreboard.md) | every suite, group by group, with a named reason for every red row |
| [the accuracy yardstick](testing.md) | which suites exist, how each is judged, and how the harness detects a verdict |
| [the measured Game Boy](measured/README.md) | "Pan Docs says X — here is what we measured, and what a wrong emulator does" |
| [picture modes](picture-modes.md) | `standard` vs `identical`: what each draws, what each costs, how to switch |
| [Mealybug, measured](mealybug.md) | the calibration argument and the denominator |
| [per-fix conformance notes](conformance-notes.md) | every accuracy bug fixed, with symptom, catching test and hardware citation |
| [GBSelfTest](gbselftest.md) | a cartridge that tests the machine it runs on and prints a code you can look up |
| [`testharness/`](../testharness/README.md) | the per-ROM scoreboard and how each verdict is obtained |
