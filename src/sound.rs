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

struct VolumeEnvelope {
    period: u8,
    goes_up: bool,
    delay: u8,
    initial_volume: u8,
    volume: u8,
}

impl VolumeEnvelope {
    fn new() -> VolumeEnvelope {
        VolumeEnvelope {
            period: 0,
            goes_up: false,
            delay: 0,
            initial_volume: 0,
            volume: 0,
        }
    }

    fn wb(&mut self, a: u16, v: u8) {
        match a {
            0xFF12 | 0xFF17 | 0xFF21 => self.write_nrx2(v, false),
            0xFF14 | 0xFF19 | 0xFF23 if v & 0x80 == 0x80 => {
                self.delay = self.period;
                self.volume = self.initial_volume;
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
            if self.period == 0 && !(!new_up && new_period == 0) {
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

        self.period = new_period;
        self.goes_up = new_up;
        self.initial_volume = v >> 4;
    }

    fn step(&mut self) {
        match self.delay {
            2.. => self.delay -= 1,
            1 => {
                self.delay = self.period;
                if self.goes_up && self.volume < 15 {
                    self.volume += 1;
                } else if !self.goes_up && self.volume > 0 {
                    self.volume -= 1;
                }
            }
            _ => {}
        }
    }

    fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.u8(self.period);
        w.bool(self.goes_up);
        w.u8(self.delay);
        w.u8(self.initial_volume);
        w.u8(self.volume);
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
    sweep_period: u8,
    sweep_shift: u8,
    sweep_decrease: bool,
    sweep_delay: u8,
    sweep_freq: u16,
    sweep_enabled: bool,
    /// A sweep calculation has run in negate mode since the last trigger —
    /// clearing NR10's negate bit while this is set disables the channel.
    sweep_negate_used: bool,
    blip: BlipBuf,
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
            sweep_period: 0,
            sweep_shift: 0,
            sweep_decrease: false,
            sweep_delay: 0,
            sweep_freq: 0,
            sweep_enabled: false,
            sweep_negate_used: false,
            blip,
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

    fn run(&mut self, start_time: u32, end_time: u32) {
        if !self.enabled || self.period == 0 {
            if self.last_amp != 0 {
                self.blip.add_delta(start_time, -self.last_amp);
                self.last_amp = 0;
                self.delay = 0;
            }
        } else {
            let mut time = start_time + self.delay;
            let pattern = DUTY_PATTERN[self.duty as usize];
            let vol = self.volume.volume as i32;

            // At each frequency-timer expiry the duty counter advances and
            // the channel latches the bit at the new position. (`delay` is
            // time-to-next-expiry, which a trigger reloads.)
            while time < end_time {
                self.duty_pos = (self.duty_pos + 1) & 7;
                self.duty_out = pattern[self.duty_pos as usize] != 0;
                let amp = if self.duty_out { vol } else { -vol };
                if amp != self.last_amp {
                    self.blip.add_delta(time, amp - self.last_amp);
                    self.last_amp = amp;
                }

                time += self.period;
            }

            self.delay = time - end_time;
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
        self.enabled = true;
        self.calculate_period();
        // Hardware reloads the frequency timer on trigger, so the first duty
        // step is a whole period away — SameSuite `channel_N_delay` measures
        // that delay as (sample length + 2) M-cycles and fails a channel that
        // steps immediately. The duty *position* is deliberately not reset.
        let pad = SQUARE_TRIGGER_ALIGN.wrapping_sub(grid_phase) & 3;
        self.delay = self.period + SQUARE_TRIGGER_DELAY + pad;
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
        if self.sweep_shift != 0 {
            let _ = self.calc_sweep_freq();
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
                let new_freq = self.calc_sweep_freq();
                if new_freq < 2048 && self.sweep_shift != 0 {
                    self.sweep_freq = new_freq;
                    self.freq = new_freq;
                    self.calculate_period();
                    let _ = self.calc_sweep_freq();
                }
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
        w.u8(self.sweep_period);
        w.u8(self.sweep_shift);
        w.bool(self.sweep_decrease);
        w.u8(self.sweep_delay);
        w.u16(self.sweep_freq);
        w.bool(self.sweep_enabled);
        w.bool(self.sweep_negate_used);
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
        self.sweep_period = r.u8()?;
        self.sweep_shift = r.u8()?;
        self.sweep_decrease = r.bool()?;
        self.sweep_delay = r.u8()?;
        self.sweep_freq = r.u16()?;
        self.sweep_enabled = r.bool()?;
        self.sweep_negate_used = r.bool()?;
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

    fn run(&mut self, start_time: u32, end_time: u32) {
        if !self.enabled || !self.dac_enabled || self.period == 0 {
            if self.last_amp != 0 {
                self.blip.add_delta(start_time, -self.last_amp);
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
                    self.blip.add_delta(time, amp - self.last_amp);
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
const NOISE_TRIGGER_DELAY: u32 = 0;

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
            last_amp: 0,
        }
    }

    fn calculate_period(&mut self) {
        let divisor = if self.divisor == 0 {
            8
        } else {
            self.divisor as u32 * 16
        };
        self.period = divisor << self.shift;
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

    fn run(&mut self, start_time: u32, end_time: u32) {
        if !self.enabled {
            if self.last_amp != 0 {
                self.blip.add_delta(start_time, -self.last_amp);
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
                    self.blip.add_delta(time, amp - self.last_amp);
                    self.last_amp = amp;
                }

                time += self.period;
            }

            self.delay = time - end_time;
        }
    }

    fn trigger(&mut self) {
        self.enabled = true;
        self.lfsr = 0x7FFF;
        self.calculate_period();
        // The noise frequency is built from two counters — the NR43 divisor
        // and the shift stage above it — and a trigger reloads only the
        // divisor, leaving the shift stage half-way through. The first LFSR
        // step therefore lands at `divisor + (period - divisor)/2`, not at a
        // full period. SameSuite `channel_4_delay` measures this across three
        // shift settings at a fixed divisor (2/4/8 M-cycle samples) and the
        // lateness of a plain full-period reload grows as 0/1/3 M-cycles,
        // which is exactly that halved shift stage.
        let divisor = if self.divisor == 0 {
            8
        } else {
            self.divisor as u32 * 16
        };
        self.delay = divisor + (self.period - divisor) / 2 + NOISE_TRIGGER_DELAY;
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
    /// Position of the APU clock on its free-running 1 MHz grid (`0..=3` base
    /// T-cycles). Kept separately from `time`, which `do_output` rebases by
    /// `output_period` (24344 T at 44.1 kHz — not a multiple of 4, so `time`
    /// cannot carry the phase). See [`SQUARE_TRIGGER_ALIGN`].
    grid_phase: u32,
    /// Reusable mixing scratch (see [`Mixdown`]).
    mixdown: Mixdown,
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
            channel1: SquareChannel::new(ch1_blip),
            channel2: SquareChannel::new(ch2_blip),
            channel3: WaveChannel::new(ch3_blip),
            channel4: NoiseChannel::new(ch4_blip),
            left_volume: 7,        // Max volume
            right_volume: 7,       // Max volume
            channel_to_left: 0xF,  // All channels to left (only lower 4 bits)
            channel_to_right: 0xF, // All channels to right (only lower 4 bits)
            frame_seq_counter: 0,
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
            mixdown: Mixdown::default(),
        }
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
    ) {
        let was_enabled = length.enabled;
        let enable = v & 0x40 == 0x40;
        let trigger = v & 0x80 == 0x80;
        length.enabled = enable;
        if first_half && !was_enabled && enable && length.value > 0 {
            length.value -= 1;
            if length.value == 0 && !trigger {
                *channel_enabled = false;
            }
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
    pub fn do_cycle(&mut self, cycles: u32) {
        if self.player.is_none() {
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
        self.grid_phase = (self.grid_phase + cycles) & 3;

        if self.time >= self.output_period {
            self.do_output();
        }
    }

    fn do_output(&mut self) {
        self.run();

        // End frame for all channels
        self.channel1.blip.end_frame(self.output_period);
        self.channel2.blip.end_frame(self.output_period);
        self.channel3.blip.end_frame(self.output_period);
        self.channel4.blip.end_frame(self.output_period);

        // Adjust timing
        self.time -= self.output_period;
        self.next_time -= self.output_period;
        self.prev_time -= self.output_period;

        self.play_samples();
    }

    /// One DIV-APU event: the 512 Hz tick that drives length, sweep and
    /// envelope. Performs the action for the *current* step, then advances.
    fn frame_seq_step(&mut self) {
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
        if self.frame_seq_counter == 7 {
            self.channel1.volume.step();
            self.channel2.volume.step();
            self.channel4.volume.step();
        }

        self.frame_seq_counter = (self.frame_seq_counter + 1) % 8;
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
                self.channel1.freq = (self.channel1.freq & 0x700) | (v as u16);
                self.channel1.calculate_period();
            }
            0xFF14 => {
                self.channel1.freq =
                    (self.channel1.freq & 0xFF) | (((v & 0x7) as u16) << 8);
                self.channel1.calculate_period();
                let fh = self.frame_seq_counter % 2 == 1;
                Self::apply_nrx4_length(
                    &mut self.channel1.length,
                    &mut self.channel1.enabled,
                    fh,
                    v,
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
                self.channel2.freq = (self.channel2.freq & 0x700) | (v as u16);
                self.channel2.calculate_period();
            }
            0xFF19 => {
                self.channel2.freq =
                    (self.channel2.freq & 0xFF) | (((v & 0x7) as u16) << 8);
                self.channel2.calculate_period();
                let fh = self.frame_seq_counter % 2 == 1;
                Self::apply_nrx4_length(
                    &mut self.channel2.length,
                    &mut self.channel2.enabled,
                    fh,
                    v,
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
                let fh = self.frame_seq_counter % 2 == 1;
                Self::apply_nrx4_length(
                    &mut self.channel3.length,
                    &mut self.channel3.enabled,
                    fh,
                    v,
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
                let fh = self.frame_seq_counter % 2 == 1;
                Self::apply_nrx4_length(
                    &mut self.channel4.length,
                    &mut self.channel4.enabled,
                    fh,
                    v,
                );
                self.channel4.volume.wb(a, v);
                if v & 0x80 == 0x80 {
                    self.channel4.trigger();
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
                    self.frame_seq_counter = u8::from(self.div_apu_bit_set());
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
///   the captain's session: `emulate_ms` 17.93 against a 16.743 ms budget,
///   97.6 % of it not executing, the frontend permanently in catch-up, and the
///   display down to 6 fps. The frontends already pace to 59.7275 Hz with a
///   deadline accumulator; a second clock inside the emulator can only fight it.
/// * **Instrumentation.** Underruns, held/dropped counts and depth min/max are
///   counted so buffer sizing is measured, not argued. `GB_AUDIO_STATS=1`
///   logs a line every ~5 s to stderr.
#[cfg(any(all(not(target_arch = "wasm32"), feature = "audio"), test))]
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
pub mod host_status {
    use std::sync::atomic::{AtomicU64, Ordering};
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
    static STREAM_ERRORS: AtomicU64 = AtomicU64::new(0);
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
            stream_errors: STREAM_ERRORS.load(Ordering::Relaxed),
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
    pub fn set_failed(reason: impl std::fmt::Display) {
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
    pub fn note_stream_error(err: impl std::fmt::Display) {
        STREAM_ERRORS.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut g) = STATE.try_lock() {
            g.1 = Some(err.to_string());
        }
    }

    /// Serialises the tests that mutate this global (here and in
    /// `telemetry`), which `cargo test` otherwise runs in parallel.
    #[cfg(test)]
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
        for frame in output.chunks_mut(channels) {
            let (left, right) = q.next_sample();
            for (i, sample) in frame.iter_mut().enumerate() {
                *sample = T::from_sample(if i % 2 == 0 { left } else { right });
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
            // cost the captain 17.6 ms of every 16.743 ms frame.
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
    use super::{AudioPlayer, Sound, CLOCKS_PER_SECOND};
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
}
