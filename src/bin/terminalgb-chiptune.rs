//! `terminalgb-chiptune` — chiptune instructions in, real Game Boy audio out.
//!
//! ```text
//! terminalgb-chiptune phrase.gbscore -o phrase.wav
//! terminalgb-chiptune tune.vgm -o tune.wav --model cgb
//! terminalgb-chiptune phrase.gbscore --dump-registers
//! terminalgb-chiptune --example > phrase.gbscore
//! ```
//!
//! Argument parsing is by hand, like the rest of this repository's binaries —
//! a CLI parser would be a third-party crate in a tool whose whole boast is
//! that the music half has no dependencies at all. See `docs/chiptune.md`.

use std::io::{Read, Write};
use std::path::PathBuf;

use terminalgb::chiptune::{render, RenderOptions};
use terminalgb_chiptune::{compile_auto, track::Model, vgm, wav};

const USAGE: &str = "\
terminalgb-chiptune — render Game Boy sound-chip instructions to audio.

USAGE:
    terminalgb-chiptune <input> [options]
    terminalgb-chiptune --example [score|regs] > phrase.gbscore

INPUT
    A score (notes), a register script (NR10-NR52 with timings), or a `.vgm`
    log. The format is detected from the content, so the extension does not
    matter. `-` reads standard input.

OPTIONS
    -o, --out <file.wav>   Write a WAV here. Default: the input's name with
                           .wav, or standard output for `-`.
        --rate <hz>        Sample rate, 4000-192000. Default 44100.
        --model <dmg|cgb>  Which console's APU. A `model` line in the source
                           wins over this. Default dmg.
        --seconds <n>      Override the piece's length.
        --dump-registers   Print the compiled register writes and render
                           nothing. This is the ground truth the audio comes
                           from — the thing that cannot be wrong.
        --vgm <file.vgm>   Also write the piece as a VGM log, so any other
                           VGM player can play it back.
        --info             Print what was parsed (writes, length, model) and
                           render nothing.
        --quiet            No progress line on stderr.
    -h, --help             This text.
    -V, --version          Version.

The chip on the other end is this emulator's APU, not a synthesiser imitating
one: SameSuite's APU group 64/69, Blargg dmg_sound 12/12. See docs/chiptune.md.
";

/// A phrase written here, so the example ships no copyrighted music: a rising
/// arpeggio over a bass line, a swept "laser", and a noise drum.
const EXAMPLE_SCORE: &str = include_str!("../../docs/chiptune-examples/demo.gbscore");
const EXAMPLE_REGS: &str = include_str!("../../docs/chiptune-examples/tone.gbregs");

struct Args {
    input: Option<PathBuf>,
    out: Option<PathBuf>,
    rate: u32,
    model: Model,
    seconds: Option<f64>,
    dump: bool,
    info: bool,
    quiet: bool,
    vgm_out: Option<PathBuf>,
}

fn main() {
    match run() {
        Ok(()) => {}
        Err(e) => {
            eprintln!("terminalgb-chiptune: {e}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<(), String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        print!("{USAGE}");
        return Ok(());
    }
    let mut a = Args {
        input: None,
        out: None,
        rate: 44_100,
        model: Model::Dmg,
        seconds: None,
        dump: false,
        info: false,
        quiet: false,
        vgm_out: None,
    };
    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        let mut next = |what: &str| -> Result<String, String> {
            i += 1;
            argv.get(i)
                .cloned()
                .ok_or_else(|| format!("{what} needs a value"))
        };
        match arg {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("terminalgb-chiptune {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--example" => {
                let which = argv.get(i + 1).map(String::as_str).unwrap_or("score");
                match which {
                    "score" => print!("{EXAMPLE_SCORE}"),
                    "regs" | "registers" => print!("{EXAMPLE_REGS}"),
                    other => {
                        return Err(format!(
                            "--example takes score or regs, not '{other}'"
                        ))
                    }
                }
                return Ok(());
            }
            "-o" | "--out" => a.out = Some(PathBuf::from(next("--out")?)),
            "--vgm" => a.vgm_out = Some(PathBuf::from(next("--vgm")?)),
            "--rate" => {
                let v = next("--rate")?;
                a.rate = v.parse().map_err(|_| format!("bad sample rate '{v}'"))?;
                if !(4_000..=192_000).contains(&a.rate) {
                    return Err("sample rate must be between 4000 and 192000".into());
                }
            }
            "--model" => {
                let v = next("--model")?;
                a.model =
                    Model::parse(&v).ok_or_else(|| format!("unknown model '{v}'"))?;
            }
            "--seconds" => {
                let v = next("--seconds")?;
                let s: f64 = v.parse().map_err(|_| format!("bad duration '{v}'"))?;
                if !(s.is_finite() && s > 0.0) {
                    return Err("--seconds must be positive".into());
                }
                a.seconds = Some(s);
            }
            "--dump-registers" | "--dump" => a.dump = true,
            "--info" => a.info = true,
            "--quiet" | "-q" => a.quiet = true,
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option '{other}' — try --help"))
            }
            other => a.input = Some(PathBuf::from(other)),
        }
        i += 1;
    }

    let input = a.input.clone().ok_or("no input file — try --help")?;
    let bytes = if input.as_os_str() == "-" {
        let mut v = Vec::new();
        std::io::stdin()
            .read_to_end(&mut v)
            .map_err(|e| format!("reading standard input: {e}"))?;
        v
    } else {
        std::fs::read(&input).map_err(|e| format!("reading {}: {e}", input.display()))?
    };

    let mut track = compile_auto(&bytes, a.model)?;
    if let Some(s) = a.seconds {
        track.duration = (s * terminalgb_chiptune::CLOCK_HZ as f64).round() as u64;
        // A piece shortened below its last write would fail validation; drop
        // the writes that now fall outside it rather than refusing.
        track.writes.retain(|w| w.at <= track.duration);
    }

    if a.dump {
        print!("{}", track.to_register_script());
        return Ok(());
    }
    if a.info {
        println!("title:     {}", track.title.as_deref().unwrap_or("(none)"));
        println!(
            "model:     {} — {}",
            track.model.name(),
            track.model.describe()
        );
        println!("writes:    {}", track.writes.len());
        println!(
            "length:    {:.3} s ({} T-cycles)",
            track.seconds(),
            track.duration
        );
        println!("rate:      {} Hz", a.rate);
        let mut used = [false; 4];
        for w in &track.writes {
            match w.reg {
                0xFF10..=0xFF14 => used[0] = true,
                0xFF16..=0xFF19 => used[1] = true,
                0xFF1A..=0xFF1E | 0xFF30..=0xFF3F => used[2] = true,
                0xFF20..=0xFF23 => used[3] = true,
                _ => {}
            }
        }
        let names = ["square 1", "square 2", "wave", "noise"];
        let live: Vec<&str> = names
            .iter()
            .zip(used)
            .filter(|(_, u)| *u)
            .map(|(n, _)| *n)
            .collect();
        println!(
            "channels:  {}",
            if live.is_empty() {
                "(none)".to_string()
            } else {
                live.join(", ")
            }
        );
        return Ok(());
    }

    if let Some(p) = &a.vgm_out {
        std::fs::write(p, vgm::encode(&track))
            .map_err(|e| format!("writing {}: {e}", p.display()))?;
        if !a.quiet {
            eprintln!("wrote {}", p.display());
        }
    }

    let opts = RenderOptions {
        sample_rate: a.rate,
        model: track.model,
        ..Default::default()
    };
    let samples = render(&track, opts);

    let out = a.out.clone().unwrap_or_else(|| {
        if input.as_os_str() == "-" {
            PathBuf::from("-")
        } else {
            input.with_extension("wav")
        }
    });
    if out.as_os_str() == "-" {
        let bytes = wav::encode(&samples, a.rate);
        std::io::stdout()
            .write_all(&bytes)
            .map_err(|e| format!("writing standard output: {e}"))?;
    } else {
        wav::write(&out, &samples, a.rate)
            .map_err(|e| format!("writing {}: {e}", out.display()))?;
    }
    if !a.quiet {
        let peak = samples
            .iter()
            .fold(0f32, |m, &(l, r)| m.max(l.abs()).max(r.abs()));
        eprintln!(
            "{} — {} samples, {:.3} s at {} Hz, peak {:.3}, {} register writes on a {}",
            if out.as_os_str() == "-" {
                "standard output".to_string()
            } else {
                out.display().to_string()
            },
            samples.len(),
            samples.len() as f64 / a.rate as f64,
            a.rate,
            peak,
            track.writes.len(),
            track.model.name().to_uppercase(),
        );
        if peak == 0.0 {
            eprintln!(
                "warning: the render is silent. Check that the APU is powered on \
                 (NR52 bit 7), that NR50/NR51 route the channel, and that the \
                 channel's DAC is on (NRx2 non-zero, or NR30 bit 7 for the wave \
                 channel)."
            );
        }
    }
    Ok(())
}
