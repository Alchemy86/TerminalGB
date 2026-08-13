//! Headless audio-capture and analysis harness — **no audio device needed**.
//!
//! The APU is always compiled (only host playback sits behind the `audio`
//! feature), so a ROM can be run headlessly with a capture `AudioPlayer` that
//! records every sample the APU + blip_buf pipeline produces. That turns
//! "it sounds bad" into numbers:
//!
//!   * **Sample count** vs `cycles * rate / 4194304` — a drifting count is a
//!     rate bug (the exact failure class behind "audio is fast/slow/rough").
//!   * **RMS / peak level** — silence where sound is expected, or clipping.
//!   * **Discontinuity detection** — the largest sample-to-sample jump. blip_buf
//!     band-limits every legitimate edge, so a huge step means a click/pop.
//!   * **Dominant frequency** (zero-crossing over the loudest window) — a tone
//!     programmed at 440 Hz must measure ~440 Hz.
//!   * **Determinism** — the same ROM + frame count must produce byte-identical
//!     samples, which makes an FNV-1a hash of the stream a pinnable regression
//!     fixture (`testharness/audio_baseline.txt`), exactly like the video
//!     frame-hash baseline. Regenerate deliberately with:
//!
//!     ```text
//!     UPDATE_AUDIO_BASELINE=1 cargo test --release --test audiocapture
//!     ```
//!
//! Every capture is also written out as a 16-bit stereo WAV under
//! `target/testrom-artifacts/audio/` so a human can *listen* to exactly what
//! the emulator produced. The WAV writer is hand-rolled (~30 lines, RIFF PCM);
//! no new dependencies.
//!
//! Fixtures:
//!   * `tone440` — a ~40-byte hand-assembled ROM (built in memory below) that
//!     programs square channel 1 to 439.84 Hz at full volume and spins. Its
//!     expected output is known *a priori*, so this checks absolute pitch,
//!     level and continuity, not just "same as yesterday".
//!   * Blargg `dmg_sound` (and singles) — exercises the register-visible APU
//!     corner cases while producing audible beeps. Its memory-mapped result
//!     ($A000 = final status once the $DE $B0 $61 signature is present) gives
//!     an objective pass/fail for the parts of the APU the CPU can observe.

use gameboy::cpu::core::Cpu;
use gameboy::sound::AudioPlayer;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const CLOCK: u64 = 4_194_304;
const RATE: u32 = 44_100;

// ---------------------------------------------------------------------------
// Capture player: records everything the APU produces. Device-free.
// ---------------------------------------------------------------------------

struct CapturePlayer {
    rate: u32,
    buf: Arc<Mutex<Vec<(f32, f32)>>>,
}

impl AudioPlayer for CapturePlayer {
    fn play(&mut self, left: &[f32], right: &[f32]) {
        let mut b = self.buf.lock().unwrap();
        for i in 0..left.len() {
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

struct Capture {
    samples: Vec<(f32, f32)>,
    cycles: u64,
    /// Blargg memory-mapped result: Some(status) once the $A001..$A003
    /// signature ($DE $B0 $61) is present. 0x80 = still running, 0 = pass.
    blargg_status: Option<(u8, String)>,
}

fn run_capture(rom_path: &Path, frames: u32) -> Capture {
    run_capture_inner(rom_path, frames, false)
}

fn run_capture_inner(rom_path: &Path, frames: u32, stop_on_verdict: bool) -> Capture {
    let data = std::fs::read(rom_path)
        .unwrap_or_else(|e| panic!("cannot read ROM {}: {e}", rom_path.display()));
    let mut cpu = Cpu::new(data, Some(rom_path.to_path_buf()));
    let buf = Arc::new(Mutex::new(Vec::new()));
    cpu.memory.enable_audio(Box::new(CapturePlayer {
        rate: RATE,
        buf: buf.clone(),
    }));

    let mut cycles: u64 = 0;
    // Generous cap: a ROM that never completes a frame can't wedge the run.
    let cap: u64 = 2_000_000_000;
    let mut frames_done = 0u32;
    while frames_done < frames && cycles < cap {
        cycles += cpu.do_cycle() as u64;
        if cpu.memory.gpu.updated {
            cpu.memory.gpu.updated = false;
            frames_done += 1;
            // TDD mode: stop as soon as the Blargg verdict lands (plus a
            // small grace so the result text is fully written).
            if stop_on_verdict
                && frames_done % 10 == 0
                && cpu.memory.rb(0xA001) == 0xDE
                && cpu.memory.rb(0xA002) == 0xB0
                && cpu.memory.rb(0xA003) == 0x61
                && cpu.memory.rb(0xA000) != 0x80
            {
                for _ in 0..5 {
                    while !cpu.memory.gpu.updated {
                        cpu.do_cycle();
                    }
                    cpu.memory.gpu.updated = false;
                }
                break;
            }
        }
    }

    // Blargg sound ROMs report to cartridge RAM: $A000 status, $A001-3
    // signature, $A004.. zero-terminated text.
    let sig = [
        cpu.memory.rb(0xA001),
        cpu.memory.rb(0xA002),
        cpu.memory.rb(0xA003),
    ];
    let blargg_status = if sig == [0xDE, 0xB0, 0x61] {
        let status = cpu.memory.rb(0xA000);
        let mut text = String::new();
        for a in 0xA004..0xA200u16 {
            let b = cpu.memory.rb(a);
            if b == 0 {
                break;
            }
            text.push(b as char);
        }
        Some((status, text))
    } else {
        None
    };

    let samples = buf.lock().unwrap().clone();
    Capture {
        samples,
        cycles,
        blargg_status,
    }
}

// ---------------------------------------------------------------------------
// The tone ROM: 32 KiB, MBC0, plays a 439.84 Hz square on channel 1 forever.
// ---------------------------------------------------------------------------

/// GB square frequency register value: f = 131072 / (2048 - x).
/// x = 1750 -> 131072 / 298 = 439.839 Hz.
const TONE_FREQ_X: u16 = 1750;
const TONE_EXPECTED_HZ: f64 = 131072.0 / (2048 - TONE_FREQ_X as u32) as f64;

fn build_tone_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];
    // Entry point: nop; jp $0150
    rom[0x100] = 0x00;
    rom[0x101] = 0xC3;
    rom[0x102] = 0x50;
    rom[0x103] = 0x01;
    // Header: title
    rom[0x134..0x134 + 7].copy_from_slice(b"TONE440");
    rom[0x147] = 0x00; // MBC0
    rom[0x148] = 0x00; // 32 KiB ROM
    rom[0x149] = 0x00; // no cart RAM
                       // Header checksum over 0x134..=0x14C
    let mut chk: u8 = 0;
    for &b in rom.iter().take(0x14D).skip(0x134) {
        chk = chk.wrapping_sub(b).wrapping_sub(1);
    }
    rom[0x14D] = chk;

    // Program at $0150: power on APU, route ch1 both sides, max master
    // volume, duty 50%, volume 15 no envelope, freq = TONE_FREQ_X, trigger
    // with length disabled, then spin.
    let lo = (TONE_FREQ_X & 0xFF) as u8;
    let hi = 0x80 | ((TONE_FREQ_X >> 8) & 0x7) as u8; // trigger, length off
    let prog: &[u8] = &[
        0x3E, 0x80, 0xE0, 0x26, // LD A,$80 ; LDH ($26),A  NR52 power on
        0x3E, 0xFF, 0xE0, 0x25, // LD A,$FF ; LDH ($25),A  NR51 all channels
        0x3E, 0x77, 0xE0, 0x24, // LD A,$77 ; LDH ($24),A  NR50 max volume
        0x3E, 0x80, 0xE0, 0x11, // LD A,$80 ; LDH ($11),A  NR11 duty 50%
        0x3E, 0xF0, 0xE0, 0x12, // LD A,$F0 ; LDH ($12),A  NR12 vol 15
        0x3E, lo, 0xE0, 0x13, //   LD A,lo  ; LDH ($13),A  NR13
        0x3E, hi, 0xE0, 0x14, //   LD A,hi  ; LDH ($14),A  NR14 trigger
        0x18, 0xFE, //             JR -2 (spin forever)
    ];
    rom[0x150..0x150 + prog.len()].copy_from_slice(prog);
    rom
}

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

struct Stats {
    samples: usize,
    seconds: f64,
    peak: f32,
    rms: f64,
    /// Fraction of sample pairs where both channels are below the silence
    /// threshold (1e-4 ~= -80 dBFS).
    silent_frac: f64,
    /// Longest run of consecutive silent samples, in milliseconds.
    longest_silence_ms: f64,
    /// Largest absolute sample-to-sample step on the left channel. blip_buf
    /// band-limits real edges; a step near the full wave amplitude in one
    /// sample period is a click.
    max_step: f32,
    /// Zero-crossing dominant frequency over the loudest 0.5 s window.
    dominant_hz: Option<f64>,
}

fn analyze(samples: &[(f32, f32)]) -> Stats {
    let n = samples.len();
    let seconds = n as f64 / RATE as f64;
    let mut peak = 0f32;
    let mut sumsq = 0f64;
    let mut silent = 0usize;
    let mut run = 0usize;
    let mut longest_run = 0usize;
    let mut max_step = 0f32;
    let mut prev_l = 0f32;
    const SILENCE: f32 = 1e-4;
    for (i, &(l, r)) in samples.iter().enumerate() {
        peak = peak.max(l.abs()).max(r.abs());
        sumsq += (l as f64) * (l as f64) + (r as f64) * (r as f64);
        if l.abs() < SILENCE && r.abs() < SILENCE {
            silent += 1;
            run += 1;
            longest_run = longest_run.max(run);
        } else {
            run = 0;
        }
        if i > 0 {
            max_step = max_step.max((l - prev_l).abs());
        }
        prev_l = l;
    }
    let rms = if n > 0 {
        (sumsq / (2.0 * n as f64)).sqrt()
    } else {
        0.0
    };

    // Dominant frequency: loudest 0.5 s window (by energy), zero crossings of
    // the AC-coupled left channel.
    let win = (RATE / 2) as usize;
    let dominant_hz = if n >= win {
        let mut best_start = 0usize;
        let mut best_energy = -1.0f64;
        let step = win / 4;
        let mut start = 0;
        while start + win <= n {
            let e: f64 = samples[start..start + win]
                .iter()
                .map(|&(l, _)| (l as f64) * (l as f64))
                .sum();
            if e > best_energy {
                best_energy = e;
                best_start = start;
            }
            start += step;
        }
        let w = &samples[best_start..best_start + win];
        let mean: f64 = w.iter().map(|&(l, _)| l as f64).sum::<f64>() / win as f64;
        let mut crossings = 0usize;
        let mut prev = w[0].0 as f64 - mean;
        for &(l, _) in &w[1..] {
            let v = l as f64 - mean;
            if prev <= 0.0 && v > 0.0 {
                crossings += 1;
            }
            prev = v;
        }
        if best_energy > 1e-6 {
            Some(crossings as f64 / (win as f64 / RATE as f64))
        } else {
            None
        }
    } else {
        None
    };

    Stats {
        samples: n,
        seconds,
        peak,
        rms,
        silent_frac: if n > 0 { silent as f64 / n as f64 } else { 1.0 },
        longest_silence_ms: longest_run as f64 * 1000.0 / RATE as f64,
        max_step,
        dominant_hz,
    }
}

// ---------------------------------------------------------------------------
// WAV writer (16-bit PCM stereo) + FNV-1a hash — hand-rolled, no deps
// ---------------------------------------------------------------------------

fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

fn interleave_i16(samples: &[(f32, f32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 4);
    for &(l, r) in samples {
        out.extend_from_slice(&to_i16(l).to_le_bytes());
        out.extend_from_slice(&to_i16(r).to_le_bytes());
    }
    out
}

fn write_wav(path: &Path, pcm: &[u8], rate: u32) {
    let mut w = Vec::with_capacity(44 + pcm.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVE");
    w.extend_from_slice(b"fmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&2u16.to_le_bytes()); // stereo
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 4).to_le_bytes()); // byte rate
    w.extend_from_slice(&4u16.to_le_bytes()); // block align
    w.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(pcm);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, w).unwrap();
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// ---------------------------------------------------------------------------
// Fixtures + baseline
// ---------------------------------------------------------------------------

struct AudioSpec {
    id: &'static str,
    /// None = the built-in tone ROM (synthesised into the work dir).
    rom: Option<&'static str>,
    frames: u32,
    /// Expected Blargg $A000 verdict, if the ROM reports one. Pinned
    /// explicitly (not just via the sample hash) so a pass→fail regression
    /// is reported as exactly that, never as an opaque hash mismatch.
    blargg_verdict: Option<u8>,
}

const SPECS: &[AudioSpec] = &[
    AudioSpec {
        id: "tone440",
        rom: None,
        frames: 300, // ~5 s
        blargg_verdict: None,
    },
    AudioSpec {
        id: "blargg/dmg_sound-01-registers",
        rom: Some("tests/dmg_sound/rom_singles/01-registers.gb"),
        frames: 300,
        blargg_verdict: Some(0x00),
    },
    AudioSpec {
        id: "blargg/dmg_sound",
        rom: Some("tests/dmg_sound/dmg_sound.gb"),
        frames: 2200,               // full suite needs ~35 s of emulated time
        blargg_verdict: Some(0x00), // 12/12 as of the T-cycle wave-window work
    },
];

fn baseline_path() -> PathBuf {
    PathBuf::from("testharness/audio_baseline.txt")
}

fn load_baseline() -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    let text = std::fs::read_to_string(baseline_path()).unwrap_or_default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

fn artifact_dir() -> PathBuf {
    PathBuf::from("target/testrom-artifacts/audio")
}

/// Stage a ROM into the artifact work dir (the MBC writes .gbsave next to the
/// ROM path; never dirty the submodule checkout).
fn staged_rom(spec: &AudioSpec) -> PathBuf {
    let dir = artifact_dir().join("work");
    std::fs::create_dir_all(&dir).unwrap();
    let dst = dir.join(spec.id.replace('/', "__")).with_extension("gb");
    let data = match spec.rom {
        Some(p) => std::fs::read(p).unwrap_or_else(|e| {
            panic!("cannot read ROM {p}: {e} (submodule initialised?)")
        }),
        None => build_tone_rom(),
    };
    std::fs::write(&dst, data).unwrap();
    dst
}

fn main() {
    // Single-ROM TDD mode: AUDIO_ROM=<path> [AUDIO_FRAMES=<n>] runs one ROM,
    // prints its stats + Blargg $A000 verdict, and exits non-zero unless the
    // verdict is pass. The red→green flip of one dmg_sound sub-test is
    // observed through exactly this mode.
    if let Ok(rom) = std::env::var("AUDIO_ROM") {
        let frames: u32 = std::env::var("AUDIO_FRAMES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3600);
        let cap = run_capture_inner(Path::new(&rom), frames, true);
        let stats = analyze(&cap.samples);
        println!(
            "{rom}: {} samples ({:.2}s emulated), rms {:.4}, peak {:.3}",
            stats.samples, stats.seconds, stats.rms, stats.peak
        );
        match &cap.blargg_status {
            Some((status, text)) => {
                println!("blargg verdict: ${status:02X}\n---\n{}\n---", text.trim());
                std::process::exit(if *status == 0 { 0 } else { 1 });
            }
            None => {
                println!("no blargg signature at $A001-3 (ran {} cycles)", cap.cycles);
                std::process::exit(2);
            }
        }
    }

    let update = std::env::var("UPDATE_AUDIO_BASELINE").is_ok();
    let baseline = load_baseline();
    let mut failures: Vec<String> = Vec::new();
    let mut new_baseline = String::from(
        "# Audio-capture baseline: FNV-1a64 of the 16-bit interleaved PCM stream.\n\
         # A changed hash means the emulator now produces different samples for the\n\
         # same ROM — inspect the WAV in target/testrom-artifacts/audio/ and re-bless\n\
         # deliberately with:\n\
         #\n\
         #   UPDATE_AUDIO_BASELINE=1 cargo test --release --test audiocapture\n\
         #\n\
         # These hashes are MODEL-dependent: the fixtures boot whichever console\n\
         # the cartridge header asks for (dmg_sound is a DMG cartridge, so a DMG),\n\
         # and a different boot state shifts the sample stream. `GB_MODEL=cgb`\n\
         # reproduces the pre-DMG-mode numbers. The ROM verdicts pinned per\n\
         # fixture below are what actually asserts accuracy; the hash only pins\n\
         # \"the samples did not change\".\n\n",
    );

    println!(
        "{:<34} {:>9} {:>8} {:>7} {:>7} {:>7} {:>8} {:>9}  NOTES",
        "FIXTURE", "SAMPLES", "SECONDS", "RMS", "PEAK", "SILENT%", "MAXSTEP", "DOMINANT"
    );
    println!("{}", "-".repeat(110));

    for spec in SPECS {
        let rom = staged_rom(spec);
        let cap = run_capture(&rom, spec.frames);

        // --- Determinism: a second run must be byte-identical. ---
        let cap2 = run_capture(&rom, spec.frames);
        if cap.samples != cap2.samples {
            failures.push(format!(
                "{}: NON-DETERMINISTIC capture (run1 {} samples, run2 {})",
                spec.id,
                cap.samples.len(),
                cap2.samples.len()
            ));
            continue;
        }

        let stats = analyze(&cap.samples);
        let pcm = interleave_i16(&cap.samples);
        let hash = fnv1a64(&pcm);
        write_wav(
            &artifact_dir().join(format!("{}.wav", spec.id.replace('/', "__"))),
            &pcm,
            RATE,
        );

        // --- Rate invariant: samples ≈ cycles * rate / clock. blip_buf emits
        // in 2000-sample blocks, so allow a few blocks of tail shortfall but
        // never an overshoot beyond one block. A count that drifts with run
        // length is a rate bug. ---
        let expected = (cap.cycles as f64 * RATE as f64 / CLOCK as f64) as i64;
        let got = stats.samples as i64;
        let block = 2000i64;
        if got > expected + block || got < expected - 4 * block {
            failures.push(format!(
                "{}: sample-count drift: got {} expected ~{} over {} cycles \
                 ({:+} samples, {:+.3}%)",
                spec.id,
                got,
                expected,
                cap.cycles,
                got - expected,
                (got - expected) as f64 * 100.0 / expected as f64
            ));
        }

        let mut notes = Vec::new();
        if let Some((status, text)) = &cap.blargg_status {
            notes.push(format!("blargg=${status:02X} {:?}", text.trim()));
        }

        // --- Pinned Blargg verdict: a pass must stay a pass. ---
        if let Some(expected) = spec.blargg_verdict {
            match &cap.blargg_status {
                Some((status, text)) if *status != expected => {
                    failures.push(format!(
                        "{}: blargg verdict ${status:02X}, expected ${expected:02X}: {:?}",
                        spec.id,
                        text.trim()
                    ));
                }
                None => failures.push(format!(
                    "{}: no blargg verdict at $A000, expected ${expected:02X}",
                    spec.id
                )),
                _ => {}
            }
        }

        // --- Fixture-specific absolute expectations ---
        match spec.id {
            "tone440" => {
                // Known tone: pitch, level, continuity are all predictable.
                match stats.dominant_hz {
                    Some(hz) if (hz / TONE_EXPECTED_HZ - 1.0).abs() < 0.02 => {
                        notes.push(format!("pitch ok ({hz:.1} Hz)"));
                    }
                    other => failures.push(format!(
                        "tone440: dominant {:?}, expected ~{TONE_EXPECTED_HZ:.1} Hz",
                        other
                    )),
                }
                // Ch1 vol 15 mixes to 0.25 peak per side.
                if stats.peak < 0.2 || stats.peak > 0.3 {
                    failures
                        .push(format!("tone440: peak {:.3}, expected ~0.25", stats.peak));
                }
                // A sustained tone must not go quiet after the trigger.
                if stats.longest_silence_ms > 200.0 {
                    failures.push(format!(
                        "tone440: {:.0} ms of silence in a sustained tone",
                        stats.longest_silence_ms
                    ));
                }
                // blip_buf spreads a square edge over its filter width; the
                // full 0.5 swing never lands inside one sample period. A step
                // >= the full swing means an unfiltered discontinuity (click).
                if stats.max_step >= 0.5 {
                    failures.push(format!(
                        "tone440: max step {:.3} — unfiltered discontinuity",
                        stats.max_step
                    ));
                }
            }
            "blargg/dmg_sound" => {
                // The suite beeps its way through 12 subtests: the capture
                // must contain actual sound, not silence.
                if stats.rms < 1e-3 {
                    failures.push(format!(
                        "dmg_sound: capture is silent (rms {:.5})",
                        stats.rms
                    ));
                }
            }
            _ => {}
        }

        // --- Pinned hash baseline ---
        let value = format!("HASH:{hash:016x} SAMPLES:{}", stats.samples);
        new_baseline.push_str(&format!("{} = {}\n", spec.id, value));
        if !update {
            match baseline.get(spec.id) {
                Some(e) if *e == value => {}
                Some(e) => failures.push(format!(
                    "{}: baseline mismatch: expected {e}, got {value} \
                     (listen to the WAV, then re-bless if intended)",
                    spec.id
                )),
                None => failures.push(format!(
                    "{}: not in audio baseline (run with UPDATE_AUDIO_BASELINE=1)",
                    spec.id
                )),
            }
        }

        println!(
            "{:<34} {:>9} {:>8.2} {:>7.4} {:>7.3} {:>7.1} {:>8.4} {:>9}  {}",
            spec.id,
            stats.samples,
            stats.seconds,
            stats.rms,
            stats.peak,
            stats.silent_frac * 100.0,
            stats.max_step,
            stats
                .dominant_hz
                .map(|h| format!("{h:.1}Hz"))
                .unwrap_or_else(|| "-".into()),
            notes.join(" ")
        );
    }

    if update {
        std::fs::write(baseline_path(), &new_baseline).unwrap();
        println!("\naudio baseline written to {}", baseline_path().display());
        return;
    }

    if !failures.is_empty() {
        eprintln!("\n❌ {} audio check(s) failed:", failures.len());
        for f in &failures {
            eprintln!("    {f}");
        }
        std::process::exit(1);
    }
    println!(
        "\nAll audio captures match the baseline. ✅ (WAVs in {})",
        artifact_dir().display()
    );
}
