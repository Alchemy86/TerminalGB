//! Diagnostic-only: drives a real Pokémon Blue into the Trade Center with
//! `gen1-walkerroom` attached (the emulator-only plugin that draws a custom
//! room by poking the running console's own memory), waits for it to install
//! the room, then dumps every address range it touches as a flat
//! `(addr, byte)` list -- so the exact same writes can be replayed through a
//! real memory-write channel instead (see `/home/aaron/ace-lab`), rather than
//! reimplementing the plugin's room-layout math a second time.
//!
//! ```sh
//! cargo run --release --features plugin-gen1-link,plugin-gen1-walkerroom,bgblink \
//!     --example room_dump -- --rom blue.gb --save room.sav --out /tmp/room-dump.txt
//! ```

use std::path::PathBuf;

use terminalgb::bgblink::BgbLink;
use terminalgb::gameboy::Gameboy;
use terminalgb::link::TransportLink;
use terminalgb::KeypadKey;
use terminalgb_plugin_gen1_walkerroom::WalkerRoomPlugin;

#[path = "support/gen1_cable_club.rs"]
mod cable_club;
use cable_club::*;

// From plugins/gen1-lobby/src/gen1.rs and gen1-walkerroom/src/lib.rs.
const W_OVERWORLD_MAP: u16 = 0xC6E8;
const V_TILESET: u16 = 0x9000;
const W_TILESET_BLOCKS_PTR: u16 = 0xD52C;
const W_TILESET_COLLISION_PTR: u16 = 0xD530;
const W_CUR_MAP_HEIGHT: u16 = 0xD368;
const W_CUR_MAP_WIDTH: u16 = 0xD369;
const W_CUR_MAP_HEIGHT_2: u16 = 0xD524;
const W_CUR_MAP_WIDTH_2: u16 = 0xD525;
const W_VIEW_POINTER: u16 = 0xD35F;
const W_TILE_MAP: u16 = 0xC3A0;

/// Ranges wide enough to cover the room's real footprint plus the blockset
/// and collision list that follow it in the same growing region, without
/// having to compute the plugin's exact layout math ourselves.
const RANGES: &[(&str, u16, usize)] = &[
    ("overworld_map+blockset+collision", W_OVERWORLD_MAP, 1200),
    ("tileset_vram", V_TILESET, 96 * 16),
    ("tile_map", W_TILE_MAP, 20 * 18),
];

fn main() {
    let mut rom: Option<PathBuf> = None;
    let mut save: Option<PathBuf> = None;
    let mut out = PathBuf::from("/tmp/room-dump.txt");
    let mut connect: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rom" => rom = args.next().map(PathBuf::from),
            "--save" => save = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from).unwrap_or(out),
            "--connect" => connect = args.next(),
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let (Some(rom), Some(connect)) = (rom, connect) else {
        eprintln!("usage: room_dump --rom ROM --save SAV --connect HOST:PORT [--out FILE]");
        std::process::exit(2);
    };

    let scratch = PathBuf::from("target/room-dump-stage");
    let staged = stage("roomdump", &rom, save.as_deref(), &scratch);
    patch_party(
        &staged,
        "trainer = BLUE\n\
         species = Machop\nlevel = 11\nnickname = ROCKY\not_name = BLUE\not_id = 1717\n\
         moves = Karate Chop, Low Kick\n",
    );

    let rom_bytes = std::fs::read(&staged.rom).expect("staged rom");
    let mut gb = Gameboy::new(rom_bytes, Some(staged.rom.clone()));
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    gb.add_plugin(Box::new(WalkerRoomPlugin::new())).expect("attach walker room");

    let transport = BgbLink::connect(connect.as_str()).expect("connect to a link partner");
    let mut link = TransportLink::new(Box::new(transport));

    let mut bot = TradeBot::default();
    let mut in_room_for = 0u32;
    let mut wrote = false;
    for frame in 0..(60 * 60 * 4) {
        if bot.stage() != Stage::InTradeRoom {
            bot.act(&mut gb, false, "roomdump");
        } else {
            in_room_for += 1;
        }
        link.run_frame(&mut gb);
        if in_room_for == 120 && !wrote {
            wrote = true;
            println!("in the room at frame {frame}; dumping");
            let mut lines = Vec::new();
            for &(label, base, len) in RANGES {
                let bytes: Vec<u8> = (0..len).map(|i| gb.peek(base + i as u16)).collect();
                let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                lines.push(format!("# {label}: {len} bytes from {base:#06x}\n{hex}"));
            }
            lines.push(format!(
                "# pointers: blocks_ptr={:#04x}{:02x} collision_ptr={:#04x}{:02x}",
                gb.peek(W_TILESET_BLOCKS_PTR + 1),
                gb.peek(W_TILESET_BLOCKS_PTR),
                gb.peek(W_TILESET_COLLISION_PTR + 1),
                gb.peek(W_TILESET_COLLISION_PTR),
            ));
            lines.push(format!(
                "# dims: w={} h={} w2={} h2={} view={:#04x}{:02x}",
                gb.peek(W_CUR_MAP_WIDTH),
                gb.peek(W_CUR_MAP_HEIGHT),
                gb.peek(W_CUR_MAP_WIDTH_2),
                gb.peek(W_CUR_MAP_HEIGHT_2),
                gb.peek(W_VIEW_POINTER + 1),
                gb.peek(W_VIEW_POINTER),
            ));
            std::fs::write(&out, lines.join("\n")).expect("write dump");
            println!("wrote {}", out.display());
            let _ = KeypadKey::A; // silence unused-import if art_data path changes
            return;
        }
    }
    eprintln!("gave up: never settled in the room (stage: {:?})", bot.stage());
    std::process::exit(1);
}
