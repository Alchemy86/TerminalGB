//! # The walker's trade room -- a harness
//!
//! Drives a real Pokemon Blue into the Cable Club's Trade Center over the BGB
//! link (the partner is the walker: `pw-pet-window` or its `link_headless`), then
//! hands over: dumps what the cartridge has to build a room from, and takes
//! screenshots of whatever the walker-room plugin has made of it.
//!
//! ```sh
//! link_headless --listen 127.0.0.1:8801 --trade-dir /tmp/kept --timeout 600 &
//! cargo run --release --features image,plugin-gen1-link,plugin-gen1-walkerroom \
//!     --example walkerroom -- --rom blue.gb --save blue.sav --connect 127.0.0.1:8801 \
//!     --out target/walkerroom
//! ```
//!
//! The cartridge and save are **copied** first (see `bgb_link`); nothing here
//! opens a path in `~/roms` for writing.

use std::path::PathBuf;

use terminalgb::bgblink::BgbLink;
use terminalgb::gameboy::Gameboy;
use terminalgb::link::TransportLink;
use terminalgb::KeypadKey;
use terminalgb_plugin_gen1_walkerroom::{Choice, Kept, WalkerRoomPlugin};

#[path = "support/gen1_cable_club.rs"]
mod cable_club;

use cable_club::*;

fn main() {
    let mut rom: Option<PathBuf> = None;
    let mut save: Option<PathBuf> = None;
    let mut connect: Option<String> = None;
    let mut out = PathBuf::from("target/walkerroom");
    let mut frames = 60 * 60 * 6;
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
        eprintln!("usage: walkerroom --rom ROM --save SAV --connect HOST:PORT [--out DIR]");
        std::process::exit(2);
    };
    std::fs::create_dir_all(&out).expect("out dir");

    let scratch = PathBuf::from("target/walkerroom-stage");
    let staged = stage("room", &rom, save.as_deref(), &scratch);
    let transport = BgbLink::connect(connect.as_str()).expect("connect to the walker");
    let mut link = TransportLink::new(Box::new(transport));
    let rom_bytes = std::fs::read(&staged.rom).expect("staged rom");
    let mut gb = Gameboy::new(rom_bytes.clone(), Some(staged.rom.clone()));
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));

    // NO_ROOM=1 skips attaching the plugin entirely, for a controlled A/B: the same
    // real cable, the same real trade, with only this one local flag different --
    // proof that the room comes from the plugin poking memory, not from the wire.
    if std::env::var("NO_ROOM").is_err() {
        gb.add_plugin(Box::new(WalkerRoomPlugin::new())).expect("attach the walker room");
    } else {
        println!("NO_ROOM set: the walker room plugin is NOT attached this run");
    }
    let mut bot = TradeBot::default();
    let mut in_room_for = 0u32;
    for frame in 0..frames {
        // The bot brings us to the room and then is stood down: what happens in
        // the room is this harness's business.
        if bot.stage() != Stage::InTradeRoom {
            bot.act(&mut gb, false, "room");
        }
        link.run_frame(&mut gb);
        if bot.stage() == Stage::InTradeRoom {
            in_room_for += 1;
        }
        if in_room_for == 240 {
            println!("in the Trade Center at frame {frame}");
            dump(&mut gb, &rom_bytes, &out);
            tour(&mut gb, &mut link, &out);
            break;
        }
    }
    link.disconnect();
}

/// The cartridge's own Club blockset (64 blocks of 4x4 tile ids), its 96 tiles
/// of VRAM, and where the player is: raw, for a script to draw.
fn dump(gb: &mut Gameboy, rom: &[u8], out: &std::path::Path) {
    let bank = gb.peek(0xD52B) as usize;
    let ptr = gb.peek(0xD52C) as u16 | (gb.peek(0xD52D) as u16) << 8;
    let mut blocks = Vec::new();
    for i in 0..(64 * 16) {
        blocks.push(rom[bank * 0x4000 + (ptr as usize - 0x4000) + i]);
    }
    std::fs::write(out.join("blocks.bin"), &blocks).unwrap();
    let mut tiles = Vec::new();
    for i in 0..(128 * 16) {
        tiles.push(gb.peek(0x9000 + i as u16));
    }
    std::fs::write(out.join("tiles.bin"), &tiles).unwrap();
    let mut font = Vec::new();
    for i in 0..(128 * 16) {
        font.push(gb.peek(0x8800 + i as u16));
    }
    std::fs::write(out.join("font.bin"), &font).unwrap();
    let mut sprites = Vec::new();
    for i in 0..(128 * 16) {
        sprites.push(gb.peek(0x8000 + i as u16));
    }
    std::fs::write(out.join("sprites.bin"), &sprites).unwrap();
    println!(
        "map {:#04x} at ({}, {}); blockset bank {bank} ptr {ptr:#06x}",
        gb.peek(0xD35E),
        gb.peek(0xD362),
        gb.peek(0xD361)
    );
}

/// Let the machine run `n` frames on the link.
fn run(gb: &mut Gameboy, link: &mut TransportLink, n: u32) {
    for _ in 0..n {
        link.run_frame(gb);
    }
}

/// Hold `key` for `hold` frames, then let go and settle.
fn tap(gb: &mut Gameboy, link: &mut TransportLink, key: KeypadKey, hold: u32) {
    gb.keydown(key);
    run(gb, link, hold);
    gb.keyup(key);
    run(gb, link, 6);
}

fn at(gb: &mut Gameboy) -> (u8, u8) {
    (gb.peek(0xD362), gb.peek(0xD361))
}

/// Walk the room, taking a picture wherever there is something to see.
fn tour(gb: &mut Gameboy, link: &mut TransportLink, out: &std::path::Path) {
    run(gb, link, 90);
    write_one(&out.join("01-room.png"), gb);
    println!("standing at {:?}; plugin: {}", at(gb), gb.plugin_status_line().unwrap_or_default());
    if let Some(p) = gb.plugin_mut::<WalkerRoomPlugin>() {
        p.set_kept(vec![
            Kept { name: "SPARK".into(), level: 12 },
            Kept { name: "ROCKY".into(), level: 11 },
        ]);
    }
    // Down the room to the send pad's row, then across onto it.
    for _ in 0..8 {
        tap(gb, link, KeypadKey::Down, 16);
    }
    write_one(&out.join("02-south.png"), gb);
    tap(gb, link, KeypadKey::Right, 16);
    run(gb, link, 30);
    println!("standing at {:?}; plugin: {}", at(gb), gb.plugin_status_line().unwrap_or_default());
    write_one(&out.join("03-send-menu.png"), gb);
    let row = |gb: &mut Gameboy, r: u16| (0..20u16).map(|c| format!("{:02x}", gb.peek(0xC3A0 + r * 20 + c))).collect::<Vec<_>>().join(" ");
    println!("hAutoBGTransferEnabled {:#04x} portion {:#04x} wJoyIgnore {:#04x}", gb.peek(0xFFBA), gb.peek(0xFFBB), gb.peek(0xCD6B));
    for r in [8u16, 12, 14, 17] {
        println!("wTileMap row {r:2}: {}", row(gb, r));
    }
    // Say no, and go on to the other pad.
    tap(gb, link, KeypadKey::B, 4);
    run(gb, link, 20);
    write_one(&out.join("04-closed.png"), gb);
    // Round the walker's stand (which is solid) to the take-back pad.
    for _ in 0..2 {
        tap(gb, link, KeypadKey::Up, 16);
    }
    for _ in 0..12 {
        tap(gb, link, KeypadKey::Right, 16);
    }
    write_one(&out.join("04b-crossing.png"), gb);
    for _ in 0..2 {
        tap(gb, link, KeypadKey::Down, 16);
    }
    run(gb, link, 30);
    println!("standing at {:?}; plugin: {}", at(gb), gb.plugin_status_line().unwrap_or_default());
    write_one(&out.join("05-take-menu.png"), gb);
    tap(gb, link, KeypadKey::Down, 4);
    tap(gb, link, KeypadKey::A, 4);
    run(gb, link, 20);
    write_one(&out.join("06-take-note.png"), gb);
    tap(gb, link, KeypadKey::A, 4);
    run(gb, link, 20);
    let choice = gb.plugin_mut::<WalkerRoomPlugin>().and_then(|p| p.take_choice());
    println!("choice: {choice:?} (expected {:?})", Choice::TakeBack(1));
    write_one(&out.join("07-after.png"), gb);
}
