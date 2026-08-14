//! Chiptune renderer harness — proves the claim, not just the plumbing.
//!
//! `cargo test --release --test chiptune` (in CI; ~2 s, no ROM, no audio
//! device, no network).
//!
//! The claim this tool makes is that a composer gets **what the hardware would
//! produce**, not a plausible imitation. So the checks here are not "did it
//! write a file": each one measures a property of the rendered audio that only
//! a real APU model gets right, against a number derived from Pan Docs rather
//! than from our own output. Every fixture is written here, in this file —
//! nothing sampled, nothing borrowed.
//!
//!  1. **Determinism** — the same score twice is byte-identical PCM. This is
//!     the property everything else rests on, and it is asserted rather than
//!     assumed.
//!  2. **Pitch** — a C major scale comes out at the frequencies the Game Boy's
//!     11-bit period register can actually express, which are *not* equal
//!     temperament. Getting the reachable pitch right is most of what a
//!     composer comes here for.
//!  3. **The envelope** — volume 15 counting down with period 3 steps every
//!     3/64 s and reaches silence after fifteen steps.
//!  4. **The sweep** — the period register, not the frequency, is what the
//!     sweep unit adds to; and an upward sweep that overflows past 2047
//!     **switches the channel off**. A tone generator with a pitch envelope
//!     bolted on cannot do either.
//!  5. **The noise LFSR** — the 7-bit width makes an exactly periodic
//!     waveform, 127 output bits long at the rate NR43 sets. The 15-bit width
//!     does not repeat inside the same window. This is the sharpest single
//!     piece of evidence that the thing on the other end is the emulated chip.
//!  6. **Duty** — 50% has odd harmonics only; 25% and 75% have identical
//!     magnitude spectra. Both are audible as timbre.
//!  7. **DMG vs CGB** — the two consoles' length-counter rules really differ,
//!     and the renderer honours the choice.
//!  8. **`.vgm` round trip** — export and re-import renders identically.
//!
//! The DSP here (Goertzel, autocorrelation, an RMS envelope) is a few dozen
//! lines and deliberately hand-rolled: a harness that needed a numerical
//! library would be a harness nobody runs.

use terminalgb::chiptune::{render, EmulatedApu, RenderOptions};
use terminalgb_chiptune::note::{square_hz, square_period};
use terminalgb_chiptune::track::CLOCK_HZ;
use terminalgb_chiptune::{compile_auto, score, vgm, wav, Model, Track};

const RATE: u32 = 44_100;

// ---------------------------------------------------------------------------
// Small, dependency-free analysis
// ---------------------------------------------------------------------------

/// Power at one frequency, by the Goertzel algorithm. Cheaper and sharper than
/// an FFT when the frequency of interest is known in advance, which it always
/// is here.
fn power_at(x: &[f32], rate: u32, hz: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / rate as f64;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in x {
        let s0 = v as f64 + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0) / (x.len() as f64).powi(2)
}

/// Dominant frequency, found by scanning a coarse grid then refining. Good to
/// well under a semitone, which is all any check here needs.
fn dominant_hz(x: &[f32], rate: u32, lo: f64, hi: f64) -> f64 {
    let mut best = (0.0f64, 0.0f64);
    let mut f = lo;
    while f < hi {
        let p = power_at(x, rate, f);
        if p > best.1 {
            best = (f, p);
        }
        f *= 1.002;
    }
    // Refine around the winner.
    let (mut bf, mut bp) = best;
    for i in -40..=40 {
        let f = best.0 * (1.0 + i as f64 * 0.00005);
        let p = power_at(x, rate, f);
        if p > bp {
            bf = f;
            bp = p;
        }
    }
    bf
}

fn rms(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len() as f64).sqrt()
}

fn left(s: &[(f32, f32)]) -> Vec<f32> {
    s.iter().map(|&(l, _)| l).collect()
}

/// Split into sounding segments on a moving-average silence gate.
fn segments(x: &[f32], rate: u32) -> Vec<(usize, usize)> {
    let w = 256usize;
    let mut env = vec![0f32; x.len()];
    let mut acc = 0f32;
    for i in 0..x.len() {
        acc += x[i].abs();
        if i >= w {
            acc -= x[i - w].abs();
        }
        env[i] = acc / w as f32;
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < x.len() {
        if env[i] > 0.02 {
            let start = i;
            while i < x.len() && env[i] > 0.02 {
                i += 1;
            }
            if i - start > rate as usize / 100 {
                out.push((start, i));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Normalised autocorrelation at one lag — the periodicity test for the LFSR.
fn autocorr(x: &[f32], lag: usize) -> f64 {
    if lag >= x.len() {
        return 0.0;
    }
    let n = x.len() - lag;
    let mean = x.iter().map(|&v| v as f64).sum::<f64>() / x.len() as f64;
    let mut num = 0.0;
    let mut den = 0.0;
    for i in 0..n {
        num += (x[i] as f64 - mean) * (x[i + lag] as f64 - mean);
    }
    for &v in &x[..n] {
        den += (v as f64 - mean).powi(2);
    }
    if den <= 0.0 {
        0.0
    } else {
        num / den
    }
}

// ---------------------------------------------------------------------------

struct Harness {
    failures: Vec<String>,
    checks: u32,
}

impl Harness {
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        self.checks += 1;
        if ok {
            println!("  PASS  {name}  —  {detail}");
        } else {
            println!("  FAIL  {name}  —  {detail}");
            self.failures.push(name.to_string());
        }
    }
}

fn render_score(src: &str) -> Vec<(f32, f32)> {
    let t = score::compile(src).unwrap_or_else(|e| panic!("compiling fixture: {e}"));
    render(&t, RenderOptions::default())
}

fn main() {
    let mut h = Harness {
        failures: Vec::new(),
        checks: 0,
    };
    println!("chiptune renderer — properties of the audio, not of the plumbing\n");

    determinism(&mut h);
    pitch(&mut h);
    envelope(&mut h);
    sweep(&mut h);
    noise_lfsr(&mut h);
    duty(&mut h);
    consoles(&mut h);
    vgm_round_trip(&mut h);
    register_layer(&mut h);

    println!("\n{} checks, {} failed", h.checks, h.failures.len());
    if !h.failures.is_empty() {
        for f in &h.failures {
            println!("  failed: {f}");
        }
        std::process::exit(1);
    }
    println!("all good");
}

// -- 1. determinism ---------------------------------------------------------

fn determinism(h: &mut Harness) {
    println!("determinism");
    let src = "tempo 140\n\
               ch1 duty 1\nch1 env 15 - 3\nch1 note C4 1/8\nch1 note E4 1/8\nch1 note G4 1/4\n\
               ch2 vol 9\nch2 note C3 1/2\n\
               ch3 wave 0123456789ABCDEFFEDCBA9876543210\nch3 note C3 1/2\n\
               ch4 noise 4 15 4\nch4 hit 1/16\nch4 rest 1/16\nch4 hit 1/16\n";
    let a = wav::interleave_i16(&render_score(src));
    let b = wav::interleave_i16(&render_score(src));
    h.check(
        "the same score renders to byte-identical PCM",
        a == b && !a.is_empty(),
        format!("{} bytes, hash {:016x}", a.len(), wav::fnv1a64(&a)),
    );

    // And a different score must not: a test that passes on an empty renderer
    // is not a test.
    let c = wav::interleave_i16(&render_score(&src.replace("C4", "D4")));
    h.check(
        "a changed note changes the samples",
        a != c,
        format!(
            "hash {:016x} vs {:016x}",
            wav::fnv1a64(&a),
            wav::fnv1a64(&c)
        ),
    );
}

// -- 2. pitch ---------------------------------------------------------------

fn pitch(h: &mut Harness) {
    println!("\npitch — the chip's reachable frequencies, not equal temperament");
    let names = ["C4", "D4", "E4", "F4", "G4", "A4", "B4", "C5"];
    let mut src = String::from("tempo 120\nmodel dmg\nch1 duty 2\nch1 vol 15\n");
    for n in names {
        src.push_str(&format!("ch1 note {n} 1/8\nch1 rest 1/16\n"));
    }
    let s = render_score(&src);
    let x = left(&s);
    let segs = segments(&x, RATE);
    h.check(
        "eight notes were played",
        segs.len() == 8,
        format!("{} sounding segments", segs.len()),
    );
    if segs.len() != 8 {
        return;
    }
    let midis = [60, 62, 64, 65, 67, 69, 71, 72];
    let mut worst = 0.0f64;
    let mut report = String::new();
    for (i, &(a, b)) in segs.iter().enumerate() {
        let core = &x[a + (b - a) / 5..a + 4 * (b - a) / 5];
        // What the chip *can* produce for this note — the period register is
        // 11 bits, so A4 is 439.84 Hz and C5 is 524.288 Hz.
        let want = square_hz(
            square_period(terminalgb_chiptune::note::midi_hz(midis[i]))
                .expect("in range"),
        );
        let got = dominant_hz(core, RATE, want * 0.85, want * 1.15);
        let cents = 1200.0 * (got / want).log2();
        worst = worst.max(cents.abs());
        report.push_str(&format!("{}={got:.2} ", names[i]));
    }
    h.check(
        "every note lands on the frequency the period register produces",
        worst < 6.0,
        format!("worst error {worst:.2} cents; {}", report.trim_end()),
    );
    // And the quantisation is real and worth reporting: C5 is 524.29 Hz on a
    // Game Boy, not 523.25.
    let ideal = 523.2511;
    let real = square_hz(square_period(ideal).unwrap());
    h.check(
        "the tool knows C5 is not exactly C5 on this machine",
        (real - 524.288).abs() < 0.01,
        format!("equal temperament {ideal:.2} Hz, Game Boy {real:.3} Hz"),
    );
}

// -- 3. envelope ------------------------------------------------------------

fn envelope(h: &mut Harness) {
    println!("\nenvelope — a 64 Hz counter with period 3, fifteen steps to silence");
    let s = render_score(
        "tempo 60\nmodel dmg\nch1 duty 2\nch1 env 15 - 3\nch1 note A4 1/1\n",
    );
    let x = left(&s);
    // RMS in 1 ms windows; find where the level first crosses each 1/15th.
    let w = RATE as usize / 1000;
    // Peak, not RMS, and over three milliseconds: a 440 Hz square's period is
    // 2.27 ms, so a one-millisecond RMS window lands on a different part of the
    // wave each time and the staircase disappears into the measurement. The
    // peak over a whole period is exactly proportional to the envelope volume.
    let levels: Vec<f64> = (0..x.len() / w - 3)
        .map(|i| {
            x[i * w..(i + 3) * w]
                .iter()
                .fold(0.0f64, |m, &v| m.max(v.abs() as f64))
        })
        .collect();
    // Hardware: period 3 at 64 Hz = 46.875 ms per step. Find the edges — the
    // milliseconds where the level drops — rather than thresholding, because a
    // staircase is defined by where it steps, not by where it happens to be.
    let step_ms = 1000.0 * 3.0 / 64.0;
    // The envelope's output is an integer 15..0 and the RMS is proportional to
    // it, so quantise the level back to a volume and find where each value
    // first appears, scanning forward. That reads the staircase directly
    // rather than chasing edges through the 1 ms measurement noise.
    let full = levels[2..12].iter().cloned().fold(0.0f64, f64::max);
    let vol_at = |i: usize| (levels[i] / full * 15.0).round() as i32;
    let mut edges: Vec<usize> = Vec::new();
    let mut i = 2usize;
    for want in (0..=14).rev() {
        while i < levels.len() && vol_at(i) > want {
            i += 1;
        }
        if i >= levels.len() {
            break;
        }
        edges.push(i);
    }
    // Judge the *rate* over the whole staircase rather than each riser: a
    // 3 ms measurement window smears any single edge by a millisecond or two,
    // but fourteen of them in a row pin the period to well under that.
    let mean_gap = if edges.len() >= 2 {
        (edges[edges.len() - 1] - edges[0]) as f64 / (edges.len() - 1) as f64
    } else {
        0.0
    };
    h.check(
        "the volume steps every 3/64 s, fifteen times",
        edges.len() == 15 && (mean_gap - step_ms).abs() < 1.0,
        format!(
            "{} steps from {} to {} ms; mean {mean_gap:.2} ms against the \
             {step_ms:.3} ms hardware period",
            edges.len(),
            edges.first().copied().unwrap_or(0),
            edges.last().copied().unwrap_or(0)
        ),
    );
    let silent_at = levels.iter().position(|&v| v < 1e-4).unwrap_or(usize::MAX);
    let expect = (step_ms * 15.0) as usize;
    h.check(
        "and reaches silence after fifteen of them",
        silent_at.abs_diff(expect) < 12,
        format!("silent at {silent_at} ms, expected {expect} ms"),
    );
}

// -- 4. sweep ---------------------------------------------------------------

fn sweep(h: &mut Harness) {
    println!("\nsweep — channel 1's, operating on the period register");

    // Downward from C5. NR10 period 3 = a sweep event every 3/128 s; shift 2
    // means x -= x>>2 each time. A *smaller* period register is a LOWER pitch,
    // so `-` falls — which is the opposite of what "decrease" sounds like it
    // should mean and is exactly the sort of thing this tool exists to settle.
    let s = render_score(
        "tempo 60\nmodel dmg\nch1 duty 2\nch1 vol 15\nch1 sweep 3 - 2\nch1 note C5 1/2\n",
    );
    let x = left(&s);
    let first = dominant_hz(&x[..RATE as usize / 100], RATE, 400.0, 700.0);
    let later = dominant_hz(
        &x[RATE as usize / 10..RATE as usize / 10 + RATE as usize / 50],
        RATE,
        50.0,
        700.0,
    );
    h.check(
        "a downward sweep starts at the note and falls",
        (first - 524.288).abs() < 8.0 && later < first / 3.0,
        format!("{first:.1} Hz at the start, {later:.1} Hz 100 ms in"),
    );

    // Upward with a big shift: the period register overflows past 2047 and the
    // hardware DISABLES the channel. Nothing about a pitch envelope predicts
    // this; it is a specific rule in the sweep unit.
    let s = render_score(
        "tempo 60\nmodel dmg\nch1 duty 2\nch1 vol 15\nch1 sweep 4 + 3\nch1 note C5 1/1\n",
    );
    let x = left(&s);
    let early = rms(&x[..RATE as usize / 100]);
    let late = rms(&x[RATE as usize / 5..RATE as usize / 2]);
    h.check(
        "an upward sweep that overflows switches the channel OFF",
        early > 0.1 && late == 0.0,
        format!("RMS {early:.4} at the start, {late:.4} after 200 ms"),
    );

    // A gentle upward sweep must survive and climb — the check above must not
    // be passing because sweeps are simply broken.
    let s = render_score(
        "tempo 60\nmodel dmg\nch1 duty 2\nch1 vol 15\nch1 sweep 5 + 6\nch1 note C4 1/2\n",
    );
    let x = left(&s);
    let a = dominant_hz(&x[..RATE as usize / 50], RATE, 200.0, 400.0);
    let b = dominant_hz(
        &x[RATE as usize / 5..RATE as usize / 5 + RATE as usize / 50],
        RATE,
        200.0,
        800.0,
    );
    h.check(
        "a gentle upward sweep climbs and keeps playing",
        b > a * 1.2 && rms(&x[RATE as usize / 5..RATE as usize / 4]) > 0.1,
        format!("{a:.1} Hz -> {b:.1} Hz over 200 ms"),
    );
}

// -- 5. noise ---------------------------------------------------------------

fn noise_lfsr(h: &mut Harness) {
    println!("\nnoise — a linear-feedback shift register, not a random-number generator");
    // NR43 = shift 4, divisor code 0. The LFSR is clocked at
    //   524288 / r / 2^(s+1)  with r = 0.5 for divisor code 0
    // = 524288 / 0.5 / 32 = 32768 Hz  (Pan Docs, "Audio Registers" -> NR43).
    let clock_hz = 524_288.0 / 0.5 / 2f64.powi(5);
    let period_7 = 127.0 / clock_hz;

    let s =
        render_score("tempo 60\nmodel dmg\nch4 vol 15\nch4 noise 4 7 0\nch4 hit 1/2\n");
    let x = left(&s);
    let seg = &x[RATE as usize / 20..RATE as usize * 2 / 5];
    let lag = (period_7 * RATE as f64).round() as usize;
    let r = autocorr(seg, lag);
    h.check(
        "the 7-bit LFSR repeats every 127 output bits",
        r > 0.9,
        format!(
            "autocorrelation {r:.3} at {lag} samples ({:.3} ms; hardware says {:.3} ms)",
            lag as f64 * 1000.0 / RATE as f64,
            period_7 * 1000.0
        ),
    );
    // Right beside it there must be no such peak, or the "period" is an
    // artefact of the analysis rather than the chip.
    let off = autocorr(seg, lag * 3 / 2);
    h.check(
        "and not at one and a half times that lag",
        off < 0.5,
        format!("autocorrelation {off:.3} at {} samples", lag * 3 / 2),
    );

    let s =
        render_score("tempo 60\nmodel dmg\nch4 vol 15\nch4 noise 4 15 0\nch4 hit 1/2\n");
    let x = left(&s);
    let seg = &x[RATE as usize / 20..RATE as usize * 2 / 5];
    let r15 = autocorr(seg, lag);
    h.check(
        "the 15-bit LFSR does not repeat there — 32767 bits is a full second",
        r15 < 0.2 && rms(seg) > 0.05,
        format!("autocorrelation {r15:.3}, RMS {:.4}", rms(seg)),
    );
}

// -- 6. duty ----------------------------------------------------------------

fn duty(h: &mut Harness) {
    println!("\nduty — the pulse width, which is the square channels' whole timbre");
    let f = square_hz(square_period(110.0).unwrap());
    let spectrum = |d: u8| -> Vec<f64> {
        let s = render_score(&format!(
            "tempo 60\nmodel dmg\nch1 duty {d}\nch1 vol 15\nch1 note A2 1/2\n"
        ));
        let x = left(&s);
        let seg = &x[RATE as usize / 20..RATE as usize * 2 / 5];
        let base = power_at(seg, RATE, f);
        (1..=8)
            .map(|n| power_at(seg, RATE, f * n as f64) / base)
            .collect()
    };

    let d2 = spectrum(2);
    // A 50% square is odd harmonics only. Even ones are 40+ dB down.
    // Three orders of magnitude separate a harmonic that is there from one
    // that is not, so the thresholds are not delicately placed.
    const ABSENT: f64 = 1e-3;
    const PRESENT: f64 = 5e-3;
    let evens = [d2[1], d2[3], d2[5], d2[7]];
    let odds = [d2[2], d2[4], d2[6]];
    h.check(
        "50% duty has odd harmonics only",
        evens.iter().all(|&p| p < ABSENT) && odds.iter().all(|&p| p > PRESENT),
        format!(
            "2nd {:.2e}, 4th {:.2e} | 3rd {:.3}, 5th {:.3}",
            d2[1], d2[3], d2[2], d2[4]
        ),
    );

    let d1 = spectrum(1);
    let d3 = spectrum(3);
    // 75% is 25% inverted: same magnitude spectrum, opposite phase. And a 25%
    // pulse loses every fourth harmonic.
    let same = d1
        .iter()
        .zip(&d3)
        .all(|(a, b)| (a - b).abs() < 0.02 * a.max(*b).max(1e-9) + 1e-6);
    h.check(
        "25% and 75% duty have the same magnitude spectrum",
        same,
        format!(
            "25%: {:.3} {:.3} {:.3}, 75%: {:.3} {:.3} {:.3}",
            d1[1], d1[2], d1[3], d3[1], d3[2], d3[3]
        ),
    );
    h.check(
        "25% duty loses every fourth harmonic",
        d1[3] < ABSENT && d1[7] < ABSENT && d1[1] > PRESENT,
        format!("4th {:.2e}, 8th {:.2e}, 2nd {:.3}", d1[3], d1[7], d1[1]),
    );

    let d0 = spectrum(0);
    h.check(
        "12.5% duty's first null is the eighth harmonic",
        d0[7] < ABSENT && d0[3] > PRESENT,
        format!("8th {:.2e}, 4th {:.3}", d0[7], d0[3]),
    );
}

// -- 7. consoles ------------------------------------------------------------

fn consoles(h: &mut Harness) {
    println!("\nDMG vs CGB — a difference a composer can hear, and choose");
    // Load a length counter while the APU is off. A DMG keeps it (the length
    // counters live outside the powered domain); a CGB discards the write.
    let src = "power off\n\
               ch1 raw NR11 $BF\n\
               ch1 raw NR52 $80\nch1 raw NR50 $77\nch1 raw NR51 $FF\n\
               ch1 raw NR12 $F0\nch1 raw NR13 $D6\nch1 raw NR14 $C6\n\
               end 100ms\n";
    let t = score::compile(src).unwrap();
    let run = |model: Model| -> (f64, u8) {
        let opts = RenderOptions {
            model,
            ..Default::default()
        };
        let mut apu = EmulatedApu::new(opts);
        let s = terminalgb_chiptune::render(&t, &mut apu, RATE);
        (rms(&left(&s)[RATE as usize / 20..]), apu.read(0xFF26) & 1)
    };
    let (dmg_rms, dmg_on) = run(Model::Dmg);
    let (cgb_rms, cgb_on) = run(Model::Cgb);
    h.check(
        "a DMG keeps a length load written while the APU was off",
        dmg_on == 0 && dmg_rms == 0.0,
        format!("NR52 channel bit {dmg_on}, RMS after 50 ms {dmg_rms:.4}"),
    );
    h.check(
        "a CGB discards it, so the same score plays on",
        cgb_on == 1 && cgb_rms > 0.1,
        format!("NR52 channel bit {cgb_on}, RMS after 50 ms {cgb_rms:.4}"),
    );
}

// -- 8. VGM -----------------------------------------------------------------

fn vgm_round_trip(h: &mut Harness) {
    println!("\nVGM — the established chiptune log format");
    let src = "tempo 132\nch1 duty 1\nch1 env 15 - 2\nch1 note E4 1/8\nch1 note G4 1/8\n\
               ch4 noise 3 7 2\nch4 hit 1/16\n";
    let t = score::compile(src).unwrap();
    let bytes = vgm::encode(&t);
    let (back, info) = match vgm::parse(&bytes, Model::Dmg) {
        Ok(v) => v,
        Err(e) => {
            h.check("a VGM we wrote parses", false, e);
            return;
        }
    };
    h.check(
        "a VGM we wrote parses, with every write intact",
        back.writes.len() == t.writes.len() && info.foreign_commands == 0,
        format!(
            "{} writes in, {} out; version {:#x}, GB clock {} Hz",
            t.writes.len(),
            back.writes.len(),
            info.version,
            info.gb_clock
        ),
    );
    // The audio must survive the trip. VGM's clock is 44.1 kHz where the APU's
    // is 4.19 MHz, so write times move by up to one VGM sample (~95 T-cycles)
    // and the PCM is *not* expected to be identical — the pitch, level and
    // note count are.
    let a = render(&t, RenderOptions::default());
    let b = render(&back, RenderOptions::default());
    let (xa, xb) = (left(&a), left(&b));
    let (na, nb) = (segments(&xa, RATE).len(), segments(&xb, RATE).len());
    h.check(
        "and renders to the same music",
        na == nb && na > 0 && (rms(&xa) - rms(&xb)).abs() < 0.02 * rms(&xa),
        format!("{na} notes vs {nb}, RMS {:.4} vs {:.4}", rms(&xa), rms(&xb)),
    );
    // A gzipped VGM must be named, not mangled.
    h.check(
        "a .vgz is reported with the fix rather than parsed as garbage",
        vgm::parse(&[0x1F, 0x8B, 0x08, 0x00], Model::Dmg)
            .err()
            .map(|e| e.contains("gunzip"))
            .unwrap_or(false),
        "detected by the gzip magic".to_string(),
    );
}

// -- 9. the register layer --------------------------------------------------

fn register_layer(h: &mut Harness) {
    println!("\nthe register layer — the thing that cannot be wrong");
    let src = "tempo 132\nch1 duty 2\nch1 env 15 - 3\nch1 note C4 1/8\nch2 note G3 1/4\n\
               ch3 wave 0123456789ABCDEFFEDCBA9876543210\nch3 note C3 1/4\n";
    let t = score::compile(src).unwrap();
    let script = t.to_register_script();
    let back =
        compile_auto(script.as_bytes(), Model::Dmg).expect("re-reading our own dump");
    h.check(
        "a score's compiled register writes round-trip through the text form",
        back.writes == t.writes && back.duration == t.duration,
        format!("{} writes, {:.3} s", t.writes.len(), t.seconds()),
    );
    let a = wav::interleave_i16(&render(&t, RenderOptions::default()));
    let b = wav::interleave_i16(&render(&back, RenderOptions::default()));
    h.check(
        "and render to byte-identical audio",
        a == b,
        format!("hash {:016x}", wav::fnv1a64(&a)),
    );

    // The shipped examples must compile — they are the first thing anyone runs.
    for (name, src) in [
        (
            "demo.gbscore",
            include_str!("../docs/chiptune-examples/demo.gbscore"),
        ),
        (
            "tone.gbregs",
            include_str!("../docs/chiptune-examples/tone.gbregs"),
        ),
    ] {
        match compile_auto(src.as_bytes(), Model::Dmg) {
            Ok(t) => {
                let s = render(&t, RenderOptions::default());
                let peak = s
                    .iter()
                    .fold(0f32, |m, &(l, r)| m.max(l.abs()).max(r.abs()));
                h.check(
                    &format!("the shipped example {name} compiles and sounds"),
                    peak > 0.05 && !s.is_empty(),
                    format!(
                        "{:.3} s, {} writes, peak {peak:.3}",
                        t.seconds(),
                        t.writes.len()
                    ),
                );
            }
            Err(e) => h.check(&format!("the shipped example {name} compiles"), false, e),
        }
    }

    // An empty track must be exact silence at the full rate, not an absent
    // chip: `Sound::do_cycle` returns immediately with no player installed, so
    // "no sound" and "no APU" look identical from outside unless this is
    // checked.
    let mut empty = Track::new();
    empty.duration = CLOCK_HZ / 2;
    let s = render(&empty, RenderOptions::default());
    h.check(
        "a silent track is silence at the full sample rate, not a missing chip",
        s.len() == RATE as usize / 2 && s.iter().all(|&(l, r)| l == 0.0 && r == 0.0),
        format!("{} samples, all zero", s.len()),
    );
}
