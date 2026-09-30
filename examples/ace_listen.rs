//! Diagnostic-only harness for `/home/aaron/ace-lab` (a standalone, separate
//! project -- see its own module doc). Drives a real Pokémon Blue up to
//! "waiting for a trade partner" and then freezes the bot, letting
//! `ace-lab` connect as clock master and run its overflow externally. Dumps
//! the bytes `ace-lab` is supposed to change ($D31D bag count, $DA80 box
//! count, $DA58) before and after a long idle wait, so the result can be read
//! off without any manual interaction.
//!
//! Not wired into any other example; exists to verify `ace-lab` against a
//! real (always staged, never the player's own) ROM copy.
//!
//! ```sh
//! cargo run --release --features plugin-gen1-link,bgblink \
//!     --example ace_listen -- --rom blue.gb --save room.sav \
//!     --listen 127.0.0.1:8790 --wait-secs 180
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use terminalgb::bgblink::BgbLink;
use terminalgb::gameboy::Gameboy;
use terminalgb::link::TransportLink;

#[path = "support/gen1_cable_club.rs"]
mod cable_club;
use cable_club::*;

fn peek_range(gb: &mut Gameboy, addr: u16, len: usize) -> Vec<u8> {
    (0..len).map(|i| gb.peek(addr + i as u16)).collect()
}

fn report(label: &str, gb: &mut Gameboy) {
    let items = gb.peek(0xD31D);
    let box_count = gb.peek(0xDA80);
    let da58 = peek_range(gb, 0xDA58, 3);
    println!("{label}: bag items={items}  box count={box_count}  $DA58={da58:02X?}");
}

fn main() {
    let mut rom: Option<PathBuf> = None;
    let mut save: Option<PathBuf> = None;
    let mut listen = "127.0.0.1:8790".to_string();
    let mut wait_secs = 180u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rom" => rom = args.next().map(PathBuf::from),
            "--save" => save = args.next().map(PathBuf::from),
            "--listen" => listen = args.next().unwrap_or(listen),
            "--wait-secs" => wait_secs = args.next().and_then(|s| s.parse().ok()).unwrap_or(wait_secs),
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(rom) = rom else {
        eprintln!("usage: ace_listen --rom ROM --save SAV --listen HOST:PORT [--wait-secs N]");
        std::process::exit(2);
    };

    let scratch = PathBuf::from("target/ace-listen-stage");
    let staged = stage("acelisten", &rom, save.as_deref(), &scratch);
    patch_party(
        &staged,
        "trainer = BLUE\n\
         species = Machop\nlevel = 11\nnickname = ROCKY\not_name = BLUE\not_id = 1717\n\
         moves = Karate Chop, Low Kick\n",
    );

    let rom_bytes = std::fs::read(&staged.rom).expect("staged rom");
    let mut gb = Gameboy::new(rom_bytes, Some(staged.rom.clone()));
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));

    report("before", &mut gb);

    let transport = BgbLink::listen(&listen).expect("listen");
    let mut link = TransportLink::new(Box::new(transport));
    println!("listening on {listen} -- run ace-lab against this address now");

    let mut bot = TradeBot::default();
    let mut frozen_at = None;
    let deadline = Instant::now() + Duration::from_secs(wait_secs);
    let mut last_report = Instant::now();
    let mut frame = 0u64;
    loop {
        if bot.stage() != Stage::TalkAndSave {
            bot.act(&mut gb, true, "acelisten");
        } else if frozen_at.is_none() {
            frozen_at = Some(frame);
            println!("frame {frame}: at the receptionist, waiting for a partner -- bot frozen here");
        }
        link.run_frame(&mut gb);
        frame += 1;
        if last_report.elapsed() >= Duration::from_secs(10) {
            last_report = Instant::now();
            report(&format!("frame {frame}"), &mut gb);
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    report("final", &mut gb);
    link.disconnect();
}
