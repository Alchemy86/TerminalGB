mod oamdma;
pub(crate) mod serial;
mod timer;

use crate::gpu::{Gpu, OamBug};
use crate::input::{InputQueue, Keypad};
use crate::mbc;
use crate::mmu::oamdma::OamDma;
use crate::mmu::serial::Serial;
use crate::mmu::timer::Timer;
use crate::mode::{GbMode, GbSpeed};
#[cfg(not(target_arch = "wasm32"))]
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
fn ppu_write_commit(address: u16, old: u8, value: u8, mode: GbMode) -> WriteCommit {
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
        0xFF47..=0xFF49 => WriteCommit {
            early: 3,
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

const WRAM_SIZE: usize = 0x8000;
const ZRAM_SIZE: usize = 0x7F;

/// The 16-bit system counter as the first cartridge instruction observes it on
/// a DMG (`DIV` therefore reads `$AB`). Measured by Mooneye
/// `acceptance/boot_div-dmgABCmgb`, which pins six successive `DIV` reads and
/// so fixes the counter's low bits as well as `DIV` itself, and cross-checked
/// by `acceptance/serial/boot_sclk_align-dmgABCmgb`, which measures the serial
/// shift clock's phase against the same counter.
const DMG_BOOT_SYSTEM_COUNTER: u16 = 0xABCC;

/// The same value for a Color console, measured by Mooneye
/// `misc/boot_div-cgbABCDE`. That ROM has a **DMG** cartridge header, so what it
/// actually measures is the compatibility path; a CGB cartridge takes a
/// different route through the boot ROM and no suite ROM pins where it ends up.
/// The same value is used for both, which is right for `ColorAsClassic` and a
/// documented approximation for `Color`.
const CGB_BOOT_SYSTEM_COUNTER: u16 = 0x2678;

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
/// See [`crate::gpu::Gpu::set_boot_position`]. Only the first DMG needs one:
/// every other console this emulator models either measures line 0 (`DMG ABC`,
/// via `boot_hwio-dmgABCmgb`'s `LY = $0A`) or does not measure `LY`/`STAT` at
/// all (`boot_hwio-S` and `misc/boot_hwio-C` both skip `$FF40`, `$FF41` and
/// `$FF44` — see their own masks). Guessing a position no ROM checks would be
/// inventing state, so those are left alone.
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
    #[cfg(not(target_arch = "wasm32"))]
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
            keypad: Keypad::default(),
            sgb: None,
            input_queue: InputQueue::default(),
            gpu: Gpu::new(),
            #[cfg(not(target_arch = "wasm32"))]
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
            bus_ticked: 0,
            gpu_bus_ticked: 0,
            gpu_defer: 0,
            ppu_ticks_with_the_bus: false,
            idu_share: false,
            fault: None,
            cart_rtc,
        };
        fill_random(&mut res.wram, 42);
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
    fn read_rom(&self, address: u16) -> u8 {
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
        // why they are per console revision and not per `GbMode`.
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
    /// The tile *pixels* are not written. On hardware they are the logo the
    /// boot ROM decompressed out of the cartridge header, and every ROM that
    /// reaches this state has already replaced them by the time anything looks;
    /// the map entries are the part that survives.
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
            self.timer.do_cycle(4);
            self.intf |= self.timer.interrupt;
            self.timer.interrupt = 0;
            // The link port's shift clock is divided from the same counter, so
            // it has to be drained here, *before* the access — otherwise an
            // `SC` write landing on an edge M-cycle would consume that edge and
            // finish a shift too early.
            self.serial_shift();
            // The APU clock advances per-access too, so a wave-RAM or NR5x
            // access lands on its exact sub-instruction cycle (Blargg
            // dmg_sound 09/10/12 measure this to 2 T-cycles). Synthesis stays
            // event-driven; this only moves the "now" that catch_up syncs to.
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(ref mut sound) = self.sound {
                sound.do_cycle(4);
            }
            self.oam_dma_tick();
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
            if self.gpu.render_mode() == crate::gpu::RenderMode::Accurate
                || self.ppu_ticks_with_the_bus
            {
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
            self.bus_ticked += 4;
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
        if self.oam_dma.idle() {
            return;
        }
        if let Some((page, i)) = self.oam_dma.step() {
            let b = self.oam_dma_source(page, i);
            // Straight to the PPU: `wb` would refuse it, since OAM belongs to
            // the DMA while it runs.
            self.gpu.wb(0xFE00 + i as u16, b);
        }
    }

    /// Read one source byte for OAM DMA. This is *not* a CPU access: it must
    /// not tick the bus or trip a register side-effect. Pages `$E0-$FF` read
    /// echo RAM, as the transfer sits on the external bus.
    fn oam_dma_source(&self, page: u8, index: u8) -> u8 {
        let mut address = ((page as u16) << 8) | index as u16;
        if address >= 0xE000 {
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
        if self.bus_tick_enabled {
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            match &self.sound {
                Some(s) => {
                    apu.bool(true);
                    s.save_state(&mut apu);
                }
                None => apu.bool(false),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            apu.bool(false);
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut ar = crate::savestate::Reader::new(&apu);
            let present = ar.bool()?;
            if present {
                if let Some(s) = self.sound.as_mut() {
                    s.load_state(&mut ar)?;
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = apu;
        }

        // MBC block.
        let mbc = r.blob()?;
        let mut mr = crate::savestate::Reader::new(&mbc);
        self.mbc.load_state(&mut mr)?;
        // A save-state load replaces cartridge RAM wholesale without the game
        // ever writing it, so the battery file has to be rewritten too — else
        // the next launch resurrects the pre-load save.
        self.mbc.battery_dirty();
        Ok(())
    }

    pub fn do_cycle(&mut self, ticks: u32) -> u32 {
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
        self.oam_dma_cycles(timerticks);
        self.intf |= self.timer.interrupt;
        self.timer.interrupt = 0;
        self.serial_shift();

        self.intf |= self.keypad.interrupt;
        self.keypad.interrupt = 0;

        // Like the timer, the accurate PPU already advanced during this
        // instruction's memory accesses; apply only the remainder.
        self.gpu
            .do_cycle(gputicks.saturating_sub(self.gpu_bus_ticked));
        self.gpu_bus_ticked = 0;
        self.intf |= self.gpu.interrupt;
        self.gpu.interrupt = 0;

        // Like the timer, the APU already advanced `bus_ticked` T-cycles
        // during this instruction's accesses (bus_tick); apply only the
        // remainder so the per-instruction total is unchanged.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(ref mut sound) = self.sound {
            sound.do_cycle(timerticks);
        }

        self.intf |= self.serial.interrupt;
        self.serial.interrupt = 0;

        // The cartridge's own crystal. `gputicks` is real time — it is already
        // divided by the CGB speed multiplier — which is what the clock on the
        // cartridge runs on. See [`crate::mbc::rtc`].
        if self.cart_rtc {
            self.mbc.rtc_tick(gputicks);
        }

        gputicks
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
        // `bus_tick_enabled` is the "a CPU instruction is running" test, and it
        // guards the flag as well as the corruption: a debug peek of OAM must
        // neither corrupt it nor eat a pending `share_idu`.
        if is_oam_page(address) && self.bus_tick_enabled {
            // A read and an IDU increment sharing one M-cycle corrupt OAM
            // differently from a plain read — see `share_idu`.
            let kind = if std::mem::take(&mut self.idu_share) {
                OamBug::ReadWrite
            } else {
                OamBug::Read
            };
            self.oam_bug(kind, self.bus_ticked);
        }
        match address {
            0x0000..=0x7FFF => self.read_rom(address),
            0x8000..=0x9FFF => self.gpu.rb(address),
            0xA000..=0xBFFF => self.mbc.readram(address),
            // Echo RAM (E000-FDFF) mirrors WRAM: the same arms serve both ranges
            // because & 0x0FFF folds the echo address onto the C000-page bank.
            0xC000..=0xCFFF | 0xE000..=0xEFFF => self.wram[address as usize & 0x0FFF],
            0xD000..=0xDFFF | 0xF000..=0xFDFF => {
                self.wram[(self.wrambank * 0x1000) | address as usize & 0x0FFF]
            }
            // OAM belongs to the DMA while a transfer runs: the CPU reads
            // $FF and its writes are dropped (Pan Docs, "OAM DMA Transfer").
            0xFE00..=0xFE9F => {
                if self.oam_dma.active() {
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
                #[cfg(not(target_arch = "wasm32"))]
                return self.sound.as_mut().map_or(0xFF, |s| {
                    s.catch_up();
                    s.rb(address)
                });
                #[cfg(target_arch = "wasm32")]
                0xFF
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
            0xFF40..=0xFF4F => self.gpu.rb(address),
            0xFF51..=0xFF55 => self.hdma_read(address),
            0xFF68..=0xFF6B => self.gpu.rb(address),
            0xFF70 => self.wrambank as u8,
            0xFF72..=0xFF73 => self.undocumented_cgb_regs[address as usize - 0xFF72],
            0xFF75 => self.undocumented_cgb_regs[2] | 0b10001111,
            // CGB PCM12/PCM34: the live 4-bit digital output of each
            // channel, low nibble first (CH1/CH3 low, CH2/CH4 high). Sample
            // accurate, so the APU has to be caught up to this very cycle.
            0xFF76..=0xFF77 => {
                #[cfg(not(target_arch = "wasm32"))]
                return self.sound.as_mut().map_or(0x00, |s| {
                    s.catch_up();
                    s.rb(address)
                });
                #[cfg(target_arch = "wasm32")]
                0x00
            }
            0xFF80..=0xFFFE => self.zram[address as usize & 0x007F],
            0xFFFF => self.inte,
            // Prohibited region FEA0-FEFF and any unmapped I/O read back 0xFF.
            // Hardware's OAM-block-dependent value here is not modelled (mmu.md).
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
    }

    pub fn wb(&mut self, address: u16, value: u8) {
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
            let plan =
                ppu_write_commit(address, self.gpu.rb(address), value, self.gbmode);
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
            0x0000..=0x7FFF => self.mbc.writerom(address, value),
            0x8000..=0x9FFF => self.gpu.wb(address, value),
            0xA000..=0xBFFF => self.mbc.writeram(address, value),
            0xC000..=0xCFFF | 0xE000..=0xEFFF => {
                self.wram[address as usize & 0x0FFF] = value
            }
            0xD000..=0xDFFF | 0xF000..=0xFDFF => {
                self.wram[(self.wrambank * 0x1000) | (address as usize & 0x0FFF)] = value
            }
            0xFE00..=0xFE9F => {
                if !self.oam_dma.active() {
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
                #[cfg(not(target_arch = "wasm32"))]
                if address == 0xFF04 {
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
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(ref mut sound) = self.sound {
                    sound.catch_up();
                    sound.wb(address, value);
                }
            }
            0xFF46 => self.oam_dma.request(value),
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
            0xFF40..=0xFF4F => self.gpu.wb(address, value),
            0xFF51..=0xFF55 => self.hdma_write(address, value),
            0xFF68..=0xFF6B => self.gpu.wb(address, value),
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
            if self.gbspeed == GbSpeed::Double {
                self.gbspeed = GbSpeed::Single;
            } else {
                self.gbspeed = GbSpeed::Double;
            }
            // The APU is wired to the unmodified 4.194304 MHz clock, so it
            // must halve the CPU T-cycles it is handed at double speed —
            // otherwise every sound plays an octave high in CGB fast mode.
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(ref mut sound) = self.sound {
                sound.set_double_speed(self.gbspeed == GbSpeed::Double);
            }
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

    #[cfg(not(target_arch = "wasm32"))]
    pub fn enable_audio(&mut self, player: Box<dyn crate::sound::AudioPlayer>) {
        let fresh = self.sound.is_none();
        if fresh {
            self.sound = Some(crate::sound::Sound::new());
        }
        let sound = self.sound.as_mut().unwrap();
        // DMG carts get DMG APU semantics (length counters survive power-off
        // etc. — what Blargg's dmg_sound asserts); CGB carts get CGB rules.
        sound.set_mode(self.gbmode != GbMode::Color);
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
    #[cfg(not(target_arch = "wasm32"))]
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
