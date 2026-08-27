//! **Author a Gen 1 area in memory and walk into it through an ordinary door.**
//!
//! This is the experiment behind [`docs/gen1/authoring-an-area.md`]. It does
//! *not* redecorate the room the player is standing in — that was already
//! proven by the Cable Club work. It builds a **new destination** out of bytes
//! we wrote, leaves the player's own bedroom stairs exactly where the cartridge
//! points them, and lets the engine's own warp path load our room.
//!
//! The whole trick is one bit. `LoadMapHeader` (`home/overworld.asm`) opens:
//!
//! ```asm
//! ld a, [wCurMapTileset]
//! ld b, a
//! res BIT_NO_PREVIOUS_MAP, a
//! ld [wCurMapTileset], a
//! ldh [hPreviousTileset], a
//! bit BIT_NO_PREVIOUS_MAP, b
//! ret nz                      ; <- returns WITHOUT loading the header
//! ```
//!
//! `BIT_NO_PREVIOUS_MAP` is bit 7 (`constants/ram_constants.asm`). It is the
//! same latch `LoadSAV` sets, which is why continuing a save trusts the header
//! in `sMainData` — the trap `docs/gen1/memory-map.md` §4 warns about. Set it
//! deliberately and it stops being a trap: the engine skips the ROM header
//! load and `LoadMapData` carries straight on to `LoadTileBlockMap`, which
//! builds the room from **our** `wCurMapDataPtr`.
//!
//! Two runs, so the finding is a measurement and not an assertion:
//!
//! * `--latch` (default) — set the bit, walk downstairs, report what survived.
//! * `--no-latch` — identical writes, bit clear. Every byte the engine
//!   overwrote is printed. That is the clobber report.
//!
//! ```text
//! cargo run --release --features image --example warproom_probe -- \
//!     --rom ~/roms/blue.gb [--shots target/warproom/shots] [--no-latch]
//! ```
//!
//! Save safety: the ROM is copied into `target/` first (the MBC writes a `.sav`
//! beside the ROM), the machine is built with that copy's path, and the run
//! never opens a START menu, so `SaveMainData` never runs. Nothing in `~/roms`
//! is opened for writing.

use std::path::{Path, PathBuf};

use terminalgb::gameboy::Gameboy;
use terminalgb::plugin::Capability;
use terminalgb::KeypadKey;

// --- Addresses, every one from `pret/pokered`'s `pokeblue.sym` ------------
// The build was checked against `roms.sha1` (both retail ROMs OK), so these
// are exact for English Red and Blue. Cite the symbol, never the bare hex —
// see docs/gen1/README.md.

/// `wTileMap` — the 20x18 on-screen tile buffer. Collision reads *this*.
const W_TILE_MAP: u16 = 0xC3A0;
/// `wOverworldMap` — the live block map, 3-block border on every side.
const W_OVERWORLD_MAP: u16 = 0xC6E8;
/// `wOverworldMapEnd`. 1300 bytes total.
const W_OVERWORLD_MAP_END: u16 = 0xCBFC;
/// `wTileMapBackup2` — 360 bytes, the largest clean scratch gap below
/// `wMainDataStart`. Only written when a menu takes a screen snapshot.
const W_TILE_MAP_BACKUP2: u16 = 0xCD81;
/// `wIsInBattle`.
const W_IS_IN_BATTLE: u16 = 0xD057;
/// `wMainDataStart` — at or above this reaches `sMainData`, i.e. the save.
const W_MAIN_DATA_START: u16 = 0xD2F7;
/// `wCurMap`.
const W_CUR_MAP: u16 = 0xD35E;
/// `wCurrentTileBlockMapViewPointer`.
const W_VIEW_PTR: u16 = 0xD35F;
/// `wYCoord`.
const W_Y_COORD: u16 = 0xD361;
/// `wXCoord`.
const W_X_COORD: u16 = 0xD362;
/// `wYBlockCoord`.
const W_Y_BLOCK_COORD: u16 = 0xD363;
/// `wXBlockCoord`.
const W_X_BLOCK_COORD: u16 = 0xD364;
/// `wCurMapHeader` = `wCurMapTileset`. The latch lives in bit 7 of this byte.
const W_CUR_MAP_TILESET: u16 = 0xD367;
/// `wCurMapHeight`, in blocks.
const W_CUR_MAP_HEIGHT: u16 = 0xD368;
/// `wCurMapWidth`, in blocks.
const W_CUR_MAP_WIDTH: u16 = 0xD369;
/// `wCurMapDataPtr` — the block map source. `LoadTileBlockMap` dereferences it
/// with a plain `ld a, [de]`, so it may point at WRAM.
const W_CUR_MAP_DATA_PTR: u16 = 0xD36A;
/// `wCurMapConnections` — bitfield, 0 = no connected maps.
const W_CUR_MAP_CONNECTIONS: u16 = 0xD370;
/// `wMapBackgroundTile` — the block `LoadTileBlockMap` fills the border with.
const W_MAP_BACKGROUND_TILE: u16 = 0xD3AD;
/// `wNumberOfWarps`.
const W_NUM_WARPS: u16 = 0xD3AE;
/// `wWarpEntries` — `MAX_WARP_EVENTS` (32) x 4 bytes: Y, X, warp ID, map ID.
const W_WARP_ENTRIES: u16 = 0xD3AF;
/// `wNumSigns`.
const W_NUM_SIGNS: u16 = 0xD4B0;
/// `wNumSprites`.
const W_NUM_SPRITES: u16 = 0xD4E1;
/// `wCurrentMapHeight2` — **in walk tiles**. The edge-of-map check reads this,
/// not `wCurMapHeight`.
const W_CUR_MAP_HEIGHT2: u16 = 0xD524;
/// `wCurrentMapWidth2` — in walk tiles.
const W_CUR_MAP_WIDTH2: u16 = 0xD525;
/// `wTilesetBank`.
const W_TILESET_BANK: u16 = 0xD52B;
/// `wTilesetBlocksPtr` — 16 bytes per block, 4x4 screen tiles.
const W_TILESET_BLOCKS_PTR: u16 = 0xD52C;
/// `wTilesetCollisionPtr` — `$FF`-terminated list of passable tile ids.
const W_TILESET_COLLISION_PTR: u16 = 0xD530;
/// `wEventFlags`. Bit 0 of byte 0 is `EVENT_FOLLOWED_OAK_INTO_LAB`.
const W_EVENT_FLAGS: u16 = 0xD747;

/// `map_const REDS_HOUSE_2F, 4, 4 ; $26`.
const REDS_HOUSE_2F: u8 = 0x26;
/// `map_const REDS_HOUSE_1F, 4, 4 ; $25` — where the bedroom stairs already go.
const REDS_HOUSE_1F: u8 = 0x25;
/// `BIT_NO_PREVIOUS_MAP EQU 7`, `constants/ram_constants.asm`.
const BIT_NO_PREVIOUS_MAP: u8 = 1 << 7;
/// `MAP_BORDER EQU 3`, `constants/map_data_constants.asm`.
const MAP_BORDER: u16 = 3;
/// `vTileset` — the 96 background tiles (`MAP_TILESET_SIZE`) a map's art lives
/// in. `$9600` onward is the text box, `$8800` the font, `$8000` sprites.
const V_TILESET: u16 = 0x9000;
/// An unused-looking slot inside those 96, used only as a marker.
const SIGNATURE_TILE: u16 = 0x50;
/// The tile every fully-walkable block of this tileset is made of, and so the
/// one worth repainting to make the floor ours.
const FLOOR_TILE: u16 = 0x01;
/// `sMainData` (`ram/sram.asm`) — SRAM bank 1.
const S_MAIN_DATA: usize = 0xA5A3;

/// The room we author. 12 x 10 blocks = 24 x 20 walk tiles; the visible area is
/// 10 x 9 walk tiles, so about two and a half screens each way. Footprint
/// `(12+6)*(10+6) = 288` of `wOverworldMap`'s 1300 bytes.
const ROOM_W: u8 = 12;
const ROOM_H: u8 = 10;

/// `warp_event 7, 1, REDS_HOUSE_1F, 3` (`data/maps/objects/RedsHouse2F.asm`).
/// The macro emits `db y, x, id-1, map`, so the stairs are at walk tile
/// (x=7, y=1) — and that is where the player lands in our room, because
/// `LoadDestinationWarpPosition` is one of the things the skipped header load
/// would have done.
const STAIRS_X: u8 = 7;
const STAIRS_Y: u8 = 1;
/// The passable tile immediately west of the stairs. Reachable from the spawn
/// without opening a menu, and the last place the bedroom's own geometry is
/// still needed.
const DOORSTEP_X: u8 = 6;
const DOORSTEP_Y: u8 = 1;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let latch = !args.iter().any(|a| a == "--no-latch");
    let rom_path = flag(&args, "--rom")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("POKE_ROM").map(PathBuf::from))
        .unwrap_or_else(|| {
            eprintln!(
                "usage: warproom_probe --rom <blue.gb> [--shots <dir>] [--no-latch]"
            );
            eprintln!("   or: POKE_ROM=<blue.gb> cargo run ... --example warproom_probe");
            std::process::exit(2);
        });
    let shots = flag(&args, "--shots").map(PathBuf::from);
    if let Some(d) = &shots {
        std::fs::create_dir_all(d).expect("shots dir");
    }

    match run(&rom_path, latch, shots.as_deref()) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("FAILED: {e}");
            std::process::exit(1);
        }
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn run(rom_path: &Path, latch: bool, shots: Option<&Path>) -> Result<(), String> {
    println!("== a Gen 1 area authored in WRAM, entered through an ordinary door ==");
    println!(
        "mode: {}\n",
        if latch {
            "--latch (BIT_NO_PREVIOUS_MAP set: the engine must skip its own header load)"
        } else {
            "--no-latch (control run: the engine loads REDS_HOUSE_1F's real header over ours)"
        }
    );

    // Stage. The MBC writes `<rom>.sav` next to the ROM, so never point the
    // emulator at the player's own copy.
    let scratch = PathBuf::from("target/warproom");
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let staged = scratch.join(rom_path.file_name().ok_or("rom filename")?);
    std::fs::copy(rom_path, &staged).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(staged.with_extension("sav"));
    let bytes = std::fs::read(&staged).map_err(|e| e.to_string())?;

    let mut gb = Gameboy::try_new_with_model(
        bytes,
        Some(staged.clone()),
        terminalgb::mode::Model::Auto,
    )
    .map_err(|e| e.to_string())?;
    // A machine with no APU is not a quiet Game Boy, it is one with the sound
    // chip pulled out, and a cartridge can tell — see `examples/blackout_probe.rs`.
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    println!(
        "cartridge: {} (checksum ${:04X})",
        gb.rom_title().trim(),
        gb.global_checksum()
    );

    new_game_to_bedroom(&mut gb)?;
    let (y0, x0) = (gb.peek(W_Y_COORD), gb.peek(W_X_COORD));
    println!(
        "in the bedroom: map ${:02X} at ({}, {}), {}x{} blocks",
        gb.peek(W_CUR_MAP),
        x0,
        y0,
        gb.peek(W_CUR_MAP_WIDTH),
        gb.peek(W_CUR_MAP_HEIGHT),
    );
    if gb.peek(W_CUR_MAP) != REDS_HOUSE_2F {
        return Err(format!("expected REDS_HOUSE_2F (${REDS_HOUSE_2F:02X})"));
    }
    if let Some(d) = shots {
        shot(d.join("01-bedroom.png"), &mut gb);
    }

    // ---------------------------------------------------------------------
    // 1. Learn the tileset we are going to build out of, from the cartridge.
    // ---------------------------------------------------------------------
    let palette = Blockset::read(&mut gb)?;
    println!(
        "\ntileset: bank ${:02X}, blocks @ ${:04X}, collision @ ${:04X}",
        palette.bank, palette.blocks_ptr, palette.coll_ptr
    );
    println!(
        "  {} passable tile ids: {}",
        palette.passable.len(),
        hexlist(&palette.passable)
    );
    println!(
        "  {} blocks; floor ${:02X} (all four walk tiles passable), \
         wall ${:02X} (none), back-wall ${:02X} (top solid, bottom walkable)",
        palette.count, palette.floor, palette.wall, palette.backwall
    );

    // ---------------------------------------------------------------------
    // 2. Build the room and install the header.
    // ---------------------------------------------------------------------
    // Lift the staircase block out of the departing room's live block map. A
    // warp needs BOTH an entry in `wWarpEntries` AND a tile the tileset itself
    // calls a warp tile: `IsPlayerStandingOnDoorTileOrWarpTile`
    // (`engine/overworld/player_state.asm`) reads `lda_coord 8, 9` and looks it
    // up in `WarpTileIDPointers`. Fail that lookup and the only remaining route
    // is `ExtraWarpCheck`, which needs a direction *held* at the instant of the
    // check. Measured: a warp entry on a plain floor block does not fire when
    // the player simply steps onto it.
    let door = door_block_of_the_stairs(&mut gb);
    println!(
        "\ndoor block: ${door:02X}, lifted from the bedroom's own block map at \
         block ({}, {}) — it carries the stair tile the tileset recognises",
        STAIRS_X / 2,
        STAIRS_Y / 2
    );
    let room = build_room(&palette, door);
    let need = (ROOM_W as u16 + 6) * (ROOM_H as u16 + 6);
    let have = W_OVERWORLD_MAP_END - W_OVERWORLD_MAP;
    println!(
        "\nroom: {}x{} blocks = {}x{} walk tiles; \
         wOverworldMap footprint (W+6)*(H+6) = {need} of {have} bytes",
        ROOM_W,
        ROOM_H,
        ROOM_W * 2,
        ROOM_H * 2
    );
    if need > have {
        return Err(format!(
            "{need} > {have}: a room bigger than the buffer is a crash"
        ));
    }
    if room.len() as u16 > 360 {
        return Err("block map does not fit in wTileMapBackup2's 360 bytes".into());
    }

    // Get into position FIRST. The header we are about to write describes our
    // room, not the bedroom, so the moment it lands the room the player is
    // standing in has the wrong width, the wrong view pointer and therefore the
    // wrong collision — they could not take another step. Walk up to the tile
    // beside the stairs, install, then take the single step that warps.
    println!("\nwalking to ({DOORSTEP_X}, {DOORSTEP_Y}), the tile beside the stairs...");
    // Coarse first, to a tile well clear of the staircase. The greedy walker
    // holds a direction for 24 frames and a Gen 1 step is 16, so it buys one
    // step *or* two — aim it at the stairs and it walks straight through them
    // before the room is installed. (Measured: it did, and landed on
    // REDS_HOUSE_1F.)
    walk_toward(&mut gb, DOORSTEP_Y, DOORSTEP_X - 3, 200);
    // Then one tile at a time, so the last tile before the door is exact.
    for _ in 0..6 {
        if (gb.peek(W_X_COORD), gb.peek(W_Y_COORD)) == (DOORSTEP_X, DOORSTEP_Y) {
            break;
        }
        step(&mut gb, KeypadKey::Right);
    }
    if (gb.peek(W_X_COORD), gb.peek(W_Y_COORD)) != (DOORSTEP_X, DOORSTEP_Y)
        || gb.peek(W_CUR_MAP) != REDS_HOUSE_2F
    {
        return Err(format!(
            "could not stop beside the stairs (map ${:02X} at ({}, {}))",
            gb.peek(W_CUR_MAP),
            gb.peek(W_X_COORD),
            gb.peek(W_Y_COORD),
        ));
    }

    // ---------------------------------------------------------------------
    // 3. Step into the stairs, and install inside the window the engine leaves.
    // ---------------------------------------------------------------------
    // The header CANNOT be installed before the step. Two of the bytes are read
    // by the departing room while the player is still in it:
    //
    //  * `wCurMapWidth` is the stride `RedrawRowOrColumn` uses to repaint the
    //    column that scrolls in, so a changed width paints the departing room
    //    out of the wrong bytes of `wOverworldMap`; and
    //  * `wCurMapTileset` — the byte the latch lives in — indexes
    //    `WarpTileIDPointers` and `DoorTileIDPointers` in
    //    `IsPlayerStandingOnDoorTileOrWarpTile`. With bit 7 set that index is
    //    off the end of both tables.
    //
    // And a Gen 1 warp is decided from the SCREEN: `CheckWarpsNoCollision`
    // matches the coordinates against `wWarpEntries` and then asks
    // `IsPlayerStandingOnDoorTileOrWarpTile`, which reads `lda_coord 8, 9` —
    // the player's own tile in `wTileMap`. Break either input and the staircase
    // stops being a staircase. Measured: installing first left the player
    // standing on the stairs in the bedroom, unable to leave.
    //
    // The window is `LoadMapData`'s own first instruction: `call DisableLCD`,
    // which spins until VBlank. So `wCurMap` has already changed one frame
    // boundary before `LoadMapHeader` runs, and everything we need to write
    // fits in that gap.
    println!("\nstepping into the stairs at ({STAIRS_X}, {STAIRS_Y})...");
    let mut installed = Vec::new();
    let mut window_frames = 0u32;
    gb.keydown(KeypadKey::Right);
    let mut committed = false;
    for f in 0..90u32 {
        gb.frame();
        if f == 12 {
            gb.keyup(KeypadKey::Right);
        }
        if !committed && gb.peek(W_CUR_MAP) != REDS_HOUSE_2F {
            committed = true;
            window_frames = f;
            installed = install(&mut gb, &room, latch);
            // Also drop a signature into the BG tileset, to find out whether
            // custom art written *before* the load survives it.
            for i in 0..16u16 {
                gb.debug_write(V_TILESET + SIGNATURE_TILE * 16 + i, 0xA5);
            }
            println!(
                "  warp committed on frame {f}: wCurMap ${:02X}. \
                 wOverworldMap[0] is still ${:02X} (the bedroom's border block), \
                 so LoadTileBlockMap has not run — installed {} block-map bytes \
                 and the header here, latch {}.",
                gb.peek(W_CUR_MAP),
                gb.peek(W_OVERWORLD_MAP),
                room.len(),
                if latch { "SET" } else { "clear" }
            );
        }
    }
    gb.keyup(KeypadKey::Right);
    for _ in 0..30 {
        gb.frame();
    }
    if !committed {
        return Err(format!(
            "the stairs did not warp (map ${:02X} at ({}, {}))",
            gb.peek(W_CUR_MAP),
            gb.peek(W_X_COORD),
            gb.peek(W_Y_COORD)
        ));
    }
    let _ = window_frames;
    // ---------------------------------------------------------------------
    // 4. THE FINDING: what did the engine keep, and what did it clobber?
    // ---------------------------------------------------------------------
    println!("\n-- what survived LoadMapData --");
    let mut clobbered = Vec::new();
    for (name, addr, wrote) in installed.iter().copied() {
        let now = gb.peek(addr);
        let mark = if now == wrote { "kept" } else { "CLOBBERED" };
        if now != wrote {
            clobbered.push((name, addr, wrote, now));
        }
        println!("  {name:<34} ${addr:04X}  wrote ${wrote:02X}  now ${now:02X}  {mark}");
    }

    // Did the block map the engine laid out actually come from our bytes?
    let stride = gb.peek(W_CUR_MAP_WIDTH) as u16 + 2 * MAP_BORDER;
    let row0 = W_OVERWORLD_MAP + MAP_BORDER * stride + MAP_BORDER;
    let live: Vec<u8> = gb.peek_range(row0, ROOM_W as usize);
    let ours = &room[..ROOM_W as usize];
    let block_map_is_ours = live == ours;
    println!(
        "\n  wOverworldMap row 0 (stride {stride}, ${row0:04X}): {}",
        hexlist(&live)
    );
    println!(
        "  our block-map row 0:                     {}",
        hexlist(ours)
    );
    println!(
        "  => the live block map {} the one we wrote",
        if block_map_is_ours { "IS" } else { "is NOT" }
    );

    if let Some(d) = shots {
        shot(
            d.join(if latch {
                "02-our-room.png"
            } else {
                "02-control.png"
            }),
            &mut gb,
        );
    }

    if !latch {
        println!(
            "\n== CONTROL RUN ==\n{} of the {} header bytes we wrote were overwritten by the \
             engine's own load of REDS_HOUSE_1F. This is why authoring a destination \
             needs the latch: `LoadMapHeader` re-reads every one of them from \
             `MapHeaderPointers` before `LoadTileBlockMap` ever looks at \
             `wCurMapDataPtr`.",
            clobbered.len(),
            installed.len()
        );
        for (name, addr, wrote, now) in &clobbered {
            println!("   {name:<34} ${addr:04X}  ${wrote:02X} -> ${now:02X}");
        }
        return Ok(());
    }

    if !clobbered.is_empty() {
        return Err(format!(
            "{} header bytes were clobbered with the latch set",
            clobbered.len()
        ));
    }
    if !block_map_is_ours {
        return Err("the engine did not build the room from our block map".into());
    }

    // ---------------------------------------------------------------------
    // 4b. Custom tile art: what the load does to VRAM.
    // ---------------------------------------------------------------------
    let sig: Vec<u8> = gb.peek_range(V_TILESET + SIGNATURE_TILE * 16, 16);
    let sig_kept = sig.iter().all(|b| *b == 0xA5);
    println!(
        "\n-- custom tile art --\n  signature written to vTileset tile ${SIGNATURE_TILE:02X} \
         before the load reads back {} => {}",
        hexlist(&sig[..4]),
        if sig_kept { "KEPT" } else { "CLOBBERED" }
    );
    println!(
        "  `LoadMapData` calls `LoadTilesetTilePatternData` AFTER `LoadMapHeader`, and it \
         copies MAP_TILESET_SIZE = 96 tiles (${:04X}-${:04X}) from `wTilesetGfxPtr` in bank \
         `wTilesetBank` unconditionally — the latch does not skip it.",
        V_TILESET,
        V_TILESET + 96 * 16 - 1
    );
    // Re-blit afterwards: this is the recommended route, and it costs no
    // emulated cycles at all.
    let mut weave = [0u8; 16];
    for r in 0..8 {
        // A diagonal weave that exists nowhere in Nintendo's cartridge.
        weave[r * 2] = 0b1000_1000u8.rotate_right(r as u32);
        weave[r * 2 + 1] = 0;
    }
    for (i, b) in weave.iter().enumerate() {
        gb.debug_write(V_TILESET + FLOOR_TILE * 16 + i as u16, *b);
    }
    let back: Vec<u8> = gb.peek_range(V_TILESET + FLOOR_TILE * 16, 16);
    println!(
        "  re-blitted our own floor pattern over tile ${FLOOR_TILE:02X} after arrival: \
         VRAM reads back {} => {}",
        if back == weave {
            "exactly what we wrote"
        } else {
            "something else"
        },
        if back == weave { "OURS" } else { "FAILED" }
    );
    if back != weave {
        return Err("VRAM did not take our tile".into());
    }
    if let Some(d) = shots {
        for _ in 0..4 {
            gb.frame();
        }
        shot(d.join("02b-our-art.png"), &mut gb);
    }

    // ---------------------------------------------------------------------
    // 5. Is it a real room? Walk it, and give it a working way out.
    // ---------------------------------------------------------------------
    // Author the exit before walking, so stepping back onto the doorway tile
    // leaves. `wWarpEntries` is the room's own warp table and it is WRAM.
    gb.debug_write(W_NUM_WARPS, 1);
    gb.debug_write(W_WARP_ENTRIES, STAIRS_Y);
    gb.debug_write(W_WARP_ENTRIES + 1, STAIRS_X);
    gb.debug_write(W_WARP_ENTRIES + 2, 0); // warp 0 of the bedroom = the stairs
    gb.debug_write(W_WARP_ENTRIES + 3, REDS_HOUSE_2F);
    println!(
        "\nauthored one warp back: ({STAIRS_X}, {STAIRS_Y}) -> REDS_HOUSE_2F warp 0"
    );

    println!("\n-- walking the room --");
    let mut visited = Vec::new();
    let mut worst = None;
    // Down off the doorway first, then to each far corner in turn.
    for (tx, ty) in [
        (STAIRS_X, 4u8),
        (2, 4),
        (2, ROOM_H * 2 - 3),
        (ROOM_W * 2 - 3, ROOM_H * 2 - 3),
        (ROOM_W * 2 - 3, 4),
    ] {
        walk_toward(&mut gb, ty, tx, 200);
        let here = (gb.peek(W_X_COORD), gb.peek(W_Y_COORD));
        visited.push(here);
        println!(
            "  aimed ({tx}, {ty}) -> resting at ({}, {})",
            here.0, here.1
        );
        if gb.peek(W_CUR_MAP) != REDS_HOUSE_1F {
            return Err(format!(
                "left the room unexpectedly (map ${:02X})",
                gb.peek(W_CUR_MAP)
            ));
        }
        if here.0 as u16 >= ROOM_W as u16 * 2 || here.1 as u16 >= ROOM_H as u16 * 2 {
            worst = Some(here);
        }
    }
    if let Some((x, y)) = worst {
        return Err(format!(
            "escaped the map extent at ({x}, {y}) — the wall ring leaks"
        ));
    }
    let reach_x = visited.iter().map(|v| v.0).max().unwrap_or(0);
    let reach_y = visited.iter().map(|v| v.1).max().unwrap_or(0);
    println!(
        "  reached ({reach_x}, {reach_y}) of a {}x{} walk-tile room; \
         every rest inside the extent, fault {:?}",
        ROOM_W * 2,
        ROOM_H * 2,
        gb.fault()
    );
    if reach_x < ROOM_W * 2 - 4 || reach_y < ROOM_H * 2 - 4 {
        return Err(
            "never reached the far side — the room is not walkable end to end".into(),
        );
    }
    if let Some(d) = shots {
        shot(d.join("03-far-corner.png"), &mut gb);
    }

    // The screen the *engine* painted, scrolling, out of our block map.
    let screen: Vec<u8> = gb.peek_range(W_TILE_MAP, 20 * 18);
    let known = palette.tiles_of(&[palette.floor, palette.wall, door]);
    let foreign = screen.iter().filter(|t| !known.contains(t)).count();
    println!(
        "\n  on-screen buffer after the walk, painted by the ENGINE from our block map:\n\
         \x20   {} of 360 tiles come from the three blocks we placed, {foreign} from anything else",
        360 - foreign
    );

    // ---------------------------------------------------------------------
    // 6. Back out through our own warp.
    // ---------------------------------------------------------------------
    println!("\n-- leaving through the warp we authored --");
    walk_toward(&mut gb, 5, STAIRS_X, 200);
    for _ in 0..8 {
        if gb.peek(W_CUR_MAP) == REDS_HOUSE_2F {
            break;
        }
        let here_x = gb.peek(W_X_COORD);
        if here_x != STAIRS_X {
            let dir = if here_x > STAIRS_X {
                KeypadKey::Left
            } else {
                KeypadKey::Right
            };
            step(&mut gb, dir);
            continue;
        }
        step(&mut gb, KeypadKey::Up);
    }
    for _ in 0..48 {
        gb.frame();
    }
    let back = gb.peek(W_CUR_MAP);
    println!(
        "  map ${back:02X} at ({}, {}), {}x{} blocks, tileset ${:02X}",
        gb.peek(W_X_COORD),
        gb.peek(W_Y_COORD),
        gb.peek(W_CUR_MAP_WIDTH),
        gb.peek(W_CUR_MAP_HEIGHT),
        gb.peek(W_CUR_MAP_TILESET),
    );
    if back != REDS_HOUSE_2F {
        return Err("the authored warp did not lead back to the bedroom".into());
    }
    if gb.peek(W_CUR_MAP_WIDTH) != 4 || gb.peek(W_CUR_MAP_HEIGHT) != 4 {
        return Err(
            "the bedroom's own header was not restored from ROM on the way back".into(),
        );
    }
    if let Some(d) = shots {
        shot(d.join("04-back-upstairs.png"), &mut gb);
    }
    println!(
        "  => the return warp ran a NORMAL LoadMapHeader (latch self-cleared), so the \
         cartridge's own 4x4 bedroom is back from ROM. Nothing we wrote outlives the visit."
    );

    // ---------------------------------------------------------------------
    // 7. Save safety, stated as a measurement rather than a promise.
    // ---------------------------------------------------------------------
    gb.flush_battery();
    let sav = staged.with_extension("sav");
    println!("\n-- save safety --");
    println!(
        "  header bytes we wrote live at or above wMainDataStart ${W_MAIN_DATA_START:04X}, \
         i.e. inside `sMainData`. They only reach the medium if `SaveMainData` runs."
    );
    if let Ok(sram) = std::fs::read(&sav) {
        // `.sav` is raw cartridge RAM, bank 0 first, no header, so an SRAM
        // address is a file offset: bank 1 starts at 0x2000 and `sMainData` is
        // at $A5A3 (`ram/sram.asm`, `pokeblue.sym`).
        let main = |wram: u16| {
            0x2000 + (S_MAIN_DATA - 0xA000) + (wram - W_MAIN_DATA_START) as usize
        };
        let get = |a: u16| sram.get(main(a)).copied().unwrap_or(0);
        println!(
            "  `{}` is {} bytes; its saved header reads map ${:02X}, {}x{} blocks, \
             wCurMapDataPtr ${:02X}{:02X}",
            sav.display(),
            sram.len(),
            get(W_CUR_MAP),
            get(W_CUR_MAP_WIDTH),
            get(W_CUR_MAP_HEIGHT),
            get(W_CUR_MAP_DATA_PTR + 1),
            get(W_CUR_MAP_DATA_PTR),
        );
        if get(W_CUR_MAP_WIDTH) == ROOM_W || get(W_CUR_MAP_DATA_PTR + 1) == 0xCD {
            return Err("our room reached the save file".into());
        }
        println!("  => nothing we authored is in it.");
    } else {
        println!("  `{}` was never created.", sav.display());
    }

    println!("\n== PASS ==");
    Ok(())
}

// ---------------------------------------------------------------------------
// The tileset, read out of the cartridge at run time.
// ---------------------------------------------------------------------------

/// What the currently-loaded tileset offers, derived rather than tabulated.
///
/// **The walkability rule.** `_GetTileAndCoordsInFrontOfPlayer`
/// (`engine/overworld/player_state.asm`) probes `wTileMap` at (8,11), (8,7),
/// (6,9) and (10,9) — always an even column and an odd/even row of the 20x18
/// screen buffer — and `CheckTilePassable` (`home/overworld.asm`) tests that
/// single byte against the `$FF`-terminated `wTilesetCollisionPtr` list. One
/// walk tile is 2x2 screen tiles, so **a walk tile is passable iff its
/// top-left screen tile is in the list**. A block is 4x4 screen tiles laid out
/// row-major (`DrawTileBlock`), so its four walk tiles have their top-left
/// tiles at block offsets 0, 2, 8 and 10.
struct Blockset {
    bank: usize,
    blocks_ptr: u16,
    coll_ptr: u16,
    passable: Vec<u8>,
    defs: Vec<[u8; 16]>,
    count: usize,
    floor: u8,
    wall: u8,
    backwall: u8,
}

impl Blockset {
    fn read(gb: &mut Gameboy) -> Result<Blockset, String> {
        let bank = gb.peek(W_TILESET_BANK) as usize;
        let blocks_ptr = le16(gb, W_TILESET_BLOCKS_PTR);
        let coll_ptr = le16(gb, W_TILESET_COLLISION_PTR);

        // Both pointers are dereferenced bank-agnostically by the engine, but
        // for a *cartridge* tileset they point into `wTilesetBank`, so read
        // them through the numbered bank rather than through whatever the game
        // happens to have mapped.
        let rd = |gb: &mut Gameboy, a: u16| -> u8 {
            if (0x4000..0x8000).contains(&a) {
                gb.with_machine(Capability::READ_MEMORY, |m| m.peek_bank(bank, a))
                    .unwrap_or(0xFF)
            } else {
                gb.peek(a)
            }
        };

        let mut passable = Vec::new();
        for i in 0..64u16 {
            let t = rd(gb, coll_ptr + i);
            if t == 0xFF {
                break;
            }
            passable.push(t);
        }
        if passable.is_empty() {
            return Err("empty collision list — wTilesetCollisionPtr looks wrong".into());
        }

        // The blockset's length is not in RAM; read a generous window and stop
        // classifying once the ids stop being plausible. 64 blocks covers every
        // Gen 1 tileset (the biggest is 128 blocks; 64 is plenty for a room).
        let mut defs = Vec::new();
        for b in 0..64usize {
            let base = blocks_ptr + (b as u16) * 16;
            let mut d = [0u8; 16];
            for (i, slot) in d.iter_mut().enumerate() {
                *slot = rd(gb, base + i as u16);
            }
            defs.push(d);
        }

        let pass = |d: &[u8; 16]| [0usize, 2, 8, 10].map(|i| passable.contains(&d[i]));
        let find =
            |want: [bool; 4]| defs.iter().position(|d| pass(d) == want).map(|i| i as u8);
        // Prefer a block made of one repeated tile. Every Gen 1 interior has a
        // plain floor and a plain wall, and picking those rather than the first
        // match keeps a screenshot readable — the first fully-walkable block in
        // this tileset is a doorway, which tiles into something that looks like
        // a bug rather than a room.
        let uniform = |want: [bool; 4]| {
            defs.iter()
                .position(|d| pass(d) == want && d.iter().all(|t| *t == d[0]))
                .map(|i| i as u8)
        };

        let floor = uniform([true; 4])
            .or_else(|| find([true; 4]))
            .ok_or("no fully walkable block in this tileset")?;
        let wall = uniform([false; 4])
            .or_else(|| find([false; 4]))
            .ok_or("no fully solid block in this tileset")?;
        // Top row solid, bottom row walkable: the standard interior back wall,
        // and exactly what an arrival tile on the room's top edge needs.
        let backwall = find([false, false, true, true]).unwrap_or(floor);

        Ok(Blockset {
            bank,
            blocks_ptr,
            coll_ptr,
            passable,
            count: defs.len(),
            defs,
            floor,
            wall,
            backwall,
        })
    }

    /// Every screen tile id used by the given blocks.
    fn tiles_of(&self, blocks: &[u8]) -> Vec<u8> {
        let mut v: Vec<u8> = blocks
            .iter()
            .flat_map(|b| self.defs[*b as usize].iter().copied())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// The block id under the departing room's staircase, read out of the live
/// block map. `LoadTileBlockMap` lays a map out with a 3-block border on every
/// side and a stride of `width + 6`, so map block `(bx, by)` is at
/// `wOverworldMap + (3 + by) * (W + 6) + 3 + bx`.
fn door_block_of_the_stairs(gb: &mut Gameboy) -> u8 {
    let w = gb.peek(W_CUR_MAP_WIDTH) as u16;
    let stride = w + 2 * MAP_BORDER;
    let (bx, by) = (STAIRS_X as u16 / 2, STAIRS_Y as u16 / 2);
    gb.peek(W_OVERWORLD_MAP + (MAP_BORDER + by) * stride + MAP_BORDER + bx)
}

/// Our room, as `wCurMapDataPtr` will see it: `ROOM_H` rows of `ROOM_W` block
/// ids, no padding — the exact format of a `maps/*.blk` file, because that is
/// what `LoadTileBlockMap` copies.
fn build_room(p: &Blockset, door: u8) -> Vec<u8> {
    let (w, h) = (ROOM_W as usize, ROOM_H as usize);
    let mut m = vec![p.floor; w * h];
    // Ring of wall. There is NO map-boundary test in the collision check, so
    // this ring is the only thing between the player and a coordinate
    // underflow.
    for x in 0..w {
        m[x] = p.wall;
        m[(h - 1) * w + x] = p.wall;
    }
    for y in 0..h {
        m[y * w] = p.wall;
        m[y * w + w - 1] = p.wall;
    }
    // The doorway: the block the player lands on. It is the *departing room's
    // own staircase block*, lifted out of its live block map — see
    // `door_block_of_the_stairs`. Top half solid so walking back up out of the
    // world is impossible; bottom-right is the stair tile itself.
    m[(STAIRS_X / 2) as usize] = door;
    // A colonnade, so a screenshot cannot be mistaken for one of Nintendo's
    // rooms: a pillar on every other block of every other row.
    for y in (2..h - 2).step_by(2) {
        for x in (2..w - 2).step_by(2) {
            m[y * w + x] = p.wall;
        }
    }
    m
}

/// Write the block map and the header. Returns the (name, address, value)
/// triples so the caller can check every one of them afterwards.
fn install(gb: &mut Gameboy, room: &[u8], latch: bool) -> Vec<(&'static str, u16, u8)> {
    for (i, b) in room.iter().enumerate() {
        gb.debug_write(W_TILE_MAP_BACKUP2 + i as u16, *b);
    }

    // `wCurrentTileBlockMapViewPointer`, from the cartridge's own formula —
    // `event_displacement` (`macros/coords.asm`):
    //   wOverworldMap + 7 + W + (W+6)*(y>>1) + (x>>1)
    // which places the view's upper-left corner two blocks above and two
    // blocks left of the player's block. Get it wrong and the first frame
    // shows the wrong part of the room.
    let w = ROOM_W as u16;
    let view = W_OVERWORLD_MAP
        + 7
        + w
        + (w + 6) * (STAIRS_Y as u16 >> 1)
        + (STAIRS_X as u16 >> 1);

    let tileset = gb.peek(W_CUR_MAP_TILESET) & !BIT_NO_PREVIOUS_MAP;
    let mut writes: Vec<(&'static str, u16, u8)> = vec![
        ("wCurMapHeight (blocks)", W_CUR_MAP_HEIGHT, ROOM_H),
        ("wCurMapWidth (blocks)", W_CUR_MAP_WIDTH, ROOM_W),
        (
            "wCurMapDataPtr lo",
            W_CUR_MAP_DATA_PTR,
            (W_TILE_MAP_BACKUP2 & 0xFF) as u8,
        ),
        (
            "wCurMapDataPtr hi",
            W_CUR_MAP_DATA_PTR + 1,
            (W_TILE_MAP_BACKUP2 >> 8) as u8,
        ),
        ("wCurMapConnections", W_CUR_MAP_CONNECTIONS, 0),
        ("wMapBackgroundTile", W_MAP_BACKGROUND_TILE, 0),
        ("wNumSigns", W_NUM_SIGNS, 0),
        ("wNumSprites", W_NUM_SPRITES, 0),
        (
            "wCurrentMapHeight2 (walk tiles)",
            W_CUR_MAP_HEIGHT2,
            ROOM_H * 2,
        ),
        (
            "wCurrentMapWidth2 (walk tiles)",
            W_CUR_MAP_WIDTH2,
            ROOM_W * 2,
        ),
        (
            "wCurrentTileBlockMapViewPointer lo",
            W_VIEW_PTR,
            (view & 0xFF) as u8,
        ),
        (
            "wCurrentTileBlockMapViewPointer hi",
            W_VIEW_PTR + 1,
            (view >> 8) as u8,
        ),
        ("wYBlockCoord", W_Y_BLOCK_COORD, STAIRS_Y & 1),
        ("wXBlockCoord", W_X_BLOCK_COORD, STAIRS_X & 1),
    ];
    // `LoadTileBlockMap` fills the whole 1300-byte buffer with
    // `wMapBackgroundTile` before copying the room in, and the 3-block border
    // it leaves round the edge is what a player sees past the map. It must be
    // our wall block — `room[0]` is the ring's corner, so it is that by
    // construction.
    if let Some(e) = writes
        .iter_mut()
        .find(|(n, _, _)| *n == "wMapBackgroundTile")
    {
        e.2 = room[0];
    }

    for (_, addr, v) in &writes {
        gb.debug_write(*addr, *v);
    }
    // The latch last, so nothing between here and the warp can act on a
    // tileset id with bit 7 set.
    gb.debug_write(
        W_CUR_MAP_TILESET,
        if latch {
            tileset | BIT_NO_PREVIOUS_MAP
        } else {
            tileset
        },
    );
    writes.push((
        "wCurMapTileset (+ latch)",
        W_CUR_MAP_TILESET,
        // After the load the engine will have cleared the latch, which is
        // correct behaviour, not a clobber — so expect the bare tileset.
        tileset,
    ));
    writes
}

// ---------------------------------------------------------------------------
// Driving the game.
// ---------------------------------------------------------------------------

/// Title screen -> Oak's speech -> naming -> the bedroom. Alternating START and
/// A is what gets past both: the naming keyboard needs **START** (mashing A
/// types `AAAAA` for ever), everything else wants A.
fn new_game_to_bedroom(gb: &mut Gameboy) -> Result<(), String> {
    for step in 0..600u32 {
        let key = if step % 2 == 0 {
            KeypadKey::Start
        } else {
            KeypadKey::A
        };
        tap(gb, key, 20);
        if gb.peek(W_CUR_MAP_HEIGHT) > 0 && gb.peek(W_CUR_MAP_WIDTH) > 0 {
            // Skip Oak's "wait, don't go out" script — not needed here, but it
            // keeps the machine in the same state as the other Gen 1 probes.
            let f = gb.peek(W_EVENT_FLAGS);
            gb.debug_write(W_EVENT_FLAGS, f | 1);
            for _ in 0..30 {
                gb.frame();
            }
            return Ok(());
        }
    }
    Err("never reached the overworld".into())
}

/// Greedily walk toward walk tile `(y, x)`. Returns when it arrives, the map
/// changes, or the budget runs out.
///
/// Lifted from `examples/shiny_hunt.rs`, whose version is already proven to
/// reach the bedroom stairs on a brand-new game: try the axis with the larger
/// error, then the other, then probe sideways, and every few failures assume a
/// **text box** rather than a wall — a box swallows every direction key and
/// looks exactly like walking into furniture. B, not A: both advance a box, but
/// an A one frame after it closes opens the START menu, which then eats every
/// direction key for the rest of the run.
fn walk_toward(gb: &mut Gameboy, ty: u8, tx: u8, budget: u32) {
    let map0 = gb.peek(W_CUR_MAP);
    let mut stuck = 0u32;
    for _ in 0..budget {
        let (y, x) = (gb.peek(W_Y_COORD), gb.peek(W_X_COORD));
        if gb.peek(W_CUR_MAP) != map0
            || gb.peek(W_IS_IN_BATTLE) != 0
            || (y == ty && x == tx)
        {
            // Let a warp finish: stepping onto a door tile starts a map change
            // that takes a fade and several frames.
            for _ in 0..48 {
                gb.frame();
            }
            return;
        }
        let dy = ty as i16 - y as i16;
        let dx = tx as i16 - x as i16;
        let vert = if dy > 0 {
            KeypadKey::Down
        } else {
            KeypadKey::Up
        };
        let horiz = if dx > 0 {
            KeypadKey::Right
        } else {
            KeypadKey::Left
        };
        let order = if dy.abs() >= dx.abs() {
            [vert, horiz, KeypadKey::Left, KeypadKey::Right, vert]
        } else {
            [horiz, vert, KeypadKey::Up, KeypadKey::Down, horiz]
        };
        if stuck > 0 && stuck % order.len() as u32 == 0 {
            for _ in 0..3 {
                tap(gb, KeypadKey::B, 30);
            }
        }
        hold(gb, order[stuck as usize % order.len()], 24);
        stuck = if (gb.peek(W_Y_COORD), gb.peek(W_X_COORD)) == (y, x) {
            stuck + 1
        } else {
            0
        };
    }
}

/// Exactly one tile. A Gen 1 step takes 16 frames and the game commits to it
/// once begun, so a 24-frame hold buys one step *or* two depending on where the
/// walk began; 10 frames on and 12 off buys one and only one.
fn step(gb: &mut Gameboy, key: KeypadKey) {
    gb.keydown(key);
    for _ in 0..10 {
        gb.frame();
    }
    gb.keyup(key);
    for _ in 0..12 {
        gb.frame();
    }
}

fn hold(gb: &mut Gameboy, key: KeypadKey, frames: u32) {
    gb.keydown(key);
    for _ in 0..frames {
        gb.frame();
    }
    gb.keyup(key);
    for _ in 0..4 {
        gb.frame();
    }
}

fn tap(gb: &mut Gameboy, key: KeypadKey, gap: u32) {
    gb.keydown(key);
    for _ in 0..4 {
        gb.frame();
    }
    gb.keyup(key);
    for _ in 0..gap {
        gb.frame();
    }
}

fn le16(gb: &mut Gameboy, addr: u16) -> u16 {
    gb.peek(addr) as u16 | ((gb.peek(addr + 1) as u16) << 8)
}

fn hexlist(v: &[u8]) -> String {
    v.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shot(path: PathBuf, gb: &mut Gameboy) {
    if let Some(img) = image::RgbaImage::from_raw(160, 144, gb.image().to_vec()) {
        let _ = img.save(&path);
        println!("  [shot] {}", path.display());
    }
}
