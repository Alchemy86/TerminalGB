//! Boot a cartridge headlessly and record what its own sound chip plays.
//!
//! This is the honest answer to "play track 5": there is no such interface on a
//! Game Boy. What every cartridge *does* have is a boot sequence that ends on a
//! title screen with the title theme playing, and that costs nothing
//! game-specific to obtain — you run the machine and listen. See
//! [`crate::music`] for where that sits in the range of possible approaches.
//!
//! Two things this module exists to get right, both learned the hard way
//! elsewhere in this repo:
//!
//! * **A machine with no [`AudioPlayer`] installed has no APU at all**, not a
//!   quiet one — `Sound::do_cycle` returns immediately when `player.is_none()`.
//!   So "I captured silence" and "I captured nothing" are different findings and
//!   a capture that cannot tell them apart is worthless. The recorder is
//!   installed before the first `do_cycle` and counts its own calls, so
//!   [`Recording::apu_ran`] answers that question directly.
//! * **Nothing is persisted.** The machine is built with `filepath = None`, so
//!   the MBC has nowhere to write cartridge RAM. Music mode must never leave a
//!   `.sav` beside somebody's ROM, and it must never overwrite a real save with
//!   the pristine RAM of a thirty-second listen. The mapper is still chosen
//!   from header byte `0x147` — `get_mbc` takes the path only for persistence
//!   (see `src/mbc/mod.rs`), so dropping it costs no accuracy.

use crate::cpu::core::Cpu;
use crate::mode::Model;
use crate::sound::AudioPlayer;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Sample rate every capture runs at. 44.1 kHz because that is what a WAV a
/// human will open expects, and because `blip_buf` is retuned to whatever rate
/// the player reports, so pitch is correct at any of them.
pub const RATE: u32 = 44_100;

/// The DMG's frame period, in emulated T-cycles.
const FRAME_TICKS: u64 = 70_224;
/// 59.7275 Hz — one frame per 70224 cycles of a 4.194304 MHz clock.
pub const FPS: f64 = 4_194_304.0 / FRAME_TICKS as f64;

/// Emulated frames that cover `seconds` of real time.
pub fn frames_for(seconds: f64) -> u32 {
    (seconds * FPS).ceil() as u32
}

/// An [`AudioPlayer`] that keeps everything it is handed.
///
/// It also counts `play` calls, which is the only way to distinguish a
/// cartridge that is genuinely silent from a machine whose APU was never
/// installed — both produce an empty-looking result otherwise.
struct Recorder {
    rate: u32,
    buf: Arc<Mutex<Vec<(f32, f32)>>>,
    calls: Arc<AtomicUsize>,
    /// Stop keeping samples once this many are held. The APU hands over 256 at
    /// a time, so a run always overshoots slightly; the cap bounds memory on a
    /// long render and the caller truncates to the exact length it asked for.
    cap: usize,
}

impl AudioPlayer for Recorder {
    fn play(&mut self, left: &[f32], right: &[f32]) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let mut b = match self.buf.lock() {
            Ok(b) => b,
            Err(p) => p.into_inner(),
        };
        if b.len() >= self.cap {
            return;
        }
        let take = left.len().min(right.len());
        b.reserve(take);
        for i in 0..take {
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

/// What one headless listen produced.
pub struct Recording {
    /// Interleaved stereo samples at [`RATE`], exactly as the APU produced them.
    pub samples: Vec<(f32, f32)>,
    pub rate: u32,
    /// Emulated frames actually run.
    pub frames: u32,
    /// `true` if the APU handed over at least one block. `false` means the
    /// sound chip never ran, which is a *bug in the harness*, not a quiet game.
    pub apu_ran: bool,
}

impl Recording {
    pub fn seconds(&self) -> f64 {
        self.samples.len() as f64 / self.rate as f64
    }
}

/// How to run a capture.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Which console to boot as. `Model::Auto` follows the cartridge's own CGB
    /// flag, which is what a person with both consoles would do.
    pub model: Model,
    /// Emulated frames to run.
    pub frames: u32,
    /// Discard this many frames of audio before keeping any. Games open on a
    /// publisher logo in silence; a WAV that starts with four seconds of
    /// nothing reads as a broken export.
    pub skip_frames: u32,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            model: Model::Auto,
            frames: frames_for(30.0),
            skip_frames: 0,
        }
    }
}

/// Boot `rom` and record what it plays.
///
/// Returns `Err` when the bytes are not a cartridge this emulator can map —
/// arbitrary bytes must never abort the process (`panic = 'abort'` makes a
/// panic process death), so the fallible constructor is used deliberately.
pub fn record(rom: &[u8], opts: Options) -> Result<Recording, String> {
    record_with(rom, opts, |_| {})
}

/// One emulated frame, offered to the hook [`record_with`] takes.
///
/// It carries the machine rather than a copy of its screen so a caller can ask
/// for a **save state** as well as a picture: that is what lets music mode start
/// an album at its title screen instead of replaying a minute of intro every
/// time somebody selects it.
pub struct FrameView<'a> {
    /// Emulated frames run so far, 1-based.
    pub number: u32,
    cpu: &'a Cpu<'a>,
}

impl FrameView<'_> {
    /// The machine's RGBA framebuffer, `160 * 144 * 4` bytes.
    pub fn rgba(&self) -> &[u8] {
        self.cpu.memory.gpu.data.as_slice()
    }

    /// The whole machine, as save-state bytes. Restoring one is how a listen
    /// resumes from an exact moment without re-running the boot.
    pub fn save_state(&self) -> Vec<u8> {
        self.cpu.save_state()
    }
}

/// [`record`], with a hook called once per emulated frame with the frame number
/// and the machine's RGBA framebuffer.
///
/// This is how cover art and audio come out of **one** boot rather than two.
/// That is not only cheaper: a second boot is a second machine, and a cartridge
/// that reaches its title screen at a slightly different moment would give a
/// cover that does not belong to the take you are listening to. It also means
/// the picture is harvested from a machine with a working APU, which matters —
/// programs that synchronise on `NR52` stall on a machine that has none (see
/// `testharness/README.md`).
pub fn record_with(
    rom: &[u8],
    opts: Options,
    on_frame: impl FnMut(FrameView<'_>),
) -> Result<Recording, String> {
    record_from(rom, None, opts, on_frame)
}

/// [`record_with`], resuming from save-state bytes rather than from reset.
///
/// `state` is a state this crate produced from the *same* cartridge; a state
/// from another one is refused by [`crate::savestate`] and reported here rather
/// than silently ignored, because a take that quietly started at the wrong
/// moment is worse than one that did not happen.
pub fn record_from(
    rom: &[u8],
    state: Option<&[u8]>,
    opts: Options,
    mut on_frame: impl FnMut(FrameView<'_>),
) -> Result<Recording, String> {
    // `None`: no save file, ever. See the module comment.
    let mut cpu = Cpu::try_new_with_model(rom.to_vec(), None, opts.model)
        .map_err(|e| e.to_string())?;
    if let Some(bytes) = state {
        cpu.load_state(bytes).map_err(|e| e.to_string())?;
    }

    let buf = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    // Keep a little more than asked for: the APU emits in 256-sample blocks, so
    // the tail of a run is short unless the run overshoots.
    let want = (opts.frames as f64 / FPS * RATE as f64).ceil() as usize;
    cpu.memory.enable_audio(Box::new(Recorder {
        rate: RATE,
        buf: buf.clone(),
        calls: calls.clone(),
        cap: want.saturating_add(4096),
    }));

    let mut frames = 0u32;
    // A cartridge that never completes a frame (LCD off forever) must not wedge
    // the run, so the loop is bounded in cycles as well as frames.
    let cycle_cap = (opts.frames as u64 + 64) * FRAME_TICKS + 4 * FRAME_TICKS;
    let mut cycles = 0u64;
    let mut dropped_until = 0usize;
    while frames < opts.frames && cycles < cycle_cap {
        cycles += cpu.do_cycle() as u64;
        if cpu.memory.gpu.updated {
            cpu.memory.gpu.updated = false;
            frames += 1;
            on_frame(FrameView {
                number: frames,
                cpu: &cpu,
            });
            if frames == opts.skip_frames {
                // Everything up to here was the logo; start the keep from the
                // sample count as it stands now.
                dropped_until = buf.lock().map(|b| b.len()).unwrap_or(0);
            }
        }
    }

    let mut samples = match buf.lock() {
        Ok(b) => b.clone(),
        Err(p) => p.into_inner().clone(),
    };
    if dropped_until > 0 && dropped_until <= samples.len() {
        samples.drain(..dropped_until);
    }
    Ok(Recording {
        samples,
        rate: RATE,
        frames,
        apu_ran: calls.load(Ordering::Relaxed) > 0,
    })
}

/// Trim (or leave) a recording so it is at most `seconds` long, and fade the
/// last few milliseconds out.
///
/// The fade is not decoration: a capture ends mid-waveform, and a WAV that stops
/// on a non-zero sample clicks in every player that loops it.
pub fn trim_and_fade(rec: &mut Recording, seconds: f64) {
    let want = (seconds * rec.rate as f64) as usize;
    if rec.samples.len() > want {
        rec.samples.truncate(want);
    }
    fade_out(&mut rec.samples, rec.rate);
}

/// Linear fade over the last 20 ms, in place.
pub fn fade_out(samples: &mut [(f32, f32)], rate: u32) {
    let n = ((rate as f64 * 0.020) as usize).min(samples.len());
    if n == 0 {
        return;
    }
    let start = samples.len() - n;
    for (i, s) in samples[start..].iter_mut().enumerate() {
        let g = 1.0 - (i as f32 + 1.0) / n as f32;
        s.0 *= g;
        s.1 *= g;
    }
}

/// Level summary of a recording — enough to say, honestly, whether anything was
/// heard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Level {
    pub peak: f32,
    pub rms: f64,
    /// Fraction of samples where both channels are below the silence floor.
    pub silent_frac: f64,
}

/// -80 dBFS. Below this a sample is silence for our purposes.
const SILENCE: f32 = 1e-4;

pub fn level(samples: &[(f32, f32)]) -> Level {
    let mut peak = 0f32;
    let mut sumsq = 0f64;
    let mut silent = 0usize;
    for &(l, r) in samples {
        peak = peak.max(l.abs()).max(r.abs());
        sumsq += (l as f64) * (l as f64) + (r as f64) * (r as f64);
        if l.abs() < SILENCE && r.abs() < SILENCE {
            silent += 1;
        }
    }
    let n = samples.len();
    Level {
        peak,
        rms: if n > 0 {
            (sumsq / (2.0 * n as f64)).sqrt()
        } else {
            0.0
        },
        silent_frac: if n > 0 { silent as f64 / n as f64 } else { 1.0 },
    }
}

/// Whether a recording holds something a person would call sound.
///
/// Deliberately generous: it answers "is this cartridge making noise", not "is
/// this music". Telling a *melody* from a sound effect is not something a level
/// meter can do, and pretending otherwise is exactly the overclaim the shelf's
/// wording is built to avoid.
pub fn is_audible(l: Level) -> bool {
    l.peak > 0.01 && l.silent_frac < 0.98
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::testrom;

    #[test]
    fn a_tone_rom_is_captured_at_the_right_pitch_and_level() {
        let rom = testrom::tone_rom();
        let rec = record(
            &rom,
            Options {
                frames: frames_for(1.0),
                ..Options::default()
            },
        )
        .expect("tone ROM maps");
        assert!(rec.apu_ran, "the APU must actually have been installed");
        // One second of audio, give or take the 256-sample block granularity.
        let n = rec.samples.len();
        assert!(
            (n as i64 - RATE as i64).abs() < 1024,
            "expected ~{RATE} samples, got {n}"
        );
        let lv = level(&rec.samples);
        assert!(
            is_audible(lv),
            "a full-volume square wave must be audible: {lv:?}"
        );
        assert!(lv.peak > 0.1, "peak {} too quiet", lv.peak);
    }

    /// The distinction that makes a silent result meaningful. A machine with no
    /// player installed has **no APU**; this proves our capture is not in that
    /// state when it reports silence.
    #[test]
    fn silence_is_reported_as_silence_not_as_an_absent_apu() {
        let rom = testrom::silent_rom();
        let rec = record(
            &rom,
            Options {
                frames: frames_for(0.5),
                ..Options::default()
            },
        )
        .expect("silent ROM maps");
        assert!(
            rec.apu_ran,
            "the APU must have run even though the cartridge made no sound"
        );
        assert!(!rec.samples.is_empty(), "a silent APU still emits samples");
        let lv = level(&rec.samples);
        assert!(!is_audible(lv), "this ROM programs no channel: {lv:?}");
    }

    #[test]
    fn arbitrary_bytes_are_an_error_not_a_crash() {
        assert!(record(&[0u8; 8], Options::default()).is_err());
        // Long enough for a header, but an unsupported mapper byte.
        let mut rom = vec![0u8; 0x8000];
        rom[0x147] = 0xEE;
        assert!(record(&rom, Options::default()).is_err());
    }

    #[test]
    fn the_fade_lands_on_silence() {
        let mut s = vec![(1.0f32, -1.0f32); RATE as usize];
        fade_out(&mut s, RATE);
        let last = *s.last().unwrap();
        assert!(last.0.abs() < 0.01 && last.1.abs() < 0.01, "{last:?}");
        // ... and leaves the body of the take alone.
        assert_eq!(s[0], (1.0, -1.0));
    }

    #[test]
    fn skipping_the_logo_drops_the_front_of_the_take() {
        let rom = testrom::tone_rom();
        let full = record(
            &rom,
            Options {
                frames: frames_for(1.0),
                ..Options::default()
            },
        )
        .unwrap();
        let skipped = record(
            &rom,
            Options {
                frames: frames_for(1.0),
                skip_frames: frames_for(0.5),
                ..Options::default()
            },
        )
        .unwrap();
        assert!(
            skipped.samples.len() < full.samples.len(),
            "skip must shorten the take: {} vs {}",
            skipped.samples.len(),
            full.samples.len()
        );
    }

    #[test]
    fn frames_for_matches_the_dmg_frame_rate() {
        assert_eq!(frames_for(1.0), 60); // 59.7275 rounded up
        assert_eq!(frames_for(0.0), 0);
    }
}
