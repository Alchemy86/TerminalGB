//! Headless scripted-input driver: play a real cartridge from a text script and
//! dump PNG frames.
//!
//! Automated suites (Blargg, Mooneye, acid2) boot a ROM and read a result. Some
//! behaviour only shows up *inside* a game — the one this was written for is the
//! battery save: you have to actually reach an in-game SAVE, lose the process,
//! relaunch, and see whether the title screen offers CONTINUE. This drives that
//! end to end with no display, no audio device and no commercial ROM committed.
//!
//! ```sh
//! cargo run --release --example rom_script -- <rom.gb> <script.txt> [outdir]
//! ```
//!
//! The ROM is copied into `outdir` first, so the battery files land there and the
//! source ROM directory is never touched. Script commands, one per line
//! (`#` starts a comment):
//!
//! ```text
//! idle <frames>                     run without input
//! tap  <key> [times]                press/release, 6 frames held
//! mash <key> <times> <hold> <gap>   repeated press/release
//! shot <name>                       write <frame>-<name>.png
//! state <name>                      write <name>.gbss (a save state)
//! sram                              report the save files on disk
//! ```
//!
//! Keys: `a b start select up down left right`.
//!
//! Environment:
//! * `PHASE=play` (default) deletes any existing save files first — a fresh
//!   cartridge. `PHASE=relaunch` keeps them, so the run sees the previous save.
//! * `EXIT=kill` calls `process::abort()` at the end instead of returning, so
//!   **no destructor runs**. That is what a closed window, a `kill`, a panic
//!   under `panic = 'abort'`, or winit's `process::exit` does — and it is the
//!   only honest way to test that a save was persisted while the emulator ran.
//!
//! A worked example (Pokémon Blue: new game, in-game SAVE, then killed) is in
//! `docs/testing.md`.

use gameboy::gameboy::Gameboy;
use gameboy::KeypadKey;
use std::path::{Path, PathBuf};

fn key(name: &str) -> Option<KeypadKey> {
    Some(match name {
        "a" => KeypadKey::A,
        "b" => KeypadKey::B,
        "start" => KeypadKey::Start,
        "select" => KeypadKey::Select,
        "up" => KeypadKey::Up,
        "down" => KeypadKey::Down,
        "left" => KeypadKey::Left,
        "right" => KeypadKey::Right,
        _ => return None,
    })
}

struct Driver {
    gb: Gameboy,
    frame: usize,
    outdir: PathBuf,
}

impl Driver {
    fn idle(&mut self, frames: usize) {
        for _ in 0..frames {
            self.gb.frame();
            self.frame += 1;
        }
    }

    /// Press `k` for `hold` frames, then release for `gap` frames.
    fn tap(&mut self, k: KeypadKey, hold: usize, gap: usize) {
        self.gb.keydown(k);
        self.idle(hold);
        self.gb.keyup(k);
        self.idle(gap);
    }

    fn shot(&mut self, name: &str) {
        let png = encode_png_rgb(self.gb.image(), 160, 144);
        let p = self.outdir.join(format!("{:06}-{name}.png", self.frame));
        std::fs::write(&p, png).unwrap();
        println!("  shot {}", p.display());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rom = PathBuf::from(&args[0]);
    let script = std::fs::read_to_string(&args[1]).unwrap();
    let outdir = PathBuf::from(
        args.get(2)
            .map(String::as_str)
            .unwrap_or("target/save-repro"),
    );
    std::fs::create_dir_all(&outdir).unwrap();

    // Work on a private copy so the captain's ROM directory is untouched.
    let work = outdir.join("cart.gb");
    std::fs::copy(&rom, &work).unwrap();

    let phase = std::env::var("PHASE").unwrap_or_else(|_| "play".into());
    if phase == "play" {
        // Fresh start: remove any stale saves next to the working copy.
        for ext in ["gbsave", "sav", "rtc"] {
            let _ = std::fs::remove_file(work.with_extension(ext));
        }
    }

    let data = std::fs::read(&work).unwrap();
    let mut d = Driver {
        gb: Gameboy::new(data, Some(work.clone())),
        frame: 0,
        outdir: outdir.clone(),
    };

    for line in script.lines() {
        let line = line.split('#').next().unwrap().trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts[0] {
            "idle" => {
                let n: usize = parts[1].parse().unwrap();
                d.idle(n);
            }
            "shot" => d.shot(parts.get(1).copied().unwrap_or("f")),
            "mash" => {
                // mash <key> <times> <hold> <gap>
                let k = key(parts[1]).unwrap();
                let times: usize = parts[2].parse().unwrap();
                let hold: usize = parts[3].parse().unwrap();
                let gap: usize = parts[4].parse().unwrap();
                for _ in 0..times {
                    d.tap(k, hold, gap);
                }
            }
            "tap" => {
                let k = key(parts[1]).unwrap();
                let n: usize = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
                for _ in 0..n {
                    d.tap(k, 6, 10);
                }
            }
            "state" => {
                // Freeze the machine here so a benchmark can start from real
                // gameplay instead of a title screen. Reaching the overworld in
                // Pokemon Blue takes ~4,500 scripted frames of menu navigation;
                // replaying that inside a timed run would measure the intro.
                let name = parts.get(1).copied().unwrap_or("state");
                let path = d.outdir.join(format!("{name}.gbss"));
                std::fs::write(&path, d.gb.save_state()).unwrap();
                println!("  state {}", path.display());
            }
            "sram" => {
                report_sram(&work.with_extension("sav"));
                report_sram(&work.with_extension("gbsave"));
            }
            other => panic!("unknown script command {other}"),
        }
    }

    println!("final frame {}", d.frame);
    if std::env::var("EXIT").as_deref() == Ok("kill") {
        // Exactly what closing the terminal window, a SIGKILL, or winit's
        // `process::exit` does: no destructors, no Drop, no last-gasp write.
        println!("  (aborting without running any destructor)");
        report_sram(&work.with_extension("sav"));
        report_sram(&work.with_extension("gbsave"));
        std::process::abort();
    }
    drop(d);
    report_sram(&work.with_extension("sav"));
    report_sram(&work.with_extension("gbsave"));
}

fn report_sram(path: &Path) {
    match std::fs::read(path) {
        Ok(b) => {
            let nz = b.iter().filter(|&&x| x != 0).count();
            println!("  {} -> {} bytes, {} non-zero", path.display(), b.len(), nz);
        }
        Err(e) => println!("  {} -> {e}", path.display()),
    }
}

// --- minimal PNG writer (borrowed from testharness/testroms.rs) -------------

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b): (u32, u32) = (1, 0);
    for &byte in bytes {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(tag);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc_input);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn encode_png_rgb(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut raw = Vec::with_capacity(height * (1 + width * 3));
    for y in 0..height {
        raw.push(0);
        for x in 0..width {
            let i = (y * width + x) * 4;
            raw.extend_from_slice(&rgba[i..i + 3]);
        }
    }
    let mut zlib = vec![0x78, 0x01];
    let mut pos = 0;
    while pos < raw.len() {
        let block = std::cmp::min(0xffff, raw.len() - pos);
        let last = if pos + block >= raw.len() { 1u8 } else { 0u8 };
        zlib.push(last);
        zlib.extend_from_slice(&(block as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(block as u16)).to_le_bytes());
        zlib.extend_from_slice(&raw[pos..pos + block]);
        pos += block;
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    png_chunk(&mut png, b"IHDR", &ihdr);
    png_chunk(&mut png, b"IDAT", &zlib);
    png_chunk(&mut png, b"IEND", &[]);
    png
}
