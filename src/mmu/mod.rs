mod oamdma;
pub(crate) mod serial;
mod timer;

use crate::gpu::{Gpu, OamBug};
use crate::input::{InputQueue, Keypad};
use crate::mbc;
use crate::mmu::oamdma::OamDma;
use crate::mmu::serial::Serial;
use crate::mmu::timer::Timer;
use crate::mode::{GbMode, GbSpeed, PpuRevision};
use crate::sound::Sound;
use std::path;

pub type StrResult<T> = Result<T, &'static str>;

/// Dots between where the batched PPU stands at the top of an instruction and
/// where the hardware's OAM scan actually is when that instruction's first
/// M-cycle hits the bus. The PPU is stepped once per instruction, and enabling
/// the LCD parks it a fixed distance from a real DMG's phase; this constant is
/// what reconciles the two. Calibrated against Blargg `oam_bug/4-scanline_timing`,
/// which brackets the 19-M-cycle corruption window to a single M-cycle at both
/// ends — change it and that test moves.
///
/// **It exists only because the PPU is batched.** The whole 12 dots are an
/// `ldh [LCDC],a` enabling the LCD: the write sets `modeclock = 4` and the
/// batched renderer then applies the instruction's *entire* twelve dots on top,
/// parking it three M-cycles past where hardware is. The accurate engine has
/// already spent those dots access by access, so its remainder is zero and it
/// lands on 4 — the real figure — which is why [`ACCURATE_OAM_BUG_PHASE`] is 0
/// rather than a second calibration. Measured, not assumed: with `-12` the
/// accurate engine fails `oam_bug` 4/5/6/8, and with `0` it passes them.
const OAM_BUG_PHASE: i32 = -12;

/// The same correction for the per-dot engine: none. See [`OAM_BUG_PHASE`].
const ACCURATE_OAM_BUG_PHASE: i32 = 0;

/// `$FE00-$FEFF`: OAM proper plus the prohibited region above it. The whole
/// page shares OAM's bus, which is why the corruption bug does not stop at
/// `$FE9F`.
#[inline]
fn is_oam_page(address: u16) -> bool {
    address & 0xFF00 == 0xFE00
}

/// How a PPU-bound write lands within its own M-cycle, for the accurate
/// engine: how many of the cycle's dots run **after** the value commits
/// (`early`), and — for the registers with a settling glitch — a one-dot
/// intermediate value the PPU sees before the real one (`stage`).
///
/// The physical model, read from docboy (`src/docboy/docboy/cpu/cpu.cpp`,
/// reimplemented per CONTRIBUTING.md): the CPU flushes a write on the
/// **second** T-cycle of the M-cycle (`flush_write()` in
/// `tick_single_speed_1`), so up to three of the cycle's four dots see the new
/// value — where a plain `bus_tick` before the access, which is what every
/// other write here gets, commits at the cycle's end. On top of that sit
/// per-register settling delays docboy documents against the very Mealybug
/// rows that measure them; each is cited at its arm below.
struct WriteCommit {
    /// Dots of the write M-cycle that run after the commit point.
    early: u32,
    /// A value the register holds for the first post-commit dot — the DMG's
    /// palette-OR glitch and the per-bit LCDC settling delay are this shape.
    stage: Option<u8>,
    /// The value the commit itself lands (`None` = the written value). Used
    /// when some bits of a register commit on a *later* schedule than the
    /// rest: the commit carries the old bits, `finalize` brings the real ones.
    dispatch: Option<u8>,
    /// A value written after the M-cycle's remaining dots have run — bits that
    /// only commit when the cycle closes.
    finalize: Option<u8>,
}

const NO_COMMIT: WriteCommit = WriteCommit {
    early: 0,
    stage: None,
    dispatch: None,
    finalize: None,
};

/// LCDC bits the DMG's PPU picks up one dot later than the rest of the
/// register: window enable, OBJ enable, BG enable (docboy `ppu.cpp`, cited
/// against `m3_lcdc_win_en_change_multiple_wx`, `m3_lcdc_obj_en_change` and
/// `m3_lcdc_bg_en_change` respectively).
const LCDC_DMG_STAGE_BITS: u8 = 0b0010_0011;

/// And the window enable is later still: it commits only when the write's
/// M-cycle closes. Measured, not taken from docboy — with it on the stage
/// schedule `m3_lcdc_win_en_change_multiple` is 521 pixels out; at
/// end-of-cycle it is pixel-exact against all three hardware references.
const LCDC_DMG_END_BITS: u8 = 0b0010_0000;

/// Where a write to `address` commits within its M-cycle. `old` is the
/// register's value before the write. Everything not named here — STAT, LYC,
/// VRAM, OAM, the CGB palette data ports — keeps the end-of-cycle commit the
/// bus has always had: each is either interrupt machinery Mooneye pins at
/// end-of-cycle calibration, or (VRAM tile data) a mid-fetch glitch of its own
/// that is not yet modelled; widening this table is a measurement, not an
/// edit.
fn ppu_write_commit(
    address: u16,
    old: u8,
    value: u8,
    mode: GbMode,
    rev: PpuRevision,
) -> WriteCommit {
    let dmg = mode.is_dmg();
    match address {
        // The DMG's BGP write glitch: for one dot after the write commits the
        // PPU resolves colours through `old | new` (docboy `ppu.cpp`: "On DMG
        // there is 1 T-cycle delay between BGP and the BGP value the PPU
        // sees. During this T-Cycle, the PPU sees the last BGP ORed with the
        // new one. [mealybug/m3_bgp_change] Verified: CGB is not affected."
        // — including a CGB running this DMG cartridge, hence `is_dmg` and
        // not "rendering as a DMG"). Measured here: the DMG column of
        // `m3_bgp_change` is 2,244 pixels without this and pixel-exact with
        // it (22 before the line-0 strobe fix in `Gpu::oam_strobe`).
        0xFF47..=0xFF49 if dmg => WriteCommit {
            early: 3,
            stage: Some(old | value),
            dispatch: None,
            finalize: None,
        },
        // On colour hardware a palette write is seen one dot later on a
        // **CPU CGB C** than on a CPU CGB D — measured against the suite's own
        // two CGB reference sets, which differ by exactly that one dot on
        // every row a mid-line palette write reaches (`docs/mealybug.md` §10).
        0xFF47..=0xFF49 => WriteCommit {
            early: if rev.late_palette_commit() { 3 } else { 2 },
            stage: None,
            dispatch: None,
            finalize: None,
        },
        // LCDC. The write that turns the LCD **on** is exempt — the restart
        // phase it establishes is the anchor every Mooneye interrupt/STAT
        // figure was calibrated against, and committing it early shifts the
        // whole PPU three dots against the CPU, which (measured) exactly
        // cancels the early commit of every later mid-line write.
        0xFF40 if value & 0x80 != 0 && old & 0x80 == 0 => NO_COMMIT,
        // On a DMG most of LCDC is seen from the commit dot; the BG and OBJ
        // enables land one dot later (the stage), and the window enable not
        // until the cycle closes — measured: `m3_lcdc_win_en_change_multiple`
        // is pixel-exact with the end-of-cycle window enable and 521 pixels
        // out with it on the stage schedule.
        0xFF40 if dmg => WriteCommit {
            early: 3,
            stage: Some((old & LCDC_DMG_STAGE_BITS) | (value & !LCDC_DMG_STAGE_BITS)),
            dispatch: Some((old & LCDC_DMG_END_BITS) | (value & !LCDC_DMG_END_BITS)),
            finalize: Some(value),
        },
        // Colour hardware sees LCDC two dots later than the DMG (docboy:
        // "CGB is not affected — it sees LCDC delayed by 2 T-Cycles by
        // default"), i.e. one dot of the cycle after visibility — except the
        // BG-map select, which measures a dot later still on our fetcher
        // (`m3_lcdc_bg_map_change` CGB 522 at end-of-cycle vs 730 a dot
        // earlier; the tile-data select in the same register measures the
        // other way, 849 vs 979 on `m3_lcdc_tile_sel_change2`).
        0xFF40 => WriteCommit {
            early: 1,
            stage: Some((old & 0b0000_1000) | (value & !0b0000_1000)),
            dispatch: None,
            finalize: None,
        },
        // Scroll registers: commit with the write flush on a DMG. On colour
        // hardware the end-of-cycle commit measures better on every affected
        // row (`m3_scy_change` CGB-C 546 vs 4,843, `m3_scx_high_5_bits` CGB
        // 12 vs 75) — consistent with docboy's "the CGB sees the register
        // file two T-cycles later than the DMG".
        0xFF42..=0xFF43 if dmg => WriteCommit {
            early: 3,
            stage: None,
            dispatch: None,
            finalize: None,
        },
        0xFF42..=0xFF43 => NO_COMMIT,
        // WX is seen one dot late on both consoles (docboy: "there is 1
        // T-cycle delay between WX and the WX value the PPU sees",
        // `m3_wx_5_change`).
        0xFF4B => WriteCommit {
            early: 2,
            stage: None,
            dispatch: None,
            finalize: None,
        },
        _ => NO_COMMIT,
    }
}

/// The most M-cycles [`MemoryManagementUnit::idle_mcycles`] will ever hand back.
///
/// The PPU horizon is never longer than a mode-3 span (172 dots, 43 M-cycles)
/// anyway, so this only bounds the cases where the PPU imposes nothing. It is
/// deliberately no larger than a long instruction's reach so that nothing
/// downstream — the APU's resampler above all — is ever handed a step it does
/// not already see in ordinary play.
const IDLE_BATCH_MAX: u32 = 64;

const WRAM_SIZE: usize = 0x8000;
const ZRAM_SIZE: usize = 0x7F;

/// The 16-bit system counter as the first cartridge instruction observes it on
/// a DMG (`DIV` therefore reads `$AB`). Measured by Mooneye
/// `acceptance/boot_div-dmgABCmgb`, which pins six successive `DIV` reads and
/// so fixes the counter's low bits as well as `DIV` itself, and cross-checked
/// by `acceptance/serial/boot_sclk_align-dmgABCmgb`, which measures the serial
/// shift clock's phase against the same counter.
const DMG_BOOT_SYSTEM_COUNTER: u16 = 0xABCC;

/// The value for a Color console running a **DMG-header** cartridge —
/// `GbMode::ColorAsClassic` — measured by Mooneye `misc/boot_div-cgbABCDE`,
/// whose cartridge header is DMG. That ROM measures the *compatibility* path
/// through the boot ROM, which a genuine CGB cartridge does not take; see
/// [`CGB_BOOT_SYSTEM_COUNTER`] for that one.
const CGB_COMPAT_BOOT_SYSTEM_COUNTER: u16 = 0x2678;

/// The system counter for a Color console running a **genuine CGB-flagged**
/// cartridge — `GbMode::Color`, the boot path `CGB_COMPAT_BOOT_SYSTEM_COUNTER`
/// above explicitly does *not* cover. Derived, not swept, from two rows of
/// gambatte's own `div/start_inc` (`test/hwtests/div/start_inc_{1,2}
/// _cgb04c_out{1E,1F}.asm`, both `$0143 = $C0`): each subtest is `jp; jp; NOP
/// ×N; LDH A,($04)` from `$0100` with `N` = 13 and 14, so the two pin both
/// `DIV`'s value *and* the exact M-cycle its low byte wraps on. Solved in
/// closed form (`4 * M-cycles-to-the-read` must land the stored counter one
/// step short of the wrap for `N=13` and past it for `N=14`) and confirmed by
/// running both ROMs against the result: `0x1EA0` reads back `$1E` after 13
/// `NOP`s and `$1F` after 14, exactly.
///
/// **A second suite ROM independently corroborates the same ballpark.**
/// BullyGB's header is `$80` (CGB **and** DMG capable, so it takes this same
/// genuine path when booted on a Color console) and its very first check
/// requires `DIV = $1F` a fixed distance after hand-off; with the old, shared
/// `0x2678` we read `$27` and it prints `Invalid initial DIV`. A blind sweep
/// against BullyGB alone had already found `$1DC0`-ish satisfies it (see the
/// history of this constant), which is within `$E0` of the value derived here
/// from an unrelated ROM — BullyGB waits for `LY >= $90` first, so its read
/// lands noticeably later than gambatte's fixed 13/14-`NOP` window, and the
/// gap between the two figures is exactly the extra time that wait spends.
/// Two independent measurements landing this close is what makes `0x1EA0`
/// trustworthy rather than a fit to one row — the objection recorded against
/// touching this constant on BullyGB's evidence alone (see conformance notes)
/// no longer applies once gambatte's own hwtests are read as a second source.
/// BullyGB is not gated by any suite here and was not re-verified end to end
/// (its check also depends on the PPU's boot phase, which this does not
/// touch); it is cited as corroboration, not as a fixed row.
const CGB_BOOT_SYSTEM_COUNTER: u16 = 0x1EA0;

/// The first DMG revision, from `acceptance/boot_div-dmg0`. Its boot ROM is a
/// shorter program than the later revisions', so it hands over with the counter
/// nearly `$9400` earlier.
const DMG0_BOOT_SYSTEM_COUNTER: u16 = 0x1830;

/// Where the first DMG's boot ROM leaves the PPU — see [`boot_ppu_dot`].
///
/// `145 * 456 + 180`: early in the second VBlank line. **Calibrated, with the
/// window stated:** `acceptance/boot_hwio-dmg0` passes for a hand-off anywhere
/// in `66216..=66380` (line 145, dots 96 to 260) and fails on both sides, so
/// this is the centre of a 168-dot band, not a fitted number. Neither edge is
/// hardware: the low one is where our fixed 172-dot mode 3 ends and the ROM's
/// `STAT` read stops seeing mode 3, and the high one is where its `LY` read
/// rolls to the next line.
///
/// **Why line 145 is the right neighbourhood, independently of the ROM.** Both
/// DMG boot ROMs end their logo animation on a delay loop synchronised to
/// `LY == 144` ([ISSOtm/gb-bootroms](https://codeberg.org/ISSOtm/gb-bootroms)
/// `src/dmg.asm`). The later revisions then run `CheckLogo` — the 48-byte logo
/// comparison and the header checksum — before handing over; the first revision
/// has no such code and jumps straight to `Done`. So DMG 0 hands the cartridge
/// a PPU that is still in VBlank, and DMG ABC one that has had nine more
/// scanlines to run. That is precisely the nine-line gap between
/// `boot_hwio-dmg0`'s `LY = $01` and `boot_hwio-dmgABCmgb`'s `LY = $0A`.
const DMG0_BOOT_PPU_DOT: u32 = 145 * 456 + 180;

/// The first CGB revision, from `misc/boot_div-cgb0`.
const CGB0_BOOT_SYSTEM_COUNTER: u16 = 0x2884;

/// A Game Boy Advance, from `misc/boot_div-A`. Four T-cycles longer than a
/// CGB's — the same `inc b` that shows up in `misc/boot_regs-A`'s `B` and `F`.
const AGB_BOOT_SYSTEM_COUNTER: u16 = 0x267C;

/// The Super Game Boy's boot ROM does not take a fixed time: it hands the
/// cartridge header to the SNES over the link port before starting the game,
/// and how long that takes depends on the header bytes. This is the duration
/// with **every transmitted bit set**; [`sgb_boot_system_counter`] adds the
/// cost of the zero bits.
///
/// Derived from `acceptance/boot_div-S`, whose comment states the dependency
/// outright ("Bootrom duration on real SGB/SGB2 depends on the header bytes,
/// including the global checksum") and whose sibling `boot_div2-S` exists to
/// catch an emulator that hard-codes one number. The two ROMs differ only in
/// their global checksum and yield the *same* value for this constant, which is
/// the check that the model below is the right shape.
const SGB_BOOT_SYSTEM_COUNTER_ALL_ONES: u16 = 0xD174;

/// How long a Super Game Boy's boot ROM takes, in system-counter ticks, for
/// this cartridge — see [`SGB_BOOT_SYSTEM_COUNTER_ALL_ONES`].
///
/// **The mechanism, from the boot ROM itself.** Before it hands over, an SGB
/// packs the cartridge header (`$0104-$014F`, 76 bytes) into six 16-byte
/// packets — each carrying a packet id and a checksum over its own share of the
/// header, with the last packet zero-padded — and clocks all 96 bytes to the
/// SNES one bit at a time over `P1`. The bit loop is *not* balanced: a set bit
/// takes a taken `jr`, a clear bit takes the fall-through **plus** a second
/// `ld a`, so **a zero bit costs one M-cycle more than a one bit**. The
/// disassembly's own comment on that instruction is "Both code paths aren't
/// equal CPU time" ([ISSOtm/gb-bootroms](https://codeberg.org/ISSOtm/gb-bootroms)
/// `src/sgb.asm`, `.sendBit`).
///
/// That is the whole of the data dependency, and it is why `boot_div-S` and
/// `boot_div2-S` — identical ROMs but for two header bytes — expect the same
/// six `DIV` readings at read points four M-cycles apart.
/// Where the PPU sits in its 70224-dot frame when the boot ROM hands over, or
/// `None` to leave it where the `LCDC` write in
/// [`MemoryManagementUnit::set_initial`] puts it (the start of line 0).
///
/// See [`crate::gpu::Gpu::set_boot_position`]. Only the first DMG gets one:
/// every other console this emulator models either measures line 0 (`DMG ABC`,
/// via `boot_hwio-dmgABCmgb`'s `LY = $0A`) or does not measure `LY`/`STAT` at
/// all (`boot_hwio-S` and `misc/boot_hwio-C` both skip `$FF40`, `$FF41` and
/// `$FF44` — see their own masks).
///
/// **Correction, 2026-08-15: a suite DOES measure it on a plain DMG.**
/// GBMicrotest's `poweron_ly_*` and `poweron_stat_*` are nothing but nine to
/// two hundred `nop`s followed by `LDH A,[$FF44]` / `[$FF41]` and a `cp`, so
/// every one of them pins where the PPU was when the cartridge started. They
/// disagree with us: hardware reads `LY = 0` at 119 M-cycles and `1` at 120,
/// where we roll over at 114, and hardware's `STAT` is still mode 1 for the
/// first six M-cycles where ours is already mode 0. **The constant was
/// nevertheless left alone, deliberately.** Sweeping it across the whole frame
/// scores 237/513 at 69900, 251 at line 0 (today), 262 at 70168 and 250 at
/// 70184 — a jagged profile with no clean optimum, which is the signature of a
/// constant absorbing an error somewhere else rather than a value being found.
/// See `docs/measured/scoreboard.md`, "The c-sp aggregation".
/// The buses a Game Boy memory access can land on.
///
/// `None` is not a bus — it is "this address is internal to the chip" (OAM,
/// the I/O block, HRAM and `IE`), which is why an OAM DMA routine is copied to
/// HRAM before it is run. See [`MemoryManagementUnit::dma_conflict`].
///
/// **A DMG has two buses and a Color console has three.** On a DMG the work
/// RAM hangs off the same external bus as the cartridge, so a DMA out of WRAM
/// blocks the cartridge and vice versa; on a CGB the work RAM has a bus of its
/// own and the two do not interfere. Gambatte's `oamdma_srcC000_busypop7FFF`
/// is the one-row proof: with a DMA running out of `$C000` it expects the DMG
/// to pop the DMA's byte and the Color console to pop the real ROM byte.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum DmaBus {
    None,
    /// Cartridge ROM and cartridge RAM — plus work RAM on a DMG.
    External,
    /// Video RAM.
    Video,
    /// Work RAM and its echo, on a Color console only.
    Wram,
}

/// What a transfer latches into OAM when a CPU **write** shares its bus.
///
/// Four answers, and they are read off Gambatte's `oamdma_*_busypush*` rows
/// rather than argued from first principles — see
/// [`MemoryManagementUnit::dma_conflict`].
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum DmaWrite {
    /// The CPU's own byte. The cartridge bus is in write mode, so nothing else
    /// is driving it and the transfer copies what the CPU put there.
    Cpu,
    /// The CPU's byte **and** the source byte, wired together. A DMG's work RAM
    /// is inside the SoC and keeps driving through a write cycle, so a transfer
    /// reading work RAM sees both — `oamdma_srcC000_busypushC001` expects
    /// `$65 & $55 = $45` where the cartridge cases expect a plain `$55`.
    And,
    /// The source byte: the CPU is locked out and the transfer is undisturbed.
    /// A Color console's own work-RAM bus behaves this way.
    Source,
    /// Zero — a Color console's video bus.
    Zero,
}

/// Horizontally double a 4-bit value into an 8-bit one: bit 3 (the leftmost
/// of the nibble's 4 source pixels) becomes output bits 7 and 6, bit 2
/// becomes bits 5 and 4, and so on. This is the "chunky pixel" doubling the
/// monochrome boot ROMs apply to the header logo — see
/// [`MemoryManagementUnit::set_boot_logo_tiles`].
fn double_nibble(n: u8) -> u8 {
    let mut out = 0u8;
    for bit in 0..4u8 {
        let v = (n >> (3 - bit)) & 1;
        out |= v << (7 - bit * 2);
        out |= v << (6 - bit * 2);
    }
    out
}

fn boot_ppu_dot(console: crate::mode::Model) -> Option<u32> {
    match console {
        crate::mode::Model::Dmg0 => Some(DMG0_BOOT_PPU_DOT),
        _ => None,
    }
}

fn sgb_boot_system_counter(header: &dyn Fn(u16) -> u8) -> u16 {
    // The six packets, as `.copyHeader` builds them: the first carries the six
    // bytes from `$014F` downwards, the rest fourteen each.
    let mut zero_bits: u32 = 0;
    let mut count = |byte: u8| zero_bits += byte.count_zeros();
    let mut addr: u16 = 0x014F;
    for (i, id) in [0xFBu8, 0xF9, 0xF7, 0xF5, 0xF3, 0xF1]
        .into_iter()
        .enumerate()
    {
        let len = if i == 0 { 6 } else { 14 };
        let mut checksum: u8 = 0;
        count(id);
        for _ in 0..len {
            let byte = header(addr);
            addr = addr.wrapping_sub(1);
            checksum = checksum.wrapping_add(byte);
            count(byte);
        }
        count(checksum);
    }
    // `.clearBuffer` zeroes the eight bytes the header does not fill, and they
    // are clocked out with everything else — 64 bits, all of them slow ones.
    zero_bits += 64;
    SGB_BOOT_SYSTEM_COUNTER_ALL_ONES.wrapping_add(4 * zero_bits as u16)
}

#[derive(PartialEq)]
enum DMAType {
    NoDma,
    Gdma,
    Hdma,
}

impl DMAType {
    fn to_u8(&self) -> u8 {
        match self {
            DMAType::NoDma => 0,
            DMAType::Gdma => 1,
            DMAType::Hdma => 2,
        }
    }

    fn from_u8(v: u8) -> Result<DMAType, crate::savestate::StateError> {
        match v {
            0 => Ok(DMAType::NoDma),
            1 => Ok(DMAType::Gdma),
            2 => Ok(DMAType::Hdma),
            _ => Err(crate::savestate::StateError::BadField("DMAType")),
        }
    }
}

pub struct MemoryManagementUnit<'a> {
    wram: [u8; WRAM_SIZE],
    zram: [u8; ZRAM_SIZE],
    pub inte: u8,
    pub intf: u8,
    pub serial: Serial<'a>,
    pub timer: Timer,
    /// OAM DMA ($FF46) — a timed 160 M-cycle transfer, see [`oamdma`].
    oam_dma: OamDma,
    /// Which bus OAM DMA is driving **this M-cycle**, or [`DmaBus::None`] when
    /// no transfer is moving a byte. See [`MemoryManagementUnit::dma_conflict`].
    dma_bus: DmaBus,
    /// The byte OAM DMA put on that bus this M-cycle, and the OAM index it went
    /// to. Only meaningful while `dma_bus` is not `None`.
    dma_byte: u8,
    dma_index: u8,
    /// What a conflicting CPU write latches into OAM this M-cycle.
    dma_write: DmaWrite,
    /// Bit 12 of the address an OAM DMA is currently driving, published
    /// alongside [`Self::dma_bus`] and consumed by
    /// [`Self::wram_bus_address`]. Not serialised, for the same reason
    /// `dma_bus` is not: it is republished before anything can read it.
    dma_a12: bool,
    /// CPU cycles the APU has been handed but not yet told about, and the
    /// budget at which they must be — see [`Self::flush_apu`].
    apu_pending: u32,
    apu_budget: u32,
    /// `!oam_dma.idle()`, cached — see [`Self::bus_tick`].
    oam_dma_busy: bool,
    /// Dots the PPU has been promised but not yet given, and the distance to
    /// the next dot at which it could do something observable — see
    /// [`Self::flush_gpu`].
    gpu_owed: u32,
    gpu_horizon: u32,
    /// The cartridge ROM, shared with the mapper rather than copied, and the
    /// byte offsets of the two windows the CPU sees — see [`Self::read_rom`].
    rom_share: std::sync::Arc<[u8]>,
    rom_lo: usize,
    rom_hi: usize,
    /// True on Color silicon, where the work RAM has a bus of its own rather
    /// than hanging off the cartridge bus. Cached from `gbmode` because
    /// [`Self::cpu_bus_of`] is on the memory-access path. Derived state, so it
    /// is not serialised.
    wram_own_bus: bool,
    pub keypad: Keypad,
    /// The Super Game Boy's ICD2, listening on `$FF00`. `None` on every other
    /// console — see [`crate::sgb`] for what it does and what it deliberately
    /// does not.
    pub sgb: Option<crate::sgb::Sgb>,
    /// Scheduled button presses from outside the machine — see
    /// [`crate::input::InputQueue`]. Empty unless something is driving the
    /// game programmatically, and **not** part of a save state.
    pub input_queue: InputQueue,
    pub gpu: Gpu,
    /// The APU, once a frontend has installed a player. `None` is **not** a
    /// quiet Game Boy — see [`crate::sound::SilentPlayer`]. Present on every
    /// target including `wasm32`.
    pub sound: Option<Sound>,
    hdma_status: DMAType,
    /// HDMA1/2 and HDMA3/4 are not a shadow of the registers — they *are* the
    /// registers, and the transfer advances them. That is what lets a second
    /// `$FF55` write with no re-write of HDMA1-4 continue where the last
    /// transfer stopped (SameSuite `dma/gbc_dma_cont`).
    hdma_src: u16,
    hdma_dst: u16,
    hdma_len: u8,
    /// One-shot: an HBlank transfer armed while the PPU was already in mode 0
    /// moves its first block immediately rather than waiting for the *next*
    /// mode-0 entry (SameSuite `dma/hdma_mode0`, `dma/hdma_lcd_off`).
    hdma_run_block: bool,
    wrambank: usize,
    pub mbc: Box<dyn mbc::MemoryBankController + 'static>,
    pub gbmode: GbMode,
    /// Which physical console this is, with [`Model::Auto`] already resolved.
    ///
    /// [`GbMode`] is what the *cartridge* sees and is enough for everything
    /// after `PC=$0100`; this is the console **revision**, and it exists because
    /// boot state is not a function of `GbMode` — a DMG0, a DMG and an MGB are
    /// all `GbMode::Classic` and hand over three different machines. Consumed
    /// by [`Self::set_initial`] and by `Registers::boot`, and deliberately
    /// **not** serialised: like `Sound::cgb_revision` it is a property of the
    /// machine a state is loaded *into*, not of the state.
    pub console: crate::mode::Model,
    gbspeed: GbSpeed,
    speed_switch_req: bool,
    undocumented_cgb_regs: [u8; 3], // 0xFF72, 0xFF73, 0xFF75
    /// The user's own boot ROM, if they supplied one and it belongs to this
    /// console. `None` is the ordinary case and the one every checked-in
    /// baseline is measured in — see [`crate::boot`].
    boot: Option<crate::boot::BootRom>,
    /// Is that boot ROM still mapped over the cartridge? Set at construction
    /// and cleared for good by the first `$FF50` write with bit 0 set. It is
    /// live machine state — a save taken during the boot animation has to
    /// resume inside it — so it is serialised.
    boot_active: bool,
    // --- Sub-instruction memory-access timing (Blargg mem_timing) ---
    //
    // The CPU executes a whole instruction "atomically" and then advances the
    // peripherals by the instruction's full length. That is fine for the PPU
    // (which nothing samples mid-instruction) but wrong for the *timer*: an
    // instruction that reads TIMA reads it on its LAST M-cycle on hardware, not
    // its first. To model that without a full cycle-stepped CPU, we advance the
    // timer by one M-cycle (4 T-cycles) at each CPU memory access, *before* the
    // access is performed, so a read/write sees the timer as of that cycle. The
    // remaining "internal" cycles of the instruction are applied in `do_cycle`.
    //
    // Only the timer is stepped per-access; the GPU/sound/DMA stay batched, so
    // frame rendering is byte-for-byte unchanged. `bus_tick_enabled` is true
    // only while a CPU instruction is executing (not during OAM DMA or setup).
    bus_tick_enabled: bool,
    /// Which shortcuts this machine is taking — see [`crate::speed::SpeedFlags`].
    /// Machine configuration, not machine state, so deliberately not
    /// serialised: a state saved by a fast run loads into an accurate machine
    /// and vice versa.
    speed: crate::speed::SpeedFlags,
    /// Whether the peripherals really do advance once per *access* — false when
    /// `instruction-bus` is on, where they advance once per instruction.
    ///
    /// This is the one liberty a certification run may take that identical mode
    /// may not. The totals are unchanged (the ticks skipped here are applied at
    /// the instruction's end, because `bus_ticked` stays zero), so `DIV`,
    /// `TIMA`, the serial clock and the APU all still run at exactly the right
    /// rate; what is lost is *which sub-instruction M-cycle* an access lands
    /// on, which is what Blargg `mem_timing` measures. Machine configuration,
    /// not machine state — deliberately not serialised, so a state saved by a
    /// speed-mode run loads into an accurate machine and vice versa.
    per_access_bus: bool,
    /// Whether the peripherals are advanced once per **horizon** rather than
    /// once per instruction — see [`crate::speed::SpeedFlags::horizon_batch`].
    /// Implies `!per_access_bus`: a horizon computed at the last flush is only
    /// valid while nothing advances a peripheral behind its back.
    horizon_batch: bool,
    /// CPU T-cycles executed but not yet handed to the peripherals. Zero
    /// unless [`Self::horizon_batch`] is on, and emptied by
    /// [`Self::flush_pending`] before anything — a register read, an interrupt
    /// dispatch, a save state — can observe that they are behind.
    pending_ticks: u32,
    /// PPU ticks [`Self::do_cycle`] owes its caller from a flush that happened
    /// part-way through an instruction, so `Gameboy::frame`'s cycle accounting
    /// is unchanged by where a flush landed.
    pending_out: u32,
    /// CPU T-cycles that may be batched before **anything** the CPU can see
    /// could change. `0` means "do not batch", which is also what it holds
    /// whenever the flag is off — so the same comparison drives both machines
    /// and there is no second code path. See [`Self::compute_run_horizon`].
    run_horizon: u32,
    bus_ticked: u32,
    /// PPU dots already spent inside this instruction's memory accesses, so
    /// [`Self::do_cycle`] applies only the instruction's remainder. Counted
    /// separately from `bus_ticked` because the PPU's clock is the CPU's
    /// divided by the double-speed multiplier. Only the accurate engine moves
    /// it; see [`Self::bus_tick`].
    gpu_bus_ticked: u32,
    /// Dots of the current write M-cycle that belong *after* the write has
    /// committed. Set by [`Self::wb`] for a PPU-bound write in accurate mode,
    /// clamped and left as the pending remainder by [`Self::bus_tick`], and
    /// consumed by `wb` immediately after the write lands. Always zero between
    /// accesses, so it is deliberately not serialised.
    gpu_defer: u32,
    /// Force the **batched** PPU to advance with the bus, as the per-dot one
    /// always does. Set only for the five M-cycles of interrupt dispatch — see
    /// [`Self::set_ppu_bus_ticking`].
    ppu_ticks_with_the_bus: bool,
    /// Set by the CPU immediately before a memory access that shares its
    /// M-cycle with a 16-bit increment/decrement — the DMG OAM-corruption bug
    /// treats that combination as its own pattern. Set and consumed inside a
    /// single instruction, so it is never live at a frame boundary and is not
    /// part of the save state. See [`MemoryManagementUnit::share_idu`].
    idu_share: bool,
    /// Why this machine is considered to have left the rails, if it has. See
    /// [`MemoryManagementUnit::report_fault`]. Not part of the save state: a
    /// fault is a fact about *this session's* host, not about the emulated
    /// machine, so it is deliberately not serialised.
    fault: Option<String>,
    /// The cartridge carries a real-time clock, so it has to be advanced every
    /// instruction. Cached from the MBC at boot (it is a property of the
    /// cartridge header, never of the run) so the common case is one bool test
    /// instead of a virtual call.
    cart_rtc: bool,
}

fn fill_random(slice: &mut [u8], start: u32) {
    // Simple LCG to generate (non-cryptographic) random values
    // Each distinct invocation should use a different start value
    const A: u32 = 1103515245;
    const C: u32 = 12345;

    let mut x = start;
    for v in slice.iter_mut() {
        x = x.wrapping_mul(A).wrapping_add(C);
        *v = ((x >> 23) & 0xFF) as u8;
    }
}

impl<'a> MemoryManagementUnit<'a> {
    /// Build the memory map with the cartridge mapped but the console not yet
    /// chosen. Every public constructor goes through here and then decides
    /// `gbmode`; keeping it in one place is what stops the DMG and CGB paths
    /// drifting apart.
    fn raw(
        data: Vec<u8>,
        file: Option<path::PathBuf>,
    ) -> StrResult<MemoryManagementUnit<'a>> {
        let mmu_mbc = mbc::get_mbc(data, file)?;
        let cart_rtc = mmu_mbc.has_rtc();
        let serial = Serial::default();
        let mut res = MemoryManagementUnit {
            wram: [0; WRAM_SIZE],
            zram: [0; ZRAM_SIZE],
            wrambank: 1,
            inte: 0,
            intf: 0,
            serial,
            timer: Timer::default(),
            oam_dma: OamDma::new(),
            dma_bus: DmaBus::None,
            dma_byte: 0xFF,
            dma_index: 0,
            dma_write: DmaWrite::Cpu,
            dma_a12: false,
            apu_pending: 0,
            apu_budget: 0,
            oam_dma_busy: false,
            gpu_owed: 0,
            gpu_horizon: 0,
            rom_share: std::sync::Arc::from(&[][..]),
            rom_lo: 0,
            rom_hi: 0,
            wram_own_bus: false,
            keypad: Keypad::default(),
            sgb: None,
            input_queue: InputQueue::default(),
            gpu: Gpu::new(),
            sound: None,
            mbc: mmu_mbc,
            gbmode: GbMode::Classic,
            console: crate::mode::Model::Dmg,
            gbspeed: GbSpeed::Single,
            speed_switch_req: false,
            hdma_src: 0,
            hdma_dst: 0,
            hdma_status: DMAType::NoDma,
            hdma_len: 0xFF,
            hdma_run_block: false,
            undocumented_cgb_regs: [0; 3],
            boot: None,
            boot_active: false,
            bus_tick_enabled: false,
            speed: crate::speed::SpeedFlags::NONE,
            per_access_bus: true,
            horizon_batch: false,
            pending_ticks: 0,
            pending_out: 0,
            run_horizon: 0,
            bus_ticked: 0,
            gpu_bus_ticked: 0,
            gpu_defer: 0,
            ppu_ticks_with_the_bus: false,
            idu_share: false,
            fault: None,
            cart_rtc,
        };
        fill_random(&mut res.wram, 42);
        res.refresh_rom_windows();
        Ok(res)
    }

    /// Boot an original Game Boy (DMG) and **refuse a CGB-only cartridge**,
    /// which is what such a cartridge's own header asks for (`0x0143 == 0xC0`
    /// means "will not run on a DMG"; on real hardware the cartridge's own boot
    /// code draws an error screen). Use
    /// [`new_with_model`](Self::new_with_model) with [`Model::Dmg`] to force it
    /// anyway.
    pub fn new(
        data: Vec<u8>,
        file: Option<path::PathBuf>,
    ) -> StrResult<MemoryManagementUnit<'a>> {
        let mut res = Self::raw(data, file)?;
        if res.rb(0x0143) == 0xC0 {
            return Err("This game does not work in Classic mode");
        }
        res.set_mode(GbMode::Classic);
        res.set_initial();
        Ok(res)
    }

    /// Boot a Game Boy Color. A cartridge that does not advertise CGB support
    /// gets the console's compatibility mode ([`GbMode::ColorAsClassic`]).
    pub fn new_cgb(
        data: Vec<u8>,
        file: Option<path::PathBuf>,
    ) -> StrResult<MemoryManagementUnit<'a>> {
        Self::new_with_model(data, file, crate::mode::Model::Cgb)
    }

    /// Boot the console named by `model`. [`Model::Auto`] picks from the
    /// cartridge header (and honours `$GB_MODEL`) — see [`Model::resolve`].
    pub fn new_with_model(
        data: Vec<u8>,
        file: Option<path::PathBuf>,
        model: crate::mode::Model,
    ) -> StrResult<MemoryManagementUnit<'a>> {
        Self::new_with_boot_rom(data, file, model, None)
    }

    /// [`Self::new_with_model`] with an optional **user-supplied boot ROM**
    /// ([`crate::boot`]).
    ///
    /// `Some(rom)` maps the image over the cartridge and leaves the machine at
    /// its reset state — every register zero, `PC = $0000`, and *none* of the
    /// hand-written handoff state, because the boot ROM is about to produce the
    /// real thing. `None`, or an image belonging to the other console, is the
    /// ordinary HLE machine and is byte-for-byte what this constructor has
    /// always built.
    ///
    /// The caller decides where the bytes came from; nothing here reads an
    /// environment variable or a path (see the layering note in
    /// [`crate::boot`]).
    pub fn new_with_boot_rom(
        data: Vec<u8>,
        file: Option<path::PathBuf>,
        model: crate::mode::Model,
        boot: Option<crate::boot::BootRom>,
    ) -> StrResult<MemoryManagementUnit<'a>> {
        let mut res = Self::raw(data, file)?;
        let cgb_flag = res.rb(0x0143);
        let console = model.console(cgb_flag);
        let mode = console.mode(cgb_flag);
        res.set_mode(mode);
        res.console = console;
        // A boot ROM for the other console is declined rather than run: it
        // would map over the wrong addresses and hand over the wrong state.
        match boot.filter(|b| b.suits(mode)) {
            Some(rom) => {
                res.boot = Some(rom);
                res.boot_active = true;
            }
            None => res.set_initial(),
        }
        Ok(res)
    }

    /// Is a user-supplied boot ROM about to run (or running)?
    ///
    /// The CPU asks this to decide whether to start at `$0000` with a blank
    /// register file or at `$0100` with the transcribed handoff state.
    pub fn boot_rom_mapped(&self) -> bool {
        self.boot_active
    }

    /// `$0000-$7FFF`: the cartridge, unless a boot ROM is still mapped over the
    /// bottom of it.
    ///
    /// The overlay is a read-only window — a write in this range is a mapper
    /// register and still reaches the cartridge, which is what the boot ROM's
    /// own logo check depends on.
    #[inline]
    /// Re-take the mapper's ROM handle and window offsets. Called at
    /// construction, after every write into `$0000-$7FFF` (the only thing that
    /// can move a window) and after `load_state`.
    fn refresh_rom_windows(&mut self) {
        if let Some(rom) = self.mbc.rom_share() {
            self.rom_share = rom;
        }
        let (lo, hi) = self.mbc.rom_window_bases();
        self.rom_lo = lo;
        self.rom_hi = hi;
    }

    fn read_rom(&self, address: u16) -> u8 {
        // The fetch path. Every opcode and every immediate comes through here,
        // and it used to be an indirect call through `Box<dyn
        // MemoryBankController>` — 10 % of a headless collector's samples. The
        // mapper still decides which bank is mapped; it just publishes the
        // answer as a byte offset (`MemoryBankController::rom_window_bases`,
        // re-read after every write into `$0000-$7FFF`) into an allocation the
        // MMU shares with it. **An index outside the ROM falls through to the
        // mapper**, so each one keeps its own open-bus value — MBC5 answers `0`
        // where the others answer `$FF`, and that difference is preserved.
        if !self.boot_active {
            let base = if address < 0x4000 {
                self.rom_lo
            } else {
                self.rom_hi
            };
            if let Some(b) = self.rom_share.get(base | (address as usize & 0x3FFF)) {
                return *b;
            }
        }
        if self.boot_active {
            if let Some(b) = self.boot.as_ref().and_then(|r| r.byte(address)) {
                return b;
            }
        }
        self.mbc.readrom(address)
    }

    /// Put the machine in the state the console's own boot ROM leaves behind at
    /// `PC=$0100`.
    ///
    /// This emulator ships no boot ROM — they are somebody else's — so by
    /// default none runs and every byte of that state has to be written here.
    /// A user who supplies their own ([`crate::boot`]) gets the real thing
    /// instead and this is not called at all; that is the only way to obtain
    /// the *video RAM* half of the handoff, which is a glyph and cannot be
    /// transcribed (see [`docs/mealybug.md`](../docs/mealybug.md) §8.6). Three
    /// Mooneye ROMs assert the *whole* of it rather than the handful of values
    /// usually quoted — `acceptance/boot_hwio-dmgABCmgb` walks `$FF00-$FF7F`
    /// plus `IE` byte by byte, and `acceptance/boot_div-dmgABCmgb` /
    /// `misc/boot_div-cgbABCDE` pin the 16-bit system counter's exact value and
    /// phase. They disagree with each other, so the values are per console.
    ///
    /// The APU's share of it is **not** here: the sound chip is installed later
    /// by [`Self::enable_audio`] (headless machines have none), so its post-boot
    /// registers live in `Sound::set_post_boot_state`. Writing them here was
    /// dead code — `self.sound` is `None` at this point and every one of those
    /// writes was silently dropped.
    fn set_initial(&mut self) {
        use crate::mode::Model;
        // The system counter is running long before the cartridge sees its
        // first instruction. These are the values a CPU read observes on the
        // first M-cycle — see `Timer::set_boot_counter`. Every one of them is a
        // `boot_div` ROM's measurement of a *different boot program*, which is
        // why they are per console revision — and, on a Color console, also
        // per `GbMode`: a genuine CGB cartridge and a DMG one take different
        // routes through the same boot ROM (see `CGB_BOOT_SYSTEM_COUNTER` and
        // `CGB_COMPAT_BOOT_SYSTEM_COUNTER`).
        let counter = match self.console {
            Model::Dmg0 => DMG0_BOOT_SYSTEM_COUNTER,
            Model::Cgb0 => CGB0_BOOT_SYSTEM_COUNTER,
            Model::Agb => AGB_BOOT_SYSTEM_COUNTER,
            // The only console whose boot ROM does not take a fixed time.
            Model::Sgb | Model::Sgb2 => {
                let rom = &*self.mbc;
                sgb_boot_system_counter(&|addr| rom.readrom(addr))
            }
            _ if self.gbmode.is_dmg() => DMG_BOOT_SYSTEM_COUNTER,
            _ if self.gbmode == GbMode::ColorAsClassic => CGB_COMPAT_BOOT_SYSTEM_COUNTER,
            _ => CGB_BOOT_SYSTEM_COUNTER,
        };
        self.timer.set_boot_counter(counter);
        // The boot ROM ran through at least one VBlank, so its flag is still
        // pending. `IE` is 0, so nothing is dispatched until the game asks.
        self.intf = 0x01;
        // `P1`: the DMG boot ROM leaves both select lines pulled low (`$CF`).
        // The CGB's leaves them released (`$FF`), and so does the SGB's — its
        // last use of the port is clocking packets to the SNES, which ends with
        // a write of `$30` (`boot_hwio-S` expects `$FF`, `boot_hwio-dmgABCmgb`
        // `$CF`). This is therefore *not* a `GbMode::is_dmg()` question.
        if self.gbmode == GbMode::Classic {
            self.keypad.wb(0x00);
        }
        self.wb(0xFF05, 0);
        self.wb(0xFF06, 0);
        self.wb(0xFF07, 0);
        // Before the LCD comes on, so the write cannot land in a blocked mode.
        if self.gbmode.is_dmg() {
            self.set_boot_logo_tiles();
            self.set_boot_logo_tilemap();
        }
        self.wb(0xFF40, 0x91);
        self.wb(0xFF42, 0);
        self.wb(0xFF43, 0);
        self.wb(0xFF45, 0);
        self.wb(0xFF47, 0xFC);
        self.wb(0xFF48, 0xFF);
        self.wb(0xFF49, 0xFF);
        self.wb(0xFF4A, 0);
        self.wb(0xFF4B, 0);
        // Where the PPU is in its frame at hand-off — see `Gpu::set_boot_position`.
        if let Some(dot) = boot_ppu_dot(self.console) {
            self.gpu.set_boot_position(dot);
        }
        if self.gbmode == GbMode::ColorAsClassic {
            // A CGB running a DMG cartridge has just loaded the compatibility
            // palettes through `BCPS`/`OCPS`, which are left pointing past the
            // last byte written with auto-increment still on (`$C8`/`$D0`).
            // A CGB *cartridge* takes a different path through the boot ROM and
            // no suite ROM measures where it leaves them, so `Color` is left
            // alone rather than guessed at.
            self.gpu.set_boot_palette_index();
        }
    }

    /// Commit the resolved hardware mode. The PPU keeps its own copy (it reads
    /// it per scanline), so the two must be set together.
    /// `$FF00`. On a Super Game Boy running a multiplayer game the low nibble
    /// answers two different questions depending on the select lines: with both
    /// released it reports **which player** is next
    /// ([`crate::sgb::Sgb::player_nibble`]), and with a row selected it reports
    /// that player's buttons — of which only player 1's exist here, so the
    /// other ports read as nothing pressed, exactly as a port with no pad
    /// plugged in does.
    fn joypad_read(&self) -> u8 {
        let raw = self.keypad.rb();
        let Some(sgb) = self.sgb.as_ref() else {
            return raw;
        };
        match sgb.player_nibble() {
            // Both lines released: the player index, not the buttons.
            Some(nibble) if raw & 0x30 == 0x30 => (raw & 0xF0) | nibble,
            // A row is selected and this is not player 1's pad.
            _ if sgb.player_index() != 0 => raw | 0x0F,
            _ => raw,
        }
    }

    /// Decompress the cartridge header's Nintendo-logo bitmap
    /// (`$0104`-`$0133`, 48 bytes) into the 24 tiles the monochrome boot ROMs
    /// place at `$8010`-`$818F` (tile ids `$01`-`$18`), before
    /// [`Self::set_boot_logo_tilemap`] places them on screen.
    ///
    /// **This is licence-clean by construction: every input byte is the
    /// cartridge's own header, present in every bootable ROM (including this
    /// project's own vendored `third_party/dmg-acid2/dmg-acid2.gb`) because
    /// real hardware refuses to boot a ROM whose header logo does not match —
    /// Pan Docs, [*The Cartridge Header*
    /// §0104-0133](https://gbdev.io/pandocs/The_Cartridge_Header.html#0104-0133--nintendo-logo).**
    /// No boot-ROM byte is read, embedded, or reproduced anywhere in this
    /// function; it is a public, documented unpack algorithm applied to data
    /// the cartridge already supplies, the same standing as the MMPX port or
    /// the hand-rolled PNG encoder.
    ///
    /// The algorithm, verified against Pan Docs' own description and, for the
    /// bitplane detail Pan Docs does not spell out, against the mechanism in
    /// the public disassembly [ISSOtm/gb-bootroms `src/dmg.asm`
    /// `Logo:`](https://codeberg.org/ISSOtm/gb-bootroms) (read to confirm the
    /// mechanism, exactly as this project already does for SameBoy and
    /// docboy elsewhere; no bytes reproduced): each **pair** of header bytes
    /// (`$0104`/`$0105`, `$0106`/`$0107`, …) is one output tile — 24 pairs,
    /// 24 tiles — and splits into four nibbles, one per *pre-scale* row of
    /// that tile's 4×4 source block (MSB = leftmost of the 4 source pixels).
    /// Each nibble is horizontally doubled into an 8-bit row and written
    /// **twice** (vertical doubling) to produce the final 8×8 tile. Crucially,
    /// only the tile's **low bitplane** is written — the boot ROM's own
    /// routine skips the high-plane byte on every row — so a lit pixel is
    /// colour index 1, not 3; both read as black under the `BGP = $FC` this
    /// project already hands over, so the difference is invisible on screen
    /// and would only show up in an exact VRAM byte comparison, which is
    /// exactly what [`GB-BOOT-07`](../../docs/mealybug.md) checks.
    ///
    /// The `®` glyph at tile `$19` (`$8190`-`$819F`) is deliberately **not**
    /// part of this: it is drawn by a separate, hardcoded routine inside the
    /// boot ROM with no cartridge-header input at all, so there is nothing
    /// here to derive it from — see `docs/mealybug.md` §8.6 for why that tile
    /// is never invented.
    fn set_boot_logo_tiles(&mut self) {
        const BASE_ADDR: u16 = 0x0104;
        const TILE_BASE: u16 = 0x8010;
        for tile in 0..24u16 {
            let b0 = self.mbc.readrom(BASE_ADDR + tile * 2);
            let b1 = self.mbc.readrom(BASE_ADDR + tile * 2 + 1);
            let nibbles = [b0 >> 4, b0 & 0xF, b1 >> 4, b1 & 0xF];
            let tile_addr = TILE_BASE + tile * 16;
            for row in 0..8u16 {
                let low = double_nibble(nibbles[(row / 2) as usize]);
                self.wb(tile_addr + row * 2, low);
                self.wb(tile_addr + row * 2 + 1, 0);
            }
        }
    }

    /// The DMG boot ROM's last act before `PC=$0100` is to scroll the Nintendo
    /// logo into place, and it leaves the **tilemap** entries behind: `$01`-`$0C`
    /// across `$9904`-`$990F`, `$0D`-`$18` across `$9924`-`$992F`, and `$19`
    /// (the ® glyph) at `$9910`. Nothing clears them, so a game that loads its
    /// own tiles over ids `$01`-`$19` and never touches the map still shows
    /// *something* in those two rows.
    ///
    /// That is not a curiosity: it is visible in the reference screenshots the
    /// GB Emulator Shootout compares against. SameSuite's `sgb/` tests write
    /// only their results table and leave rows 8 and 9 alone, so on hardware
    /// they render as the test ROM's own hex font and on a machine that booted
    /// with a blank tilemap they are empty — a whole-screen difference that has
    /// nothing to do with what the test measures.
    ///
    /// The tile *pixels* for `$01`-`$18` are written by
    /// [`Self::set_boot_logo_tiles`], called just before this. Tile `$19`'s
    /// pixels are not, and never will be — see that function's doc comment.
    fn set_boot_logo_tilemap(&mut self) {
        for (i, addr) in (0x9904u16..=0x990F).enumerate() {
            self.wb(addr, 0x01 + i as u8);
        }
        for (i, addr) in (0x9924u16..=0x992F).enumerate() {
            self.wb(addr, 0x0D + i as u8);
        }
        self.wb(0x9910, 0x19);
    }

    fn set_mode(&mut self, mode: GbMode) {
        self.gbmode = mode;
        // Keep the console consistent for callers that set a mode directly
        // (`new`/`new_cgb`), so `console` is never a lie about `gbmode`.
        self.console = match mode {
            GbMode::Classic => crate::mode::Model::Dmg,
            GbMode::Super => crate::mode::Model::Sgb,
            GbMode::Super2 => crate::mode::Model::Sgb2,
            GbMode::Color | GbMode::ColorAsClassic => crate::mode::Model::Cgb,
        };
        self.gpu.gbmode = mode;
        // Color silicon gives the work RAM its own bus — see [`DmaBus`]. A CGB
        // running a DMG cartridge is still Color silicon, so this follows the
        // console and not the cartridge.
        self.wram_own_bus = !mode.is_dmg();
        // The ICD2 exists only on an SGB, so a machine that is not one pays
        // nothing: `Option::is_some` on the `$FF00` path and no state to
        // serialise.
        self.sgb = mode.is_sgb().then(crate::sgb::Sgb::new);
    }

    /// Advance the timer by exactly one M-cycle for a single CPU memory access,
    /// *before* the access reads/writes the bus. See the `bus_tick_enabled`
    /// field docs. No-op unless a CPU instruction is currently executing.
    #[inline]
    fn bus_tick(&mut self) {
        if self.bus_tick_enabled {
            // Speed mode: the peripherals advance once per instruction instead
            // of once per access. Leaving `bus_ticked` at zero is the whole
            // mechanism — `do_cycle` then applies the instruction's full tick
            // count at its end, so nothing runs at the wrong *rate*. The PPU
            // block below still runs: for the per-dot engine the per-access
            // step *is* the timing, and for the batched one it is the five
            // M-cycles of interrupt dispatch, which decide their own vector.
            if !self.per_access_bus {
                self.bus_tick_ppu();
                return;
            }
            // Measurement only, compiled out of every shipping build. Returning
            // here leaves `bus_ticked` at zero, so `do_cycle` applies the whole
            // instruction's ticks at its end — the totals are unchanged and
            // only the sub-instruction phase is lost. See `crate::profile`.
            #[cfg(feature = "profile")]
            if crate::profile::bus_tick_skipped() {
                return;
            }
            self.timer.do_mcycle_now();
            // Every one of these is a *test* before a store, not a store. This
            // runs once per memory access of every instruction and the answer
            // is almost always "nothing happened": an unconditional
            // read-modify-write of `intf` and an unconditional `mem::take` of
            // the serial edges are four dirty cache lines a bus cycle for two
            // events that occur a handful of times a frame.
            if self.timer.interrupt != 0 {
                self.intf |= self.timer.interrupt;
                self.timer.interrupt = 0;
            }
            // The link port's shift clock is divided from the same counter, so
            // it has to be drained here, *before* the access — otherwise an
            // `SC` write landing on an edge M-cycle would consume that edge and
            // finish a shift too early.
            if self.timer.serial_edge_pending() {
                self.serial_shift();
            }
            // The APU clock advances per-access too, so a wave-RAM or NR5x
            // access lands on its exact sub-instruction cycle (Blargg
            // dmg_sound 09/10/12 measure this to 2 T-cycles). Synthesis stays
            // event-driven; this only moves the "now" that catch_up syncs to.
            #[cfg(feature = "profile")]
            let (apu, dma) = (
                !crate::profile::apu_skipped(),
                !crate::profile::oam_dma_skipped(),
            );
            #[cfg(not(feature = "profile"))]
            let (apu, dma) = (true, true);
            if apu {
                // Accumulate, and only call the APU when it could do something
                // other than add to a counter — see [`Self::flush_apu`].
                self.apu_pending += 4;
                if self.apu_pending >= self.apu_budget {
                    self.flush_apu();
                }
            }
            // `oam_dma_tick` returns immediately when nothing is running, but
            // *reaching* it is a call and a load from the far end of a 150 KiB
            // struct. Asking here keeps the common M-cycle straight-line.
            // `OamDma::idle()` loads two fields from the far end of a 150 KiB
            // struct, on every memory access, to answer "no" essentially
            // always — 2.4 % of a collector's samples. The answer is cached
            // beside the other per-access state instead.
            if dma && self.oam_dma_busy {
                self.oam_dma_tick();
            }
            // The accurate PPU is a per-dot machine, so it has to advance
            // per-access too — the whole point of it is that a write to
            // `BGP`/`SCX`/`LCDC`/`WX` lands on the exact dot the program put it
            // on, and an instruction-granular PPU cannot see that. The fast
            // renderer reads each register once per scanline anyway, so it is
            // left batched and pays nothing.
            //
            // In double speed the PPU runs at half the CPU's rate, matching the
            // `ticks / cpudivider` division in `do_cycle`; 4 / 2 is exact, so
            // no phase is lost across the split.
            //
            // The MMU's own flag is tested first deliberately: it is false and
            // in cache, whereas `Gpu::render_mode` is a load from another
            // structure that the whole-scanline engine otherwise never touches
            // here. Same answer either way — both sides are plain reads.
            self.bus_tick_ppu();
            self.bus_ticked += 4;
        }
    }

    /// The per-access half of the PPU's clock, split out of [`Self::bus_tick`]
    /// so speed mode can skip the peripherals without skipping this.
    #[inline]
    fn bus_tick_ppu(&mut self) {
        if self.ppu_ticks_with_the_bus
            || self.gpu.render_mode() == crate::gpu::RenderMode::Accurate
        {
            // This hands the PPU dots directly, so anything the batched
            // engine is still owed has to go in first or the two arrive out
            // of order — 210 Gambatte rows say so.
            self.flush_gpu();
            let step = match self.gbspeed {
                GbSpeed::Single => 4,
                GbSpeed::Double => 2,
            };
            // A PPU-bound write commits partway through its own M-cycle,
            // not at the end of it — see `wb` and [`ppu_write_commit`].
            // `gpu_defer` arrives here as the *request* (how many of this
            // M-cycle's dots belong after the write) and leaves as the
            // *pending remainder* `wb` will apply once the write has
            // landed. Zero on every read and on every non-PPU write.
            let defer = self.gpu_defer.min(step);
            self.gpu.do_cycle(step - defer);
            self.intf |= self.gpu.interrupt;
            self.gpu.interrupt = 0;
            self.gpu_bus_ticked += step - defer;
            self.gpu_defer = defer;
        }
    }

    /// Hand the link port the shift-clock edges the system counter has just
    /// produced, and publish any interrupt that finishes a transfer.
    #[inline]
    fn serial_shift(&mut self) {
        let edges = self.timer.take_serial_edges();
        if edges != 0 {
            self.serial.shift_clock(edges);
            self.intf |= self.serial.interrupt;
            self.serial.interrupt = 0;
        }
    }

    /// Advance OAM DMA by one M-cycle, moving at most one byte. Called from
    /// every path that consumes an M-cycle, so a transfer takes the same 160
    /// M-cycles whether the CPU is busy with memory accesses or idling.
    #[inline]
    fn oam_dma_tick(&mut self) {
        // The transfer writes OAM, which the PPU reads while it draws.
        self.flush_gpu();
        if self.oam_dma.idle() {
            self.dma_bus = DmaBus::None;
            self.oam_dma_busy = false;
            self.gpu.clear_oam_dma();
            return;
        }
        if let Some((page, i)) = self.oam_dma.step() {
            let b = self.oam_dma_source(page, i);
            // Publish what the transfer put on its source bus this M-cycle, so
            // a CPU access that shares the bus can see it — see
            // [`Self::dma_conflict`].
            self.dma_bus = self.dma_bus_of_page(page);
            self.dma_write = self.dma_write_rule(self.dma_bus, page);
            self.dma_byte = b;
            self.dma_index = i;
            // Bit 12 of the address the transfer is driving — see
            // [`Self::wram_bus_address`].
            self.dma_a12 = page & 0x10 != 0;
            // Straight to the PPU: `wb` would refuse it, since OAM belongs to
            // the DMA while it runs.
            self.gpu.wb(0xFE00 + i as u16, b);
            // The PPU is on the other end of the OAM address bus the transfer
            // is driving — see [`crate::gpu::Gpu::oam_read`]. Published after
            // the write, so `dest` counts the bytes that have landed.
            self.gpu.set_oam_dma_dest(i + 1);
        } else {
            self.dma_bus = DmaBus::None;
            // Requested but not yet copying.
            self.gpu.set_oam_dma_dest(0);
        }
    }

    /// Hand the APU the cycles it has been owed, and take a new budget.
    ///
    /// `Sound::do_cycle` is called once per CPU memory access — 11,051 times a
    /// frame on Pokémon Blue's overworld — and **99.7 % of those calls do
    /// nothing but add to a counter**: the APU only acts once every
    /// `output_period` (24344 T, about three times a frame), and `Sound::run`
    /// fires 31 times a frame against those 11,051 calls. The call itself was
    /// 12.9 % of a collector's leaf samples.
    ///
    /// So `bus_tick` accumulates instead, and this is the only thing that
    /// actually advances the APU clock. **It is exact, not an approximation**,
    /// because the budget is `Sound::cycles_to_output` — the flush lands on the
    /// same cycle `do_output` would have, so no sample moves and no register
    /// read-back changes. The alternative that was tried first and rejected —
    /// flushing at the end of the instruction — is cheaper still and is *not*
    /// exact: it slides the sample stream by up to one instruction.
    ///
    /// Everything that observes the APU flushes first: `catch_up` before an
    /// `NRxx`/wave-RAM read or write, `div_write` on a `$FF04` write, and the
    /// instruction's own remainder in `Self::do_cycle`. Miss one and
    /// `catch_up` synchronises to a stale `time`.
    ///
    /// The budget is zeroed rather than recomputed whenever the machine
    /// changes underneath it (a player attached, a speed switch), so the next
    /// access flushes and takes a fresh one.
    #[inline]
    fn flush_apu(&mut self) {
        let pending = std::mem::take(&mut self.apu_pending);
        if let Some(ref mut sound) = self.sound {
            if pending > 0 {
                sound.do_cycle(pending);
            }
            self.apu_budget = sound.cycles_to_output();
        } else {
            self.apu_budget = u32::MAX;
        }
    }

    /// Give the PPU the dots it is owed, and take a new horizon.
    ///
    /// The batched renderer pays a whole chunk of bookkeeping per *call* — the
    /// line test, the three-way mode test, `refresh_stat_line` — however few
    /// dots that call carries, and `Self::do_cycle` called it once per
    /// instruction. Accumulating instead and applying them together is about a
    /// third of a headless frame.
    ///
    /// **It is exact, not an approximation.** The bound is
    /// [`Gpu::defer_dots`] — the distance to the next dot at which the PPU
    /// does anything a CPU can see — and `Self::do_cycle` flushes on the
    /// instruction whose own dots *reach* it, so a mode change, an `LY`
    /// increment, a STAT or VBlank interrupt, `updated` and the HBlank latch
    /// all land on exactly the instruction they landed on before. Everything
    /// that reads the PPU out of turn flushes first: VRAM, OAM, the `$FF4x`
    /// and `$FF6x` registers, a VRAM DMA, and the frontend's own accessors.
    #[inline]
    fn flush_gpu(&mut self) {
        let owed = std::mem::take(&mut self.gpu_owed);
        if owed > 0 {
            self.gpu.do_cycle(owed);
        }
        self.gpu_horizon = self.gpu.defer_dots();
    }

    /// True when a CPU access to `address` lands on the bus OAM DMA is driving
    /// in this very M-cycle.
    ///
    /// **The rule, and it is one rule and not two.** The DMA controller does
    /// not have a bus of its own: it drives the source address onto whichever
    /// bus that address lives on, and reads the byte back off it. A CPU access
    /// on the same bus in the same M-cycle therefore does not get its own
    /// memory — the two are looking at the same wires, so the value is
    /// whatever is on them. A read gets the DMA's byte; a write puts the CPU's
    /// data on the wires, and the DMA copies *that* into OAM instead of what
    /// the source held. The CPU's own write still lands wherever it was
    /// addressed.
    ///
    /// This is the largest single named cause in the whole c-sp aggregation —
    /// `TGB-DMA-001`, 562 Gambatte `oamdma/` rows — and Gambatte's own sources
    /// are what pin it. `oamdma_src0000_busypush0001` pushes `$55AA` with
    /// `SP = $0001` while a DMA runs out of ROM, so the high byte is written to
    /// `$0000`, which is ROM and keeps nothing; the ROM then reads `$FE9D` and
    /// expects **`$55`**, the byte the CPU wrote, rather than the `$65` the
    /// source holds there. `oamdma_src0000_busypop7FFF` is the mirror image: it
    /// pops from `$7FFF`, whose ROM byte is `$55`, and expects `$65` — the byte
    /// the DMA was reading.
    ///
    /// HRAM, I/O, OAM and `IE` are internal to the chip and are on neither bus,
    /// which is why an OAM DMA routine is always copied to HRAM before it is
    /// run (Pan Docs, "OAM DMA Transfer").
    #[inline]
    fn dma_conflict(&self, address: u16) -> bool {
        self.dma_bus != DmaBus::None
            && self.bus_tick_enabled
            && self.cpu_bus_of(address) == self.dma_bus
    }

    /// `$FEA0-$FEFF`, which Nintendo calls prohibited and which is not a hole.
    ///
    /// Pan Docs, ["FEA0–FEFF range"](https://gbdev.io/pandocs/Memory_Map.html):
    /// it returns `$FF` while OAM is blocked, and otherwise the answer depends
    /// on the console.
    ///
    /// * **DMG, MGB, SGB, SGB2** — reads return `$00`, and a read while OAM is
    ///   blocked triggers the OAM-corruption bug (which `is_oam_page` already
    ///   routes here: `$FE00-$FEFF` is one bus).
    /// * **CGB revision E, AGB, AGS, GBP** — a read returns the high nibble of
    ///   the low address byte twice, so `$FEAx` is `$AA` and `$FEBx` is `$BB`.
    /// * **CGB revisions 0-D** — a real, unique RAM area, "masked with a
    ///   revision-specific value". Deliberately **not** implemented: we do not
    ///   know the mask for any revision, and the two suites that reach this
    ///   block name different machines.
    ///
    /// **The two suites disagree, and the disagreement is the finding.**
    /// Gambatte's Color reference console is CPU CGB C, so its
    /// `oamdma_*_busypushFEA1` and `busypushFF01` rows pop back exactly what
    /// they wrote here. `cgb-acid-hell`'s hardware-photographed reference does
    /// not: implementing the block as plain RAM was measured at **+26 more
    /// Gambatte rows and cgb-acid-hell 2 → 176 wrong pixels**, so its reference
    /// device is a revision E or an AGB. This emulator already targets CGB-E
    /// elsewhere (`sound::CgbRevision`), a two-pixel distance to a hardware
    /// photograph is worth more than 26 rows of a suite measuring another
    /// stepping, and "RAM with no mask" would be a guess where the nibble rule
    /// is documented. If a revision knob with an `0-D`/`E` boundary is ever
    /// added — neither `mode::PpuRevision` nor `sound::CgbRevision` has it —
    /// this is where it goes and those are the numbers.
    #[inline]
    fn read_unusable(&self, address: u16) -> u8 {
        if self.gbmode.is_dmg() {
            0x00
        } else {
            let low = (address & 0xFF) as u8;
            (low & 0xF0) | (low >> 4)
        }
    }

    /// Which bus an OAM DMA reading from `page` drives.
    ///
    /// Pages `$E0-$FF` are the part the two consoles disagree about. A DMG
    /// folds them onto echo RAM and reads work RAM through them; a Color
    /// console puts the address on the cartridge bus, where nothing answers, so
    /// the transfer copies `$FF` — which is exactly what Gambatte's
    /// `oamdma_srcE000_*` rows expect of each machine.
    #[inline]
    fn dma_bus_of_page(&self, page: u8) -> DmaBus {
        match page {
            0x80..=0x9F => DmaBus::Video,
            0xC0..=0xDF if self.wram_own_bus => DmaBus::Wram,
            _ => DmaBus::External,
        }
    }

    /// What a conflicting CPU write puts in OAM — see [`DmaWrite`].
    #[inline]
    fn dma_write_rule(&self, bus: DmaBus, page: u8) -> DmaWrite {
        match bus {
            // Only a Color console has this bus, and on it the CPU simply does
            // not get the cycle.
            DmaBus::Wram => DmaWrite::Source,
            DmaBus::Video if self.wram_own_bus => DmaWrite::Zero,
            // A DMG's work RAM is on the same bus as the cartridge *and* inside
            // the SoC, which is the one case where both ends drive.
            DmaBus::External if !self.wram_own_bus && page >= 0xC0 => DmaWrite::And,
            _ => DmaWrite::Cpu,
        }
    }

    /// The work-RAM address a CPU access actually reaches while an OAM DMA is
    /// running — which is **not always the one the CPU asked for**.
    ///
    /// The address bus is shared; only the data buses are separate. So while a
    /// transfer drives an address, work RAM latches **that** address's bit 12
    /// rather than the CPU's, and a write to `$C000` lands at `$D000` (in
    /// whatever `SVBK` bank is selected) whenever the transfer's source address
    /// has bit 12 set — and, symmetrically, a write to `$D000` lands at `$C000`
    /// when it does not. Bits 0-11 stay the CPU's.
    ///
    /// **Measured, not inferred, and it is the correction of a note that used
    /// to stand here.** `docs/conformance-notes.md` recorded ~19 Gambatte
    /// `cgb04c` rows as "one unexplained Color behaviour", having tested and
    /// rejected the obvious reading — that `$C000` and `$D000` alias — because
    /// Pan Docs ([SVBK](https://gbdev.io/pandocs/CGB_Registers.html)) says bank
    /// 0 is always at `$C000-$CFFF`. **Pan Docs is right and so was that
    /// refusal**: this is not aliasing, and the banked window is where it has
    /// always been. What moves is which address the write uses.
    ///
    /// The rule was read off a probe cartridge run through
    /// [`tools/sameboy-oracle`](../../tools/sameboy-oracle/README.md) and this
    /// emulator side by side (`docs/measured/oam-dma-work-ram.md`): eleven
    /// source pages, both halves of work RAM as the write target, and every
    /// `SVBK` bank read back. It is **external-bus transfers only** — a
    /// video-bus source (`$80-$9F`) does not redirect, and a work-RAM source
    /// conflicts on its own bus and the access is resolved before reaching here.
    ///
    /// **Reads follow the same rule and the same symmetry**, which the same
    /// probe measures: with the transfer sourced from `$7F00`, a read of
    /// `$C000` returns what `$D000` holds, and a read of `$D000` during a
    /// `$0000`-sourced transfer returns what `$C000` holds.
    #[inline]
    fn wram_bus_address(&self, address: u16) -> u16 {
        if self.wram_own_bus
            && self.bus_tick_enabled
            && self.dma_bus == DmaBus::External
            && matches!(address, 0xC000..=0xFDFF)
        {
            let a12 = if self.dma_a12 { 0x1000 } else { 0 };
            (address & !0x1000) | a12
        } else {
            address
        }
    }

    /// Which bus a CPU access to `address` drives.
    #[inline]
    fn cpu_bus_of(&self, address: u16) -> DmaBus {
        match address {
            0x0000..=0x7FFF | 0xA000..=0xBFFF => DmaBus::External,
            0x8000..=0x9FFF => DmaBus::Video,
            0xC000..=0xFDFF if self.wram_own_bus => DmaBus::Wram,
            0xC000..=0xFDFF => DmaBus::External,
            _ => DmaBus::None,
        }
    }

    /// Read one source byte for OAM DMA. This is *not* a CPU access: it must
    /// not tick the bus or trip a register side-effect. Pages `$E0-$FF` read
    /// echo RAM, as the transfer sits on the external bus.
    fn oam_dma_source(&self, page: u8, index: u8) -> u8 {
        let mut address = ((page as u16) << 8) | index as u16;
        if address >= 0xE000 {
            // A Color console does not fold these onto echo RAM: it puts the
            // address on the cartridge bus, where nothing answers. Gambatte's
            // `oamdma_srcE000_*`/`srcFF00_*` rows expect `$FF` in OAM there and
            // the echoed work RAM on a DMG.
            if self.wram_own_bus {
                return 0xFF;
            }
            address -= 0x2000;
        }
        match address {
            0x0000..=0x7FFF => self.read_rom(address),
            0x8000..=0x9FFF => self.gpu.rb(address),
            0xA000..=0xBFFF => self.mbc.readram(address),
            0xC000..=0xCFFF => self.wram[address as usize & 0x0FFF],
            _ => self.wram[(self.wrambank * 0x1000) | (address as usize & 0x0FFF)],
        }
    }

    /// Advance OAM DMA over the instruction's remaining internal cycles.
    #[inline]
    fn oam_dma_cycles(&mut self, ticks: u32) {
        if self.oam_dma.idle() {
            // Keep the cache honest. [`Self::oam_dma_busy`] is a *cache* of
            // `!oam_dma.idle()`, and the per-access path clears it in
            // [`Self::oam_dma_tick`]; this path is the only one a machine with
            // `instruction-bus` on ever takes, and without this the flag
            // latches true for the rest of the session after the game's first
            // transfer.
            //
            // **The PPU's copy has to be cleared here too, and this arm is NOT
            // unreachable on the default machine.** The transfer's last
            // M-cycle leaves `OamDma::pos` at `LEN` inside `bus_tick` without
            // the flag moving, so the instruction's remainder arrives here and
            // *this* is where the transfer is noticed to be over. Clear only
            // the MMU's half and `Gpu::dma_owns_oam` latches on for the rest of
            // the session — the object scan then never reads OAM again.
            // Measured: 192 of 550 frames of the GBSelfTest cartridge, whose
            // `GB-DMA-06` is what found it.
            self.dma_bus = DmaBus::None;
            self.oam_dma_busy = false;
            self.gpu.clear_oam_dma();
            return;
        }
        for _ in 0..ticks / 4 {
            self.oam_dma_tick();
        }
    }

    /// Advance one M-cycle that carries no bus access — the idle cycles of
    /// interrupt dispatch. Same accounting as [`Self::bus_tick`], so the
    /// instruction's remainder in [`Self::do_cycle`] stays correct.
    pub(crate) fn internal_tick(&mut self) {
        self.bus_tick();
    }

    /// Hand the PPU an OAM bus cycle that landed `dots` T-cycles into the
    /// current instruction. No-op unless a CPU instruction is executing —
    /// the offset is only meaningful then, and OAM DMA drives the PPU
    /// directly rather than through `rb`/`wb`.
    #[inline]
    fn oam_bug(&mut self, kind: OamBug, dots: u32) {
        if self.bus_tick_enabled && !self.speed.no_oam_bug {
            // `dots` is measured from the top of the instruction, which is
            // where the *batched* PPU still stands. The accurate engine has
            // already been walked forward to this very access, so predicting
            // the same distance again would look a whole instruction into the
            // future. Only the dots not yet applied are still ahead of it.
            let (ahead, phase) =
                if self.gpu.render_mode() == crate::gpu::RenderMode::Accurate {
                    (
                        dots as i32 - self.gpu_bus_ticked as i32,
                        ACCURATE_OAM_BUG_PHASE,
                    )
                } else {
                    (dots as i32, OAM_BUG_PHASE)
                };
            self.gpu.oam_bug(kind, ahead + phase);
        }
    }

    /// The CPU's 16-bit increment/decrement unit put `value` on the address
    /// bus during an M-cycle that carries no memory access of its own —
    /// `inc rr`, `dec rr`, and the implied `dec sp` at the head of a `push`.
    ///
    /// Called *before* that M-cycle is consumed, so the offset is one M-cycle
    /// past what the bus has already spent. On a DMG this is the whole cause
    /// of the OAM-corruption bug: the IDU is wired to the address bus and
    /// asserts the address even with no read or write behind it (Pan Docs,
    /// ["OAM Corruption Bug"](https://gbdev.io/pandocs/OAM_Corruption_Bug.html)).
    #[inline]
    pub(crate) fn idu_tick(&mut self, value: u16) {
        if is_oam_page(value) {
            // The corruption is predicted from the PPU's own dot counter, so
            // both the batch and the PPU have to be current.
            self.flush_pending();
            self.flush_gpu();
            self.oam_bug(OamBug::Write, self.bus_ticked + 4);
        }
    }

    /// Flag the *next* memory access as sharing its M-cycle with a 16-bit
    /// increment/decrement: `ld a,[hli]`, `ld a,[hld]` and the first read of a
    /// `pop`. `address` is the address that access will use, which in every
    /// such instruction is also the IDU's operand — so flagging only OAM
    /// addresses means the flag can never be left set for a *later* access to
    /// go on and consume.
    #[inline]
    pub(crate) fn share_idu(&mut self, address: u16) {
        self.idu_share = is_oam_page(address);
    }

    /// Enable per-access timer stepping for the duration of one CPU
    /// instruction. Returns the previous value so it can be restored (OAM DMA
    /// re-uses `rb`/`wb` internally and must not be double-counted).
    /// Apply a set of speed flags. See [`crate::speed::SpeedFlags`].
    pub fn set_speed_flags(&mut self, flags: crate::speed::SpeedFlags) {
        self.speed = flags;
        // `horizon_batch` **includes** the instruction-bus liberty by
        // construction: a horizon computed at the last flush is only valid
        // while nothing advances a peripheral behind its back, and the
        // per-access tick does exactly that. Asking for one gets both.
        self.flush_pending();
        self.per_access_bus = !(flags.instruction_bus || flags.horizon_batch);
        self.horizon_batch = flags.horizon_batch;
        self.run_horizon = 0;
        if let Some(s) = self.sound.as_mut() {
            s.set_state_frozen(flags.no_apu_state);
        }
    }

    /// Whether the peripherals really are advanced once per bus access — the
    /// machine's answer rather than the flags', which is what a test wants.
    #[cfg(test)]
    pub(crate) fn per_access_bus_for_test(&self) -> bool {
        self.per_access_bus
    }

    /// Which shortcuts are in effect.
    pub fn speed_flags(&self) -> crate::speed::SpeedFlags {
        self.speed
    }

    pub fn set_bus_ticking(&mut self, on: bool) -> bool {
        let prev = self.bus_tick_enabled;
        self.bus_tick_enabled = on;
        prev
    }

    /// Make the **batched** PPU advance one M-cycle per bus tick, the way the
    /// per-dot one already does, and return the previous setting.
    ///
    /// Used for exactly one thing: the five M-cycles of interrupt dispatch.
    /// The PPU is running during them on hardware, and `Cpu::handleinterrupt`
    /// decides the vector *in the middle* of them from `IE & IF` as they stand
    /// at M4 — so an interrupt raised by the PPU while the CPU is pushing `PC`
    /// can redirect the dispatch it is already inside. With the PPU frozen
    /// until the end of the dispatch it cannot, and that is observable:
    /// `mealybug/m2_win_en_toggle` enables VBlank *and* mode-2 STAT and toggles
    /// `LCDC.5` in one handler while the other sets it, so which of the two
    /// runs first inverts the whole frame's window pattern. Real DMG and real
    /// CGB photograph identically there, even though `misc/ppu/vblank_stat_intr-C`
    /// shows the CGB raising STAT one M-cycle *before* VBlank — the redirect is
    /// what reconciles the two, and it needs the PPU to be running.
    ///
    /// The dots are counted into `gpu_bus_ticked`, so the lump `do_cycle` at
    /// the end of the dispatch subtracts them and total PPU time is unchanged.
    pub(crate) fn set_ppu_bus_ticking(&mut self, on: bool) -> bool {
        let prev = self.ppu_ticks_with_the_bus;
        self.ppu_ticks_with_the_bus = on;
        prev
    }

    /// Serialise the MMU and every device it owns. The APU is written as a
    /// length-prefixed, self-describing block so a reader that lacks a `Sound`
    /// instance (wasm, or the headless core) can skip it — that is what lets a
    /// native state with audio load cleanly elsewhere.
    /// The PPU must be current: [`Self::flush_gpu`] defers it between
    /// observations and a save state is an observation.
    pub(crate) fn flush_for_observer(&mut self) {
        self.flush_pending();
        self.flush_gpu();
    }

    pub(crate) fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.bytes(&self.wram);
        w.bytes(&self.zram);
        w.u8(self.inte);
        w.u8(self.intf);
        w.usize(self.wrambank);
        w.u8(self.gbmode.to_u8());
        w.u8(self.gbspeed.to_u8());
        w.bool(self.speed_switch_req);
        w.u8(self.hdma_status.to_u8());
        w.u16(self.hdma_src);
        w.u16(self.hdma_dst);
        w.u8(self.hdma_len);
        w.bool(self.hdma_run_block);
        w.bytes(&self.undocumented_cgb_regs);
        // Whether the user's boot ROM is still mapped. The *image* is never
        // stored — it is the host's file, exactly as the cartridge is — so a
        // state taken during the boot animation can only be loaded into a
        // machine that has one, and says so rather than resuming into the
        // cartridge at `PC=$0000`.
        w.bool(self.boot_active);
        // Sub-instruction timer stepping (mem_timing fix). These are reset to
        // false/0 between instructions, so at a normal frame-boundary save they
        // are default — but capturing them keeps the state complete regardless
        // of when it is taken.
        w.bool(self.bus_tick_enabled);
        w.u32(self.bus_ticked);

        self.serial.save_state(w);
        self.timer.save_state(w);
        self.oam_dma.save_state(w);
        self.keypad.save_state(w);
        // SGB block (present flag + payload), length-prefixed and skippable so
        // a state taken on a Super Game Boy can still be read by a build that
        // is not one.
        let mut sgb = crate::savestate::Writer::new();
        match &self.sgb {
            Some(icd) => {
                sgb.bool(true);
                icd.save_state(&mut sgb);
            }
            None => sgb.bool(false),
        }
        w.blob(&sgb.buf);
        self.gpu.save_state(w);

        // APU block (present flag + payload), length-prefixed & skippable.
        let mut apu = crate::savestate::Writer::new();
        match &self.sound {
            Some(s) => {
                apu.bool(true);
                s.save_state(&mut apu);
            }
            None => apu.bool(false),
        }
        w.blob(&apu.buf);

        // MBC block, length-prefixed (RAM size varies by cartridge).
        let mut mbc = crate::savestate::Writer::new();
        self.mbc.save_state(&mut mbc);
        w.blob(&mbc.buf);
    }

    pub(crate) fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        r.fill(&mut self.wram)?;
        r.fill(&mut self.zram)?;
        self.inte = r.u8()?;
        self.intf = r.u8()?;
        self.wrambank = r.usize()?;
        self.gbmode = GbMode::from_u8(r.u8()?)?;
        self.gbspeed = GbSpeed::from_u8(r.u8()?)?;
        self.speed_switch_req = r.bool()?;
        self.hdma_status = DMAType::from_u8(r.u8()?)?;
        self.hdma_src = r.u16()?;
        self.hdma_dst = r.u16()?;
        self.hdma_len = r.u8()?;
        self.hdma_run_block = r.bool()?;
        r.fill(&mut self.undocumented_cgb_regs)?;
        self.boot_active = r.bool()?;
        if self.boot_active && self.boot.is_none() {
            return Err(crate::savestate::StateError::BadField(
                "state was taken inside a boot ROM this machine does not have",
            ));
        }
        self.bus_tick_enabled = r.bool()?;
        self.bus_ticked = r.u32()?;

        self.serial.load_state(r)?;
        self.timer.load_state(r)?;
        self.oam_dma.load_state(r)?;
        self.oam_dma_busy = !self.oam_dma.idle();
        // The PPU's view of a transfer is republished every M-cycle one runs,
        // so it is not in the state; a load that lands with nothing running
        // has to start from "the CPU owns OAM" rather than from whatever the
        // machine being loaded into was doing.
        if !self.oam_dma_busy {
            self.gpu.clear_oam_dma();
        }
        self.gpu_owed = 0;
        self.gpu_horizon = 0;
        // The horizon batch is machine *position*, never machine state: a
        // state is always saved with it empty (`flush_for_observer`), so a
        // load starts one afresh rather than restoring one.
        self.pending_ticks = 0;
        self.pending_out = 0;
        self.run_horizon = 0;
        self.keypad.load_state(r)?;
        // SGB block. Only parsed when this machine is a Super Game Boy; the
        // length prefix means a DMG build skips it whole.
        let sgb = r.blob()?;
        let mut sr = crate::savestate::Reader::new(&sgb);
        if sr.bool()? {
            if let Some(icd) = self.sgb.as_mut() {
                icd.load_state(&mut sr)?;
            }
        }
        self.gpu.load_state(r)?;

        // APU block. Parse only if this build has a Sound instance to restore
        // into; otherwise the whole block is skipped (its length was consumed).
        let apu = r.blob()?;
        {
            let mut ar = crate::savestate::Reader::new(&apu);
            let present = ar.bool()?;
            if present {
                if let Some(s) = self.sound.as_mut() {
                    s.load_state(&mut ar)?;
                }
            }
        }

        // MBC block.
        let mbc = r.blob()?;
        let mut mr = crate::savestate::Reader::new(&mbc);
        self.mbc.load_state(&mut mr)?;
        // The mapper may have restored a different bank.
        self.refresh_rom_windows();
        // A save-state load replaces cartridge RAM wholesale without the game
        // ever writing it, so the battery file has to be rewritten too — else
        // the next launch resurrects the pre-load save.
        self.mbc.battery_dirty();
        Ok(())
    }

    /// The most M-cycles a **halted** CPU may be advanced in one step without
    /// any of this machine behaving differently — see [`Self::do_cycle`], which
    /// is what actually runs them, and `Cpu::exec`, which is the only caller.
    ///
    /// **Why this exists.** Pokémon Blue is halted for **11,818 of the 14,322**
    /// `exec` calls in a frame (measured on the PSP, `crate::profile`), and each
    /// one of those idle M-cycles pays for the whole per-instruction pipeline —
    /// the VRAM-DMA test, the HBlank latch, the timer, OAM DMA, the serial
    /// drain, the PPU, the APU, four interrupt folds — to advance the machine by
    /// four dots. On a wide out-of-order desktop that is noise; on the PSP's
    /// in-order MIPS it was two thirds of the frame.
    ///
    /// **Why it is exact, and not an approximation.** Nothing is skipped: the
    /// same peripherals are advanced by the same total, through the same code
    /// [`Self::do_cycle`] already runs for a six-M-cycle instruction. The only
    /// thing that moves is *when the CPU looks at `IE & IF`*, so the horizon is
    /// simply "how far until any of them could set a bit":
    ///
    /// * the PPU, [`Gpu::idle_dots`] — its next mode edge or line wrap;
    /// * the timer, [`Timer::idle_mcycles`] — its next `TIMA` overflow;
    /// * and a flat refusal to batch at all if anything else is in flight: a
    ///   VRAM DMA, a link transfer in flight (which finishes on the timer's own
    ///   shift edges), or an interrupt already waiting to be folded.
    ///
    /// The batch stops **on** the edge rather than after it, so the interrupt is
    /// folded into `IF` on the very M-cycle it would have been and the CPU wakes
    /// on the same one. [`IDLE_BATCH_MAX`] caps it anyway, so no consumer
    /// downstream ever sees a step longer than a long instruction's worth.
    pub(crate) fn idle_mcycles(&self) -> u32 {
        // Measurement only, compiled out of every shipping build.
        #[cfg(feature = "profile")]
        if crate::profile::idle_batch_skipped() {
            return 1;
        }
        if self.hdma_status != DMAType::NoDma {
            #[cfg(feature = "profile")]
            crate::profile::count_idle_reason(0);
            return 1;
        }
        if self.serial.transfer_in_flight() {
            #[cfg(feature = "profile")]
            crate::profile::count_idle_reason(1);
            return 1;
        }
        if self.keypad.interrupt != 0
            || self.timer.interrupt != 0
            || self.gpu.interrupt != 0
            || self.serial.interrupt != 0
        {
            #[cfg(feature = "profile")]
            crate::profile::count_idle_reason(2);
            return 1;
        }
        // **Measured against the PPU's true position, not its lagging one.**
        // `flush_gpu` leaves it up to `gpu_owed` dots behind, and this horizon
        // is a distance from where it *is*, so the owed dots have already been
        // spent against it. Without the subtraction a halted CPU idles that
        // much too long and wakes late — Gambatte's
        // `halt/late_m0int_halt_m0stat_*` group counts it, 38 rows.
        let dots = self.gpu.idle_dots().saturating_sub(self.gpu_owed);
        if dots == 0 {
            #[cfg(feature = "profile")]
            crate::profile::count_idle_reason(3);
            // No PPU horizon to work from (LCD off, or the per-dot engine).
            // Leave the machine stepping one M-cycle at a time rather than
            // inventing a bound: `Gameboy::frame`'s cycle cap is what ends an
            // LCD-off frame, and overshooting it would move that boundary.
            return 1;
        }
        // The PPU runs at half the CPU's rate in double speed, so a dot horizon
        // buys twice as many M-cycles there.
        let dots_per_mcycle = match self.gbspeed {
            GbSpeed::Single => 4,
            GbSpeed::Double => 2,
        };
        let t = self.timer.idle_mcycles();
        #[cfg(feature = "profile")]
        crate::profile::count_idle_reason(if t < dots.div_ceil(dots_per_mcycle) {
            4
        } else {
            5
        });
        let n = dots.div_ceil(dots_per_mcycle).min(t);
        n.clamp(1, IDLE_BATCH_MAX)
    }

    /// Empty the horizon batch **now**, so that whatever is about to look at a
    /// peripheral sees where it really is.
    ///
    /// Called by every observation there is: any access in `$FE00-$FF7F`, an
    /// interrupt dispatch, a halted CPU about to compute its own horizon, the
    /// OAM-corruption hook, a speed switch, a save state and the frame
    /// boundary. **That list is the whole of why the flag leaves a game's own
    /// arithmetic — and therefore its work RAM — byte-identical**: Gen 1's
    /// `Random_` reads `rDIV`, and it reads it through `read_at`.
    ///
    /// One always-false test when the flag is off, because `pending_ticks` is
    /// then never written.
    #[inline]
    pub(crate) fn flush_pending(&mut self) {
        if self.pending_ticks != 0 {
            self.run_pending();
        }
    }

    /// The flush itself, kept out of line so the per-access path is one
    /// predicted branch and nothing else.
    #[cold]
    #[inline(never)]
    fn run_pending(&mut self) {
        let n = std::mem::take(&mut self.pending_ticks);
        self.pending_out += self.flush_peripherals(n);
        self.run_horizon = self.compute_run_horizon();
    }

    /// Advance the peripherals by `cputicks` from inside a batch.
    ///
    /// **This is [`Self::do_cycle`]'s body minus the two things a batch
    /// refuses to run through**, and it is a separate function rather than a
    /// second call of that body for a measured reason: the per-instruction
    /// body is ~4 KiB, and giving it a second caller — or a second inlined
    /// copy — costs **5-7 % of the accurate frame** because LLVM then stops
    /// inlining it into `Gameboy::frame`. Measured four ways; see
    /// `docs/measured/speed-ledger.md`.
    ///
    /// The two omissions are safe by construction, not by hope:
    /// [`Self::compute_run_horizon`] returns `0` — "do not batch" — while a
    /// VRAM DMA or an OAM DMA is in flight, so `perform_vramdma` and
    /// `oam_dma_cycles` can have nothing to do at a point this is reached.
    /// Everything else is the same work in the same order.
    #[cold]
    #[inline(never)]
    fn flush_peripherals(&mut self, cputicks: u32) -> u32 {
        let cpudivider = match self.gbspeed {
            GbSpeed::Single => 1,
            GbSpeed::Double => 2,
        };
        let gputicks = cputicks / cpudivider;
        self.gpu.clear_hblank_latch();
        let timerticks = cputicks.saturating_sub(self.bus_ticked);
        self.bus_ticked = 0;
        self.timer.do_cycle_cold(timerticks);
        if self.timer.interrupt != 0 {
            self.intf |= self.timer.interrupt;
            self.timer.interrupt = 0;
        }
        if self.timer.serial_edge_pending() {
            self.serial_shift();
        }
        if self.keypad.interrupt != 0 {
            self.intf |= self.keypad.interrupt;
            self.keypad.interrupt = 0;
        }
        self.gpu_owed += gputicks.saturating_sub(self.gpu_bus_ticked);
        self.gpu_bus_ticked = 0;
        if self.gpu_owed >= self.gpu_horizon {
            self.flush_gpu();
        }
        if self.gpu.interrupt != 0 {
            self.intf |= self.gpu.interrupt;
            self.gpu.interrupt = 0;
        }
        self.apu_pending += timerticks;
        if self.apu_pending >= self.apu_budget {
            self.flush_apu();
        }
        if self.serial.interrupt != 0 {
            self.intf |= self.serial.interrupt;
            self.serial.interrupt = 0;
        }
        if self.cart_rtc {
            self.mbc.rtc_tick(gputicks);
        }
        gputicks
    }

    /// How many CPU T-cycles may be run before **anything** the CPU can see
    /// could change — the minimum of every peripheral's own horizon, and `0`
    /// where there is no safe bound at all.
    ///
    /// This is [`Self::idle_mcycles`]'s question asked of a *running* CPU, and
    /// it is PyBoy's `min(timer._cycles_to_interrupt, lcd._cycles_to_interrupt,
    /// …)` in this emulator's terms (`docs/measured/against-the-field.md` §4).
    /// The refusals are `idle_mcycles`'s, for its reasons: a VRAM or OAM DMA
    /// has to move a byte per M-cycle, a link transfer finishes on the timer's
    /// own shift edges, and an interrupt already waiting to be folded has to
    /// reach `IF` before the next instruction boundary looks at it.
    ///
    /// **It does not have to bound a register read**, because every one of
    /// those flushes first — see [`Self::flush_pending`].
    #[cold]
    fn compute_run_horizon(&self) -> u32 {
        if !self.horizon_batch
            || self.hdma_status != DMAType::NoDma
            || self.oam_dma_busy
            || self.serial.transfer_in_flight()
            || self.keypad.interrupt != 0
            || self.timer.interrupt != 0
            || self.gpu.interrupt != 0
            || self.serial.interrupt != 0
        {
            return 0;
        }
        // The PPU runs at half the CPU's rate in double speed, so a dot
        // horizon buys twice as many T-cycles there.
        let ticks_per_dot = match self.gbspeed {
            GbSpeed::Single => 1,
            GbSpeed::Double => 2,
        };
        // Measured from where the PPU *is*, not where it is owed to be — the
        // same subtraction `idle_mcycles` makes, and for the same 38 Gambatte
        // rows.
        let dots = self.gpu_horizon.saturating_sub(self.gpu_owed);
        if dots == 0 {
            // No PPU horizon to work from: the LCD is off, or this is the
            // per-dot engine, which is stepped per bus access by design.
            return 0;
        }
        let ppu = dots.saturating_mul(ticks_per_dot);
        let timer = self.timer.idle_mcycles().saturating_mul(4);
        // The APU raises no interrupt, but it does have to produce its samples
        // on the cycle `do_output` would have — see [`Self::flush_apu`].
        let apu = self.apu_budget.saturating_sub(self.apu_pending);
        ppu.min(timer).min(apu)
    }

    /// Land the batch and throw away the horizon, so the next instruction
    /// takes a fresh one.
    ///
    /// **The flush is not optional here even where the caller has just done
    /// one.** [`Self::do_cycle`]'s prologue keys on `run_horizon != 0`, so a
    /// horizon zeroed while ticks were still accumulated would strand them:
    /// nothing would take `pending_ticks` again until the next `$FE00-$FF7F`
    /// access. One test when there is nothing to flush, which is the common
    /// case at every one of these call sites.
    #[inline]
    pub(crate) fn renegotiate_horizon(&mut self) {
        self.flush_pending();
        self.run_horizon = 0;
    }

    /// Advance the machine by one CPU instruction's worth of cycles — or, with
    /// [`Self::horizon_batch`] on, accumulate them and advance once per
    /// horizon.
    ///
    /// **There is one body and one call site of it**, which is why the batch
    /// costs the accurate machine nothing: `run_horizon` is zero whenever the
    /// flag is off, so the deferral test is never taken and what remains is
    /// the original body behind two perfectly-predicted branches.
    pub fn do_cycle(&mut self, ticks: u32) -> u32 {
        let mut ticks = ticks;
        let mut carry = 0;
        // `run_horizon` is the whole test: it is zero whenever the flag is off
        // **and** whenever the flag is on but a DMA or a pending interrupt has
        // refused the batch, and both mean "run the body with this
        // instruction's own ticks". One load, one predicted branch.
        if self.run_horizon != 0 {
            self.pending_ticks += ticks;
            if self.pending_ticks < self.run_horizon {
                return std::mem::take(&mut self.pending_out);
            }
            ticks = std::mem::take(&mut self.pending_ticks);
            carry = std::mem::take(&mut self.pending_out);
        }
        let cpudivider = match self.gbspeed {
            GbSpeed::Single => 1,
            GbSpeed::Double => 2,
        };
        let vramticks = self.perform_vramdma();
        // The HBlank-entry latch has now been seen; consume it. It used to
        // clear itself at the top of `Gpu::do_cycle`, which is the same thing
        // for a PPU stepped once per instruction and *wrong* for one stepped
        // once per bus access — see [`crate::gpu::Gpu::may_hdma`].
        self.gpu.clear_hblank_latch();
        let gputicks = ticks / cpudivider + vramticks;
        let cputicks = ticks + vramticks * cpudivider;

        // The timer was already advanced by `bus_ticked` T-cycles during this
        // instruction's memory accesses; apply only the remaining internal
        // cycles here so the total per instruction is unchanged.
        debug_assert!(
            self.bus_ticked <= cputicks,
            "bus_ticked {} exceeded instruction ticks {}",
            self.bus_ticked,
            cputicks
        );
        let timerticks = cputicks.saturating_sub(self.bus_ticked);
        self.bus_ticked = 0;
        self.timer.do_cycle(timerticks);
        #[cfg(feature = "profile")]
        let dma = !crate::profile::oam_dma_skipped();
        #[cfg(not(feature = "profile"))]
        let dma = true;
        if dma && self.oam_dma_busy {
            self.oam_dma_cycles(timerticks);
        }
        if self.timer.interrupt != 0 {
            self.intf |= self.timer.interrupt;
            self.timer.interrupt = 0;
        }
        // Gated for the same reason `bus_tick` gates it: draining
        // unconditionally dirties a cache line once per instruction for an
        // event that happens ~137 times a frame.
        if self.timer.serial_edge_pending() {
            self.serial_shift();
        }

        if self.keypad.interrupt != 0 {
            self.intf |= self.keypad.interrupt;
            self.keypad.interrupt = 0;
        }

        // Like the timer, the accurate PPU already advanced during this
        // instruction's memory accesses; apply only the remainder.
        self.gpu_owed += gputicks.saturating_sub(self.gpu_bus_ticked);
        self.gpu_bus_ticked = 0;
        if self.gpu_owed >= self.gpu_horizon {
            self.flush_gpu();
        }
        // **The fold stays here, at the end of the instruction, and does not
        // move into `flush_gpu`.** A flush can now happen part-way through an
        // instruction — a `$FF41` read will cause one — and folding there would
        // hand the CPU a STAT interrupt earlier than the batched engine ever
        // did. Gambatte's `miscmstatirq`, `lycEnable`, `m0enable` and `halt`
        // groups count exactly that: 181 rows.
        if self.gpu.interrupt != 0 {
            self.intf |= self.gpu.interrupt;
            self.gpu.interrupt = 0;
        }

        // Like the timer, the APU already advanced `bus_ticked` T-cycles
        // during this instruction's accesses (bus_tick); apply only the
        // remainder so the per-instruction total is unchanged.
        #[cfg(feature = "profile")]
        let apu = !crate::profile::apu_skipped();
        #[cfg(not(feature = "profile"))]
        let apu = true;
        if apu {
            // The remainder of the instruction, plus whatever `bus_tick`
            // accumulated — see [`Self::flush_apu`].
            self.apu_pending += timerticks;
            if self.apu_pending >= self.apu_budget {
                self.flush_apu();
            }
        }

        if self.serial.interrupt != 0 {
            self.intf |= self.serial.interrupt;
            self.serial.interrupt = 0;
        }

        // The cartridge's own crystal. `gputicks` is real time — it is already
        // divided by the CGB speed multiplier — which is what the clock on the
        // cartridge runs on. See [`crate::mbc::rtc`].
        if self.cart_rtc {
            self.mbc.rtc_tick(gputicks);
        }

        if self.horizon_batch {
            self.run_horizon = self.compute_run_horizon();
        }
        carry + gputicks
    }

    /// Side-effect-free bus read — Tier 1 of the debug surface
    /// ([`docs/debugging.md`](../../docs/debugging.md) §"Read-only inspector").
    ///
    /// Unlike [`MemoryManagementUnit::rb`] this takes `&self`, so it cannot
    /// tick the bus, catch the APU up, or trip a register's read side-effects.
    /// That is the whole point: an inspector must be able to look at memory
    /// between instructions without perturbing the machine it is inspecting.
    ///
    /// The consequence is that the **I/O range `$FF00-$FF7F` reads back
    /// `0xFF`**: every live value there is behind a device whose read is not
    /// `&self` (the APU needs `catch_up`, the joypad multiplexes on the last
    /// write). Plain memory — ROM, VRAM, cartridge RAM, WRAM and its echo, OAM,
    /// HRAM and `IE` — is exact.
    pub fn peek(&self, address: u16) -> u8 {
        match address {
            0x0000..=0x7FFF => self.read_rom(address),
            0x8000..=0x9FFF => self.gpu.rb(address),
            0xA000..=0xBFFF => self.mbc.readram(address),
            0xC000..=0xCFFF | 0xE000..=0xEFFF => self.wram[address as usize & 0x0FFF],
            0xD000..=0xDFFF | 0xF000..=0xFDFF => {
                self.wram[(self.wrambank * 0x1000) | address as usize & 0x0FFF]
            }
            0xFE00..=0xFE9F => self.gpu.rb(address),
            0xFF80..=0xFFFE => self.zram[address as usize & 0x007F],
            0xFFFF => self.inte,
            _ => 0xFF,
        }
    }

    // Address dispatch. See docs/components/mmu.md §"The dispatch" and Pan Docs
    // memory map: https://gbdev.io/pandocs/Memory_Map.html
    pub fn rb(&mut self, address: u16) -> u8 {
        self.bus_tick();
        self.read_at(address)
    }

    /// The **opcode and immediate fetch**, which is the same read as
    /// [`Self::rb`] with the arms a program counter cannot reach left out.
    ///
    /// `PC` spends essentially the whole of a game in ROM, and that read was
    /// the single hottest line in a headless profile — the M-cycle hook, then
    /// the OAM-corruption test, then the DMA-conflict test, then a match over
    /// the whole address space, to reach the first arm. Here the common case is
    /// the bus tick and two loads.
    ///
    /// **Everything it skips, it skips because it cannot apply**, and anything
    /// it is not sure of falls through to the general path: the OAM bug fires
    /// only for `$FE00-$FEFF`, the boot overlay and a DMA conflict are both
    /// tested, and an index outside the ROM goes to the mapper so each one
    /// keeps its own open-bus value.
    #[inline]
    pub fn fetch(&mut self, address: u16) -> u8 {
        self.bus_tick();
        if address < 0x8000 && !self.boot_active && !self.dma_conflict(address) {
            let base = if address < 0x4000 {
                self.rom_lo
            } else {
                self.rom_hi
            };
            if let Some(b) = self.rom_share.get(base | (address as usize & 0x3FFF)) {
                return *b;
            }
        }
        self.read_at(address)
    }

    /// [`Self::rb`] without the M-cycle — the dispatch itself.
    fn read_at(&mut self, address: u16) -> u8 {
        // **The batch's read half.** Everything from OAM to the end of the I/O
        // block reports a peripheral's live position, so the batch has to land
        // before the answer is composed — that is what keeps `DIV`, `TIMA`,
        // `LY`, `STAT` and `NR52` exact at the instant a program looks at
        // them. HRAM and `IE` are above this window and hold nothing a
        // peripheral writes.
        if self.pending_ticks != 0 && (0xFE00..=0xFF7F).contains(&address) {
            self.run_pending();
        }
        // `bus_tick_enabled` is the "a CPU instruction is running" test, and it
        // guards the flag as well as the corruption: a debug peek of OAM must
        // neither corrupt it nor eat a pending `share_idu`.
        if is_oam_page(address) && self.bus_tick_enabled {
            // The corruption is predicted from the PPU's own dot counter, so
            // it must be current — see [`Self::flush_gpu`].
            self.flush_gpu();
            // A read and an IDU increment sharing one M-cycle corrupt OAM
            // differently from a plain read — see `share_idu`.
            let kind = if std::mem::take(&mut self.idu_share) {
                OamBug::ReadWrite
            } else {
                OamBug::Read
            };
            self.oam_bug(kind, self.bus_ticked);
        }
        // A read that shares OAM DMA's bus reads the wires, not the memory —
        // see [`Self::dma_conflict`]. Ahead of the dispatch because it is one
        // predictable branch and it overrides every arm it can reach.
        if self.dma_conflict(address) {
            // A Color console's video bus loses the byte: the transfer stores
            // zero for this index whether the CPU read or wrote, while the CPU
            // still sees what was on the bus (`oamdma_src8000_busypop9FFF`
            // expects `$00` in OAM on `cgb04c` and the real tile byte on a
            // DMG).
            if self.dma_write == DmaWrite::Zero {
                self.gpu.wb(0xFE00 + u16::from(self.dma_index), 0x00);
            }
            return self.dma_byte;
        }
        match address {
            0x0000..=0x7FFF => self.read_rom(address),
            // The PPU holds the VRAM bus through mode 3 — see
            // [`Gpu::cpu_can_reach_vram`]. `bus_tick_enabled` keeps the block on
            // the CPU's own accesses: a debug peek routes through here with the
            // tick suppressed and must see the real byte.
            0x8000..=0x9FFF => {
                self.flush_gpu();
                if self.bus_tick_enabled && !self.gpu.cpu_can_read_vram() {
                    0xFF
                } else {
                    self.gpu.rb(address)
                }
            }
            0xA000..=0xBFFF => self.mbc.readram(address),
            // Echo RAM (E000-FDFF) mirrors WRAM: the same arms serve both ranges
            // because & 0x0FFF folds the echo address onto the C000-page bank.
            // A running transfer can move this read's bit 12 — see
            // [`Self::wram_bus_address`].
            0xC000..=0xFDFF => match self.wram_bus_address(address) {
                a @ (0xC000..=0xCFFF | 0xE000..=0xEFFF) => self.wram[a as usize & 0x0FFF],
                a => self.wram[(self.wrambank * 0x1000) | (a as usize & 0x0FFF)],
            },
            // OAM belongs to the DMA while a transfer runs: the CPU reads
            // $FF and its writes are dropped (Pan Docs, "OAM DMA Transfer").
            // It belongs to the PPU through modes 2 and 3 for the same reason
            // VRAM does — see [`Gpu::cpu_can_reach_oam`].
            0xFE00..=0xFE9F => {
                self.flush_gpu();
                if self.oam_dma.active()
                    || (self.bus_tick_enabled && !self.gpu.cpu_can_read_oam())
                {
                    0xFF
                } else {
                    self.gpu.rb(address)
                }
            }
            0xFF00 => self.joypad_read(),
            0xFF01..=0xFF02 => self.serial.rb(address),
            0xFF04..=0xFF07 => self.timer.rb(address),
            // IF: only 5 interrupt bits exist; the unused top 3 always read 1
            // (Pan Docs "Interrupts").
            0xFF0F => self.intf | 0b11100000,
            0xFF10..=0xFF3F => {
                // Catch the APU up to "now" first: NR52 status, length and
                // sweep state are otherwise evaluated lazily in ~45 ms blocks
                // and read stale (see Sound::catch_up).
                self.flush_apu();
                self.sound.as_mut().map_or(0xFF, |s| {
                    s.catch_up();
                    s.rb(address)
                })
            }
            // `$FF4F` (VBK) is deliberately absent from this list: the PPU
            // answers it, because a Color console in compatibility mode still
            // reads it back ($FE) even though its writes are ignored.
            0xFF4D | 0xFF51..=0xFF55 | 0xFF6C | 0xFF70
                if self.gbmode != GbMode::Color =>
            {
                0xFF
            }
            0xFF72..=0xFF73 | 0xFF75..=0xFF77 if self.gbmode.is_dmg() => 0xFF,
            0xFF4D => {
                0b01111110
                    | (if self.gbspeed == GbSpeed::Double {
                        0x80
                    } else {
                        0
                    })
                    | (if self.speed_switch_req { 1 } else { 0 })
            }
            // $FF46 reads back the last page written, transfer running or not
            // (Mooneye `acceptance/oam_dma/reg_read`).
            0xFF46 => self.oam_dma.read_reg(),
            // `$FF50` is the boot-ROM unmap latch. Only bit 0 exists and it is
            // one-way; the other seven read 1. With no boot ROM there is
            // nothing mapped, so it reads `$FF` — which is what
            // `acceptance/boot_hwio-dmgABCmgb` expects and has always seen.
            0xFF50 => 0xFE | u8::from(!self.boot_active),
            0xFF40..=0xFF4F => {
                self.flush_gpu();
                self.gpu.rb(address)
            }
            0xFF51..=0xFF55 => self.hdma_read(address),
            0xFF68..=0xFF6B => {
                self.flush_gpu();
                self.gpu.rb(address)
            }
            0xFF70 => self.wrambank as u8,
            0xFF72..=0xFF73 => self.undocumented_cgb_regs[address as usize - 0xFF72],
            0xFF75 => self.undocumented_cgb_regs[2] | 0b10001111,
            // CGB PCM12/PCM34: the live 4-bit digital output of each
            // channel, low nibble first (CH1/CH3 low, CH2/CH4 high). Sample
            // accurate, so the APU has to be caught up to this very cycle.
            0xFF76..=0xFF77 => {
                self.flush_apu();
                self.sound.as_mut().map_or(0x00, |s| {
                    s.catch_up();
                    s.rb(address)
                })
            }
            0xFF80..=0xFFFE => self.zram[address as usize & 0x007F],
            0xFFFF => self.inte,
            // `$FEA0-$FEFF` is on OAM's bus, so it is `$FF` whenever OAM is,
            // and console-dependent otherwise — see [`Self::read_unusable`].
            0xFEA0..=0xFEFF => {
                if self.oam_dma.active()
                    || (self.bus_tick_enabled && !self.gpu.cpu_can_read_oam())
                {
                    0xFF
                } else {
                    self.read_unusable(address)
                }
            }
            // Any unmapped I/O reads back 0xFF.
            _ => 0xFF,
        }
    }

    pub fn rw(&mut self, address: u16) -> u16 {
        (self.rb(address) as u16) | ((self.rb(address + 1) as u16) << 8)
    }

    /// **The emulated program has left the rails — stop persisting anything.**
    ///
    /// A Game Boy has no fault handler: a program counter that walks off its own
    /// code keeps executing whatever bytes it finds, and one of the things it
    /// reliably does is store them into cartridge RAM. Our autosave then writes
    /// that over the player's save file. A scout measured **4,378 bytes of
    /// cartridge RAM changed within six frames** of Pokémon Blue crashing out of
    /// a link battle, and the battery persisted them. That is the worst class of
    /// bug an emulator can have: the game is recoverable, the save file is not.
    ///
    /// So a fault does exactly one thing, and does it for **any** cause, not for
    /// the one path that was reported: it
    /// [poisons the battery](crate::mbc::MemoryBankController::poison_battery)
    /// so no later write can reach the file, and records why. The last good save
    /// stays on disk, and the user is told rather than silently robbed.
    ///
    /// It deliberately does **not** stop, reset or otherwise "fix" the machine.
    /// Detectors are heuristics and the emulator staying watchable is useful;
    /// only the irreversible act — writing the file — is withdrawn.
    ///
    /// Sticky for the session and idempotent: the first reason is the diagnosis,
    /// later ones are consequences of it.
    pub fn report_fault(&mut self, reason: &str) {
        if self.fault.is_some() {
            return;
        }
        self.fault = Some(reason.to_string());
        self.mbc.poison_battery(reason);
        // Loud, once. A guard that protects the save silently is a guard the
        // user cannot act on — and the action here (restart; the good `.sav` is
        // untouched) is one only they can take.
        eprintln!(
            "terminalgb: FAULT — {reason}. The battery save will NOT be written \
             again this session; the existing save file is left untouched. \
             Restart the emulator to recover."
        );
    }

    /// Why this machine is considered to have faulted, if it has.
    pub fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }

    /// Read a byte **without perturbing timing** — the observer's read.
    ///
    /// This is the `peek` of the Tier-1 inspector in `docs/debugging.md`, and
    /// what [`crate::plugin::Machine::peek`] is built on. It reuses the normal
    /// dispatch (so banking, echo RAM and I/O read-back all behave) but
    /// suppresses the per-access bus tick, because a debugger or plugin looking
    /// at memory must not advance the timer and APU and change the run it is
    /// observing.
    pub fn debug_peek(&mut self, address: u16) -> u8 {
        let prev = self.set_bus_ticking(false);
        let v = self.rb(address);
        self.set_bus_ticking(prev);
        v
    }

    /// Write a byte without perturbing timing. The `poke` of the same design.
    /// **This can corrupt a running game or its save** — it is reachable only
    /// through a plugin that declared
    /// [`Capability::WRITE_MEMORY`](crate::plugin::Capability::WRITE_MEMORY),
    /// which the frontend then flags to the user.
    pub fn debug_poke(&mut self, address: u16, value: u8) {
        let prev = self.set_bus_ticking(false);
        self.wb(address, value);
        self.set_bus_ticking(prev);
    }

    /// Apply one frame's worth of externally-scheduled button input. Called by
    /// the host **once per emulated frame, before the frame runs** — see
    /// [`crate::input::InputQueue`]. Costs one `Option` test when nothing is
    /// driving the game.
    pub fn advance_input(&mut self) {
        self.input_queue.advance(&mut self.keypad);
        // A button press can raise the joypad interrupt, which is one of the
        // terms a horizon is built from.
        self.renegotiate_horizon();
    }

    pub fn wb(&mut self, address: u16, value: u8) {
        // The batch's write half — same window as [`Self::read_at`], plus the
        // reason a read does not need: a write to `DIV`, `TAC`, `LCDC`, `LYC`,
        // `STAT`, `$FF46`, `$FF55` or an `NRxx` register **moves the horizon
        // itself**, so the one in hand is thrown away and the next instruction
        // takes a fresh one.
        if self.horizon_batch && (0xFE00..=0xFF7F).contains(&address) {
            self.renegotiate_horizon();
        }
        // A write commits partway through its own M-cycle, not at the end of
        // it — see [`ppu_write_commit`]. `bus_tick` advances the per-dot PPU
        // one whole M-cycle before every access, which put every mid-scanline
        // register boundary late by most of an M-cycle; for the registers the
        // pipeline consumes it now holds back the commit plan's `early` dots,
        // applies the plan's one-dot stage value if there is one, and the tail
        // of the M-cycle follows the real value below the dispatch.
        // Only the registers [`ppu_write_commit`] actually schedules, so a
        // write to anything else — `$FF46` above all, which is OAM DMA and
        // never reaches the PPU at all — does not pay a `Gpu::rb` to be told
        // `NO_COMMIT`.
        let commit = if self.bus_tick_enabled
            && matches!(address, 0xFF40 | 0xFF42 | 0xFF43 | 0xFF47..=0xFF49 | 0xFF4B)
            && self.gpu.render_mode() == crate::gpu::RenderMode::Accurate
        {
            let plan = ppu_write_commit(
                address,
                self.gpu.rb(address),
                value,
                self.gbmode,
                self.gpu.cgb_ppu_revision(),
            );
            self.gpu_defer = plan.early;
            plan
        } else {
            NO_COMMIT
        };
        self.bus_tick();
        // A write and an IDU increment in the same M-cycle behave as a single
        // write, so `idu_share` is deliberately not consulted here.
        if is_oam_page(address) {
            self.oam_bug(OamBug::Write, self.bus_ticked);
        }
        // A write that shares OAM DMA's bus puts the CPU's data on the wires,
        // and the transfer copies *that* into OAM rather than the byte its
        // source holds — see [`Self::dma_conflict`]. **The write itself never
        // reaches its own address**: the DMA owns the bus, so the memory the
        // CPU addressed never sees the cycle. `oamdma_src0000_busypushC001`
        // pins both halves at once — it pushes `$55AA` at `SP = $C001` during a
        // DMA and then pops the same two addresses back, and expects the values
        // that were there *before* the push.
        if self.dma_conflict(address) {
            let latched = match self.dma_write {
                DmaWrite::Cpu => value,
                DmaWrite::And => self.dma_byte & value,
                DmaWrite::Source => self.dma_byte,
                DmaWrite::Zero => 0x00,
            };
            self.gpu.wb(0xFE00 + u16::from(self.dma_index), latched);
            return;
        }
        if let Some(staged) = commit.stage {
            if self.gpu_defer > 0 {
                self.gpu.wb(address, staged);
                self.gpu.do_cycle(1);
                self.intf |= self.gpu.interrupt;
                self.gpu.interrupt = 0;
                self.gpu_bus_ticked += 1;
                self.gpu_defer -= 1;
                // Fall through: the ordinary dispatch below lands the commit
                // value and the rest of the M-cycle follows it.
            }
        }
        let value = commit.dispatch.unwrap_or(value);
        match address {
            0x0000..=0x7FFF => {
                self.mbc.writerom(address, value);
                self.refresh_rom_windows();
            }
            // Dropped outright through mode 3 — see [`Gpu::cpu_can_reach_vram`].
            0x8000..=0x9FFF => {
                self.flush_gpu();
                if !self.bus_tick_enabled || self.gpu.cpu_can_write_vram() {
                    self.gpu.wb(address, value)
                }
            }
            0xA000..=0xBFFF => self.mbc.writeram(address, value),
            // A running transfer can move this write's bit 12 — see
            // [`Self::wram_bus_address`].
            0xC000..=0xFDFF => match self.wram_bus_address(address) {
                a @ (0xC000..=0xCFFF | 0xE000..=0xEFFF) => {
                    self.wram[a as usize & 0x0FFF] = value
                }
                a => self.wram[(self.wrambank * 0x1000) | (a as usize & 0x0FFF)] = value,
            },
            0xFE00..=0xFE9F => {
                self.flush_gpu();
                if !self.oam_dma.active()
                    && (!self.bus_tick_enabled || self.gpu.cpu_can_write_oam())
                {
                    self.gpu.wb(address, value)
                }
            }
            0xFF00 => {
                self.keypad.wb(value);
                // The SGB's ICD2 sees every write to the select lines, whatever
                // the write was for — a packet bit and a joypad poll are the
                // same two wires.
                if let Some(sgb) = self.sgb.as_mut() {
                    sgb.write(value);
                }
            }
            0xFF01..=0xFF02 => self.serial.wb(address, value),
            0xFF04..=0xFF07 => {
                // A DIV reset is also an APU event: the frame sequencer is
                // clocked by the falling edge of a DIV bit, so zeroing DIV
                // while that bit is set clocks it (Sound::div_write). The APU
                // must be caught up to this cycle first, exactly as for a
                // direct NRxx access.
                if address == 0xFF04 {
                    self.flush_apu();
                    if let Some(ref mut sound) = self.sound {
                        sound.catch_up();
                        sound.div_write();
                    }
                }
                self.timer.wb(address, value)
            }
            0xFF10..=0xFF3F => {
                // Catch-up before the write so the frame-sequencer position
                // the write interacts with is current (see Sound::catch_up).
                self.flush_apu();
                if let Some(ref mut sound) = self.sound {
                    sound.catch_up();
                    sound.wb(address, value);
                }
            }
            0xFF46 => {
                self.oam_dma.request(value);
                self.oam_dma_busy = !self.oam_dma.idle();
                // The controller takes the OAM address lines a couple of dots
                // from now, not when the first byte lands two M-cycles later —
                // see [`crate::gpu::Gpu::dma_owns_oam`].
                self.gpu.arm_oam_dma();
            }
            // Writing bit 0 unmaps the boot ROM, for good: there is no way back
            // short of a reset, which is why the boot ROM's last instruction is
            // this write and the byte after it is the cartridge's `$0100`.
            0xFF50 => {
                if value & 0x01 != 0 {
                    self.boot_active = false;
                }
            }
            0xFF4D | 0xFF4F | 0xFF51..=0xFF55 | 0xFF6C | 0xFF70 | 0xFF76..=0xFF77
                if self.gbmode != GbMode::Color => {}
            0xFF72..=0xFF73 | 0xFF75..=0xFF77 if self.gbmode.is_dmg() => {}
            // KEY1 bit 0 is a request, not a trigger: `STOP` performs the
            // switch. Writing 0 back withdraws it (only bit 0 is writable).
            0xFF4D => self.speed_switch_req = value & 0x1 == 0x1,
            0xFF40..=0xFF4F => {
                self.flush_gpu();
                self.gpu.wb(address, value)
            }
            0xFF51..=0xFF55 => self.hdma_write(address, value),
            0xFF68..=0xFF6B => {
                self.flush_gpu();
                self.gpu.wb(address, value)
            }
            0xFF0F => self.intf = value,
            0xFF70 => {
                self.wrambank = match value & 0x7 {
                    0 => 1,
                    n => n as usize,
                };
            }
            0xFF72..=0xFF73 => {
                self.undocumented_cgb_regs[address as usize - 0xFF72] = value
            }
            0xFF75 => self.undocumented_cgb_regs[2] = value,
            0xFF80..=0xFFFE => self.zram[address as usize & 0x007F] = value,
            0xFFFF => self.inte = value,
            _ => {}
        };
        // The rest of the write M-cycle, now that the write has committed.
        let rest = std::mem::take(&mut self.gpu_defer);
        if rest > 0 {
            self.gpu.do_cycle(rest);
            self.intf |= self.gpu.interrupt;
            self.gpu.interrupt = 0;
            self.gpu_bus_ticked += rest;
        }
        // Bits that only commit when the cycle closes (`WriteCommit::finalize`).
        if let Some(fv) = commit.finalize {
            self.gpu.wb(address, fv);
        }
    }

    pub fn ww(&mut self, address: u16, value: u16) {
        self.wb(address, (value & 0xFF) as u8);
        self.wb(address + 1, (value >> 8) as u8);
    }

    /// Has the game asked for a speed switch (KEY1 bit 0)? `STOP` reads this
    /// to decide which branch of the Pan Docs STOP chart it is on.
    pub fn speed_switch_requested(&self) -> bool {
        self.speed_switch_req
    }

    /// Reset the 16-bit system counter, exactly as a write to `DIV` does —
    /// `STOP` resets it on every branch of the chart that stops the CPU.
    pub fn reset_div(&mut self) {
        self.timer.wb(0xFF04, 0);
    }

    pub fn switch_speed(&mut self) {
        if self.speed_switch_req {
            // The batch was accumulated at the old clock; land it first.
            self.renegotiate_horizon();
            if self.gbspeed == GbSpeed::Double {
                self.gbspeed = GbSpeed::Single;
            } else {
                self.gbspeed = GbSpeed::Double;
            }
            // The APU is wired to the unmodified 4.194304 MHz clock, so it
            // must halve the CPU T-cycles it is handed at double speed —
            // otherwise every sound plays an octave high in CGB fast mode.
            // Flush first: the cycles owed were handed over at the old speed.
            self.flush_apu();
            self.flush_gpu();
            if let Some(ref mut sound) = self.sound {
                sound.set_double_speed(self.gbspeed == GbSpeed::Double);
            }
            self.apu_budget = 0;
        }
        self.speed_switch_req = false;
    }

    fn hdma_read(&self, a: u16) -> u8 {
        match a {
            // HDMA1-4 are write-only (Pan Docs, "VRAM DMA transfers") — the
            // address counters behind them are not readable.
            0xFF55 => {
                self.hdma_len
                    | if self.hdma_status == DMAType::NoDma {
                        0x80
                    } else {
                        0
                    }
            }
            _ => 0xFF,
        }
    }

    fn hdma_write(&mut self, a: u16, v: u8) {
        match a {
            // The low 4 bits of the source and the low 4 bits *and* the top 3
            // bits of the destination are dropped by the hardware — the
            // destination is a 13-bit offset into VRAM. Writing them here, into
            // the live counters, is what SameSuite `dma/gdma_addr_mask` checks.
            0xFF51 => self.hdma_src = (self.hdma_src & 0x00FF) | ((v as u16) << 8),
            0xFF52 => self.hdma_src = (self.hdma_src & 0xFF00) | ((v & 0xF0) as u16),
            0xFF53 => {
                self.hdma_dst = (self.hdma_dst & 0x00FF) | (((v & 0x1F) as u16) << 8)
            }
            0xFF54 => self.hdma_dst = (self.hdma_dst & 0xFF00) | ((v & 0xF0) as u16),
            0xFF55 => {
                // Bits 0-6 are a real register and are written whatever bit 7
                // says — including on the write that *cancels* a running
                // HBlank transfer, which is why SameSuite `dma/hdma_lcd_off`
                // reads back $80 (not $80 | the blocks that were left) after
                // stopping one with a `$00` write.
                self.hdma_len = v & 0x7F;
                if self.hdma_status == DMAType::Hdma && v & 0x80 == 0 {
                    self.hdma_status = DMAType::NoDma;
                    self.hdma_run_block = false;
                    return;
                }
                if v & 0x80 == 0x80 {
                    self.hdma_status = DMAType::Hdma;
                    // Armed while the PPU is already in mode 0 — or with the
                    // LCD off, where the mode reads 0 forever — the first block
                    // moves at once instead of waiting for the next mode-0
                    // *entry*, which with the LCD off would never come.
                    self.hdma_run_block = self.gpu.in_hblank();
                } else {
                    self.hdma_status = DMAType::Gdma;
                    self.hdma_run_block = false;
                }
            }
            _ => {}
        };
    }

    fn perform_vramdma(&mut self) -> u32 {
        // Reads `may_hdma`, which is the HBlank-entry latch, and writes VRAM.
        if self.hdma_status != DMAType::NoDma {
            self.flush_gpu();
        }
        match self.hdma_status {
            DMAType::NoDma => 0,
            DMAType::Gdma => self.perform_gdma(),
            DMAType::Hdma => self.perform_hdma(),
        }
    }

    fn perform_hdma(&mut self) -> u32 {
        if !self.hdma_run_block && !self.gpu.may_hdma() {
            return 0;
        }
        self.hdma_run_block = false;

        self.perform_vramdma_row();
        if self.hdma_len == 0x7F {
            self.hdma_status = DMAType::NoDma;
        }

        8
    }

    fn perform_gdma(&mut self) -> u32 {
        let len = self.hdma_len as u32 + 1;
        for _i in 0..len {
            self.perform_vramdma_row();
        }

        self.hdma_status = DMAType::NoDma;
        len * 8
    }

    fn perform_vramdma_row(&mut self) {
        for _ in 0..0x10 {
            let b: u8 = self.rb(self.hdma_src);
            // The destination is 13 bits wide: it wraps *inside* VRAM. Letting
            // it run on past $9FFF used to walk the write straight into
            // `Gpu::wb`'s catch-all `panic!` — process death, and with
            // `panic = 'abort'` a lost battery save, from nothing worse than a
            // guest pointing a DMA near the end of VRAM.
            self.gpu.wb(0x8000 | (self.hdma_dst & 0x1FFF), b);
            self.hdma_src = self.hdma_src.wrapping_add(1);
            self.hdma_dst = self.hdma_dst.wrapping_add(1);
        }

        if self.hdma_len == 0 {
            self.hdma_len = 0x7F;
        } else {
            self.hdma_len -= 1;
        }
    }

    pub fn enable_audio(&mut self, player: Box<dyn crate::sound::AudioPlayer>) {
        let fresh = self.sound.is_none();
        if fresh {
            self.sound = Some(crate::sound::Sound::new());
            self.apu_budget = 0;
        }
        let frozen = self.speed.no_apu_state;
        let sound = self.sound.as_mut().unwrap();
        // A chip installed *after* the flags were set must still honour them —
        // otherwise `no-apu-state` silently depends on call order.
        sound.set_state_frozen(frozen);
        // DMG carts get DMG APU semantics (length counters survive power-off
        // etc. — what Blargg's dmg_sound asserts); CGB carts get CGB rules.
        sound.set_mode(self.gbmode != GbMode::Color);
        // An AGB's frequency timer latches its reload two T-cycles earlier than
        // a CPU CGB D/E's — the one APU difference between the two consoles
        // this emulator models, and the whole of SameSuite
        // `channel_1_freq_change_timing-A`. The console is already resolved
        // here, so `GB_MODEL=agb` is the only knob it needs.
        sound.set_agb(self.console == crate::mode::Model::Agb);
        if fresh {
            // The APU is installed after `set_initial` has run (a headless
            // machine may never get one at all), so its share of the post-boot
            // state is applied here — see `Sound::set_post_boot_state`. Only on
            // a *fresh* chip: re-attaching a player mid-run must not rewind the
            // sound chip to its boot state.
            sound.set_post_boot_state(self.gbmode.is_sgb());
        }
        sound.set_player(player);
    }

    /// Tell the audio player whether the frontend is running faster than real
    /// time (fast-forward). See `AudioPlayer::set_fast_forward`.
    pub fn set_audio_fast_forward(&mut self, enabled: bool) {
        if let Some(ref mut sound) = self.sound {
            sound.set_fast_forward(enabled);
        }
    }
}

#[cfg(test)]
mod vramdma_tests {
    use super::*;

    /// A CGB machine with a blank 32 KiB cartridge. Nothing here needs a real
    /// game: every value the tests below feed the DMA registers is something
    /// ordinary guest code can write.
    fn cgb() -> MemoryManagementUnit<'static> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0143] = 0xC0; // CGB-only: the DMA registers exist at all
        let mut mem = MemoryManagementUnit::new_cgb(rom, None).unwrap();
        // A recognisable source block in WRAM, and the LCD off — which is where
        // a game does its bulk VRAM loading, and where three of SameSuite's four
        // DMA tests run.
        for i in 0..0x40u16 {
            mem.wb(0xC000 + i, (i as u8) ^ 0x5A);
        }
        mem.wb(0xFF40, 0x00);
        mem
    }

    /// Point HDMA1-4 at `src`/`dst`, with junk in every bit the hardware drops.
    fn arm(mem: &mut MemoryManagementUnit, src: u16, dst: u16) {
        mem.wb(0xFF51, (src >> 8) as u8);
        mem.wb(0xFF52, (src as u8) | 0x0F);
        mem.wb(0xFF53, ((dst >> 8) as u8) | 0xE0);
        mem.wb(0xFF54, (dst as u8) | 0x0F);
    }

    fn vram(mem: &mut MemoryManagementUnit, a: u16, n: u16) -> Vec<u8> {
        (0..n).map(|i| mem.rb(a + i)).collect()
    }

    #[test]
    fn an_hdma_source_outside_the_legal_ranges_does_not_kill_the_process() {
        let mut mem = cgb();
        // $8000 is VRAM — not a legal HDMA source. Hardware ignores/masks it;
        // it must not abort us.
        mem.wb(0xFF51, 0x80);
        mem.wb(0xFF52, 0x00);
        mem.wb(0xFF53, 0x00);
        mem.wb(0xFF54, 0x00);
        mem.wb(0xFF55, 0x00); // GDMA, one block
        mem.do_cycle(4);
    }

    #[test]
    fn a_vram_dma_running_off_the_end_of_vram_does_not_kill_the_process() {
        let mut mem = cgb();
        arm(&mut mem, 0xC000, 0x9FF0);
        mem.wb(0xFF55, 0x7F); // GDMA, 128 blocks — runs far past $9FFF
        mem.do_cycle(4);
        // The destination wrapped back to the start of VRAM rather than
        // escaping into cartridge RAM.
        assert_eq!(vram(&mut mem, 0x9FF0, 4), vec![0x5A, 0x5B, 0x58, 0x59]);
        assert_eq!(vram(&mut mem, 0x8000, 4), vec![0x4A, 0x4B, 0x48, 0x49]);
    }

    /// SameSuite `dma/gdma_addr_mask`: the low 4 bits of both addresses and the
    /// top 3 bits of the destination are dropped, so junk in them lands the
    /// transfer at the aligned address anyway.
    #[test]
    fn the_dma_addresses_are_masked() {
        let mut mem = cgb();
        arm(&mut mem, 0xC000, 0x8800);
        mem.wb(0xFF55, 0x00); // GDMA, one block
        mem.do_cycle(4);

        let want: Vec<u8> = (0..0x10u8).map(|i| i ^ 0x5A).collect();
        assert_eq!(vram(&mut mem, 0x8800, 0x10), want, "landed at $8800");
        assert!(
            vram(&mut mem, 0x8810, 0x10).iter().all(|&b| b == 0),
            "nothing beyond the one block"
        );
    }

    /// SameSuite `dma/gbc_dma_cont`: HDMA1-4 are the transfer's own counters, so
    /// a second `$FF55` write with no re-write of them continues where the first
    /// stopped instead of copying the same block again.
    #[test]
    fn a_second_transfer_continues_where_the_last_one_stopped() {
        let mut mem = cgb();
        arm(&mut mem, 0xC000, 0x8800);
        mem.wb(0xFF55, 0x00);
        mem.do_cycle(4);
        mem.wb(0xFF55, 0x00);
        mem.do_cycle(4);

        let want: Vec<u8> = (0..0x20u8).map(|i| i ^ 0x5A).collect();
        assert_eq!(vram(&mut mem, 0x8800, 0x20), want);
    }

    /// SameSuite `dma/hdma_lcd_off`: with the LCD off the PPU's mode reads 0
    /// forever, so an HBlank transfer armed then moves exactly one block — and
    /// then stalls, because there is no *entry* into mode 0 to trigger the next.
    #[test]
    fn an_hblank_transfer_started_with_the_lcd_off_moves_one_block_and_stalls() {
        let mut mem = cgb();
        arm(&mut mem, 0xC000, 0x8800);
        mem.wb(0xFF55, 0x83); // HBlank DMA, 4 blocks
        mem.do_cycle(4);

        assert_eq!(mem.rb(0xFF55), 0x02, "one block done, three left, running");
        for _ in 0..1000 {
            mem.do_cycle(4);
        }
        assert_eq!(mem.rb(0xFF55), 0x02, "and it never moves another");

        let want: Vec<u8> = (0..0x10u8).map(|i| i ^ 0x5A).collect();
        assert_eq!(vram(&mut mem, 0x8800, 0x10), want);
        assert!(vram(&mut mem, 0x8810, 0x10).iter().all(|&b| b == 0));

        // Cancelling writes the length register too, so $FF55 reads back $80 —
        // not $80 | the blocks that were left.
        mem.wb(0xFF55, 0x00);
        assert_eq!(mem.rb(0xFF55), 0x80);
    }

    /// The same one-shot with the LCD on: armed during mode 0, the first block
    /// moves at once rather than waiting for the *next* HBlank (SameSuite
    /// `dma/hdma_mode0`).
    #[test]
    fn an_hblank_transfer_armed_during_mode_0_moves_a_block_at_once() {
        let mut mem = cgb();
        mem.wb(0xFF40, 0x80);
        // The line the PPU *restarts* on has no OAM scan — `STAT` reads mode 0
        // there and goes straight to mode 3 (`Gpu::skip_first_oam`; Mooneye
        // `acceptance/ppu/lcdon_timing-GS`: "line 0 starts with mode 0 and goes
        // straight to mode 3"). So walk on to the next line's real mode 2
        // rather than assuming the enable puts us in one.
        for _ in 0..200 {
            if mem.rb(0xFF41) & 0x03 == 2 {
                break;
            }
            mem.do_cycle(4);
        }
        assert_eq!(mem.rb(0xFF41) & 0x03, 2, "not in HBlank yet");
        arm(&mut mem, 0xC000, 0x8800);
        mem.wb(0xFF55, 0x83);
        mem.do_cycle(4);
        // Bit 7 reads 0 while a transfer is live, so this is "3 blocks left".
        assert_eq!(mem.rb(0xFF55), 0x03, "armed in mode 2: nothing moved");

        // Walk into mode 0 and let the HBlank entry move a block.
        while mem.rb(0xFF41) & 0x03 != 0 {
            mem.do_cycle(4);
        }
        mem.do_cycle(4);
        assert_eq!(mem.rb(0xFF55), 0x02, "the HBlank entry moved one block");

        // Re-arming while already in mode 0 moves one immediately.
        mem.wb(0xFF55, 0x00); // cancel
        mem.wb(0xFF55, 0x83);
        mem.do_cycle(4);
        assert_eq!(mem.rb(0xFF55), 0x02);
    }

    /// HDMA1-4 are write-only (Pan Docs, "VRAM DMA transfers").
    #[test]
    fn the_dma_address_registers_read_back_as_open_bus() {
        let mut mem = cgb();
        arm(&mut mem, 0xC000, 0x8800);
        for a in 0xFF51..=0xFF54u16 {
            assert_eq!(mem.rb(a), 0xFF, "{a:04X}");
        }
        assert_eq!(mem.rb(0xFF55), 0xFF, "idle: bit 7 set, length $7F");
    }
}

/// The DMG OAM-corruption bug, at the machine level.
///
/// [`crate::gpu::oambug`]'s own tests pin the three corruption *patterns*;
/// these pin the parts that only exist once a CPU, an MMU and a PPU are wired
/// together: which console has the defect, which bus cycles trigger it, and
/// when in the scanline they have to land. Nothing here needs a ROM — every
/// value fed to the bus is something ordinary guest code can produce.
#[cfg(test)]
mod oam_bug_tests {
    use super::*;
    use crate::mode::Model;

    /// A machine with a blank 32 KiB cartridge, OAM filled with a recognisable
    /// pattern, and the PPU parked mid-way through the OAM scan of a visible
    /// scanline — which is the only place the bug can happen.
    fn machine_in_oam_scan(model: Model) -> MemoryManagementUnit<'static> {
        machine_with_header(model, 0x00)
    }

    fn machine_with_header(model: Model, cgb_flag: u8) -> MemoryManagementUnit<'static> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0143] = cgb_flag;
        let mut mem = MemoryManagementUnit::new_with_model(rom, None, model).unwrap();
        mem.wb(0xFF40, 0x00); // LCD off: OAM is ours to fill
        for i in 0..0xA0u16 {
            mem.wb(0xFE00 + i, i as u8);
        }
        mem.wb(0xFF40, 0x81); // LCD on
                              // Step past line 0 — which the PPU restarts on with no OAM scan — and
                              // then far enough into the next line's scan that the phase correction
                              // cannot carry the prediction out of mode 2 at either end.
        mem.gpu.do_cycle(456 + 40);
        mem
    }

    fn oam(mem: &MemoryManagementUnit) -> Vec<u8> {
        mem.gpu.voam.to_vec()
    }

    /// Run `f` as if it were one M-cycle inside an executing instruction.
    fn during_instruction(
        mem: &mut MemoryManagementUnit,
        f: impl FnOnce(&mut MemoryManagementUnit),
    ) {
        let prev = mem.set_bus_ticking(true);
        f(mem);
        mem.set_bus_ticking(prev);
        mem.bus_ticked = 0;
    }

    #[test]
    fn a_16_bit_increment_over_oam_corrupts_it_on_a_dmg() {
        let mut mem = machine_in_oam_scan(Model::Dmg);
        let before = oam(&mem);
        during_instruction(&mut mem, |m| m.idu_tick(0xFE00));
        assert_ne!(oam(&mem), before, "inc rr over OAM left OAM untouched");
    }

    /// The whole point of the IDU hook: this cycle carries no memory access at
    /// all, so an implementation that only watches `rb`/`wb` sees nothing.
    #[test]
    fn the_trigger_is_the_address_bus_not_a_memory_access() {
        let mut mem = machine_in_oam_scan(Model::Dmg);
        let before = oam(&mem);
        // `add hl,rr` and friends go through the ALU and never assert the
        // address, so nothing calls `idu_tick` for them — Blargg
        // `oam_bug/3-non_causes`. A value outside the page must also be inert.
        during_instruction(&mut mem, |m| m.idu_tick(0xFDFF));
        during_instruction(&mut mem, |m| m.idu_tick(0xFF00));
        assert_eq!(
            oam(&mem),
            before,
            "an address outside $FE00-$FEFF triggered"
        );
    }

    /// `$FEA0-$FEFF` is prohibited memory, not a hole: it shares OAM's bus, so
    /// a `pop` with `SP` up there corrupts OAM just the same.
    #[test]
    fn the_prohibited_region_is_on_the_same_bus() {
        let mut mem = machine_in_oam_scan(Model::Dmg);
        let before = oam(&mem);
        during_instruction(&mut mem, |m| m.idu_tick(0xFEF0));
        assert_ne!(oam(&mem), before, "$FEF0 is on OAM's bus and must trigger");
    }

    /// Pan Docs: "Game Boy Color and Advance are not affected by this bug, even
    /// when running monochrome software." `ColorAsClassic` is CGB *hardware* in
    /// compatibility mode, so it is excluded too — which is why
    /// `blargg/oam_bug@cgb` is pinned FAIL in the baseline.
    #[test]
    fn color_hardware_does_not_have_the_defect() {
        // A CGB cartridge on a CGB, and a monochrome one on the same console —
        // compatibility mode is still Color silicon.
        for (model, flag) in [(Model::Cgb, 0xC0u8), (Model::Cgb, 0x00)] {
            let mut mem = machine_with_header(model, flag);
            assert_ne!(mem.gbmode, GbMode::Classic, "wanted CGB hardware");
            let before = oam(&mem);
            during_instruction(&mut mem, |m| m.idu_tick(0xFE00));
            during_instruction(&mut mem, |m| {
                m.rb(0xFE00);
            });
            during_instruction(&mut mem, |m| m.wb(0xFE00, 0x55));
            assert_eq!(
                oam(&mem)[8..],
                before[8..],
                "{model:?} (header {flag:#04X}) corrupted OAM: CGB silicon has no OAM bug"
            );
        }
    }

    /// Outside the OAM scan there is nothing for the CPU to collide with. Both
    /// the LCD being off and the PPU being past mode 2 must be inert — Blargg
    /// `oam_bug/3-non_causes` case 2 and `6-timing_no_bug`.
    #[test]
    fn nothing_happens_outside_the_oam_scan() {
        let mut mem = machine_in_oam_scan(Model::Dmg);
        mem.gpu.do_cycle(80); // past the scan, into pixel transfer
        let before = oam(&mem);
        during_instruction(&mut mem, |m| m.idu_tick(0xFE00));
        assert_eq!(oam(&mem), before, "mode 3 corrupted OAM");

        let mut mem = machine_in_oam_scan(Model::Dmg);
        mem.wb(0xFF40, 0x01); // LCD off
        let before = oam(&mem);
        during_instruction(&mut mem, |m| m.idu_tick(0xFE00));
        assert_eq!(oam(&mem), before, "an LCD-off access corrupted OAM");
    }

    /// An observer must not change the run it is watching: `debug_peek` and
    /// `debug_poke` suppress the bus tick, and the OAM bug rides on that same
    /// flag. Without this a trainer panel reading OAM ten times a second would
    /// quietly garble the sprites it was reporting on.
    #[test]
    fn a_debug_peek_or_poke_of_oam_cannot_corrupt_it() {
        let mut mem = machine_in_oam_scan(Model::Dmg);
        let before = oam(&mem);
        mem.debug_peek(0xFE00);
        mem.debug_poke(0xFE00, before[0]);
        assert_eq!(oam(&mem), before, "an inspector perturbed the machine");
    }

    /// A `pop`/`ld a,[hl±]` flags its read as sharing an M-cycle with the IDU.
    /// The flag is keyed on the address so it cannot survive to be consumed by
    /// some *later*, unrelated OAM read — which would silently apply the wrong
    /// corruption pattern.
    #[test]
    fn the_idu_share_flag_cannot_leak_to_a_later_access() {
        let mut mem = machine_in_oam_scan(Model::Dmg);
        mem.share_idu(0xC000);
        assert!(!mem.idu_share, "a non-OAM address must not arm the flag");

        mem.share_idu(0xFE00);
        assert!(mem.idu_share);
        during_instruction(&mut mem, |m| {
            m.rb(0xFE00);
        });
        assert!(
            !mem.idu_share,
            "the flag outlived the access that consumed it"
        );
    }

    /// The read-and-IDU pattern is genuinely a third pattern, not a relabelled
    /// read: at the same row it must produce a different OAM.
    #[test]
    fn a_read_sharing_its_cycle_with_the_idu_is_its_own_pattern() {
        let mut plain = machine_in_oam_scan(Model::Dmg);
        during_instruction(&mut plain, |m| {
            m.rb(0xFE00);
        });

        let mut shared = machine_in_oam_scan(Model::Dmg);
        shared.share_idu(0xFE00);
        during_instruction(&mut shared, |m| {
            m.rb(0xFE00);
        });

        assert_ne!(oam(&plain), oam(&shared));
    }
}

/// The state the boot ROM leaves behind — see [`MemoryManagementUnit::set_initial`].
///
/// These are transcriptions of the tables Mooneye's `boot_hwio` ROMs walk, so
/// the whole of `$FF00-$FF7F` is pinned here in a form a reader can diff
/// against the ROM source, not just the handful of values that usually get
/// quoted. Both ROMs skip the registers that move with time (`DIV`, `LY`,
/// `STAT`) or with the machine (wave RAM, `DMA`, the object palettes), and so
/// does this test — the counter's exact value is asserted separately below,
/// which is what `boot_div` measures.
#[cfg(test)]
mod boot_state_tests {
    use super::*;

    /// The APU is installed after the machine boots, so a headless MMU has no
    /// sound chip and reads `$FF` across `NR10-NR52`. Every test here wants a
    /// complete machine.
    struct NullPlayer;
    impl crate::sound::AudioPlayer for NullPlayer {
        fn play(&mut self, _l: &[f32], _r: &[f32]) {}
        fn samples_rate(&self) -> u32 {
            44_100
        }
        fn underflowed(&self) -> bool {
            false
        }
    }

    fn machine(model: crate::mode::Model) -> MemoryManagementUnit<'static> {
        let rom = vec![0u8; 0x8000];
        let mut mem = MemoryManagementUnit::new_with_model(rom, None, model).unwrap();
        mem.enable_audio(Box::new(NullPlayer));
        mem
    }

    /// `(address, expected)`, from `mooneye-test-suite/acceptance/boot_hwio-dmgABCmgb.s`.
    const DMG_HWIO: &[(u16, u8)] = &[
        (0xFF00, 0xCF), // P1: both select lines pulled low, nothing pressed
        (0xFF01, 0x00),
        (0xFF02, 0x7E),
        (0xFF03, 0xFF),
        (0xFF05, 0x00),
        (0xFF06, 0x00),
        (0xFF07, 0xF8),
        (0xFF0F, 0xE1), // IF: the boot ROM's VBlank is still pending
        (0xFF10, 0x80),
        (0xFF11, 0xBF),
        (0xFF12, 0xF3),
        (0xFF13, 0xFF),
        (0xFF14, 0xBF),
        (0xFF16, 0x3F),
        (0xFF17, 0x00),
        (0xFF19, 0xBF),
        (0xFF1A, 0x7F),
        (0xFF1C, 0x9F),
        (0xFF1E, 0xBF),
        (0xFF20, 0xFF),
        (0xFF21, 0x00),
        (0xFF22, 0x00),
        (0xFF23, 0xBF),
        (0xFF24, 0x77),
        (0xFF25, 0xF3),
        (0xFF26, 0xF1), // NR52: powered on, channel 1 still flagged enabled
        (0xFF40, 0x91),
        (0xFF42, 0x00),
        (0xFF43, 0x00),
        (0xFF45, 0x00),
        (0xFF47, 0xFC),
        (0xFF4A, 0x00),
        (0xFF4B, 0x00),
        (0xFF4C, 0xFF),
        (0xFF4D, 0xFF),
        (0xFF4F, 0xFF), // a DMG has no VRAM bank register at all
        (0xFF68, 0xFF),
        (0xFF70, 0xFF),
        (0xFFFF, 0x00),
    ];

    /// The differences a Color console makes, from `misc/boot_hwio-C.s` (which
    /// also runs a DMG cartridge, so this is compatibility mode).
    const CGB_HWIO: &[(u16, u8)] = &[
        (0xFF00, 0xFF), // the CGB boot ROM leaves the select lines released
        (0xFF4F, 0xFE), // VBK reads back even though its writes are ignored
        (0xFF68, 0xC8), // BCPS: one past the compatibility palettes, auto-inc on
        (0xFF69, 0xFF), // ...but the data ports stay closed in compat mode
        (0xFF6A, 0xD0),
        (0xFF6B, 0xFF),
        (0xFF6C, 0xFF),
        (0xFF70, 0xFF),
        (0xFF72, 0x00),
        (0xFF73, 0x00),
        (0xFF75, 0x8F),
    ];

    #[test]
    fn a_dmg_boots_into_the_state_its_boot_rom_leaves() {
        let mut mem = machine(crate::mode::Model::Dmg);
        for &(a, want) in DMG_HWIO {
            assert_eq!(mem.rb(a), want, "${a:04X}");
        }
        assert_eq!(mem.rb(0xFF04), 0xAB, "DIV = the counter's top byte");
    }

    #[test]
    fn a_color_console_boots_into_a_different_state() {
        let mut mem = machine(crate::mode::Model::Cgb);
        // Everything the two consoles agree on.
        for &(a, want) in DMG_HWIO {
            if CGB_HWIO.iter().any(|&(b, _)| b == a) {
                continue;
            }
            assert_eq!(mem.rb(a), want, "${a:04X}");
        }
        for &(a, want) in CGB_HWIO {
            assert_eq!(mem.rb(a), want, "${a:04X}");
        }
        assert_eq!(mem.rb(0xFF04), 0x26);
    }

    /// The boot ROM leaves the Nintendo logo's **tilemap** entries behind, and
    /// they are still on screen when a game that never clears the map draws its
    /// first frame — which is exactly what the GB Emulator Shootout's reference
    /// screenshots for SameSuite's `sgb/` tests show.
    #[test]
    fn the_boot_rom_leaves_its_logo_in_the_tilemap() {
        let mut mem = machine(crate::mode::Model::Dmg);
        for (i, addr) in (0x9904u16..=0x990F).enumerate() {
            assert_eq!(mem.rb(addr), 0x01 + i as u8, "${addr:04X}");
        }
        for (i, addr) in (0x9924u16..=0x992F).enumerate() {
            assert_eq!(mem.rb(addr), 0x0D + i as u8, "${addr:04X}");
        }
        assert_eq!(mem.rb(0x9910), 0x19, "the ® tile");
        // Everything around it is untouched.
        assert_eq!(mem.rb(0x9903), 0x00);
        assert_eq!(mem.rb(0x9911), 0x00);
        assert_eq!(mem.rb(0x9944), 0x00);

        // A Super Game Boy is DMG hardware and boots the same way.
        let mut sgb = machine(crate::mode::Model::Sgb);
        assert_eq!(sgb.rb(0x9904), 0x01);

        // The Color boot ROM draws the logo differently and this is not a
        // claim about it; it is left as it was.
        let mut cgb = machine(crate::mode::Model::Cgb);
        assert_eq!(cgb.rb(0x9904), 0x00);
    }

    fn machine_with_header(
        model: crate::mode::Model,
        header: [u8; 48],
    ) -> MemoryManagementUnit<'static> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0104..0x0134].copy_from_slice(&header);
        let mut mem = MemoryManagementUnit::new_with_model(rom, None, model).unwrap();
        mem.enable_audio(Box::new(NullPlayer));
        mem
    }

    /// `double_nibble` is the "chunky pixel" horizontal doubling the boot
    /// ROMs apply to the header logo: bit 3 (leftmost of the source nibble's
    /// 4 pixels) becomes output bits 7 and 6, and so on down to bit 0 →
    /// bits 1 and 0.
    #[test]
    fn nibbles_double_into_bytes_leftmost_bit_first() {
        assert_eq!(double_nibble(0x0), 0x00);
        assert_eq!(double_nibble(0xF), 0xFF);
        assert_eq!(double_nibble(0x8), 0xC0, "leftmost bit alone -> bits 7,6");
        assert_eq!(double_nibble(0x1), 0x03, "rightmost bit alone -> bits 1,0");
        assert_eq!(double_nibble(0xA), 0xCC, "0b1010 -> 0b11001100");
    }

    /// `set_boot_logo_tiles` derives every byte from the cartridge's own
    /// header — verified here with a synthetic, non-Nintendo pattern, because
    /// the algorithm is what is under test, not any specific bitmap. See its
    /// doc comment for the licence argument and the citations.
    #[test]
    fn the_boot_rom_decompresses_the_headers_own_logo_tiles() {
        let mut header = [0u8; 48];
        // Tile $01 (the first tile of the top half) comes from header bytes
        // 0-1 ($0104-$0105).
        header[0] = 0xF0;
        header[1] = 0x00;
        // Tile $0D (the first tile of the *bottom* half) comes from header
        // bytes 24-25 ($011C-$011D) -- Pan Docs' own split point.
        header[24] = 0x00;
        header[25] = 0x0F;
        let mut mem = machine_with_header(crate::mode::Model::Dmg, header);

        // Nibble $F (rows 0-1 of tile $01's source block) doubles to $FF on
        // the low plane; the high plane is never written. The other three
        // source nibbles are zero, so the rest of the tile is blank.
        let want_01 = [
            0xFF, 0x00, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00,
        ];
        for (i, want) in want_01.into_iter().enumerate() {
            assert_eq!(mem.rb(0x8010 + i as u16), want, "tile $01 byte {i}");
        }

        // Nibble $F is the *last* of tile $0D's four source nibbles, so it
        // lands on the tile's rows 6-7 instead.
        let want_0d = [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF,
            0x00, 0xFF, 0x00,
        ];
        for (i, want) in want_0d.into_iter().enumerate() {
            assert_eq!(
                mem.rb(0x8010 + 12 * 16 + i as u16),
                want,
                "tile $0D byte {i}"
            );
        }

        // Tile $19 -- the ® glyph's own slot -- is never written by this
        // function, whatever the header holds: there is nothing in the
        // cartridge to derive it from.
        for addr in 0x8190u16..=0x819F {
            assert_eq!(mem.rb(addr), 0x00, "${addr:04X} untouched");
        }

        // A Color console running in DMG-compatibility mode does not take
        // this path at all -- see the module doc on `set_boot_logo_tilemap`.
        let mut cgb = machine_with_header(crate::mode::Model::Cgb, header);
        assert_eq!(cgb.rb(0x8010), 0x00);
    }

    /// A Super Game Boy answers `$FF00` with a player index once a game has
    /// asked for multiplayer, and that has to survive the trip through the MMU
    /// — the joypad and the ICD2 share the register.
    #[test]
    fn a_super_game_boy_reports_the_player_through_the_joypad_register() {
        let mut mem = machine(crate::mode::Model::Sgb);
        assert!(mem.sgb.is_some(), "an SGB has an ICD2");
        assert_eq!(mem.rb(0xFF00) & 0x0F, 0x0F, "one player until asked");

        // MLT_REQ 1 = two players, sent as the pulse sequence a game uses.
        let mut packet = [0u8; 16];
        packet[0] = (0x11 << 3) | 1;
        packet[1] = 0x01;
        let mut writes = vec![0x00u8, 0x30];
        for b in packet {
            for i in 0..8 {
                writes.push(if b >> i & 1 == 1 { 0x10 } else { 0x20 });
                writes.push(0x30);
            }
        }
        writes.extend_from_slice(&[0x20, 0x30]);
        for w in writes {
            mem.wb(0xFF00, w);
        }
        assert_eq!(mem.rb(0xFF00) & 0x0F, 0x0F, "player 1");
        mem.wb(0xFF00, 0x10);
        mem.wb(0xFF00, 0x30);
        assert_eq!(mem.rb(0xFF00) & 0x0F, 0x0E, "player 2");

        // A DMG has no ICD2, so the same pulses mean nothing.
        let mut dmg = machine(crate::mode::Model::Dmg);
        assert!(dmg.sgb.is_none());
        assert_eq!(dmg.rb(0xFF00) & 0x0F, 0x0F);
    }

    /// The counter is not merely "`DIV` reads `$AB`": `boot_div` pins its low
    /// bits too, by reading `DIV` six times at known offsets and requiring the
    /// reads to land either side of an increment. The first read on a DMG is
    /// 13 M-cycles in and must see `$AC00` exactly.
    #[test]
    fn the_system_counter_boots_with_the_right_phase() {
        let mut mem = machine(crate::mode::Model::Dmg);
        // 13 M-cycles of bus accesses, then the read: exactly the count from
        // `$0100` to the `ldh a,(DIV)` in `acceptance/boot_div-dmgABCmgb`.
        mem.set_bus_ticking(true);
        for _ in 0..13 {
            mem.rb(0xC000);
        }
        assert_eq!(mem.rb(0xFF04), 0xAC, "DIV has just incremented to $AC");
        mem.set_bus_ticking(false);
    }

    /// An internally clocked transfer is not instantaneous, and with nothing
    /// attached it still finishes — shifting in the idle-high `$FF` an empty
    /// link port presents (`acceptance/serial/boot_sclk_align`).
    #[test]
    fn a_link_transfer_takes_eight_shift_clocks_and_completes_unanswered() {
        let mut mem = machine(crate::mode::Model::Dmg);
        mem.wb(0xFF01, 0x42);
        mem.wb(0xFF02, 0x81);
        assert_eq!(mem.rb(0xFF01), 0x42, "nothing has been shifted yet");
        assert_eq!(mem.rb(0xFF02) & 0x80, 0x80, "the transfer is still running");
        // Seven shift clocks (512 T each) is not enough; the eighth is.
        mem.do_cycle(512 * 7);
        assert_eq!(mem.rb(0xFF01), 0x42);
        assert_eq!(
            mem.intf & 0x08,
            0,
            "no serial interrupt before the last bit"
        );
        mem.do_cycle(512);
        assert_eq!(mem.rb(0xFF01), 0xFF, "an unplugged cable reads all ones");
        assert_eq!(
            mem.rb(0xFF02) & 0x80,
            0,
            "SC's start bit clears on completion"
        );
        assert_ne!(mem.intf & 0x08, 0, "and the serial interrupt is requested");
    }

    /// The three consoles whose `boot_hwio` differs from a DMG's, each pinned
    /// against the ROM that measures it. These are *whole machines*, not
    /// variations on a theme: get any one of these wrong and its ROM says so.
    #[test]
    fn each_console_revision_boots_into_its_own_state() {
        use crate::mode::Model;

        // `acceptance/boot_hwio-S`. A Super Game Boy's boot ROM ends by
        // clocking packets out of `P1`, so it leaves the select lines released
        // like a CGB's rather than pulled low like a DMG's — and it sets
        // channel 1 up without ever triggering it, so `NR52` reads `$F0`.
        let mut sgb = machine(Model::Sgb);
        assert_eq!(sgb.rb(0xFF00), 0xFF, "P1 released on an SGB");
        assert_eq!(sgb.rb(0xFF26), 0xF0, "no start-up beep on an SGB");
        // Everything else it checks is the DMG's.
        for &(a, want) in DMG_HWIO {
            if matches!(a, 0xFF00 | 0xFF26) {
                continue;
            }
            assert_eq!(sgb.rb(a), want, "SGB ${a:04X}");
        }

        // An MGB is a DMG in every byte of I/O; only `A` differs, which is the
        // CPU's business (`Registers::boot`).
        let mut mgb = machine(Model::Mgb);
        for &(a, want) in DMG_HWIO {
            assert_eq!(mgb.rb(a), want, "MGB ${a:04X}");
        }

        // `acceptance/boot_hwio-dmg0`: a shorter boot ROM, so a much smaller
        // system counter and a PPU that has not reached line 0 yet.
        let mut dmg0 = machine(Model::Dmg0);
        assert_eq!(dmg0.rb(0xFF04), 0x18, "DIV on a DMG 0");
        assert_eq!(dmg0.rb(0xFF44), 145, "LY: still in VBlank at hand-off");
        for &(a, want) in DMG_HWIO {
            // The three the ROM's own mask skips or expects differently.
            if matches!(a, 0xFF04 | 0xFF41 | 0xFF44) {
                continue;
            }
            assert_eq!(dmg0.rb(a), want, "DMG0 ${a:04X}");
        }
    }

    /// A Super Game Boy's boot ROM clocks the cartridge header to the SNES one
    /// bit at a time through an **unbalanced** loop, so how long it takes — and
    /// therefore the system counter the cartridge inherits — is a function of
    /// the header bytes. Mooneye ships `boot_div-S` and `boot_div2-S`, the same
    /// program with a different global checksum, precisely to catch an emulator
    /// that hard-codes one duration.
    #[test]
    fn an_sgb_boot_takes_longer_for_a_header_full_of_zero_bits() {
        /// The 96-byte packet buffer, built the long way round so the packing
        /// in `sgb_boot_system_counter` is checked against something other than
        /// itself — the 6/14 split, the six packet ids, the per-packet checksum
        /// and the eight bytes `.clearBuffer` zeroes are all load-bearing, and
        /// an off-by-one in any of them is worth several M-cycles.
        fn buffer(header: &dyn Fn(u16) -> u8) -> Vec<u8> {
            let mut out = vec![0u8; 8]; // the zeroed tail
            let mut addr = 0x014Fu16;
            for (i, id) in [0xFBu8, 0xF9, 0xF7, 0xF5, 0xF3, 0xF1]
                .into_iter()
                .enumerate()
            {
                let mut bytes = Vec::new();
                for _ in 0..if i == 0 { 6 } else { 14 } {
                    bytes.push(header(addr));
                    addr = addr.wrapping_sub(1);
                }
                let cks = bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b));
                out.push(id);
                out.push(cks);
                out.extend_from_slice(&bytes);
            }
            assert_eq!(out.len(), 96, "six packets of sixteen bytes");
            assert_eq!(addr, 0x0103, "the whole header $0104-$014F, and no more");
            out
        }
        let expect = |h: &dyn Fn(u16) -> u8| {
            let zeros: u32 = buffer(h).iter().map(|b| b.count_zeros()).sum();
            SGB_BOOT_SYSTEM_COUNTER_ALL_ONES.wrapping_add(4 * zeros as u16)
        };

        for h in [
            &(|_: u16| 0xFFu8) as &dyn Fn(u16) -> u8,
            &|_| 0x00,
            &|a| (a & 0xFF) as u8,
            &|a| if a == 0x014E { 0x12 } else { 0xFF },
        ] {
            assert_eq!(sgb_boot_system_counter(h), expect(h));
        }

        // The property `boot_div2-S` exists to enforce: two cartridges that
        // differ only in their global checksum do **not** boot in the same time.
        let a = sgb_boot_system_counter(&|a| if a == 0x014E { 0x12 } else { 0xFF });
        let b = sgb_boot_system_counter(&|a| if a == 0x014E { 0xA7 } else { 0xFF });
        assert_ne!(a, b, "the boot duration must follow the header");
        // And a header of zero bits is the slowest one there is.
        assert!(sgb_boot_system_counter(&|_| 0x00) > sgb_boot_system_counter(&|_| 0xFF));
    }

    /// Only the first DMG needs its PPU parked away from line 0; guessing a
    /// position for a console whose `boot_hwio` ROM does not measure `LY`
    /// would be inventing state.
    #[test]
    fn only_the_console_whose_rom_measures_it_gets_a_ppu_boot_position() {
        use crate::mode::Model;
        assert_eq!(boot_ppu_dot(Model::Dmg0), Some(DMG0_BOOT_PPU_DOT));
        for m in [
            Model::Dmg,
            Model::Mgb,
            Model::Sgb,
            Model::Sgb2,
            Model::Cgb,
            Model::Cgb0,
            Model::Agb,
        ] {
            assert_eq!(boot_ppu_dot(m), None, "{m:?}");
        }
    }
}

/// OAM DMA's bus conflict — `TGB-DMA-001`, and the largest single named cause
/// in the c-sp aggregation.
///
/// Every expectation here is Gambatte's, named at the assertion: those ROMs
/// were verified on a DMG-CPU-08 and a CPU CGB C, and the whole point of the
/// model is that it is one physical rule rather than a table of cases. See
/// [`MemoryManagementUnit::dma_conflict`] and `docs/measured/timing.md`.
#[cfg(test)]
mod dma_conflict_tests {
    use super::*;

    fn machine(model: crate::mode::Model) -> MemoryManagementUnit<'static> {
        // A ROM whose low page is distinguishable from anything we put in RAM.
        let mut rom = vec![0u8; 0x8000];
        for (i, b) in rom.iter_mut().enumerate().take(0x100) {
            *b = (0x10 + i) as u8;
        }
        rom[0x0147] = 0x00;
        let mut mem = MemoryManagementUnit::new_with_model(rom, None, model).unwrap();
        // Everything below models a CPU instruction running, which is what the
        // per-access bus tick and every access-blocking rule are gated on.
        mem.bus_tick_enabled = true;
        mem
    }

    /// Fill work RAM's first page with a pattern the ROM cannot produce, and
    /// start a transfer out of it. Returns the machine parked one M-cycle
    /// before the first byte moves, so the next access is the conflict.
    fn dma_from(model: crate::mode::Model, page: u8) -> MemoryManagementUnit<'static> {
        let mut mem = machine(model);
        for i in 0..0xA0usize {
            mem.wram[i] = (0xA0 + i) as u8;
        }
        mem.wb(0xFF46, page);
        // M = 0 was the write itself; M = 1 is idle.
        mem.internal_tick();
        mem
    }

    /// `oamdma_src0000_busypop7FFF`: the ROM byte at `$7FFF` is `$55` and the
    /// test expects the byte the transfer was reading instead.
    #[test]
    fn a_read_on_the_transfers_bus_reads_the_wires() {
        let mut mem = dma_from(crate::mode::Model::Dmg, 0xC0);
        // The first byte of the transfer is work RAM's `$A0`; a DMG's work RAM
        // is on the cartridge bus, so a ROM read in the same M-cycle sees it.
        assert_eq!(mem.rb(0x0000), 0xA0);
        assert_eq!(mem.rb(0x0000), 0xA1, "and the next M-cycle's byte after it");
    }

    /// HRAM is internal to the chip and is on neither bus — which is why every
    /// OAM DMA routine ever written is copied there before it is run.
    #[test]
    fn hram_and_the_io_block_are_never_conflicted() {
        let mut mem = dma_from(crate::mode::Model::Dmg, 0xC0);
        mem.zram[0] = 0x5A;
        assert_eq!(mem.rb(0xFF80), 0x5A);
        assert_eq!(mem.rb(0xFF80), 0x5A);
    }

    /// A Color console gives its work RAM a bus of its own, so a transfer out
    /// of work RAM leaves the cartridge alone — `oamdma_srcC000_busypop7FFF`
    /// expects the DMG to pop the transfer's byte and the CGB to pop the real
    /// ROM byte.
    #[test]
    fn a_color_console_keeps_work_ram_off_the_cartridge_bus() {
        let mut dmg = dma_from(crate::mode::Model::Dmg, 0xC0);
        let mut cgb = dma_from(crate::mode::Model::Cgb, 0xC0);
        assert_eq!(dmg.rb(0x0010), 0xA0, "DMG: one bus, so the read conflicts");
        assert_eq!(cgb.rb(0x0010), 0x20, "CGB: the ROM answers for itself");
        // ...and the CGB's own work-RAM bus still conflicts with itself.
        assert_eq!(cgb.rb(0xC000), 0xA1);
    }

    /// `oamdma_src0000_busypushC001` pushes during a transfer and then pops the
    /// same two addresses back, expecting what was there *before* the push: the
    /// transfer owns the bus, so the addressed memory never sees the cycle.
    #[test]
    fn a_conflicting_write_never_reaches_its_own_address() {
        let mut mem = dma_from(crate::mode::Model::Dmg, 0x00);
        mem.wram[0x100] = 0x12;
        // $C100 is on the cartridge bus on a DMG, and the transfer is reading
        // ROM, so this write is lost.
        mem.wb(0xC100, 0x99);
        assert_eq!(mem.wram[0x100], 0x12);
    }

    /// The same write is not lost entirely — it is what the transfer copies.
    /// With a cartridge source the bus is in write mode and nothing else is
    /// driving, so OAM gets the CPU's byte verbatim.
    #[test]
    fn a_conflicting_write_is_what_the_transfer_copies() {
        let mut mem = dma_from(crate::mode::Model::Dmg, 0x00);
        mem.wb(0xC100, 0x99);
        assert_eq!(mem.gpu.rb(0xFE00), 0x99);
    }

    /// ...except out of a DMG's work RAM, which is inside the SoC and keeps
    /// driving through the write cycle, so both ends reach OAM.
    /// `oamdma_srcC000_busypushC001` expects `$65 & $55 = $45` exactly where the
    /// cartridge cases expect a plain `$55`.
    #[test]
    fn a_dmg_work_ram_source_wires_the_two_together() {
        let mut mem = dma_from(crate::mode::Model::Dmg, 0xC0);
        mem.wb(0x0000, 0x55);
        assert_eq!(mem.gpu.rb(0xFE00), 0xA0 & 0x55);
    }

    /// A Color console's work-RAM bus locks the CPU out instead: the write is
    /// dropped and the transfer copies what work RAM held
    /// (`oamdma_srcC000_busypushC001` on `cgb04c`).
    #[test]
    fn a_color_work_ram_source_is_undisturbed_by_a_write() {
        let mut mem = dma_from(crate::mode::Model::Cgb, 0xC0);
        mem.wram[0x100] = 0x12;
        mem.wb(0xC100, 0x99);
        assert_eq!(mem.gpu.rb(0xFE00), 0xA0, "the transfer copied work RAM");
        assert_eq!(mem.wram[0x100], 0x12, "and the write is still lost");
    }

    /// A Color console's video bus loses the byte altogether, whether the CPU
    /// read or wrote (`oamdma_src8000_busypop9FFF` expects `$00` in OAM on
    /// `cgb04c` and the real tile byte on a DMG).
    #[test]
    fn a_color_video_bus_conflict_stores_zero() {
        let mut mem = dma_from(crate::mode::Model::Cgb, 0x80);
        mem.gpu.wb(0x8000, 0x77);
        let _ = mem.rb(0x9000);
        assert_eq!(mem.gpu.rb(0xFE00), 0x00);
    }

    /// A Color console does not fold source pages `$E0-$FF` onto echo RAM: it
    /// puts them on the cartridge bus, where nothing answers
    /// (`oamdma_srcE000_*` on `cgb04c`).
    #[test]
    fn color_source_pages_above_df_read_open_bus() {
        let mut dmg = dma_from(crate::mode::Model::Dmg, 0xE0);
        let mut cgb = dma_from(crate::mode::Model::Cgb, 0xE0);
        dmg.internal_tick();
        cgb.internal_tick();
        assert_eq!(dmg.gpu.rb(0xFE00), 0xA0, "DMG: echo of work RAM");
        assert_eq!(cgb.gpu.rb(0xFE00), 0xFF, "CGB: nothing drives the bus");
    }

    /// The prohibited block `$FEA0-$FEFF` is not a hole: Pan Docs gives it a
    /// different answer per console, and `$FF` is only right while OAM is
    /// blocked. Gambatte's `oamdma_*_busypushFF01` pops `$FEFF` back and
    /// expects `$00` on a DMG.
    #[test]
    fn the_prohibited_block_answers_per_console() {
        let mut dmg = machine(crate::mode::Model::Dmg);
        let mut cgb = machine(crate::mode::Model::Cgb);
        let mut agb = machine(crate::mode::Model::Agb);
        for m in [&mut dmg, &mut cgb, &mut agb] {
            // Out of every PPU mode that would blank it.
            m.bus_tick_enabled = false;
        }
        assert_eq!(dmg.rb(0xFEA5), 0x00);
        assert_eq!(dmg.rb(0xFEFF), 0x00);
        assert_eq!(cgb.rb(0xFEA5), 0xAA, "the low byte's high nibble, twice");
        assert_eq!(cgb.rb(0xFEB0), 0xBB);
        assert_eq!(agb.rb(0xFEFF), 0xFF);
    }

    /// ...and it is `$FF` while a transfer owns OAM, on every console, because
    /// it is on OAM's bus.
    #[test]
    fn the_prohibited_block_is_blocked_with_oam() {
        let mut mem = dma_from(crate::mode::Model::Dmg, 0xC0);
        assert_eq!(mem.rb(0xFEA5), 0xFF);
    }

    /// Writes there keep nothing. On the Color revisions `0-D` they would —
    /// Pan Docs calls it "a unique RAM area, masked with a revision-specific
    /// value" — but no revision's mask is documented, and implementing it
    /// unmasked was measured at 26 more Gambatte rows against `cgb-acid-hell`
    /// going from 2 wrong pixels to 176. See [`read_unusable`].
    ///
    /// [`read_unusable`]: MemoryManagementUnit::read_unusable
    #[test]
    fn a_write_to_the_prohibited_block_keeps_nothing() {
        let mut mem = machine(crate::mode::Model::Cgb);
        mem.bus_tick_enabled = false;
        mem.wb(0xFEA5, 0x5A);
        assert_eq!(mem.rb(0xFEA5), 0xAA);
    }

    /// Nothing above conflicts once the transfer is over.
    #[test]
    fn the_bus_comes_back_when_the_transfer_ends() {
        let mut mem = dma_from(crate::mode::Model::Dmg, 0xC0);
        for _ in 0..0xA0 {
            mem.internal_tick();
        }
        assert_eq!(mem.rb(0x0010), 0x20);
    }

    /// A debug peek must be able to read memory during a transfer: it is not a
    /// CPU access and does not own a bus cycle.
    #[test]
    fn a_debug_peek_is_not_a_bus_cycle() {
        let mut mem = dma_from(crate::mode::Model::Dmg, 0xC0);
        assert_eq!(mem.debug_peek(0x0010), 0x20);
    }
}
