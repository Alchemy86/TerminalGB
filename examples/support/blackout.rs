//! Getting a real cartridge to the instant its last Pokémon faints, and
//! watching what happens next.
//!
//! Shared by [`examples/blackout_probe.rs`](../blackout_probe.rs), which
//! reproduces the failure and reports it, and `testharness/blackout.rs`, which
//! gates it. `#[path]`-included by both, the same way
//! `examples/support/paperclaims.rs` is — so the thing that is measured and the
//! thing that is gated cannot drift apart.
//!
//! # What is being watched
//!
//! Gen 1 does not end a run when your last Pokémon faints. `HandleBlackOut`
//! (`home/overworld.asm`) fades the screen out, stops the music, halves your
//! money and warps you to `wLastBlackoutMap` — the last Pokémon Centre you
//! healed at, which before Viridian is Pallet Town, so you wake up outside your
//! own front door.
//!
//! `HandleBlackOut`'s second instruction is `call StopMusic`, and `StopMusic`
//! ends in a spin on `wAudioFadeOutControl` that only the audio engine can
//! release. `FadeOutAudio` (`home/fade_audio.asm`) releases it by walking the
//! master volume down to zero — **reading `rAUDVOL` ($FF24) back**, decrementing
//! both nibbles, writing it again, and stopping only when the read-back is
//! exactly zero. A machine with no APU answers `$FF` to every read in
//! `$FF10-$FF3F` and drops every write, so the volume never falls and the
//! screen stays black for ever.

#![allow(dead_code)]

use terminalgb::gameboy::Gameboy;
use terminalgb::KeypadKey;
use terminalgb_plugin_battle_control::preset::PresetFile;
use terminalgb_plugin_battle_control::{wram, BattlePlugin, Phase};

use std::path::{Path, PathBuf};

/// `wAudioFadeOutControl` — non-zero while a fade is in flight; `StopMusic`
/// spins until the audio engine zeroes it. AtlasGB `pokemon-rb`, `$CFC7`.
pub const AUDIO_FADE_OUT_CONTROL: u16 = 0xCFC7;
/// `wAudioFadeOutCounter`, the per-step countdown. AtlasGB, `$CFC9`.
pub const AUDIO_FADE_OUT_COUNTER: u16 = 0xCFC9;
/// `wCurMap`. AtlasGB, `$D35E`.
pub const CUR_MAP: u16 = 0xD35E;
/// `wLastBlackoutMap` — where blacking out returns you. AtlasGB, `$D719`.
pub const LAST_BLACKOUT_MAP: u16 = 0xD719;
/// `rAUDVOL` (NR50), the master volume `FadeOutAudio` walks down to zero.
pub const R_AUDVOL: u16 = 0xFF24;
/// Pallet Town — map 0, and the value a new game starts `wLastBlackoutMap` at.
pub const PALLET_TOWN: u8 = 0;
/// `wPartyCount` ($D163) through the end of `wPartyMonNicks`: the whole party
/// block, 404 bytes. AtlasGB.
pub const PARTY_BLOCK_LEN: usize = 404;

/// The preset both callers arm: one starter at level 5 against a wild Pokémon
/// far above it, so the loss does not depend on a roll.
pub const DOOMED: &str = "\
[doomed]
description = A party of one against a wild Pokemon it cannot beat

player   = Charmander
level    = 5
moves    = Scratch
nickname = DOOMED

wild     = Rattata
level    = 60
";

/// What the machine did in the observation window.
pub struct Outcome {
    /// The game left the blackout: the fade cleared, the map changed and there
    /// is a picture again.
    pub recovered: bool,
    pub observed: u32,
    pub map_at_faint: u8,
    pub map_at_end: u8,
    pub last_blackout_map: u8,
    pub is_in_battle_at_end: u8,
    pub shades_at_end: usize,
    pub fade_control_at_end: u8,
    pub fade_counter_at_end: u8,
    pub audvol_at_end: u8,
    pub lead_hp_at_faint: u16,
    pub lead_max_at_faint: u16,
    pub lead_hp_at_end: u16,
    pub lead_max_hp_at_end: u16,
    /// The first frame of the window at which the screen showed more than one
    /// shade again, if it ever did.
    pub screen_back_at: Option<u32>,
}

/// Get a machine to the instant the party faints, and hand back a save state.
///
/// The battle plugin is used only here, to build a legal party and arm the
/// fight through `wCurOpponent` — the cartridge's own mechanism, and what its
/// debug menu uses. It is left behind at the save state, because plugin state
/// is not machine state: the observation runs on a plain `Gameboy` with nothing
/// attached, so nothing but the cartridge can move a byte.
///
/// `setup_dir`, when given, also receives `party.bin` and `setup.txt` — exactly
/// what a foreign emulator needs to reproduce this with none of our tooling.
/// See `tools/blackout/mgba-blackout.c`.
pub fn prepare(
    rom: &[u8],
    staged: &Path,
    setup_dir: Option<&Path>,
) -> Result<Vec<u8>, String> {
    let mut gb = Gameboy::new(rom.to_vec(), Some(staged.to_path_buf()));
    // Prepare on a correct machine — a quiet Game Boy, not one with no sound
    // chip. The point of the A/B is to remove the chip from a machine that had
    // one, which is exactly what a headless host does.
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));

    new_game(&mut gb)?;

    let presets = PresetFile::parse(DOOMED, "blackout")
        .map_err(|e| format!("built-in preset: {e}"))?;
    let plugin = BattlePlugin::new(rom.to_vec(), presets)
        .map_err(|e| format!("cannot build the battle plugin: {e}"))?;
    gb.add_plugin(Box::new(plugin))
        .map_err(|e| format!("cannot attach the battle plugin: {e:?}"))?;
    gb.plugin_mut::<BattlePlugin>()
        .ok_or("the battle plugin vanished")?
        .arm("doomed")
        .map_err(|e| format!("cannot arm the preset: {e}"))?;

    // The plugin polls `wCurOpponent` from the overworld loop, so starting the
    // fight needs frames, not input.
    let mut started = false;
    for _ in 0..600 {
        gb.frame();
        match gb.plugin::<BattlePlugin>().map(|p| p.phase()) {
            Some(Phase::Fighting) => {
                started = true;
                break;
            }
            Some(Phase::Failed(m)) => {
                return Err(format!("the battle never started: {m}"))
            }
            _ => {}
        }
    }
    if !started {
        return Err("the battle never started".into());
    }

    if let Some(dir) = setup_dir {
        write_setup(&mut gb, dir)?;
    }

    // Play it out. A press every `MENU_GAP` frames: Gen 1's `HandleMenuInput`
    // delays several frames after accepting one and does not see a button that
    // arrives inside that delay.
    let mut fainted = false;
    for _ in 0..900u32 {
        if gb.peek(wram::addr::IS_IN_BATTLE) == 0xFF {
            fainted = true;
            break;
        }
        press(&mut gb, KeypadKey::A);
    }
    if !fainted {
        return Err("the party never fainted — the opposition was survivable, \
                    or the menus were not worked"
            .into());
    }
    gb.clear_input();
    Ok(gb.save_state())
}

/// Run the blackout out from a prepared state, with or without a sound chip,
/// and with no input at all.
pub fn observe(
    rom: &[u8],
    staged: &Path,
    state: &[u8],
    apu: bool,
    frames: u32,
) -> Result<(Outcome, Gameboy), String> {
    let mut gb = Gameboy::new(rom.to_vec(), Some(staged.to_path_buf()));
    if apu {
        gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    }
    gb.load_state(state)
        .map_err(|e| format!("cannot load the prepared state: {e:?}"))?;

    let map_at_faint = gb.peek(CUR_MAP);
    let last_blackout_map = gb.peek(LAST_BLACKOUT_MAP);
    let lead_hp_at_faint = be16(&mut gb, wram::addr::PARTY_MONS + 1);
    let lead_max_at_faint = be16(&mut gb, wram::addr::PARTY_MONS + 34);

    // Nothing is pressed from here. Whatever the game does next it does on its
    // own, which is what makes this a statement about the machine and not about
    // a policy.
    let mut screen_back_at = None;
    let mut recovered = false;
    let mut observed = 0;
    for frame in 0..frames {
        gb.frame();
        observed = frame + 1;
        let shades = distinct_shades(gb.image());
        if shades > 1 && screen_back_at.is_none() {
            screen_back_at = Some(frame);
        }
        if gb.peek(wram::addr::IS_IN_BATTLE) == 0 && shades > 1 {
            recovered = true;
            // Let the map settle so the reported position is the real one.
            for _ in 0..120 {
                gb.frame();
                observed += 1;
            }
            break;
        }
    }

    let outcome = Outcome {
        recovered,
        observed,
        map_at_faint,
        map_at_end: gb.peek(CUR_MAP),
        last_blackout_map,
        is_in_battle_at_end: gb.peek(wram::addr::IS_IN_BATTLE),
        shades_at_end: distinct_shades(gb.image()),
        fade_control_at_end: gb.peek(AUDIO_FADE_OUT_CONTROL),
        fade_counter_at_end: gb.peek(AUDIO_FADE_OUT_COUNTER),
        audvol_at_end: gb.peek(R_AUDVOL),
        lead_hp_at_faint,
        lead_max_at_faint,
        lead_hp_at_end: be16(&mut gb, wram::addr::PARTY_MONS + 1),
        lead_max_hp_at_end: be16(&mut gb, wram::addr::PARTY_MONS + 34),
        screen_back_at,
    };
    Ok((outcome, gb))
}

/// Write what a foreign emulator needs: the party block the plugin built, and
/// the wild opponent to poke into `wCurOpponent`.
fn write_setup(gb: &mut Gameboy, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {dir:?}: {e}"))?;
    let party: Vec<u8> = (0..PARTY_BLOCK_LEN)
        .map(|i| gb.peek(wram::addr::PARTY_COUNT + i as u16))
        .collect();
    let species = gb.peek(wram::addr::ENEMY_MON_SPECIES2);
    let level = gb.peek(wram::addr::CUR_ENEMY_LEVEL);
    std::fs::write(dir.join("party.bin"), &party)
        .map_err(|e| format!("cannot write party.bin: {e}"))?;
    std::fs::write(
        dir.join("setup.txt"),
        format!(
            "# what a foreign emulator needs to reproduce this — see \
             tools/blackout/mgba-blackout.c\n\
             party_addr=0x{:04X}\n\
             party_len={}\n\
             cur_opponent_addr=0x{:04X}\n\
             cur_opponent={}\n\
             cur_enemy_level_addr=0x{:04X}\n\
             cur_enemy_level={}\n",
            wram::addr::PARTY_COUNT,
            PARTY_BLOCK_LEN,
            wram::addr::CUR_OPPONENT,
            species,
            wram::addr::CUR_ENEMY_LEVEL,
            level
        ),
    )
    .map_err(|e| format!("cannot write setup.txt: {e}"))
}

/// Count the distinct greys on screen. A Game Boy frame is four shades, so
/// "one shade" is a categorically blank picture and needs no threshold — the
/// same rule `screen::halfblocks`' colour guard uses.
pub fn distinct_shades(rgba: &[u8]) -> usize {
    let mut seen = [false; 256];
    for px in rgba.chunks_exact(4) {
        seen[px[0] as usize] = true;
    }
    seen.iter().filter(|s| **s).count()
}

pub fn be16(gb: &mut Gameboy, at: u16) -> u16 {
    ((gb.peek(at) as u16) << 8) | gb.peek(at + 1) as u16
}

/// Boot the staged cartridge into a brand-new game and stop in the overworld.
///
/// Mashing START/A plays Gen 1's intro on its own; the play clock moving is
/// what says the game is running rather than sitting on the title screen.
pub fn new_game(gb: &mut Gameboy) -> Result<(), String> {
    let mut running = false;
    for step in 0..500u32 {
        let key = if step % 2 == 0 {
            KeypadKey::Start
        } else {
            KeypadKey::A
        };
        hold(gb, key, 4);
        run(gb, 20);
        if gb.peek(0xDA43) > 0 || gb.peek(0xDA44) > 4 {
            running = true;
            break;
        }
    }
    if !running {
        return Err("never reached the overworld from the title screen".into());
    }
    // B, never A: both dismiss a text box, but B also closes a menu and can
    // never open one.
    for _ in 0..8 {
        if !menu_is_open(gb) {
            break;
        }
        hold(gb, KeypadKey::B, 4);
        run(gb, 20);
    }
    run(gb, 60);
    if menu_is_open(gb) {
        return Err("could not close the START menu".into());
    }
    Ok(())
}

/// Read the game's own screen and look for the START menu. Gen 1 writes
/// character codes straight into `wTileMap` and loads its font so a tile id
/// *is* the code of the glyph it draws.
fn menu_is_open(gb: &mut Gameboy) -> bool {
    let tiles: Vec<u8> = (0..20 * 18)
        .map(|i| gb.peek(wram::addr::TILE_MAP + i as u16))
        .collect();
    terminalgb_plugin_trainer::text::decode_screen(&tiles).contains("OPTION")
}

/// One press, then the gap Gen 1's own menu handler needs before it will see
/// the next one.
pub fn press(gb: &mut Gameboy, key: KeypadKey) {
    hold(gb, key, 4);
    run(gb, 24);
}

pub fn hold(gb: &mut Gameboy, key: KeypadKey, frames: u32) {
    gb.keydown(key);
    run(gb, frames);
    gb.keyup(key);
}

pub fn run(gb: &mut Gameboy, frames: u32) {
    for _ in 0..frames {
        gb.frame();
    }
}

/// Copy the cartridge into `target/` before running it. The MBC writes its
/// battery save **next to the ROM path**, so running one in place drops `.sav`
/// files into whatever directory it came from.
pub fn stage(src: &Path, rom: &[u8], dir: &str) -> Result<PathBuf, String> {
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {dir:?}: {e}"))?;
    let name = src
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("rom.gb"));
    let dst = dir.join(name);
    std::fs::write(&dst, rom).map_err(|e| format!("cannot write {dst:?}: {e}"))?;
    // A stale save would put the game on the CONTINUE screen and change
    // everything downstream.
    for ext in ["sav", "srm", "gbsave"] {
        let _ = std::fs::remove_file(dst.with_extension(ext));
    }
    Ok(dst)
}
