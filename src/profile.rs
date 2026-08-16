//! Measurement-only instrumentation — **behind the `profile` cargo feature,
//! off by default**, adding no dependency and compiled out of every shipping
//! build.
//!
//! It exists because a single aggregate "22.4 ms a frame" cannot be optimised,
//! only guessed at. The PSP's clock (`sceKernelGetSystemTimeWide`) costs
//! **0.785 µs a call** and is quantised to 1 µs, so timing individual calls is
//! hopeless there: one `MemoryManagementUnit::bus_tick` is smaller than the
//! measurement of it, and there are ~70,000 of them a frame. Two things work
//! instead, and this module supplies both:
//!
//!  * **Free counters.** How many instructions, bus ticks and scanlines a frame
//!    cost. An increment is one instruction and the numbers make two runs
//!    comparable — an A/B whose instruction counts differ is not an A/B.
//!  * **Switches that remove one component's work**, timed over whole frames
//!    where the clock's resolution is irrelevant (a frame is ~22,000 µs). The
//!    marginal cost of a component is then `t(normal) - t(without it)`.
//!
//! **Say which switch diverges.** [`Switches::no_pixels`] cannot be seen by the
//! cartridge at all — nothing a Game Boy program can read depends on the
//! framebuffer, which is emulator output — so that row is exact.
//! [`Switches::no_bus_tick`] *is* guest-visible (it is the accuracy-for-speed
//! trade `docs/portability.md` §3.7 scopes), and the run it measures is only
//! comparable to the baseline as far as the counters say it is.
//!
//! Single-threaded by construction: the state is a plain `static mut` reached
//! through raw pointers, exactly as a profiler's would be, and every frontend
//! that uses it drives one `Gameboy` on one thread.

/// Work a run may skip. See the module docs for which of these the guest can
/// tell apart.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Switches {
    /// Skip pixel generation (`Gpu::renderscan`). **Guest-invisible**: the
    /// framebuffer is emulator output and no cartridge can read it back, so the
    /// CPU executes exactly the same instructions either way.
    pub no_pixels: bool,
    /// Skip the per-access half of the bus (`MemoryManagementUnit::bus_tick`):
    /// the timer, the serial shift clock, the APU and OAM DMA then advance once
    /// at the end of each instruction instead of before each access.
    ///
    /// **Guest-visible, deliberately.** The *totals* are unchanged — the ticks
    /// this skips are applied at the instruction's end instead, because
    /// `bus_ticked` stays zero — so `DIV` and `TIMA` still run at the right
    /// rate; what is lost is which sub-instruction M-cycle an access lands on,
    /// i.e. Blargg `mem_timing`. This is the price of that accuracy, measured.
    pub no_bus_tick: bool,
    /// Skip the timer entirely (`Timer::do_cycle`, everywhere). **Guest-
    /// visible**: `DIV` and `TIMA` freeze. Diagnostic only — read the
    /// instruction count before believing the row.
    pub no_timer: bool,
    /// Skip `Gpu::refresh_stat_line`, the STAT-line recomputation the PPU does
    /// at the end of every chunk. **Guest-visible**: no STAT interrupts.
    pub no_stat: bool,
    /// Step a halted CPU one M-cycle at a time, as it did before
    /// `MemoryManagementUnit::idle_mcycles` existed. **Guest-invisible** — the
    /// batching is exactly equivalent by construction, so this row is the
    /// honest "before" for it, measured by the same instrument on the same
    /// frames.
    pub no_idle_batch: bool,
    /// Advance the timer one M-cycle at a time, as [`Timer::do_cycle`] did
    /// before `Timer::batch_mcycles` existed. **Guest-invisible** — the closed
    /// form is exactly equivalent, and `timer::tests` proves it against the
    /// loop — so this row is that batch's honest "before".
    ///
    /// [`Timer::do_cycle`]: crate::mmu::timer::Timer::do_cycle
    pub no_timer_batch: bool,
    /// Skip the APU (`Sound::do_cycle`, everywhere). **Guest-visible**: the
    /// frame sequencer, the length counters and `NR52` freeze. Diagnostic only
    /// — it sizes what a machine with a sound chip pays *for having one*, which
    /// `Gameboy::set_render_audio` deliberately does not skip.
    pub no_apu: bool,
    /// Skip the OAM DMA's per-M-cycle tick. **Guest-visible**: a transfer never
    /// advances. Diagnostic only — it sizes the "is a transfer running?" test
    /// that every M-cycle pays whether or not one is.
    pub no_oam_dma: bool,
}

/// What a frame cost in events rather than in time.
#[derive(Clone, Copy, Default)]
pub struct Counters {
    /// CPU instructions executed (a HALT/STOP idle M-cycle counts as one).
    pub instructions: u32,
    /// Calls to `MemoryManagementUnit::bus_tick` that did work.
    pub bus_ticks: u32,
    /// Calls to `Gpu::renderscan` — visible scanlines drawn.
    pub scanlines: u32,
    /// M-cycles burnt by a halted CPU.
    pub halts: u32,
    /// How many `exec` calls those M-cycles arrived in — the batching factor.
    pub halt_batches: u32,
    /// Why a halted M-cycle could not be batched: [dma, serial, pending irq,
    /// no ppu horizon, timer, ok].
    pub idle_reason: [u32; 6],
}

struct State {
    switches: Switches,
    counters: Counters,
}

static mut STATE: State = State {
    switches: Switches {
        no_pixels: false,
        no_bus_tick: false,
        no_timer: false,
        no_stat: false,
        no_idle_batch: false,
        no_timer_batch: false,
        no_apu: false,
        no_oam_dma: false,
    },
    counters: Counters {
        instructions: 0,
        bus_ticks: 0,
        scanlines: 0,
        halts: 0,
        halt_batches: 0,
        idle_reason: [0; 6],
    },
};

#[inline(always)]
fn state() -> &'static mut State {
    // SAFETY: single-threaded by contract (see the module docs). The feature is
    // off in every shipping build.
    unsafe { &mut *(&raw mut STATE) }
}

/// Choose what the next frames skip.
pub fn set_switches(s: Switches) {
    state().switches = s;
}

/// What the next frames skip.
pub fn switches() -> Switches {
    state().switches
}

/// Read the counters and zero them.
pub fn take_counters() -> Counters {
    core::mem::take(&mut state().counters)
}

#[inline(always)]
pub(crate) fn count_instruction() {
    state().counters.instructions += 1;
}

/// Count a bus tick and say whether its work should be skipped.
#[inline(always)]
pub(crate) fn bus_tick_skipped() -> bool {
    let s = state();
    s.counters.bus_ticks += 1;
    s.switches.no_bus_tick
}

#[inline(always)]
pub(crate) fn count_halt(mcycles: u32) {
    state().counters.halts += mcycles;
    state().counters.halt_batches += 1;
}

#[inline(always)]
pub(crate) fn count_idle_reason(i: usize) {
    state().counters.idle_reason[i] += 1;
}

#[inline(always)]
pub(crate) fn idle_batch_skipped() -> bool {
    state().switches.no_idle_batch
}

#[inline(always)]
pub(crate) fn timer_skipped() -> bool {
    state().switches.no_timer
}

#[inline(always)]
pub(crate) fn timer_batch_skipped() -> bool {
    state().switches.no_timer_batch
}

#[inline(always)]
pub(crate) fn apu_skipped() -> bool {
    state().switches.no_apu
}

#[inline(always)]
pub(crate) fn oam_dma_skipped() -> bool {
    state().switches.no_oam_dma
}

#[inline(always)]
pub(crate) fn stat_skipped() -> bool {
    state().switches.no_stat
}

/// Count a scanline and say whether its pixels should be skipped.
#[inline(always)]
pub(crate) fn pixels_skipped() -> bool {
    let s = state();
    s.counters.scanlines += 1;
    s.switches.no_pixels
}
