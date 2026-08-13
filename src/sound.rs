use blip_buf::BlipBuf;

const WAVE_PATTERN: [[i32; 8]; 4] = [
    [-1, -1, -1, -1, 1, -1, -1, -1],
    [-1, -1, -1, -1, 1, 1, -1, -1],
    [-1, -1, 1, 1, 1, 1, -1, -1],
    [1, 1, 1, 1, -1, -1, 1, 1],
];
// Game Boy CPU runs at 4.194304 MHz
const CLOCKS_PER_SECOND: u32 = 4194304;
const CLOCKS_PER_FRAME: u32 = CLOCKS_PER_SECOND / 512;
const OUTPUT_SAMPLE_COUNT: usize = 2000;
const OUTPUT_SAMPLE_COUNT_U32: u32 = 2000;
const SAMPLE_RATE: u32 = 44100;

pub trait AudioPlayer: Send {
    fn play(&mut self, left_channel: &[f32], right_channel: &[f32]);
    fn samples_rate(&self) -> u32;
    fn underflowed(&self) -> bool;
    /// Hint from the frontend that emulation is deliberately running faster
    /// than real time (fast-forward). Players that pace the emulator against
    /// the audio clock (the cpal high-watermark throttle) must NOT wait in
    /// this mode — the documented fast-forward contract is "drop samples
    /// rather than pitch-shift", and waiting would silently force 1x speed.
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
            0xFF12 | 0xFF17 | 0xFF21 => {
                self.period = v & 0x7;
                self.goes_up = v & 0x8 == 0x8;
                self.initial_volume = v >> 4;
                self.volume = self.initial_volume;
            }
            0xFF14 | 0xFF19 | 0xFF23 if v & 0x80 == 0x80 => {
                self.delay = self.period;
                self.volume = self.initial_volume;
            }
            _ => (),
        }
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

struct SquareChannel {
    enabled: bool,
    dac_enabled: bool,
    volume: VolumeEnvelope,
    length: LengthCounter,
    duty: u8,
    phase: u8,
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
            phase: 1,
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
            let pattern = WAVE_PATTERN[self.duty as usize];
            let vol = self.volume.volume as i32;

            while time < end_time {
                let amp = vol * pattern[self.phase as usize];
                if amp != self.last_amp {
                    self.blip.add_delta(time, amp - self.last_amp);
                    self.last_amp = amp;
                }

                time += self.period;
                self.phase = (self.phase + 1) % 8;
            }

            self.delay = time - end_time;
        }
    }

    fn trigger(&mut self) {
        self.enabled = true;
        self.calculate_period();
        self.delay = 0; // No initial delay for square channels
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
        w.u8(self.phase);
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
        self.phase = r.u8()?;
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
    blip: BlipBuf,
    last_amp: i32,
}

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
            blip,
            last_amp: 0,
        }
    }

    fn calculate_period(&mut self) {
        if self.freq > 2047 {
            self.period = 0;
        } else {
            self.period = (2048 - self.freq as u32) * 2;
        }
    }

    fn run(&mut self, start_time: u32, end_time: u32) {
        if !self.enabled || !self.dac_enabled || self.period == 0 {
            if self.last_amp != 0 {
                self.blip.add_delta(start_time, -self.last_amp);
                self.last_amp = 0;
                self.delay = 0;
            }
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

            while time < end_time {
                let byte = self.waveram[self.wave_pos as usize / 2];
                let sample = if self.wave_pos & 1 == 0 {
                    byte >> 4
                } else {
                    byte & 0xF
                };

                // Shift left by 2 for 4x amplitude
                let amp = ((sample << 2) >> volshift) as i32;

                if amp != self.last_amp {
                    self.blip.add_delta(time, amp - self.last_amp);
                    self.last_amp = amp;
                }

                time += self.period;
                self.wave_pos = (self.wave_pos + 1) % 32;
            }

            self.delay = time - end_time;
        }
    }

    fn trigger(&mut self) {
        self.enabled = true;
        self.wave_pos = 0;
        self.calculate_period();
        self.delay = 4; // Initial delay for wave channel
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
        self.last_amp = r.i32()?;
        Ok(())
    }
}

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
        self.delay = 0;
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

    pub fn do_cycle(&mut self, cycles: u32) {
        if self.player.is_none() {
            return;
        }

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

    fn run(&mut self) {
        // Run frame sequencer
        while self.next_time <= self.time {
            // Run channels up to frame boundary
            self.channel1.run(self.prev_time, self.next_time);
            self.channel2.run(self.prev_time, self.next_time);
            self.channel3.run(self.prev_time, self.next_time);
            self.channel4.run(self.prev_time, self.next_time);

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
            self.prev_time = self.next_time;
            self.next_time += CLOCKS_PER_FRAME;
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

        let mut left_buf = vec![0f32; OUTPUT_SAMPLE_COUNT];
        let mut right_buf = vec![0f32; OUTPUT_SAMPLE_COUNT];

        let mut ch1_buf = vec![0i16; OUTPUT_SAMPLE_COUNT];
        let mut ch2_buf = vec![0i16; OUTPUT_SAMPLE_COUNT];
        let mut ch3_buf = vec![0i16; OUTPUT_SAMPLE_COUNT];
        let mut ch4_buf = vec![0i16; OUTPUT_SAMPLE_COUNT];

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
            player.play(&left_buf, &right_buf);
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
            // Wave RAM
            0xFF30..=0xFF3F => {
                if !self.channel3.enabled {
                    self.channel3.waveram[a as usize - 0xFF30]
                } else {
                    // Return current sample when playing
                    self.channel3.waveram[self.channel3.wave_pos as usize / 2]
                }
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
                self.channel1.volume.wb(a, v);
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
                    self.channel1.trigger();
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
                self.channel2.volume.wb(a, v);
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
                    self.channel2.trigger();
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
                    self.channel3.trigger();
                    self.channel3.enabled &= self.channel3.dac_enabled;
                }
            }
            // Noise channel
            0xFF20 => self.channel4.length.wb(a, v),
            0xFF21 => {
                self.channel4.volume.wb(a, v);
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
            // Wave RAM
            0xFF30..=0xFF3F => {
                if !self.channel3.enabled {
                    self.channel3.waveram[a as usize - 0xFF30] = v;
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
                }
                if !self.on && will_be_on {
                    // Powering on resets the frame sequencer so its NEXT step
                    // is 0 (length-clocking) — but the underlying 8192-cycle
                    // divider is DIV-driven and keeps running, which is why
                    // `next_time` is deliberately left alone (dmg_sound
                    // 07-len sweep period sync: "Powering up APU MODs next
                    // frame time with 8192" / "resets 128 Hz sweep divider").
                    self.frame_seq_counter = 0;
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
/// * **High watermark instead of hard drop.** The producer consults
///   `over_high_water()` and *waits* for the callback to drain (audio-crystal-
///   as-master-clock, the community's "sync to audio" advice; see Near's
///   dynamic-rate-control article). Samples are dropped only if the consumer
///   is provably stalled, to bound latency.
/// * **Instrumentation.** Underruns, held/dropped counts and depth min/max are
///   counted so buffer sizing is measured, not argued. `GB_AUDIO_STATS=1`
///   logs a line every ~5 s to stderr.
#[cfg(any(all(not(target_arch = "wasm32"), feature = "audio"), test))]
pub mod sample_queue {
    use std::collections::VecDeque;

    /// Producer block size: `Sound::play_samples` always hands over exactly
    /// this many samples (blip_buf's OUTPUT_SAMPLE_COUNT), ~45 ms at 44.1 kHz.
    pub const BLOCK: usize = super::OUTPUT_SAMPLE_COUNT;
    /// The callback drains only once this many samples are queued (~159 ms at
    /// 44.1 kHz): the jitter cushion. Sized by the simulation in
    /// `queue_tests`, not picked: samples arrive in whole 45 ms blocks, so the
    /// *effective* floor is START_FILL - BLOCK, and the largest silently
    /// absorbed production stall is (START_FILL - BLOCK)/rate ≈ 113 ms — which
    /// rides out a 90 ms stall plus 8 ms of scheduling jitter with margin.
    /// 3*BLOCK measurably underruns in that profile (4 underruns / 5 min in
    /// the sim); this value measures zero.
    pub const START_FILL: usize = 7 * BLOCK / 2;
    /// Producer waits above this (~272 ms at 44.1 kHz). Queue latency is
    /// bounded by this plus the device's own buffer.
    pub const HIGH_WATER: usize = 6 * BLOCK;

    #[derive(Default, Clone, Copy)]
    pub struct QueueStats {
        /// Times the consumer ran dry after playback had started.
        pub underruns: u64,
        /// Samples substituted with the decaying hold value (priming included).
        pub held: u64,
        /// Samples discarded because the consumer was stalled at the high
        /// watermark. Zero in healthy operation.
        pub dropped: u64,
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

        /// Last resort when the consumer is stalled: bound latency by
        /// discarding the oldest samples down to the high watermark.
        pub fn drop_to_high_water(&mut self) {
            let n = self.over_high_water();
            self.queue.drain(0..n);
            self.stats.dropped += n as u64;
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

#[cfg(all(not(target_arch = "wasm32"), feature = "audio"))]
pub mod cpal_audio {
    use super::sample_queue::{SampleQueue, HIGH_WATER, START_FILL};
    use super::AudioPlayer;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, Sample};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    pub struct CpalPlayer {
        queue: Arc<Mutex<SampleQueue>>,
        sample_rate: u32,
        /// Callback count observed at the previous `play()`: if it has not
        /// moved while we are over the watermark, the consumer is stalled and
        /// waiting on it would only freeze the emulator.
        last_seen_callbacks: u64,
        /// Fast-forward: the frontend is deliberately outrunning real time, so
        /// the watermark policy switches from *wait* to *drop-oldest* (waiting
        /// would silently clamp the game back to 1x).
        fast_forward: bool,
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

        pub fn new() -> Option<(CpalPlayer, cpal::Stream)> {
            let device = match cpal::default_host().default_output_device() {
                Some(e) => e,
                None => {
                    #[cfg(debug_assertions)]
                    eprintln!("No default output device found");
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
                            #[cfg(debug_assertions)]
                            eprintln!("Failed to get supported configs: {}", err);
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
                            #[cfg(debug_assertions)]
                            eprintln!("No supported F32 audio configuration found");
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
            let config: cpal::StreamConfig = selected_config.into();

            let err_fn = |err| {
                #[cfg(debug_assertions)]
                eprintln!("An error occurred on the output audio stream: {}", err);
            };

            let shared_queue =
                Arc::new(Mutex::new(SampleQueue::new(START_FILL, HIGH_WATER)));
            let stream_buffer = shared_queue.clone();

            let player = CpalPlayer {
                queue: shared_queue,
                sample_rate: config.sample_rate.0,
                last_seen_callbacks: 0,
                fast_forward: false,
                stats_enabled: std::env::var("GB_AUDIO_STATS").is_ok(),
                last_stats_log: Instant::now(),
            };

            let stream = match sample_format {
                cpal::SampleFormat::F32 => device.build_output_stream(
                    &config,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
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
                        #[cfg(debug_assertions)]
                        eprintln!("Failed to play audio stream: {}", e);
                        return None;
                    }
                    Some((player, s))
                }
                Err(e) => {
                    #[cfg(debug_assertions)]
                    eprintln!("Failed to build audio stream: {}", e);
                    None
                }
            }
        }
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
        fn play(&mut self, left_channel: &[f32], right_channel: &[f32]) {
            let callbacks_before;
            {
                let mut q = match self.queue.lock() {
                    Ok(q) => q,
                    Err(_) => return,
                };
                q.push(left_channel, right_channel);
                callbacks_before = q.stats.callbacks;
            }

            // Audio-clock throttle. Above the high watermark the emulator is
            // running ahead of the sound card; *wait* for the callback to
            // drain instead of dropping samples. This makes the audio crystal
            // the master clock whenever the frame timer runs fast — the
            // standard emulator answer to producer/consumer clock drift (see
            // docs/audio.md). Waiting is pointless if the consumer is stalled
            // (stream error, device gone): detect that by the callback counter
            // not advancing, and bound latency by dropping instead.
            //
            // Fast-forward opts out of waiting entirely: the overrun is
            // intentional, and the documented contract is "drop samples
            // rather than pitch-shift" — waiting would clamp the game to 1x.
            if self.fast_forward {
                if let Ok(mut q) = self.queue.lock() {
                    q.drop_to_high_water();
                }
                return;
            }
            let deadline = Instant::now() + Duration::from_millis(150);
            loop {
                let (over, callbacks_now) = match self.queue.lock() {
                    Ok(q) => (q.over_high_water(), q.stats.callbacks),
                    Err(_) => return,
                };
                if over == 0 {
                    break;
                }
                let stalled = callbacks_now == self.last_seen_callbacks
                    && callbacks_now == callbacks_before;
                if stalled || Instant::now() >= deadline {
                    if let Ok(mut q) = self.queue.lock() {
                        q.drop_to_high_water();
                    }
                    break;
                }
                // ~1 ms ≈ 44 samples at 44.1 kHz; the wait to drain one block
                // is tens of iterations.
                std::thread::sleep(Duration::from_millis(1));
            }
            if let Ok(q) = self.queue.lock() {
                self.last_seen_callbacks = q.stats.callbacks;
            }

            if self.stats_enabled
                && self.last_stats_log.elapsed() >= Duration::from_secs(5)
            {
                self.last_stats_log = Instant::now();
                if let Ok(mut q) = self.queue.lock() {
                    let s = q.stats;
                    eprintln!(
                        "audio: depth {}..{} (now {}), underruns {}, held {}, \
                         dropped {}, pushed {}, consumed {}, callbacks {}",
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

        fn set_fast_forward(&mut self, enabled: bool) {
            self.fast_forward = enabled;
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
    use super::sample_queue::{SampleQueue, BLOCK, HIGH_WATER, START_FILL};

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
    /// are pushed in whole 2000-sample blocks, exactly as `play_samples`
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

        let mut q = SampleQueue::new(START_FILL, HIGH_WATER);
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

    /// Healthy case: matched rates, realistic OS jitter (up to 8 ms late
    /// frames), a 90 ms production stall every 5 s (a slow terminal draw).
    /// Requirement: ZERO underruns over five minutes, depth bounded (no drops
    /// would occur), i.e. the cushion rides through every hiccup silently.
    #[test]
    fn no_underruns_with_realistic_jitter_and_stalls() {
        let r = simulate(300.0, 512, 8.0, 5.0, 90.0);
        assert_eq!(
            r.queue.stats.underruns, 0,
            "underruns in the healthy profile — cushion too small"
        );
        assert!(
            r.post_prime_depth_max <= HIGH_WATER + BLOCK,
            "depth {} exceeded high watermark + one block — latency unbounded",
            r.post_prime_depth_max
        );
        assert_eq!(r.queue.stats.dropped, 0);
        // The cushion must actually be exercised: depth should dip well below
        // START_FILL during stalls (otherwise the test proves nothing).
        assert!(
            r.post_prime_depth_min < START_FILL,
            "stalls never stressed the queue (min depth {})",
            r.post_prime_depth_min
        );
    }

    /// A stall longer than the cushion (~136 ms at 44.1 kHz) MUST underrun —
    /// this pins the meaning of START_FILL: it is the largest production gap
    /// that stays inaudible. If this test starts passing with 0 underruns,
    /// START_FILL grew and the latency story changed; revisit docs/audio.md.
    #[test]
    fn stall_longer_than_cushion_underruns_once_and_recovers() {
        let r = simulate(60.0, 512, 0.0, 20.0, 400.0);
        assert!(
            r.queue.stats.underruns >= 1,
            "a 400 ms stall cannot be absorbed by a {} ms cushion",
            START_FILL as f64 * 1000.0 / RATE
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

    /// Producer overrun: pushing far past the high watermark then dropping
    /// bounds latency and counts every discarded sample.
    #[test]
    fn drop_to_high_water_bounds_latency() {
        let mut q = SampleQueue::new(START_FILL, HIGH_WATER);
        let block = vec![0.1f32; BLOCK];
        for _ in 0..10 {
            q.push(&block, &block);
        }
        assert_eq!(q.over_high_water(), 10 * BLOCK - HIGH_WATER);
        q.drop_to_high_water();
        assert_eq!(q.len(), HIGH_WATER);
        assert_eq!(q.stats.dropped, (10 * BLOCK - HIGH_WATER) as u64);
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
