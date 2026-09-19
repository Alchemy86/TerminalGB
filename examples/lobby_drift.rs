//! **Does a remote avatar stand still in the world while you walk?**
//!
//! The maintainer's report was that the other player "moves with you and then
//! auto-corrects". The mechanism is the one the cartridge already solves for
//! its own NPCs: `AdvancePlayerSprite`'s `.spriteShiftLoop`
//! (`pret/pokered`, `home/overworld.asm`) shifts every sprite the engine
//! maintains two pixels against the player's motion on each of the eight frames
//! of a walk, because `wYCoord`/`wXCoord` do not move until the step *finishes*.
//! Rewriting an avatar's screen pixels part-way through a step undoes that
//! shift, and the avatar rides the camera until the coordinates catch up.
//!
//! This probe measures it, with two real consoles on a real cable, in the real
//! Trade Center:
//!
//! ```text
//! cargo run --release --features image,plugin-gen1-link,plugin-trainer,plugin-gen1-lobby \
//!   --example lobby_drift -- --rom <blue.gb> [--save <blue.sav>]
//! ```
//!
//! It prints a PASS/FAIL and exits non-zero on failure, so it is a regression
//! check and not only a demonstration. Give it a cartridge whose save has been
//! to a Cable Club; it stages copies under `target/lobby-drift/` and never
//! opens anything beside the original.

use std::path::PathBuf;

use terminalgb::gameboy::Gameboy;
use terminalgb::link::LinkCable;
use terminalgb::KeypadKey;

use terminalgb_plugin_gen1_lobby::avatar::Peer;
use terminalgb_plugin_gen1_lobby::gen1::{
    MAP_TRADE_CENTER, S1_X_PIXELS, S1_Y_PIXELS, SPRITE_STRUCT_LEN, W_CUR_MAP,
    W_LINK_STATE, W_SPRITE_STATE_DATA1, W_WALK_COUNTER, W_X_COORD, W_Y_COORD,
};
use terminalgb_plugin_gen1_lobby::LobbyPlugin;

#[path = "support/gen1_cable_club.rs"]
mod cable_club;
use cable_club::*;

/// Slot 1 is where `avatar::stream` puts the nearest person in the room.
const SLOT1: u16 = W_SPRITE_STATE_DATA1 + SPRITE_STRUCT_LEN;

fn main() {
    let mut rom: Option<PathBuf> = None;
    let mut save: Option<PathBuf> = None;
    let mut verbose = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--rom" => rom = it.next().map(PathBuf::from),
            "--save" => save = it.next().map(PathBuf::from),
            "--verbose" => verbose = true,
            _ => {}
        }
    }
    let Some(rom) = rom else {
        eprintln!(
            "lobby_drift: --rom <blue.gb> is required (and a save that has been \
             to a Cable Club, else --save)"
        );
        std::process::exit(2);
    };

    let scratch = PathBuf::from("target/lobby-drift");
    let a = stage("a", &rom, save.as_deref(), &scratch);
    let b = stage("b", &rom, save.as_deref(), &scratch);
    let mut gb_a =
        Gameboy::new(std::fs::read(&a.rom).expect("rom a"), Some(a.rom.clone()));
    // A machine with no APU is not a quiet Game Boy, it is one with the sound
    // chip pulled out, and a cartridge can tell — see `examples/blackout_probe.rs`.
    gb_a.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    let mut gb_b =
        Gameboy::new(std::fs::read(&b.rom).expect("rom b"), Some(b.rom.clone()));
    // Same rule as above: a machine with no APU is not a quiet Game Boy.
    gb_b.enable_audio(Box::new(terminalgb::sound::SilentPlayer));

    // Two cores stepped by one loop never drift, so their link handshakes never
    // interleave — `docs/link-cable.md` §"Phase".
    for _ in 0..3 {
        gb_b.frame();
    }

    let mut cable = LinkCable::new();
    let (mut bot_a, mut bot_b) = (TradeBot::default(), TradeBot::default());
    let mut arrived = None;
    for f in 0..60 * 60 * 5 {
        bot_a.act(&mut gb_a, verbose, "A");
        bot_b.act(&mut gb_b, verbose, "B");
        cable.run_frame(&mut gb_a, &mut gb_b);
        if verbose && f % 600 == 0 {
            println!(
                "[{f:6}] A:{:<14} map {:#04x}   B:{:<14} map {:#04x}",
                bot_a.stage_label(),
                gb_a.peek(W_CUR_MAP),
                bot_b.stage_label(),
                gb_b.peek(W_CUR_MAP),
            );
        }
        if in_the_room(&mut gb_a) && in_the_room(&mut gb_b) {
            arrived = Some(f);
            break;
        }
    }
    let Some(f) = arrived else {
        // **Say where it stopped.** A bare "never reached the Trade Center"
        // says nothing about whether the bot could not navigate or the save
        // never loaded — and those look identical from outside, because a bot
        // mashing A through a NEW GAME's opening is a bot pressing A. `map
        // $26` with an empty party is Red's bedroom, i.e. a new game; a real
        // Cable Club save lands in a Pokémon Center with a party.
        eprintln!(
            "lobby_drift: never reached the Trade Center — A stopped at {} on \
             map {:#04x} with {} in the party, B at {} on map {:#04x}. Give it \
             --save pointing at a save that has been to a Cable Club, and \
             --verbose to watch the stages.",
            bot_a.stage_label(),
            gb_a.peek(W_CUR_MAP),
            gb_a.peek(W_PARTY_COUNT),
            bot_b.stage_label(),
            gb_b.peek(W_CUR_MAP),
        );
        std::process::exit(1);
    };
    println!("both consoles in the Trade Center at frame {f}");

    // The room, and one synthetic person standing in it. No relay: what is
    // being measured is what the plugin writes, not how the crowd arrives.
    gb_a.add_plugin(Box::new(LobbyPlugin::new("RED")))
        .expect("attach lobby");
    for _ in 0..30 {
        cable.run_frame(&mut gb_a, &mut gb_b);
    }
    let (mx, my) = (gb_a.peek(W_X_COORD), gb_a.peek(W_Y_COORD));
    let peer = Peer::new(2, "BLUE", mx.wrapping_sub(2), my);
    if let Some(p) = gb_a.plugin_mut::<LobbyPlugin>() {
        p.set_peers(vec![peer.clone()]);
    }
    for _ in 0..8 {
        cable.run_frame(&mut gb_a, &mut gb_b);
    }
    println!(
        "player at ({mx},{my}); the other person at ({},{}) in slot 1 at \
         ypx={:#04x} xpx={:#04x}",
        peer.x,
        peer.y,
        gb_a.peek(SLOT1 + S1_Y_PIXELS),
        gb_a.peek(SLOT1 + S1_X_PIXELS),
    );

    // --- walk, and watch what happens to somebody standing still -----------
    //
    // The avatar's WORLD position is its screen pixels plus however far the
    // camera has scrolled since the step began. The camera has moved
    // `2 * (WALK_FRAMES - wWalkCounter)` pixels, so a peer who is standing
    // still must satisfy
    //
    //   ypx(frame) + 2 * (8 - walkcounter) == ypx(before the step)
    //
    // on **every** frame of the step. Under the defect `ypx` never moves, so
    // the left-hand side climbs by two a frame — which is the avatar sliding
    // across the floor.
    let mut worst = 0i32;
    let mut samples = 0usize;
    let mut anchor: Option<i32> = None;
    gb_a.keydown(KeypadKey::Down);
    // Three steps. The peer must stay inside `avatar::visible`'s window for the
    // whole walk, or the slot is legitimately cleared and there is nothing left
    // to measure.
    for _ in 0..(20 * 3) {
        cable.run_frame(&mut gb_a, &mut gb_b);
        let walk = gb_a.peek(W_WALK_COUNTER) as i32;
        let ypx = gb_a.peek(SLOT1 + S1_Y_PIXELS) as i32;
        if walk == 0 {
            anchor = Some(ypx);
            continue;
        }
        let Some(base) = anchor else { continue };
        // Wrapping is not a concern: one step is sixteen pixels and the peer
        // stays two tiles from the player throughout.
        // Screen pixels are a `u8` and wrap; the honest comparison is the
        // signed 8-bit distance.
        let world = ypx + 2 * (8 - walk);
        let err = (((world - base) as i8) as i32).abs();
        if verbose {
            println!("  walk={walk} ypx={ypx:#04x} base={base:#04x} err={err}");
        }
        worst = worst.max(err);
        samples += 1;
    }
    gb_a.keyup(KeypadKey::Down);
    println!(
        "{samples} mid-step samples; worst world-position error {worst} px \
         (0 = the avatar never moved in the world)"
    );

    // A step is sixteen pixels, so anything at or above half a tile is the
    // avatar visibly riding the camera. Two pixels of slack absorbs the one
    // frame in which the engine's shift and our own bookkeeping can disagree.
    if worst > 2 {
        println!("FAIL: the other player drags with the camera by up to {worst} px");
        std::process::exit(1);
    }
    println!("PASS: the other player stands still in the world while you walk");
}

fn in_the_room(gb: &mut Gameboy) -> bool {
    gb.peek(W_CUR_MAP) == MAP_TRADE_CENTER && gb.peek(W_LINK_STATE) == 0x01
}
