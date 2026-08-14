mod oamdma;
mod serial;
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
const OAM_BUG_PHASE: i32 = -12;

/// `$FE00-$FEFF`: OAM proper plus the prohibited region above it. The whole
/// page shares OAM's bus, which is why the corruption bug does not stop at
/// `$FE9F`.
#[inline]
fn is_oam_page(address: u16) -> bool {
    address & 0xFF00 == 0xFE00
}

const WRAM_SIZE: usize = 0x8000;
const ZRAM_SIZE: usize = 0x7F;

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
    gbspeed: GbSpeed,
    speed_switch_req: bool,
    undocumented_cgb_regs: [u8; 3], // 0xFF72, 0xFF73, 0xFF75
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
            input_queue: InputQueue::default(),
            gpu: Gpu::new(),
            #[cfg(not(target_arch = "wasm32"))]
            sound: None,
            mbc: mmu_mbc,
            gbmode: GbMode::Classic,
            gbspeed: GbSpeed::Single,
            speed_switch_req: false,
            hdma_src: 0,
            hdma_dst: 0,
            hdma_status: DMAType::NoDma,
            hdma_len: 0xFF,
            hdma_run_block: false,
            undocumented_cgb_regs: [0; 3],
            bus_tick_enabled: false,
            bus_ticked: 0,
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
        let mut res = Self::raw(data, file)?;
        let mode = model.resolve(res.rb(0x0143));
        res.set_mode(mode);
        res.set_initial();
        Ok(res)
    }

    fn set_initial(&mut self) {
        self.wb(0xFF05, 0);
        self.wb(0xFF06, 0);
        self.wb(0xFF07, 0);
        self.wb(0xFF10, 0x80);
        self.wb(0xFF11, 0xBF);
        self.wb(0xFF12, 0xF3);
        self.wb(0xFF14, 0xBF);
        self.wb(0xFF16, 0x3F);
        self.wb(0xFF16, 0x3F);
        self.wb(0xFF17, 0);
        self.wb(0xFF19, 0xBF);
        self.wb(0xFF1A, 0x7F);
        self.wb(0xFF1B, 0xFF);
        self.wb(0xFF1C, 0x9F);
        self.wb(0xFF1E, 0xFF);
        self.wb(0xFF20, 0xFF);
        self.wb(0xFF21, 0);
        self.wb(0xFF22, 0);
        self.wb(0xFF23, 0xBF);
        self.wb(0xFF24, 0x77);
        self.wb(0xFF25, 0xF3);
        self.wb(0xFF26, 0xF1);
        self.wb(0xFF40, 0x91);
        self.wb(0xFF42, 0);
        self.wb(0xFF43, 0);
        self.wb(0xFF45, 0);
        self.wb(0xFF47, 0xFC);
        self.wb(0xFF48, 0xFF);
        self.wb(0xFF49, 0xFF);
        self.wb(0xFF4A, 0);
        self.wb(0xFF4B, 0);
    }

    /// Commit the resolved hardware mode. The PPU keeps its own copy (it reads
    /// it per scanline), so the two must be set together.
    fn set_mode(&mut self, mode: GbMode) {
        self.gbmode = mode;
        self.gpu.gbmode = mode;
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
            // The APU clock advances per-access too, so a wave-RAM or NR5x
            // access lands on its exact sub-instruction cycle (Blargg
            // dmg_sound 09/10/12 measure this to 2 T-cycles). Synthesis stays
            // event-driven; this only moves the "now" that catch_up syncs to.
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(ref mut sound) = self.sound {
                sound.do_cycle(4);
            }
            self.oam_dma_tick();
            self.bus_ticked += 4;
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
            0x0000..=0x7FFF => self.mbc.readrom(address),
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
            self.gpu.oam_bug(kind, dots as i32 + OAM_BUG_PHASE);
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
        self.bus_tick_enabled = r.bool()?;
        self.bus_ticked = r.u32()?;

        self.serial.load_state(r)?;
        self.timer.load_state(r)?;
        self.oam_dma.load_state(r)?;
        self.keypad.load_state(r)?;
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

        self.intf |= self.keypad.interrupt;
        self.keypad.interrupt = 0;

        self.gpu.do_cycle(gputicks);
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
            0x0000..=0x7FFF => self.mbc.readrom(address),
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
            0x0000..=0x7FFF => self.mbc.readrom(address),
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
            0xFF00 => self.keypad.rb(),
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
            0xFF4D | 0xFF4F | 0xFF51..=0xFF55 | 0xFF6C | 0xFF70
                if self.gbmode != GbMode::Color =>
            {
                0xFF
            }
            0xFF72..=0xFF73 | 0xFF75..=0xFF77 if self.gbmode == GbMode::Classic => 0xFF,
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
            "gameboy: FAULT — {reason}. The battery save will NOT be written \
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
        self.bus_tick();
        // A write and an IDU increment in the same M-cycle behave as a single
        // write, so `idu_share` is deliberately not consulted here.
        if is_oam_page(address) {
            self.oam_bug(OamBug::Write, self.bus_ticked);
        }
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
            0xFF00 => self.keypad.wb(value),
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
            0xFF4D | 0xFF4F | 0xFF51..=0xFF55 | 0xFF6C | 0xFF70 | 0xFF76..=0xFF77
                if self.gbmode != GbMode::Color => {}
            0xFF72..=0xFF73 | 0xFF75..=0xFF77 if self.gbmode == GbMode::Classic => {}
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
        if self.sound.is_none() {
            self.sound = Some(crate::sound::Sound::new());
        }
        let sound = self.sound.as_mut().unwrap();
        // DMG carts get DMG APU semantics (length counters survive power-off
        // etc. — what Blargg's dmg_sound asserts); CGB carts get CGB rules.
        sound.set_mode(self.gbmode != GbMode::Color);
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
