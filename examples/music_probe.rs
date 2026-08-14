//! Music mode, headless — what the shelf would show, as text.
//!
//! The shelf is a picture and a sound, and neither can be asserted in a terminal
//! log. This is how a claim about it gets *evidence*: point it at a directory of
//! cartridges and it prints, per album, whether a usable cover was found and
//! whether anything was heard — the two things music mode promises.
//!
//! ```text
//! cargo run --release --example music_probe -- ~/roms
//! cargo run --release --example music_probe -- ~/roms --covers /tmp/covers
//! cargo run --release --example music_probe -- ~/roms --wav /tmp/wav --seconds 30
//! cargo run --release --example music_probe -- --demo        # built-in fixtures
//! ```
//!
//! **`--covers` and `--wav` write art and audio derived from the cartridge, so
//! they must never be pointed inside this repository.** The tool refuses a
//! destination under the working tree for exactly that reason: what it produces
//! belongs to whoever owns the ROM, and this repo is public and MPL-2.0.

use std::path::{Path, PathBuf};
use terminalgb::music::{self, capture, cover, library, AudioVerdict};
use terminalgb::screen::romlib;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        usage();
        return;
    }

    let mut dir: Option<PathBuf> = None;
    let mut covers: Option<PathBuf> = None;
    let mut wav: Option<PathBuf> = None;
    let mut seconds = 30.0f64;
    let mut probe_seconds = library::ProbeOptions::default().seconds;
    let mut demo = false;
    let mut timeline = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--demo" => demo = true,
            "--timeline" => timeline = true,
            "--covers" => {
                i += 1;
                covers = args.get(i).map(PathBuf::from);
            }
            "--wav" => {
                i += 1;
                wav = args.get(i).map(PathBuf::from);
            }
            "--seconds" => {
                i += 1;
                seconds = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(seconds);
            }
            "--probe-seconds" => {
                i += 1;
                probe_seconds = args
                    .get(i)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(probe_seconds);
            }
            other if !other.starts_with('-') => dir = Some(PathBuf::from(other)),
            other => {
                eprintln!("music_probe: unknown flag {other}");
                usage();
                std::process::exit(2);
            }
        }
        i += 1;
    }

    for (what, p) in [("--covers", covers.as_ref()), ("--wav", wav.as_ref())] {
        if let Some(p) = p {
            if inside_repo(p) {
                eprintln!(
                    "music_probe: refusing {what} {} — it is inside this repository.\n\
                     Art and audio decoded from a cartridge are not ours to commit; \
                     write them somewhere outside the working tree.",
                    p.display()
                );
                std::process::exit(2);
            }
        }
    }

    let opts = library::ProbeOptions {
        seconds: probe_seconds,
        cover: cover::Options::default(),
    };

    // The work list: either the built-in fixtures (no ROM needed at all) or a
    // directory of the user's own cartridges.
    let roms: Vec<(romlib::RomEntry, Vec<u8>)> = if demo {
        vec![
            fixture("tone.gb", music::testrom::tone_rom()),
            fixture("silence.gb", music::testrom::silent_rom()),
            fixture("blank.gb", music::testrom::blank_rom()),
        ]
    } else {
        let Some(dir) = dir else {
            usage();
            std::process::exit(2);
        };
        let entries = romlib::scan_dir(&dir);
        if entries.is_empty() {
            eprintln!("music_probe: no .gb/.gbc ROMs in {}", dir.display());
            std::process::exit(1);
        }
        entries
            .into_iter()
            .filter_map(|e| {
                let data = std::fs::read(&e.path).ok()?;
                Some((e, data))
            })
            .collect()
    };

    println!(
        "MUSIC MODE probe — {} cartridge(s), {probe_seconds:.0}s of boot each\n",
        roms.len()
    );

    let mut with_cover = 0usize;
    let mut with_music = 0usize;
    for (entry, data) in &roms {
        if timeline {
            print_timeline(&entry.title, data, opts);
        }
        let album = library::probe(entry.clone(), data, opts);
        let cover_note = match album.covers.len() {
            0 => "no cover".to_string(),
            n => {
                with_cover += 1;
                let c = &album.covers[0];
                format!(
                    "{n} candidate(s), best from {:.1}s (score {})",
                    c.at_seconds(),
                    c.score
                )
            }
        };
        if album.verdict == AudioVerdict::Audible {
            with_music += 1;
        }
        println!("  {}", album.entry.title);
        println!(
            "    file    {}",
            album
                .entry
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
        println!(
            "    cart    {} · {} KiB",
            album.entry.mbc,
            album.entry.size / 1024
        );
        println!("    audio   {}", album.now_playing_label());
        println!(
            "    level   peak {:.3} · rms {:.4} · {:.0}% silent",
            album.level.peak,
            album.level.rms,
            album.level.silent_frac * 100.0
        );
        println!("    cover   {cover_note}");
        println!("    probe   {} ms", album.probe_ms);
        if let Some(e) = &album.error {
            println!("    error   {e}");
        }

        if let Some(dest) = covers.as_ref() {
            if let Some(c) = album.cover() {
                let p = dest.join(format!("{}.png", safe(&album.entry.title)));
                match write_png(&p, &c.rgba) {
                    Ok(()) => println!("    wrote   {}", p.display()),
                    Err(e) => println!("    cover write failed: {e}"),
                }
            }
        }
        if let Some(dest) = wav.as_ref() {
            let p = library::unique_export_path(
                dest,
                &library::export_name(&album.entry.title, seconds),
            );
            match library::export_wav(data, &p, seconds, album.start().state.as_deref()) {
                Ok(ex) => println!(
                    "    wrote   {} ({:.0}s, {} KiB, {})",
                    ex.path.display(),
                    ex.seconds,
                    ex.bytes / 1024,
                    ex.verdict.label()
                ),
                Err(e) => println!("    wav write failed: {e}"),
            }
        }
        println!();
    }

    println!(
        "{}/{} produced a usable cover · {}/{} produced audible music",
        with_cover,
        roms.len(),
        with_music,
        roms.len()
    );
    println!(
        "\nEvery album above is the '{}' source: {}.",
        library::TrackSource::Boot.label(),
        library::TrackSource::Boot.describe()
    );
    println!(
        "Sample rate {} Hz. Nothing was written beside any ROM.",
        capture::RATE
    );
}

fn fixture(name: &str, rom: Vec<u8>) -> (romlib::RomEntry, Vec<u8>) {
    let entry = romlib::entry_from_bytes(Path::new(name), &rom);
    (entry, rom)
}

fn usage() {
    eprintln!(
        "usage: music_probe <ROM-DIR> [--covers DIR] [--wav DIR] [--seconds N] \
         [--probe-seconds N]\n       music_probe --demo\n\n\
         Prints what music mode's shelf would show for every cartridge in a \
         directory:\n  whether a usable cover was found, and whether anything \
         was heard.\n\n\
         --covers / --wav write files derived from the cartridge and are \
         refused inside this repository."
    );
}

/// Refuse a destination inside the working tree. Art and audio decoded from a
/// commercial cartridge are not ours to commit — see `docs/music-mode.md`.
fn inside_repo(p: &Path) -> bool {
    let Ok(cwd) = std::env::current_dir() else {
        return false;
    };
    // `target/` is build output and already ignored, so it is not a hazard.
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    abs.starts_with(&cwd) && !abs.starts_with(cwd.join("target"))
}

fn safe(title: &str) -> String {
    title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn write_png(path: &Path, rgba: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let img = image::RgbaImage::from_raw(160, 144, rgba.to_vec())
        .ok_or_else(|| "frame is not 160x144 RGBA".to_string())?;
    img.save(path).map_err(|e| e.to_string())
}

/// Every sampled frame, with its score and how far it moved from the previous
/// one. The diagnostic for the cover heuristic: a title screen shows up as a
/// long run of frames that barely move.
fn print_timeline(title: &str, rom: &[u8], opts: library::ProbeOptions) {
    println!("  timeline for {title}");
    let mut prev: Option<Vec<u8>> = None;
    let _ = capture::record_with(
        rom,
        capture::Options {
            frames: capture::frames_for(opts.seconds),
            ..capture::Options::default()
        },
        |view| {
            let frame = view.number;
            if frame < opts.cover.warmup || frame % opts.cover.sample_every != 0 {
                return;
            }
            let rgba = view.rgba();
            let same = prev
                .as_ref()
                .map(|p| cover::similar(p, rgba))
                .unwrap_or(false);
            println!(
                "    f{frame:<5} {:>5.1}s  score {:>5}  detail {:>4}  {}",
                frame as f64 / capture::FPS,
                cover::score(rgba),
                cover::detail(rgba),
                if same { "same" } else { "CUT" }
            );
            prev = Some(rgba.to_vec());
        },
    );
    println!();
}
