#[cfg(feature = "embedded")]
use crate::compat::*;

use crate::cpu::registers::Registers;
use crate::cpu::{data, ld, misc, stack};
use crate::mmu::MemoryManagementUnit;
use crate::mode::Model;

#[allow(dead_code)]
pub enum Interrupt {
    Vblank = 0x01,
    LCDStat = 0x02,
    Timer = 0x04,
    Serial = 0x08,
    Joypad = 0x10,
}

pub struct Cpu<'a> {
    pub registers: Registers,
    pub ime: bool,
    /// `EI` delay counter: 2 = armed by this instruction, 1 = IME is set at
    /// the start of the next one, 0 = idle. `DI` has no counter — it is
    /// immediate (see [`Cpu::updateime`]).
    pub setei: u32,
    pub halt: u32,
    /// Set when `HALT` is executed with IME=0 while an interrupt is pending
    /// (`IE & IF & 0x1F != 0`). On hardware this triggers the "HALT bug": the
    /// CPU does not halt, and the program counter fails to increment when the
    /// next opcode is fetched, so the byte after `HALT` is read twice.
    pub halt_bug: bool,
    /// Why the CPU is not executing: [`Cpu::STOP_MODE`] (stopped by `STOP`,
    /// restarted by a selected `P1` line going low) or [`Cpu::STOP_LOCKED`]
    /// (hung on an undefined opcode, never restarted). 0 = running.
    pub stop: u32,
    /// T-cycles left of the HALT a CGB speed switch runs the CPU through,
    /// counted in the *new* clock — see [`Cpu::SPEED_SWITCH_HALT_TICKS`].
    /// 0 when no switch is in flight.
    pub speed_switch_halt: u32,
    pub memory: MemoryManagementUnit<'a>,
    /// Attached plugins — see [`crate::plugin`]. Lives here rather than in the
    /// MMU so dispatch can hand a plugin `&mut MemoryManagementUnit` (two
    /// disjoint fields) without a self-borrow. Empty unless a frontend attaches
    /// something, and an empty host costs one `is_empty` test per transfer.
    pub plugins: crate::plugin::PluginHost,

    // To debug
    _executed_operations: Vec<u8>,
}

impl Cpu<'_> {
    /// Boot a machine for `data`, choosing the console from the cartridge
    /// header (and honouring `$GB_MODEL`) — see [`Model::Auto`].
    ///
    /// Until this gained a model, every cartridge booted a Game Boy Color:
    /// a DMG-only game ran in the Color console's compatibility mode, which is
    /// a *different machine* (boot registers `A=0x11 HL=0x007C` instead of
    /// `A=0x01 HL=0x014D`, and the undocumented CGB registers respond). Test
    /// ROMs that print their boot registers can tell, which is why nine
    /// Mooneye acceptance tests changed verdict when this was added.
    pub fn new(
        data: impl Into<crate::mbc::RomBytes>,
        file: Option<crate::mbc::SavePath>,
    ) -> Self {
        Self::new_with_model(data, file, Model::Auto)
    }

    /// Boot a specific console. `Model::Auto` is [`Cpu::new`].
    ///
    /// Panics on a cartridge this emulator cannot map (unknown MBC type, an
    /// image too short to hold a header, an unreadable `.sav`). Embedders
    /// taking bytes from a user should prefer [`Cpu::try_new_with_model`] —
    /// under `[profile.release] panic = 'abort'` a panic here takes the whole
    /// host process down.
    pub fn new_with_model(
        data: impl Into<crate::mbc::RomBytes>,
        file: Option<crate::mbc::SavePath>,
        model: Model,
    ) -> Self {
        Self::try_new_with_model(data, file, model).unwrap()
    }

    /// [`Cpu::new_with_model`] without the panic: the cartridge is rejected
    /// with the reason instead.
    pub fn try_new_with_model(
        data: impl Into<crate::mbc::RomBytes>,
        file: Option<crate::mbc::SavePath>,
        model: Model,
    ) -> Result<Self, &'static str> {
        Self::try_new_with_boot_rom(data, file, model, None)
    }

    /// [`Cpu::new_with_model`] with an optional **user-supplied boot ROM**
    /// ([`crate::boot`]). Panics on an unmappable cartridge, like
    /// [`Cpu::new_with_model`].
    pub fn new_with_boot_rom(
        data: impl Into<crate::mbc::RomBytes>,
        file: Option<crate::mbc::SavePath>,
        model: Model,
        boot: Option<crate::boot::BootRom>,
    ) -> Self {
        Self::try_new_with_boot_rom(data, file, model, boot).unwrap()
    }

    /// [`Cpu::try_new_with_model`] with an optional user-supplied boot ROM.
    ///
    /// With one mapped the machine starts at **reset** — every register zero,
    /// `PC = $0000` — and the boot ROM produces the handoff state itself. With
    /// none (or one belonging to the other console) this is byte-for-byte the
    /// machine every baseline in this repository is measured on.
    pub fn try_new_with_boot_rom(
        data: impl Into<crate::mbc::RomBytes>,
        file: Option<crate::mbc::SavePath>,
        model: Model,
        boot: Option<crate::boot::BootRom>,
    ) -> Result<Self, &'static str> {
        let memory =
            MemoryManagementUnit::new_with_boot_rom(data.into(), file, model, boot)?;
        let registers = if memory.boot_rom_mapped() {
            Registers::at_reset()
        } else {
            Registers::boot(memory.console, memory.gbmode)
        };

        Ok(Cpu {
            registers,
            memory,
            ime: false,
            setei: 0,
            halt: 0,
            halt_bug: false,
            stop: 0,
            speed_switch_halt: 0,
            plugins: crate::plugin::PluginHost::new(),
            _executed_operations: Vec::new(),
        })
    }
    /// Serialise the **complete** machine to a versioned, ROM-checked byte
    /// blob. See [`crate::savestate`] for the format and the BESS verdict.
    pub fn save_state(&self) -> Vec<u8> {
        use crate::savestate::{Writer, FORMAT_VERSION, MAGIC};
        let mut w = Writer::new();
        w.bytes(&MAGIC);
        w.u16(FORMAT_VERSION);

        // ROM identity — never the ROM itself. Title + global checksum let a
        // load refuse a mismatched cartridge.
        w.string(&self.memory.mbc.romname());
        w.u16(self.memory.mbc.global_checksum());

        // CPU core.
        self.registers.save_state(&mut w);
        w.bool(self.ime);
        w.u32(self.setei);
        w.u32(self.halt);
        // HALT-bug latch: set by a `HALT` that trips the DMG/CGB bug and cleared
        // on the next opcode fetch, so it can be live at a frame boundary —
        // omitting it would silently desync the instruction after HALT.
        w.bool(self.halt_bug);
        // STOP mode and the speed-switch HALT both outlive a frame boundary:
        // a stopped CPU sits there until a button is pressed, and the switch
        // itself is ~0.93 frames long.
        w.u32(self.stop);
        w.u32(self.speed_switch_halt);

        // Everything the MMU owns (RAM, IO, PPU, APU, timer, serial, MBC…).
        self.memory.save_state(&mut w);
        w.buf
    }

    /// Restore a machine from bytes written by [`Cpu::save_state`]. Fails loudly
    /// on a bad magic, a version outside
    /// `MIN_SUPPORTED_VERSION..=FORMAT_VERSION` (see
    /// [`crate::savestate::MIN_SUPPORTED_VERSION`]), or a different
    /// cartridge, rather than loading a subtly corrupt state. The ROM
    /// currently loaded is kept.
    ///
    /// A version older than [`FORMAT_VERSION`] but at or above
    /// [`crate::savestate::MIN_SUPPORTED_VERSION`] is **upgraded**, not
    /// refused: `version` is threaded down to every reader that has grown a
    /// field since that version, and each one defaults what an older writer
    /// did not carry — see [`crate::savestate::MIN_SUPPORTED_VERSION`]'s doc
    /// comment for what that promises.
    pub fn load_state(
        &mut self,
        bytes: &[u8],
    ) -> Result<(), crate::savestate::StateError> {
        use crate::savestate::{
            Reader, StateError, FORMAT_VERSION, MIN_SUPPORTED_VERSION,
        };
        let mut r = Reader::new(bytes);
        r.expect_magic()?;
        let version = r.u16()?;
        if !(MIN_SUPPORTED_VERSION..=FORMAT_VERSION).contains(&version) {
            return Err(StateError::UnsupportedVersion(version));
        }

        let title = r.string()?;
        let checksum = r.u16()?;
        let cur_title = self.memory.mbc.romname();
        let cur_checksum = self.memory.mbc.global_checksum();
        if title != cur_title || checksum != cur_checksum {
            return Err(StateError::RomMismatch {
                expected: cur_title,
                found: title,
            });
        }

        self.registers.load_state(&mut r)?;
        self.ime = r.bool()?;
        self.setei = r.u32()?;
        self.halt = r.u32()?;
        self.halt_bug = r.bool()?;
        self.stop = r.u32()?;
        self.speed_switch_halt = r.u32()?;

        self.memory.load_state(&mut r, version)?;
        Ok(())
    }

    fn mut_find_or_insert<T: PartialEq>(vec: &mut Vec<T>, val: T) -> &mut T {
        if let Some(i) = vec.iter().position(|each| *each == val) {
            &mut vec[i]
        } else {
            vec.push(val);
            vec.last_mut().unwrap()
        }
    }
    pub fn debug(&mut self, op: u8) {
        Cpu::mut_find_or_insert(&mut self._executed_operations, op);
        // println!("{} {:#01x} {}", op, op, format!("{:?}", self.registers));
        // println!("{} {}", op, format!("{:?}", self.registers));
    }
    /// The program counter has reached the unusable block or I/O, which is
    /// the runaway-`PC` fault the window above describes.
    ///
    /// Out of line, and behind the range test rather than beside it: this runs
    /// once per instruction, `Cpu::exec` is inlined into `Gameboy::frame`, and
    /// keeping the `fault()` load and the message out of that function is
    /// worth 1.3 % of a headless accurate frame. The test itself, and what it
    /// does when it fires, are unchanged.
    #[cold]
    #[inline(never)]
    fn report_runaway_pc(&mut self) {
        if self.memory.fault().is_none() {
            self.memory.report_fault(&format!(
                "the CPU is executing from {:#06x}, which is not memory a program \
                 can live in (unusable block / I-O) — the game has crashed",
                self.registers.pc
            ));
        }
    }

    pub fn get_byte(&mut self) -> u8 {
        let pc = self.memory.fetch(self.registers.pc);
        self.registers.pc = self.registers.pc.wrapping_add(1);
        pc
    }
    pub fn get_word(&mut self) -> u16 {
        let w = self.memory.rw(self.registers.pc);
        self.registers.pc += 2;
        w
    }
    /// Advance the one-instruction `EI` delay. `DI` has no delay on DMG/CGB
    /// (Mooneye `acceptance/di_timing-GS`: "DI is expected to disable
    /// interrupts immediately"), so it is applied by the opcode itself.
    fn updateime(&mut self) {
        // Test before storing. This runs once per instruction and the counter
        // is zero in all but the two after an `EI`, so the unconditional
        // write-back was a store to the CPU's hottest cache line for nothing.
        if self.setei == 0 {
            return;
        }
        self.setei = match self.setei {
            2 => 1,
            1 => {
                self.ime = true;
                0
            }
            _ => 0,
        };
    }

    /// [`Cpu::stop`]: stopped by `STOP`, waiting on the joypad.
    pub const STOP_MODE: u32 = 1;
    /// [`Cpu::stop`]: locked up by an undefined opcode, as hardware does.
    pub const STOP_LOCKED: u32 = 2;

    /// How long the CPU sits in HALT while a CGB speed switch completes,
    /// counted in T-cycles of the **new** clock. Pan Docs' STOP chart: *"HALT
    /// mode will exit automatically after about 0x20000 T-cycles"*. Switching
    /// into double speed that is 65536 cycles of the unchanged 4.194304 MHz
    /// clock the PPU runs on — 143 scanlines and a bit, which is why the
    /// display visibly collapses across a speed switch.
    ///
    /// `0x20000` is also exactly **two wraps of the 16-bit system counter**,
    /// which is reset on the way in and then runs at the new speed. That is
    /// what makes `DIV` read back ~0 the instant the CPU resumes, and it is
    /// what `daid/speed_switch_timing_div` measures: on this machine that ROM
    /// passes for a duration anywhere in `0x1FFF4..=0x20000`, and the
    /// documented value is the top of that window. (SameBoy uses a measured
    /// `0x20008`; the eight cycles between the two are below the resolution of
    /// this core's per-access cycle accounting.)
    const SPEED_SWITCH_HALT_TICKS: u32 = 0x0002_0000;

    /// `STOP` (`$10`). The whole of Pan Docs' *"Using the STOP Instruction"*
    /// decision chart (by Lior Halphon), which the three branch flags below
    /// index into:
    ///
    /// * a **selected button already held** means STOP does not stop at all —
    ///   `DIV` is not reset and no speed switch happens;
    /// * a **speed switch requested** (KEY1 bit 0) resets `DIV`, toggles the
    ///   clock and parks the CPU in HALT for [`SPEED_SWITCH_HALT_TICKS`];
    /// * otherwise the CPU enters **STOP mode** proper and only a `P1` line
    ///   going low restarts it;
    /// * and an **interrupt pending** (`IE & IF != 0`) in any of those makes
    ///   STOP a one-byte opcode, so the `$00` after it is executed normally.
    ///
    /// The one square of the chart that reads "the CPU glitches
    /// non-deterministically" — a button held *and* a switch requested — is
    /// resolved here the way SameBoy resolves it: no switch, no stop, no `DIV`
    /// reset. An emulator has to do something, and hanging or panicking on a
    /// cartridge-chosen combination is not it.
    ///
    /// [`SPEED_SWITCH_HALT_TICKS`]: Cpu::SPEED_SWITCH_HALT_TICKS
    fn stop_instruction(&mut self) -> u32 {
        let exit_by_joyp = self.memory.keypad.rb() & 0x0F != 0x0F;
        let speed_switch = self.memory.speed_switch_requested() && !exit_by_joyp;
        let interrupt_pending = self.memory.inte & self.memory.intf & 0x1F != 0;

        if !exit_by_joyp {
            self.enter_stop_mode();
        }

        // "STOP is normally a 2-byte instruction where the second byte is
        // ignored" — but with an interrupt pending it becomes a 1-byte opcode
        // and the next byte is executed as an instruction of its own.
        let cycles = if interrupt_pending {
            1
        } else {
            self.get_byte();
            2
        };

        if speed_switch {
            self.memory.switch_speed();
            if !interrupt_pending {
                self.speed_switch_halt = Self::SPEED_SWITCH_HALT_TICKS;
            }
        }

        if speed_switch || exit_by_joyp {
            self.leave_stop_mode();
            if !interrupt_pending {
                self.halt = 1;
            } else {
                self.speed_switch_halt = 0;
            }
        }

        cycles
    }

    /// Stop the CPU: reset the system counter and cut the PPU off from VRAM.
    ///
    /// Pan Docs: *"On CGB, leaving the LCD enabled when invoking STOP will
    /// result in a black screen. Except if the LCD is in Mode 3, where it will
    /// keep drawing the current screen."* Mode 3 is exactly the mode in which
    /// the PPU already owns VRAM, so the rule is one rule: the PPU keeps
    /// whatever access it holds at the instant the CPU stops. On a DMG the PPU
    /// stops outright and the panel goes blank.
    fn enter_stop_mode(&mut self) {
        self.stop = Self::STOP_MODE;
        self.memory.reset_div();
        let fill = if self.memory.gbmode.is_dmg() {
            Some(255)
        } else if self.memory.gpu.mode() == 3 {
            None
        } else {
            Some(0)
        };
        self.memory.gpu.set_stop_fill(fill);
    }

    fn leave_stop_mode(&mut self) {
        self.stop = 0;
        self.memory.gpu.set_stop_fill(None);
    }

    /// **How long an interrupt takes to reach the CPU depends on where inside
    /// the M-cycle it was requested, and on whether the CPU is running or
    /// halted.** This is the one case in this machine where the two differ.
    ///
    /// Every interrupt source here flips on an M-cycle boundary — VBlank, the
    /// timer, the serial port, the mode-0 and mode-1 STAT sources — except the
    /// **mode-2 OAM strobe**, which is asserted part-way through the last
    /// M-cycle of the previous scanline
    /// ([`Gpu::oam_strobe`](crate::gpu::Gpu), `OAM_STROBE_LEAD`). A CPU
    /// executing instructions is watching the line as it goes and dispatches at
    /// the instruction boundary it has already reached; a CPU sitting in `HALT`
    /// is one M-cycle slower, so the whole handler — and every mid-scanline
    /// register write in it — lands one M-cycle later.
    ///
    /// **This is what the two PPU suites were disagreeing about.** Mealybug's
    /// ROMs take that interrupt out of a `nop` slide and photograph where a
    /// mid-line write lands, so they measure the *running* latency; Mooneye's
    /// five `acceptance/ppu/intr_2_*` take it out of `HALT` and count cycles to
    /// a mode change, so they measure the *halted* one. Modelling them as the
    /// same latency makes the two irreconcilable and forces a compensating fudge
    /// into the strobe; modelling the difference satisfies both untouched.
    ///
    /// Two independent references describe the same mechanism:
    ///
    /// * **docboy** (MIT; `src/docboy/docboy/cpu/cpu.cpp`,
    ///   `INTERRUPTS_TIMINGS`) tabulates the latency per source, per halt
    ///   state, and per T-cycle of the M-cycle. Its DMG row for STAT is
    ///   `{1, 1, 1, 2}` running against `{1, 2, 2, 2}` halted: identical when
    ///   the request lands on the M-cycle's first T-cycle, one M-cycle longer
    ///   when it lands part-way through.
    /// * **SameBoy** (Expat; `Core/sm83_cpu.c`, `GB_cpu_run`) reaches it from
    ///   the other side — its halt path advances the clock *before* sampling
    ///   `IE & IF` and again after, so a halted CPU samples the line at a
    ///   different point in the cycle from a running one, while the run path
    ///   samples at the instruction boundary with nothing advanced.
    ///
    /// Only the mode-2 strobe can answer `true` here, so no other interrupt's
    /// timing moves — which is exactly why `acceptance/di_timing-GS`,
    /// `halt_ime1_timing2-GS`, `halt_ime0_nointr_timing`,
    /// `ppu/intr_1_2_timing-GS` and `ppu/hblank_ly_scx_timing-GS` (all of which
    /// wake from `HALT` on a boundary-aligned source) are untouched.
    fn mid_mcycle_request_reaches_halt_late(&mut self) -> bool {
        // Nothing pending: no request to be late about, and the marker must not
        // be consumed by an unrelated M-cycle of a long HALT.
        if self.memory.inte & self.memory.intf & 0x1F == 0 {
            return false;
        }
        // The delay belongs to the request itself, so it is only spent when the
        // request that would be served *is* the mid-cycle one. Priority is the
        // lowest set bit, so STAT must be the winner (VBlank outranks it).
        if self.memory.inte & self.memory.intf & 0x03 != 0x02 {
            return false;
        }
        self.memory.gpu.take_stat_irq_mid_mcycle()
    }

    // Interrupt dispatch. See docs/components/cpu.md §"Interrupts" and Pan Docs:
    // https://gbdev.io/pandocs/Interrupts.html
    fn handleinterrupt(&mut self) -> u32 {
        // HALT wakes on any pending+enabled interrupt even with IME off; a
        // still-masked (IME) CPU wakes but does not dispatch (Pan Docs "HALT").
        if !self.ime && self.halt == 0 {
            return 0;
        }

        if self.memory.inte & self.memory.intf & 0x1F == 0 {
            return 0;
        }

        self.halt = 0;
        if !self.ime {
            return 0;
        }
        self.ime = false;
        // A dispatch steps the PPU with the bus and pushes `PC` through `wb`,
        // so anything a horizon batch is still holding has to land first —
        // see `MemoryManagementUnit::flush_pending`.
        self.memory.flush_pending();

        // Dispatch is **5 M-cycles**, and where the two PC pushes fall inside
        // them is observable: Mooneye `acceptance/intr_timing` states "serving
        // an interrupt is supposed to take 5 M-cycles" and measures it against
        // DIV. Hardware spends M1/M2 idle, pushes PC-high on M3 and PC-low on
        // M4, and fetches from the vector on M5 (Gekkio, *Complete Technical
        // Reference* §"Interrupts"). Ticking the bus across all five keeps the
        // timer/APU phase right through dispatch instead of applying 20 T in
        // one lump afterwards.
        let prev = self.memory.set_bus_ticking(true);
        // The PPU runs during the dispatch too, and it has to be *seen* to:
        // the vector is chosen at M4 and a PPU interrupt raised before then can
        // redirect it. See `MemoryManagementUnit::set_ppu_bus_ticking`.
        let prev_ppu = self.memory.set_ppu_bus_ticking(true);
        self.memory.internal_tick();
        self.memory.internal_tick();

        let pc = self.registers.pc;
        // M3: push PC high. With SP = $0000 this lands *on IE* — see below.
        self.registers.sp = self.registers.sp.wrapping_sub(1);
        self.memory.wb(self.registers.sp, (pc >> 8) as u8);

        // The vector is decided here, from `IE & IF` as they stand at the start
        // of M4: a write to IE by the high-byte push can still cancel the
        // dispatch (PC becomes $0000 and *no* IF bit is acknowledged) or
        // redirect it to a different vector, while the low-byte push's own
        // write to IE is too late to matter
        // (Mooneye `acceptance/interrupts/ie_push`).
        let triggered = self.memory.inte & self.memory.intf & 0x1F;

        // M4: push PC low.
        self.registers.sp = self.registers.sp.wrapping_sub(1);
        self.memory.wb(self.registers.sp, pc as u8);
        self.memory.internal_tick();
        self.memory.set_bus_ticking(prev);
        self.memory.set_ppu_bus_ticking(prev_ppu);

        // Priority = lowest set bit wins, so the fixed order is
        // VBlank(0) > STAT(1) > Timer(2) > Serial(3) > Joypad(4). Only that one
        // bit is acknowledged/cleared; the others stay pending in IF.
        // Vector = 0x40, 0x48, 0x50, 0x58, 0x60 (0x0040 | n<<3).
        self.registers.pc = if triggered == 0 {
            0x0000
        } else {
            let n = triggered.trailing_zeros();
            self.memory.intf &= !(1 << n);
            0x0040 | ((n as u16) << 3)
        };

        5
    }

    /// The CPU has executed an opcode that does not exist.
    ///
    /// On hardware these eleven bytes (`$D3 $DB $DD $E3 $E4 $EB $EC $ED $F4
    /// $FC $FD`) **lock the CPU up**: it stops fetching and only a reset
    /// recovers it (Pan Docs,
    /// <https://gbdev.io/pandocs/CPU_Instruction_Set.html>). No real game
    /// executes one on purpose, so reaching here is a definitive statement that
    /// the program counter has walked off the program — which is why this is
    /// also a [fault](crate::mmu::MemoryManagementUnit::report_fault): from this
    /// point on, whatever the machine stores into cartridge RAM is not a save.
    ///
    /// This used to `panic!`, i.e. abort the process under
    /// `[profile.release] panic = 'abort'`. That was worse in both directions:
    /// it is not what the hardware does, and it kills the emulator at the exact
    /// moment the user most wants to be told what happened — taking any
    /// unwritten battery RAM with it.
    fn illegal_opcode(&mut self, op: u8) -> u32 {
        // Leave PC on the offending byte so a debugger shows it.
        self.registers.pc = self.registers.pc.wrapping_sub(1);
        self.memory.report_fault(&format!(
            "the CPU executed undefined opcode {op:#04x} at {:#06x} — on hardware \
             this locks the CPU up, so the game has crashed",
            self.registers.pc
        ));
        // Locked up: stop fetching, but keep the peripherals running so the
        // frontend stays alive to show the message. `STOP_LOCKED`, not
        // `STOP_MODE` — a button press restarts a stopped CPU and must not
        // restart a hung one.
        self.stop = Self::STOP_LOCKED;
        1
    }

    pub fn exec(&mut self) -> u32 {
        #[cfg(feature = "profile")]
        crate::profile::count_instruction();
        // Not executing: either stopped by `STOP` — which is *not* HALT, so an
        // interrupt does not restart it, only one of the P10-P13 lines going
        // low does (Pan Docs, "Using the STOP Instruction") — or locked up by
        // an undefined opcode, which nothing wakes at all. Tested before
        // interrupt dispatch for that reason. The rest of the machine keeps
        // running either way, so frames still complete and the frontend stays
        // responsive.
        if self.stop != 0 {
            if self.stop == Self::STOP_LOCKED || self.memory.keypad.rb() & 0x0F == 0x0F {
                return 1;
            }
            self.leave_stop_mode();
        }

        self.updateime();
        if self.halt == 1 && self.mid_mcycle_request_reaches_halt_late() {
            // Still halted for this M-cycle; the request is acted on next one.
            return 1;
        }
        match self.handleinterrupt() {
            0 => {}
            n => return n,
        };

        if self.halt == 1 {
            // A halted CPU does nothing until an interrupt arrives, so run as
            // many idle M-cycles at once as the machine can be advanced without
            // one becoming possible — often a whole mode-3 span. Exactly
            // equivalent, and the difference between 42 fps and full speed on a
            // handheld: see `MemoryManagementUnit::idle_mcycles`.
            // The halt horizon is measured from where the peripherals *are*,
            // so land the batch before asking for it.
            self.memory.flush_pending();
            let mut idle = self.memory.idle_mcycles();
            // The HALT a CGB speed switch runs through ends by itself after
            // `SPEED_SWITCH_HALT_TICKS`, with no interrupt involved — so the
            // countdown is a fourth horizon, and the batch has to stop on the
            // M-cycle that empties it or the switch would end late.
            if self.speed_switch_halt > 0 {
                idle = idle.min(self.speed_switch_halt.div_ceil(4)).max(1);
                self.speed_switch_halt = self.speed_switch_halt.saturating_sub(4 * idle);
                if self.speed_switch_halt == 0 {
                    self.halt = 0;
                }
            }
            #[cfg(feature = "profile")]
            crate::profile::count_halt(idle);
            idle
        } else {
            // An interrupt woke the CPU before the switch countdown expired
            // ("Unless an interrupt occurs before then…" on the STOP chart).
            self.speed_switch_halt = 0;
            // Step the timer per memory access for the duration of this
            // instruction so reads/writes land on the correct M-cycle
            // (Blargg mem_timing). See MemoryManagementUnit::bus_tick.
            self.memory.set_bus_ticking(true);
            let cycles = self.operation();
            self.memory.set_bus_ticking(false);
            cycles
        }
    }

    pub fn do_cycle(&mut self) -> u32 {
        let ticks = self.exec() * 4;
        // Answer any link-cable transfer the instruction just started, *before*
        // `MMU::do_cycle` folds `serial.interrupt` into `IF` — so a plugin
        // partner produces exactly the interrupt timing the host-callback path
        // already did. `take_pending` always clears, so an unanswered transfer
        // behaves like an unplugged cable rather than being answered late.
        if let Some(sent) = self.memory.serial.take_pending() {
            if let Some(reply) = self.plugins.on_serial_transfer(sent, &mut self.memory) {
                self.memory.serial.complete_transfer(reply);
            }
        }
        self.memory.do_cycle(ticks)

        // match self.delay {
        //     0 => {}
        //     1 => {
        //         self.delay = 0;
        //         self.ime = 1;
        //     }
        //     2 => {
        //         self.delay = 1;
        //     }
        //     _ => {}
        // }

        // let mut ticks = if self.halt == 0 && self.stop == 0 {
        //     self.operation()
        // } else {
        //     self.memory.switch_speed();
        //     self.stop = 0;
        //     1
        // };

        // // See http://bgb.bircd.org/pandocs.htm#interrupts
        // if self.ime != 0 || self.halt != 0 {
        //     let ints = self.memory.intf & self.memory.inte;

        //     if ints != 0 {
        //         let i = ints.trailing_zeros();
        //         if self.ime != 0 {
        //             self.memory.intf &= !(1 << (i as u32));
        //         }
        //         self.ime = 0;
        //         self.halt = 0;
        //         self.stop = 0;
        //         match i {
        //             0 => {
        //                 stack::rst(self, 0x40);
        //             }
        //             1 => {
        //                 stack::rst(self, 0x48);
        //             }
        //             2 => {
        //                 stack::rst(self, 0x50);
        //             }
        //             3 => {
        //                 stack::rst(self, 0x58);
        //             }
        //             4 => {
        //                 stack::rst(self, 0x60);
        //             }
        //             _ => {}
        //         }
        //         ticks += 1;
        //     }
        // }

        // self.ticks += ticks;
        // return ticks;
    }

    fn operation(&mut self) -> u32 {
        // Cheap, general "the program counter has left the program" test: one
        // subtract and one compare per instruction, on a branch that is never
        // taken by a working game. $FEA0-$FF7F is the unusable block and the
        // I/O registers — two regions that cannot hold code, so fetching an
        // opcode from either means the CPU is executing something that is not a
        // program. Unlike an undefined opcode this is a *heuristic* about where
        // PC is rather than a hardware lock-up, so it reports the fault (which
        // only withdraws the right to write the save file) and lets the machine
        // run on.
        //
        // **OAM ($FE00-$FE9F) is deliberately NOT in that window.** It is real
        // RAM the CPU may read whenever the PPU is not in mode 2 or 3 (Pan Docs,
        // "VRAM/OAM accessibility"), so a program *can* legally execute from it
        // — and three Mooneye ROMs that PASS do exactly that to time an access
        // onto a known byte (`acceptance/add_sp_e_timing`, `ld_hl_sp_e_timing`,
        // `oam_dma_start`). Including OAM made every one of them print a crash
        // report and, far worse for a player, silently stop writing the battery
        // save for the rest of the session. See docs/conformance-notes.md.
        if self.registers.pc.wrapping_sub(0xFEA0) < 0x00E0 {
            self.report_runaway_pc();
        }
        let op = self.get_byte();
        if self.halt_bug {
            // HALT bug: the opcode was fetched but PC must not have advanced,
            // so rewind it — the same byte is fetched again next time (and any
            // operands of this instruction are read one byte early).
            self.halt_bug = false;
            self.registers.pc = self.registers.pc.wrapping_sub(1);
        }
        // self.debug(op);
        // println!("{:?}", self.registers);
        match op {
            0x00 => 1,
            0x01 => {
                ld::bcnn(self);
                3
            }
            0x02 => {
                ld::bcm_a(self);
                2
            }
            0x03 => {
                data::incbc(self);
                2
            }
            0x04 => {
                data::incr_b(self);
                1
            }
            0x05 => {
                data::decr_b(self);
                1
            }
            0x06 => {
                ld::rr_b(self);
                2
            }
            0x07 => {
                misc::rlca(self);
                1
            }
            0x08 => {
                ld::mmsp(self);
                5
            }
            0x09 => {
                data::addhlbc(self);
                2
            }
            0x0A => {
                ld::abcm(self);
                2
            }
            0x0B => {
                data::decbc(self);
                2
            }
            0x0C => {
                data::incr_c(self);
                1
            }
            0x0D => {
                data::decr_c(self);
                1
            }
            0x0E => {
                ld::rr_c(self);
                2
            }
            0x0F => {
                misc::rrca(self);
                1
            }
            0x10 => self.stop_instruction(),
            0x11 => {
                ld::denn(self);
                3
            }
            0x12 => {
                ld::dem_a(self);
                2
            }
            0x13 => {
                data::incde(self);
                2
            }
            0x14 => {
                data::incr_d(self);
                1
            }
            0x15 => {
                data::decr_d(self);
                1
            }
            0x16 => {
                ld::rr_d(self);
                2
            }
            0x17 => {
                misc::rla(self);
                1
            }
            0x18 => {
                stack::jrn(self);
                3
            }
            0x19 => {
                data::addhlde(self);
                2
            }
            0x1A => {
                ld::adem(self);
                2
            }
            0x1B => {
                data::decde(self);
                2
            }
            0x1C => {
                data::incr_e(self);
                1
            }
            0x1D => {
                data::decr_e(self);
                1
            }
            0x1E => {
                ld::rr_e(self);
                2
            }
            0x1F => {
                misc::rra(self);
                1
            }
            0x20 => stack::jrnzn(self),
            0x21 => {
                ld::hlnn(self);
                3
            }
            0x22 => {
                ld::hlia(self);
                2
            }
            0x23 => {
                data::inchl(self);
                2
            }
            0x24 => {
                data::incr_h(self);
                1
            }
            0x25 => {
                data::decr_h(self);
                1
            }
            0x26 => {
                self.registers.h = self.get_byte();
                2
            }
            0x27 => {
                ld::rr_h(self);
                1
            }
            0x28 => stack::jrzn(self),
            0x29 => {
                data::addhlhl(self);
                2
            }
            0x2A => {
                ld::ahli(self);
                2
            }
            0x2B => {
                data::dechl(self);
                2
            }
            0x2C => {
                data::incr_l(self);
                1
            }
            0x2D => {
                data::decr_l(self);
                1
            }
            0x2E => {
                ld::rr_l(self);
                2
            }
            0x2F => {
                data::cpl(self);
                1
            }
            0x30 => stack::jrncn(self),
            0x31 => {
                ld::spnn(self);
                3
            }
            0x32 => {
                ld::hld_a(self);
                2
            }
            0x33 => {
                data::incsp(self);
                2
            }
            0x34 => {
                data::inchlm(self);
                3
            }
            0x35 => {
                data::dechlm(self);
                3
            }
            0x36 => {
                ld::hlmn(self);
                3
            }
            0x37 => {
                data::scf(self);
                1
            }
            0x38 => stack::jrcn(self),
            0x39 => {
                data::addhlsp(self);
                2
            }
            0x3A => {
                ld::ahld(self);
                2
            }
            0x3B => {
                data::decsp(self);
                2
            }
            0x3C => {
                data::incr_a(self);
                1
            }
            0x3D => {
                data::decr_a(self);
                1
            }
            0x3E => {
                ld::rr_a(self);
                2
            }
            0x3F => {
                data::ccf(self);
                1
            }
            0x40 => {
                // ld::rr_bb(self);
                1
            }
            0x41 => {
                ld::rr_bc(self);
                1
            }
            0x42 => {
                ld::rr_bd(self);
                1
            }
            0x43 => {
                ld::rr_be(self);
                1
            }
            0x44 => {
                ld::rr_bh(self);
                1
            }
            0x45 => {
                ld::rr_bl(self);
                1
            }
            0x46 => {
                ld::r_hlm_b(self);
                2
            }
            0x47 => {
                ld::rr_ba(self);
                1
            }
            0x48 => {
                ld::rr_cb(self);
                1
            }
            0x49 => {
                // ld::rr_cc(self);
                1
            }
            0x4A => {
                ld::rr_cd(self);
                1
            }
            0x4B => {
                ld::rr_ce(self);
                1
            }
            0x4C => {
                ld::rr_ch(self);
                1
            }
            0x4D => {
                ld::rr_cl(self);
                1
            }
            0x4E => {
                ld::r_hlm_c(self);
                2
            }
            0x4F => {
                ld::rr_ca(self);
                1
            }
            0x50 => {
                ld::rr_db(self);
                1
            }
            0x51 => {
                ld::rr_dc(self);
                1
            }
            0x52 => 1,
            0x53 => {
                ld::rr_de(self);
                1
            }
            0x54 => {
                ld::rr_dh(self);
                1
            }
            0x55 => {
                ld::rr_dl(self);
                1
            }
            0x56 => {
                ld::r_hlm_d(self);
                2
            }
            0x57 => {
                ld::rr_da(self);
                1
            }
            0x58 => {
                ld::rr_eb(self);
                1
            }
            0x59 => {
                ld::rr_ec(self);
                1
            }
            0x5A => {
                ld::rr_ed(self);
                1
            }
            0x5B => 1,
            0x5C => {
                ld::rr_eh(self);
                1
            }
            0x5D => {
                ld::rr_el(self);
                1
            }
            0x5E => {
                ld::r_hlm_e(self);
                2
            }
            0x5F => {
                ld::rr_ea(self);
                1
            }
            0x60 => {
                ld::rr_hb(self);
                1
            }
            0x61 => {
                ld::rr_hc(self);
                1
            }
            0x62 => {
                ld::rr_hd(self);
                1
            }
            0x63 => {
                ld::rr_he(self);
                1
            }
            0x64 => 1,
            0x65 => {
                ld::rr_hl(self);
                1
            }
            0x66 => {
                ld::r_hlm_h(self);
                2
            }
            0x67 => {
                ld::rr_ha(self);
                1
            }
            0x68 => {
                ld::rr_lb(self);
                1
            }
            0x69 => {
                ld::rr_lc(self);
                1
            }
            0x6A => {
                ld::rr_ld(self);
                1
            }
            0x6B => {
                ld::rr_le(self);
                1
            }
            0x6C => {
                ld::rr_lh(self);
                1
            }
            0x6D => 1,
            0x6E => {
                ld::r_hlm_l(self);
                2
            }
            0x6F => {
                ld::rr_la(self);
                1
            }
            0x70 => {
                ld::hlmr_b(self);
                2
            }
            0x71 => {
                ld::hlmr_c(self);
                2
            }
            0x72 => {
                ld::hlmr_d(self);
                2
            }
            0x73 => {
                ld::hlmr_e(self);
                2
            }
            0x74 => {
                ld::hlmr_h(self);
                2
            }
            0x75 => {
                ld::hlmr_l(self);
                2
            }
            0x76 => {
                // HALT with interrupts disabled but one already pending does
                // not halt on hardware; instead it trips the HALT bug (PC not
                // incremented on the next fetch — the following byte is read
                // twice). Otherwise, halt normally.
                if !self.ime && (self.memory.inte & self.memory.intf & 0x1F) != 0 {
                    self.halt_bug = true;
                } else {
                    self.halt = 1;
                }
                1
            }
            0x77 => {
                ld::hlmr_a(self);
                2
            }
            0x78 => {
                ld::rr_ab(self);
                1
            }
            0x79 => {
                ld::rr_ac(self);
                1
            }
            0x7A => {
                ld::rr_ad(self);
                1
            }
            0x7B => {
                ld::rr_ae(self);
                1
            }
            0x7C => {
                ld::rr_ah(self);
                1
            }
            0x7D => {
                ld::rr_al(self);
                1
            }
            0x7E => {
                ld::r_hlm_a(self);
                2
            }
            0x7F => {
                // ld::rr_aa(self);
                1
            }
            0x80 => {
                data::addr_b(self);
                1
            }
            0x81 => {
                data::addr_c(self);
                1
            }
            0x82 => {
                data::addr_d(self);
                1
            }
            0x83 => {
                data::addr_e(self);
                1
            }
            0x84 => {
                data::addr_h(self);
                1
            }
            0x85 => {
                data::addr_l(self);
                1
            }
            0x86 => {
                data::addhl(self);
                2
            }
            0x87 => {
                data::addr_a(self);
                1
            }
            0x88 => {
                data::adcr_b(self);
                1
            }
            0x89 => {
                data::adcr_c(self);
                1
            }
            0x8A => {
                data::adcr_d(self);
                1
            }
            0x8B => {
                data::adcr_e(self);
                1
            }
            0x8C => {
                data::adcr_h(self);
                1
            }
            0x8D => {
                data::adcr_l(self);
                1
            }
            0x8E => {
                data::adchl(self);
                2
            }
            0x8F => {
                data::adcr_a(self);
                1
            }
            0x90 => {
                data::subr_b(self);
                1
            }
            0x91 => {
                data::subr_c(self);
                1
            }
            0x92 => {
                data::subr_d(self);
                1
            }
            0x93 => {
                data::subr_e(self);
                1
            }
            0x94 => {
                data::subr_h(self);
                1
            }
            0x95 => {
                data::subr_l(self);
                1
            }
            0x96 => {
                data::subhl(self);
                2
            }
            0x97 => {
                data::subr_a(self);
                1
            }
            0x98 => {
                data::sbcr_b(self);
                1
            }
            0x99 => {
                data::sbcr_c(self);
                1
            }
            0x9A => {
                data::sbcr_d(self);
                1
            }
            0x9B => {
                data::sbcr_e(self);
                1
            }
            0x9C => {
                data::sbcr_h(self);
                1
            }
            0x9D => {
                data::sbcr_l(self);
                1
            }
            0x9E => {
                data::sbchl(self);
                2
            }
            0x9F => {
                data::sbcr_a(self);
                1
            }
            0xA0 => {
                data::andr_b(self);
                1
            }
            0xA1 => {
                data::andr_c(self);
                1
            }
            0xA2 => {
                data::andr_d(self);
                1
            }
            0xA3 => {
                data::andr_e(self);
                1
            }
            0xA4 => {
                data::andr_h(self);
                1
            }
            0xA5 => {
                data::andr_l(self);
                1
            }
            0xA6 => {
                data::andhl(self);
                2
            }
            0xA7 => {
                data::andr_a(self);
                1
            }
            0xA8 => {
                data::xorr_b(self);
                1
            }
            0xA9 => {
                data::xorr_c(self);
                1
            }
            0xAA => {
                data::xorr_d(self);
                1
            }
            0xAB => {
                data::xorr_e(self);
                1
            }
            0xAC => {
                data::xorr_h(self);
                1
            }
            0xAD => {
                data::xorr_l(self);
                1
            }
            0xAE => {
                data::xorhl(self);
                2
            }
            0xAF => {
                data::xorr_a(self);
                1
            }
            0xB0 => {
                data::orr_b(self);
                1
            }
            0xB1 => {
                data::orr_c(self);
                1
            }
            0xB2 => {
                data::orr_d(self);
                1
            }
            0xB3 => {
                data::orr_e(self);
                1
            }
            0xB4 => {
                data::orr_h(self);
                1
            }
            0xB5 => {
                data::orr_l(self);
                1
            }
            0xB6 => {
                data::orhl(self);
                2
            }
            0xB7 => {
                data::orr_a(self);
                1
            }
            0xB8 => {
                data::cpr_b(self);
                1
            }
            0xB9 => {
                data::cpr_c(self);
                1
            }
            0xBA => {
                data::cpr_d(self);
                1
            }
            0xBB => {
                data::cpr_e(self);
                1
            }
            0xBC => {
                data::cpr_h(self);
                1
            }
            0xBD => {
                data::cpr_l(self);
                1
            }
            0xBE => {
                data::cphl(self);
                2
            }
            0xBF => {
                data::cpr_a(self);
                1
            }
            0xC0 => stack::retnz(self),
            0xC1 => {
                stack::popbc(self);
                3
            }
            0xC2 => stack::jpnznn(self),
            0xC3 => {
                stack::jpnn(self);
                4
            }
            0xC4 => stack::callnznn(self),
            0xC5 => {
                stack::pushbc(self);
                4
            }
            0xC6 => {
                data::addn(self);
                2
            }
            0xC7 => {
                stack::rst(self, 0x00);
                4
            }
            0xC8 => stack::retz(self),
            0xC9 => {
                stack::ret(self);
                4
            }
            0xCA => stack::jpznn(self),
            0xCB => misc::cbmap(self),
            0xCC => stack::callznn(self),
            0xCD => {
                stack::callnn(self);
                6
            }
            0xCE => {
                data::adcn(self);
                2
            }
            0xCF => {
                stack::rst(self, 0x08);
                4
            }
            0xD0 => stack::retnc(self),
            0xD1 => {
                stack::popde(self);
                3
            }
            0xD2 => stack::jpncnn(self),
            0xD4 => stack::callncnn(self),
            0xD5 => {
                stack::pushde(self);
                4
            }
            0xD6 => {
                data::subn(self);
                2
            }
            0xD7 => {
                stack::rst(self, 0x10);
                4
            }
            0xD8 => stack::retc(self),
            0xD9 => {
                // RETI re-enables interrupts with no delay, unlike EI.
                stack::reti(self);
                self.ime = true;
                self.setei = 0;
                4
            }
            0xDA => stack::jpcnn(self),
            0xDC => stack::callcnn(self),
            0xDE => {
                data::sbcn(self);
                2
            }
            0xDF => {
                stack::rst(self, 0x18);
                4
            }
            0xE0 => {
                ld::ion_a(self);
                3
            }
            0xE1 => {
                stack::pophl(self);
                3
            }
            0xE2 => {
                ld::ioca(self);
                2
            }
            0xE5 => {
                stack::pushhl(self);
                4
            }
            0xE6 => {
                data::andn(self);
                2
            }
            0xE7 => {
                stack::rst(self, 0x20);
                4
            }
            0xE8 => {
                data::addspn(self);
                4
            }
            0xE9 => {
                stack::jphl(self);
                1
            }
            0xEA => {
                ld::mm_a(self);
                4
            }
            0xEE => {
                data::orn(self);
                2
            }
            0xEF => {
                stack::rst(self, 0x28);
                4
            }
            0xF0 => {
                ld::aion(self);
                3
            }
            0xF1 => {
                stack::popaf(self);
                3
            }
            0xF2 => {
                ld::aioc(self);
                2
            }
            0xF3 => {
                // DI is immediate on DMG/MGB/SGB, and it cancels a pending EI
                // (Mooneye `acceptance/di_timing-GS`, `acceptance/rapid_di_ei`).
                self.ime = false;
                self.setei = 0;
                1
            }
            0xF5 => {
                stack::pushaf(self);
                4
            }
            0xF6 => {
                let v = self.get_byte();
                data::alu_or(self, v);
                2
            }
            0xF7 => {
                stack::rst(self, 0x30);
                4
            }
            0xF8 => {
                let r = data::alu_add16imm(self);
                data::set_hl(self, r);
                3
            }
            0xF9 => {
                ld::hlspn(self);
                2
            }
            0xFA => {
                ld::amm(self);
                4
            }
            0xFB => {
                // IME is set after the instruction *following* EI. A second EI
                // while the delay is already running does not restart it —
                // otherwise a run of EIs never enables interrupts at all
                // (Mooneye `acceptance/ei_sequence`).
                if self.setei == 0 {
                    self.setei = 2;
                }
                1
            }
            0xFE => {
                data::cpn(self);
                2
            }
            0xFF => {
                stack::rst(self, 0x38);
                4
            }
            // $D3 $DB $DD $E3 $E4 $EB $EC $ED $F4 $FC $FD are not instructions.
            // Hardware does not "not implement" them — the CPU locks up and
            // only a reset recovers (Pan Docs, "Unused opcodes"). A runaway
            // `PC` in any game reaches this.
            _ => self.illegal_opcode(op),
        }
    }
}

#[cfg(test)]
mod lockup_tests {
    use super::*;

    /// A 32 KiB cartridge whose entry point is `op`.
    fn boot(op: u8) -> Cpu<'static> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0100] = op;
        Cpu::new(rom, None)
    }

    /// The eleven bytes that are not instructions. Hardware locks the CPU up
    /// until reset; aborting the process instead takes any unwritten battery
    /// RAM with it under `panic = 'abort'` — a runaway `PC` in any game gets
    /// here. See the `_ =>` arm of `Cpu::operation`.
    #[test]
    fn an_undefined_opcode_locks_the_cpu_up_instead_of_killing_the_process() {
        for op in [
            0xD3u8, 0xDB, 0xDD, 0xE3, 0xE4, 0xEB, 0xEC, 0xED, 0xF4, 0xFC, 0xFD,
        ] {
            let mut cpu = boot(op);
            for _ in 0..1000 {
                cpu.do_cycle();
            }
            assert_eq!(
                cpu.registers.pc, 0x0100,
                "{op:#04X} must sit on the offending byte, not run on"
            );
            assert_ne!(cpu.stop, 0, "{op:#04X} must be latched as locked up");
        }
    }

    /// The rest of the machine keeps running through a lock-up — the PPU still
    /// produces frames, so the frontend stays responsive rather than freezing.
    #[test]
    fn the_ppu_keeps_running_while_the_cpu_is_locked_up() {
        let mut cpu = boot(0xD3);
        let mut cycles = 0u32;
        while cycles < 70224 * 2 {
            cycles += cpu.do_cycle();
        }
        assert!(cpu.memory.gpu.updated, "frames still complete");
    }

    /// An interrupt does not wake a locked-up CPU (unlike `HALT`).
    #[test]
    fn no_interrupt_wakes_a_locked_up_cpu() {
        let mut cpu = boot(0xD3);
        cpu.do_cycle();
        cpu.ime = true;
        cpu.memory.inte = 0x1F;
        cpu.memory.intf = 0x1F;
        for _ in 0..100 {
            cpu.do_cycle();
        }
        assert_eq!(cpu.registers.pc, 0x0100, "no vector was dispatched");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 32 KiB ROM-only cartridge whose entry point runs `code`.
    fn rom_running(code: &[u8]) -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0100] = 0x00; // NOP
        rom[0x0101] = 0xC3; // JP $0150
        rom[0x0102] = 0x50;
        rom[0x0103] = 0x01;
        rom[0x0150..0x0150 + code.len()].copy_from_slice(code);
        rom
    }

    /// Cycles from an identical starting point to the STAT vector, for a CPU
    /// that meets the **mode-2 OAM strobe** while waiting in `HALT` and for one
    /// meeting it in the middle of a `nop` slide.
    ///
    /// Both machines run the same prologue, so they enter the wait at the same
    /// PPU dot; the only difference is what they are doing when the strobe
    /// arrives. Returns `(halted, running)` in dots.
    fn dots_to_stat_vector(mode: crate::gpu::RenderMode) -> (u32, u32) {
        // DI; STAT = mode-2 source; IE = STAT; wait for LY == $42; clear IF
        // (so the interrupt measured is the *next* strobe, not one that piled
        // up while IME was off); EI. Laid out from $0150:
        //   $015F  XOR A          <- the measurement starts here
        //   $0163  the tail under test
        let prologue: &[u8] = &[
            0xF3, // DI
            0x3E, 0x20, 0xE0, 0x41, // LD A,$20 ; LDH ($41),A   (STAT)
            0x3E, 0x02, 0xE0, 0xFF, // LD A,$02 ; LDH ($FF),A   (IE)
            0xF0, 0x44, 0xFE, 0x42, 0x20, 0xFA, // -: LDH A,($44); CP $42; JR NZ,-
            0xAF, 0xE0, 0x0F, //       XOR A    ; LDH ($0F),A   (IF)
            0xFB, // EI
        ];
        let measure = |tail: &[u8]| -> u32 {
            let mut code = prologue.to_vec();
            code.extend_from_slice(tail);
            let mut cpu = Cpu::new(rom_running(&code), None);
            cpu.memory.set_render_mode(mode);
            let mut dots = 0;
            let mut waiting = true;
            for _ in 0..200_000 {
                let d = cpu.do_cycle();
                // Start counting once the `LY` wait has been left, so the
                // prologue's own length cannot enter the comparison.
                if waiting {
                    waiting = cpu.registers.pc < 0x015F;
                } else {
                    dots += d;
                }
                if cpu.registers.pc == 0x0048 {
                    return dots;
                }
            }
            panic!("the STAT vector was never reached");
        };
        // 0x76 = HALT. The slide is nops, long enough to span a whole line.
        (measure(&[0x76]), measure(&[0x00; 200]))
    }

    /// **The mode-2 OAM strobe reaches a halted CPU one M-cycle later than a
    /// running one, and no other interrupt in the machine does.**
    ///
    /// It is the only source here that is asserted part-way through an M-cycle
    /// rather than on its boundary (see `OAM_STROBE_LEAD`), and a CPU sitting
    /// in `HALT` samples the interrupt line at a different point in the cycle
    /// from one that is executing. docboy tabulates it directly
    /// (`INTERRUPTS_TIMINGS`, DMG STAT row `{1,1,1,2}` running vs `{1,2,2,2}`
    /// halted); SameBoy's `GB_cpu_run` advances the clock around its halt-path
    /// `IE & IF` sample and not around the run-path one.
    ///
    /// Modelling the two as equal is what forced `Gpu::oam_strobe`'s lead to be
    /// a knob: Mooneye's five `acceptance/ppu/intr_2_*` take this interrupt out
    /// of `HALT` and Mealybug's ROMs take it out of a `nop` slide, so no single
    /// strobe position satisfies both.
    #[test]
    fn a_halted_cpu_meets_the_oam_strobe_one_m_cycle_later_than_a_running_one() {
        let (halted, running) = dots_to_stat_vector(crate::gpu::RenderMode::Accurate);
        assert_eq!(
            halted,
            running + 4,
            "halted {halted} vs running {running} dots to the STAT vector"
        );
    }

    /// And the whole-scanline renderer is untouched by it: it has no
    /// sub-instruction phase for a mid-M-cycle assertion to live in, so both
    /// paths must still cost exactly the same.
    #[test]
    fn the_standard_renderer_does_not_pay_the_mid_m_cycle_delay() {
        let (halted, running) = dots_to_stat_vector(crate::gpu::RenderMode::Fast);
        assert_eq!(halted, running, "the default engine must not move");
    }

    fn run(code: &[u8], steps: usize) -> Cpu<'static> {
        let mut cpu = Cpu::new(rom_running(code), None);
        for _ in 0..steps {
            cpu.do_cycle();
        }
        cpu
    }

    /// This used to `panic!`, i.e. abort the process under `panic = 'abort'`.
    /// Hardware locks the CPU up instead — and either way the program has left
    /// its own code, which must stop the emulator persisting cartridge RAM.
    #[test]
    fn an_undefined_opcode_faults_and_locks_up_instead_of_panicking() {
        // LD A,$11 ; $DD (undefined) ; LD A,$22
        let cpu = run(&[0x3E, 0x11, 0xDD, 0x3E, 0x22], 8);
        assert!(
            cpu.memory.fault().is_some(),
            "an undefined opcode is a definitive 'the game has crashed'"
        );
        assert_ne!(cpu.stop, 0, "the CPU must be locked up, as hardware is");
        assert_eq!(
            cpu.registers.a, 0x11,
            "nothing after the undefined opcode may execute"
        );
    }

    /// $FC is exactly as undefined as the other ten. It used to be silently
    /// treated as a NOP, which hid a crash instead of reporting it.
    #[test]
    fn opcode_fc_is_undefined_too() {
        assert!(run(&[0xFC], 4).memory.fault().is_some());
    }

    /// The general detector: the unusable block and the I/O registers cannot
    /// hold a program, so fetching an opcode there means the program counter has
    /// walked off the program.
    #[test]
    fn executing_from_memory_that_cannot_hold_code_is_a_fault() {
        for target in [0xFEA0u16, 0xFF10, 0xFF7F] {
            // JP <target>
            let cpu = run(&[0xC3, target as u8, (target >> 8) as u8], 6);
            assert!(
                cpu.memory.fault().is_some(),
                "executing from {target:#06x} must be reported"
            );
        }
    }

    /// ...but OAM is not one of them. It is real RAM the CPU may read outside
    /// PPU modes 2 and 3, and three Mooneye acceptance ROMs that PASS execute
    /// from it on purpose. Treating that as a crash poisoned the battery, so a
    /// game legitimately running a routine out of OAM silently stopped saving.
    #[test]
    fn running_from_oam_is_not_a_fault() {
        // JP $FE00, with a `JR -2` spin planted there first.
        //
        // **The LCD goes off first, and that is the machine, not the test.**
        // Under the default `identical` engine the PPU owns OAM during modes 2
        // and 3, so a fetch from `$FE00` there reads `$FF` — `RST $38` — and the
        // CPU leaves OAM before it can prove anything about the fault window.
        // Mooneye's three ROMs that genuinely execute from OAM
        // (`add_sp_e_timing`, `ld_hl_sp_e_timing`, `oam_dma_start`) all do it
        // where OAM is reachable, and so does this.
        let mut cpu = Cpu::new(rom_running(&[0xC3, 0x00, 0xFE]), None);
        cpu.memory.wb(0xFF40, 0x00);
        cpu.memory.wb(0xFE00, 0x18);
        cpu.memory.wb(0xFE01, 0xFE);
        for _ in 0..16 {
            cpu.do_cycle();
        }
        assert_eq!(cpu.registers.pc, 0xFE00);
        assert_eq!(cpu.memory.fault(), None);
    }

    /// ...and the branch must be genuinely never-taken for a working game: HRAM
    /// ($FF80-$FFFE) is where real games *do* put code (the OAM DMA routine).
    #[test]
    fn running_from_hram_is_not_a_fault() {
        // JP $FF80, with a `JR -2` spin planted there first.
        let mut cpu = Cpu::new(rom_running(&[0xC3, 0x80, 0xFF]), None);
        cpu.memory.wb(0xFF80, 0x18);
        cpu.memory.wb(0xFF81, 0xFE);
        for _ in 0..16 {
            cpu.do_cycle();
        }
        assert_eq!(cpu.registers.pc, 0xFF80);
        assert_eq!(cpu.memory.fault(), None);
    }

    /// A fault is a fact about this session, not about the machine, and it is
    /// the first diagnosis that matters.
    #[test]
    fn the_first_fault_reason_is_the_one_kept() {
        let mut cpu = Cpu::new(rom_running(&[0x00]), None);
        cpu.memory.report_fault("first");
        cpu.memory.report_fault("second");
        assert_eq!(cpu.memory.fault(), Some("first"));
    }
}

/// `STOP` — the instruction, its four branches, and the display of a stopped
/// machine. Device-free: each test hand-assembles a 32 KiB cartridge and drives
/// the core directly, so they run under plain `cargo test`.
///
/// The behaviours pinned here are the ones `daid/stop_instr`,
/// `daid/stop_instr_gbc_mode3` and `daid/speed_switch_timing_div` measure with
/// a whole ROM; see [`Cpu::stop_instruction`] for the Pan Docs citations.
#[cfg(test)]
mod stop_tests {
    use super::Cpu;
    use crate::gpu::{HEIGHT, WIDTH};
    use crate::input::KeypadKey;
    use crate::mode::Model;

    /// A minimal cartridge whose program starts at `$0150`. `cgb` sets the
    /// CGB flag, which is what makes `KEY1` respond at all — a Color console
    /// running a cartridge without it is in compatibility mode, where the CGB
    /// registers read `$FF` and a speed switch cannot be asked for.
    fn cart(program: &[u8], cgb: bool) -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        // $0100: NOP; JP $0150 — the entry point every header has.
        rom[0x0100] = 0x00;
        rom[0x0101] = 0xC3;
        rom[0x0102] = 0x50;
        rom[0x0103] = 0x01;
        for (i, b) in b"STOPTEST".iter().enumerate() {
            rom[0x0134 + i] = *b;
        }
        rom[0x0143] = if cgb { 0xC0 } else { 0x00 };
        rom[0x0147] = 0x00; // ROM ONLY
        rom[0x0148] = 0x00; // 32 KiB
        rom[0x0149] = 0x00; // no RAM
        let mut sum: u8 = 0;
        for b in rom.iter().take(0x014D).skip(0x0134) {
            sum = sum.wrapping_sub(*b).wrapping_sub(1);
        }
        rom[0x014D] = sum;
        rom[0x0150..0x0150 + program.len()].copy_from_slice(program);
        rom
    }

    fn machine(program: &[u8], model: Model) -> Cpu<'static> {
        Cpu::new_with_model(cart(program, false), None, model)
    }

    /// A Color console running a Color cartridge — the only machine on which
    /// `STOP` can switch speed.
    fn color_machine(program: &[u8]) -> Cpu<'static> {
        Cpu::new_with_model(cart(program, true), None, Model::Cgb)
    }

    /// Run until `pc` is reached (or `budget` instructions are spent), and
    /// report the T-cycles the PPU saw on the way — which is the 4.194304 MHz
    /// clock, whatever speed the CPU is running at.
    fn run_to(cpu: &mut Cpu<'_>, pc: u16, budget: u32) -> u32 {
        let mut ppu_ticks = 0;
        for _ in 0..budget {
            if cpu.registers.pc == pc {
                return ppu_ticks;
            }
            ppu_ticks += cpu.do_cycle();
        }
        panic!("never reached ${pc:04X}");
    }

    /// Spin until `DIV` reads `$40`, then arm KEY1 and switch. Waiting on a
    /// known, *non-zero* `DIV` is `daid/speed_switch_timing_div`'s own trick,
    /// and it is what makes the counter reset observable: without it the
    /// switch is a whole number of counter wraps and DIV comes out reading
    /// whatever it read going in.
    ///
    /// `LDH A,($04) / CP $40 / JR NZ,-6 / LD A,$01 / LDH ($4D),A / STOP`
    const SWITCH: &[u8] = &[
        0xF0, 0x04, 0xFE, 0x40, 0x20, 0xFA, 0x3E, 0x01, 0xE0, 0x4D, 0x10, 0x00, 0x00,
    ];
    const STOP_AT: u16 = 0x0150 + 10;

    #[test]
    fn a_speed_switch_runs_the_cpu_through_a_halt_two_counter_wraps_long() {
        let mut cpu = color_machine(SWITCH);
        // Sanity: single speed, and the request is visible in KEY1.
        assert_eq!(cpu.memory.rb(0xFF4D) & 0x80, 0x00);
        run_to(&mut cpu, STOP_AT, 100_000);
        assert_eq!(cpu.memory.rb(0xFF4D) & 0x01, 0x01, "KEY1 request not armed");
        assert_eq!(
            cpu.memory.timer.rb(0xFF04),
            0x40,
            "test setup: DIV should be $40"
        );

        // The STOP itself, then however long the CPU sits in the HALT it puts
        // us in. PC does not move while halted, so the exit is what is timed.
        cpu.do_cycle();
        assert_eq!(cpu.halt, 1, "the switch parks the CPU in HALT mode");
        let mut ticks = 0;
        for _ in 0..200_000 {
            if cpu.halt == 0 {
                break;
            }
            ticks += cpu.do_cycle();
        }

        // The PPU (and so the LCD) keeps running for the whole switch. The
        // switch is SPEED_SWITCH_HALT_TICKS of the *new* clock, so half that
        // many of the PPU's, which never changes speed.
        let switch_ppu_ticks = Cpu::SPEED_SWITCH_HALT_TICKS / 2;
        assert!(
            ticks >= switch_ppu_ticks && ticks < switch_ppu_ticks + 8,
            "the switch took {ticks} PPU ticks, expected ~{switch_ppu_ticks}"
        );
        // 65536 dots is 143.7 scanlines — the "display collapses for a short
        // moment" Pan Docs warns about.
        assert_eq!(switch_ppu_ticks / 456, 143);

        // Double speed now, request withdrawn.
        assert_eq!(cpu.memory.rb(0xFF4D) & 0x81, 0x80);
        // The counter was reset on the way in and then ran at the new speed for
        // exactly two wraps, so DIV comes back out at ~0 — this is the whole of
        // `daid/speed_switch_timing_div`.
        assert!(
            cpu.memory.timer.rb(0xFF04) == 0x00,
            "DIV came out of the switch at {:02X}, not ~0",
            cpu.memory.timer.rb(0xFF04)
        );
        // And the CPU really is executing again.
        assert_eq!(cpu.stop, 0);
        assert_eq!(cpu.halt, 0);
    }

    #[test]
    fn a_speed_switch_is_a_no_op_without_the_key1_request() {
        // STOP with no request and no button: STOP mode, not a speed switch.
        let mut cpu = color_machine(&[0x10, 0x00, 0x00]);
        run_to(&mut cpu, 0x0150, 100);
        cpu.do_cycle();
        assert_eq!(cpu.stop, Cpu::STOP_MODE);
        assert_eq!(cpu.memory.rb(0xFF4D) & 0x80, 0x00, "speed must not change");
    }

    #[test]
    fn stop_mode_ends_only_when_a_selected_button_goes_low() {
        // LDH ($00),A with A=0 selects both button rows, then STOP.
        let program = &[0xAF, 0xE0, 0x00, 0x10, 0x00, 0x3C];
        let mut cpu = machine(program, Model::Cgb);
        run_to(&mut cpu, 0x0155, 100);
        let pc = cpu.registers.pc;

        // Thousands of cycles later the CPU has not moved: an interrupt cannot
        // end STOP mode, only the joypad can.
        cpu.memory.intf = 0x1F;
        cpu.memory.inte = 0x1F;
        cpu.ime = true;
        for _ in 0..10_000 {
            cpu.do_cycle();
        }
        assert_eq!(cpu.stop, Cpu::STOP_MODE);
        assert_eq!(cpu.registers.pc, pc, "a stopped CPU must not execute");

        cpu.memory.keypad.keydown(KeypadKey::Start);
        cpu.do_cycle();
        assert_eq!(cpu.stop, 0);
    }

    #[test]
    fn stop_does_not_stop_when_a_button_is_already_held() {
        let program = &[0xAF, 0xE0, 0x00, 0x10, 0x00, 0x3C];
        let mut cpu = machine(program, Model::Cgb);
        cpu.memory.keypad.keydown(KeypadKey::A);
        run_to(&mut cpu, 0x0153, 100);
        // Give DIV a value we would notice being cleared.
        for _ in 0..200 {
            cpu.do_cycle();
        }
        let div = cpu.memory.timer.rb(0xFF04);
        assert_ne!(div, 0, "test setup: DIV should be non-zero here");

        cpu.do_cycle(); // the STOP itself
        assert_eq!(cpu.stop, 0, "a held button means STOP does not stop");
        // "DIV is not reset" — the one branch of the chart that leaves it alone.
        assert!(cpu.memory.timer.rb(0xFF04) >= div);
        assert_eq!(cpu.halt, 1, "HALT mode is entered instead");
    }

    #[test]
    fn a_pending_interrupt_makes_stop_a_one_byte_opcode() {
        // STOP, then $3C (INC A) as the "second byte". With an interrupt
        // pending STOP is one byte, so that $3C is a real instruction — and
        // STOP mode is still entered, so it never runs.
        let program = &[0x10, 0x3C];
        let mut cpu = machine(program, Model::Cgb);
        run_to(&mut cpu, 0x0150, 100);
        cpu.memory.inte = 0x04;
        cpu.memory.intf = 0x04;
        cpu.ime = false;
        cpu.do_cycle();
        assert_eq!(cpu.stop, Cpu::STOP_MODE);
        assert_eq!(cpu.registers.pc, 0x0151, "one byte, not two");

        // Without a pending interrupt the same STOP swallows its operand.
        let mut cpu = machine(program, Model::Cgb);
        run_to(&mut cpu, 0x0150, 100);
        cpu.memory.inte = 0;
        cpu.memory.intf = 0;
        cpu.do_cycle();
        assert_eq!(cpu.registers.pc, 0x0152, "two bytes");
    }

    /// The shade every pixel of the frame holds, or `None` if they differ.
    fn flat_shade(cpu: &Cpu<'_>) -> Option<u8> {
        let first = cpu.memory.gpu.data[0];
        cpu.memory.gpu.data[..WIDTH * HEIGHT * 4]
            .chunks(4)
            .all(|px| px[..3].iter().all(|p| *p == first))
            .then_some(first)
    }

    /// Turn the LCD on, put something on screen, then STOP — and report what
    /// the panel shows a frame later.
    fn stopped_screen(model: Model, wait_for_mode_3: bool) -> Option<u8> {
        // LD A,$91 / LDH ($40),A   — LCD + BG on
        // LD A,$1E / LDH ($47),A   — a BGP that is not all-one-shade
        // (optionally) spin until STAT reports mode 3
        // STOP
        let mut program = vec![0x3E, 0x91, 0xE0, 0x40, 0x3E, 0x1E, 0xE0, 0x47];
        if wait_for_mode_3 {
            // .wait: LDH A,($41) / AND $03 / CP $03 / JR NZ,.wait
            program.extend_from_slice(&[0xF0, 0x41, 0xE6, 0x03, 0xFE, 0x03, 0x20, 0xF8]);
        }
        program.extend_from_slice(&[0x10, 0x00, 0x00]);
        let mut cpu = machine(&program, model);
        // Give the PPU some non-blank tile data to draw, so a frozen screen is
        // distinguishable from a blanked one.
        for a in 0x8000u16..0x8010 {
            cpu.memory.gpu.vram[(a & 0x1FFF) as usize] = 0xA5;
        }
        for _ in 0..400_000 {
            cpu.do_cycle();
            if cpu.stop == Cpu::STOP_MODE {
                break;
            }
        }
        assert_eq!(cpu.stop, Cpu::STOP_MODE, "the CPU never stopped");
        // Two full frames (70224 dots each) of the stopped machine, so every
        // scanline has been drawn again since the CPU stopped.
        for _ in 0..40_000 {
            cpu.do_cycle();
        }
        flat_shade(&cpu)
    }

    #[test]
    fn a_stopped_dmg_blanks_the_panel_and_a_stopped_color_goes_black() {
        // Pan Docs: a DMG stops the PPU outright — the panel goes blank.
        assert_eq!(stopped_screen(Model::Dmg, false), Some(255));
        // "On CGB, leaving the LCD enabled when invoking STOP will result in a
        // black screen."
        assert_eq!(stopped_screen(Model::Cgb, false), Some(0));
    }

    #[test]
    fn a_color_stopped_during_mode_3_keeps_drawing_the_screen() {
        // "…Except if the LCD is in Mode 3, where it will keep drawing the
        // current screen." So the frame is *not* a flat shade: the PPU kept
        // the VRAM access it already held and drew the tile data.
        assert_eq!(
            stopped_screen(Model::Cgb, true),
            None,
            "a mode-3 STOP must leave the picture alone"
        );
    }
}

/// Proves the whole-`Cpu` upgrade path — `version` reaching
/// [`crate::gpu::Gpu::load_state`] through [`Cpu::load_state`] and
/// [`crate::mmu::MemoryManagementUnit::load_state`] — against a state whose
/// GPU block is genuinely old-shaped, spliced into an otherwise completely
/// real machine snapshot (registers, MMU, serial, timer, OAM DMA, keypad,
/// SGB/APU/MBC blocks all real, untouched bytes). This is the automated
/// counterpart to loading an actual save file written by the last commit at
/// `FORMAT_VERSION` 22 (`dd584e1`) — see `AGENTS.md`'s save-state section for
/// that manual demonstration and the exact command.
#[cfg(test)]
mod savestate_compat {
    use super::Cpu;
    use crate::savestate::{Writer, MIN_SUPPORTED_VERSION};

    /// Splice a genuinely `version`-shaped GPU block into `cpu`'s own real
    /// `save_state()` bytes. Everything outside the GPU block — registers,
    /// every MMU field, the serial/timer/OAM-DMA/keypad blocks, the
    /// SGB/APU/MBC blobs — is `cpu.save_state()`'s own real, unmodified
    /// output; only the GPU segment is downgraded, via
    /// [`crate::gpu::Gpu::save_state_as_of`]. Locating that segment needs no
    /// knowledge of any other field's width: the real (current-format) GPU
    /// bytes are themselves computed and searched for as a contiguous
    /// subsequence of the full blob — reliable because that segment carries
    /// the whole VRAM and framebuffer, which makes it enormous and unique.
    fn downgrade(cpu: &Cpu, version: u16) -> Vec<u8> {
        let full = cpu.save_state();

        let mut gpu_current = Writer::new();
        cpu.memory.gpu.save_state(&mut gpu_current);
        let needle = gpu_current.buf.as_slice();
        let pos = full
            .windows(needle.len())
            .position(|w| w == needle)
            .expect("the GPU block must appear verbatim inside the full state");

        let mut gpu_old = Writer::new();
        cpu.memory.gpu.save_state_as_of(&mut gpu_old, version);

        let mut out = Vec::with_capacity(full.len() - needle.len() + gpu_old.buf.len());
        out.extend_from_slice(&full[..pos]);
        out.extend_from_slice(&gpu_old.buf);
        out.extend_from_slice(&full[pos + needle.len()..]);
        // Stamp the older version over the real one — everything up to this
        // point copied the real header bytes, version word included.
        out[4..6].copy_from_slice(&version.to_le_bytes());
        out
    }

    /// A genuinely v22-shaped state — the oldest this build still
    /// accepts — loads into the current build and the machine keeps running.
    /// `MIN_SUPPORTED_VERSION` itself is the version under test, so a future
    /// change that raises the floor exercises exactly the new floor here too.
    #[test]
    fn a_v22_shaped_state_loads_and_the_machine_keeps_running() {
        let rom = vec![0u8; 0x8000]; // ROM-only cartridge, entry point all zero (NOP)
        let mut source = Cpu::new(rom.clone(), None);
        for _ in 0..200 {
            source.do_cycle();
        }
        let old = downgrade(&source, MIN_SUPPORTED_VERSION);

        let mut loaded = Cpu::new(rom, None);
        loaded.load_state(&old).unwrap_or_else(|e| {
            panic!("v{MIN_SUPPORTED_VERSION} state should load: {e}")
        });

        // The machine is not merely "not rejected" — it runs on.
        for _ in 0..1000 {
            loaded.do_cycle();
        }
    }

    /// A version below [`MIN_SUPPORTED_VERSION`] is refused exactly as before
    /// this task — the reject-on-mismatch rule stays absolute outside the
    /// named, reviewed window.
    #[test]
    fn a_version_below_the_supported_floor_is_still_refused() {
        let rom = vec![0u8; 0x8000];
        let mut cpu = Cpu::new(rom, None);
        for _ in 0..10 {
            cpu.do_cycle();
        }
        let too_old = downgrade(&cpu, MIN_SUPPORTED_VERSION - 1);
        match cpu.load_state(&too_old) {
            Err(crate::savestate::StateError::UnsupportedVersion(v)) => {
                assert_eq!(v, MIN_SUPPORTED_VERSION - 1)
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }
}
