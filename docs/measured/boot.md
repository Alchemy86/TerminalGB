# Boot state — the Game Boy is nine machines, not four

← [The measured Game Boy](README.md)

We do not run a boot ROM by default, so **every byte of the state one leaves behind at
`PC = $0100` has to be written by hand** — and most of it was not being written at all. Eleven
Mooneye rows turned green when it was, and ten of the eleven had one cause: *we only modelled
four of the nine consoles.*

Hardware reference, not restated here:
[Power-Up Sequence](https://gbdev.io/pandocs/Power_Up_Sequence.html) ·
[The Cartridge Header](https://gbdev.io/pandocs/The_Cartridge_Header.html). The authority for
the *per-console* tables is not Pan Docs but the Mooneye ROMs themselves —
`acceptance/boot_hwio-dmgABCmgb.s` and `misc/boot_hwio-C.s` hold the whole `$FF00-$FF7F` map,
and they were verified by their author on real DMG, MGB and CGB units.

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`6c20451`](https://github.com/Alchemy86/TerminalGB/tree/6c20451306663018288d34253c199fad507ae6f6) · regenerate with `make docs-links`, verify with `make docs-check`.

## A boot ROM is part of the silicon, so a revision is a machine

Mooneye ships a `boot_regs` / `boot_div` / `boot_hwio` ROM **per console**, and no two consoles
hand the cartridge the same state — so passing one is failing the others.

`Model` ([`src/mode.rs:75`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mode.rs#L75)) now carries nine values:

```
auto / dmg / dmg0 / mgb / cgb / cgb0 / agb / sgb / sgb2
```

**The revisions are boot state only, and are deliberately not in `GbMode`.** From `PC = $0100` a
DMG 0 and a DMG are identical silicon, so `GbMode` still has five values, the save-state format
did not move, and the console is **not serialised** — it is a property of the machine a state
is loaded *into*.

**`Model::Auto` must never infer a revision** (`auto_never_infers_a_revision`). A pre-release
DMG, a Pocket or an AGB is something you ask for by name (`GB_MODEL=dmg0|mgb|cgb0|agb`);
inferring one would silently change every game's boot registers.

### The system counter does not start at zero

The single most consequential number here, because **games seed their RNG from `DIV`**. From a
zeroed counter the seed is the same every launch, and it is a value hardware never presents.

| Console | System counter at hand-off |
|---|---|
| DMG / MGB | `$ABCC` |
| DMG 0 | `$1830` |
| CGB / CGB E, DMG-header cartridge | `$2678` |
| CGB / CGB E, CGB-flagged cartridge | `$1EA0` |
| CGB 0 | `$2884` |
| AGB | `$267C` |
| SGB / SGB 2 | `$D174` **+ 4 × (zero bits in the cartridge header)** — see below |

Constants at [`src/mmu/mod.rs:207`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/mod.rs#L207) onwards.

**The CGB row is split because a Color console runs two different boot programs.** A
DMG-header cartridge takes the compatibility path (`$2678`, Mooneye `misc/boot_div-cgbABCDE`,
whose own header is DMG); a CGB-flagged one does not (`$1EA0`, derived from gambatte's
`div/start_inc_{1,2}_cgb04c`, which are `$0143 = $C0` and pin the wrap M-cycle as well as the
value). **`CGB 0` and `AGB` are NOT split, and that is a known gap rather than a measurement:**
their figures come from `misc/boot_div-cgb0` and `misc/boot_div-A`, whose cartridge headers are
DMG, so both are compatibility-path numbers being used for genuine Color cartridges too. No
suite ROM measures either console on the genuine path, so there is nothing to derive a
corrected value from — unlike the CGB case, where gambatte's hwtests supplied exactly that.

**`boot_div` is much stronger than "`DIV` reads `$AB`".** It reads `DIV` six times at
instruction offsets chosen to fall either side of an increment, so the counter's **low bits**
are pinned too. `Timer::set_boot_counter` ([`src/mmu/timer.rs:116`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/timer.rs#L116))
stores the value 4 T lower than the hardware number, because the bus advances the counter
*before* the access that observes it — the constant in the source stays the hardware figure
and the adjustment happens in exactly one place.

### An SGB's boot ROM does not take a fixed time

`acceptance/boot_div2-S` exists to catch you hard-coding one.

The SGB boot ROM clocks the 76-byte cartridge header to the SNES as six 16-byte packets (id +
checksum + data, eight bytes of zero padding = 96 bytes, **768 bits**) through an
**unbalanced** bit loop: a set bit takes a taken `jr`; a clear bit falls through *and* runs a
second `ld a`. So **a zero bit costs one M-cycle more**, and the boot time is a function of the
cartridge in the slot.

The disassembly says so in its own comment — *"Both code paths aren't equal CPU time …
NINTENDOOOOO"* ([ISSOtm/gb-bootroms](https://codeberg.org/ISSOtm/gb-bootroms), `src/sgb.asm`).
`sgb_boot_system_counter` ([`src/mmu/mod.rs:147`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/mod.rs#L147)) is that model, and the
two ROMs independently give the same all-ones base `$D174` — which is what says the *shape* is
right rather than merely fitted.

### Things two consoles genuinely disagree about

| | DMG | CGB |
|---|---|---|
| `P1` (joypad) | `$CF` — both select lines pulled low | `$FF` — released |
| `IF` | `$E1` | `$E1` (the boot ROM's VBlank is still pending on both) |
| `A` / `HL` | `$01` / `$014D` | `$11` / `$007C` |

Two more that are one instruction each, and are how a cartridge *detects* its host:
**`A = $FF` identifies an MGB**, and **`B` bit 0 identifies an AGB** — the AGB boot ROM ends
with an `inc b`, which is both why `B` holds `$01` and why `F` reads `$00` instead of the CGB's
`Z`.

**`P1 = $CF` is a `GbMode::Classic` fact, not an "is DMG-class" one.** An SGB's last use of the
port is clocking packets, ending on `$30`, so it leaves the lines **released** like a CGB. Its
last audio write is `NR14 = $07`, not `$87` — channel 1 is set up and never triggered, so
**`NR52` reads `$F0` and an SGB has no start-up ding.**

**DMG 0's boot ROM is a different program, not a variation.** It has no header-checksum
lock-up, so it never runs the code that leaves the registers tidy (`F=$00 B=$FF DE=$00C1
HL=$8403`), hands over with the counter nearly `$9400` lower, and — because it skips
`CheckLogo` — leaves the **PPU nine scanlines earlier**, still in VBlank.

---

## Three things a hand-written boot state gets wrong

Each was a real bug here, each is a trap for any emulator doing the same.

### 1. Do not write the *read-back* values into `NR11` / `NR14`

They read `$BF`, but that is `$80` and `$87` ORed with their write-only bits. Writing `$BF`
loads a length counter of **1 with the length counter enabled**, so channel 1 switches itself
off within 4 ms and `NR52` reads `$F0` — while `boot_hwio` reads `NR52` about 2.8 ms in and
requires `$F1`.

Replay the writes the boot ROM actually makes:

```
NR52 = $80   NR11 = $80   NR12 = $F3   NR51 = $F3
NR50 = $77   NR13 = $C1   NR14 = $87
```

That is the only formulation that gets both the read-back *and* the channel-enable flip-flop
right — and it falls out for free that `NR10`/`NR2x`/`NR3x`/`NR4x` are never written at all,
their read-back masks supplying every expected bit.

**The boot beep is deliberately not reproduced.** On hardware the second "ding" is still
ringing at hand-off, but its envelope volume is not readable from any register, so how far it
has decayed cannot be derived. The channel-enable flip-flop is set, as hardware has it; the
envelope is left at zero.

> A related trap that hid this for a long time: `set_initial`'s APU block was **dead code**.
> `self.sound` is `None` at construction, so all twenty-two `NR*` writes were dropped and the
> chip installed afterwards started from zeroed defaults. It lives in
> `Sound::set_post_boot_state` ([`src/sound.rs`](../../src/sound.rs)) now, applied only to a
> freshly created chip — re-attaching a player mid-run must not rewind it.

### 2. An unanswered link transfer still completes

Eight falling edges of system-counter bit 8 (8192 Hz, ~1 ms/byte); then `SB` takes the byte,
`SC` bit 7 clears, `IF` bit 3 is set. With nothing attached the byte is `$FF` — **the console
drives the clock, so the transfer runs regardless.** Dropping it was what made
`serial/boot_sclk_align` print "No serial intr".

**The one-M-cycle subtlety, and only one ROM in any suite can see it:** the serial edge must be
taken from the counter *as an access observes it* (`div + 4`), not from `div`. Use `div` and
the interrupt arrives one instruction late — `boot_sclk_align` reports `C: 90!` with `$91`.
Verified by making that change deliberately.

**No unit test can catch it.** Every other measurement of the serial clock is relative to an
`SC` write, and a constant phase offset cancels; only alignment against a boot-time counter
exposes it.

### 3. The boot ROM leaves a tilemap behind, and it matters far more than it sounds

`set_boot_logo_tilemap` ([`src/mmu/mod.rs:589`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/mod.rs#L589)) writes `$01`–`$0C` to
`$9904`–`$990F`, `$0D`–`$18` to `$9924`–`$992F` and `$19` (the `®`) to `$9910`, for every
DMG-class machine, **before** `LCDC` is enabled so the write cannot land in a blocked PPU mode.

A ROM that draws on part of the screen and never clears the tilemap renders its *own* tiles in
rows 8 and 9 on hardware. We rendered blanks. Under a shade-rank image comparison that is a
**whole-screen** mismatch, because one shade fewer on screen re-ranks every white pixel — and
both SameSuite `sgb/` ROMs reported their own verdict as PASS while failing the Shootout's
screenshot comparison for this reason alone.

The tile **pixels** are still not written; on hardware they are the logo decompressed out of
the cartridge header, and every ROM has replaced them by the time anything looks.

**Where the PPU is at hand-off is a per-console constant**, and only DMG 0 gets one
(`boot_ppu_dot`, [`src/mmu/mod.rs:140`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/mod.rs#L140)): `boot_hwio-S` and
`misc/boot_hwio-C` **skip** `$FF40`/`$FF41`/`$FF44` in their own masks, so no ROM measures it
on those machines and none is invented. The DMG 0 constant is calibrated **with its window
stated** — line 145 dots 96..260 all pass, so it is the centre of a 168-dot band, and neither
edge is hardware.

---

## The alternative: bring your own boot ROM

Because one thing genuinely cannot be reconstructed — the boot logo's **tile pixels**, of
which the `®` glyph is the load-bearing eight bytes for the Mealybug suite.

A register value or a counter phase is *a fact about hardware* and not copyrightable
expression, which is why the whole `$FF00-$FF7F` map above is transcribed by hand. **A glyph
bitmap is expressive content**, and is not ours at any size. So instead:
[`src/boot.rs`](../../src/boot.rs) will load one the *user* supplies.

```
${XDG_CONFIG_HOME:-~/.config}/terminalgb/dmg_boot.bin    256 bytes
${XDG_CONFIG_HOME:-~/.config}/terminalgb/cgb_boot.bin   2304 bytes
$GB_BOOT_ROM=<path>                                     overrides both
```

**Three rules, not negotiable: never bundle one, never fetch one, fail soft.** No file,
unreadable, wrong length or wrong console → exactly the hand-written machine above. A DMG image
on a Color machine is *declined*, not approximated.

**The gated suites must never pick one up automatically.** Only the terminal frontend
auto-loads; `examples/mealybug` takes `--boot-rom <path>` explicitly and **refuses to combine
it with `--baseline`**. A score that depended on what happens to be in somebody's `~/.config`
would not be a gate.

Measured through the shipped path, `GB_PPU=accurate`:

| | no boot ROM | with one |
|---|---:|---:|
| Mealybug total distance | 693,023 px | **483,005 px** (−30 %) |
| `m3_obp0_change` | 22,750 | **108** |
| `m3_lcdc_obj_en_change` | 22,894 | **256** |
| `m3_scx_low_3_bits` | 22,500 | **324** |

**No row flips.** The boot ROM removes a *content* error the emulator could never fix; what
remains in those rows is mid-scanline timing, which is ours. The measurement was made with
hand-written stand-in images that reproduce the post-boot state and then unmap themselves —
never committed, and cross-checked against injecting the same state directly, to the pixel.

---

## What a wrong emulator looks like

| Symptom | Cause |
|---|---|
| A game's "random" encounters are identical every launch | the system counter starting at zero instead of `$ABCC` / `$2678` / `$1EA0` |
| A **CGB-flagged** game's first frames read a `DIV` no real cartridge of it ever saw — its RNG stream, its APU phase and every timer-relative event shifted by a constant | one Color boot counter for both cartridge headers. The compatibility path's `$2678` is `$7D8` T-cycles away from the genuine path's `$1EA0`, and a DMG-header test ROM cannot see the difference |
| A game enables the VBlank interrupt, `EI`s at start-up, and waits a whole frame for an interrupt hardware already had pending | `IF` initialised to `$E0` instead of `$E1` |
| A game reads `P1` before writing it and mis-detects the console | `P1 = $FF` on a DMG instead of `$CF` |
| A boot-register test ROM prints `HL: 007C` where a DMG shows `014D`, though every assertion inside it passed | a Color machine booted for a DMG cartridge — invisible to a player, fatal to a screenshot-judged suite |
| A cartridge cannot tell an MGB from a DMG, or an AGB from a CGB | `A = $FF` / `B` bit 0 not modelled |
| An SGB plays a start-up ding | `NR14 = $87` copied from the DMG instead of the SGB's `$07` |
| A test ROM's screenshot differs across the *whole* frame while its own verdict says PASS | the boot ROM's leftover tilemap not written — one shade fewer re-ranks every pixel |
