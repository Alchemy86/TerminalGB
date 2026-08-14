//! The host half of the chiptune renderer: this repository's emulated APU,
//! wired up as a [`terminalgb_chiptune::Apu`].
//!
//! Everything about *music* — notes, durations, `.vgm`, the register script,
//! the WAV encoder — lives in the dependency-free `terminalgb-chiptune` crate.
//! This file is the sixty lines that hand it a real sound chip, and it is
//! deliberately the only place the two meet.
//!
//! # Why this is not a synthesiser
//!
//! [`Sound`] is the APU the accuracy suites are scored against, unmodified and
//! not reimplemented: SameSuite's APU group at **64 of 69** and Blargg's
//! `dmg_sound` at **12 of 12**, both judged by the test ROMs' own verdicts (see
//! `docs/chiptune.md` for how to re-measure). So the sweep's overflow check,
//! the noise channel's LFSR, the envelope's two-stage counter and the DMG's
//! wave-RAM window are the *measured* hardware behaviours, not a plausible
//! model of them. That is the entire point of rendering here rather than in a
//! tone generator.
//!
//! # Determinism
//!
//! The APU is stepped in integer T-cycles from a track whose times are
//! integers, and this file pins the one piece of ambient state [`Sound`] would
//! otherwise read from the environment (`$GB_CGB_REVISION`). Nothing here
//! reads a clock. The same track therefore renders to byte-identical samples,
//! which `testharness/chiptune.rs` asserts.

use std::sync::{Arc, Mutex};

use terminalgb_chiptune::{Apu, Model, Track};

use crate::sound::{AudioPlayer, CgbRevision, Sound};

/// Everything a render needs to know about the machine, as opposed to the
/// music.
#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    pub sample_rate: u32,
    /// Which console's APU semantics to use. Overridden by the track's own
    /// `model` line when one is present, because that is the composer being
    /// explicit.
    pub model: Model,
    /// Which CGB CPU revision's APU quirks to model. Pinned rather than read
    /// from `$GB_CGB_REVISION` so a render is reproducible; ignored on a DMG,
    /// where `Sound` forces the modern rule anyway.
    pub cgb_revision: CgbRevision,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            sample_rate: 44_100,
            model: Model::Dmg,
            cgb_revision: CgbRevision::CgbC,
        }
    }
}

/// Records every sample the APU produces. Device-free — the same shape the
/// offline audio harness has used since it was written.
struct CapturePlayer {
    rate: u32,
    buf: Arc<Mutex<Vec<(f32, f32)>>>,
}

impl AudioPlayer for CapturePlayer {
    fn play(&mut self, left: &[f32], right: &[f32]) {
        let mut b = self.buf.lock().unwrap();
        for i in 0..left.len().min(right.len()) {
            b.push((left[i], right[i]));
        }
    }
    fn samples_rate(&self) -> u32 {
        self.rate
    }
    fn underflowed(&self) -> bool {
        false
    }
}

/// The emulated Game Boy sound chip, ready to be driven by
/// [`terminalgb_chiptune::render`].
pub struct EmulatedApu {
    sound: Sound,
    buf: Arc<Mutex<Vec<(f32, f32)>>>,
}

impl EmulatedApu {
    pub fn new(opts: RenderOptions) -> EmulatedApu {
        let mut sound = Sound::new();
        let buf = Arc::new(Mutex::new(Vec::new()));
        // The APU is inert without a player — `Sound::do_cycle` returns
        // immediately when none is installed, so a renderer that forgot this
        // would produce silence and look like a bug in the music.
        sound.set_player(Box::new(CapturePlayer {
            rate: opts.sample_rate,
            buf: buf.clone(),
        }));
        sound.set_mode(opts.model == Model::Dmg);
        sound.set_cgb_revision(opts.cgb_revision);
        EmulatedApu { sound, buf }
    }

    /// Read an APU register back exactly as the CPU would — `NR52`'s
    /// channel-enable flip-flops included. Useful for showing a composer that
    /// a length counter really did switch a channel off.
    pub fn read(&self, addr: u16) -> u8 {
        self.sound.rb(addr)
    }
}

impl Apu for EmulatedApu {
    fn write(&mut self, addr: u16, value: u8) {
        // Synchronise before the access, exactly as `MemoryManagementUnit`
        // does on every APU register touch. Without it the chip's length,
        // sweep and envelope state is up to a ~45 ms block stale at the moment
        // the register is written, which is what made every timing-sensitive
        // `dmg_sound` subtest fail before catch-up-on-access existed.
        self.sound.catch_up();
        self.sound.wb(addr, value);
    }

    fn step(&mut self, t_cycles: u32) {
        self.sound.do_cycle(t_cycles);
    }

    fn drain(&mut self, out: &mut Vec<(f32, f32)>) {
        let mut b = self.buf.lock().unwrap();
        out.append(&mut b);
    }
}

/// Render a track to stereo samples on a fresh chip. The convenience wrapper
/// most callers want.
pub fn render(track: &Track, opts: RenderOptions) -> Vec<(f32, f32)> {
    let opts = RenderOptions {
        model: track.model,
        ..opts
    };
    let mut apu = EmulatedApu::new(opts);
    terminalgb_chiptune::render(track, &mut apu, opts.sample_rate)
}

/// Render straight to a `.wav` file.
pub fn render_to_wav(
    track: &Track,
    opts: RenderOptions,
    path: &std::path::Path,
) -> std::io::Result<usize> {
    let samples = render(track, opts);
    terminalgb_chiptune::wav::write(path, &samples, opts.sample_rate)?;
    Ok(samples.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminalgb_chiptune::{score, CLOCK_HZ};

    fn peak(s: &[(f32, f32)]) -> f32 {
        s.iter()
            .fold(0f32, |m, &(l, r)| m.max(l.abs()).max(r.abs()))
    }

    /// Count zero crossings on the left channel over the whole buffer and turn
    /// that into a frequency. Crude but sufficient to tell a note from silence
    /// and one octave from another.
    fn dominant_hz(s: &[(f32, f32)], rate: u32) -> f64 {
        let mut crossings = 0usize;
        let mut prev = 0f32;
        for &(l, _) in s {
            if (prev <= 0.0 && l > 0.0) || (prev >= 0.0 && l < 0.0) {
                crossings += 1;
            }
            prev = l;
        }
        crossings as f64 * rate as f64 / (2.0 * s.len() as f64)
    }

    #[test]
    fn a_note_comes_out_at_the_pitch_it_was_written_at() {
        // A4 held for a second. The Game Boy's period register cannot express
        // 440 exactly — it produces 439.84 Hz — and that is what a composer
        // needs to be told, so the check is against the chip's real pitch.
        let t =
            score::compile("tempo 60\nch1 vol 15\nch1 note A4 1/1\ntail 0t\n").unwrap();
        let s = render(&t, RenderOptions::default());
        assert!(peak(&s) > 0.1, "peak {}", peak(&s));
        let hz = dominant_hz(&s[..44_100], 44_100);
        assert!((hz - 439.84).abs() < 4.0, "measured {hz} Hz");
    }

    #[test]
    fn the_same_track_renders_to_byte_identical_samples() {
        let t = score::compile(
            "tempo 140\n\
             ch1 env 15 - 3\nch1 note C4 1/8\nch1 note E4 1/8\nch1 note G4 1/4\n\
             ch4 noise 4 15 4\nch4 hit 1/16\n",
        )
        .unwrap();
        let a = render(&t, RenderOptions::default());
        let b = render(&t, RenderOptions::default());
        assert_eq!(
            terminalgb_chiptune::wav::interleave_i16(&a),
            terminalgb_chiptune::wav::interleave_i16(&b)
        );
        assert!(!a.is_empty());
    }

    #[test]
    fn the_sample_count_matches_the_track_length() {
        let t = score::compile("tempo 120\nch1 note C4 1/4\nend 2s\n").unwrap();
        let s = render(&t, RenderOptions::default());
        assert_eq!(s.len(), 88_200);
        assert_eq!(t.duration, 2 * CLOCK_HZ);
    }

    #[test]
    fn an_empty_track_is_exact_silence_not_an_absent_chip() {
        // The failure this guards against is subtle: with no player installed
        // `Sound::do_cycle` returns immediately, so "no sound" and "no APU"
        // look identical from outside. A powered-off APU must still emit
        // silence at the full sample rate.
        let mut t = Track::new();
        t.duration = CLOCK_HZ / 2;
        let s = render(&t, RenderOptions::default());
        assert_eq!(s.len(), 22_050);
        assert_eq!(peak(&s), 0.0);
    }

    #[test]
    fn the_dmg_and_cgb_length_rules_really_differ() {
        // Load a length counter while the APU is powered *off*. On a DMG the
        // write lands (length counters live outside the powered domain); on a
        // CGB it is ignored. Read NR11 back... no: NRx1's length bits are
        // write-only. Observe it instead the way hardware lets you — through
        // the channel switching itself off when the counter expires.
        let src = "power off\n\
                   ch1 raw NR11 $BF\n\
                   ch1 raw NR52 $80\n\
                   ch1 raw NR50 $77\n\
                   ch1 raw NR51 $FF\n\
                   ch1 raw NR12 $F0\n\
                   ch1 raw NR13 $D6\n\
                   ch1 raw NR14 $C6\n\
                   end 20ms\n";
        let t = terminalgb_chiptune::score::compile(src).unwrap();
        let dmg = EmulatedApu::new(RenderOptions {
            model: Model::Dmg,
            ..Default::default()
        });
        let cgb = EmulatedApu::new(RenderOptions {
            model: Model::Cgb,
            ..Default::default()
        });
        let run = |mut apu: EmulatedApu| {
            terminalgb_chiptune::render(&t, &mut apu, 44_100);
            apu.read(0xFF26) & 0x01
        };
        // The render runs `FLUSH_TAIL_CYCLES` (~62 ms) past the track's end, so
        // the chip is observed ~82 ms in — comfortably after the DMG's 3.9 ms
        // and comfortably before the CGB's 250 ms.
        //
        // DMG: the length load of 63 survived the power-off, so one step of the
        // 64-step counter was left and the channel switched itself off.
        assert_eq!(run(dmg), 0, "DMG: length counter should have expired");
        // CGB: the write while off was ignored, so the trigger reloaded the
        // full 64 steps (250 ms) and the channel is still playing.
        assert_eq!(run(cgb), 1, "CGB: channel should still be enabled");
    }
}
