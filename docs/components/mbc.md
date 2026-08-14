# Cartridges — Memory Bank Controllers (MBC)

> Source: [`src/mbc/`](../../src/mbc) · Related: [MMU](mmu.md)
> [← Component index](README.md) · [🗺️ Code map](../codemap.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/gameboy/tree/main); line references documented as of commit [`4c727ec`](https://github.com/Alchemy86/gameboy/tree/4c727ec8ea465327f9ff5568ae171f6ca4d915d4) · regenerate with `make docs-links`, verify with `make docs-check`.

## 1. What it is

A Game Boy cartridge is not just a ROM chip — for anything bigger than 32 KB it also
contains a tiny controller chip, the **MBC**, that lets a 16-bit CPU reach far more than
64 KB of data. The CPU only sees a small window (`0x0000–0x7FFF` for ROM,
`0xA000–0xBFFF` for RAM); the MBC swaps which slice of the cartridge's much larger ROM/RAM
is visible in that window. The game changes banks by *writing* to what looks like ROM —
those writes don't store data, they're commands to the controller.

In this emulator each MBC type is a Rust struct implementing one trait, chosen from the
cartridge header when a ROM is loaded.

## 2. The real hardware

The window the CPU sees:

- **`0x0000–0x3FFF`** — usually fixed to ROM bank 0.
- **`0x4000–0x7FFF`** — the switchable ROM bank.
- **`0xA000–0xBFFF`** — switchable cartridge RAM (if present), often battery-backed.

Banking is controlled by **writing to ROM address ranges**, which the MBC decodes as
register writes (enable RAM, set ROM bank, set RAM bank, select mode). The cartridge type
is a single byte in the header at **`0x0147`**; the ROM/RAM sizes are at `0x0148`/`0x0149`.
(Pan Docs — [MBCs](https://gbdev.io/pandocs/MBCs.html).)

## 3. History and versions — what each chip adds

This is the "history / versions" that matters for a cartridge: not model names, but what
each controller could *do*, and which games needed it.

- **No MBC / "ROM only" (types `0x00`, `0x08` ROM+RAM, `0x09` ROM+RAM+BATTERY).** Up to
  32 KB, no banking, no controller, and optionally 8 KB of (battery-backed) RAM. Early/
  small titles: *Tetris*, *Dr. Mario*, *Alleyway*.
- **MBC1 (`0x01–0x03`).** The classic controller: up to **2 MB ROM** (125 usable banks)
  and **32 KB RAM**, with a mode bit that trades the two high ROM-bank bits for RAM-bank
  selection. Most early-to-mid library: *Super Mario Land*, *The Legend of Zelda: Link's
  Awakening*, *Castlevania*. A well-known wrinkle is the **MBC1M multicart** variant and
  the bank-0-remap behaviour in mode 1.
- **MBC2 (`0x05–0x06`).** Small ROM (≤256 KB) but **built-in 512 × 4-bit RAM** on the chip
  itself — a nibble RAM, no external SRAM. *Kirby's Pinball Land*, *Final Fantasy Legend*.
- **MBC3 (`0x0F–0x13`).** Up to **2 MB ROM**, **32 KB RAM**, and crucially a **real-time
  clock (RTC)** — a battery-backed clock that keeps ticking with the power off. This is
  what made *Pokémon Gold/Silver/Crystal* day/night cycles and *Harvest Moon* work.
- **MBC5 (`0x19–0x1E`).** The CGB-era controller: guaranteed to work in **double-speed
  mode**, up to **8 MB ROM** (a full 9-bit bank number) and **128 KB RAM**, with optional
  **rumble** (`0x1C–0x1E`). Nearly every Game Boy Color game: *Pokémon Crystal*,
  *Wario Land 3*, *Perfect Dark*.

**This emulator implements MBC0/1/2/3/5.** MBC6/7, MMM01, HuC1/3, and the Pocket Camera
are not supported.

## 4. How it works here

**The trait** — `MemoryBankController`
([`src/mbc/mod.rs:11`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mod.rs#L11)): `readrom`, `readram`, `writerom`,
`writeram`, plus a default `romname` that reads the title bytes at `0x134`.

**Type dispatch** — `get_mbc` ([`src/mbc/mod.rs:64`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mod.rs#L64)) reads
`data[0x147]` and constructs the right controller:

```
0x00/08/09  → MBC0     (no mapper; 0x09 is battery-backed)
0x01..=0x03 → MBC1     (mod.rs:54)
0x05..=0x06 → MBC2     (mod.rs:56)
0x0F..=0x13 → MBC3     (mod.rs:58)
0x19..=0x1E → MBC5     (mod.rs:60)
```

Each controller: **MBC0** (no banking, optional 8 KB RAM for `0x08`/`0x09`,
[`mbc0.rs:4`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc0.rs#L4)); **MBC1**
(5-bit + 2-bit bank registers, mode select, bank-0 remap;
[`mbc1.rs:164–243`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc1.rs#L164)); **MBC2** (512-byte nibble RAM: the low nibble is
stored, reads OR in `0xF0`; [`mbc2.rs:27`,`72`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc2.rs#L27)); **MBC3** (RTC clocked by
emulated time — see below; [`mbc3.rs`](../../src/mbc/mbc3.rs) +
[`rtc.rs`](../../src/mbc/rtc.rs)); **MBC5** (9-bit ROM bank split across
`0x2000` and `0x3000`; [`mbc5.rs:90`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc5.rs#L90)).

**Battery saves** live in [`src/mbc/battery.rs`](../../src/mbc/battery.rs), shared by every controller. A `Battery`
exists only for a battery cart subtype *with* a real filepath and either some declared RAM or
an RTC; otherwise it is `Battery::none()` and persists nothing (the web and FFI builds).

- **Written while the emulator runs, not on `Drop`.** `writeram` calls `Battery::touch`;
  `Gameboy::frame` calls `MemoryBankController::autosave` once per frame, which writes the
  save out after the game has left cartridge RAM alone for 60 frames (~1 s), or after 600
  frames of continuous writing. `Drop` still flushes, but only as a last chance — **no
  frontend can guarantee `Drop` runs** (see the sharp edge below), and relying on it was the
  cause of total save loss.
- **Atomic.** Every write goes to `<file>.tmp`, is `fsync`ed, then `rename`d over the
  target, so an interrupted write can never truncate an existing save.
- **Never writes unless the game wrote.** A session that does not touch cartridge RAM
  leaves the save file alone — the old code unconditionally wrote its pristine all-zero RAM,
  which is how one failed load turned into permanent data loss.
- **File format: `<rom>.sav`, raw cartridge RAM, bank 0 first, no header.** Byte-for-byte
  what FlashGBX writes when it dumps a real cartridge and what every emulator reads, so
  saves move in both directions. `<rom>.srm` is accepted on load as well.
- **The MBC3 clock is a 48-byte footer appended to that same file** — the VBA-M / BGB /
  FlashGBX convention, all little-endian:

  | offset | size | field |
  |---|---|---|
  | 0x00 | 4 | RTC S (seconds), as `u32` |
  | 0x04 | 4 | RTC M (minutes) |
  | 0x08 | 4 | RTC H (hours) |
  | 0x0C | 4 | RTC DL (day counter, low 8 bits) |
  | 0x10 | 4 | RTC DH — bit 0 = day bit 8, bit 6 = HALT, bit 7 = day CARRY |
  | 0x14 | 20 | the same five registers again: the latched copy |
  | 0x28 | 8 | `u64` UNIX timestamp of the write |

  It is written **only for header types `0x0F` and `0x10`**, the MBC3 variants that really
  have a clock. Type `0x13` is RAM+BATTERY with no RTC and gets nothing extra. Detection on
  load is by size — `filesize - ramsize` of 48 (or 44, an older 32-bit-timestamp variant)
  means a footer. On load the counters are restored as stored and then advanced by
  `now - timestamp` seconds (`Rtc::restore`), so the clock gains exactly the wall time that
  passed while the emulator was closed — which is what the coin cell does on hardware. A
  halted clock gains nothing, and a timestamp from the future advances nothing rather than
  running backwards.
- **Legacy `<rom>.gbsave`** (8-byte **big-endian** RTC epoch anchor prepended to the RAM) is
  still **read** when no `.sav` exists and is migrated on the next flush; the old file is
  left on disk untouched as a backup and never written again. Nothing else in the ecosystem
  ever read that format, and the prefix was applied even to type `0x13` — so every foreign
  save loaded 8 bytes shifted and every save we wrote was shifted for everyone else.
- **A save that cannot be read is never overwritten.** A read error poisons the battery so
  later writes are refused, and construction fails loudly rather than silently starting a
  new game on top of an existing save.
- **A save written by a crashed game is never written at all.** `Battery::poison` is the same
  rule pointed the other way, and any part of the emulator can invoke it through
  `MemoryManagementUnit::report_fault`: once something establishes that the CPU is no longer
  executing the program, the bytes in cartridge RAM are not a save, and the autosave would
  carry them straight over the real one. Measured on a real crash: **4,378 bytes of cartridge
  RAM changed within six frames.** The latch covers `tick`, `flush` and the last-chance
  `Drop`, is sticky for the session, and the user is told rather than silently robbed. Full
  story: [`../link-battle.md`](../link-battle.md).

**The header checksum** — `check_checksum`
([`src/mbc/mod.rs:116`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mod.rs#L116), `#[allow(dead_code)]`) sums `0x134..0x14D`
and compares `0x14D`. Its two tests (`checksum_zero`, `checksum_ones`,
[`mod.rs:130`,`137`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mod.rs#L130)) are **2 of the 4 tests in the whole repo**.

### Honest approximations & known bugs

- **The MBC3-for-everything bug.** `get_mbc` short-circuits at the top:

  ```rust
  if filepath.is_none() {
      return mbc3::MBC3::new_without_save(data)...   // mbc/mod.rs:43-45
  }
  ```

  When there is no filepath — **always true for the web and FFI builds** — it constructs
  an **MBC3 regardless of `0x0147`**. An MBC1 or MBC5 game in the browser is mis-driven as
  MBC3, so its banking is wrong. This is the scout report's finding, confirmed at
  [`mbc/mod.rs:68`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mod.rs#L68).
- **MBC3 RTC is driven by emulated time** ([`rtc.rs`](../../src/mbc/rtc.rs)): a sub-second
  divider counts T-cycles of *real console* time, ticking the counter chain once every
  4,194,304 of them, and `MemoryManagementUnit::do_cycle` advances it once per instruction.
  The host wall clock is used for exactly one thing — the gap between a save being written
  and reloaded, which is what the coin cell covers. *(This used to derive every register from
  `SystemTime::now()`, which made the in-game clock run at whatever multiple of real time the
  emulator was running at, and panicked outright if the host clock predated the UNIX epoch.
  ax6 `rtc3test` 0/3 → 3/3.)*
- **MBC5 rumble is not implemented** — the motor-control range is an explicit no-op
  (`0x6000..=0x7FFF => { /* ? */ }`, [`mbc5.rs:101`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc5.rs#L101)); rumble carts
  still load (RAM/battery handled) but never rumble.
- **`Drop` is not a shutdown hook — do not persist anything in one.** The desktop
  frontend's `glutin`/`winit` `event_loop.run()` **never returns**: winit 0.26 calls
  `process::exit(0)` on `ControlFlow::Exit` (`winit-0.26.1/src/platform_impl/linux/x11/mod.rs:393`
  and the Wayland equivalent), so the `Gameboy` moved into the closure is never dropped.
  `[profile.release] panic = 'abort'` skips destructors too, as does closing a terminal
  window or any `kill`. This is why cartridge RAM is now persisted per-frame; anything else
  that must survive a shutdown needs the same treatment.
- **Header byte `0x148` is not trusted for sizing.** `rom_banks` falls back to the ROM's own
  length and never returns fewer than 2 banks. The unofficial codes `0x52`/`0x53`/`0x54`
  (1.1/1.2/1.5 MiB) occur in real cartridges and used to make every `% rombanks` a
  divide-by-zero. Real dumpers agree: FlashGBX uses `0x148` for display only and measures the
  hardware.
- **RAM size code `0x01` is emulated as a full 8 KiB bank, not 2 KiB.** The ecosystem treats
  `0x01` as 0x800 bytes, so our `.sav` for such a cart would be 8192 bytes where FlashGBX
  writes 2048. Left as-is deliberately: our own header note records that no real cartridge
  ever shipped a 2 KiB chip, and some test ROMs rely on the whole bank. The related hardware
  behaviour we do *not* model is that an undersized SRAM chip **mirrors** across the
  0xA000-0xBFFF window — a ready-made conformance check (it is how FlashGBX recovers the true
  save size), recorded as test #17 below.
- **MBC3 latches the RTC on any write to `0x6000-0x7FFF`** — and that is not an
  approximation. This page used to record it as a deviation from the `0x00`→`0x01` sequence;
  it is not. CasualPokePlayer's `latch-rtc-test` writes one *random* byte to `0x6000` per row
  and its reference dump has every row reading back fresh values. Requiring the pair was tried
  and turned that ROM's output into a screen of zeroes. Pan Docs describes the idiom games
  use, not the decode.
- **MBC3's ROM-bank register masks to 7 bits (`v & 0x7F`, 2 MiB).** MBC30 allows 8. Every
  real MBC30 cartridge is 2 MiB so this is currently invisible, but 4 MiB MBC30 ROM hacks
  exist and would mis-bank. (64 KiB MBC30 RAM *does* work — the RAM-bank register already
  keeps 3 bits.)
- **MMM01 and M161 multicart mappers are still unsupported** — like MBC1M they hide behind
  ordinary-looking headers, but unlike MBC1M they are different *mappers*, not a different
  wiring of MBC1, so nothing here drives them. (MBC1M itself is now detected and wired
  correctly — see §5.)

## 5. Known quirks and edge cases

- **RAM enable gate.** Cartridge RAM only responds after the game writes `0x0A` to the
  enable range; otherwise reads return open-bus. Modelled per-controller.
- **MBC1 bank-0 remap.** Writing bank `0` to the low register selects bank `1`
  ([`mbc1.rs:217`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc1.rs#L217)); in mode 1 the `0x0000–0x3FFF` region can show a
  non-zero bank ([`mbc1.rs:173`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc1.rs#L173)).
- **MBC1M multicart wiring.** A 1 MiB multi-game compilation cart ignores the top bit of
  BANK1 and moves BANK2 to bank bits 4-5, so mode 1 shows banks `$00/$10/$20/$30` in
  `0x0000–0x3FFF` — that is how its menu switches games. The `00 → 01` translation still
  looks at all five written bits first, so writing `$10` selects bank `$00`. Such a cart
  cannot be told from a plain MBC1 by its own header; it is identified by the second
  cartridge header (Nintendo logo at `$40104`) that the compilation leaves in bank `$10`,
  and `GB_MBC1_MULTICART=1|0` overrides that heuristic in either direction. Pan Docs,
  ["MBC1M"](https://gbdev.io/pandocs/MBC1.html).
- **MBC2's 4-bit RAM.** Only the low nibble of each of 512 bytes is real; reads force the
  high nibble to `1` ([`mbc2.rs:72`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc2.rs#L72)).
- **MBC3 RTC latch.** Any write to `0x6000–0x7FFF` snapshots the counters into the registers
  the game reads (`Rtc::write_latch`, [`rtc.rs`](../../src/mbc/rtc.rs)); games write `0x00`
  then `0x01`. The day counter's sticky carry bit sets past 511 days (`Rtc::advance_day`).
- **MBC3 RTC counters are counters, not a time.** Seconds and minutes are 6-bit, hours 5-bit,
  the day counter 9-bit. Each carries only when it rolls over from its natural maximum (59,
  59, 23); a value loaded *above* that maximum counts on to the top of its physical width and
  wraps to zero **without** carrying — so 62 minutes becomes 63 and then 0, leaving the hours
  alone. `Rtc::advance_counter`.
- **MBC3 RTC halt freezes the sub-second divider too** (`DH` bit 6), so a clock stopped 600 ms
  into a second and restarted a while later ticks 400 ms after the restart, not a full second.
- **Writing the seconds register restarts the sub-second divider**; writing any other RTC
  register leaves it running.
- **MBC5 9-bit bank.** `0x2000–0x2FFF` sets the low 8 bits, `0x3000–0x3FFF` bit 0 is the
  9th ([`mbc5.rs:93–98`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc5.rs#L93)).

## 6. Expected behaviours as tests

> **Coverage today: the two header-checksum tests (#1) and the battery-save durability
> harness (#9–#16, `testharness/battery.rs`).** All *banking* behaviour is still untested —
> though #11 exercises MBC3 RAM banking end to end as a side effect.

1. **Header checksum.** `check_checksum` accepts a header whose `0x14D` matches the
   computed sum and rejects one that doesn't. ✅ **Covered** (`checksum_zero`,
   `checksum_ones`, [`mbc/mod.rs:130`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mod.rs#L130)).
2. **Type dispatch honours `0x0147`.** A ROM with `0x0147 = 0x01` loads as MBC1, `0x19` as
   MBC5, etc. — **when a filepath is given**. *(Not covered.)*
3. **The no-filepath path mis-selects MBC3.** With `filepath = None`, an MBC1 ROM is
   constructed as MBC3. **This is a bug: the test should assert the *correct* type and is
   expected to FAIL until fixed.** *(Not covered; known-divergent.)*
4. **MBC1 ROM banking.** After writing bank `2` to `0x2000`, reads at `0x4000` come from
   ROM bank 2; writing `0` selects bank 1. *(Not covered.)*
5. **MBC1 RAM enable + banking mode.** RAM is inaccessible until `0x0A` is written to
   `0x0000`; in mode 1 the RAM bank register selects among 4 RAM banks. *(Not covered.)*
6. **MBC2 nibble RAM.** Writing `0xFF` to a RAM cell reads back as `0xFF` but only 4 bits
   are stored (upper nibble always 1). *(Not covered.)*
7. **MBC3 RTC latch.** After a latch, the seconds register reads a stable value until the
   next latch — and *any* single write to `0x6000–0x7FFF` is a latch. ✅ **Covered**
   (`src/mbc/rtc.rs` unit tests `reads_come_from_the_latch_not_the_counters`,
   `any_single_write_latches`; externally by CasualPokePlayer `latch-rtc-test`).
   - *The clock ticks once per emulated second, not once per host second.* ✅ **Covered**
     (`one_second_is_one_second_of_emulated_time`; externally by ax6 `rtc3test` "Basic tests",
     which times it with the Game Boy's own timer to 1000 ms ± 1 ms).
   - *Out-of-range RTC values count on without carrying*, and a carry *into* an out-of-range
     counter still increments it. ✅ **Covered**
     (`an_out_of_range_time_ticks_without_carrying`,
     `an_overflowing_counter_wraps_without_carrying`,
     `a_carry_into_an_out_of_range_counter_increments_it`; externally by `rtc3test` "Range
     tests").
   - *The sub-second divider restarts on a seconds write and on nothing else, and freezes
     while the clock is halted.* ✅ **Covered**
     (`only_a_seconds_write_resets_the_sub_second_divider`, `halting_freezes_the_divider_too`;
     externally by `rtc3test` "Sub-second writes").
8. **MBC5 9-bit ROM bank.** Selecting bank `0x100` (via `0x3000` bit 0) reads from ROM
   bank 256. *(Not covered.)*
9. **Battery save survives an unclean exit.** A game writes cartridge RAM; the process dies
   with **no destructor run at all**; the save is on disk and complete. ✅ **Covered**
   (`testharness/battery.rs`, `cargo test --release --test battery`).
   *Player-visible symptom when this fails: you save in Pokémon, quit, relaunch, and the
   title screen offers only "NEW GAME".*
10. **Battery save is a raw `.sav`.** The file is exactly `ramsize` bytes of cartridge RAM
    with no header, so other emulators can read it. ✅ **Covered** (same harness).
11. **Every RAM bank reloads through the bank register.** A fresh machine reads back each
    of the four MBC3 banks correctly. A save whose banks are stored out of order has the
    right file size but fails the game's own checksum, which looks identical to an empty
    save. ✅ **Covered** (same harness).
12. **A session that saves nothing does not touch the save file.** ✅ **Covered** (same
    harness) — this is the guard against one failed load erasing a real save.
13. **Legacy `.gbsave` migrates.** It loads, is rewritten as `.sav`, and the original file
    is left byte-identical as a backup. ✅ **Covered** (same harness).
14. **A foreign `.sav` loads**, including the VBA-M/mGBA convention of a 48-byte RTC block
    appended to the RAM. ✅ **Covered** (same harness).
15. **A plain `.sav` loads at offset 0.** A save from another emulator or a real cartridge
    is not shifted, and nothing is prepended to what we write. ✅ **Covered** (same harness).
    *Symptom when this fails: every foreign save is corrupt on load and every save we write
    is corrupt for everyone else — and for a game with a save checksum, indistinguishable
    from "NEW GAME".*
16. **Malformed or unusual input does not crash.** A RAM-less MBC5+RUMBLE bank write, an
    unofficial ROM-size code, a `.sav` far shorter than the cartridge's RAM, a ROM truncated
    below 32 KiB, and a no-mapper ROM+RAM+BATTERY cart (`0x09`). ✅ **Covered** (same
    harness). *Under `panic = 'abort'` each of these was an instant process death, which also
    means the player's save never got written.*
17. **Undersized cartridge RAM mirrors.** On a 2 KiB chip, `0xA800` reads the same byte as
    `0xA000`. *(Not covered — we allocate a full 8 KiB bank; see the honest-approximations
    note above.)*
18. **MBC3 RTC footer round-trip.** After a save and reload the clock reads the stored time
    plus the wall time that elapsed while the emulator was closed. ✅ **Covered** — the
    48-byte layout, the 44-byte variant and the footer-only-for-clock-carts rule are
    unit-tested in [`battery.rs`](../../src/mbc/battery.rs), and the catch-up itself in
    [`rtc.rs`](../../src/mbc/rtc.rs) (`the_clock_runs_while_the_emulator_is_shut`,
    `a_halted_clock_does_not_move_while_the_emulator_is_shut`,
    `a_long_catch_up_matches_ticking_second_by_second`). *(Still no battery-RTC cartridge in
    the end-to-end harness.)*
19. **An MBC1M multicart banks by bits 4-5, an ordinary MBC1 by bits 5-6.** Writing `$10`
    to BANK1 selects bank `$00` on a multicart and bank `$10` on a plain cart, and mode 1
    shows `$00/$10/$20/$30` versus `$00/$20/$40/$60` in `0x0000–0x3FFF`. ✅ **Covered**
    (`mbc1::tests::multicart_banking_matches_the_mooneye_table`, the 256-entry expectation
    transcribed from Mooneye's `multicart_rom_8Mb.s`).
20. **A cart is only treated as a multicart when it looks like one.** A 1 MiB MBC1 image
    with no second header in bank `$10` keeps the ordinary wiring; the false-positive
    direction is the dangerous one, since it strands half the ROM. ✅ **Covered**
    (`a_plain_1mb_cart_keeps_the_ordinary_wiring`,
    `multicart_detection_requires_a_1mb_image`).

## 7. Test ROMs that exercise it

| Suite | What it checks | Meaning of a failure |
|---|---|---|
| **Mooneye `emulator-only/mbc1/`** (`bits_bank1`, `bits_ramg`, `rom_512kb`, `multicart_rom_8Mb`, …) | Every MBC1 register bit, sizes, multicart | Precisely targets §4/§5. All green — the whole `emulator-only` suite is **28/28** (`examples/mooneye_scout.rs`). |
| **Mooneye `emulator-only/mbc2/`, `mbc5/`** | MBC2 nibble RAM + register bits; MBC5 9-bit bank | Banking maths errors. |
| **Blargg `bits-bank1`** (repo screenshot: *failing*) | ROM-bank register bit behaviour | One of the repo's four known-failing screenshots — a real banking/bit gap. |
| **ax6 [`rtc3test`](https://github.com/aaaaaa123456789/rtc3test)** (`rtc3test-1/2/3` in the shootout) | The MBC3 RTC end to end: tick interval to 1 ms, register ranges and invalid-value rollovers, sub-second write behaviour to 1.5 ms | A clock that is not driven by emulated time fails all three. **3/3** — not in CI (screenshot-judged, and neither the ROM nor its reference is ours to vendor). |
| **CasualPokePlayer `latch-rtc-test`, `rtc-invalid-banks-test`, `ramg-mbc3-test`** | Latching on a single write; RAM-bank values `0x0D-0x0F`; the width of the RAM-enable gate | **3/3**, same caveat. `latch-rtc-test` is the authority for "any write latches". |

The declared `retrio/gb-test-roms` submodule (Blargg) is **uninitialised**; Mooneye's
`emulator-only/` MBC tests are the definitive suite here and are **not wired**. See
[testing](../testing.md).
