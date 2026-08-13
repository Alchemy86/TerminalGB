mod serial;
mod timer;

use crate::gpu::Gpu;
use crate::input::Keypad;
use crate::mbc;
use crate::mmu::serial::Serial;
use crate::mmu::timer::Timer;
use crate::mode::{GbMode, GbSpeed};
#[cfg(not(target_arch = "wasm32"))]
use crate::sound::Sound;
use std::path;

pub type StrResult<T> = Result<T, &'static str>;

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
    hdma: [u8; 4],
    pub inte: u8,
    pub intf: u8,
    pub serial: Serial<'a>,
    pub timer: Timer,
    pub keypad: Keypad,
    pub gpu: Gpu,
    #[cfg(not(target_arch = "wasm32"))]
    pub sound: Option<Sound>,
    hdma_status: DMAType,
    hdma_src: u16,
    hdma_dst: u16,
    hdma_len: u8,
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
        let serial = Serial::default();
        let mut res = MemoryManagementUnit {
            wram: [0; WRAM_SIZE],
            zram: [0; ZRAM_SIZE],
            hdma: [0; 4],
            wrambank: 1,
            inte: 0,
            intf: 0,
            serial,
            timer: Timer::default(),
            keypad: Keypad::default(),
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
            undocumented_cgb_regs: [0; 3],
            bus_tick_enabled: false,
            bus_ticked: 0,
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
            self.bus_ticked += 4;
        }
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
        w.bytes(&self.hdma);
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
        w.bytes(&self.undocumented_cgb_regs);
        // Sub-instruction timer stepping (mem_timing fix). These are reset to
        // false/0 between instructions, so at a normal frame-boundary save they
        // are default — but capturing them keeps the state complete regardless
        // of when it is taken.
        w.bool(self.bus_tick_enabled);
        w.u32(self.bus_ticked);

        self.serial.save_state(w);
        self.timer.save_state(w);
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
        r.fill(&mut self.hdma)?;
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
        r.fill(&mut self.undocumented_cgb_regs)?;
        self.bus_tick_enabled = r.bool()?;
        self.bus_ticked = r.u32()?;

        self.serial.load_state(r)?;
        self.timer.load_state(r)?;
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
            0xFE00..=0xFE9F => self.gpu.rb(address),
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
            0xFF40..=0xFF4F => self.gpu.rb(address),
            0xFF51..=0xFF55 => self.hdma_read(address),
            0xFF68..=0xFF6B => self.gpu.rb(address),
            0xFF70 => self.wrambank as u8,
            0xFF72..=0xFF73 => self.undocumented_cgb_regs[address as usize - 0xFF72],
            0xFF75 => self.undocumented_cgb_regs[2] | 0b10001111,
            0xFF76..=0xFF77 => 0x00, // CGB PCM registers. Not yet implemented.
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

    pub fn wb(&mut self, address: u16, value: u8) {
        self.bus_tick();
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
            0xFE00..=0xFE9F => self.gpu.wb(address, value),
            0xFF00 => self.keypad.wb(value),
            0xFF01..=0xFF02 => self.serial.wb(address, value),
            0xFF04..=0xFF07 => self.timer.wb(address, value),
            0xFF10..=0xFF3F => {
                // Catch-up before the write so the frame-sequencer position
                // the write interacts with is current (see Sound::catch_up).
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(ref mut sound) = self.sound {
                    sound.catch_up();
                    sound.wb(address, value);
                }
            }
            0xFF46 => self.oamdma(value),
            0xFF4D | 0xFF4F | 0xFF51..=0xFF55 | 0xFF6C | 0xFF70 | 0xFF76..=0xFF77
                if self.gbmode != GbMode::Color => {}
            0xFF72..=0xFF73 | 0xFF75..=0xFF77 if self.gbmode == GbMode::Classic => {}
            0xFF4D => {
                if value & 0x1 == 0x1 {
                    self.speed_switch_req = true;
                }
            }
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

    pub fn switch_speed(&mut self) {
        if self.speed_switch_req {
            if self.gbspeed == GbSpeed::Double {
                self.gbspeed = GbSpeed::Single;
            } else {
                self.gbspeed = GbSpeed::Double;
            }
        }
        self.speed_switch_req = false;
    }

    // OAM DMA (FF46): copy 0xA0 bytes from XX00-XX9F into OAM. Deliberately
    // instantaneous and unguarded here; hardware takes 160 M-cycles and restricts
    // the CPU to HRAM meanwhile — see mmu.md and https://gbdev.io/pandocs/OAM_DMA_Transfer.html
    fn oamdma(&mut self, value: u8) {
        // OAM DMA re-uses rb/wb internally; it is a single (instantaneous, in
        // this model) transfer, not 160 CPU memory accesses, so suppress the
        // per-access timer stepping while it runs.
        let prev = self.set_bus_ticking(false);
        let base = (value as u16) << 8;
        for i in 0..0xA0 {
            let b = self.rb(base + i);
            self.wb(0xFE00 + i, b);
        }
        self.set_bus_ticking(prev);
    }

    fn hdma_read(&self, a: u16) -> u8 {
        match a {
            0xFF51..=0xFF54 => self.hdma[(a - 0xFF51) as usize],
            0xFF55 => {
                self.hdma_len
                    | if self.hdma_status == DMAType::NoDma {
                        0x80
                    } else {
                        0
                    }
            }
            _ => panic!("The address {:04X} should not be handled by hdma_read", a),
        }
    }

    fn hdma_write(&mut self, a: u16, v: u8) {
        match a {
            0xFF51 => self.hdma[0] = v,
            0xFF52 => self.hdma[1] = v & 0xF0,
            0xFF53 => self.hdma[2] = v & 0x1F,
            0xFF54 => self.hdma[3] = v & 0xF0,
            0xFF55 => {
                if self.hdma_status == DMAType::Hdma {
                    if v & 0x80 == 0 {
                        self.hdma_status = DMAType::NoDma;
                    };
                    return;
                }
                let src = ((self.hdma[0] as u16) << 8) | (self.hdma[1] as u16);
                let dst = ((self.hdma[2] as u16) << 8) | (self.hdma[3] as u16) | 0x8000;
                if !(src <= 0x7FF0 || (0xA000..=0xDFF0).contains(&src)) {
                    panic!("HDMA transfer with illegal start address {:04X}", src);
                }

                self.hdma_src = src;
                self.hdma_dst = dst;
                self.hdma_len = v & 0x7F;

                self.hdma_status = if v & 0x80 == 0x80 {
                    DMAType::Hdma
                } else {
                    DMAType::Gdma
                };
            }
            _ => panic!("The address {:04X} should not be handled by hdma_write", a),
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
        if !self.gpu.may_hdma() {
            return 0;
        }

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
        let mmu_src = self.hdma_src;
        for j in 0..0x10 {
            let b: u8 = self.rb(mmu_src + j);
            self.gpu.wb(self.hdma_dst + j, b);
        }
        self.hdma_src += 0x10;
        self.hdma_dst += 0x10;

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
