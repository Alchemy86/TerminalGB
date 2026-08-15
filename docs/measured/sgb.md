# The Super Game Boy — a joypad port that is also a command channel

← [The measured Game Boy](README.md)

The SGB is the one peripheral where **the interesting question is what to leave out**, and
where getting the scope right turned a "new subsystem, declare it out of scope" recommendation
into about three hundred lines.

Hardware reference, not restated here:
[SGB Command Packet Transfer](https://gbdev.io/pandocs/SGB_Command_Packet_Transfer.html) ·
[Multiplayer (`MLT_REQ`)](https://gbdev.io/pandocs/SGB_Command_MLT_REQ.html) ·
[Joypad Input](https://gbdev.io/pandocs/Joypad_Input.html). Component page and the non-goal
argument in full: [`docs/components/sgb.md`](../components/sgb.md).

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`6c20451`](https://github.com/Alchemy86/TerminalGB/tree/6c20451306663018288d34253c199fad507ae6f6) · regenerate with `make docs-links`, verify with `make docs-check`.

## Scope: only one part of an SGB is *observable*, and it is the part worth building

An audit rated this work lowest priority and recommended dropping it, on the grounds that it is
a new subsystem and that bgb fails two of the three published rows as well. **The size estimate
was wrong, and the reason generalises.**

All three of the Shootout's SGB reference screenshots are plain 160×144 DMG-grey screens. No
border, no SGB palette, no SNES side appears in any of them — because **the border, the
palettes and the SGB's own sound are one-way traffic to a screen the cartridge cannot read
back.** No test ROM can assert them, and no *game* can either. What every one of the three
tests measures is the same thing: the command-packet receiver on `$FF00` and the `MLT_REQ`
multiplayer joypad index, which is the only part of an SGB a Game Boy program can see.

So border/palette/sound are a **documented non-goal**, and the argument is that one — not
effort. If they are ever wanted they are a *display* feature, not an emulation one.

*What it scores:* SameSuite `sgb/command_mlt_req` and `sgb/command_mlt_req_1_incrementing` (the
ROM's own `RESULT_CODE`), `cpp/sgb-ext-test` (its author's reference screenshot), and Mooneye
`acceptance/boot_regs-sgb` / `-sgb2` — five rows, all green, all gated by `make test-external`.
All three published SGB rows are pixel-exact against the Shootout's own references (**0 of
23,040 pixels differ on each**).

*Calibration before anyone quotes that:* of the eighteen emulators on the published board, only
**GSE** passes all three SGB rows. SameBoy passes one, bgb one, mGBA two, and six decline the
SGB model entirely.

---

## Four protocol rules the prose leaves ambiguous, and the ROMs settle

A packet is 16 bytes sent LSB-first by pulsing `P14` / `P15`. Pan Docs describes the idiom; it
does not decide these four, and an implementation can be plausible and wrong on every one of
them. Each is a one-line check in its own right.

| Rule | What the ROM does to separate it | Pinned by |
|---|---|---|
| **`$00` arms *and* aborts** — and a bare `$00` not followed by `$30` starts nothing at all. The opening `$00` must be acknowledged by a `$30` before any pulse is read as a bit | `sgb-ext-test`'s `SendPacketShortStart` sends a packet without that acknowledgement and expects it to be **ignored entirely** | `a_transfer_without_its_opening_pulse_is_ignored` |
| **The bit is the last line held low.** `$20 → $10 → $30` transmits a `1`; `$10 → $20 → $30` transmits a `0`. An implementation that latches on the *first* falling edge gets both backwards | `sgb-ext-test` sends exactly those two sequences for the same bit position | `the_last_line_held_low_decides_the_bit` |
| **The command runs on the last data bit, not on the stop bit**, so a corrupt stop cannot cancel it | `SendPacketCorruptStop` terminates a packet with a `1` where the stop bit should be `0` and still expects the command to have taken effect | `a_corrupt_stop_bit_does_not_cancel_the_command` |
| **The player index advances on the *rising* edge of `P15` and on nothing else** — so `$10 → $30` moves it and `$20 → $30` does not | `command_mlt_req_1_incrementing` is eight pulse patterns that test only this — **the cheapest single check of the three ROMs** | `only_a_rising_p15_edge_advances_the_player` |

The joypad register reports `$F - index` in its low nibble whenever both select lines are
released. [`src/sgb.rs`](../../src/sgb.rs) is the whole implementation.

---

## `MLT_REQ 2` is three players, not four — and SameBoy gets this wrong

The single most useful row here, because it is where implementations diverge and the divergence
is one line of code.

`player_count = (param & 3) + 1`, **with no correction.** So the illegal parameter `2` requests
*three* players; the index is masked with `player_count - 1 = 2` and can therefore only ever be
0 or 2, and a game that sends it sees the joypad alternate between player 1 and player **3** —
not cycle 1-2-3-4, and not stay on 1.

Applying the command then advances the index `player_count` further times. That is a whole
number of turns for 1, 2 and 4 players and therefore invisible; for 3 it is the entire point,
and it is what sixteen of `command_mlt_req`'s twenty-four samples measure. (Physically: the SNES
polls every controller it now believes is attached.)

**SameBoy fails this row**, and the whole difference is `Core/sgb.c`'s
`if (player_count == 3) player_count++;` — rounding the illegal request up to four. **Do not
"fix" ours to match it.**

> **The rule that carries: an illegal parameter must not be normalised.** Rounding a
> nonsensical request to the nearest sensible one is the instinct, and it is exactly what a
> test ROM written to probe the hardware's arithmetic is looking for. This is not an SGB fact;
> it is a fact about writing emulators.

### The model was derived, not guessed

Fifty-nine ground-truth constraints: 24 + 8 expected bytes from the two SameSuite ROMs' own
`CorrectResults` tables, plus 27 player counts decoded out of `sgb-ext-test`'s reference PNG.
The four rules above plus the index rule are **the only model that satisfies all fifty-nine.**

If you change any rule here, re-check it against all three ROMs, not one.

---

## `Model::Auto` must never pick an SGB

The cartridge SGB flag (`$0146 == 0x03`) says the game *offers* a border, **not that the player
owns the peripheral.**

Auto-booting one would change every SGB-enhanced game's boot registers — Pokémon Red/Blue
included — for no visible gain, since the border is a non-goal. `Model::Sgb` / `Sgb2` are
things you ask for by name (`GB_MODEL=sgb|sgb2`), exactly as with the
[console revisions](boot.md#a-boot-rom-is-part-of-the-silicon-so-a-revision-is-a-machine).

The suite runners select it the way the Shootout's own manifest does: SameSuite's runner from
the header flag, Mooneye's from the `-sgb` / `-sgb2` filename suffix. **Reading the machine off
the ROM name is the runner's job, not the emulator's** — the same rule
[SameSuite's contradictory CGB revisions](apu.md#the-game-boy-color-shipped-in-revisions-and-its-apu-changed-between-them)
and [Mealybug's three reference sets](ppu.md#the-color-consoles-ppu-shipped-in-revisions-too)
both teach.

**An SGB is DMG-class silicon, not a fifth kind of machine.** `GbMode::is_dmg()` is what every
"this is DMG hardware" branch asks — Classic **and** both SGBs, never `ColorAsClassic` — which
covers the PPU's OAM bug, the undocumented CGB registers, the boot counter, `P1`'s boot state
and `STOP`'s screen fill. Its *boot* state genuinely differs, though, and in two ways worth
knowing:

- **`P1 = $CF` is a `GbMode::Classic` fact, not an "is DMG-class" one.** An SGB's last use of
  the port is clocking packets, ending on `$30`, so it leaves the lines **released** (`$FF`)
  like a CGB.
- **An SGB has no start-up ding.** Its last audio write is `NR14 = $07`, not `$87` — channel 1
  is set up and never triggered, so `NR52` reads `$F0`.
- **Its boot ROM does not take a fixed time**, because it clocks the cartridge header to the
  SNES through an unbalanced bit loop — see [boot.md](boot.md#an-sgbs-boot-rom-does-not-take-a-fixed-time).

---

## The row that was not an SGB bug at all

Both SameSuite `sgb/` ROMs reported their own verdict as **PASS** while failing the Shootout's
screenshot comparison — and the cause was the DMG boot ROM's leftover tilemap, which is nothing
to do with the SGB.

Two shades on screen instead of three re-ranks **every white pixel** under a shade-rank
comparison, so the whole frame mismatched over two rows of glyphs. Full detail in
[boot.md § the boot ROM leaves a tilemap behind](boot.md#3-the-boot-rom-leaves-a-tilemap-behind-and-it-matters-far-more-than-it-sounds).

> **Two verdict methods disagreeing is the finding**, and here they disagreed for a reason in
> neither subsystem the row names. A project that scores itself by one method never sees this;
> that is why the `cpp` group is now run against its author's reference PNGs as well.

---

## What a wrong emulator looks like

| Symptom | Cause |
|---|---|
| A two-player SGB game reads player 1's buttons for both players | `MLT_REQ` not implemented — the joypad index never advances |
| Every bit of every SGB command arrives inverted | latching the bit on the first falling edge instead of on the last line held low |
| A game's SGB setup packet is silently dropped, and the game carries on as a plain DMG | requiring more than the last data bit to run the command, so a corrupt or absent stop cancels it |
| `MLT_REQ 2` cycles four players | normalising the illegal parameter — the one row SameBoy fails |
| Pokémon Red/Blue's boot registers are wrong on an emulator that has never been asked for an SGB | `Model::Auto` inferring the peripheral from the cartridge's SGB flag |
| Every SGB test ROM passes its own verdict and fails a screenshot comparison over the whole frame | the DMG boot ROM's leftover tilemap, not an SGB defect at all |
