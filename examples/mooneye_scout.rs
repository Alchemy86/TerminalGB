//! Score the emulator against Mooneye GB's `acceptance` suite using **Mooneye's
//! own documented criterion**: run until the `LD B,B` (`$40`) magic breakpoint,
//! then require the Fibonacci registers `B=3 C=5 D=8 E=13 H=21 L=34`.
//!
//! Deliberately dependency-free and read-only: it never looks at a reference
//! image, so its verdict cannot drift with our own rendering. That criterion is
//! what the mooneye-test-suite README defines a pass to be; the GBEmulatorShootout
//! instead screenshot-compares, which additionally fails us on boot-register
//! differences that have nothing to do with the behaviour under test.
//!
//! usage: `mooneye_scout <dir-or-rom>...`
//!        `GB_SCOUT_TRACE=1` prints the register dump for every ROM.
//!        `GB_SCOUT_SCREEN=1` also prints the ROM's own assertion table, read
//!        back out of the BG tilemap (the suite loads its font so a tile id
//!        *is* the ASCII code of the glyph it draws — see AGENTS.md
//!        "Debugging a Mooneye failure: read the screen"). A failing test names
//!        the register it disagreed about, which the Fibonacci dump cannot.

use gameboy::cpu::core::Cpu;
use gameboy::sound::AudioPlayer;
use std::path::{Path, PathBuf};

/// Device-free sink. A real machine always has an APU installed; running
/// without one silently changes what some ROMs observe.
struct NullPlayer;
impl AudioPlayer for NullPlayer {
    fn play(&mut self, _l: &[f32], _r: &[f32]) {}
    fn samples_rate(&self) -> u32 {
        44_100
    }
    fn underflowed(&self) -> bool {
        false
    }
}

#[derive(PartialEq)]
enum Verdict {
    Pass,
    Fail,
    Timeout,
}

/// Read the screen back as text. Mooneye's `print_string` writes character
/// codes straight into the BG tilemap and its font is loaded so `tile id ==
/// ASCII code`, so the tilemap *is* the text on screen.
fn screen_text(cpu: &mut Cpu<'static>) -> String {
    let lcdc = cpu.memory.rb(0xFF40);
    let base: u16 = if lcdc & 0x08 != 0 { 0x9C00 } else { 0x9800 };
    let mut lines = Vec::new();
    for y in 0..18u16 {
        let mut line = String::new();
        for x in 0..20u16 {
            let tile = cpu.memory.rb(base + y * 32 + x);
            line.push(if (0x20..0x7F).contains(&tile) {
                tile as char
            } else {
                ' '
            });
        }
        let line = line.trim_end().to_string();
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines.join("\n")
}

fn run(rom: &Path) -> (Verdict, String) {
    let data = match std::fs::read(rom) {
        Ok(d) => d,
        Err(e) => return (Verdict::Timeout, format!("read error: {e}")),
    };
    let mut cpu = Cpu::new(data, Some(rom.to_path_buf()));
    cpu.memory.enable_audio(Box::new(NullPlayer));

    let mut budget: u64 = 400_000_000;
    loop {
        // `LD B,B` is the suite's magic breakpoint; peek so the fetch is not
        // perturbed by the check itself.
        if cpu.memory.peek(cpu.registers.pc) == 0x40 {
            let r = &cpu.registers;
            let ok =
                r.b == 3 && r.c == 5 && r.d == 8 && r.e == 13 && r.h == 21 && r.l == 34;
            let mut dump = format!(
                "b={:02X} c={:02X} d={:02X} e={:02X} h={:02X} l={:02X}",
                r.b, r.c, r.d, r.e, r.h, r.l
            );
            if std::env::var("GB_SCOUT_SCREEN").is_ok() {
                // The ROM has only just hit `LD B,B`; give it the frames it
                // needs to finish drawing its report before reading it back.
                for _ in 0..200_000 {
                    cpu.do_cycle();
                }
                dump.push('\n');
                dump.push_str(&screen_text(&mut cpu));
            }
            return (if ok { Verdict::Pass } else { Verdict::Fail }, dump);
        }
        budget = budget.saturating_sub(cpu.do_cycle() as u64);
        if budget == 0 {
            return (Verdict::Timeout, String::from("no LD B,B reached"));
        }
    }
}

fn collect(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(p)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        entries.sort();
        for e in entries {
            collect(&e, out);
        }
    } else if p.extension().map(|e| e == "gb").unwrap_or(false) {
        out.push(p.to_path_buf());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: mooneye_scout <dir-or-rom>...");
        std::process::exit(2);
    }
    let trace = std::env::var("GB_SCOUT_TRACE").is_ok();
    let mut roms = Vec::new();
    for a in &args {
        collect(Path::new(a), &mut roms);
    }

    // Stage every ROM in `target/` first: the MBC writes a `.sav` next to the
    // ROM path, and the suite checkout must stay clean.
    let stage =
        std::env::temp_dir().join(format!("mooneye-scout-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&stage);

    let (mut pass, mut fail, mut timeout) = (0, 0, 0);
    let mut failed = Vec::new();
    for rom in &roms {
        let staged = stage.join(rom.file_name().unwrap());
        std::fs::copy(rom, &staged).unwrap();
        let (v, dump) = run(&staged);
        let tag = match v {
            Verdict::Pass => {
                pass += 1;
                "PASS"
            }
            Verdict::Fail => {
                fail += 1;
                "FAIL"
            }
            Verdict::Timeout => {
                timeout += 1;
                "TIMEOUT"
            }
        };
        if tag != "PASS" {
            failed.push(rom.display().to_string());
        }
        if trace || tag != "PASS" {
            println!("{tag:7} {}  {dump}", rom.display());
        } else {
            println!("{tag:7} {}", rom.display());
        }
    }
    let _ = std::fs::remove_dir_all(&stage);
    println!(
        "\n{pass} pass, {fail} fail, {timeout} timeout  ({} total)",
        roms.len()
    );
}
