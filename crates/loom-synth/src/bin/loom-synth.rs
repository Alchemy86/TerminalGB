//! `loom-synth` — render a named preset, or compare two renders.
//!
//! ```text
//! loom-synth render chiptune-funk -o funk.wav --bars 8
//! loom-synth render analog-pad -o pad.wav --bars 8 --seed 7
//! loom-synth compare a.wav b.wav
//! ```
//!
//! Hand-rolled argument parsing, like `terminalgb-chiptune`'s own binary in
//! this workspace — a CLI parser would be a dependency in a crate whose
//! whole point is having none.

use std::path::PathBuf;

use loom_synth::sequence::Piece;
use loom_synth::{compare, presets, wav};

const USAGE: &str = "\
loom-synth — render synthesised pieces, or compare two renders.

USAGE:
    loom-synth render <preset> [OPTIONS]
    loom-synth compare <a.wav> <b.wav>
    loom-synth list

PRESETS
    chiptune-funk   ShowReel's reference chiptune, reproduced through this
                    engine's general primitives (see src/presets.rs).
    analog-pad      An original piece: a slow, filtered pad the chiptune
                    idiom cannot produce (no Game Boy channel has a filter).

RENDER OPTIONS
    -o, --out <file.wav>   Where to write the WAV. Default: <preset>.wav
        --bars <n>          How many bars to play. Default: 8
        --seed <n>          Seeds the noise-using voices. Default: 1
        --rate <hz>         Sample rate. Default: 48000

COMPARE
    Prints duration/peak/RMS for each file, plus windowed envelope- and
    brightness-correlation between them (see src/compare.rs's module docs
    for what those measure and why).
";

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&argv) {
        eprintln!("loom-synth: {e}");
        std::process::exit(1);
    }
}

fn run(argv: &[String]) -> Result<(), String> {
    match argv.first().map(String::as_str) {
        None | Some("-h") | Some("--help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some("list") => {
            println!("chiptune-funk  analog-pad");
            Ok(())
        }
        Some("render") => cmd_render(&argv[1..]),
        Some("compare") => cmd_compare(&argv[1..]),
        Some(other) => Err(format!("unknown command '{other}' — try --help")),
    }
}

fn piece_for(name: &str, bars: usize, seed: u64) -> Result<Piece, String> {
    match name {
        "chiptune-funk" | "chiptune" | "funk" => Ok(presets::chiptune_funk(bars, seed)),
        "analog-pad" | "pad" => Ok(presets::analog_pad(bars, seed)),
        other => Err(format!("unknown preset '{other}' — see `loom-synth list`")),
    }
}

fn cmd_render(argv: &[String]) -> Result<(), String> {
    let preset = argv
        .first()
        .ok_or("render needs a preset name — see `loom-synth list`")?;
    let mut out: Option<PathBuf> = None;
    let mut bars: usize = 8;
    let mut seed: u64 = 1;
    let mut rate: u32 = 48_000;

    let mut i = 1;
    while i < argv.len() {
        let arg = argv[i].as_str();
        let mut next = |what: &str| -> Result<String, String> {
            i += 1;
            argv.get(i)
                .cloned()
                .ok_or_else(|| format!("{what} needs a value"))
        };
        match arg {
            "-o" | "--out" => out = Some(PathBuf::from(next("--out")?)),
            "--bars" => bars = next("--bars")?.parse().map_err(|_| "bad --bars")?,
            "--seed" => seed = next("--seed")?.parse().map_err(|_| "bad --seed")?,
            "--rate" => rate = next("--rate")?.parse().map_err(|_| "bad --rate")?,
            other => return Err(format!("unknown option '{other}'")),
        }
        i += 1;
    }
    let piece = piece_for(preset, bars, seed)?;
    let out = out.unwrap_or_else(|| PathBuf::from(format!("{preset}.wav")));

    let started = std::time::Instant::now();
    let samples = piece.render(rate as f64);
    wav::write_mono(&out, &samples, rate)
        .map_err(|e| format!("writing {}: {e}", out.display()))?;
    let secs = samples.len() as f64 / rate as f64;
    println!(
        "{}: {preset}, {bars} bars @ {:.1} bpm, {secs:.2}s, {} samples, rendered in {:.2?}",
        out.display(),
        piece.bpm,
        samples.len(),
        started.elapsed()
    );
    Ok(())
}

fn cmd_compare(argv: &[String]) -> Result<(), String> {
    if argv.len() != 2 {
        return Err("compare needs exactly two WAV paths".to_string());
    }
    let (a, ra, sra) = wav::read(std::path::Path::new(&argv[0]))
        .map_err(|e| format!("reading {}: {e}", argv[0]))?;
    let (b, rb, srb) = wav::read(std::path::Path::new(&argv[1]))
        .map_err(|e| format!("reading {}: {e}", argv[1]))?;
    if sra != srb {
        return Err(format!(
            "sample rates differ: {sra} Hz vs {srb} Hz — compare needs matching rates"
        ));
    }
    // Both files may be stereo (both this crate's own writer and a genuine
    // stereo WAV); collapse to mono by averaging channels rather than
    // assuming which layout a foreign file uses.
    let mono = |samples: &[f32], channels: u16| -> Vec<f32> {
        if channels <= 1 {
            samples.to_vec()
        } else {
            samples
                .chunks(channels as usize)
                .map(|c| c.iter().sum::<f32>() / channels as f32)
                .collect()
        }
    };
    let ma = mono(&a, ra);
    let mb = mono(&b, rb);
    let r = compare::compare(&ma, &mb, sra);
    println!(
        "a: {} — {:.2}s, peak {:.3}, rms {:.3}",
        argv[0], r.duration_secs_a, r.peak_a, r.rms_a
    );
    println!(
        "b: {} — {:.2}s, peak {:.3}, rms {:.3}",
        argv[1], r.duration_secs_b, r.peak_b, r.rms_b
    );
    println!("envelope correlation:   {:.4}", r.envelope_correlation);
    println!("brightness correlation: {:.4}", r.brightness_correlation);
    Ok(())
}
