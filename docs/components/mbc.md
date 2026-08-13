# Cartridges — Memory Bank Controllers (MBC)

> Source: [`src/mbc/`](../../src/mbc) · Related: [MMU](mmu.md)
> [← Component index](README.md) · [🗺️ Code map](../codemap.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/gameboy/tree/main); line references documented as of commit [`f333968`](https://github.com/Alchemy86/gameboy/tree/f333968b3a9f0513bc53fb29d52a600562195ca0) · regenerate with `make docs-links`, verify with `make docs-check`.

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

- **No MBC / "ROM only" (type `0x00`).** Up to 32 KB, no banking, no controller. Early/
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
0x00        → MBC0     (mod.rs:51)
0x01..=0x03 → MBC1     (mod.rs:54)
0x05..=0x06 → MBC2     (mod.rs:56)
0x0F..=0x13 → MBC3     (mod.rs:58)
0x19..=0x1E → MBC5     (mod.rs:60)
```

Each controller: **MBC0** (no banking, [`mbc0.rs:4`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc0.rs#L4)); **MBC1**
(5-bit + 2-bit bank registers, mode select, bank-0 remap;
[`mbc1.rs:99–186`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc1.rs#L99)); **MBC2** (512-byte nibble RAM masked with
`| 0xF0`; [`mbc2.rs:27`,`72`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc2.rs#L27)); **MBC3** (RTC via the host clock —
see below; [`mbc3.rs`](../../src/mbc/mbc3.rs)); **MBC5** (9-bit ROM bank split across
`0x2000` and `0x3000`; [`mbc5.rs:90`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc5.rs#L90)).

**Battery saves** live in [`src/mbc/battery.rs`](../../src/mbc/battery.rs), shared by MBC1/2/3/5. A `Battery`
exists only for a battery cart subtype *with* a real filepath and a non-zero declared RAM
size; otherwise it is `Battery::none()` and persists nothing (the web and FFI builds).

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
- **File formats.** `<rom>.sav` is **raw cartridge RAM, no header** — the universal format,
  interchangeable with other emulators in both directions. MBC3's clock goes in a separate
  `<rom>.rtc` sidecar (48 bytes little-endian: 5 × `u32` current registers, 5 × `u32`
  latched, `u64` UNIX timestamp — the VBA-M/mGBA layout). On load a `.sav` of
  `ramsize + 48` bytes is also accepted, which is how VBA-M and mGBA append that same block.
  The legacy `<rom>.gbsave` (8-byte big-endian RTC anchor + RAM) is still **read** when no
  `.sav` exists and is migrated to `.sav` on the next flush; the old file is left on disk
  untouched as a backup and never written again.
- **A save that cannot be read is never overwritten.** A read error poisons the battery so
  later writes are refused, and construction fails loudly rather than silently starting a
  new game on top of an existing save.

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
- **MBC3 RTC uses the host wall clock**, not tick counters. `rtc_zero` is a Unix-epoch
  baseline and registers are derived from `SystemTime::now()`
  ([`mbc3.rs:118`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc3.rs#L118)); it **panics if the system clock predates the
  epoch** ([`mbc3.rs:156`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc3.rs#L156)). Sub-second precision and the exact
  halt/latch cycle behaviour are approximate.
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
- **The RTC is not persisted for a timer-only cart (`0x0F`, TIMER+BATTERY, no RAM).** The
  battery keys off cartridge RAM size, so a `0x0F` cart gets no files at all. No commercial
  cart uses `0x0F`.
- **No MBC1M multicart detection** — only the mode-1 `& 0xE0` bank-0 remap
  ([`mbc1.rs:104`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc1.rs#L104)) is present; true multicart masking (`& 0x30`) is
  absent.

## 5. Known quirks and edge cases

- **RAM enable gate.** Cartridge RAM only responds after the game writes `0x0A` to the
  enable range; otherwise reads return open-bus. Modelled per-controller.
- **MBC1 bank-0 remap.** Writing bank `0` to the low register selects bank `1`
  ([`mbc1.rs:135`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc1.rs#L135)); in mode 1 the `0x0000–0x3FFF` region can show a
  non-zero bank ([`mbc1.rs:104`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc1.rs#L104)).
- **MBC2's 4-bit RAM.** Only the low nibble of each of 512 bytes is real; reads force the
  high nibble to `1` ([`mbc2.rs:72`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc2.rs#L72)).
- **MBC3 RTC latch.** Writing `0x00` then `0x01` to `0x6000–0x7FFF` latches the current
  time into readable registers ([`mbc3.rs:113`,`229`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc3.rs#L113)); the day
  counter's carry bit sets at 512 days ([`mbc3.rs:144`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc3.rs#L144)).
- **MBC5 9-bit bank.** `0x2000–0x2FFF` sets the low 8 bits, `0x3000–0x3FFF` bit 0 is the
  9th ([`mbc5.rs:93–98`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mbc5.rs#L93)).

## 6. Expected behaviours as tests

> **Coverage today: the two header-checksum tests (#1) and the battery-save durability
> harness (#9–#14, `testharness/battery.rs`).** All *banking* behaviour is still untested —
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
7. **MBC3 RTC latch.** After a latch sequence, the seconds register reads a stable value
   until the next latch. *(Not covered.)*
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
15. **MBC3 RTC sidecar round-trip.** After a save and reload the clock reads the stored time
    plus the wall time that elapsed while the emulator was closed. *(Not covered — no
    battery-RTC cart is available to the harness; the `.rtc` encode/decode is unit-tested in
    [`battery.rs`](../../src/mbc/battery.rs).)*

## 7. Test ROMs that exercise it

| Suite | What it checks | Meaning of a failure |
|---|---|---|
| **Mooneye `emulator-only/mbc1/`** (`bits_bank1`, `bits_ramg`, `rom_512kb`, `multicart_rom_8Mb`, …) | Every MBC1 register bit, sizes, multicart | Precisely targets §4/§5 — the multicart test is an expected red. |
| **Mooneye `emulator-only/mbc2/`, `mbc5/`** | MBC2 nibble RAM + register bits; MBC5 9-bit bank | Banking maths errors. |
| **Blargg `bits-bank1`** (repo screenshot: *failing*) | ROM-bank register bit behaviour | One of the repo's four known-failing screenshots — a real banking/bit gap. |
| *(No dedicated MBC3-RTC ROM in the common suites)* | — | RTC accuracy would need a custom test given the wall-clock implementation. |

The declared `retrio/gb-test-roms` submodule (Blargg) is **uninitialised**; Mooneye's
`emulator-only/` MBC tests are the definitive suite here and are **not wired**. See
[testing](../testing.md).
