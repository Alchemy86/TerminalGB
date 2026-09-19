#[cfg(feature = "embedded")]
use crate::compat::*;

use blip_buf::BlipBuf;

/// The four square duty patterns, in hardware's own bit order (Pan Docs,
/// "Audio Registers" → NRx1). Position 0 is the one the duty counter holds
/// immediately after an APU power-on, which is what makes the phase of these
/// rows observable: SameSuite reads the channel's live digital output through
/// PCM12/PCM34 and compares it position by position, so a rotated table (which
/// sounds identical — a square wave's phase is inaudible) fails every
/// `channel_N_duty` / `channel_N_align` subtest.
const DUTY_PATTERN: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1], // 12.5%
    [1, 0, 0, 0, 0, 0, 0, 1], // 25%
    [1, 0, 0, 0, 0, 1, 1, 1], // 50%
    [0, 1, 1, 1, 1, 1, 1, 0], // 75%
];
// Game Boy CPU runs at 4.194304 MHz
const CLOCKS_PER_SECOND: u32 = 4194304;
const CLOCKS_PER_FRAME: u32 = CLOCKS_PER_SECOND / 512;
/// How many samples the APU accumulates before handing a block to the player.
///
/// **This value is the floor under end-to-end audio latency.** The consumer can
/// never hold less than one block, and the hand-off queue must cushion a whole
/// block of production burstiness, so the smallest safe queue is several blocks
/// deep. At the original 2000 samples a block is 45 ms at 44.1 kHz — which
/// forced a 159 ms cushion and a 272 ms watermark, i.e. a quarter-second of
/// delay before anything else was even counted, felt on every button press.
///
/// 256 samples is 5.8 ms: comfortably shorter than one emulated frame (16.7 ms,
/// which is the real granularity at which a frame-paced frontend produces
/// audio), so the queue's inherent sawtooth is set by the frame rate rather
/// than by this. Measured cost of the 8x more frequent hand-off is below the
/// noise floor of a headless benchmark — the per-block work is O(samples), so
/// the total is unchanged and only the per-call overhead multiplies, which is
/// why `play_samples` reuses its scratch buffers instead of allocating.
const OUTPUT_SAMPLE_COUNT: usize = 256;
const OUTPUT_SAMPLE_COUNT_U32: u32 = 256;
const SAMPLE_RATE: u32 = 44100;

pub trait AudioPlayer: Send {
    fn play(&mut self, left_channel: &[f32], right_channel: &[f32]);
    fn samples_rate(&self) -> u32;
    fn underflowed(&self) -> bool;
    /// Hint from the frontend that emulation is deliberately running faster
    /// than real time (fast-forward). **A player must never pace the emulator
    /// by blocking in [`play`](AudioPlayer::play)** — it is called from inside
    /// `Cpu::do_cycle`, so any wait there is charged to the emulated frame (see
    /// `cpal_audio::CpalPlayer::play`). The cpal player therefore bounds
    /// latency by discarding, which is already the documented fast-forward
    /// contract ("drop samples rather than pitch-shift"), and this hint needs
    /// no special case there. It stays on the trait for players that do want
    /// one.
    fn set_fast_forward(&mut self, _enabled: bool) {}
}

/// An [`AudioPlayer`] that throws the samples away.
///
/// **A machine always has an APU, whether or not the host has a sound card.**
/// [`Sound::do_cycle`] returns immediately while no player is installed, so a
/// frontend that only calls `enable_audio` when it found a device is not
/// running a quiet Game Boy — it is running one with *no APU at all*. That is
/// observable from the cartridge: `NR52` reads back as powered off, the length
/// counters and frame sequencer never advance, and the CGB PCM12/PCM34
/// registers report nothing. Test ROMs notice (Blargg `dmg_sound`,
/// `interrupt_time` — which spins on `NR52` — and 61 SameSuite APU ROMs), and
/// so does any game that synchronises through the APU.
///
/// So this is what a frontend installs when there is no device, and it is what
/// the whole product does on a build without the `audio` feature. The cost is
/// the APU's own ~30 µs/frame plus a mixdown into a discarded buffer, measured
/// at +0.02 ms/frame against a 16.743 ms budget.
///
/// It reports the same 44.1 kHz the cpal player negotiates by default, so the
/// resampler runs at the rate it normally would.
pub struct SilentPlayer;

impl AudioPlayer for SilentPlayer {
    fn play(&mut self, _left_channel: &[f32], _right_channel: &[f32]) {}
    fn samples_rate(&self) -> u32 {
        SAMPLE_RATE
    }
    fn underflowed(&self) -> bool {
        false
    }
}

struct VolumeEnvelope {
    period: u8,
    goes_up: bool,
    delay: u8,
    initial_volume: u8,
    volume: u8,
    /// The envelope's *pending tick* flip-flop.
    ///
    /// The 3-bit period counter runs at 64 Hz, but the step it asks for is
    /// carried out by a separate latch that is set the moment the counter
    /// reads zero and acted on at the next **512 Hz** DIV-APU event. So an
    /// envelope resumed mid-note steps within ~2 ms rather than waiting a
    /// whole 64 Hz period — SameSuite `channel_N_nrx2_speed_change` and the
    /// long half of `channel_N_nrx2_glitch` both count that extra step.
    clock: bool,
}

impl VolumeEnvelope {
    fn new() -> VolumeEnvelope {
        VolumeEnvelope {
            period: 0,
            goes_up: false,
            delay: 0,
            initial_volume: 0,
            volume: 0,
            clock: false,
        }
    }

    fn wb(&mut self, a: u16, v: u8) {
        match a {
            0xFF12 | 0xFF17 | 0xFF21 => self.write_nrx2(v, false),
            0xFF14 | 0xFF19 | 0xFF23 if v & 0x80 == 0x80 => {
                self.delay = self.period;
                self.volume = self.initial_volume;
                self.clock = false;
            }
            _ => (),
        }
    }

    /// NRx2 write. **The current volume is not reloaded** — only a trigger
    /// does that; NRx2 sets the value the *next* trigger will load. Writing it
    /// while the channel is playing instead perturbs the live volume, the
    /// behaviour known as "zombie mode".
    ///
    /// The rule below is CGB revision E's, derived from SameSuite
    /// `channel_N_volume` (which sweeps every combination of old and new
    /// direction and period against the resulting PCM12 reading) and confirmed
    /// against the short half of `channel_N_nrx2_glitch`. SameSuite's own
    /// README records that this glitch "behaves differently across CPU-CGB
    /// revisions" and that only revision E is documented — DMG's variant, as
    /// described on the gg8 wiki, is a different formula and is not what a
    /// CGB-booting emulator should implement.
    fn write_nrx2(&mut self, v: u8, channel_enabled: bool) {
        let new_period = v & 0x7;
        let new_up = v & 0x8 == 0x8;

        if channel_enabled {
            let mut vol = self.volume as i32;
            // The envelope timer sits idle while its period is 0. Loading a
            // fresh NRx2 restarts it and it fires once immediately — in the
            // OLD direction, because the direction latch has not caught up.
            // Writing an envelope that is off in both senses (counting down
            // with period 0) restarts nothing.
            if self.period == 0 && (new_up || new_period != 0) {
                vol += if self.goes_up { 1 } else { -1 };
            }
            // Switching the envelope from decrement to increment adds two.
            if new_up && !self.goes_up {
                vol += 2;
            }
            // Reversing the direction reverses the counter itself.
            if new_up != self.goes_up {
                vol = 16 - vol;
            }
            self.volume = (vol & 0x0F) as u8;
        }

        // A step already asked for takes its reload from the *new* period.
        if self.clock {
            self.delay = new_period;
        }
        self.period = new_period;
        self.goes_up = new_up;
        self.initial_volume = v >> 4;
    }

    /// One 512 Hz DIV-APU event, in the order the hardware performs it.
    ///
    /// `sixty_four_hz` is true on the one event in eight that clocks the
    /// envelope's period counter. The counter is *free-running* — it reloads
    /// from NRx2 when it reads zero rather than stopping there — so an
    /// envelope parked at zero (which is where a period-0 NRx2 leaves it)
    /// starts stepping as soon as a non-zero period is written, without
    /// waiting for a trigger. `channel_N_nrx2_speed_change` tests 3-7 enable
    /// the envelope mid-note and read the volume back one 64 Hz period at a
    /// time; they see the first step arrive one *512 Hz* event later, not one
    /// 64 Hz period later, which is why the latch is separate from the count.
    fn div_event(&mut self, sixty_four_hz: bool, channel_enabled: bool) {
        if sixty_four_hz && !self.clock && self.delay > 0 {
            self.delay -= 1;
        }
        if self.clock {
            self.clock = false;
            if self.period != 0 {
                if self.goes_up && self.volume < 15 {
                    self.volume += 1;
                } else if !self.goes_up && self.volume > 0 {
                    self.volume -= 1;
                }
            }
        }
        // The "secondary" half of the event: an exhausted counter reloads and
        // asks for a step, which the latch above performs next time round.
        if channel_enabled && self.delay == 0 {
            self.delay = self.period;
            self.clock = true;
        }
    }

    fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.u8(self.period);
        w.bool(self.goes_up);
        w.u8(self.delay);
        w.u8(self.initial_volume);
        w.u8(self.volume);
        w.bool(self.clock);
    }

    fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.period = r.u8()?;
        self.goes_up = r.bool()?;
        self.delay = r.u8()?;
        self.initial_volume = r.u8()?;
        self.volume = r.u8()?;
        self.clock = r.bool()?;
        Ok(())
    }
}

struct LengthCounter {
    enabled: bool,
    value: u16,
    max: u16,
}

impl LengthCounter {
    fn new(max: u16) -> Self {
        LengthCounter {
            enabled: false,
            value: 0,
            max,
        }
    }

    fn wb(&mut self, a: u16, v: u8) {
        match a {
            // NRx1 for the 64-step channels carries the duty cycle in bits 7-6
            // (square) / nothing (noise); only bits 5-0 are the length load.
            // Without the mask a duty-carrying write (e.g. NR11 = 0x80, duty 50%
            // length 0 — what Blargg's dmg_sound and most games write) computes
            // 64 - 128: underflow panic in debug, wrap in release.
            0xFF11 | 0xFF16 | 0xFF20 => self.value = self.max - ((v & 0x3F) as u16),
            0xFF1B => self.value = 256 - (v as u16),
            // NRx4 (enable + trigger reload) is handled by
            // Sound::apply_nrx4_length — it needs the frame-sequencer phase
            // and the channel-enable flag for the extra-clocking quirks.
            _ => (),
        }
    }

    /// One 256 Hz length clock. Returns true when the counter just expired —
    /// the caller must then disable the channel (hardware clears the channel
    /// enable; nothing short of a trigger re-enables it, so a later length
    /// reload or length-disable must NOT resurrect the NR52 status bit —
    /// dmg_sound 02-len ctr tests 8 and 10).
    fn step(&mut self) -> bool {
        if self.enabled && self.value > 0 {
            self.value -= 1;
            return self.value == 0;
        }
        false
    }

    fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.bool(self.enabled);
        w.u16(self.value);
        w.u16(self.max);
    }

    fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.enabled = r.bool()?;
        self.value = r.u16()?;
        self.max = r.u16()?;
        Ok(())
    }
}

/// Extra T-cycles beyond one full period before a square channel's first duty
/// step after a trigger. SameSuite `channel_1_delay` / `channel_2_delay`
/// document the total as "(sample length + 2) ticks from the moment channel 1
/// is enabled until PCM12 is affected" and step their read point one M-cycle
/// at a time, so this constant is measured, not guessed.
const SQUARE_TRIGGER_DELAY: u32 = 6;

/// Phase, on the APU's free-running 1 MHz (4 T-cycle) grid, that a square
/// channel's first post-trigger duty step lands on.
///
/// The frequency timer is clocked by that grid, not by the write: a trigger
/// reloads the counter but the reload only takes effect at the next 1 MHz
/// edge, so the true delay is "period + [`SQUARE_TRIGGER_DELAY`], rounded up
/// to the grid". At single speed the CPU is locked to the same grid and this
/// is invisible; at double speed the CPU can write NRx4 on either half of a
/// grid cell, which is exactly what SameSuite's `channel_N_align` tests do —
/// they shift the trigger by one double-speed M-cycle (2 T) at a time and
/// show the first duty step *not* moving with it.
const SQUARE_TRIGGER_ALIGN: u32 = 0;

/// T-cycles knocked off [`SQUARE_TRIGGER_DELAY`] when the trigger is a
/// *re*-trigger of an already-running channel. SameSuite `channel_N_restart`
/// states it outright: "after restarting, the start delay from the 'delay'
/// test is actually 1 tick shorter. The countdown for the next sample is
/// reset, but the new pulse's first sample will be the next sample the old
/// pulse would have played" — which is also why the duty position is left
/// alone here.
const SQUARE_RESTART_SPEEDUP: u32 = 4;

/// How late, in T-cycles, the frequency timer samples the value it reloads
/// itself with after an expiry.
///
/// The reload is not instantaneous. A square channel's period register is
/// read a few cycles *after* the expiry that reloads the timer, so a write to
/// NRx3/NRx4 landing inside that window still chooses the period the timer
/// has just reloaded with: the next sample then arrives a whole **new** period
/// from that expiry, rather than the old countdown finishing first. That is
/// one duty step earlier than the plain "a frequency write takes effect at the
/// next expiry" rule gives, and it is the whole of
/// `channel_1_freq_change_timing`.
///
/// **Measured, not chosen** (`-cgbDE`, the CPU CGB revision this emulator
/// models by default). The ROM sweeps the write across the expiry one nop at a
/// time and reads PCM12 twice — right after the write, and 16 nops later once
/// the channel has frozen on the sample it will hold — at single **and**
/// double speed, so it pins the window from both sides. The passing band is
/// `5..=6` T-cycles, uniform across both speeds; `7` and above fails every
/// variant of the ROM. **Do not "tidy" this to a round 4** — 4 is the *other*
/// console's answer, [`SQUARE_RELOAD_LATCH_AGB`].
const SQUARE_RELOAD_LATCH: u32 = 6;

/// The same window on a **Game Boy Advance** — two T-cycles narrower.
///
/// The suite ships this measurement as its own ROM,
/// `channel_1_freq_change_timing-A`, beside `-cgbDE` and `-cgb0BC`; the three
/// tables are the same program and disagree only about this window, so **no
/// single machine can pass more than one of them**. That is the same shape as
/// SameSuite's `-cgb0` / `-cgbB` pair, which this emulator already answers by
/// booting the console the ROM's filename names (see [`CgbRevision`] and
/// `examples/samesuite.rs`); an AGB is simply another console, and this
/// repository already models one for its boot state ([`crate::mode::Model::Agb`]).
///
/// The behaviour behind the two numbers is the correction the reload applies to
/// the duty position when a frequency write drops NRx4's high bits from 7:
/// SameBoy applies it unconditionally on CPU CGB D and E and makes it depend on
/// the 2 MHz clock phase on an AGB and on CPU CGB C and older (`Core/apu.c`,
/// the `GB_IO_NR14` case — *"On an AGB, as well as on CGB C and earlier, it
/// behaves slightly different on double speed"*). Read there, reimplemented
/// here as the width of the window itself, which is what this ROM measures.
///
/// **Measured the same way, and the two bands do not overlap or touch the
/// edges of the sweep:** `1..=2` fails both, `3..=4` passes `-A` only, `5..=6`
/// passes `-cgbDE` only, `7` fails both. Nothing else in the 76-ROM suite moves
/// anywhere in `1..=6`, so the console selection is the *whole* of the
/// difference.
const SQUARE_RELOAD_LATCH_AGB: u32 = 4;

/// How long the sweep unit's recalculation takes, in T-cycles, for a given
/// NR10 shift.
///
/// The sweep shifter is a serial shift-and-add: it walks the shadow frequency
/// down one bit position per 1 MHz tick, so a larger shift takes longer, and
/// the overflow check that can kill the channel only happens when it finishes.
/// SameSuite `channel_1_sweep` reads PCM12 one M-cycle at a time across the
/// event and shows the channel still playing for exactly 8 M-cycles after a
/// shift-7 sweep updates the frequency, then silent — hence `shift + 1`.
fn sweep_calc_delay_for(shift: u8) -> u32 {
    (shift as u32 + 1) * 4
}

/// T-cycles after a channel-1 trigger during which the sweep unit's shadow
/// register is not yet readable — the "restart hold".
///
/// A trigger reloads the shadow from the frequency registers, and that reload
/// is not instantaneous: for three M-cycles afterwards the overflow check
/// reads the shadow as zero. So a 128 Hz sweep event landing inside the window
/// latches nothing, and the delayed overflow check it arms compares zeros —
/// the channel survives an overflow that would have killed it a cycle later.
///
/// SameSuite `channel_1_sweep_restart_2` is entirely this window: it re-triggers
/// channel 1 at `$7FF` (so `$7FF + $7FF` overflows) a fixed 0-7 M-cycles before
/// a sweep event and requires the channel to die **only** when the event lands
/// a full three M-cycles after the trigger. Measured in this APU's clock phase:
/// an event at 12 T after the trigger kills, one at 8 T does not.
const SWEEP_RESTART_HOLD: u32 = 12;

struct SquareChannel {
    enabled: bool,
    dac_enabled: bool,
    volume: VolumeEnvelope,
    length: LengthCounter,
    duty: u8,
    /// Position in the duty pattern. Advanced at each frequency-timer expiry,
    /// reset only by an APU power-off — a trigger reloads the timer
    /// but leaves the position alone (gg8 wiki, "Gameboy sound hardware" →
    /// Trigger Event).
    duty_pos: u8,
    /// The duty bit the channel is **currently driving into its DAC**. It is a
    /// latch, not a function of `duty_pos`: it only takes a new value when the
    /// frequency timer expires, so a freshly powered-on channel reads 0 for
    /// the whole first pattern position even where the duty table says high.
    /// SameSuite `channel_N_duty` measures exactly that — with duty 0 (one
    /// high position out of eight) the first cycle's high is missing and the
    /// second is not.
    duty_out: bool,
    freq: u16,
    period: u32,
    delay: u32,
    /// T-cycles since the frequency timer last latched a reload value, i.e.
    /// since the last expiry. Saturates, and a trigger parks it out of range —
    /// a trigger loads the timer itself, so the expiry the reload latch would
    /// edit no longer exists. Only ever read within [`SQUARE_RELOAD_LATCH`] of
    /// zero, by [`SquareChannel::write_freq`]; `period - delay` cannot stand in
    /// for it, because a freshly triggered channel walks that same range with
    /// no expiry behind it (SameSuite `channel_N_freq_change` writes NRx3
    /// exactly there).
    since_expiry: u32,
    sweep_period: u8,
    sweep_shift: u8,
    sweep_decrease: bool,
    sweep_delay: u8,
    sweep_freq: u16,
    sweep_enabled: bool,
    /// A sweep calculation has run in negate mode since the last trigger —
    /// clearing NR10's negate bit while this is set disables the channel.
    sweep_negate_used: bool,
    /// T-cycles until the pending sweep *recalculation* runs (0 = none).
    ///
    /// The sweep unit does not check for overflow at the instant it writes a
    /// new frequency: the shifter needs time, so the second calculation — the
    /// one whose overflow kills the channel — lands a few cycles later. Every
    /// event that (re)starts the shifter arms this: a 128 Hz sweep event, a
    /// trigger with a non-zero shift, and an NR10 write. See
    /// [`sweep_calc_delay_for`].
    sweep_calc_delay: u32,
    /// The pending calculation was armed with a zero shift, so the "paused
    /// while NR10's shift reads zero" rule does not apply to it.
    sweep_calc_unshifted: bool,
    /// The shifted value the pending overflow check adds to the shadow,
    /// latched when that check was **armed** — not recomputed when it fires,
    /// so an NR10 shift written in between does not change it, and a sweep
    /// event inside the restart hold latches nothing at all. See
    /// [`SWEEP_RESTART_HOLD`].
    sweep_addend: u16,
    /// T-cycles left of the post-trigger restart hold, during which the sweep
    /// shadow reads zero ([`SWEEP_RESTART_HOLD`]).
    sweep_hold: u32,
    blip: BlipBuf,
    /// Whether waveform transitions reach `blip` at all. See
    /// [`Sound::set_render_audio`]; never serialised, because it is a property
    /// of the host, not of the machine.
    render: bool,
    last_amp: i32,
}

impl SquareChannel {
    fn new(blip: BlipBuf) -> SquareChannel {
        SquareChannel {
            enabled: false,
            dac_enabled: false,
            volume: VolumeEnvelope::new(),
            length: LengthCounter::new(64),
            duty: 1,
            duty_pos: 0,
            duty_out: false,
            freq: 0,
            period: 2048,
            delay: 0,
            since_expiry: u32::MAX,
            sweep_period: 0,
            sweep_shift: 0,
            sweep_decrease: false,
            sweep_delay: 0,
            sweep_freq: 0,
            sweep_enabled: false,
            sweep_negate_used: false,
            sweep_calc_delay: 0,
            sweep_calc_unshifted: false,
            sweep_addend: 0,
            sweep_hold: 0,
            blip,
            render: true,
            last_amp: 0,
        }
    }

    fn calculate_period(&mut self) {
        if self.freq > 2047 {
            self.period = 0;
        } else {
            self.period = (2048 - self.freq as u32) * 4;
        }
    }

    /// Write the frequency registers of a *running* channel.
    ///
    /// The frequency timer keeps its countdown — a frequency write never
    /// reloads it — so the new period normally takes effect only at the next
    /// expiry. The exception is [`SQUARE_RELOAD_LATCH`]: the expiry that has
    /// just happened has not yet committed to a reload value, so a write
    /// inside that window is applied to it retroactively and the next sample
    /// lands a whole *new* period from that expiry.
    ///
    /// The window is tested against [`SquareChannel::since_expiry`] and **not**
    /// against `period - delay`, which looks equivalent and is not: a channel
    /// that has been triggered but not yet expired sits in that range with no
    /// reload behind it to edit.
    fn write_freq(&mut self, freq: u16, latch: u32) {
        let old_period = self.period;
        let inside_latch = self.enabled && old_period != 0 && self.since_expiry < latch;
        self.freq = freq;
        self.calculate_period();
        if inside_latch && self.period != 0 {
            // Re-do the last reload with the value just written: the next
            // sample is now a whole new period from that expiry.
            self.delay = (self.delay + self.period).saturating_sub(old_period);
        }
    }

    /// Advance the channel over `[start_time, end_time)`, splitting the window
    /// at a pending sweep overflow check so the channel goes quiet on the
    /// exact cycle hardware kills it (see [`SquareChannel::sweep_calc_delay`]).
    /// Hand a waveform transition to the band-limited resampler — unless
    /// [`Sound::set_render_audio`] has been turned off, in which case the
    /// transition is still computed and `last_amp` still updated, and only the
    /// *output* is dropped. See that method for why that is guest-invisible.
    #[inline(always)]
    fn delta(&mut self, time: u32, amount: i32) {
        if self.render {
            self.blip.add_delta(time, amount);
        }
    }

    fn run(&mut self, start_time: u32, end_time: u32) {
        // A pending calculation is *paused*, not cancelled, while NR10's shift
        // field reads zero — there is nothing for the shifter to do, so it
        // stops where it is and resumes if a shift is written back. SameSuite
        // `channel_1_sweep_restart` round 3 is exactly this: it clears NR10
        // inside the window and the channel, which was about to be killed by
        // the overflow check, keeps playing.
        if self.sweep_calc_delay > 0
            && (self.sweep_shift != 0 || self.sweep_calc_unshifted)
            && end_time > start_time
        {
            let span = end_time - start_time;
            if self.sweep_calc_delay <= span {
                let mid = start_time + self.sweep_calc_delay;
                self.sweep_hold = self.sweep_hold.saturating_sub(self.sweep_calc_delay);
                self.sweep_calc_delay = 0;
                self.run_span(start_time, mid);
                // The recalculation's only job is the overflow check; the
                // shadow frequency was already written at the sweep event.
                self.sweep_overflow_check();
                self.sweep_hold = self.sweep_hold.saturating_sub(end_time - mid);
                self.run_span(mid, end_time);
                return;
            }
            self.sweep_calc_delay -= span;
        }
        self.sweep_hold = self
            .sweep_hold
            .saturating_sub(end_time.saturating_sub(start_time));
        self.run_span(start_time, end_time);
    }

    fn run_span(&mut self, start_time: u32, end_time: u32) {
        if !self.enabled || self.period == 0 {
            if self.last_amp != 0 {
                self.delta(start_time, -self.last_amp);
                self.last_amp = 0;
                self.delay = 0;
            }
        } else {
            let mut time = start_time + self.delay;
            let pattern = DUTY_PATTERN[self.duty as usize];
            let vol = self.volume.volume as i32;
            let mut last_expiry = None;

            // At each frequency-timer expiry the duty counter advances and
            // the channel latches the bit at the new position. (`delay` is
            // time-to-next-expiry, which a trigger reloads.)
            while time < end_time {
                self.duty_pos = (self.duty_pos + 1) & 7;
                self.duty_out = pattern[self.duty_pos as usize] != 0;
                let amp = if self.duty_out { vol } else { -vol };
                if amp != self.last_amp {
                    self.delta(time, amp - self.last_amp);
                    self.last_amp = amp;
                }

                last_expiry = Some(time);
                time += self.period;
            }

            self.delay = time - end_time;
            self.since_expiry = match last_expiry {
                Some(t) => end_time - t,
                None => self
                    .since_expiry
                    .saturating_add(end_time.saturating_sub(start_time)),
            };
        }
    }

    /// The channel's live 4-bit digital output — what PCM12/PCM34 report
    /// (Pan Docs, "Audio Registers" → PCM12/PCM34; CGB only). Zero whenever
    /// the channel or its DAC is off; otherwise the envelope volume gated by
    /// the current duty bit.
    fn digital_output(&self) -> u8 {
        if self.enabled && self.dac_enabled && self.duty_out {
            self.volume.volume
        } else {
            0
        }
    }

    /// `grid_phase` is the APU clock's position on its 1 MHz grid (`0..=3`) at
    /// the cycle the trigger lands — see [`SQUARE_TRIGGER_ALIGN`].
    fn trigger(&mut self, grid_phase: u32) {
        let restart = self.enabled;
        self.enabled = true;
        self.calculate_period();
        // Hardware reloads the frequency timer on trigger, so the first duty
        // step is a whole period away — SameSuite `channel_N_delay` measures
        // that delay as (sample length + 2) M-cycles and fails a channel that
        // steps immediately. The duty *position* is deliberately not reset.
        let pad = SQUARE_TRIGGER_ALIGN.wrapping_sub(grid_phase) & 3;
        let base = if restart {
            SQUARE_TRIGGER_DELAY - SQUARE_RESTART_SPEEDUP
        } else {
            SQUARE_TRIGGER_DELAY
        };
        self.delay = self.period + base + pad;
        // The trigger latched the reload itself, so there is no just-expired
        // reload for a following NRx3/NRx4 write to edit.
        self.since_expiry = u32::MAX;
        // Sweep trigger semantics (gg8 wiki; dmg_sound 04-sweep): the shadow
        // frequency and timer ALWAYS reload, the internal enable flag is set
        // iff period or shift is non-zero (a stale `true` from an earlier
        // trigger must not survive — 04-sweep test 10), the negate-used flag
        // resets, and a non-zero shift performs an immediate calculation +
        // overflow check (which can disable the channel on the spot).
        self.sweep_freq = self.freq;
        self.sweep_delay = if self.sweep_period == 0 {
            8
        } else {
            self.sweep_period
        };
        self.sweep_enabled = self.sweep_period != 0 || self.sweep_shift != 0;
        self.sweep_negate_used = false;
        self.sweep_calc_delay = 0;
        // The shadow reload is not instantaneous: for [`SWEEP_RESTART_HOLD`]
        // T-cycles the overflow check reads the shadow as zero, and the addend
        // a sweep event would latch in that window is never latched. The
        // trigger's own check is armed after the hold closes, so it gets the
        // addend for the frequency just written (SameSuite
        // `channel_1_sweep_restart_2`).
        self.sweep_hold = SWEEP_RESTART_HOLD;
        self.sweep_addend = if self.sweep_shift != 0 {
            self.freq >> self.sweep_shift
        } else {
            0
        };
        if self.sweep_shift != 0 {
            // A trigger's own calculation starts one M-cycle later than a
            // sweep event's: the write has to land before the shifter starts.
            // `channel_1_sweep_restart` round 2 pins the difference — it
            // re-triggers one M-cycle after a sweep event and the channel
            // survives two M-cycles longer than round 5, which rewrites NR10
            // at the same moment and does not restart the shifter.
            self.sweep_calc_delay = sweep_calc_delay_for(self.sweep_shift) + 4;
            self.sweep_calc_unshifted = false;
        }
    }

    /// Start the shifter: the recalculation + overflow check runs
    /// [`sweep_calc_delay_for`] T-cycles from now.
    ///
    /// The value the check will add is latched **here**, at arm time — inside
    /// the post-trigger restart hold the shadow reads zero, so nothing is
    /// latched and the check compares whatever the trigger left behind.
    fn arm_sweep_calc(&mut self) {
        self.sweep_calc_delay = sweep_calc_delay_for(self.sweep_shift);
        self.sweep_calc_unshifted = self.sweep_shift == 0;
        if self.sweep_hold == 0 {
            self.sweep_addend = self.sweep_freq >> self.sweep_shift;
        }
    }

    /// The overflow check a pending calculation performs when its shifter
    /// finishes: the sweep shadow plus the addend latched when the check was
    /// armed. Inside the restart hold the shadow reads zero, which is what
    /// lets a channel survive a sweep event that lands a cycle or two after
    /// its trigger (see [`SWEEP_RESTART_HOLD`]).
    fn sweep_overflow_check(&mut self) {
        if self.sweep_decrease {
            // Negate mode never overflows; the calculation only arms the
            // negate-exit quirk (dmg_sound 05-sweep details tests 4-6).
            self.sweep_negate_used = true;
            return;
        }
        let shadow = if self.sweep_hold == 0 {
            self.sweep_freq
        } else {
            0
        };
        if shadow as u32 + self.sweep_addend as u32 > 2047 {
            self.enabled = false;
        }
    }

    /// One sweep frequency calculation + overflow check. Every calculation
    /// made in negate mode is remembered (`sweep_negate_used`): clearing the
    /// negate bit in NR10 afterwards kills the channel (the sweep negate
    /// quirk — dmg_sound 05-sweep details tests 4-6).
    fn calc_sweep_freq(&mut self) -> u16 {
        let offset = self.sweep_freq >> self.sweep_shift;
        if self.sweep_decrease {
            self.sweep_negate_used = true;
            self.sweep_freq.wrapping_sub(offset)
        } else {
            let new_freq = self.sweep_freq + offset;
            if new_freq > 2047 {
                self.enabled = false;
            }
            new_freq
        }
    }

    fn sweep_step(&mut self) {
        if !self.sweep_enabled {
            return;
        }
        if self.sweep_delay > 1 {
            self.sweep_delay -= 1;
        } else {
            self.sweep_delay = if self.sweep_period == 0 {
                8
            } else {
                self.sweep_period
            };
            if self.sweep_period != 0 {
                if self.sweep_shift != 0 {
                    let new_freq = self.calc_sweep_freq();
                    if new_freq < 2048 {
                        self.sweep_freq = new_freq;
                        self.freq = new_freq;
                        self.calculate_period();
                    }
                }
                // The overflow check on the *next* value is not made here —
                // the shifter takes time. SameSuite `channel_1_sweep` measures
                // the gap: with shift 7 the channel keeps playing for 8 more
                // M-cycles after the frequency update before the check kills
                // it. With shift 0 nothing is written at all and the whole
                // event is just this delayed check
                // (`channel_1_sweep_restart_2`).
                self.arm_sweep_calc();
            }
        }
    }

    // The `blip` buffer holds transient output deltas that are drained every
    // output period; it is not part of the logical channel state, so it is not
    // serialised (a fresh, correctly-rated BlipBuf is kept on load). `last_amp`
    // is saved to preserve delta continuity.
    fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.bool(self.enabled);
        w.bool(self.dac_enabled);
        self.volume.save_state(w);
        self.length.save_state(w);
        w.u8(self.duty);
        w.u8(self.duty_pos);
        w.bool(self.duty_out);
        w.u16(self.freq);
        w.u32(self.period);
        w.u32(self.delay);
        w.u32(self.since_expiry);
        w.u8(self.sweep_period);
        w.u8(self.sweep_shift);
        w.bool(self.sweep_decrease);
        w.u8(self.sweep_delay);
        w.u16(self.sweep_freq);
        w.bool(self.sweep_enabled);
        w.bool(self.sweep_negate_used);
        w.u32(self.sweep_calc_delay);
        w.bool(self.sweep_calc_unshifted);
        w.u16(self.sweep_addend);
        w.u32(self.sweep_hold);
        w.i32(self.last_amp);
    }

    fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.enabled = r.bool()?;
        self.dac_enabled = r.bool()?;
        self.volume.load_state(r)?;
        self.length.load_state(r)?;
        self.duty = r.u8()?;
        self.duty_pos = r.u8()?;
        self.duty_out = r.bool()?;
        self.freq = r.u16()?;
        self.period = r.u32()?;
        self.delay = r.u32()?;
        self.since_expiry = r.u32()?;
        self.sweep_period = r.u8()?;
        self.sweep_shift = r.u8()?;
        self.sweep_decrease = r.bool()?;
        self.sweep_delay = r.u8()?;
        self.sweep_freq = r.u16()?;
        self.sweep_enabled = r.bool()?;
        self.sweep_negate_used = r.bool()?;
        self.sweep_calc_delay = r.u32()?;
        self.sweep_calc_unshifted = r.bool()?;
        self.sweep_addend = r.u16()?;
        self.sweep_hold = r.u32()?;
        self.last_amp = r.i32()?;
        Ok(())
    }
}

struct WaveChannel {
    enabled: bool,
    dac_enabled: bool,
    length: LengthCounter,
    freq: u16,
    period: u32,
    delay: u32,
    volume_shift: u8,
    waveram: [u8; 16],
    wave_pos: u8,
    /// The raw 4-bit sample the channel is currently driving into its DAC.
    /// Latched at each fetch, which is why it survives a re-trigger (hardware
    /// resets `wave_pos` but does *not* refill the sample buffer — Pan Docs,
    /// "Audio Details" → Wave Channel) and is what PCM34 reports.
    sample_latch: u8,
    /// T-cycles from the channel's most recent wave-RAM byte fetch to the
    /// point `run()` last synced to (`u32::MAX` = no fetch yet). Consulted at
    /// CPU access time for the DMG wave-RAM access window: on DMG, wave RAM
    /// is only readable/writable while CH3 is enabled during the couple of
    /// T-cycles around the channel's own fetch (Pan Docs "Audio Details" →
    /// Wave RAM access; Blargg dmg_sound 09/12). Valid because the MMU steps
    /// the APU clock per bus access and calls `catch_up` before every APU
    /// register access, so `run()` has always synced to the exact access
    /// cycle when this is read.
    since_fetch: u32,
    blip: BlipBuf,
    /// Whether waveform transitions reach `blip` at all. See
    /// [`Sound::set_render_audio`]; never serialised, because it is a property
    /// of the host, not of the machine.
    render: bool,
    last_amp: i32,
}

/// T-cycles added to the wave channel's first fetch delay on trigger, beyond
/// one full period: hardware reloads the frequency timer with period + 4
/// (calibrated against Blargg dmg_sound 09 — a ±2-cycle error shifts every
/// windowed read/write and fails 09/10/12; matches SameBoy's
/// `(freq ^ 0x7FF) + 3` 2-MHz reload).
const WAVE_TRIGGER_DELAY: u32 = 4;

/// Width of the DMG wave-RAM access window, in T-cycles since the channel's
/// last fetch. CPU accesses land on a 4-T-cycle grid while fetches land on a
/// 2-T-cycle grid, so this 2-cycle window admits exactly one fetch phase —
/// which is why dmg_sound 09's answer alternates $FF / current-byte as it
/// steps its read time 2 cycles per iteration.
const WAVE_ACCESS_WINDOW: u32 = 2;

impl WaveChannel {
    fn new(blip: BlipBuf) -> WaveChannel {
        WaveChannel {
            enabled: false,
            dac_enabled: false,
            length: LengthCounter::new(256),
            freq: 0,
            period: 2048,
            delay: 0,
            volume_shift: 0,
            waveram: [
                0x84, 0x40, 0x43, 0xAA, 0x2D, 0x78, 0x92, 0x3C, 0x60, 0x59, 0x59, 0xB0,
                0x34, 0xB8, 0x2E, 0xDA,
            ], // Default wave pattern
            wave_pos: 0,
            sample_latch: 0,
            since_fetch: u32::MAX,
            blip,
            render: true,
            last_amp: 0,
        }
    }

    /// The wave-RAM byte index CH3 is reading at the current sync point, if
    /// the CPU may access wave RAM right now under DMG rules — `None` means
    /// the access window is closed (reads see $FF, writes are dropped). The
    /// window is the `WAVE_ACCESS_WINDOW` T-cycles *after* a fetch: the CPU
    /// sees the byte CH3 just loaded into its sample latch. Calibrated
    /// against Blargg dmg_sound 09/10/12 (each steps its access phase in
    /// 2-T-cycle increments, so a ±2-cycle error in either constant flips
    /// every windowed access and fails all three CRCs).
    fn dmg_access_byte(&self) -> Option<usize> {
        if !self.enabled {
            // Channel off: wave RAM is freely accessible (not this path).
            return None;
        }
        if self.since_fetch <= WAVE_ACCESS_WINDOW {
            Some(self.wave_pos as usize / 2)
        } else {
            None
        }
    }

    fn calculate_period(&mut self) {
        if self.freq > 2047 {
            self.period = 0;
        } else {
            self.period = (2048 - self.freq as u32) * 2;
        }
    }

    /// Live 4-bit digital output for PCM34 — the latched sample shifted by the
    /// NR32 volume code (0 = mute).
    fn digital_output(&self) -> u8 {
        if !self.enabled || !self.dac_enabled {
            return 0;
        }
        match self.volume_shift {
            1 => self.sample_latch,
            2 => self.sample_latch >> 1,
            3 => self.sample_latch >> 2,
            _ => 0,
        }
    }

    /// Hand a waveform transition to the band-limited resampler — unless
    /// [`Sound::set_render_audio`] has been turned off, in which case the
    /// transition is still computed and `last_amp` still updated, and only the
    /// *output* is dropped. See that method for why that is guest-invisible.
    #[inline(always)]
    fn delta(&mut self, time: u32, amount: i32) {
        if self.render {
            self.blip.add_delta(time, amount);
        }
    }

    fn run(&mut self, start_time: u32, end_time: u32) {
        if !self.enabled || !self.dac_enabled || self.period == 0 {
            if self.last_amp != 0 {
                self.delta(start_time, -self.last_amp);
                self.last_amp = 0;
                self.delay = 0;
            }
            self.since_fetch = self
                .since_fetch
                .saturating_add(end_time.saturating_sub(start_time));
        } else {
            let mut time = start_time + self.delay;

            // Volume shift for wave channel (output at 4x amplitude)
            let volshift = match self.volume_shift {
                0 => 4 + 2, // Mute
                1 => 0,     // 100%
                2 => 1,     // 50%
                3 => 2,     // 25%
                _ => 4 + 2,
            };

            let mut last_fetch: Option<u32> = None;
            while time < end_time {
                // The position advances *then* the sample is fetched, which is
                // why a freshly triggered CH3 never plays wave sample 0: the
                // trigger resets the position to 0 and the first fetch is
                // already sample 1 (Pan Docs, "Audio Details" → Wave Channel;
                // SameSuite `channel_3_first_sample` puts a distinguishable
                // nibble in each half of byte 0 and reads back the second).
                self.wave_pos = (self.wave_pos + 1) % 32;
                let byte = self.waveram[self.wave_pos as usize / 2];
                let sample = if self.wave_pos & 1 == 0 {
                    byte >> 4
                } else {
                    byte & 0xF
                };

                self.sample_latch = sample;
                // Shift left by 2 for 4x amplitude
                let amp = ((sample << 2) >> volshift) as i32;

                if amp != self.last_amp {
                    self.delta(time, amp - self.last_amp);
                    self.last_amp = amp;
                }

                last_fetch = Some(time);
                time += self.period;
            }

            self.delay = time - end_time;
            self.since_fetch = match last_fetch {
                Some(t) => end_time - t,
                None => self
                    .since_fetch
                    .saturating_add(end_time.saturating_sub(start_time)),
            };
        }
    }

    fn trigger(&mut self) {
        self.enabled = true;
        self.wave_pos = 0;
        self.calculate_period();
        // Hardware reloads the frequency timer with one full period plus a
        // small fixed lag; the first wave-RAM fetch after a trigger lands
        // period + WAVE_TRIGGER_DELAY T-cycles later (dmg_sound 09 measures
        // this to 2 T-cycles).
        self.delay = self.period + WAVE_TRIGGER_DELAY;
    }

    fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.bool(self.enabled);
        w.bool(self.dac_enabled);
        self.length.save_state(w);
        w.u16(self.freq);
        w.u32(self.period);
        w.u32(self.delay);
        w.u8(self.volume_shift);
        w.bytes(&self.waveram);
        w.u8(self.wave_pos);
        w.u8(self.sample_latch);
        // Live at any save point while CH3 plays: the DMG access window is
        // derived from it, so dropping it desyncs windowed wave-RAM
        // reads/writes after a load (format v3).
        w.u32(self.since_fetch);
        w.i32(self.last_amp);
    }

    fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.enabled = r.bool()?;
        self.dac_enabled = r.bool()?;
        self.length.load_state(r)?;
        self.freq = r.u16()?;
        self.period = r.u32()?;
        self.delay = r.u32()?;
        self.volume_shift = r.u8()?;
        r.fill(&mut self.waveram)?;
        self.wave_pos = r.u8()?;
        self.sample_latch = r.u8()?;
        self.since_fetch = r.u32()?;
        self.last_amp = r.i32()?;
        Ok(())
    }
}

/// T-cycles from a noise-channel trigger to its first LFSR step, beyond the
/// reload of the frequency timer. SameSuite `channel_4_delay` measures the
/// total as "sample length + 3 M-cycles" and steps its read point one M-cycle
/// at a time; `channel_4_lfsr` then pins the *phase* by reconstructing the
/// LFSR from 128 consecutive samples, so an error here rotates the whole
/// reconstructed sequence.
const NOISE_TRIGGER_DELAY: u32 = 2;

/// Phase, on the APU's 1 MHz grid, that the noise channel's first post-trigger
/// LFSR step lands on — the same free-running-clock effect as
/// [`SQUARE_TRIGGER_ALIGN`], and measured the same way by SameSuite's
/// `channel_4_align` (which shifts the trigger one double-speed M-cycle at a
/// time and shows the first sample moving in 4 T-cycle steps, not 2).
const NOISE_TRIGGER_ALIGN: u32 = 0;

struct NoiseChannel {
    enabled: bool,
    dac_enabled: bool,
    volume: VolumeEnvelope,
    length: LengthCounter,
    shift: u8,
    width_mode: bool,
    divisor: u8,
    lfsr: u16,
    period: u32,
    delay: u32,
    blip: BlipBuf,
    /// Whether waveform transitions reach `blip` at all. See
    /// [`Sound::set_render_audio`]; never serialised, because it is a property
    /// of the host, not of the machine.
    render: bool,
    last_amp: i32,
}

impl NoiseChannel {
    fn new(blip: BlipBuf) -> NoiseChannel {
        NoiseChannel {
            enabled: false,
            dac_enabled: false,
            volume: VolumeEnvelope::new(),
            length: LengthCounter::new(64),
            shift: 0,
            width_mode: false,
            divisor: 0,
            lfsr: 0x7FFF,
            period: 8,
            delay: 0,
            blip,
            render: true,
            last_amp: 0,
        }
    }

    /// NR43's divisor stage, in T-cycles: code 0 is half a tick (8), every
    /// other code is that many 16 T-cycle ticks (Pan Docs, "Audio Registers"
    /// → NR43).
    fn divisor_period(&self) -> u32 {
        if self.divisor == 0 {
            8
        } else {
            self.divisor as u32 * 16
        }
    }

    /// What the divisor stage is left holding by a trigger — half its period,
    /// raced against the tick already in flight on the free-running grid.
    /// See [`NoiseChannel::trigger`].
    fn trigger_divisor(&self, grid_phase: u32) -> u32 {
        let tick = if self.divisor == 0 { 8 } else { 16 };
        let phase = grid_phase & 4;
        (self.divisor_period() / 2 + phase).max(tick - phase)
    }

    fn calculate_period(&mut self) {
        self.period = self.divisor_period() << self.shift;
    }

    /// Live 4-bit digital output for PCM34. The LFSR drives the DAC high when
    /// its bit 0 is **clear** (Pan Docs, "Audio Details" → Noise Channel).
    fn digital_output(&self) -> u8 {
        if self.enabled && self.dac_enabled && self.lfsr & 1 == 0 {
            self.volume.volume
        } else {
            0
        }
    }

    /// Hand a waveform transition to the band-limited resampler — unless
    /// [`Sound::set_render_audio`] has been turned off, in which case the
    /// transition is still computed and `last_amp` still updated, and only the
    /// *output* is dropped. See that method for why that is guest-invisible.
    #[inline(always)]
    fn delta(&mut self, time: u32, amount: i32) {
        if self.render {
            self.blip.add_delta(time, amount);
        }
    }

    fn run(&mut self, start_time: u32, end_time: u32) {
        if !self.enabled {
            if self.last_amp != 0 {
                self.delta(start_time, -self.last_amp);
                self.last_amp = 0;
                self.delay = 0;
            }
        } else {
            let mut time = start_time + self.delay;

            while time < end_time {
                // Update LFSR
                let bit = (self.lfsr & 1) ^ ((self.lfsr >> 1) & 1);
                self.lfsr = (self.lfsr >> 1) | (bit << 14);
                if self.width_mode {
                    self.lfsr = (self.lfsr & !0x40) | (bit << 6);
                }

                // Calculate amplitude based on LFSR bit 0
                let amp = if self.lfsr & 1 == 0 {
                    self.volume.volume as i32
                } else {
                    -(self.volume.volume as i32)
                };

                if amp != self.last_amp {
                    self.delta(time, amp - self.last_amp);
                    self.last_amp = amp;
                }

                time += self.period;
            }

            self.delay = time - end_time;
        }
    }

    fn trigger(&mut self, grid_phase: u32) {
        let restart = self.enabled;
        self.enabled = true;
        self.lfsr = 0x7FFF;
        self.calculate_period();
        // The noise frequency is built from two counters — the NR43 divisor
        // and the shift stage above it — and a trigger reloads *neither*: it
        // leaves both half-way through. So the first LFSR step lands at
        // `(period - divisor)/2 + divisor/2`, not at a full period.
        //
        // The one exception is that the divisor stage cannot be left holding
        // less than one of its own ticks (16 T-cycles, or 8 for code 0, which
        // is a half tick to begin with), so for codes 0 and 1 the half-reload
        // is floored at a whole tick. SameSuite pins the whole shape:
        // `channel_4_delay` moves the shift only (0/1/3 M-cycles late for a
        // plain full-period reload), and `channel_4_frequency_alignment`
        // sweeps the divisor at nine (divisor, shift) pairs — the four
        // encodings of one 16 M-cycle sample ($0C/$1A/$29/$38) start
        // 0/0/+2/+1 M-cycles apart, which is this floor and nothing else.
        // `channel_4_equivalent_frequencies` re-measures the same four over
        // two whole samples.
        let divisor = self.trigger_divisor(grid_phase);
        let pad = NOISE_TRIGGER_ALIGN.wrapping_sub(grid_phase) & 3;
        self.delay = (self.period - self.divisor_period()) / 2
            + divisor
            + NOISE_TRIGGER_DELAY
            + pad;
        // Re-triggering a *running* channel costs one whole sample more. The
        // divider that clocks the LFSR is free-running — a trigger reloads the
        // countdown to its next tick but not the divider itself, so the first
        // post-restart step waits for the divider's next edge rather than
        // landing on the reloaded countdown. SameSuite `channel_4_lfsr_restart`
        // and `_fast` reconstruct 128 consecutive LFSR outputs after a restart:
        // without this the whole sequence is right but one sample early.
        if restart {
            self.delay += self.period;
        }
    }

    fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.bool(self.enabled);
        w.bool(self.dac_enabled);
        self.volume.save_state(w);
        self.length.save_state(w);
        w.u8(self.shift);
        w.bool(self.width_mode);
        w.u8(self.divisor);
        w.u16(self.lfsr);
        w.u32(self.period);
        w.u32(self.delay);
        w.i32(self.last_amp);
    }

    fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.enabled = r.bool()?;
        self.dac_enabled = r.bool()?;
        self.volume.load_state(r)?;
        self.length.load_state(r)?;
        self.shift = r.u8()?;
        self.width_mode = r.bool()?;
        self.divisor = r.u8()?;
        self.lfsr = r.u16()?;
        self.period = r.u32()?;
        self.delay = r.u32()?;
        self.last_amp = r.i32()?;
        Ok(())
    }
}

/// Read-back masks for `NR10`–`NR52` (`0xFF10..=0xFF26`): a CPU read returns
/// the last written byte OR'd with this mask — unused/write-only bits read as
/// ones. This is the exact table Blargg's `dmg_sound` `01-registers` asserts
/// (`tests/dmg_sound/source/01-registers.s`, `masks:`), and it matches Pan
/// Docs' Audio_Registers page. Note NR13/NR31/NR33/NR41 read as pure `0xFF`
/// (frequency and length loads are write-only), NRx2 reads back verbatim, and
/// NR52's entry covers only its always-set bits 4–6 (bit 7 + status bits are
/// live state, composed in `rb`).
const NR_READBACK_MASKS: [u8; 23] = [
    0x80, 0x3F, 0x00, 0xFF, 0xBF, // NR10-NR14 (0xFF15 unused ->
    0xFF, 0x3F, 0x00, 0xFF, 0xBF, //  slot 5 reads 0xFF), NR21-NR24
    0x7F, 0xFF, 0x9F, 0xFF, 0xBF, // NR30-NR34 (0xFF1F unused ->
    0xFF, 0xFF, 0x00, 0x00, 0xBF, //  slot 15), NR41-NR44
    0x00, 0x00, 0x70, // NR50, NR51, NR52
];

/// Which CGB CPU revision's APU length-counter quirks to model.
///
/// The Game Boy Color shipped several CPU steppings and the APU's
/// **extra length clocking** rule was changed part-way through the run. The
/// rule everyone means by "extra length clocking" is: writing NRx4 while the
/// frame sequencer's *next* step is one that does **not** clock length, with
/// length previously disabled and now enabled and non-zero, decrements the
/// length counter once.
///
/// On CPU CGB revisions 0 and B the "and now enabled" half of that condition
/// is simply absent — a write that leaves length *disabled* clocks it too.
/// That is a silicon bug: it breaks Prehistorik Man, and Nintendo fixed it in
/// CPU CGB C. Every later revision (C, D, E) and every DMG behaves the way
/// [`CgbRevision::CgbC`] models, which is why it is the default and the only
/// value a game should ever run under.
///
/// Source for both the rule and the revision split: the header comment of
/// SameSuite's `apu/channel_*/channel_*_extra_length_clocking-cgb0B.asm`
/// (test written by Matt Currie), which is quoting the gbdev community's
/// documentation of the behaviour.
///
/// This exists so the conformance runner can score the five SameSuite ROMs
/// whose *filenames* name a revision (`-cgb0B`, `-cgb0`, `-cgbB`); it is not a
/// console-selection feature. See `examples/samesuite.rs` and
/// `docs/conformance-notes.md`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CgbRevision {
    /// CPU CGB 0 — the launch stepping.
    Cgb0,
    /// CPU CGB B. As `Cgb0`, plus CH3's own extra-length-clock quirk: the
    /// clock that takes CH3's counter to zero does not disable the channel;
    /// the *next* one does. SameSuite ships `channel_3_extra_length_clocking`
    /// twice for exactly this difference.
    CgbB,
    /// CPU CGB C and later (C, D, E) — and the behaviour every DMG has. The
    /// default, and the only revision a real game should be run under.
    #[default]
    CgbC,
}

impl CgbRevision {
    /// `true` for the revisions where an NRx4 write clocks length whenever it
    /// was *previously* disabled, whatever the write leaves it as.
    fn extra_clock_ignores_new_enable(self) -> bool {
        matches!(self, CgbRevision::Cgb0 | CgbRevision::CgbB)
    }

    /// Parse the spelling SameSuite uses in its ROM filenames (`cgb0`,
    /// `cgbB`, `cgb0B`, `cgb0BC`, `cgbDE`), case-insensitively. A multi-letter
    /// tag means "these revisions agree here", so it maps to any one of them
    /// we model.
    ///
    /// `CgbC` lumps C, D and E together. An earlier note here warned that
    /// `channel_1_freq_change_timing` (`-cgb0BC` / `-cgbDE` / `-A`) is a
    /// *different* axis and would need `CgbC` split — **splitting it would win
    /// nothing, and that was measured.** We model CGB-E and pass `-cgbDE`; of
    /// the other two, `-A` is the Game Boy Advance (every CGB revision tag is
    /// spelled `cgb…`, and SameSuite's `apu/README.md` names the AGB line as
    /// the device family still to be verified), which is not a machine this
    /// emulator has at all; and `-cgb0BC` differs from `-cgbDE` in *when
    /// PCM12 reads back*, not in the frequency timer — the "registers PCM12
    /// and PCM34 report a glitched amplitude if read in the same M-cycle they
    /// change" quirk of revisions C and older, which that same README lists as
    /// not yet understood or documented by anyone.
    ///
    /// Unrecognised tags return `None`, i.e. the caller's default, so adding
    /// one is still additive if a future ROM does separate C from D/E.
    pub fn from_tag(tag: &str) -> Option<CgbRevision> {
        match tag.to_ascii_lowercase().as_str() {
            "cgb0" | "0" | "cgb0b" => Some(CgbRevision::Cgb0),
            "cgbb" | "b" => Some(CgbRevision::CgbB),
            "cgbc" | "c" | "cgbd" | "d" | "cgbe" | "e" | "cgb0bc" | "cgbde" => {
                Some(CgbRevision::CgbC)
            }
            _ => None,
        }
    }

    /// `$GB_CGB_REVISION`, for A/B-ing the quirk without a rebuild. Unset is
    /// the default; a typo **warns and is ignored** rather than silently
    /// running a game on buggy silicon — the same rule `$GB_MODEL` follows.
    fn from_env() -> CgbRevision {
        match crate::env_var("GB_CGB_REVISION") {
            None => CgbRevision::default(),
            Some(s) if s.is_empty() => CgbRevision::default(),
            Some(s) => CgbRevision::from_tag(&s).unwrap_or_else(|| {
                crate::host_warn!(
                    "warning: GB_CGB_REVISION={s:?} is not one of cgb0/cgbB/cgbC — ignoring"
                );
                CgbRevision::default()
            }),
        }
    }
}

/// How an NRx4 write's extra length clock behaves, for one channel on one
/// revision. Built by [`Sound::extra_length_rule`].
#[derive(Clone, Copy)]
struct ExtraLengthRule {
    /// Clock even when the write leaves length disabled (CPU CGB ≤ B).
    ignore_new_enable: bool,
    /// The clock that reaches zero does not disable the channel; the next one
    /// does (CH3 on CPU CGB B only).
    disable_one_clock_late: bool,
}

impl ExtraLengthRule {
    /// CPU CGB C and later, and every DMG.
    const MODERN: ExtraLengthRule = ExtraLengthRule {
        ignore_new_enable: false,
        disable_one_clock_late: false,
    };
}

/// Reusable scratch for one output block. Allocated once; `play_samples` runs
/// every 5.8 ms of emulated audio and must not allocate.
struct Mixdown {
    left: Vec<f32>,
    right: Vec<f32>,
    ch1: Vec<i16>,
    ch2: Vec<i16>,
    ch3: Vec<i16>,
    ch4: Vec<i16>,
}

impl Default for Mixdown {
    fn default() -> Self {
        Mixdown {
            left: vec![0.0; OUTPUT_SAMPLE_COUNT],
            right: vec![0.0; OUTPUT_SAMPLE_COUNT],
            ch1: vec![0; OUTPUT_SAMPLE_COUNT],
            ch2: vec![0; OUTPUT_SAMPLE_COUNT],
            ch3: vec![0; OUTPUT_SAMPLE_COUNT],
            ch4: vec![0; OUTPUT_SAMPLE_COUNT],
        }
    }
}

pub struct Sound {
    on: bool,
    channel1: SquareChannel,
    channel2: SquareChannel,
    channel3: WaveChannel,
    channel4: NoiseChannel,
    left_volume: u8,
    right_volume: u8,
    channel_to_left: u8,
    channel_to_right: u8,
    frame_seq_counter: u8,
    frame_seq_skip: bool,
    /// Raw last-written bytes for `0xFF10..=0xFF25`. Hardware read-back is
    /// `written | mask` (see [`NR_READBACK_MASKS`]) — the decoded channel
    /// state below is what *plays*, this shadow is what *reads*.
    regs: [u8; 22],
    /// DMG APU semantics (true for DMG carts — `GbMode::Classic` /
    /// `ColorAsClassic`): length counters survive power-off and NRx1 stays
    /// writable while off. CGB (false) clears lengths on power-off and
    /// ignores those writes. Blargg's `dmg_sound` asserts the DMG side.
    dmg_mode: bool,
    player: Option<Box<dyn AudioPlayer>>,
    time: u32,
    prev_time: u32,
    next_time: u32,
    output_period: u32,
    /// CGB double-speed mode: the CPU ticks twice as fast, the APU does not.
    double_speed: bool,
    /// Odd CPU T-cycle carried across a `do_cycle` when halving for double
    /// speed.
    cpu_frac: u32,
    /// Position of the APU clock on its free-running grid, in base T-cycles
    /// (`0..=15`). Bits 0-1 are the 1 MHz phase the square channels' trigger
    /// alignment keys off ([`SQUARE_TRIGGER_ALIGN`]); bit 2 is the extra
    /// half-tick the noise channel's divisor stage races against
    /// ([`NoiseChannel::trigger_divisor`]). Kept separately from `time`, which
    /// `do_output` rebases by `output_period` (24344 T at 44.1 kHz — not a
    /// multiple of 4, so `time` cannot carry the phase).
    grid_phase: u32,
    /// Which CGB CPU revision's APU quirks to model (see [`CgbRevision`]).
    /// Machine configuration, not cartridge state — deliberately **not**
    /// serialised, so a save state restores into whatever machine is running.
    cgb_revision: CgbRevision,
    /// This machine is a Game Boy Advance rather than a Game Boy Color. The
    /// only APU difference this emulator models is the width of the frequency
    /// timer's reload window ([`SQUARE_RELOAD_LATCH_AGB`]). Machine
    /// configuration like `cgb_revision`, and **not** serialised for the same
    /// reason: a save state restores into whatever machine is running.
    agb: bool,
    /// Reusable mixing scratch (see [`Mixdown`]).
    mixdown: Mixdown,
    /// Whether the *samples* are produced at all — see
    /// [`Sound::set_render_audio`]. Host configuration like `cgb_revision`
    /// and `agb`, and **not** serialised for the same reason.
    render_audio: bool,
    /// See [`Sound::set_state_frozen`]. Machine configuration, not state:
    /// never serialised.
    state_frozen: bool,
}

impl Default for Sound {
    fn default() -> Self {
        Self::new()
    }
}

impl Sound {
    pub fn new() -> Sound {
        let mut ch1_blip = BlipBuf::new(OUTPUT_SAMPLE_COUNT_U32 * 2);
        let mut ch2_blip = BlipBuf::new(OUTPUT_SAMPLE_COUNT_U32 * 2);
        let mut ch3_blip = BlipBuf::new(OUTPUT_SAMPLE_COUNT_U32 * 2);
        let mut ch4_blip = BlipBuf::new(OUTPUT_SAMPLE_COUNT_U32 * 2);
        ch1_blip.set_rates(CLOCKS_PER_SECOND as f64, SAMPLE_RATE as f64);
        ch2_blip.set_rates(CLOCKS_PER_SECOND as f64, SAMPLE_RATE as f64);
        ch3_blip.set_rates(CLOCKS_PER_SECOND as f64, SAMPLE_RATE as f64);
        ch4_blip.set_rates(CLOCKS_PER_SECOND as f64, SAMPLE_RATE as f64);

        let output_period = ((OUTPUT_SAMPLE_COUNT_U32 as u64 * CLOCKS_PER_SECOND as u64)
            / SAMPLE_RATE as u64) as u32;

        Sound {
            on: false,
            render_audio: true,
            state_frozen: false,
            channel1: SquareChannel::new(ch1_blip),
            channel2: SquareChannel::new(ch2_blip),
            channel3: WaveChannel::new(ch3_blip),
            channel4: NoiseChannel::new(ch4_blip),
            left_volume: 7,        // Max volume
            right_volume: 7,       // Max volume
            channel_to_left: 0xF,  // All channels to left (only lower 4 bits)
            channel_to_right: 0xF, // All channels to right (only lower 4 bits)
            frame_seq_counter: 0,
            frame_seq_skip: false,
            regs: [0; 22],
            dmg_mode: true,
            player: None,
            time: 0,
            prev_time: 0,
            next_time: CLOCKS_PER_FRAME,
            output_period,
            double_speed: false,
            cpu_frac: 0,
            grid_phase: 0,
            cgb_revision: CgbRevision::from_env(),
            agb: false,
            mixdown: Mixdown::default(),
        }
    }

    /// Whether the APU produces **samples**. `true` in every ordinary build.
    ///
    /// The exact sibling of [`crate::gpu::Gpu::set_render_pixels`], and it
    /// rests on the same argument: samples are emulator *output*. No address a
    /// cartridge can reach carries them, they are not in a save state (the
    /// `blip` buffers are explicitly runtime plumbing — see
    /// [`Sound::save_state`]), and the only consumer is the host
    /// [`AudioPlayer`]. A headless collector that installs
    /// [`SilentPlayer`] — which every scenario, longplay and swarm run does,
    /// because a machine with *no* APU is a machine a cartridge can tell apart
    /// — is paying for a band-limited resample it then throws away.
    ///
    /// **Guest-invisible by construction, and the construction is narrow on
    /// purpose.** Everything that decides what the guest can read still runs
    /// unchanged: the frame sequencer, the length counters, the sweep and
    /// envelope units, the duty and wave positions, the LFSR, `since_fetch`,
    /// the channel-enable flip-flops behind `NR52`, and `last_amp`. Every
    /// serialised field therefore holds the same value it would have held —
    /// so a save state taken with rendering off is byte-identical to one taken
    /// with it on. What is skipped is `blip_buf::add_delta`, `end_frame`, and
    /// with them the mixdown and the hand-off in
    /// [`Sound::play_samples`], which returns at its own sample-count guard.
    ///
    /// Safe to toggle at any time: with rendering off blip sits idle at offset
    /// zero, so turning it back on resumes into an empty resampler. The audio
    /// either side of the gap is correct; the gap itself is silence, which is
    /// the honest thing for output nobody asked for.
    pub fn set_render_audio(&mut self, on: bool) {
        self.render_audio = on;
        self.channel1.render = on;
        self.channel2.render = on;
        self.channel3.render = on;
        self.channel4.render = on;
    }

    /// Freeze the chip's state machines as well as its output — the
    /// `no-apu-state` speed flag. **Guest-visible and not in the speed preset**:
    /// `NR52`, the length counters, the sweep and envelope units and
    /// `PCM12`/`PCM34` all stop, so a game that waits on any of them hangs.
    /// See [`crate::speed::SpeedFlags::no_apu_state`].
    pub fn set_state_frozen(&mut self, on: bool) {
        self.state_frozen = on;
    }

    /// Whether the APU is producing samples — see [`Sound::set_render_audio`].
    pub fn render_audio(&self) -> bool {
        self.render_audio
    }

    pub fn set_player(&mut self, player: Box<dyn AudioPlayer>) {
        self.player = Some(player);
        let sample_rate = self.player.as_ref().unwrap().samples_rate();
        self.channel1
            .blip
            .set_rates(CLOCKS_PER_SECOND as f64, sample_rate as f64);
        self.channel2
            .blip
            .set_rates(CLOCKS_PER_SECOND as f64, sample_rate as f64);
        self.channel3
            .blip
            .set_rates(CLOCKS_PER_SECOND as f64, sample_rate as f64);
        self.channel4
            .blip
            .set_rates(CLOCKS_PER_SECOND as f64, sample_rate as f64);

        // Recalculate output period based on actual sample rate
        self.output_period = ((OUTPUT_SAMPLE_COUNT_U32 as u64 * CLOCKS_PER_SECOND as u64)
            / sample_rate as u64) as u32;
    }

    /// Serialise the logical APU state. The host `player` and the per-channel
    /// `blip` buffers are runtime plumbing, not machine state, and are left in
    /// place by a load. Because audio never feeds back into the CPU or PPU, the
    /// APU has no effect on frame-output determinism — but it is captured so
    /// sound continues coherently across a save/load.
    pub(crate) fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.bool(self.on);
        self.channel1.save_state(w);
        self.channel2.save_state(w);
        self.channel3.save_state(w);
        self.channel4.save_state(w);
        w.u8(self.left_volume);
        w.u8(self.right_volume);
        w.u8(self.channel_to_left);
        w.u8(self.channel_to_right);
        w.u8(self.frame_seq_counter);
        w.bool(self.frame_seq_skip);
        for b in self.regs {
            w.u8(b);
        }
        w.bool(self.dmg_mode);
        w.u32(self.time);
        w.u32(self.prev_time);
        w.u32(self.next_time);
        w.u32(self.output_period);
        w.bool(self.double_speed);
        w.u32(self.cpu_frac);
        w.u32(self.grid_phase);
    }

    pub(crate) fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.on = r.bool()?;
        self.channel1.load_state(r)?;
        self.channel2.load_state(r)?;
        self.channel3.load_state(r)?;
        self.channel4.load_state(r)?;
        self.left_volume = r.u8()?;
        self.right_volume = r.u8()?;
        self.channel_to_left = r.u8()?;
        self.channel_to_right = r.u8()?;
        self.frame_seq_counter = r.u8()?;
        self.frame_seq_skip = r.bool()?;
        for b in self.regs.iter_mut() {
            *b = r.u8()?;
        }
        self.dmg_mode = r.bool()?;
        self.time = r.u32()?;
        self.prev_time = r.u32()?;
        self.next_time = r.u32()?;
        self.output_period = r.u32()?;
        self.double_speed = r.bool()?;
        self.cpu_frac = r.u32()?;
        self.grid_phase = r.u32()?;
        Ok(())
    }

    /// Select DMG vs CGB APU semantics (see the `dmg_mode` field docs).
    /// Called by the MMU once the cartridge's mode is known.
    pub fn set_mode(&mut self, dmg: bool) {
        self.dmg_mode = dmg;
    }

    /// Model a specific CGB CPU revision's APU quirks — see [`CgbRevision`].
    /// Leave it alone for games: the default is the fixed silicon.
    pub fn set_cgb_revision(&mut self, rev: CgbRevision) {
        self.cgb_revision = rev;
    }

    /// Tell the APU this machine is a Game Boy Advance. Set from
    /// `MemoryManagementUnit::console`, so `GB_MODEL=agb` (or an explicit
    /// [`crate::mode::Model::Agb`]) is the whole of the user-facing knob —
    /// there is deliberately no second one.
    pub fn set_agb(&mut self, agb: bool) {
        self.agb = agb;
    }

    /// Width of the frequency timer's reload window on this console, in
    /// T-cycles — see [`SQUARE_RELOAD_LATCH`] / [`SQUARE_RELOAD_LATCH_AGB`].
    fn reload_latch(&self) -> u32 {
        if self.agb {
            SQUARE_RELOAD_LATCH_AGB
        } else {
            SQUARE_RELOAD_LATCH
        }
    }

    /// The extra-length-clocking rule in force for one channel. A DMG is
    /// never affected: the revision split is a CGB CPU stepping difference,
    /// so `dmg_mode` pins the modern rule whatever the knob says.
    fn extra_length_rule(&self, wave: bool) -> ExtraLengthRule {
        if self.dmg_mode {
            return ExtraLengthRule::MODERN;
        }
        ExtraLengthRule {
            ignore_new_enable: self.cgb_revision.extra_clock_ignores_new_enable(),
            disable_one_clock_late: wave && self.cgb_revision == CgbRevision::CgbB,
        }
    }

    /// Put the APU into the state the console's boot ROM leaves behind at
    /// `PC=$0100`, by **replaying the writes the boot ROM actually makes** —
    /// not by stuffing the register shadow with what a read comes back with.
    ///
    /// That distinction is the whole of this function. `NR11` reads `$BF` and
    /// `NR14` reads `$BF`, but those are `$80` and `$87` ORed with their
    /// write-only bits ([`NR_READBACK_MASKS`]). Writing the *read-back* values
    /// instead would load a length counter of 1 with the length counter
    /// **enabled**, so channel 1 would switch itself off within 4 ms and
    /// `NR52` would read `$F0` — which is precisely what Mooneye
    /// `acceptance/boot_hwio-dmgABCmgb` catches, since it reads `NR52` some
    /// 2.8 ms in and requires `$F1`.
    ///
    /// The one thing not reproduced is the *sound*: the boot ROM's second beep
    /// is still ringing at hand-off on real hardware, and its envelope volume
    /// is not readable from any register, so how far it has decayed cannot be
    /// derived. We never played that beep, so emitting its tail would be
    /// inventing audio the machine never produced — the envelope is left at
    /// zero, i.e. fully decayed. Every register, including the channel-enable
    /// flip-flop `NR52` reports, matches hardware.
    /// `sgb` selects the Super Game Boy's boot ROM, which sets channel 1 up in
    /// exactly the same way and then **does not trigger it** — its last audio
    /// write is `NR14 = $07`, not `$87`, so there is no start-up beep and the
    /// channel-enable flip-flop is still clear at hand-off. That single bit is
    /// the whole difference between `boot_hwio-dmgABCmgb` (`NR52 = $F1`) and
    /// `boot_hwio-S` (`NR52 = $F0`).
    pub fn set_post_boot_state(&mut self, sgb: bool) {
        self.wb(0xFF26, 0x80); // power on
        self.wb(0xFF11, 0x80); // CH1 duty 2, length load 0 (counter = 64)
        self.wb(0xFF12, 0xF3); // CH1 envelope: 15, decreasing, period 3
        self.wb(0xFF25, 0xF3); // NR51: all four left, CH1+CH2 right
        self.wb(0xFF24, 0x77); // NR50: both sides at volume 7
        self.wb(0xFF13, 0xC1); // CH1 frequency low
        self.wb(0xFF14, if sgb { 0x07 } else { 0x87 }); // trigger — except on an SGB
        self.channel1.volume.volume = 0;
    }

    /// NRx4 length-counter semantics, including the "extra length clocking"
    /// quirks (gg8 wiki "Gameboy sound hardware" → Obscure Behavior; Blargg
    /// dmg_sound 03-trigger). `first_half` = the frame sequencer's *next*
    /// step is one that does NOT clock length (odd step pending), i.e. we
    /// are in the first half of a 256 Hz length period:
    ///
    /// * Enabling a previously-disabled counter in the first half clocks it
    ///   once immediately; if that clock zeroes it and there is no trigger,
    ///   the channel is disabled on the spot.
    /// * A trigger reloads a zero counter with max — but with length enabled
    ///   in the first half it loads max-1 (the reload is clocked too).
    fn apply_nrx4_length(
        length: &mut LengthCounter,
        channel_enabled: &mut bool,
        first_half: bool,
        v: u8,
        rule: ExtraLengthRule,
    ) {
        let was_enabled = length.enabled;
        let enable = v & 0x40 == 0x40;
        let trigger = v & 0x80 == 0x80;
        length.enabled = enable;
        let extra_clock =
            first_half && !was_enabled && (enable || rule.ignore_new_enable);
        if extra_clock && length.value > 0 {
            length.value -= 1;
            if length.value == 0 && !trigger && !rule.disable_one_clock_late {
                *channel_enabled = false;
            }
        } else if extra_clock && !trigger && rule.disable_one_clock_late {
            // CPU CGB B, CH3 only: the clock that reached zero left the
            // channel playing, and this is the one that turns it off.
            *channel_enabled = false;
        }
        if trigger && length.value == 0 {
            length.value = if enable && first_half {
                length.max - 1
            } else {
                length.max
            };
        }
    }

    /// Forward the frontend's fast-forward hint to the player (see
    /// `AudioPlayer::set_fast_forward`). No-op without a player.
    pub fn set_fast_forward(&mut self, enabled: bool) {
        if let Some(ref mut p) = self.player {
            p.set_fast_forward(enabled);
        }
    }

    /// Process all emulated time that has already elapsed (frame-sequencer
    /// steps included) *before* a CPU register access lands. The APU is
    /// otherwise evaluated lazily in ~45 ms blocks (`do_output`), which is
    /// fine for the sample stream but makes NR52/length/sweep state stale by
    /// up to a block at read time — Blargg's dmg_sound waits a few length
    /// periods (3.9 ms each) and reads NR52, so lazy state fails every
    /// timing-sensitive subtest. Classic catch-up-on-access synchronisation;
    /// cost is bounded by APU-register access frequency, not bus traffic.
    pub fn catch_up(&mut self) {
        self.run();
    }

    /// Advance the APU by `cycles` **CPU** T-cycles.
    ///
    /// At double speed the CPU's tick is half a base clock tick, so the APU —
    /// which is wired to the unmodified 4.194304 MHz clock — advances half as
    /// far. `cpu_frac` carries the odd tick so an odd `cycles` never loses
    /// time (the APU must stay exactly `cycles/2` behind, or pitch drifts).
    /// How many **CPU** cycles this APU can be handed before [`Sound::do_cycle`]
    /// would do anything but add to a counter.
    ///
    /// `do_cycle` advances `time` and `grid_phase` and then, once every
    /// `output_period` (24344 T, about three times an emulated frame), runs the
    /// channels and hands a block to the player. It is called **once per CPU
    /// memory access** — 11,051 times a frame on Pokémon Blue's overworld — and
    /// 99.7 % of those calls only add to the counter.
    ///
    /// This lets the caller batch those: accumulate the cycles, and flush only
    /// when the accumulation reaches this budget or when something is about to
    /// *observe* the APU. That is exact rather than approximate — the flush
    /// lands on the same cycle `do_output` would have — which is why the answer
    /// is a budget rather than "flush at the end of the instruction". See
    /// [`MemoryManagementUnit::flush_apu`](crate::mmu::MemoryManagementUnit).
    ///
    /// `u32::MAX` when there is no player, because then `do_cycle` returns at
    /// its first line and the clock is not running at all.
    pub fn cycles_to_output(&self) -> u32 {
        if self.player.is_none() {
            return u32::MAX;
        }
        let need = self.output_period.saturating_sub(self.time).max(1);
        if self.double_speed {
            // `do_cycle` halves what it is handed, carrying the odd cycle in
            // `cpu_frac`: it advances by `(cpu_frac + cycles) >> 1`, so it takes
            // `2 * need - cpu_frac` CPU cycles to cover `need` APU ones.
            (need * 2).saturating_sub(self.cpu_frac).max(1)
        } else {
            need
        }
    }

    pub fn do_cycle(&mut self, cycles: u32) {
        if self.player.is_none() || self.state_frozen {
            return;
        }
        let cycles = if self.double_speed {
            let total = self.cpu_frac + cycles;
            self.cpu_frac = total & 1;
            total >> 1
        } else {
            cycles
        };

        // Time advances even while the APU is powered off (NR52 bit 7 = 0).
        // Real hardware keeps driving the output DACs — they just sit at
        // silence — so the sample *stream* must keep flowing at the full rate.
        // The old early-return froze sample production while off, which (a)
        // starves a real audio device (the host queue drains and underruns) and
        // (b) breaks the invariant that samples produced == cycles * rate /
        // 4194304, which the offline capture harness checks. Channels that are
        // disabled contribute no blip deltas, so an off APU synthesises exact
        // silence at zero extra cost.
        self.time += cycles;
        self.grid_phase = (self.grid_phase + cycles) & 15;

        if self.time >= self.output_period {
            self.do_output();
        }
    }

    fn do_output(&mut self) {
        self.run();

        // End frame for all channels. Skipped with rendering off: with no
        // deltas going in, ending a frame would still advance blip's own clock,
        // so `samples_avail` would climb until the buffer overflowed. Left
        // alone, blip sits idle at offset 0 and `play_samples` returns at its
        // own `sample_count < OUTPUT_SAMPLE_COUNT` guard.
        if self.render_audio {
            self.channel1.blip.end_frame(self.output_period);
            self.channel2.blip.end_frame(self.output_period);
            self.channel3.blip.end_frame(self.output_period);
            self.channel4.blip.end_frame(self.output_period);
        }

        // Adjust timing
        self.time -= self.output_period;
        self.next_time -= self.output_period;
        self.prev_time -= self.output_period;

        self.play_samples();
    }

    /// One DIV-APU event: the 512 Hz tick that drives length, sweep and
    /// envelope. Performs the action for the *current* step, then advances.
    fn frame_seq_step(&mut self) {
        if self.frame_seq_skip {
            self.frame_seq_skip = false;
            return;
        }
        if self.frame_seq_counter % 2 == 0 {
            // Length expiry disables the channel itself (NR52 status bit),
            // not just its output — see LengthCounter::step.
            if self.channel1.length.step() {
                self.channel1.enabled = false;
            }
            if self.channel2.length.step() {
                self.channel2.enabled = false;
            }
            if self.channel3.length.step() {
                self.channel3.enabled = false;
            }
            if self.channel4.length.step() {
                self.channel4.enabled = false;
            }
        }
        if self.frame_seq_counter % 4 == 2 {
            self.channel1.sweep_step();
        }
        // The envelope is driven at the full 512 Hz; only its period counter
        // is clocked at 64 Hz, and the step it asks for lands on the *next*
        // event — which is why the counter runs on step 6 while the volume
        // still changes on step 7, where it always did. Step 6 is also the
        // phase this frame sequencer's length (even steps) and sweep (step 2
        // of 4) already imply: all three are one step ahead of the event that
        // acts on them. See VolumeEnvelope::div_event.
        let sixty_four_hz = self.frame_seq_counter == 6;
        let (c1, c2, c4) = (
            self.channel1.enabled,
            self.channel2.enabled,
            self.channel4.enabled,
        );
        self.channel1.volume.div_event(sixty_four_hz, c1);
        self.channel2.volume.div_event(sixty_four_hz, c2);
        self.channel4.volume.div_event(sixty_four_hz, c4);

        self.frame_seq_counter = (self.frame_seq_counter + 1) % 8;
    }

    /// Are we in the *first half* of a 256 Hz length period — i.e. will the
    /// next DIV-APU event leave the length counters alone? That is what the
    /// NRx4 extra-length-clocking quirks key off (see `apply_nrx4_length`).
    ///
    /// A pending power-on skip counts as first half: the next event does
    /// nothing at all, so the next length clock is a whole period away, which
    /// is exactly the condition the quirk describes. SameSuite
    /// `div_write_trigger_10` measures this — it is the extra length clock the
    /// NR14 write performs itself that makes its channels expire two events
    /// earlier than `div_write_trigger`'s.
    fn first_half(&self) -> bool {
        self.frame_seq_skip || self.frame_seq_counter % 2 == 1
    }

    /// Is the DIV bit that clocks the DIV-APU currently **set**?
    ///
    /// The frame sequencer is not a free-running divider: it is clocked by the
    /// falling edge of a bit of the system counter that `DIV` exposes — bit 12
    /// (`DIV` bit 4) at single speed, bit 13 at double speed, so the event rate
    /// is 512 Hz either way (Pan Docs, "Audio Details" → DIV-APU). We keep the
    /// event *phase* in `next_time` rather than a counter copy: the counter is
    /// zero at `next_time - CLOCKS_PER_FRAME` and the bit is set over the
    /// second half of the period, i.e. within `CLOCKS_PER_FRAME / 2` of the
    /// next edge. `Sound::div_write` is what keeps that identity true.
    fn div_apu_bit_set(&self) -> bool {
        self.next_time.saturating_sub(self.time) <= CLOCKS_PER_FRAME / 2
    }

    /// The CPU wrote `DIV` (`$FF04`), resetting the system counter. If the
    /// DIV-APU bit was set the reset drops it — a falling edge — so a DIV-APU
    /// event fires *immediately*, and the next natural event is a full period
    /// away. SameSuite `div_write_trigger` measures exactly this: it triggers
    /// a length-enabled channel and then resets DIV while `DIV == $10`, once
    /// per requested event, and reads back NR52.
    ///
    /// The caller must have caught the APU up to the write's own cycle first.
    pub fn div_write(&mut self) {
        if self.div_apu_bit_set() {
            self.frame_seq_step();
        }
        self.next_time = self.time + CLOCKS_PER_FRAME;
    }

    /// CGB speed switch. The APU always runs off the unmodified 4.194304 MHz
    /// clock, so at double speed it must advance at *half* the CPU's tick rate
    /// (`do_cycle` is fed CPU T-cycles). The DIV-APU stays at 512 Hz because
    /// the divider bit moves up one place at the same time — both facts are
    /// captured by simply halving here and leaving `CLOCKS_PER_FRAME` alone.
    pub fn set_double_speed(&mut self, double: bool) {
        if double == self.double_speed {
            return;
        }
        self.catch_up();
        self.double_speed = double;
        self.cpu_frac = 0;
    }

    fn run(&mut self) {
        // Run frame sequencer
        while self.next_time <= self.time {
            // Run channels up to frame boundary
            self.channel1.run(self.prev_time, self.next_time);
            self.channel2.run(self.prev_time, self.next_time);
            self.channel3.run(self.prev_time, self.next_time);
            self.channel4.run(self.prev_time, self.next_time);

            self.prev_time = self.next_time;
            self.next_time += CLOCKS_PER_FRAME;
            self.frame_seq_step();
        }

        // Run remaining time
        if self.prev_time != self.time {
            self.channel1.run(self.prev_time, self.time);
            self.channel2.run(self.prev_time, self.time);
            self.channel3.run(self.prev_time, self.time);
            self.channel4.run(self.prev_time, self.time);
            self.prev_time = self.time;
        }
    }

    fn play_samples(&mut self) {
        // Check if any channel has samples available
        let sample_count = self
            .channel1
            .blip
            .samples_avail()
            .min(self.channel2.blip.samples_avail())
            .min(self.channel3.blip.samples_avail())
            .min(self.channel4.blip.samples_avail()) as usize;

        if sample_count < OUTPUT_SAMPLE_COUNT {
            return;
        }

        // Scratch buffers live on `self`: this runs once per 5.8 ms of emulated
        // audio, and six heap allocations at that rate is pure waste.
        let Mixdown {
            left: left_buf,
            right: right_buf,
            ch1: ch1_buf,
            ch2: ch2_buf,
            ch3: ch3_buf,
            ch4: ch4_buf,
        } = &mut self.mixdown;

        self.channel1.blip.read_samples(&mut ch1_buf[..], false);
        self.channel2.blip.read_samples(&mut ch2_buf[..], false);
        self.channel3.blip.read_samples(&mut ch3_buf[..], false);
        self.channel4.blip.read_samples(&mut ch4_buf[..], false);

        let left_vol = (self.left_volume as f32 / 7.0) * (1.0 / 15.0) * 0.25;
        let right_vol = (self.right_volume as f32 / 7.0) * (1.0 / 15.0) * 0.25;

        for i in 0..OUTPUT_SAMPLE_COUNT {
            let mut left = 0.0f32;
            let mut right = 0.0f32;

            // Channel 1
            if self.channel_to_left & 0x01 != 0 {
                left += ch1_buf[i] as f32 * left_vol;
            }
            if self.channel_to_right & 0x01 != 0 {
                right += ch1_buf[i] as f32 * right_vol;
            }

            // Channel 2
            if self.channel_to_left & 0x02 != 0 {
                left += ch2_buf[i] as f32 * left_vol;
            }
            if self.channel_to_right & 0x02 != 0 {
                right += ch2_buf[i] as f32 * right_vol;
            }

            // Channel 3 (Wave)
            if self.channel_to_left & 0x04 != 0 {
                left += (ch3_buf[i] as f32 / 4.0) * left_vol; // Wave channel is 4x amplitude
            }
            if self.channel_to_right & 0x04 != 0 {
                right += (ch3_buf[i] as f32 / 4.0) * right_vol; // Wave channel is 4x amplitude
            }

            // Channel 4 (Noise)
            if self.channel_to_left & 0x08 != 0 {
                left += ch4_buf[i] as f32 * left_vol;
            }
            if self.channel_to_right & 0x08 != 0 {
                right += ch4_buf[i] as f32 * right_vol;
            }

            left_buf[i] = left;
            right_buf[i] = right;
        }

        if let Some(ref mut player) = self.player {
            player.play(&self.mixdown.left, &self.mixdown.right);
        }
    }

    pub fn rb(&self, a: u16) -> u8 {
        match a {
            // NR10..NR51: hardware reads back the raw stored byte OR the
            // register's always-set mask — write-only bits (frequency loads,
            // length loads) read as 1s. One rule for all 22 registers; the
            // per-register truth lives in NR_READBACK_MASKS. This is exactly
            // what Blargg's dmg_sound 01-registers asserts, value-by-value.
            0xFF10..=0xFF25 => {
                let i = (a - 0xFF10) as usize;
                self.regs[i] | NR_READBACK_MASKS[i]
            }
            0xFF26 => {
                // Status bits report the channel-enable flip-flops. Length
                // expiry and DAC-off clear them; nothing but a trigger (with
                // DAC on) sets them.
                (if self.on { 0x80 } else { 0 })
                    | 0x70
                    | if self.channel1.enabled { 0x01 } else { 0 }
                    | if self.channel2.enabled { 0x02 } else { 0 }
                    | if self.channel3.enabled { 0x04 } else { 0 }
                    | if self.channel4.enabled { 0x08 } else { 0 }
            }
            // Wave RAM. While CH3 is enabled every address maps to the byte
            // the channel is currently reading — but on DMG only during the
            // narrow window around the channel's own fetch; outside it reads
            // return $FF (Pan Docs "Audio Details"; dmg_sound 09).
            0xFF30..=0xFF3F => {
                if !self.channel3.enabled {
                    self.channel3.waveram[a as usize - 0xFF30]
                } else if self.dmg_mode {
                    match self.channel3.dmg_access_byte() {
                        Some(idx) => self.channel3.waveram[idx],
                        None => 0xFF,
                    }
                } else {
                    // CGB: always readable while playing, at the current byte.
                    self.channel3.waveram[self.channel3.wave_pos as usize / 2]
                }
            }
            // PCM12 / PCM34 (CGB only, undocumented but real): the live 4-bit
            // digital output of each channel — CH1/CH3 in the low nibble,
            // CH2/CH4 in the high nibble. These are read-only and are what
            // every SameSuite APU test uses to observe the APU sample by
            // sample (Pan Docs, "Audio Registers" → PCM12/PCM34).
            0xFF76 => {
                self.channel1.digital_output() | (self.channel2.digital_output() << 4)
            }
            0xFF77 => {
                self.channel3.digital_output() | (self.channel4.digital_output() << 4)
            }
            _ => 0xFF,
        }
    }

    pub fn wb(&mut self, a: u16, v: u8) {
        if !self.on && a != 0xFF26 {
            match a {
                // Wave RAM is unaffected by APU power: always writable.
                0xFF30..=0xFF3F => {
                    if !self.channel3.enabled {
                        self.channel3.waveram[a as usize - 0xFF30] = v;
                    }
                }
                // DMG only: length counters live outside the powered domain —
                // NRx1 length loads still land while the APU is off (the
                // readable duty bits do NOT: the register shadow stays 0, and
                // NRx1 length bits are write-only anyway, so power-off
                // read-back stays all-clear as 01-registers requires).
                0xFF11 if self.dmg_mode => self.channel1.length.wb(a, v),
                0xFF16 if self.dmg_mode => self.channel2.length.wb(a, v),
                0xFF1B if self.dmg_mode => self.channel3.length.wb(a, v),
                0xFF20 if self.dmg_mode => self.channel4.length.wb(a, v),
                _ => {}
            }
            return;
        }

        // Shadow every NR10..NR51 write so read-back returns the raw byte
        // (see rb / NR_READBACK_MASKS). The decode below is what plays.
        if let 0xFF10..=0xFF25 = a {
            self.regs[(a - 0xFF10) as usize] = v;
        }

        match a {
            0xFF10 => {
                let new_decrease = v & 0x08 == 0x08;
                // Sweep negate quirk: leaving negate mode after at least one
                // negate-mode calculation since the last trigger disables the
                // channel immediately (Pan Docs; dmg_sound 05-sweep details).
                if self.channel1.sweep_decrease
                    && !new_decrease
                    && self.channel1.sweep_negate_used
                {
                    self.channel1.enabled = false;
                }
                self.channel1.sweep_period = (v >> 4) & 0x7;
                self.channel1.sweep_decrease = new_decrease;
                self.channel1.sweep_shift = v & 0x7;
            }
            0xFF11 => {
                self.channel1.duty = v >> 6;
                self.channel1.length.wb(a, v);
            }
            0xFF12 => {
                let live = self.channel1.enabled;
                self.channel1.volume.write_nrx2(v, live);
                self.channel1.dac_enabled = v & 0xF8 != 0;
                self.channel1.enabled =
                    self.channel1.enabled && self.channel1.dac_enabled;
            }
            0xFF13 => {
                let f = (self.channel1.freq & 0x700) | (v as u16);
                let latch = self.reload_latch();
                self.channel1.write_freq(f, latch);
            }
            0xFF14 => {
                let f = (self.channel1.freq & 0xFF) | (((v & 0x7) as u16) << 8);
                let latch = self.reload_latch();
                self.channel1.write_freq(f, latch);
                let fh = self.first_half();
                let rule = self.extra_length_rule(false);
                Self::apply_nrx4_length(
                    &mut self.channel1.length,
                    &mut self.channel1.enabled,
                    fh,
                    v,
                    rule,
                );
                self.channel1.volume.wb(a, v);
                if v & 0x80 == 0x80 {
                    self.channel1.trigger(self.grid_phase);
                    // A trigger cannot enable a channel whose DAC is off
                    // (Pan Docs, Audio_details "Triggering").
                    self.channel1.enabled &= self.channel1.dac_enabled;
                }
            }
            0xFF16 => {
                self.channel2.duty = v >> 6;
                self.channel2.length.wb(a, v);
            }
            0xFF17 => {
                let live = self.channel2.enabled;
                self.channel2.volume.write_nrx2(v, live);
                self.channel2.dac_enabled = v & 0xF8 != 0;
                self.channel2.enabled =
                    self.channel2.enabled && self.channel2.dac_enabled;
            }
            0xFF18 => {
                let f = (self.channel2.freq & 0x700) | (v as u16);
                let latch = self.reload_latch();
                self.channel2.write_freq(f, latch);
            }
            0xFF19 => {
                let f = (self.channel2.freq & 0xFF) | (((v & 0x7) as u16) << 8);
                let latch = self.reload_latch();
                self.channel2.write_freq(f, latch);
                let fh = self.first_half();
                let rule = self.extra_length_rule(false);
                Self::apply_nrx4_length(
                    &mut self.channel2.length,
                    &mut self.channel2.enabled,
                    fh,
                    v,
                    rule,
                );
                self.channel2.volume.wb(a, v);
                if v & 0x80 == 0x80 {
                    self.channel2.trigger(self.grid_phase);
                    self.channel2.enabled &= self.channel2.dac_enabled;
                }
            }
            // Wave channel
            0xFF1A => {
                self.channel3.dac_enabled = v & 0x80 == 0x80;
                // DAC off silences AND disables the channel immediately.
                self.channel3.enabled &= self.channel3.dac_enabled;
            }
            0xFF1B => self.channel3.length.wb(a, v),
            0xFF1C => self.channel3.volume_shift = (v >> 5) & 0x3,
            0xFF1D => {
                self.channel3.freq = (self.channel3.freq & 0x700) | (v as u16);
                self.channel3.calculate_period();
            }
            0xFF1E => {
                self.channel3.freq =
                    (self.channel3.freq & 0xFF) | (((v & 0x7) as u16) << 8);
                self.channel3.calculate_period();
                let fh = self.first_half();
                let rule = self.extra_length_rule(true);
                Self::apply_nrx4_length(
                    &mut self.channel3.length,
                    &mut self.channel3.enabled,
                    fh,
                    v,
                    rule,
                );
                if v & 0x80 == 0x80 {
                    // DMG wave-RAM corruption: retriggering CH3 while it is
                    // enabled and about to fetch a byte on this very cycle
                    // rewrites the first bytes of wave RAM. If the imminent
                    // fetch targets one of the first four bytes, only the
                    // first byte is overwritten with it; otherwise the
                    // aligned four-byte block containing it is copied over
                    // bytes 0-3 (Pan Docs "Audio Details" → triggering while
                    // reading; dmg_sound 10).
                    if self.dmg_mode && self.channel3.enabled && self.channel3.delay == 0
                    {
                        let idx = (self.channel3.wave_pos as usize + 1) & 31;
                        let byte = idx / 2;
                        if byte < 4 {
                            self.channel3.waveram[0] = self.channel3.waveram[byte];
                        } else {
                            let src = byte & !3;
                            self.channel3.waveram.copy_within(src..src + 4, 0);
                        }
                    }
                    self.channel3.trigger();
                    self.channel3.enabled &= self.channel3.dac_enabled;
                }
            }
            // Noise channel
            0xFF20 => self.channel4.length.wb(a, v),
            0xFF21 => {
                let live = self.channel4.enabled;
                self.channel4.volume.write_nrx2(v, live);
                self.channel4.dac_enabled = v & 0xF8 != 0;
                self.channel4.enabled =
                    self.channel4.enabled && self.channel4.dac_enabled;
            }
            0xFF22 => {
                self.channel4.shift = v >> 4;
                self.channel4.width_mode = v & 0x08 == 0x08;
                self.channel4.divisor = v & 0x07;
                self.channel4.calculate_period();
            }
            0xFF23 => {
                let fh = self.first_half();
                let rule = self.extra_length_rule(false);
                Self::apply_nrx4_length(
                    &mut self.channel4.length,
                    &mut self.channel4.enabled,
                    fh,
                    v,
                    rule,
                );
                self.channel4.volume.wb(a, v);
                if v & 0x80 == 0x80 {
                    self.channel4.trigger(self.grid_phase);
                    self.channel4.enabled &= self.channel4.dac_enabled;
                }
            }
            // Wave RAM. Same access rules as reads (see rb): while CH3 plays,
            // DMG writes land on the channel's current byte only inside the
            // fetch window and are dropped outside it; CGB writes always land
            // on the current byte (dmg_sound 12).
            0xFF30..=0xFF3F => {
                if !self.channel3.enabled {
                    self.channel3.waveram[a as usize - 0xFF30] = v;
                } else if self.dmg_mode {
                    if let Some(idx) = self.channel3.dmg_access_byte() {
                        self.channel3.waveram[idx] = v;
                    }
                } else {
                    self.channel3.waveram[self.channel3.wave_pos as usize / 2] = v;
                }
            }
            0xFF24 => {
                self.left_volume = (v >> 4) & 0x7;
                self.right_volume = v & 0x7;
            }
            0xFF25 => {
                self.channel_to_left = v >> 4;
                self.channel_to_right = v & 0xF;
            }
            0xFF26 => {
                let will_be_on = v & 0x80 == 0x80;
                if self.on && !will_be_on {
                    // Power-off zeroes NR10-NR51 (Pan Docs). The clear writes
                    // must land while `self.on` is still true: this function
                    // ignores every register but NR52 when the APU is off, so
                    // flipping `on` first would turn the loop into a no-op
                    // (the old bug — registers and channel-status bits
                    // survived a power cycle).
                    //
                    // DMG: the length *counters* live outside the powered
                    // domain and survive a power cycle (gg8 wiki "Gameboy
                    // sound hardware", Obscure Behavior; Blargg dmg_sound
                    // 08-len ctr during power) — the wb(_, 0) sweep below
                    // would reload them via the NRx1 handlers, so save and
                    // restore around it.
                    let saved_lengths = (
                        self.channel1.length.value,
                        self.channel2.length.value,
                        self.channel3.length.value,
                        self.channel4.length.value,
                    );
                    for i in 0xFF10..=0xFF25 {
                        self.wb(i, 0);
                    }
                    if self.dmg_mode {
                        self.channel1.length.value = saved_lengths.0;
                        self.channel2.length.value = saved_lengths.1;
                        self.channel3.length.value = saved_lengths.2;
                        self.channel4.length.value = saved_lengths.3;
                    }
                    self.channel1.enabled = false;
                    self.channel2.enabled = false;
                    self.channel3.enabled = false;
                    self.channel4.enabled = false;
                    // Powering off resets the square duty counters (gg8 wiki,
                    // "Gameboy sound hardware" → Power Control). Every
                    // SameSuite APU test power-cycles the APU before each
                    // subtest precisely to get a known duty position, so this
                    // is the zero the whole suite measures from.
                    self.channel1.duty_pos = 0;
                    self.channel1.duty_out = false;
                    self.channel2.duty_pos = 0;
                    self.channel2.duty_out = false;
                    // CH3's sample buffer is part of the powered domain, so it
                    // empties too. A trigger deliberately does *not* refill it
                    // (Pan Docs, "Audio Details"), so without this a channel
                    // re-triggered after a power cycle would report the sample
                    // it was playing before the power-off for a whole period —
                    // SameSuite `channel_3_first_sample` reads exactly that
                    // stale nibble.
                    self.channel3.sample_latch = 0;
                }
                if !self.on && will_be_on {
                    // Powering on resets the frame sequencer so its NEXT step
                    // is 0 (length-clocking) — but the underlying 8192-cycle
                    // divider is DIV-driven and keeps running, which is why
                    // `next_time` is deliberately left alone (dmg_sound
                    // 07-len sweep period sync: "Powering up APU MODs next
                    // frame time with 8192" / "resets 128 Hz sweep divider").
                    //
                    // …unless the DIV-APU bit is already SET as the APU comes
                    // up, in which case the first event is skipped and the
                    // sequencer effectively starts at step 1. SameSuite
                    // `div_write_trigger_10` states it in those words and
                    // measures it: it waits for `DIV == $10` before powering
                    // the APU on, and the observable difference is that the
                    // next NRx4 length-enable write now lands in the "first
                    // half" of a length period and so clocks length once
                    // itself (see apply_nrx4_length).
                    self.frame_seq_counter = 0;
                    self.frame_seq_skip = self.div_apu_bit_set();
                    // The APU's own clock divider is held in reset while it is
                    // powered down, so the 1 MHz grid the channel frequency
                    // timers count on restarts here. That is what makes
                    // SameSuite's measurements repeatable at all: every APU
                    // subtest power-cycles the APU first, and the alignment
                    // its `channel_N_align` tests measure is relative to *this*
                    // instant, not to boot (see SQUARE_TRIGGER_ALIGN).
                    self.grid_phase = 0;
                }
                self.on = will_be_on;
            }
            _ => {}
        }
    }
}

/// Bounded producer/consumer hand-off between the emulator (which pushes
/// 2000-sample blocks from `Sound::play_samples`) and the audio callback
/// (which drains at exactly the sound card's rate). Device-free so the policy
/// is testable without host audio — see `queue_tests` below and docs/audio.md.
///
/// Policy, chosen from the measured failure mode (see docs/audio.md):
///
/// * **Prime before playback.** The callback emits hold samples until the
///   queue first reaches `start_fill`. Starting to drain a nearly-empty queue
///   guarantees an immediate underrun.
/// * **Hold-and-fade on underrun, never a step to silence.** An empty queue
///   repeats the last sample, decaying it toward zero (~1 ms time constant per
///   1000 samples at 44.1 kHz). A hard step to 0.0 is itself a wideband click —
///   SDL-style "play silence on underrun" is the *baseline*, not the best
///   (https://wiki.libsdl.org/SDL2/SDL_QueueAudio). After an underrun the
///   queue re-primes to `start_fill` before draining again, so one hiccup
///   costs one smooth gap instead of a burst of 45 ms on/off "blocky" cycles.
/// * **High watermark, and the producer never waits at it.** Above
///   `high_water()` the emulator is ahead of the sound card, and the oldest
///   samples are discarded down to `start_fill()` ([`SampleQueue::bound_latency`]).
///   The obvious alternative — *wait* for the callback to drain, making the
///   audio crystal the master clock — is **wrong here and was a shipped bug**:
///   [`hand_off`] is called from `Sound::play_samples`, which runs inside
///   `Cpu::do_cycle`, so the wait is charged to the emulated frame. Measured on
///   the maintainer's session: `emulate_ms` 17.93 against a 16.743 ms budget,
///   97.6 % of it not executing, the frontend permanently in catch-up, and the
///   display down to 6 fps. The frontends already pace to 59.7275 Hz with a
///   deadline accumulator; a second clock inside the emulator can only fight it.
/// * **Instrumentation.** Underruns, held/dropped counts and depth min/max are
///   counted so buffer sizing is measured, not argued. `GB_AUDIO_STATS=1`
///   logs a line every ~5 s to stderr.
// Also compiled for `wasm32` (whatever the `audio` feature says — that feature
// means *cpal*, which wasm never has): `web_audio` hands samples over through
// exactly this queue, so the browser and the sound card share one hand-off
// policy rather than growing two.
#[cfg(any(feature = "audio", target_arch = "wasm32", test))]
pub mod sample_queue {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// The producer half of the hand-off, whole. Extracted from
    /// `cpal_audio::CpalPlayer::play` so the one property that matters can be
    /// tested with no audio device at all: **this must return in microseconds
    /// however full the queue is, because it runs inside an emulated frame**.
    /// `testharness/audioperf.rs` is the gate.
    pub fn hand_off(queue: &Mutex<SampleQueue>, left: &[f32], right: &[f32]) {
        let Ok(mut q) = queue.lock() else {
            return;
        };
        q.push(left, right);
        q.bound_latency();
    }

    /// Producer block size: `Sound::play_samples` always hands over exactly
    /// this many samples, ~5.8 ms at 44.1 kHz.
    pub const BLOCK: usize = super::OUTPUT_SAMPLE_COUNT;

    /// The callback drains only once this many samples are queued: the jitter
    /// cushion, and — because the producer waits at [`HIGH_WATER`] — the lower
    /// end of the latency the player actually feels.
    ///
    /// **Sized against the real symptom, measured both ways.** The previous
    /// values (7000 / 12000 samples, when a block was 45 ms) measured zero
    /// underruns but parked end-to-end latency at ~300 ms, which the player
    /// feels as sound arriving after every button press. Cushioning is not free:
    /// every queued sample is delay.
    ///
    /// What the cushion has to cover, at 44.1 kHz:
    ///   * one frame of production burstiness — a frame-paced frontend emits a
    ///     whole frame's audio at once, 70224 cycles = **16.7 ms**;
    ///   * one device callback — measured at 118 frames = **2.7 ms** with the
    ///     buffer pinned (it was 1882 frames = 42.7 ms when left to the host);
    ///   * scheduler jitter on top.
    ///
    /// The guarantee chosen: **the frontend may drop one whole frame without an
    /// audible glitch.** Note that a dropped frame costs more than a frame of
    /// audio: the queue drains continuously, so the production *gap* is the
    /// missed frame plus the normal 16.7 ms period. One frame + 8 ms of
    /// measured scheduler jitter is ~41 ms, so the cushion is 1792 samples =
    /// **40.6 ms**. That is a principled promise rather than a fudge, and the
    /// simulation in `queue_tests` holds it to exactly that promise (it also
    /// shows a 20 ms stall still clears with 17 ms of depth to spare).
    ///
    /// A two-frame guarantee would need ~58 ms of cushion and put end-to-end
    /// latency back around 70 ms — perceptible again. Given the *measured*
    /// stall rate (4 late frames in 5375, p99.9 = 0.0 ms), one frame is the
    /// right side of that trade.
    ///
    /// Override with `GB_AUDIO_START_FILL` to re-run the sweep in
    /// `examples/audio_latency.rs`; `docs/audio.md` has the measured results.
    pub fn start_fill() -> usize {
        env_samples("GB_AUDIO_START_FILL", 7 * BLOCK)
    }

    /// Producer waits above this. Because the producer *blocks* here rather
    /// than being nudged, this — not [`start_fill`] — is what sets steady-state
    /// latency whenever the frame timer runs even slightly fast, which it
    /// always does. That is why it was the dominant term in the measured
    /// 300 ms: at 12000 samples the queue simply parked at 272 ms.
    ///
    /// 2816 samples = **63.8 ms** leaves the queue room to breathe above the
    /// cushion without itself becoming the latency.
    pub fn high_water() -> usize {
        env_samples("GB_AUDIO_HIGH_WATER", 11 * BLOCK)
    }

    fn env_samples(key: &str, default: usize) -> usize {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n >= BLOCK && *n <= 64 * BLOCK)
            .unwrap_or(default)
    }

    #[derive(Default, Clone, Copy)]
    pub struct QueueStats {
        /// Times the consumer ran dry after playback had started.
        pub underruns: u64,
        /// Samples substituted with the decaying hold value (priming included).
        pub held: u64,
        /// Samples discarded at the high watermark **after playback started**.
        /// Zero in healthy operation — this is the number the soak test gates
        /// on, because every one of them is audio a listener would have heard.
        pub dropped: u64,
        /// Samples discarded at the high watermark *before the device consumed
        /// its first sample*. Counted apart from [`dropped`] because nothing
        /// has been played yet: opening a cpal stream takes a couple of hundred
        /// milliseconds, during which the emulator is already synthesising, and
        /// discarding the oldest of that backlog is what makes playback start
        /// from *current* audio instead of a fifth of a second behind. Expect a
        /// few thousand here on every run; it is a startup cost, not a glitch.
        pub dropped_before_playback: u64,
        /// Queue depth extremes observed at callback entry.
        pub depth_min: usize,
        pub depth_max: usize,
        pub callbacks: u64,
        pub pushed: u64,
        pub consumed: u64,
    }

    pub struct SampleQueue {
        queue: VecDeque<(f32, f32)>,
        hold: (f32, f32),
        priming: bool,
        start_fill: usize,
        high_water: usize,
        pub stats: QueueStats,
    }

    impl SampleQueue {
        pub fn new(start_fill: usize, high_water: usize) -> Self {
            assert!(start_fill <= high_water);
            SampleQueue {
                queue: VecDeque::new(),
                hold: (0.0, 0.0),
                priming: true,
                start_fill,
                high_water,
                stats: QueueStats {
                    depth_min: usize::MAX,
                    ..Default::default()
                },
            }
        }

        pub fn push(&mut self, left: &[f32], right: &[f32]) {
            for i in 0..left.len().min(right.len()) {
                self.queue.push_back((left[i], right[i]));
            }
            self.stats.pushed += left.len().min(right.len()) as u64;
        }

        pub fn len(&self) -> usize {
            self.queue.len()
        }

        pub fn is_empty(&self) -> bool {
            self.queue.is_empty()
        }

        /// How far above the high watermark the queue is (0 = producer may
        /// continue immediately).
        pub fn over_high_water(&self) -> usize {
            self.queue.len().saturating_sub(self.high_water)
        }

        /// Bound latency by discarding the oldest samples. This is the *only*
        /// back-pressure the producer applies — see
        /// `cpal_audio::CpalPlayer::play` for why it may not wait instead.
        ///
        /// **The ceiling and the target are deliberately different levels.**
        /// [`high_water`] is where clipping *starts*; the queue is drained back
        /// to [`start_fill`], not to the ceiling. Draining to the ceiling
        /// leaves zero headroom, so the very next production burst clips again
        /// and every frame sheds a handful of samples for ever (measured: 668
        /// samples over a 120 s soak, with the consumer perfectly healthy).
        /// Draining to the cushion level restores the full `high_water -
        /// start_fill` band, which is what absorbs one frame's burst plus
        /// scheduler jitter.
        ///
        /// It is also what sets steady-state latency now that nothing blocks:
        /// opening the device always overfills the queue (see
        /// [`QueueStats::dropped_before_playback`]), so wherever this clip
        /// leaves the queue is where it stays. That is `start_fill` — the
        /// cushion the buffer sizing was actually reasoned about.
        pub fn bound_latency(&mut self) {
            if self.over_high_water() == 0 {
                return;
            }
            let n = self.queue.len() - self.start_fill;
            self.queue.drain(0..n);
            if self.stats.consumed == 0 {
                self.stats.dropped_before_playback += n as u64;
            } else {
                self.stats.dropped += n as u64;
            }
        }

        /// Record callback entry for depth instrumentation.
        pub fn note_callback(&mut self) {
            let d = self.queue.len();
            self.stats.depth_min = self.stats.depth_min.min(d);
            self.stats.depth_max = self.stats.depth_max.max(d);
            self.stats.callbacks += 1;
        }

        /// Produce the next output sample. Never blocks; substitutes a
        /// decaying hold sample while priming or after running dry.
        pub fn next_sample(&mut self) -> (f32, f32) {
            if self.priming {
                if self.queue.len() >= self.start_fill {
                    self.priming = false;
                } else {
                    return self.held_sample();
                }
            }
            match self.queue.pop_front() {
                Some(s) => {
                    self.hold = s;
                    self.stats.consumed += 1;
                    s
                }
                None => {
                    // Ran dry mid-playback: count it once, go back to priming
                    // so we rebuild a full cushion before draining again.
                    self.stats.underruns += 1;
                    self.priming = true;
                    self.held_sample()
                }
            }
        }

        fn held_sample(&mut self) -> (f32, f32) {
            self.stats.held += 1;
            let out = self.hold;
            // Fade toward zero rather than stepping: ~0.999^44100 per second
            // decays fully in well under a second, with no wideband edge.
            self.hold = (self.hold.0 * 0.999, self.hold.1 * 0.999);
            out
        }

        /// Reset depth extremes after a stats report.
        pub fn reset_depth_window(&mut self) {
            self.stats.depth_min = self.queue.len();
            self.stats.depth_max = self.queue.len();
        }

        /// Take exactly `n` samples, or nothing at all.
        ///
        /// The counterpart of [`Self::next_sample`] for a consumer that
        /// *schedules* audio rather than being pulled at the device rate — the
        /// browser's Web Audio path (see [`super::web_audio`]). Such a consumer
        /// must never be handed a held/faded sample: it places what it takes on
        /// a timeline, so a substituted sample would stretch that timeline by a
        /// sample's worth of time and the clock would drift for the rest of the
        /// run. "Not enough yet" is therefore `false` and no samples moved, and
        /// the scheduler simply lets its lead shrink until the producer catches
        /// up.
        pub fn drain_block(
            &mut self,
            n: usize,
            left: &mut Vec<f32>,
            right: &mut Vec<f32>,
        ) -> bool {
            if self.queue.len() < n {
                return false;
            }
            left.clear();
            right.clear();
            for (l, r) in self.queue.drain(0..n) {
                left.push(l);
                right.push(r);
                self.hold = (l, r);
            }
            self.priming = false;
            self.stats.consumed += n as u64;
            true
        }
    }
}

/// Playing the APU's output in a browser — the `wasm32` half of
/// [`AudioPlayer`], and the counterpart of [`cpal_audio`].
///
/// # Why the split is where it is
///
/// A Web Audio `AudioContext` is a JavaScript object: it is not `Send`, and
/// [`AudioPlayer`] is (the FFI build parks a whole `Gameboy` in a
/// `static Mutex`). It also cannot be touched from anywhere but the page's main
/// thread. So **no JS object goes anywhere near the APU**. What lives here is:
///
/// * [`WebAudioPlayer`] — the producer half. It is what
///   [`crate::gameboy::Gameboy::enable_audio`] is given, it holds nothing but a
///   [`sample_queue::SampleQueue`], and [`AudioPlayer::play`] is the same
///   non-blocking hand-off cpal uses (push, then bound latency by *discarding*
///   the oldest). **It must never wait** — `play` is called from inside
///   `Cpu::do_cycle`, so anything it does is charged to the emulated frame
///   (`docs/audio.md` §4c).
/// * [`Scheduler`] — the consumer half's *decisions*, with no `web-sys` in
///   sight, so the anti-glitch logic is unit-testable under a plain
///   `cargo test` on the host instead of only in a browser.
///
/// The twenty lines that actually call `AudioContext` live in the frontend
/// (`crate::screen::web`), where every other host assumption already lives.
///
/// # Why blocks are *scheduled* rather than pulled
///
/// The browser offers no equivalent of a cpal callback that we may use here:
/// an `AudioWorklet` runs on the audio thread and would need either a
/// `SharedArrayBuffer` (which needs COOP/COEP headers a static GitHub Pages
/// deployment cannot set) or a per-block `postMessage`, and `ScriptProcessorNode`
/// is deprecated and runs on the main thread anyway. What is left, and what is
/// the standard technique, is to hand the context short `AudioBuffer`s with an
/// explicit start time and keep a scheduling cursor a fixed distance ahead of
/// `AudioContext.currentTime`.
///
/// Two properties make that not crackle, and both are this type's job:
///
/// 1. **Never schedule in the past.** `start(when)` with `when` already gone
///    plays *immediately*, so a late block overlaps the one before it — a
///    click, not a gap. The cursor is therefore reset (and an underrun
///    counted) the moment it is not strictly ahead of `now`.
/// 2. **Never substitute samples to fill a block.** The cursor advances by
///    exactly `block / sample_rate` per block, so it *is* the output clock;
///    inserting a padded sample would slide it permanently. A short queue means
///    "schedule nothing this tick", not "schedule something else".
#[cfg(any(target_arch = "wasm32", test))]
pub mod web_audio {
    use super::sample_queue::{self, SampleQueue};
    use super::AudioPlayer;
    use std::sync::{Arc, Mutex};

    /// Samples per scheduled `AudioBuffer`: 512 = **11.6 ms** at 44.1 kHz.
    ///
    /// Small enough that the lead is quantised finely (the cursor overshoots
    /// its target by at most one block) and large enough that the page creates
    /// ~86 `AudioBuffer`/`AudioBufferSourceNode` pairs a second rather than the
    /// ~172 a 256-sample producer block would cost.
    pub const SCHEDULE_BLOCK: usize = 512;

    /// How far ahead of `AudioContext.currentTime` the scheduler keeps the
    /// cursor: 4096 samples = **93 ms** at 44.1 kHz, 85 ms at 48 kHz.
    ///
    /// **This is measured, not chosen, and the first value tried was half of
    /// it.** The desktop cushion is 1792 samples / 40.6 ms, sized to absorb one
    /// dropped frame — but on the desktop a cpal *callback thread* keeps
    /// pulling while the frame loop stalls. In a browser there is no second
    /// thread: `requestAnimationFrame` produces the audio **and** schedules it,
    /// so one stalled callback starves both halves at once, and the cushion has
    /// to cover the whole stall.
    ///
    /// At 2048 samples a real Chrome run measured **~2 underruns per second**
    /// with the emulator itself at a perfect 59.6 fps — the page's own
    /// animation-frame jitter, nothing to do with emulation. The bound that
    /// matters is [`crate::screen::pace::MAX_CATCHUP`] frames (134 ms), because
    /// a gap longer than that is resynchronised rather than replayed and no
    /// cushion can cover it; 4096 samples sits just under that and measured
    /// **0 underruns**.
    ///
    /// The cost is honest latency, and it is the trade the browser forces: a
    /// gap is heard as a click, 93 ms of delay is heard as nothing at all
    /// unless you are looking for it.
    pub const TARGET_LEAD: usize = 4096;

    /// The rate the APU is asked to resample to when the page has not told us
    /// what its `AudioContext` runs at. Retuned by
    /// [`super::Sound::set_player`] from [`AudioPlayer::samples_rate`], so the
    /// only thing that matters is that this is the *truth* about the stream —
    /// a mismatch is the one thing that makes a tone sharp or flat.
    pub const FALLBACK_RATE: u32 = super::SAMPLE_RATE;

    /// The hand-off queue between the APU and the scheduler, sized for this
    /// path.
    ///
    /// **Deliberately not `sample_queue::start_fill()`/`high_water()`.** Those
    /// size a queue that a *device callback* drains at exactly the sample
    /// rate, where the queue itself is the whole of the latency. Here the
    /// scheduler owns the latency ([`TARGET_LEAD`]) and the queue is only the
    /// transfer buffer between two halves of one animation frame — so clipping
    /// it at the desktop's 2816 samples destroys precisely the backlog the
    /// scheduler needs to rebuild its cushion after a stall, and (with a lead
    /// above the ceiling) can stop it ever starting. Measured with the desktop
    /// sizing: ~2,560 samples discarded **per second** on an otherwise healthy
    /// browser run. The bound is still real — a hidden tab must not grow the
    /// queue without limit — just four leads out instead of half of one.
    pub fn new_queue() -> SampleQueue {
        SampleQueue::new(TARGET_LEAD, TARGET_LEAD * 4)
    }

    /// One block of audio, with the time it must start at on the
    /// `AudioContext` clock.
    pub struct Block {
        /// `AudioContext.currentTime` value to `start()` this buffer at.
        pub when: f64,
        pub left: Vec<f32>,
        pub right: Vec<f32>,
    }

    /// The producer half — what the machine is handed.
    pub struct WebAudioPlayer {
        queue: Arc<Mutex<SampleQueue>>,
        rate: u32,
    }

    impl WebAudioPlayer {
        /// Build a player and the queue handle its consumer drains.
        ///
        /// `rate` must be the rate the `AudioContext` actually runs at
        /// (`ctx.sampleRate`), not a guess: the APU emits exactly `rate`
        /// samples per *emulated* second, so pitch is correct if and only if
        /// the two agree.
        pub fn new(rate: u32) -> (WebAudioPlayer, Arc<Mutex<SampleQueue>>) {
            let queue = Arc::new(Mutex::new(new_queue()));
            (
                WebAudioPlayer {
                    queue: queue.clone(),
                    rate,
                },
                queue,
            )
        }
    }

    impl AudioPlayer for WebAudioPlayer {
        fn play(&mut self, left: &[f32], right: &[f32]) {
            sample_queue::hand_off(&self.queue, left, right);
        }

        fn samples_rate(&self) -> u32 {
            self.rate
        }

        fn underflowed(&self) -> bool {
            // The queue substitutes a decaying hold sample of its own; nothing
            // upstream needs to know, and answering `true` would only make
            // `Sound` re-prime on top of a queue that already is.
            false
        }
    }

    /// What the scheduler has had to do — the numbers a browser run is judged
    /// on, since nobody can hear a headless one.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct SchedulerStats {
        /// Blocks handed to the `AudioContext`.
        pub blocks: u64,
        /// Times the cursor had fallen to or behind `currentTime`, so a gap
        /// was heard and the lead had to be rebuilt. **This is the crackle
        /// counter**: zero over a real run is the claim to make.
        pub underruns: u64,
        /// Ticks on which the producer had not supplied a whole block yet.
        /// Harmless on its own — the lead simply shrinks — but a rising count
        /// beside a flat `blocks` means the emulator is not keeping up.
        pub starved: u64,
    }

    /// The consumer half's decisions. No `web-sys`, no clock of its own: the
    /// caller passes `AudioContext.currentTime` in.
    pub struct Scheduler {
        rate: f64,
        block: usize,
        target_lead: f64,
        /// When the next block must start, on the caller's clock. `None` until
        /// the first block, so the very first tick is not counted as an
        /// underrun.
        cursor: Option<f64>,
        /// Has anything ever been scheduled? Distinguishes "not started yet"
        /// (prime) from "started and ran dry" (restart at once) — see the
        /// priming note in [`Scheduler::next_block`].
        started: bool,
        stats: SchedulerStats,
    }

    impl Scheduler {
        pub fn new(rate: u32) -> Scheduler {
            Scheduler {
                rate: rate as f64,
                block: SCHEDULE_BLOCK,
                target_lead: TARGET_LEAD as f64 / rate as f64,
                cursor: None,
                started: false,
                stats: SchedulerStats::default(),
            }
        }

        pub fn stats(&self) -> SchedulerStats {
            self.stats
        }

        /// Seconds of audio already scheduled and not yet played, or 0 before
        /// anything has been. This is the latency a listener feels, so it is
        /// worth reporting rather than assuming.
        pub fn lead(&self, now: f64) -> f64 {
            self.cursor.map_or(0.0, |c| (c - now).max(0.0))
        }

        /// The next block to hand to the audio device, or `None` when nothing
        /// is due — call in a loop until it answers `None`.
        ///
        /// `now` is `AudioContext.currentTime`.
        pub fn next_block(
            &mut self,
            now: f64,
            queue: &Mutex<SampleQueue>,
        ) -> Option<Block> {
            // (1) Never schedule in the past. A cursor that has reached `now`
            //     means the context has already played everything we gave it:
            //     there is a gap either way, and placing the next block behind
            //     `now` would make it *overlap* rather than follow — a click,
            //     not the silence it really is. `start(when)` with `when`
            //     already gone plays immediately; the browser does not drop it.
            //
            //     The cursor is only ever *committed* once a block has really
            //     been taken (bottom of this function), which is what keeps a
            //     dry spell to one underrun rather than one per tick.
            if self.cursor.is_some_and(|c| c <= now) {
                self.stats.underruns += 1;
                self.cursor = None;
            }

            let mut q = match queue.lock() {
                Ok(q) => q,
                // A poisoned lock must not take the page down over audio.
                Err(_) => return None,
            };
            q.note_callback();

            // (2) Prime before the *first* block, and never again.
            //
            //     Starting the moment a single block exists puts the cursor
            //     11.6 ms ahead of a producer that only delivers 16.7 ms of
            //     audio every 16.7 ms — so the first block runs out before the
            //     second can be filled, and the run opens with an underrun.
            //     Measured: exactly one, every time, until this was here.
            //
            //     **Re-priming after an underrun is the opposite mistake and
            //     was measured too.** The cushion can only ever be rebuilt out
            //     of a *backlog*, and after a stall the backlog is exactly what
            //     exists (the pacer catches the missed frames up in one go).
            //     Waiting for a full lead before restarting instead adds tens
            //     of milliseconds of further silence, during which the producer
            //     keeps filling until the queue's latency bound clips it — so
            //     one stall became a self-sustaining cycle of underruns and
            //     dropped audio, ~2 of each per second in a real browser.
            //     Restart immediately and let the drain rebuild the lead.
            let need = if self.started {
                self.block
            } else {
                TARGET_LEAD.max(self.block)
            };

            //     The restart offset is one block, not a whole target lead:
            //     the cursor is where the next block *starts*, and the caller
            //     loops until `cursor - now` reaches the target. Restarting at
            //     `now + target_lead` would satisfy the gate below immediately
            //     and schedule nothing at all.
            let cursor = match self.cursor {
                Some(c) => c,
                None => now + self.block as f64 / self.rate,
            };
            if cursor - now >= self.target_lead {
                return None; // Far enough ahead already.
            }
            if q.len() < need {
                // (3) Nothing is substituted. The lead shrinks by one tick and
                //     we try again; padding a block with held samples would
                //     advance the cursor by time no real audio occupies, and
                //     the output clock would slide for the rest of the run.
                self.stats.starved += 1;
                return None;
            }

            let mut left = Vec::new();
            let mut right = Vec::new();
            if !q.drain_block(self.block, &mut left, &mut right) {
                self.stats.starved += 1;
                return None;
            }

            self.cursor = Some(cursor + self.block as f64 / self.rate);
            self.started = true;
            self.stats.blocks += 1;
            Some(Block {
                when: cursor,
                left,
                right,
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::sound::sample_queue::SampleQueue;

        const RATE: u32 = 44_100;

        fn queue(samples: usize) -> Mutex<SampleQueue> {
            // A band wide enough that `bound_latency` never fires in these
            // tests: what is under test here is the scheduler, not the clip.
            let mut q = SampleQueue::new(0, 1 << 20);
            #[allow(clippy::let_and_return)]
            let buf = vec![0.25f32; samples];
            q.push(&buf, &buf);
            Mutex::new(q)
        }

        /// Drain everything the scheduler will give at this instant.
        fn tick(s: &mut Scheduler, now: f64, q: &Mutex<SampleQueue>) -> Vec<Block> {
            let mut out = Vec::new();
            while let Some(b) = s.next_block(now, q) {
                out.push(b);
            }
            out
        }

        /// **The click rule.** `AudioBufferSourceNode.start(when)` with `when`
        /// already past plays *immediately* rather than being skipped, so a
        /// late block overlaps its predecessor — which is heard as a click,
        /// not as the gap it really is. Every block must therefore be placed
        /// strictly in the future, including on the tick where the pipeline
        /// has just run dry.
        #[test]
        fn a_block_is_never_scheduled_in_the_past() {
            let q = queue(SCHEDULE_BLOCK * 64);
            let mut s = Scheduler::new(RATE);
            let mut now = 0.0f64;
            let mut placed = Vec::new();
            for _ in 0..20 {
                for b in tick(&mut s, now, &q) {
                    assert!(
                        b.when >= now,
                        "block scheduled {:.6}s in the past",
                        now - b.when
                    );
                    placed.push(b.when);
                }
                // A stall far longer than anything already scheduled: the
                // cursor is now behind, which is exactly the case that must
                // not produce a past-dated block.
                now += 1.0;
            }
            assert!(!placed.is_empty());
            assert!(
                s.stats().underruns > 0,
                "a 1s stall per tick must be reported as an underrun"
            );
        }

        /// **The drift rule.** The cursor *is* the output clock, so a block
        /// must carry exactly `SCHEDULE_BLOCK` real samples and advance the
        /// cursor by exactly that much time. Padding a short block with
        /// substituted samples (which is what `SampleQueue::next_sample` does
        /// for a device callback, correctly) would slide the timeline
        /// permanently.
        #[test]
        fn scheduled_blocks_are_contiguous_and_never_padded() {
            let q = queue(SCHEDULE_BLOCK * 16);
            let mut s = Scheduler::new(RATE);
            let blocks = tick(&mut s, 0.0, &q);
            assert!(blocks.len() >= 2);
            let step = SCHEDULE_BLOCK as f64 / f64::from(RATE);
            for w in blocks.windows(2) {
                assert_eq!(w[0].left.len(), SCHEDULE_BLOCK);
                assert_eq!(w[0].right.len(), SCHEDULE_BLOCK);
                assert!(
                    (w[1].when - w[0].when - step).abs() < 1e-12,
                    "gap of {}s between blocks, expected {step}s",
                    w[1].when - w[0].when
                );
            }
        }

        /// A queue with less than a whole block in it schedules **nothing**,
        /// and leaves what it has alone for the next tick.
        #[test]
        fn a_short_queue_schedules_nothing_and_keeps_its_samples() {
            let q = queue(SCHEDULE_BLOCK - 1);
            let mut s = Scheduler::new(RATE);
            assert!(tick(&mut s, 0.0, &q).is_empty());
            assert_eq!(s.stats().blocks, 0);
            assert!(s.stats().starved > 0);
            assert_eq!(q.lock().unwrap().len(), SCHEDULE_BLOCK - 1);
        }

        /// The lead is what a listener feels as latency, so it is bounded: a
        /// bottomless queue must not let one tick schedule minutes of audio.
        #[test]
        fn the_lead_is_bounded_even_with_an_unlimited_queue() {
            let q = queue(SCHEDULE_BLOCK * 4096);
            let mut s = Scheduler::new(RATE);
            let target = TARGET_LEAD as f64 / f64::from(RATE);
            for i in 0..600 {
                let now = i as f64 * 1.0 / 60.0;
                tick(&mut s, now, &q);
                let lead = s.lead(now);
                assert!(
                    lead <= target + SCHEDULE_BLOCK as f64 / f64::from(RATE) + 1e-9,
                    "lead {lead}s exceeded the {target}s target by more than one block"
                );
            }
        }

        /// **The property the whole design exists for: a steady producer must
        /// not glitch.** Ten seconds of virtual time with the emulator
        /// delivering one frame's audio every 16.743 ms and the page pumping
        /// at 60 Hz — the real shape of the browser path. Zero underruns, and
        /// the latency parked in the designed band.
        ///
        /// Teeth: raise `TARGET_LEAD` far above the production burst and this
        /// still passes; drop the "never schedule in the past" reset and the
        /// underrun count goes wrong, which is what the first test pins.
        #[test]
        fn a_steady_producer_never_underruns_over_ten_seconds() {
            let q = Mutex::new(new_queue());
            let mut s = Scheduler::new(RATE);
            let frame = 16_743.0 / 1_000_000.0f64;
            // Samples one emulated frame is worth, as the APU produces them.
            let per_frame = (f64::from(RATE) * frame).round() as usize;
            let buf = vec![0.1f32; per_frame];

            let mut produced_at = 0.0f64;
            let mut pump_at = 0.0f64;
            let mut t = 0.0f64;
            let mut max_lead: f64 = 0.0;
            while t < 10.0 {
                t = produced_at.min(pump_at);
                if produced_at <= pump_at {
                    sample_queue::hand_off(&q, &buf, &buf);
                    produced_at += frame;
                } else {
                    tick(&mut s, t, &q);
                    max_lead = max_lead.max(s.lead(t));
                    pump_at += 1.0 / 60.0;
                }
            }
            let st = s.stats();
            assert_eq!(st.underruns, 0, "steady production must not glitch: {st:?}");
            assert!(
                st.blocks > 700,
                "expected ~10s of blocks, got {}",
                st.blocks
            );
            let target = TARGET_LEAD as f64 / f64::from(RATE);
            assert!(
                max_lead <= target + SCHEDULE_BLOCK as f64 / f64::from(RATE) + 1e-9,
                "latency drifted to {max_lead}s"
            );
            assert_eq!(
                q.lock().unwrap().stats.dropped,
                0,
                "a producer on the same clock as the consumer must never be clipped"
            );
        }

        /// **The queue must be able to hold a whole scheduling lead.** The
        /// scheduler primes on [`TARGET_LEAD`] samples and the queue's latency
        /// bound clips above its high watermark: set the second below the
        /// first and the two deadlock — the queue is clipped every time it
        /// approaches the depth the scheduler is waiting for, so **not one
        /// sample is ever played**. Reached exactly that way, by reusing the
        /// desktop's `start_fill()`/`high_water()` here.
        #[test]
        fn the_queue_can_hold_a_whole_scheduling_lead() {
            let q = Mutex::new(new_queue());
            let mut s = Scheduler::new(RATE);
            let buf = vec![0.2f32; TARGET_LEAD * 2];
            sample_queue::hand_off(&q, &buf, &buf);
            assert!(
                q.lock().unwrap().len() >= TARGET_LEAD,
                "the latency bound clipped below the priming threshold"
            );
            assert!(
                !tick(&mut s, 0.0, &q).is_empty(),
                "a full queue must be able to start the scheduler"
            );
        }

        /// The producer half is what the machine is handed, so it must be
        /// `Send` — the FFI build parks a whole `Gameboy` in a `static Mutex`.
        /// This is a compile-time assertion; it exists because holding a
        /// `web_sys::AudioContext` here (the obvious first design) would break
        /// it, and the error would appear a long way from the cause.
        #[test]
        fn the_player_half_holds_nothing_that_is_not_send() {
            fn assert_send<T: Send>() {}
            assert_send::<WebAudioPlayer>();
            assert_send::<Box<dyn AudioPlayer>>();
        }
    }
}

/// Whether host audio is actually live, and if it is not, **why**.
///
/// This exists because an audio device error used to be *swallowed*: the
/// failure paths in [`cpal_audio::CpalPlayer::new`] carried a real `cpal` error
/// straight into a `#[cfg(debug_assertions)] eprintln!`, so a release build
/// discarded it (and the compiler warned about the unused binding on every
/// `--features audio` build). The visible result was an emulator with no sound
/// and no explanation — the single worst diagnostic outcome, because "silent"
/// and "broken" look identical.
///
/// `eprintln!` is not the fix here: the terminal frontend lives inside the
/// alternate screen and a stray line corrupts it. So the reason is *published*
/// instead — the Status panel reads [`snapshot`] for its `Audio` row (beside
/// `Video`), telemetry emits it as `audio_host`, and a failure raises a toast
/// on entering a game.
///
/// Always compiled, feature or no feature: without `audio` the state is simply
/// [`State::Off`], which is the honest answer for a build with no host player.
/// Any frontend that attaches its own [`AudioPlayer`] (web, FFI, a custom
/// backend) may publish its own state here.
#[cfg(not(feature = "embedded"))]
pub mod host_status {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    /// What the host audio path is doing right now.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub enum State {
        /// No host player was attached — this build has no audio feature, or
        /// the frontend never asked for one. Not an error.
        #[default]
        Off,
        /// A stream is open and playing.
        Live { sample_rate: u32, channels: u16 },
        /// A stream was asked for and could not be opened. The reason is in
        /// [`Snapshot::last_error`].
        Failed,
    }

    /// A consistent read of the whole status.
    #[derive(Debug, Clone, Default)]
    pub struct Snapshot {
        pub state: State,
        /// Errors reported by cpal on the stream *after* it opened. Non-zero
        /// with `state == Live` means sound is degrading, which is otherwise
        /// completely invisible.
        pub stream_errors: u64,
        /// Most recent failure text, whatever its source.
        pub last_error: Option<String>,
    }

    impl Snapshot {
        /// One short line for a status panel: never a guess, never empty.
        pub fn label(&self) -> String {
            match self.state {
                State::Off => "off".to_string(),
                State::Live {
                    sample_rate,
                    channels,
                } => {
                    let base = format!("● LIVE {}Hz ×{}", sample_rate, channels);
                    if self.stream_errors > 0 {
                        format!("{base} ({} err)", self.stream_errors)
                    } else {
                        base
                    }
                }
                State::Failed => match &self.last_error {
                    Some(e) => format!("failed: {e}"),
                    None => "failed".to_string(),
                },
            }
        }

        /// The telemetry/JSON spelling of [`State`].
        pub fn state_name(&self) -> &'static str {
            match self.state {
                State::Off => "off",
                State::Live { .. } => "live",
                State::Failed => "failed",
            }
        }
    }

    // The message is behind a `Mutex` but the counter is an atomic on purpose:
    // `note_stream_error` runs on cpal's *audio callback* thread, where blocking
    // on a lock is never acceptable. The counter always lands; the text is
    // best-effort via `try_lock`.
    //
    // **`AtomicU32`, not `AtomicU64`, and that is a portability constraint, not
    // a preference.** This module is compiled on every target — including the
    // embedding build — and a 32-bit machine need not have 64-bit atomics: on
    // `mipsel-sony-psp` (a PSP) `std::sync::atomic::AtomicU64` does not exist at
    // all, and the whole core failed to compile on that one line. `Snapshot`
    // still reports a `u64` so the published type is unchanged; a counter of
    // audio-device errors cannot reach 2^32 in a session. See
    // docs/portability.md.
    static STREAM_ERRORS: AtomicU32 = AtomicU32::new(0);
    static STATE: Mutex<(State, Option<String>)> = Mutex::new((State::Off, None));

    fn with_state<R>(f: impl FnOnce(&mut (State, Option<String>)) -> R) -> Option<R> {
        // A poisoned lock must not take the emulator down over a status string.
        match STATE.lock() {
            Ok(mut g) => Some(f(&mut g)),
            Err(p) => Some(f(&mut p.into_inner())),
        }
    }

    /// Read the current status.
    pub fn snapshot() -> Snapshot {
        let (state, last_error) = with_state(|s| (s.0, s.1.clone())).unwrap_or_default();
        Snapshot {
            state,
            stream_errors: STREAM_ERRORS.load(Ordering::Relaxed).into(),
            last_error,
        }
    }

    /// Publish "a stream is open and playing".
    pub fn set_live(sample_rate: u32, channels: u16) {
        STREAM_ERRORS.store(0, Ordering::Relaxed);
        with_state(|s| {
            *s = (
                State::Live {
                    sample_rate,
                    channels,
                },
                None,
            )
        });
    }

    /// Publish "audio was asked for and could not be started", with the reason.
    pub fn set_failed(reason: impl core::fmt::Display) {
        let reason = reason.to_string();
        with_state(|s| *s = (State::Failed, Some(reason)));
    }

    /// Back to [`State::Off`] — a new session before it has tried anything.
    pub fn reset() {
        STREAM_ERRORS.store(0, Ordering::Relaxed);
        with_state(|s| *s = (State::Off, None));
    }

    /// Record an error cpal raised on a *running* stream.
    ///
    /// Called from the audio callback thread: counts unconditionally (atomic),
    /// stores the text only if the lock is free.
    pub fn note_stream_error(err: impl core::fmt::Display) {
        STREAM_ERRORS.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut g) = STATE.try_lock() {
            g.1 = Some(err.to_string());
        }
    }

    /// Serialises the tests that mutate this global (here and in
    /// `telemetry`), which `cargo test` otherwise runs in parallel.
    #[cfg(test)]
    #[cfg(not(feature = "embedded"))]
    pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: Mutex<()> = Mutex::new(());
        L.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // These share one global, so they run as a single sequential test.
        #[test]
        fn status_reports_off_live_and_failure_reasons() {
            let _g = test_lock();
            reset();
            let s = snapshot();
            assert_eq!(s.state, State::Off);
            assert_eq!(s.label(), "off");
            assert_eq!(s.state_name(), "off");

            set_failed("no default output device");
            let s = snapshot();
            assert_eq!(s.state, State::Failed);
            // The whole point: the reason survives into what a human reads.
            assert_eq!(s.label(), "failed: no default output device");
            assert_eq!(s.last_error.as_deref(), Some("no default output device"));

            set_live(48_000, 2);
            let s = snapshot();
            assert_eq!(
                s.state,
                State::Live {
                    sample_rate: 48_000,
                    channels: 2
                }
            );
            assert_eq!(s.label(), "● LIVE 48000Hz ×2");
            // set_live clears a stale reason from an earlier attempt.
            assert!(s.last_error.is_none());

            note_stream_error("device disconnected");
            let s = snapshot();
            assert_eq!(s.stream_errors, 1);
            assert_eq!(s.label(), "● LIVE 48000Hz ×2 (1 err)");
            assert_eq!(s.last_error.as_deref(), Some("device disconnected"));

            reset();
            assert_eq!(snapshot().stream_errors, 0);
        }
    }
}

/// Playback volume: a **host-side attenuation of the finished sample stream**,
/// and nothing else.
///
/// # Why this is not NR50
///
/// The Game Boy has its own master volume — NR50's two 3-bit fields, one per
/// stereo side — and it is tempting to turn that down when the speakers are too
/// loud. **Do not.** NR50 is *emulated behaviour*: the accuracy suites measure
/// it (Blargg `dmg_sound`, SameSuite), a game can read it back from `$FF24`,
/// and its value is captured in save states and in the offline audio baselines.
/// Quietening the machine to quieten the room would corrupt what we model and
/// move those scores — the emulator would no longer be emulating the console
/// the player owns.
///
/// So the attenuation lives *past the end of emulation*: it is applied in the
/// device callback ([`cpal_audio::cpal_thread`]), to samples the APU has
/// already produced and handed over. Consequences, all of them deliberate:
///
/// * **Emulation is bit-identical at any setting.** Nothing upstream of the
///   audio device — APU state, register read-back, the offline capture harness
///   in `testharness/audiocapture.rs`, its hashes — can observe this at all.
/// * **Zero cost on the frame path.** `Sound::play_samples` runs inside
///   `Cpu::do_cycle`; audio work there is charged to the emulated frame, which
///   is how a blocking hand-off once cost 17.9 ms of every 16.743 ms frame
///   (see `docs/audio.md` §4c). The gain is a single relaxed atomic load per
///   *device callback* on the audio thread, and a multiply per sample there.
/// * **A change is heard immediately**, rather than after the ~40 ms already
///   sitting in the hand-off queue.
///
/// Scaling is done in `f32`, before the sample is converted to the device
/// format, so a quiet setting is quiet rather than crunchy: attenuating an
/// integer sample throws away low bits, attenuating a float does not.
///
/// # The control
///
/// One level, `0..=100`, plus a separate mute flag that remembers the level —
/// unmuting returns to where you were, never to full. The taper is `x²`
/// (the usual approximation of an audio-taper pot), so the ladder sounds evenly
/// spaced rather than doing nothing until it collapses: 100 % is unity gain
/// (exactly today's output), 50 % is −12 dB, 10 % is −40 dB.
pub mod output_volume {
    #[cfg(feature = "embedded")]
    use crate::compat::*;
    use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

    /// Loudest setting. At this level the gain is exactly 1.0 — the samples
    /// reach the device untouched, i.e. what the emulator did before there was
    /// a volume control at all.
    pub const MAX: u8 = 100;

    /// How far one key press moves the level.
    pub const STEP: u8 = 10;

    /// Where a fresh install starts.
    ///
    /// **Deliberately not `MAX`.** The out-of-the-box level was full scale and
    /// the report that produced this control was "so bloody loud" — a default
    /// nobody has ever asked to be louder is a default that is wrong. 50 % is
    /// −12 dB: quiet enough to start a game without reaching for the desktop
    /// mixer, and two key presses from where it used to be.
    pub const DEFAULT: u8 = 50;

    static LEVEL: AtomicU8 = AtomicU8::new(DEFAULT);
    static MUTED: AtomicBool = AtomicBool::new(false);

    /// The multiplier the device callback applies. `0.0` while muted.
    ///
    /// Two relaxed loads and a multiply, read **once per device callback**
    /// (not per sample). No lock: this runs on cpal's audio thread, where
    /// blocking is never acceptable — the same rule
    /// [`super::host_status::note_stream_error`] follows.
    pub fn gain() -> f32 {
        if MUTED.load(Ordering::Relaxed) {
            0.0
        } else {
            gain_for(LEVEL.load(Ordering::Relaxed))
        }
    }

    /// The gain a given level maps to, ignoring mute. `x²` audio taper.
    pub fn gain_for(level: u8) -> f32 {
        let x = level.min(MAX) as f32 / MAX as f32;
        x * x
    }

    /// Current level, `0..=100`. Unaffected by mute — see [`muted`].
    pub fn level() -> u8 {
        LEVEL.load(Ordering::Relaxed)
    }

    /// Set the level, clamped to `0..=MAX`. Does **not** change mute: a level
    /// set while muted is what unmuting will return to.
    pub fn set_level(level: u8) {
        LEVEL.store(level.min(MAX), Ordering::Relaxed);
    }

    pub fn muted() -> bool {
        MUTED.load(Ordering::Relaxed)
    }

    pub fn set_muted(m: bool) {
        MUTED.store(m, Ordering::Relaxed);
    }

    /// Flip mute; returns the new state. The level is untouched, which is the
    /// whole promise of a mute key: unmuting returns you to where you were.
    pub fn toggle_mute() -> bool {
        let m = !MUTED.load(Ordering::Relaxed);
        set_muted(m);
        m
    }

    /// Move one rung up or down the ladder and return the new level.
    ///
    /// Touching the level **unmutes**: pressing volume-up on a muted emulator
    /// obviously means "let me hear it", and leaving it silent while the number
    /// climbs is the kind of control that reads as broken.
    pub fn step(up: bool) -> u8 {
        let cur = LEVEL.load(Ordering::Relaxed).min(MAX);
        let next = if up {
            (cur.saturating_add(STEP)).min(MAX)
        } else {
            // Round down to the rung below when the level did not start on one
            // (a config file may say `volume = 35`).
            cur.saturating_sub(if cur % STEP == 0 { STEP } else { cur % STEP })
        };
        MUTED.store(false, Ordering::Relaxed);
        LEVEL.store(next, Ordering::Relaxed);
        next
    }

    /// One short label for a status row: `70%`, `0% (silent)`, `MUTED (70%)`.
    /// Mute is spelled out rather than shown as `0%` — "why is there no sound"
    /// has to be answerable from the screen.
    pub fn label() -> String {
        let l = level();
        if muted() {
            format!("MUTED ({l}%)")
        } else if l == 0 {
            "0% (silent)".to_string()
        } else {
            format!("{l}%")
        }
    }

    /// Serialises every test that moves this process-wide setting — here, in
    /// `cpal_audio`, and in the terminal frontend — which `cargo test`
    /// otherwise runs in parallel. Same device as
    /// [`super::host_status::test_lock`].
    #[cfg(test)]
    pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
        L.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// One global, so the volume tests run as a single sequential test.
        #[test]
        fn the_control_behaves_like_a_volume_knob() {
            let _g = super::test_lock();
            // Unity at the top: at MAX the emulator's output is untouched, so
            // "loudest" is exactly what shipped before this control existed.
            assert_eq!(gain_for(MAX), 1.0);
            assert_eq!(gain_for(0), 0.0);
            // Monotone, and an audio taper rather than a linear one.
            let mut prev = -1.0;
            for l in 0..=MAX {
                let g = gain_for(l);
                assert!(g > prev, "gain must rise with level at {l}");
                assert!((0.0..=1.0).contains(&g), "gain out of range at {l}");
                prev = g;
            }
            assert!((gain_for(50) - 0.25).abs() < 1e-6, "50% is -12 dB");

            set_level(DEFAULT);
            set_muted(false);
            assert_eq!(level(), DEFAULT);
            assert!((gain() - gain_for(DEFAULT)).abs() < 1e-6);

            // Mute is silence...
            assert!(toggle_mute());
            assert_eq!(gain(), 0.0, "muted output must be silence");
            assert_eq!(label(), format!("MUTED ({DEFAULT}%)"));
            // ...and unmuting returns to the previous level, not to full.
            assert!(!toggle_mute());
            assert_eq!(level(), DEFAULT);
            assert!((gain() - gain_for(DEFAULT)).abs() < 1e-6);

            // The ladder clamps at both ends.
            set_level(MAX);
            assert_eq!(step(true), MAX);
            set_level(0);
            assert_eq!(step(false), 0);
            assert_eq!(label(), "0% (silent)");

            // An off-ladder level from a config file lands on a rung.
            set_level(35);
            assert_eq!(step(false), 30);
            assert_eq!(step(true), 40);

            // Changing the level unmutes.
            set_muted(true);
            step(true);
            assert!(!muted());

            set_level(DEFAULT);
            set_muted(false);
        }

        /// Scaling is float, so a quiet setting stays clean: the classic way to
        /// get "crunchy at low volume" is to attenuate the *integer* sample and
        /// throw away its low bits. Nothing clips at the top either.
        #[test]
        fn quiet_is_clean_and_loud_does_not_clip() {
            let src: Vec<f32> =
                (0..1024).map(|i| (i as f32 * 0.03).sin() * 0.9).collect();
            for level in [10u8, 20, 50, 100] {
                let g = gain_for(level);
                let out: Vec<f32> = src.iter().map(|s| s * g).collect();
                assert!(out.iter().all(|s| s.abs() <= 1.0), "level {level} clipped");
                // Distinct input samples stay distinct after scaling — no
                // quantisation staircase, which is what integer scaling does.
                let distinct: std::collections::BTreeSet<u32> =
                    out.iter().map(|s| s.to_bits()).collect();
                let src_distinct: std::collections::BTreeSet<u32> =
                    src.iter().map(|s| s.to_bits()).collect();
                assert_eq!(
                    distinct.len(),
                    src_distinct.len(),
                    "level {level} collapsed distinct samples together"
                );
            }
        }
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "audio"))]
pub mod cpal_audio {
    use super::sample_queue;
    use super::sample_queue::{high_water, start_fill, SampleQueue};
    use super::AudioPlayer;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, Sample};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// Frames per device callback we ask cpal for.
    ///
    /// 256 frames is 5.8 ms at 44.1 kHz. The host typically keeps a small
    /// multiple of this in flight, so the device's own contribution to
    /// end-to-end delay lands in the low tens of milliseconds instead of the
    /// tens-to-hundreds `BufferSize::Default` produces. Small enough to matter,
    /// large enough that the callback is not so frequent it risks xruns.
    /// Override with `GB_AUDIO_DEVICE_FRAMES` (0 = let the host choose).
    const DEVICE_FRAMES: u32 = 256;

    /// Latency the *device* adds, sampled from cpal's callback timestamps.
    ///
    /// End-to-end delay is not just our queue: it is `queue_depth/rate` **plus**
    /// whatever the host stack holds between the callback returning and the
    /// speaker moving. cpal reports both instants — `timestamp().callback` (now)
    /// and `timestamp().playback` (when this buffer will be heard) — so the
    /// device's share is measurable rather than assumed. Measuring it is the
    /// difference between tuning the number that matters and tuning the one we
    /// happen to own.
    #[derive(Default)]
    pub struct DeviceLatency {
        /// Most recent playback-minus-callback, in microseconds.
        pub micros: std::sync::atomic::AtomicU64,
        /// Frames requested by the most recent callback.
        pub callback_frames: std::sync::atomic::AtomicUsize,
    }

    pub struct CpalPlayer {
        queue: Arc<Mutex<SampleQueue>>,
        device_latency: Arc<DeviceLatency>,
        /// Frames per device callback the stream was configured with, if we
        /// were able to pin it (`None` = the host chose).
        configured_buffer: Option<u32>,
        sample_rate: u32,
        stats_enabled: bool,
        last_stats_log: Instant,
    }

    impl CpalPlayer {
        /// Diagnostic handle to the shared queue (underrun/depth counters).
        /// Grab it before handing the player to `enable_audio`; used by
        /// `examples/audio_soak.rs` to report measured stats after a real run.
        pub fn queue_handle(&self) -> Arc<Mutex<SampleQueue>> {
            self.queue.clone()
        }

        /// Live device-side latency measurement (see [`DeviceLatency`]).
        pub fn device_latency(&self) -> Arc<DeviceLatency> {
            self.device_latency.clone()
        }

        /// The device buffer size we asked for, if the request was accepted.
        pub fn configured_buffer(&self) -> Option<u32> {
            self.configured_buffer
        }

        pub fn new() -> Option<(CpalPlayer, cpal::Stream)> {
            let device = match cpal::default_host().default_output_device() {
                Some(e) => e,
                None => {
                    super::host_status::set_failed("no default output device");
                    return None;
                }
            };

            let wanted_samplerate = cpal::SampleRate(super::SAMPLE_RATE);

            // Pick an output config. Whatever rate we settle on is reported back
            // to the APU via `samples_rate()`, and `Sound::set_player` retunes
            // blip_buf's resampler to exactly that rate — so pitch is correct for
            // *any* rate we choose, as long as the number we report is the rate
            // the stream actually runs at. The job here is therefore only to pick
            // a *sensible, real* config, not to force 44100.
            //
            // The old code walked `supported_output_configs()` and broke on the
            // FIRST F32 range regardless of suitability. On a typical ALSA/
            // PipeWire device the first F32 range is `channels=1, min=1,
            // max=384000` — i.e. it would build a MONO stream (dropping the right
            // channel) and, on any device whose first F32 range does not span
            // 44100, fall through to `with_max_sample_rate()` and open the stream
            // at 96k/192k/384k. Prefer the device's advertised default config
            // instead: it carries the device's real channel count and a rate the
            // hardware genuinely runs at, so no host-side resample and no
            // channel-count surprise.
            let selected_config = match device.default_output_config() {
                Ok(def) if def.sample_format() == cpal::SampleFormat::F32 => def,
                _ => {
                    // Default is missing or not F32: scan every F32 range and keep
                    // the best one, preferring (a) ranges that span 44100 and then
                    // (b) the most channels (>= stereo). Only settle for a range's
                    // max rate when nothing spans 44100.
                    let configs = match device.supported_output_configs() {
                        Ok(e) => e,
                        Err(err) => {
                            super::host_status::set_failed(format!(
                                "device has no listable output configs: {err}"
                            ));
                            return None;
                        }
                    };
                    let mut best: Option<cpal::SupportedStreamConfigRange> = None;
                    for f in configs {
                        if f.sample_format() != cpal::SampleFormat::F32 {
                            continue;
                        }
                        let spans = f.min_sample_rate() <= wanted_samplerate
                            && wanted_samplerate <= f.max_sample_rate();
                        best = Some(match best {
                            None => f,
                            Some(b) => {
                                let b_spans = b.min_sample_rate() <= wanted_samplerate
                                    && wanted_samplerate <= b.max_sample_rate();
                                if (spans, f.channels()) > (b_spans, b.channels()) {
                                    f
                                } else {
                                    b
                                }
                            }
                        });
                    }
                    let best = match best {
                        Some(b) => b,
                        None => {
                            super::host_status::set_failed(
                                "device advertises no F32 output configuration",
                            );
                            return None;
                        }
                    };
                    if best.min_sample_rate() <= wanted_samplerate
                        && wanted_samplerate <= best.max_sample_rate()
                    {
                        best.with_sample_rate(wanted_samplerate)
                    } else {
                        best.with_max_sample_rate()
                    }
                }
            };
            let sample_format = selected_config.sample_format();
            let channels = selected_config.channels();
            let selected_buffer_size = *selected_config.buffer_size();
            let config: cpal::StreamConfig = selected_config.into();

            // Errors on a *running* stream. This closure is invoked on cpal's
            // audio thread, so it must not block or print — it publishes to the
            // status slot (atomic counter + best-effort message), which the
            // Status panel and telemetry read. Previously it discarded `err`
            // entirely, so a device that died mid-game simply went quiet.
            let err_fn = |err| super::host_status::note_stream_error(err);

            let shared_queue =
                Arc::new(Mutex::new(SampleQueue::new(start_fill(), high_water())));
            let stream_buffer = shared_queue.clone();
            let device_latency = Arc::new(DeviceLatency::default());
            let cb_latency = device_latency.clone();

            // Pin the device buffer size. Left to `BufferSize::Default` the
            // host picks, and on ALSA/PipeWire that is routinely tens of
            // milliseconds — latency we pay on every sound and cannot see. It
            // is separately configurable from our own queue and is often the
            // larger of the two, so ask for a small period explicitly and
            // measure what we actually got.
            let mut config = config;
            let configured_buffer = requested_buffer_frames(&selected_buffer_size);
            if let Some(frames) = configured_buffer {
                config.buffer_size = cpal::BufferSize::Fixed(frames);
            }

            let player = CpalPlayer {
                queue: shared_queue,
                device_latency: device_latency.clone(),
                configured_buffer,
                sample_rate: config.sample_rate.0,
                stats_enabled: std::env::var("GB_AUDIO_STATS").is_ok(),
                last_stats_log: Instant::now(),
            };

            let stream = match sample_format {
                cpal::SampleFormat::F32 => device.build_output_stream(
                    &config,
                    move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                        note_device_latency(
                            info,
                            data.len() / channels as usize,
                            &cb_latency,
                        );
                        cpal_thread(data, &stream_buffer, channels as usize)
                    },
                    err_fn,
                    None,
                ),
                _ => return None,
            };

            match stream {
                Ok(s) => {
                    if let Err(e) = s.play() {
                        super::host_status::set_failed(format!(
                            "stream would not start: {e}"
                        ));
                        return None;
                    }
                    super::host_status::set_live(config.sample_rate.0, channels);
                    Some((player, s))
                }
                Err(e) => {
                    super::host_status::set_failed(format!(
                        "could not open output stream: {e}"
                    ));
                    None
                }
            }
        }
    }

    /// Frames per device callback to request.
    ///
    /// `GB_AUDIO_DEVICE_FRAMES=0` restores `BufferSize::Default` (let the host
    /// decide) — that is the pre-fix behaviour and the A/B control.
    fn requested_buffer_frames(supported: &cpal::SupportedBufferSize) -> Option<u32> {
        let wanted: u32 = std::env::var("GB_AUDIO_DEVICE_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEVICE_FRAMES);
        if wanted == 0 {
            return None;
        }
        match supported {
            cpal::SupportedBufferSize::Range { min, max } => {
                Some(wanted.clamp(*min, *max))
            }
            // The host will not tell us its limits; asking for a fixed size is
            // then a guess that can fail stream construction, so don't.
            cpal::SupportedBufferSize::Unknown => None,
        }
    }

    fn note_device_latency(
        info: &cpal::OutputCallbackInfo,
        frames: usize,
        out: &Arc<DeviceLatency>,
    ) {
        use std::sync::atomic::Ordering;
        let ts = info.timestamp();
        if let Some(d) = ts.playback.duration_since(&ts.callback) {
            out.micros.store(d.as_micros() as u64, Ordering::Relaxed);
        }
        out.callback_frames.store(frames, Ordering::Relaxed);
    }

    fn cpal_thread<T: Sample + FromSample<f32>>(
        output: &mut [T],
        queue: &Arc<Mutex<SampleQueue>>,
        channels: usize,
    ) {
        let mut q = match queue.lock() {
            Ok(q) => q,
            Err(_) => return,
        };
        q.note_callback();
        // Playback volume is applied HERE — on the audio thread, to finished
        // samples, on their way out of the process. Never by touching NR50:
        // that register is emulated behaviour the accuracy suites measure. See
        // `super::output_volume` for the full argument. Read once per callback
        // (two relaxed loads), in `f32` before the device-format conversion so
        // a quiet setting does not lose the sample's low bits.
        let gain = super::output_volume::gain();
        for frame in output.chunks_mut(channels) {
            let (left, right) = q.next_sample();
            for (i, sample) in frame.iter_mut().enumerate() {
                *sample = T::from_sample(if i % 2 == 0 { left } else { right } * gain);
            }
        }
    }

    impl AudioPlayer for CpalPlayer {
        /// Hand one block to the device queue. **This runs inside
        /// `Cpu::do_cycle`, i.e. inside one emulated frame, and therefore must
        /// never block** — see the module note on the audio-clock throttle.
        fn play(&mut self, left_channel: &[f32], right_channel: &[f32]) {
            // Push, then bound latency by discarding — never by waiting. See
            // `sample_queue::hand_off` and the module docs for why waiting here
            // cost the maintainer 17.6 ms of every 16.743 ms frame.
            sample_queue::hand_off(&self.queue, left_channel, right_channel);

            if self.stats_enabled
                && self.last_stats_log.elapsed() >= Duration::from_secs(5)
            {
                self.last_stats_log = Instant::now();
                if let Ok(mut q) = self.queue.lock() {
                    let s = q.stats;
                    eprintln!(
                        "audio: depth {}..{} (now {}), underruns {}, held {}, \
                         dropped {} (+{} pre-roll), pushed {}, consumed {}, \
                         callbacks {}",
                        if s.depth_min == usize::MAX {
                            0
                        } else {
                            s.depth_min
                        },
                        s.depth_max,
                        q.len(),
                        s.underruns,
                        s.held,
                        s.dropped,
                        s.dropped_before_playback,
                        s.pushed,
                        s.consumed,
                        s.callbacks,
                    );
                    q.reset_depth_window();
                }
            }
        }

        fn samples_rate(&self) -> u32 {
            self.sample_rate
        }

        fn underflowed(&self) -> bool {
            match self.queue.lock() {
                Ok(q) => q.is_empty(),
                Err(_) => true,
            }
        }

        /// No-op: the hand-off is non-blocking in every mode, so fast-forward
        /// already gets the documented "drop samples rather than pitch-shift"
        /// behaviour from the latency bound in `play`.
        fn set_fast_forward(&mut self, _enabled: bool) {}
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::sound::output_volume;

        /// What actually reaches the sound card, driven through the real
        /// callback with no device attached.
        ///
        /// This is the only place playback volume is applied, so it is the only
        /// place it can be checked: the queue holds the APU's samples unchanged
        /// at every setting (emulation is untouched), and the *device buffer*
        /// carries the attenuation. Mute must be exact silence, and unmuting
        /// must come back to the level it left — a mute key that returns you to
        /// full scale is worse than no mute key.
        #[test]
        fn the_device_buffer_carries_the_volume_and_the_queue_does_not() {
            let _g = output_volume::test_lock();

            let src: Vec<f32> = (0..512).map(|i| (i as f32 * 0.05).sin()).collect();
            let render = |level: u8, muted: bool| -> Vec<f32> {
                output_volume::set_level(level);
                output_volume::set_muted(muted);
                let q = Arc::new(Mutex::new(SampleQueue::new(0, 4096)));
                sample_queue::hand_off(&q, &src, &src);
                // The producer side is untouched by volume: what the APU made
                // is what is queued.
                assert_eq!(q.lock().unwrap().len(), src.len());
                let mut out = vec![0.0f32; src.len() * 2];
                cpal_thread(&mut out, &q, 2);
                out
            };

            let peak = |v: &[f32]| v.iter().fold(0.0f32, |a, s| a.max(s.abs()));
            let full = render(100, false);
            let half = render(50, false);
            let low = render(10, false);
            let silent = render(50, true);

            // 100 % is unity: the samples reach the device exactly as the APU
            // made them, i.e. what this emulator did before it had a volume.
            for (o, s) in full.chunks(2).zip(&src) {
                assert_eq!(o[0], *s);
            }
            assert!((peak(&half) - peak(&full) * 0.25).abs() < 1e-6);
            assert!((peak(&low) - peak(&full) * 0.01).abs() < 1e-6);
            assert!(peak(&full) <= 1.0, "no clipping at the top of the range");
            // Mute is digital silence, not "very quiet".
            assert!(silent.iter().all(|s| *s == 0.0));

            // Unmute returns to the level, not to full.
            output_volume::set_muted(false);
            assert_eq!(output_volume::level(), 50);
            let back = render(output_volume::level(), false);
            for (a, b) in back.iter().zip(&half) {
                assert_eq!(a, b);
            }

            output_volume::set_level(output_volume::DEFAULT);
            output_volume::set_muted(false);
        }
    }
}

#[cfg(test)]
mod queue_tests {
    //! Virtual-time producer/consumer simulation of the audio hand-off queue.
    //! This is how the queue bound and policy were *chosen*: instead of
    //! picking a nicer buffer number, the producer cadence (2000-sample blip
    //! blocks from a 59.7275 Hz frame loop) and consumer cadence (a fixed-size
    //! audio callback at 44.1 kHz) are replayed against the policy with
    //! deterministic jitter and stalls, and underruns/depth are *counted*.
    //! Wall-clock-free, device-free, exact same `SampleQueue` code the cpal
    //! player runs.
    use super::sample_queue::{high_water, start_fill, SampleQueue, BLOCK};

    const RATE: f64 = 44100.0;
    /// True DMG frame period: 70224 cycles / 4.194304 MHz.
    const FRAME_S: f64 = 70224.0 / 4_194_304.0;

    /// Deterministic LCG (no external randomness — results are reproducible).
    struct Lcg(u64);
    impl Lcg {
        fn next_ms(&mut self, max_ms: f64) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) as f64 / (1u64 << 31) as f64) * max_ms
        }
    }

    struct SimResult {
        queue: SampleQueue,
        /// Depth at callback entry, min/max, *after* the priming phase ended.
        post_prime_depth_min: usize,
        post_prime_depth_max: usize,
    }

    /// Replay `seconds` of virtual time. The producer emulates frames on a
    /// deadline accumulator (like the fixed frontends): frame k is *scheduled*
    /// at k*FRAME_S but lands late by `jitter_ms`-bounded noise, plus a
    /// `stall_ms` production stall every `stall_every_s` (a slow terminal
    /// draw, a GC hiccup...). Samples accumulate at 44100/emulated-second and
    /// are pushed in whole `BLOCK`-sample blocks, exactly as `play_samples`
    /// does. The consumer is an audio callback of `cb` frames every cb/RATE
    /// seconds, jitter-free (the sound card's DMA clock).
    fn simulate(
        seconds: f64,
        cb: usize,
        jitter_ms: f64,
        stall_every_s: f64,
        stall_ms: f64,
    ) -> SimResult {
        // Producer push events: (virtual time, block).
        let mut pushes: Vec<f64> = Vec::new();
        let mut rng = Lcg(0x5EED);
        let mut acc = 0f64;
        let mut behind = 0f64; // accumulated stall not yet recovered
        let frames = (seconds / FRAME_S).ceil() as usize;
        for k in 0..frames {
            let scheduled = k as f64 * FRAME_S;
            let mut late = rng.next_ms(jitter_ms) / 1000.0;
            if stall_every_s > 0.0 {
                let phase = scheduled % stall_every_s;
                if phase < FRAME_S {
                    behind += stall_ms / 1000.0;
                }
            }
            // The deadline accumulator catches up after a stall (bounded, but
            // the sim's stalls are within the bound): the *schedule* is not
            // pushed back, the late frames just land in a burst as the loop
            // spins through the backlog.
            late += behind;
            behind = (behind - FRAME_S).max(0.0); // catch-up: one extra frame per period
            acc += RATE * FRAME_S;
            while acc >= BLOCK as f64 {
                acc -= BLOCK as f64;
                pushes.push(scheduled + late);
            }
        }

        let mut q = SampleQueue::new(start_fill(), high_water());
        let mut push_idx = 0usize;
        let mut post_min = usize::MAX;
        let mut post_max = 0usize;
        let n_callbacks = (seconds * RATE / cb as f64) as usize;
        let mut primed_once = false;
        for j in 0..n_callbacks {
            let t = j as f64 * cb as f64 / RATE;
            while push_idx < pushes.len() && pushes[push_idx] <= t {
                let block = vec![0.1f32; BLOCK];
                q.push(&block, &block);
                push_idx += 1;
            }
            q.note_callback();
            if primed_once {
                post_min = post_min.min(q.len());
                post_max = post_max.max(q.len());
            }
            for _ in 0..cb {
                q.next_sample();
            }
            if q.stats.consumed > 0 {
                primed_once = true;
            }
        }
        SimResult {
            queue: q,
            post_prime_depth_min: post_min,
            post_prime_depth_max: post_max,
        }
    }

    /// Healthy case, with the stall profile taken from **measurement** rather
    /// than assumption.
    ///
    /// The original profile here was a 90 ms production stall every 5 s. That
    /// is what forced a 159 ms cushion, and a cushion is latency: end-to-end
    /// delay measured 299 ms, which the player feels after every button press.
    /// Instrumenting the real frame loop (`examples/audio_latency.rs` reports
    /// the lateness distribution) shows that profile is not what happens: over
    /// 5375 frames of Pokémon Blue, **4 frames (0.075%) finished more than 1 ms
    /// late**, p99 and p99.9 both 0.0 ms, with rare isolated excursions.
    ///
    /// So the profile encodes the *promise* instead: 8 ms of per-frame jitter
    /// plus **one whole frame dropped every 2 seconds** — hundreds of times
    /// more stall time than measured, and exactly the guarantee [`start_fill`]
    /// is sized for. Requirement is unchanged: ZERO underruns over five minutes
    /// with bounded depth.
    ///
    /// The cushion's limit is explicit: a production gap longer than
    /// `start_fill / rate` (~41 ms) costs one re-prime. That is the price of
    /// ~50 ms end-to-end instead of 299 ms, and the failure mode is a
    /// fade-and-rebuild, not a click.
    #[test]
    fn no_underruns_with_realistic_jitter_and_stalls() {
        // 8 ms jitter, plus one whole frame dropped every 2 s.
        let r = simulate(300.0, 512, 8.0, 2.0, 1000.0 / 59.7275);
        assert_eq!(
            r.queue.stats.underruns, 0,
            "underruns in the healthy profile — cushion too small"
        );
        assert!(
            r.post_prime_depth_max <= high_water() + BLOCK,
            "depth {} exceeded high watermark + one block — latency unbounded",
            r.post_prime_depth_max
        );
        assert_eq!(r.queue.stats.dropped, 0);
        // The cushion must actually be exercised: depth should dip well below
        // START_FILL during stalls (otherwise the test proves nothing).
        assert!(
            r.post_prime_depth_min < start_fill(),
            "stalls never stressed the queue (min depth {})",
            r.post_prime_depth_min
        );
    }

    /// A stall longer than the cushion MUST underrun — this pins the meaning
    /// of the cushion: it is the largest production gap that stays inaudible,
    /// and it is bought with latency. If this starts passing with 0 underruns,
    /// the cushion grew and the latency story changed; revisit docs/audio.md.
    ///
    /// This is the honest cost of the 300 ms -> 48 ms latency reduction: the
    /// absorbable gap shrank with it. The trade is deliberate, and the profile
    /// above is what says it is the right one on measured evidence.
    #[test]
    fn stall_longer_than_cushion_underruns_once_and_recovers() {
        let r = simulate(60.0, 512, 0.0, 20.0, 400.0);
        assert!(
            r.queue.stats.underruns >= 1,
            "a 400 ms stall cannot be absorbed by a {} ms cushion",
            start_fill() as f64 * 1000.0 / RATE
        );
        // Each stall costs at most ONE rebuffer (underrun -> re-prime), never
        // a burst of on/off gaps: 60 s / one stall per 20 s = 3 stalls max.
        assert!(
            r.queue.stats.underruns <= 3,
            "each stall must cost at most one underrun, got {}",
            r.queue.stats.underruns
        );
        assert_eq!(r.queue.stats.dropped, 0);
    }

    /// Underrun output: the queue holds the *last* sample and fades it,
    /// never stepping to zero. A step to silence is a wideband click — the
    /// exact "blocky" artifact this policy replaces.
    #[test]
    fn underrun_holds_and_fades_instead_of_stepping_to_zero() {
        let mut q = SampleQueue::new(2 * BLOCK, 4 * BLOCK);
        let block = vec![0.25f32; 2 * BLOCK];
        q.push(&block, &block);
        for _ in 0..2 * BLOCK {
            q.next_sample();
        }
        // Queue now dry: next sample must be ~the last one, not 0.0.
        let (l, _) = q.next_sample();
        assert_eq!(q.stats.underruns, 1);
        assert!(
            (l - 0.25).abs() < 0.01,
            "underrun stepped from 0.25 to {l} — audible click"
        );
        // ...and it decays smoothly toward zero.
        let mut prev = l;
        for _ in 0..20_000 {
            let (l, _) = q.next_sample();
            assert!(l <= prev && (prev - l) < 0.001, "fade not smooth");
            prev = l;
        }
        assert!(prev < 0.01, "hold did not decay (DC offset parked)");
        // A single underrun while dry — not one per held sample.
        assert_eq!(q.stats.underruns, 1);
    }

    /// After an underrun the queue re-primes: it does NOT drain the very next
    /// block (which would immediately underrun again — the 45 ms on / 45 ms
    /// off "blocky and broken" cycle). It waits for the full cushion.
    #[test]
    fn reprimes_after_underrun() {
        let mut q = SampleQueue::new(2 * BLOCK, 4 * BLOCK);
        let block = vec![0.25f32; 2 * BLOCK];
        q.push(&block, &block);
        for _ in 0..2 * BLOCK + 10 {
            q.next_sample(); // drain past empty -> underrun, re-priming
        }
        let one = vec![0.5f32; BLOCK];
        q.push(&one, &one); // one block: below START_FILL
        let (l, _) = q.next_sample();
        assert!(
            (l - 0.5).abs() > 0.2,
            "queue drained a lone block while re-priming (blocky cycle)"
        );
        q.push(&one, &one); // second block: cushion complete
        let (l, _) = q.next_sample();
        assert!(
            (l - 0.5).abs() < 1e-6,
            "queue failed to resume after re-prime"
        );
    }

    /// Producer overrun: pushing far past the high watermark clips back to the
    /// *cushion* (not the ceiling) and counts every discarded sample, split by
    /// whether the device had started playing.
    #[test]
    fn clipping_drains_to_the_cushion_not_to_the_ceiling() {
        let mut q = SampleQueue::new(start_fill(), high_water());
        let block = vec![0.1f32; BLOCK];
        // Push well past the watermark whatever it is currently sized at.
        let pushes = high_water() / BLOCK + 4;
        for _ in 0..pushes {
            q.push(&block, &block);
        }
        assert_eq!(q.over_high_water(), pushes * BLOCK - high_water());
        let over = pushes * BLOCK - start_fill();
        q.bound_latency();
        // Drained to the cushion, leaving the whole band free again — not to
        // the ceiling, which would re-clip on the next burst for ever.
        assert_eq!(q.len(), start_fill());
        // Nothing has been played yet, so this is startup backlog, not a
        // glitch — it must not land in the counter the soak test gates on.
        assert_eq!(q.stats.dropped, 0);
        assert_eq!(q.stats.dropped_before_playback, over as u64);

        // Once the device has taken a sample, further clipping is real audio
        // the listener would have heard, and is counted as such.
        q.next_sample();
        for _ in 0..pushes {
            q.push(&block, &block);
        }
        assert!(q.over_high_water() > 0);
        let expected = q.len() - start_fill();
        q.bound_latency();
        assert_eq!(q.len(), start_fill());
        assert_eq!(q.stats.dropped, expected as u64);
        assert_eq!(q.stats.dropped_before_playback, over as u64);
    }
}

#[cfg(test)]
mod tests {
    //! Objective audio checks. These drive the real APU + blip_buf resampler
    //! through a mock `AudioPlayer` that just records the left channel, so they
    //! need no host audio device and run under a plain `cargo test` (the `audio`
    //! feature only gates *host playback*, never synthesis). They pin down two
    //! facts the "rough and fast" investigation established:
    //!   1. the APU emits exactly the reported sample rate per emulated second
    //!      (so wall-clock speed, not resampling, governs perceived tempo), and
    //!   2. a programmed tone comes out at the correct pitch.
    use super::{
        AudioPlayer, CgbRevision, LengthCounter, Sound, CLOCKS_PER_FRAME,
        CLOCKS_PER_SECOND, SQUARE_TRIGGER_DELAY,
    };
    use std::sync::{Arc, Mutex};

    struct Capture {
        rate: u32,
        samples: Arc<Mutex<Vec<f32>>>,
    }
    impl AudioPlayer for Capture {
        fn play(&mut self, left: &[f32], _right: &[f32]) {
            self.samples.lock().unwrap().extend_from_slice(left);
        }
        fn samples_rate(&self) -> u32 {
            self.rate
        }
        fn underflowed(&self) -> bool {
            false
        }
    }

    /// Build a `Sound` wired to a capture player at `rate`, powered on with a
    /// 50%-ish duty square on channel 1 at frequency register `freq_x`.
    fn tone(rate: u32, freq_x: u16) -> (Sound, Arc<Mutex<Vec<f32>>>) {
        let samples = Arc::new(Mutex::new(Vec::new()));
        let mut snd = Sound::new();
        snd.set_player(Box::new(Capture {
            rate,
            samples: samples.clone(),
        }));
        snd.wb(0xFF26, 0x80); // APU power on
        snd.wb(0xFF25, 0xFF); // route all channels to L+R
        snd.wb(0xFF24, 0x77); // master volume max both sides
        snd.wb(0xFF11, 0x00); // ch1 duty 12.5%, length load 0
        snd.wb(0xFF12, 0xF0); // ch1 volume 15, DAC on, no envelope
        snd.wb(0xFF13, (freq_x & 0xFF) as u8);
        snd.wb(0xFF14, 0x80 | (((freq_x >> 8) & 0x7) as u8)); // trigger, length off
        (snd, samples)
    }

    fn run_seconds(snd: &mut Sound, secs: u32) {
        let total = CLOCKS_PER_SECOND * secs;
        let mut done = 0u32;
        while done < total {
            snd.do_cycle(64);
            done += 64;
        }
    }

    /// The DMG wave-RAM access window is derived from
    /// `WaveChannel::since_fetch`, which is live at any point CH3 is playing —
    /// so it must be serialised, or a state saved with the window open loads
    /// with it closed and windowed reads desync ($FF instead of the fetched
    /// byte). This test has teeth: deliberately dropping the `since_fetch`
    /// write/read pair from `WaveChannel::{save,load}_state` makes the loaded
    /// machine read $FF below while the original reads $84 (verified by doing
    /// exactly that — the assert fires).
    #[test]
    fn wave_window_state_survives_save_load() {
        let samples = Arc::new(Mutex::new(Vec::new()));
        let mut snd = Sound::new();
        snd.set_player(Box::new(Capture {
            rate: 44100,
            samples,
        }));
        snd.wb(0xFF26, 0x80); // APU power on
        snd.wb(0xFF1A, 0x80); // CH3 DAC on
        snd.wb(0xFF1C, 0x20); // CH3 volume 100%
        snd.wb(0xFF1D, 0xFE); // freq low: freq = 0x7FE -> period 4 T-cycles
        snd.wb(0xFF1E, 0x87); // trigger + freq high
                              // Trigger reloads the fetch timer with period + 4 = 8 T-cycles, so the
                              // first wave-RAM fetch lands at T+8; sync to T+10 puts us 2 T-cycles
                              // after it — inside the DMG access window.
        snd.do_cycle(10);
        snd.catch_up();
        let live = snd.rb(0xFF30);
        assert_ne!(
            live, 0xFF,
            "DMG wave window should be open 2 cycles after a fetch"
        );

        let mut w = crate::savestate::Writer::new();
        snd.save_state(&mut w);
        let mut loaded = Sound::new();
        let mut r = crate::savestate::Reader::new(&w.buf);
        loaded.load_state(&mut r).expect("APU state load");
        assert_eq!(
            loaded.rb(0xFF30),
            live,
            "windowed wave-RAM read desynced across save/load"
        );

        // And the full APU block must round-trip byte-identically.
        let mut w2 = crate::savestate::Writer::new();
        loaded.save_state(&mut w2);
        assert_eq!(w.buf, w2.buf, "APU state not byte-identical after load");
    }

    /// Count of produced samples per emulated second must equal the rate we told
    /// the APU, independent of that rate. This is what guarantees pitch is
    /// correct for whatever rate the device negotiates: blip is retuned to the
    /// reported rate, so N seconds of emulation always yields N*rate samples.
    #[test]
    fn output_matches_reported_rate() {
        let secs = 10u32;
        for &rate in &[44100u32, 48000, 22050] {
            let (mut snd, cap) = tone(rate, 1750);
            run_seconds(&mut snd, secs);
            let n = cap.lock().unwrap().len() as i64;
            let expected = (rate * secs) as i64;
            // blip only emits in fixed 2000-sample blocks, so up to a couple of
            // blocks may be withheld at the tail — a small absolute shortfall,
            // never an overshoot. Tolerance is in blocks (rate-independent) so it
            // stays tight in *rate* terms: a genuine rate mismatch (e.g. emitting
            // 44100 for a 48000 device) is a ~4000-sample/sec drift that blows
            // straight past this. That the count tracks `rate` and not a fixed
            // 44100 is the point — pitch follows whatever rate the device reports.
            let block = super::OUTPUT_SAMPLE_COUNT as i64;
            assert!(
                n <= expected + block && n >= expected - 3 * block,
                "rate {rate}: produced {n} samples, expected ~{expected} \
                 (drift {} samples)",
                n - expected
            );
        }
    }

    /// PCM12/PCM34 report each channel's live 4-bit digital output, CH1/CH3
    /// in the low nibble and CH2/CH4 in the high nibble — the window every
    /// SameSuite APU test observes the APU through. A channel that is off, or
    /// whose DAC is off, reports zero.
    #[test]
    fn pcm_registers_report_each_channel_digital_output() {
        let mut snd = Sound::new();
        snd.set_player(Box::new(Capture {
            rate: 44100,
            samples: Arc::new(Mutex::new(Vec::new())),
        }));
        snd.set_mode(false); // CGB
        snd.wb(0xFF26, 0x80); // power on
        assert_eq!(snd.rb(0xFF76), 0x00, "nothing triggered yet");
        assert_eq!(snd.rb(0xFF77), 0x00);

        // CH2: duty 3 (high for six of eight positions), volume 9, no envelope.
        snd.wb(0xFF16, 0xC0);
        snd.wb(0xFF17, 0x90);
        snd.wb(0xFF18, 0x00);
        snd.wb(0xFF19, 0x80); // trigger, frequency 0 -> slowest sweep of the duty
                              // Run until the duty latch has taken a high position.
        let mut high = None;
        for _ in 0..64 {
            snd.do_cycle(2048 * 4);
            snd.catch_up();
            let v = snd.rb(0xFF76);
            if v != 0 {
                high = Some(v);
                break;
            }
        }
        assert_eq!(
            high,
            Some(0x90),
            "CH2 output belongs in the high nibble of PCM12 and carries the envelope volume"
        );

        // Killing the DAC must zero it immediately, channel-enable and all.
        snd.wb(0xFF17, 0x00);
        snd.catch_up();
        assert_eq!(snd.rb(0xFF76), 0x00, "a dead DAC reports zero");
    }

    /// Writing DIV clocks the frame sequencer when the DIV-APU bit is set: the
    /// reset drops the bit, and a falling edge *is* the DIV-APU event. Pinned
    /// through the length counter, which is what SameSuite `div_write_trigger`
    /// reads back through NR52.
    #[test]
    fn a_div_write_clocks_the_frame_sequencer() {
        for (advance, expect_clock) in [(0u32, false), (CLOCKS_PER_FRAME / 2, true)] {
            let mut snd = Sound::new();
            snd.set_player(Box::new(Capture {
                rate: 44100,
                samples: Arc::new(Mutex::new(Vec::new())),
            }));
            snd.set_mode(false);
            snd.wb(0xFF26, 0x80);
            // CH1: length 1, DAC on, trigger with length enabled.
            snd.wb(0xFF11, 0x3F);
            snd.wb(0xFF12, 0xF0);
            snd.wb(0xFF14, 0xC0);
            assert_eq!(snd.rb(0xFF26) & 1, 1, "channel 1 is on after the trigger");

            snd.do_cycle(advance);
            snd.catch_up();
            snd.div_write();
            snd.catch_up();
            assert_eq!(
                snd.rb(0xFF26) & 1 == 0,
                expect_clock,
                "DIV write with the DIV-APU bit {} should{} clock length",
                if expect_clock { "set" } else { "clear" },
                if expect_clock { "" } else { " not" },
            );
        }
    }

    /// At CGB double speed the CPU ticks twice as fast and the APU does not:
    /// it is wired to the unmodified 4.194304 MHz clock. Feeding it CPU ticks
    /// unhalved made every sound an octave high *and* — because the audio queue
    /// paces the emulator — throttled CGB double-speed games to about half
    /// speed. Same emulated second, same sample count, either way.
    #[test]
    fn double_speed_halves_the_apu_clock() {
        let mut counts = Vec::new();
        for double in [false, true] {
            let (mut snd, cap) = tone(44100, 1750);
            snd.set_double_speed(double);
            // One emulated second of *CPU* ticks: twice as many at double speed.
            let cpu_ticks = if double {
                CLOCKS_PER_SECOND * 2
            } else {
                CLOCKS_PER_SECOND
            };
            // Small steps: `do_cycle` is fed instruction-sized ticks in
            // practice, and blip's frame buffer is sized for that.
            let chunk = 64;
            for _ in 0..(cpu_ticks / chunk) {
                snd.do_cycle(chunk);
            }
            counts.push(cap.lock().unwrap().len());
        }
        let (single, double) = (counts[0] as i64, counts[1] as i64);
        assert!(
            (single - double).abs() <= super::OUTPUT_SAMPLE_COUNT as i64,
            "one emulated second must yield the same sample count at either \
             speed: {single} single vs {double} double"
        );
    }

    /// A tone programmed to ~440 Hz must come out at ~440 Hz. Proves the
    /// synthesis + resampling pitch is correct — so any perceived "fast" is
    /// wall-clock pacing, not the APU.
    #[test]
    fn tone_pitch_is_correct() {
        let rate = 44100u32;
        // GB square: f = 131072 / (2048 - x). x = 1750 -> 439.84 Hz.
        let freq_x = 1750u16;
        let expected = 131072.0 / (2048 - freq_x) as f64;
        let (mut snd, cap) = tone(rate, freq_x);
        run_seconds(&mut snd, 2);
        let s = cap.lock().unwrap();
        assert!(s.len() > rate as usize, "not enough samples captured");

        // Measure fundamental by upward zero-crossings of the AC-coupled signal.
        let mean: f32 = s.iter().sum::<f32>() / s.len() as f32;
        let mut crossings = 0usize;
        let mut prev = s[0] - mean;
        for &x in &s[1..] {
            let v = x - mean;
            if prev <= 0.0 && v > 0.0 {
                crossings += 1;
            }
            prev = v;
        }
        let dur = s.len() as f64 / rate as f64;
        let measured = crossings as f64 / dur;
        let err = (measured / expected - 1.0).abs();
        assert!(
            err < 0.03,
            "expected {expected:.2} Hz, measured {measured:.2} Hz (err {:.2}%)",
            err * 100.0
        );
    }

    /// A sweep that overflows does not kill the channel at the instant it
    /// updates the frequency — the shifter takes `shift + 1` M-cycles and the
    /// overflow check that disables the channel happens at the end of that.
    /// SameSuite `channel_1_sweep` reads PCM12 one M-cycle at a time across
    /// this window; before the delay existed the channel went silent 8
    /// M-cycles early.
    #[test]
    fn a_sweep_overflow_kills_the_channel_late_not_instantly() {
        let mut snd = Sound::new();
        snd.set_player(Box::new(Capture {
            rate: 44100,
            samples: Arc::new(Mutex::new(Vec::new())),
        }));
        snd.wb(0xFF26, 0x80);
        snd.wb(0xFF10, 0x17); // sweep: period 1, add, shift 7
        snd.wb(0xFF11, 0x80);
        snd.wb(0xFF13, 0xFF);
        snd.wb(0xFF12, 0x80); // volume 8, DAC on
        snd.wb(0xFF14, 0x87); // trigger, freq $7FF -> $7FF + $F overflows

        // The trigger's own calculation is armed, not performed: the channel
        // is still enabled for `(7 + 2) * 4` T-cycles.
        assert!(snd.channel1.enabled, "trigger must not disable instantly");
        for _ in 0..8 {
            snd.do_cycle(4);
            snd.catch_up();
        }
        assert!(
            snd.channel1.enabled,
            "channel died within 8 M-cycles of the trigger"
        );
        snd.do_cycle(8);
        snd.catch_up();
        assert!(
            !snd.channel1.enabled,
            "the overflow check never ran: channel still enabled"
        );
    }

    /// A 128 Hz sweep event that lands a cycle or two after a trigger cannot
    /// kill the channel: the trigger's shadow reload takes three M-cycles and
    /// the overflow check reads the shadow as zero until it finishes, so the
    /// check that event arms compares zeros. SameSuite
    /// `channel_1_sweep_restart_2` walks a re-trigger past a sweep event one
    /// M-cycle at a time and requires the channel to survive every position
    /// except the one where the event lands a full three M-cycles after it.
    #[test]
    fn a_sweep_event_inside_the_restart_hold_cannot_kill_the_channel() {
        /// `gap` T-cycles between the trigger and the 128 Hz sweep event.
        fn survives(gap: u32) -> bool {
            let mut snd = Sound::new();
            snd.set_player(Box::new(Capture {
                rate: 44100,
                samples: Arc::new(Mutex::new(Vec::new())),
            }));
            snd.wb(0xFF26, 0x80);
            snd.wb(0xFF10, 0x10); // sweep: period 1, add, shift 0
            snd.wb(0xFF11, 0x80);
            snd.wb(0xFF13, 0xFF);
            snd.wb(0xFF12, 0x80); // volume 8, DAC on
            snd.wb(0xFF14, 0x87); // trigger at $7FF, so $7FF + $7FF overflows
            for _ in 0..gap / 4 {
                snd.do_cycle(4);
                snd.catch_up();
            }
            snd.channel1.sweep_step();
            // Let the check that event armed run to completion.
            for _ in 0..4 {
                snd.do_cycle(4);
                snd.catch_up();
            }
            snd.channel1.enabled
        }
        assert!(
            survives(super::SWEEP_RESTART_HOLD - 4),
            "a sweep event inside the restart hold killed the channel"
        );
        assert!(
            !survives(super::SWEEP_RESTART_HOLD),
            "a sweep event after the hold must still kill an overflowing channel"
        );
    }

    /// Clearing NR10's shift field parks a calculation that is already in
    /// flight, so a channel that was about to be killed by the overflow check
    /// keeps playing (`channel_1_sweep_restart` round 3).
    #[test]
    fn clearing_the_sweep_shift_parks_a_pending_overflow_check() {
        let mut snd = Sound::new();
        snd.set_player(Box::new(Capture {
            rate: 44100,
            samples: Arc::new(Mutex::new(Vec::new())),
        }));
        snd.wb(0xFF26, 0x80);
        snd.wb(0xFF10, 0x17);
        snd.wb(0xFF11, 0x80);
        snd.wb(0xFF13, 0xFF);
        snd.wb(0xFF12, 0x80);
        snd.wb(0xFF14, 0x87);
        snd.wb(0xFF10, 0x00); // sweep off before the shifter finishes
        for _ in 0..64 {
            snd.do_cycle(4);
            snd.catch_up();
        }
        assert!(
            snd.channel1.enabled,
            "a parked calculation must not disable the channel"
        );
    }

    /// The envelope's period counter is free-running: parking it at zero with
    /// a period-0 NRx2 and then writing a non-zero period starts it stepping
    /// again without a trigger, and the first step lands one 512 Hz event
    /// later rather than a whole 64 Hz period later
    /// (`channel_N_nrx2_speed_change` tests 3-7).
    #[test]
    fn enabling_the_envelope_mid_note_steps_within_one_div_apu_event() {
        let mut snd = Sound::new();
        snd.set_player(Box::new(Capture {
            rate: 44100,
            samples: Arc::new(Mutex::new(Vec::new())),
        }));
        snd.wb(0xFF26, 0x80);
        snd.wb(0xFF11, 0xC0);
        snd.wb(0xFF13, 0xFF);
        snd.wb(0xFF12, 0x88); // volume 8, up, period 0 -> envelope parked
        snd.wb(0xFF14, 0x87);
        // Let the parked envelope sit for a couple of 64 Hz periods.
        for _ in 0..(CLOCKS_PER_FRAME * 16 / 8) {
            snd.do_cycle(8);
        }
        snd.catch_up();
        assert_eq!(
            snd.channel1.volume.volume, 8,
            "a parked envelope must not step"
        );

        snd.wb(0xFF12, 0x89); // same volume/direction, period 1
        let after_write = snd.channel1.volume.volume;
        assert_eq!(
            after_write, 9,
            "the NRx2 write itself steps the envelope once"
        );
        // One 512 Hz DIV-APU event is 8192 T-cycles.
        for _ in 0..(CLOCKS_PER_FRAME / 8 + 8) {
            snd.do_cycle(8);
        }
        snd.catch_up();
        assert_eq!(
            snd.channel1.volume.volume, 10,
            "the resumed envelope must step at the next 512 Hz event, not the \
             next 64 Hz one"
        );
    }

    /// …while a *triggered* envelope still steps on the 64 Hz boundary it
    /// always did. The period counter runs one DIV-APU event ahead of the step
    /// it schedules, so moving the count to step 6 keeps the volume change on
    /// step 7. SameSuite `channel_N_volume_div` measures that boundary to the
    /// M-cycle and goes red if the step slides a whole 512 Hz event later.
    #[test]
    fn a_triggered_envelope_still_steps_on_the_64hz_boundary() {
        let mut snd = Sound::new();
        snd.set_player(Box::new(Capture {
            rate: 44100,
            samples: Arc::new(Mutex::new(Vec::new())),
        }));
        snd.wb(0xFF26, 0x80);
        snd.wb(0xFF11, 0x80);
        snd.wb(0xFF13, 0xFC);
        snd.wb(0xFF12, 0x81); // volume 8, down, period 1
        snd.wb(0xFF14, 0x87);
        let mut t = 0u32;
        while snd.channel1.volume.volume == 8 && t < 200_000 {
            snd.do_cycle(4);
            snd.catch_up();
            t += 4;
        }
        // One 64 Hz envelope period is 8 * CLOCKS_PER_FRAME T-cycles.
        let period = CLOCKS_PER_FRAME * 8;
        assert!(
            t <= period,
            "first envelope step landed at {t} T-cycles, past the 64 Hz \
             boundary at {period}"
        );
    }

    /// The delay from a CH4 trigger to its first LFSR step, for every
    /// `(divisor, shift)` pair SameSuite measures. Nothing in CI runs
    /// SameSuite, so this is the gate on that calibration: the numbers are
    /// read off `apu/channel_4/channel_4_frequency_alignment.asm` (nine NR43
    /// encodings, whose `CorrectResults` rows differ by whole M-cycles) and
    /// `channel_4_delay.asm`, not off our own output.
    ///
    /// Read them as `(period - divisor)/2 + max(divisor/2, one tick) + 2`:
    /// a trigger leaves both counters half-way through, and the divisor stage
    /// cannot be left holding less than one of its own ticks. The four
    /// encodings of a single 16 M-cycle sample — $1A, $0C, $29, $38 — are the
    /// load-bearing rows: they must come out 34/34/42/38, i.e. 0/0/+2/+1
    /// M-cycles apart, and a `divisor/2` with no floor gets $1A and $0C wrong
    /// by 4 and 8 M-cycles (`channel_4_equivalent_frequencies` red).
    #[test]
    fn the_noise_trigger_delay_matches_samesuite() {
        // (NR43, expected delay in T-cycles)
        const CASES: [(u8, u32); 11] = [
            (0x00, 10), // divisor code 0, shift 0 — channel_4_delay
            (0x08, 10), //   …and its 7-bit twin
            (0x09, 18), // code 1, shift 0 — frequency_alignment row 0
            (0x18, 14), // code 0, shift 1 — row 1 (and channel_4_delay)
            (0x0A, 18), // code 2, shift 0 — row 2
            (0x28, 22), // code 0, shift 2 — row 3 (and channel_4_delay)
            (0x0B, 26), // code 3, shift 0 — row 4: the floor must NOT apply
            (0x1A, 34), // code 2, shift 1 — row 5
            (0x0C, 34), // code 4, shift 0 — row 6
            (0x29, 42), // code 1, shift 2 — row 7: the floor DOES apply
            (0x38, 38), // code 0, shift 3 — row 8
        ];
        for (nr43, want) in CASES {
            let mut snd = Sound::new();
            snd.set_player(Box::new(Capture {
                rate: 44100,
                samples: Arc::new(Mutex::new(Vec::new())),
            }));
            snd.wb(0xFF26, 0x80); // APU on — also zeroes the 1 MHz grid phase
            snd.wb(0xFF21, 0xF0); // NR42: volume 15, no envelope
            snd.wb(0xFF22, nr43);
            snd.wb(0xFF23, 0x80); // NR44: trigger
            assert_eq!(
                snd.channel4.delay, want,
                "NR43=${nr43:02X}: first LFSR step at {} T-cycles, hardware \
                 says {want}",
                snd.channel4.delay
            );
        }
    }

    /// A square channel triggered at frequency `$7FC`: period 16 T, first
    /// expiry at [`SQUARE_TRIGGER_DELAY`] past a whole period, i.e. T=22.
    fn ticking_square() -> Sound {
        let mut snd = Sound::new();
        snd.set_player(Box::new(Capture {
            rate: 44100,
            samples: Arc::new(Mutex::new(Vec::new())),
        }));
        snd.wb(0xFF26, 0x80); // APU on — also zeroes the 1 MHz grid phase
        snd.wb(0xFF11, 0x00); // NR11: duty 0
        snd.wb(0xFF12, 0xF8); // NR12: volume 15, no envelope, DAC on
        snd.wb(0xFF13, 0xFC); // NR13/NR14: frequency $7FC -> period 16 T
        snd.wb(0xFF14, 0x87); // trigger
        assert_eq!(snd.channel1.period, 16);
        assert_eq!(snd.channel1.delay, 16 + SQUARE_TRIGGER_DELAY);
        snd
    }

    /// A Game Boy Advance latches the reload **two T-cycles earlier** than a
    /// CPU CGB D/E does — [`SQUARE_RELOAD_LATCH_AGB`].
    ///
    /// The same timer as `a_frequency_write_just_after_an_expiry_redoes_that_reload`
    /// above, so the two tests differ in exactly one thing: a write 4 T past
    /// the expiry at T=38 is inside a CGB D/E's window (6 T) and outside an
    /// AGB's (4 T), so the CGB re-does the reload and the AGB lets the old
    /// countdown finish. That is the whole of SameSuite
    /// `channel_1_freq_change_timing-A` versus `-cgbDE`, neither of which
    /// `cargo test` runs.
    ///
    /// `Model::Auto` never picks an AGB, so nothing here reaches an ordinary
    /// game: the flag is only set for a machine the user asked for by name.
    #[test]
    fn an_agb_latches_its_frequency_reload_two_cycles_earlier() {
        for (agb, expect_relatch) in [(false, true), (true, false)] {
            let mut snd = ticking_square();
            snd.set_agb(agb);
            snd.channel1.run(0, 42);
            assert_eq!(snd.channel1.since_expiry, 4, "expiry at 38, write at 42");
            assert_eq!(snd.channel1.delay, 12, "old countdown had 12 T left");

            snd.wb(0xFF13, 0x00); // frequency $700 -> period 1024 T
            assert_eq!(snd.channel1.period, 1024);
            let expected = if expect_relatch { 1024 - 4 } else { 12 };
            assert_eq!(
                snd.channel1.delay, expected,
                "agb={agb}: a CGB D/E re-does the reload from the expiry at 38, \
                 an AGB does not and the old 12 T countdown finishes"
            );
        }
    }

    /// The frequency-timer reload latch — [`SQUARE_RELOAD_LATCH`].
    ///
    /// The rule, and both sides of it, are SameSuite
    /// `channel_1_freq_change_timing-cgbDE`'s; nothing in `cargo test` runs
    /// SameSuite, so this is the gate. Numbers below are the rule applied to a
    /// known timer, not a recording of our output: with expiries on a 16 T grid
    /// starting at T=22, a write at T=40 sits 2 T past the expiry at 38 and so
    /// lands inside the window, and the channel's next sample must be a whole
    /// **new** period from 38 — not the 14 T left of the old one.
    #[test]
    fn a_frequency_write_just_after_an_expiry_redoes_that_reload() {
        let mut snd = ticking_square();
        snd.channel1.run(0, 40);
        assert_eq!(snd.channel1.since_expiry, 2, "expiries are 22, 38, 54, …");
        assert_eq!(snd.channel1.delay, 14, "old countdown had 14 T left");

        snd.wb(0xFF13, 0x00); // frequency $700 -> period 1024 T
        assert_eq!(snd.channel1.period, 1024);
        assert_eq!(
            snd.channel1.delay,
            1024 - 2,
            "the next sample must be one whole new period from the expiry at \
             38, i.e. at 1062"
        );
    }

    /// The other side of the same constant: a write that misses the window
    /// leaves the running countdown alone, and the new period only applies
    /// from the *next* expiry. This is the plain documented rule, and it is
    /// what all but a few T-cycles of every frequency write does.
    #[test]
    fn a_frequency_write_outside_the_latch_leaves_the_countdown_alone() {
        let mut snd = ticking_square();
        snd.channel1.run(0, 46);
        assert_eq!(snd.channel1.since_expiry, 8);
        assert_eq!(snd.channel1.delay, 8);

        snd.wb(0xFF13, 0x00);
        assert_eq!(snd.channel1.period, 1024);
        assert_eq!(snd.channel1.delay, 8, "the expiry at 54 still happens");
    }

    /// A trigger loads the frequency timer itself, so there is no just-expired
    /// reload behind it for the latch to edit — however close the write is.
    ///
    /// This is not hypothetical tidiness: `since_expiry` exists *because* of
    /// it. Inferring "time since the last expiry" as `period - delay` reads 2 T
    /// here, inside the window, and retimes a channel that has never expired —
    /// which is exactly where SameSuite `channel_1_freq_change` /
    /// `channel_2_freq_change` write NRx3, and both go red on it.
    #[test]
    fn the_latch_does_not_reach_back_past_a_trigger() {
        let mut snd = ticking_square();
        snd.channel1.run(0, 8);
        assert_eq!(snd.channel1.delay, 14, "first expiry is still at 22");
        assert_eq!(
            snd.channel1.period - snd.channel1.delay,
            2,
            "…and `period - delay` would call that 2 T past an expiry"
        );

        snd.wb(0xFF13, 0x00);
        assert_eq!(snd.channel1.period, 1024);
        assert_eq!(
            snd.channel1.delay, 14,
            "the trigger's own countdown must run out first"
        );
    }

    /// Extra length clocking, on each CPU CGB revision we model. Nothing in
    /// `cargo test` runs SameSuite, so this is the gate on
    /// [`CgbRevision`]: the expectations are the rule stated in the header of
    /// `apu/channel_*/channel_*_extra_length_clocking-cgb0B.asm` and the
    /// `CorrectResults` tables of the two `channel_3_extra_length_clocking`
    /// variants, not our own output.
    ///
    /// Each case is one NRx4 write landing in the extra-clocking half of the
    /// frame sequencer, on a channel that is playing with length **disabled**
    /// — the situation the ROMs set up — and the write leaves length disabled
    /// too (`v = $00`, as the ROMs write). Modern silicon must ignore it
    /// entirely; CGB ≤ B must clock it; and on CGB B, CH3 must survive the
    /// clock that reaches zero and die on the next one.
    #[test]
    fn extra_length_clocking_follows_the_cgb_revision() {
        // (revision, wave channel?, starting length, writes, still playing?)
        const CASES: [(CgbRevision, bool, u16, u32, bool); 9] = [
            // CPU CGB C and later: a write that leaves length disabled is inert.
            (CgbRevision::CgbC, false, 1, 1, true),
            (CgbRevision::CgbC, false, 1, 8, true),
            // CPU CGB 0: one clock per write, whatever the write leaves behind.
            (CgbRevision::Cgb0, false, 1, 1, false),
            (CgbRevision::Cgb0, false, 2, 1, true),
            (CgbRevision::Cgb0, false, 2, 2, false),
            (CgbRevision::Cgb0, true, 1, 1, false),
            // CPU CGB B: as CGB 0, but CH3 needs one write more.
            (CgbRevision::CgbB, true, 1, 1, true),
            (CgbRevision::CgbB, true, 1, 2, false),
            (CgbRevision::CgbB, true, 2, 2, true),
        ];
        for (rev, wave, len, writes, want_playing) in CASES {
            let mut snd = Sound::new();
            snd.set_mode(false); // a CGB — the split is a CGB CPU stepping
            snd.set_cgb_revision(rev);
            let rule = snd.extra_length_rule(wave);
            let mut length = LengthCounter::new(if wave { 256 } else { 64 });
            length.value = len;
            let mut playing = true;
            for _ in 0..writes {
                Sound::apply_nrx4_length(&mut length, &mut playing, true, 0x00, rule);
            }
            assert_eq!(
                playing,
                want_playing,
                "{rev:?}, {} channel, length {len}, {writes} write(s): \
                 playing={playing}, hardware says {want_playing}",
                if wave { "wave" } else { "square/noise" }
            );
        }
    }

    /// A DMG has no CGB CPU stepping, so the knob must not reach it — a
    /// mis-set `$GB_CGB_REVISION` may not change how a DMG game plays.
    #[test]
    fn the_revision_quirk_never_reaches_a_dmg() {
        let mut snd = Sound::new();
        snd.set_mode(true);
        snd.set_cgb_revision(CgbRevision::Cgb0);
        let mut length = LengthCounter::new(64);
        length.value = 1;
        let mut playing = true;
        Sound::apply_nrx4_length(
            &mut length,
            &mut playing,
            true,
            0x00,
            snd.extra_length_rule(false),
        );
        assert!(playing, "the CGB 0/B length quirk leaked into DMG mode");
        assert_eq!(length.value, 1);
    }

    /// Cartridge code can put any byte in any APU register in any order, and
    /// the emulator ships with `panic = 'abort'` — a panic here is process
    /// death and a lost battery save. Sweep every register with every value
    /// and run the APU between writes.
    #[test]
    fn no_apu_register_value_can_panic_the_emulator() {
        let mut snd = Sound::new();
        snd.set_player(Box::new(Capture {
            rate: 44100,
            samples: Arc::new(Mutex::new(Vec::new())),
        }));
        for reg in 0xFF10u16..=0xFF3F {
            for v in 0..=255u8 {
                snd.wb(0xFF26, 0x80); // keep the APU powered for the write
                snd.wb(reg, v);
                snd.do_cycle(4);
                snd.catch_up();
                let _ = snd.rb(reg);
                let _ = snd.rb(0xFF76);
                let _ = snd.rb(0xFF77);
                // …and again with the APU powered down.
                snd.wb(0xFF26, 0x00);
                snd.wb(reg, v);
                snd.do_cycle(4);
                let _ = snd.rb(reg);
            }
        }
    }
}
