//! **Chiptune instructions in, real Game Boy audio out.**
//!
//! A composer feeds this crate notes, a `.vgm` log or raw register writes and
//! gets back the samples an actual Game Boy would produce — because the thing
//! on the other end is not a synthesiser imitating a Game Boy, it is this
//! repository's emulated APU. That distinction is the whole value: the answer
//! is authoritative rather than approximate. See `docs/chiptune.md` for the
//! accuracy score the claim rests on and how to check it.
//!
//! # The three input layers
//!
//! 1. [`regs`] — **register writes with timing**. `NR10`-`NR52` and wave RAM,
//!    each at an exact T-cycle. This is the honest hardware interface and the
//!    layer that cannot be wrong, because it is what a cartridge puts on the
//!    bus. Everything else compiles to it, and [`Track::to_register_script`]
//!    prints it back out so a composer can always see what the chip is told.
//! 2. [`score`] — **notes, durations, envelopes, duty, sweep**. Small and
//!    obvious rather than a whole tracker language: one construct per hardware
//!    feature, named after the hardware feature, plus a `raw` escape hatch.
//! 3. [`vgm`] — **the established chiptune log format**. A VGM *is* a timed
//!    register log, so importing one involves no interpretation at all.
//!
//! # Where the sound chip comes from
//!
//! This crate has **no dependencies** and does not depend on the emulator.
//! The host supplies the chip through the [`Apu`] trait — in this repository
//! that is `terminalgb::chiptune::EmulatedApu`, about sixty lines over
//! `terminalgb::sound::Sound`. Keeping the trait here is what makes the
//! dependency graph acyclic (`terminalgb` -> `terminalgb-chiptune`, never back), the
//! same arrangement `terminalgb-plugin-api` uses and for the same reason.
//!
//! ```no_run
//! # use terminalgb_chiptune::{score, render, Apu, wav};
//! # fn demo(apu: &mut dyn Apu) -> Result<(), String> {
//! let track = score::compile("tempo 120\nch1 env 15 - 2\nch1 note C4 1/4\n")?;
//! let samples = render(&track, apu, 44_100);
//! wav::write(std::path::Path::new("out.wav"), &samples, 44_100).unwrap();
//! # Ok(())
//! # }
//! ```

pub mod note;
pub mod regs;
pub mod score;
pub mod track;
pub mod vgm;
// Unconditional and dependency-free on purpose: other crates in this workspace
// use it as *the* WAV encoder, so it must not sit behind a feature.
pub mod wav;

pub use note::Duration;
pub use track::{Model, RegisterWrite, Track, CLOCK_HZ};

/// The emulated sound chip, supplied by the host.
///
/// The contract is deliberately four methods and no configuration: the host
/// constructs the chip already set to the console model and sample rate it
/// wants, because those are questions about *the machine*, not about the
/// music. Everything here is what a cartridge can do to the APU and nothing
/// more.
pub trait Apu {
    /// Write one APU register. `addr` is absolute, `0xFF10..=0xFF3F`.
    ///
    /// The implementation must synchronise the chip to the current cycle
    /// before the write lands — that is what the emulator's MMU does on every
    /// APU access, and it is what makes length, sweep and envelope state
    /// correct at the instant a register is touched rather than up to a block
    /// stale.
    fn write(&mut self, addr: u16, value: u8);

    /// Advance the chip by `t_cycles` of the unmodified 4.194304 MHz clock.
    fn step(&mut self, t_cycles: u32);

    /// Move every stereo sample produced so far into `out`.
    fn drain(&mut self, out: &mut Vec<(f32, f32)>);
}

/// Render a track: play its register writes at their exact cycles and collect
/// the samples the chip produces.
///
/// Deterministic by construction — integer time throughout, no clock read, no
/// environment read, no floating-point accumulation of position. The same
/// track rendered twice produces byte-identical samples, which
/// `testharness/chiptune.rs` asserts rather than assumes.
///
/// The returned vector is exactly `duration * rate / 4194304` sample pairs.
/// The chip hands samples over in blocks, so rendering overshoots by up to one
/// block and truncates — anything else would silently drop the last few
/// milliseconds of every piece.
pub fn render(track: &Track, apu: &mut dyn Apu, sample_rate: u32) -> Vec<(f32, f32)> {
    let mut writes = track.writes.clone();
    writes.sort_by_key(|w| w.at);

    let mut out: Vec<(f32, f32)> = Vec::new();
    let mut now: u64 = 0;
    for w in &writes {
        step_to(apu, &mut now, w.at.min(track.duration), &mut out);
        apu.write(w.reg, w.value);
    }
    // One extra block past the end so the final partial block is flushed, then
    // cut to the exact length asked for.
    let want = (track.duration as u128 * sample_rate as u128 / CLOCK_HZ as u128) as usize;
    step_to(apu, &mut now, track.duration + FLUSH_TAIL_CYCLES, &mut out);
    apu.drain(&mut out);
    out.truncate(want);
    // A chip that produced fewer samples than the arithmetic says is a rate
    // bug, but silence is a better answer than a short buffer for a caller
    // that is about to write a WAV header from the length.
    out.resize(want, (0.0, 0.0));
    out
}

/// How far the chip is advanced per [`Apu::step`] call.
///
/// **Not a free choice, and it cost an afternoon to find out.** A Game Boy APU
/// hands its resampler a block of samples when its internal clock crosses the
/// block period — *once* per call, because it is written to be driven by a CPU
/// that never advances it by more than one instruction. Overshoot that
/// boundary by much and the deltas for the overshot part are added on top of a
/// buffer that already holds a block, and the resampler's fixed buffer is
/// exceeded. 1024 T-cycles is already too far; 64 is three times the longest
/// Game Boy instruction and has margin at every sample rate.
///
/// Chunking is otherwise **invisible**: the chip evaluates its channels lazily
/// over exact time ranges, so where the boundaries fall does not change a
/// single sample. Only the register writes' cycles matter, and those land
/// exactly. That is why this can be a constant rather than a tuning knob.
const STEP_CHUNK: u32 = 64;

/// Extra time rendered past the end of a track so the chip's last, partial
/// sample block is handed over before the render stops. The samples are then
/// truncated away — this changes nothing a listener hears, but it does mean a
/// caller reading chip *state* after a render is looking at the machine this
/// much later than the track's end.
///
/// Sized to cover one block at any sample rate down to 4 kHz.
pub const FLUSH_TAIL_CYCLES: u64 = CLOCK_HZ / 16;

fn step_to(apu: &mut dyn Apu, now: &mut u64, target: u64, out: &mut Vec<(f32, f32)>) {
    while *now < target {
        let chunk = (target - *now).min(STEP_CHUNK as u64) as u32;
        apu.step(chunk);
        *now += chunk as u64;
        apu.drain(out);
    }
}

/// Sniff the format of an input file and compile it.
///
/// Order is by content, not by extension, so a `.txt` full of register writes
/// still works and a `.vgm` that is really gzipped is named rather than
/// mis-parsed.
pub fn compile_auto(bytes: &[u8], model: Model) -> Result<Track, String> {
    if bytes.starts_with(b"Vgm ") || bytes.starts_with(&[0x1F, 0x8B]) {
        return vgm::parse(bytes, model).map(|(t, _)| t);
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "input is neither a VGM file nor UTF-8 text".to_string())?;
    let mut track = if looks_like_register_script(text) {
        regs::compile(text)?
    } else {
        score::compile(text)?
    };
    // An explicit `model` line in the source wins; otherwise the caller's
    // choice does.
    if !text
        .lines()
        .any(|l| l.trim_start().to_ascii_lowercase().starts_with("model "))
    {
        track.model = model;
    }
    Ok(track)
}

/// A register script's lines start with a time; a score's start with a word.
/// Cheap, and only used to pick a parser — both report their own errors.
fn looks_like_register_script(text: &str) -> bool {
    for line in text.lines() {
        let line = match line.find(['#', ';']) {
            Some(i) => &line[..i],
            None => line,
        }
        .trim();
        if line.is_empty() {
            continue;
        }
        let first = line.split_whitespace().next().unwrap_or("");
        if matches!(
            first.to_ascii_lowercase().as_str(),
            "model" | "tempo" | "title"
        ) {
            continue;
        }
        return first
            .trim_start_matches('+')
            .starts_with(|c: char| c.is_ascii_digit());
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chip that records what it was told, so the crate's own timing can be
    /// tested with no emulator present.
    #[derive(Default)]
    struct FakeApu {
        cycles: u64,
        log: Vec<(u64, u16, u8)>,
        /// One sample per 100 cycles, so `drain` returns something.
        produced: u64,
    }

    impl Apu for FakeApu {
        fn write(&mut self, addr: u16, value: u8) {
            self.log.push((self.cycles, addr, value));
        }
        fn step(&mut self, t: u32) {
            self.cycles += t as u64;
        }
        fn drain(&mut self, out: &mut Vec<(f32, f32)>) {
            let want = self.cycles / 100;
            while self.produced < want {
                out.push((0.0, 0.0));
                self.produced += 1;
            }
        }
    }

    #[test]
    fn every_write_lands_on_the_cycle_it_asked_for() {
        let mut t = Track::new();
        t.push(0, 0xFF26, 0x80);
        t.push(1_000, 0xFF13, 0xD6);
        t.push(1_000, 0xFF14, 0x86);
        t.push(500_000, 0xFF12, 0x00);
        t.duration = 1_000_000;
        let mut apu = FakeApu::default();
        render(&t, &mut apu, 44_100);
        assert_eq!(
            apu.log,
            vec![
                (0, 0xFF26, 0x80),
                (1_000, 0xFF13, 0xD6),
                (1_000, 0xFF14, 0x86),
                (500_000, 0xFF12, 0x00),
            ]
        );
    }

    #[test]
    fn the_sample_count_is_exactly_the_duration() {
        let mut t = Track::new();
        t.duration = CLOCK_HZ; // one second
        let mut apu = FakeApu::default();
        let s = render(&t, &mut apu, 44_100);
        assert_eq!(s.len(), 44_100);
    }

    #[test]
    fn a_score_and_a_register_script_are_told_apart() {
        assert!(!looks_like_register_script("tempo 120\nch1 note C4 1/4\n"));
        assert!(looks_like_register_script("model dmg\n0t NR52 $80\n"));
        assert!(looks_like_register_script("# hi\n+10ms NR52 $80\n"));
    }

    #[test]
    fn compile_auto_reaches_all_three_front_ends() {
        let a = compile_auto(b"tempo 120\nch1 note C4 1/4\n", Model::Dmg).unwrap();
        assert!(!a.writes.is_empty());
        let b = compile_auto(b"0t NR52 $80\nend 1s\n", Model::Dmg).unwrap();
        assert_eq!(b.writes.len(), 1);
        let v = vgm::encode(&b);
        let c = compile_auto(&v, Model::Dmg).unwrap();
        assert_eq!(c.writes.len(), 1);
    }
}
