//! The receiving half of the patch-list-offset probe: drives a real Pokémon
//! Blue into the Cable Club against `pw-link`'s `ace_probe` example, which
//! sends a protocol-legal trade stream whose patch list carries two
//! out-of-range entries (see `ace_probe.rs`'s doc comment for the arithmetic).
//! Dumps a WRAM window around $D8A4 (`wEnemyMons`) through $DAC0 before
//! anything connects and again right after the exchange completes, so the two
//! can be diffed by hand.
//!
//! Stops as soon as the bot reaches `Stage::AtMachine` -- the exchange has to
//! be complete by then, since the received party must already be patched to
//! be shown in the selection menu -- and never selects or confirms a trade.
//!
//! ```sh
//! # in pokewalker/software: cargo run --release --example ace_probe
//! cargo run --release --features image,plugin-gen1-link,plugin-trainer \
//!     --example ace_probe_dump -- --rom blue.gb --save room.sav \
//!     --connect 127.0.0.1:8811 --out /tmp/ace-probe
//! ```

use std::path::PathBuf;

use terminalgb::bgblink::BgbLink;
use terminalgb::gameboy::Gameboy;
use terminalgb::link::TransportLink;

#[path = "support/gen1_cable_club.rs"]
mod cable_club;
use cable_club::*;

const WINDOW_START: u16 = 0xD8A4; // wEnemyMons
const WINDOW_LEN: usize = 0xDAC0 - 0xD8A4;

fn dump(gb: &mut Gameboy) -> Vec<u8> {
    (0..WINDOW_LEN).map(|i| gb.peek(WINDOW_START + i as u16)).collect()
}

fn print_targets(label: &str, w: &[u8]) {
    let at = |addr: u16| w[(addr - WINDOW_START) as usize];
    println!(
        "{label}: $DA58 = {:#04x}   $DA80 = {:#04x}",
        at(0xDA58),
        at(0xDA80)
    );
}

fn main() {
    let mut rom: Option<PathBuf> = None;
    let mut save: Option<PathBuf> = None;
    let mut connect: Option<String> = None;
    let mut out = PathBuf::from("target/ace-probe");
    let mut frames = 60 * 60 * 3;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rom" => rom = args.next().map(PathBuf::from),
            "--save" => save = args.next().map(PathBuf::from),
            "--connect" => connect = args.next(),
            "--out" => out = args.next().map(PathBuf::from).unwrap_or(out),
            "--frames" => frames = args.next().and_then(|s| s.parse().ok()).unwrap_or(frames),
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let (Some(rom), Some(connect)) = (rom, connect) else {
        eprintln!("usage: ace_probe_dump --rom ROM --save SAV --connect HOST:PORT [--out DIR]");
        std::process::exit(2);
    };
    std::fs::create_dir_all(&out).expect("out dir");

    let scratch = PathBuf::from("target/ace-probe-stage");
    let staged = stage("ace", &rom, save.as_deref(), &scratch);
    patch_party(
        &staged,
        "trainer = BLUE\n\
         species = Machop\nlevel = 11\nnickname = ROCKY\not_name = BLUE\not_id = 1717\n\
         moves = Karate Chop, Low Kick\n",
    );

    let rom_bytes = std::fs::read(&staged.rom).expect("staged rom");
    let mut gb = Gameboy::new(rom_bytes, Some(staged.rom.clone()));
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));

    let before = dump(&mut gb);
    std::fs::write(out.join("before.bin"), &before).unwrap();
    print_targets("before", &before);

    let transport = BgbLink::connect(connect.as_str()).expect("connect to the probe");
    let mut link = TransportLink::new(Box::new(transport));
    let mut bot = TradeBot::default();

    for frame in 0..frames {
        if bot.stage() != Stage::AtMachine {
            bot.act(&mut gb, true, "ace");
        }
        link.run_frame(&mut gb);
        if bot.stage() == Stage::AtMachine {
            println!("at the trade machine at frame {frame} -- exchange should be complete");
            let after = dump(&mut gb);
            std::fs::write(out.join("after.bin"), &after).unwrap();
            print_targets("after ", &after);
            if before != after {
                println!(
                    "DIFFERENCE: {} byte(s) in the window changed",
                    before.iter().zip(after.iter()).filter(|(a, b)| a != b).count()
                );
            } else {
                println!("no difference in the window at all");
            }
            link.disconnect();
            return;
        }
    }
    eprintln!("gave up after {frames} frames without reaching the trade machine (stage: {:?})", bot.stage());
    link.disconnect();
    std::process::exit(1);
}
