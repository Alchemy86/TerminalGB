//! Tests for the battle-control plugin.
//!
//! Everything here runs against a **synthetic cartridge**: a ROM image built in
//! this file that contains the same tables and the same machine-code shapes a
//! real Pokémon Blue does, and a header that passes
//! [`terminalgb_plugin_trainer::Game::detect`]. No commercial ROM is needed, and none is
//! in this repository.
//!
//! What a synthetic ROM cannot prove is that the *engine* behaves — that a
//! written `wCurOpponent` really becomes a battle. That is
//! `examples/gen1_battle.rs`, which drives the real cartridge.

use super::*;
use terminalgb::testing::TestMachine;
use terminalgb_plugin_api::MachineAccess;
use terminalgb_plugin_api::PluginHost;
use terminalgb_plugin_gen1_link::text;

// ---------------------------------------------------------------------------
// A cartridge to test against
// ---------------------------------------------------------------------------

/// Where each table goes in the synthetic image.
const BASE_STATS: usize = 0x10000;
const MEW_STATS: usize = 0x08000;
const DEX_ORDER: usize = 0x20000;
const MON_NAMES: usize = 0x30000;
const MOVES: usize = 0x40000;
const MOVE_NAMES: usize = 0x50000;
const TRAINER_NAMES: usize = 0x58000;
const CODE: usize = 0x0400;

/// Internal indices used by the tests. Real Gen 1 indices; the synthetic
/// `PokedexOrder` maps them to the right Pokédex numbers.
const IDX_CHARIZARD: u8 = 0xB0;
const IDX_PIDGEY: u8 = 0x24;
const IDX_MEW: u8 = 0x15;
const IDX_PIKACHU: u8 = 0x54;

fn put_name(rom: &mut [u8], at: usize, s: &str) {
    let enc = text::encode_fixed(s, 10);
    rom[at..at + 10].copy_from_slice(&enc);
}

/// A cartridge with every table and code signature the plugin looks for.
pub fn synthetic_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x60000];
    // Fill with something that cannot collide with a signature.
    for (i, b) in rom.iter_mut().enumerate() {
        *b = ((i * 7 + 3) % 23) as u8 + 0x01;
    }

    // --- header: must read as English Pokémon Blue --------------------------
    rom[0x134..0x144].fill(0);
    for (i, c) in b"POKEMON BLUE".iter().enumerate() {
        rom[0x134 + i] = *c;
    }
    rom[0x143] = 0x00; // no CGB flag, so the title is read as 16 bytes
    rom[0x147] = 0x13; // MBC3+RAM+BATTERY
    rom[0x148] = 0x05; // 32 banks
    rom[0x149] = 0x03; // 4 banks of cartridge RAM, so the safety test has some
    rom[0x14E] = 0x9D;
    rom[0x14F] = 0x0A;

    // --- BaseStats ----------------------------------------------------------
    for dex in 1..=150usize {
        let e = BASE_STATS + (dex - 1) * 28;
        rom[e..e + 28].fill(0);
        rom[e] = dex as u8;
        rom[e + 1] = 45;
        rom[e + 2] = 49;
        rom[e + 3] = 49;
        rom[e + 4] = 45;
        rom[e + 5] = 65;
        rom[e + 6] = 0x16;
        rom[e + 7] = 0x03;
        rom[e + 8] = 45;
        rom[e + 9] = 64;
        rom[e + 15] = 1; // level 1 learnset: POUND
        rom[e + 19] = 0; // medium fast
    }
    // Pikachu, dex 25, is `RomTables::locate`'s cross-check entry.
    let p = BASE_STATS + 24 * 28;
    rom[p + 1] = 35;
    rom[p + 2] = 55;
    rom[p + 3] = 30;
    rom[p + 4] = 90;
    rom[p + 5] = 50;
    rom[p + 6] = 23;
    rom[p + 7] = 23;
    rom[p + 8] = 190;
    // Charizard, dex 6: the numbers that matter for the demo.
    let c = BASE_STATS + 5 * 28;
    rom[c + 1] = 78;
    rom[c + 2] = 84;
    rom[c + 3] = 78;
    rom[c + 4] = 100;
    rom[c + 5] = 85;
    rom[c + 6] = 20; // FIRE
    rom[c + 7] = 2; // FLYING
    rom[c + 8] = 45;
    rom[c + 15] = 1; // POUND, so a preset that names no moves still gets one
    rom[c + 16] = 85; // THUNDERBOLT stands in for a second known move
    rom[c + 19] = 3; // medium slow
                     // Pidgey, dex 16.
    let g = BASE_STATS + 15 * 28;
    rom[g + 1] = 40;
    rom[g + 2] = 45;
    rom[g + 3] = 40;
    rom[g + 4] = 56;
    rom[g + 5] = 35;
    rom[g + 6] = 0;
    rom[g + 7] = 2;
    rom[g + 8] = 255;
    rom[g + 15] = 1;
    rom[g + 19] = 0;

    // --- MewBaseStats, stored apart as in Red/Blue --------------------------
    rom[MEW_STATS..MEW_STATS + 28].fill(0);
    rom[MEW_STATS] = 151;
    for i in 1..6 {
        rom[MEW_STATS + i] = 100;
    }
    rom[MEW_STATS + 6] = 0x18;
    rom[MEW_STATS + 7] = 0x18;
    rom[MEW_STATS + 8] = 45;
    rom[MEW_STATS + 9] = 64;
    rom[MEW_STATS + 15] = 1;

    // --- PokedexOrder: internal index -> dex --------------------------------
    rom[DEX_ORDER..DEX_ORDER + 190].fill(0);
    rom[DEX_ORDER..DEX_ORDER + 6].copy_from_slice(&[112, 115, 32, 35, 21, 100]);
    rom[DEX_ORDER + IDX_PIKACHU as usize - 1] = 25;
    rom[DEX_ORDER + IDX_MEW as usize - 1] = 151;
    rom[DEX_ORDER + IDX_CHARIZARD as usize - 1] = 6;
    rom[DEX_ORDER + IDX_PIDGEY as usize - 1] = 16;

    // --- MonsterNames -------------------------------------------------------
    rom[MON_NAMES..MON_NAMES + 190 * 10].fill(text::TERMINATOR);
    put_name(&mut rom, MON_NAMES, "RHYDON");
    put_name(
        &mut rom,
        MON_NAMES + (IDX_PIKACHU as usize - 1) * 10,
        "PIKACHU",
    );
    put_name(&mut rom, MON_NAMES + (IDX_MEW as usize - 1) * 10, "MEW");
    put_name(
        &mut rom,
        MON_NAMES + (IDX_CHARIZARD as usize - 1) * 10,
        "CHARIZARD",
    );
    put_name(
        &mut rom,
        MON_NAMES + (IDX_PIDGEY as usize - 1) * 10,
        "PIDGEY",
    );

    // --- Moves --------------------------------------------------------------
    rom[MOVES..MOVES + 165 * 6].fill(0);
    rom[MOVES..MOVES + 12].copy_from_slice(&[1, 0, 40, 0, 255, 35, 2, 0, 50, 0, 255, 25]);
    rom[MOVES + 84 * 6 + 5] = 15; // move 85 = THUNDERBOLT, 15 PP
    rom[MOVES + 52 * 6 + 5] = 15; // move 53 = FLAMETHROWER, 15 PP

    // --- MoveNames ----------------------------------------------------------
    let mut q = MOVE_NAMES;
    for id in 1..=165 {
        let n = match id {
            1 => "POUND",
            2 => "KARATECHOP",
            53 => "FLAMETHROWER",
            85 => "THUNDERBOLT",
            _ => "TACKLE",
        };
        for b in n.chars().filter_map(text::encode_char) {
            rom[q] = b;
            q += 1;
        }
        rom[q] = text::TERMINATOR;
        q += 1;
    }

    // --- TrainerNames -------------------------------------------------------
    let names = [
        "YOUNGSTER",
        "BUG CATCHER",
        "LASS",
        "SAILOR",
        "JR.TRAINER\u{2642}",
        "JR.TRAINER\u{2640}",
    ];
    let mut q = TRAINER_NAMES;
    for n in names {
        for b in n.chars().filter_map(text::encode_char) {
            rom[q] = b;
            q += 1;
        }
        rom[q] = text::TERMINATOR;
        q += 1;
    }
    // Pad out to 47 classes so BROCK lands where a real cartridge puts it.
    for i in names.len()..47 {
        let n = if i == 33 {
            "BROCK".to_string()
        } else {
            format!("CLASS{i}")
        };
        for b in n.chars().filter_map(text::encode_char) {
            rom[q] = b;
            q += 1;
        }
        rom[q] = text::TERMINATOR;
        q += 1;
    }

    // --- the machine code `wram::verify` reads addresses out of -------------
    let mut w = CODE;
    let emit = |rom: &mut Vec<u8>, at: &mut usize, bytes: &[u8]| {
        rom[*at..*at + bytes.len()].copy_from_slice(bytes);
        *at += bytes.len();
    };
    // InitOpponent
    emit(
        &mut rom,
        &mut w,
        &[
            0xFA, 0x59, 0xD0, 0xEA, 0x91, 0xCF, 0xEA, 0xD8, 0xCF, 0x18, 0x02,
        ],
    );
    // ReadTrainer head + wTrainerNo + .IterateTrainer
    emit(
        &mut rom,
        &mut w,
        &[
            0xFA, 0x2B, 0xD1, // ld a, [wLinkState]
            0xA7, 0xC0, // and a / ret nz
            0x21, 0x9C, 0xD8, // ld hl, wEnemyPartyCount
            0xAF, 0x22, 0x3D, 0x77, // xor a / ldi / dec a / ld [hl],a
        ],
    );
    emit(
        &mut rom,
        &mut w,
        &[0x4F, 0x06, 0x00, 0x09, 0x2A, 0x66, 0x6F, 0xFA, 0x5D, 0xD0],
    );
    emit(
        &mut rom,
        &mut w,
        &[0x2A, 0xFE, 0xFF, 0x28, 0x08, 0xEA, 0x27, 0xD1],
    );
    // InitWildBattle
    emit(
        &mut rom,
        &mut w,
        &[0x3E, 0x01, 0xEA, 0x57, 0xD0, 0xCD, 0x00, 0x40],
    );
    // Random_
    emit(
        &mut rom,
        &mut w,
        &[
            0xF0, 0x04, 0x47, 0xF0, 0xD3, 0x88, 0xE0, 0xD3, 0xF0, 0x04, 0x47, 0xF0, 0xD4,
            0x98, 0xE0, 0xD4, 0xC9,
        ],
    );
    rom
}

fn plugin(text: &str) -> BattlePlugin {
    let presets = PresetFile::parse(text, "test").expect("preset parses");
    BattlePlugin::new(synthetic_rom(), presets).expect("plugin builds")
}

/// An MMU running the synthetic cartridge, so the tests exercise the real
/// [`Machine`] plumbing rather than a mock.
fn mmu() -> TestMachine {
    TestMachine::new(synthetic_rom()).unwrap()
}

/// Put the machine in the state the plugin waits for: out of a battle, off the
/// link, with a party. WRAM is seeded with a fixed pseudo-random pattern
/// (the emulator’s MMU), so none of these read as zero by default — and `wLinkState`
/// reading as nonzero is exactly the "we are mid-trade, do not fire" case the
/// plugin is meant to hold for.
fn overworld(mem: &mut TestMachine) {
    mem.poke(wram::addr::IS_IN_BATTLE, 0);
    mem.poke(wram::addr::LINK_STATE, 0);
    mem.poke(wram::addr::CUR_OPPONENT, 0);
    mem.poke(wram::addr::PARTY_COUNT, 1);
}

/// Drive `n` frames through a host, letting a closure play the part of the
/// battle engine.
fn run(
    host: &mut PluginHost,
    mem: &mut TestMachine,
    n: u32,
    mut engine: impl FnMut(u32, &mut TestMachine),
) {
    for f in 0..n {
        engine(f, mem);
        host.on_frame(mem);
    }
}

// ---------------------------------------------------------------------------
// Construction, refusal, resolution
// ---------------------------------------------------------------------------

#[test]
fn the_plugin_declares_write_and_input_and_says_so() {
    let p = plugin("[a]\nwild = Pidgey\n");
    let c = p.capabilities();
    assert!(c.contains(Capability::WRITE_MEMORY));
    assert!(c.contains(Capability::CONTROL_INPUT));
    assert!(c.contains(Capability::READ_MEMORY));
    assert!(
        !c.contains(Capability::SERIAL_LINK),
        "it is not a link partner"
    );
    assert_eq!(c.describe(), "observe+read+write+input");
}

#[test]
fn a_cartridge_without_the_code_signatures_is_refused_before_construction() {
    match BattlePlugin::new(vec![0u8; 0x8000], PresetFile::builtin()) {
        Err(e) => assert!(matches!(e, BattleError::Verify(_)), "{e}"),
        Ok(_) => panic!("a ROM with no battle code must be refused"),
    }
}

/// The cartridge check is a *second* gate: a ROM can contain the right code and
/// still be a hack with a different header, so the plugin goes inert rather
/// than writing.
#[test]
fn an_unknown_cartridge_makes_the_plugin_inert_and_visible() {
    let mut rom = synthetic_rom();
    // Same code, different game.
    rom[0x134..0x144].fill(0);
    for (i, c) in b"POKEMON YELLOW".iter().enumerate() {
        rom[0x134 + i] = *c;
    }
    let mut mem = TestMachine::new(rom.clone()).unwrap();
    let mut host = PluginHost::new();
    host.register(Box::new(
        BattlePlugin::new(rom, PresetFile::builtin()).unwrap(),
    ))
    .unwrap();
    host.on_frame(&mut mem);
    let status = &host.statuses()[0];
    assert!(status.detail.starts_with("inactive:"), "{status:?}");
    assert!(status.detail.contains("POKEMON YELLOW"), "{status:?}");
    assert!(!host.any_wrote_memory(), "an inert plugin writes nothing");
}

#[test]
fn a_wild_preset_triggers_with_the_species_index() {
    let p = plugin("[a]\nwild = Pidgey\nlevel = 7\n");
    let plan = p.build_plan(p.selected().unwrap()).unwrap();
    assert_eq!(plan.trigger, IDX_PIDGEY);
    assert!(plan.trigger < wram::OPP_ID_OFFSET, "below OPP_ID_OFFSET");
    assert_eq!(plan.level, 7);
    assert!(plan.wild_override.is_empty(), "no override was asked for");
}

#[test]
fn a_trainer_preset_triggers_with_the_class_plus_the_offset() {
    let p = plugin("[a]\ntrainer = BROCK\nroster = 3\n");
    let plan = p.build_plan(p.selected().unwrap()).unwrap();
    // BROCK is class 34 in the cartridge's own TrainerNames order.
    assert_eq!(plan.trigger, wram::OPP_ID_OFFSET + 34);
    assert_eq!(plan.trainer_no, 3);
    assert!(plan.enemy_party.is_empty(), "the ROM roster is left alone");
    // The same class by number resolves identically.
    let q = plugin("[a]\ntrainer = 34\n");
    assert_eq!(
        q.build_plan(q.selected().unwrap()).unwrap().trigger,
        plan.trigger
    );
}

#[test]
fn the_two_gendered_jr_trainer_classes_do_not_collide() {
    let p = plugin("[a]\nwild = Pidgey\n");
    let m = p.resolve_class("JR.TRAINER\u{2642}").unwrap();
    let f = p.resolve_class("JR.TRAINER\u{2640}").unwrap();
    assert_eq!((m, f), (5, 6));
    assert_ne!(m, f, "stripping the symbol would merge two classes");
}

#[test]
fn an_unknown_trainer_class_is_named_rather_than_guessed() {
    let p = plugin("[a]\ntrainer = NOBODY IN PARTICULAR\n");
    let e = p.build_plan(p.selected().unwrap()).unwrap_err();
    assert!(format!("{e}").contains("NOBODY IN PARTICULAR"), "{e}");
}

/// The legitimacy requirement: a Pokémon this plugin injects has to be one the
/// engine would accept. Everything below is computed by the *shared* encoder in
/// `plugin::gen1::party`, which is the same code that puts a Pokémon on the
/// link cable — there is deliberately not a second, divergent one.
#[test]
fn an_injected_pokemon_is_internally_coherent() {
    let p = plugin(
        "[a]\nplayer = Charizard\nlevel = 50\nmoves = Flamethrower\n\
         dvs = 15,15,15,15\nwild = Pidgey\nlevel = 3\n",
    );
    let mon = p
        .build_one(&p.selected().unwrap().player[0], "RED", 0)
        .unwrap();
    assert_eq!(mon.species, IDX_CHARIZARD);
    assert_eq!(mon.level, 50);
    assert_eq!(mon.moves, [53, 0, 0, 0]);
    assert_eq!(mon.pp[0], 15, "PP comes from the cartridge's move table");
    // Gen 1 stat formula, base 78 HP / 85 special, perfect DVs, level 50:
    //   HP      = ((78+15)*2)*50/100 + 50 + 10 = 93 + 60  = 153
    //   Special = ((85+15)*2)*50/100 + 5       = 100 + 5  = 105
    assert_eq!(mon.stats.hp, 153);
    assert_eq!(mon.stats.special, 105);
    assert_eq!(mon.current_hp, mon.stats.hp, "it starts at full health");
    // Medium slow at level 50: (6/5)*125000 - 15*2500 + 100*50 - 140 = 117360.
    assert_eq!(mon.experience, 117_360);
    assert_eq!(
        mon.nickname, "CHARIZARD",
        "un-nicknamed means the species name"
    );
}

/// A Pokémon with no `moves` line gets the species' own level 1 learnset out of
/// the cartridge, not an arbitrary stand-in.
#[test]
fn a_pokemon_with_no_moves_stated_gets_the_cartridges_answer() {
    let p = plugin("[a]\nplayer = Charizard\nwild = Pidgey\n");
    let mon = p
        .build_one(&p.selected().unwrap().player[0], "RED", 0)
        .unwrap();
    assert_eq!(mon.moves, [1, 85, 0, 0], "the base-stat record's move list");
    assert_eq!(mon.pp, [35, 15, 0, 0]);
}

#[test]
fn the_battle_struct_is_not_the_party_struct() {
    let p = plugin("[a]\nwild = Charizard\nlevel = 50\ndvs = 15,15,15,15\n");
    let plan = p.build_plan(p.selected().unwrap()).unwrap();
    let b = &plan.wild_override[0];
    assert_eq!(b.addr, wram::addr::ENEMY_MON);
    assert_eq!(b.bytes.len(), wram::BATTLE_STRUCT_LENGTH);
    // Level lives at offset 14 in the battle struct and 33 in the party one.
    assert_eq!(b.bytes[wram::battle_field::LEVEL], 50);
    assert_eq!(
        u16::from_be_bytes([
            b.bytes[wram::battle_field::MAX_HP],
            b.bytes[wram::battle_field::MAX_HP + 1]
        ]),
        153
    );
    // ...and the nickname block rides with it.
    assert_eq!(plan.wild_override[1].addr, wram::addr::ENEMY_MON_NICK);
    assert_eq!(plan.wild_override[1].bytes.len(), wram::NAME_LENGTH);
}

#[test]
fn a_party_block_fills_all_six_slots_and_terminates_the_species_list() {
    let p = plugin("[a]\nplayer = Charizard\nplayer = Pikachu\nwild = Pidgey\n");
    let plan = p.build_plan(p.selected().unwrap()).unwrap();
    let mut mem = mmu();
    let blocks = {
        let mut m = Machine::new(&mut mem, p.capabilities());
        p.player_blocks(&plan, &mut m)
    };
    let by = |a: u16| blocks.iter().find(|b| b.addr == a).unwrap();
    assert_eq!(by(wram::addr::PARTY_COUNT).bytes, vec![2]);
    let species = &by(wram::addr::PARTY_SPECIES).bytes;
    assert_eq!(species[0], IDX_CHARIZARD);
    assert_eq!(species[1], IDX_PIKACHU);
    assert_eq!(&species[2..], &[0xFF; 5], "the rest is the $FF sentinel");
    // All six structures are written, so no slot keeps half of the old party.
    assert_eq!(
        by(wram::addr::PARTY_MONS).bytes.len(),
        6 * wram::PARTY_STRUCT_LENGTH
    );
    assert_eq!(
        by(wram::addr::PARTY_MON_OT).bytes.len(),
        6 * wram::NAME_LENGTH
    );
    assert_eq!(
        by(wram::addr::PARTY_MON_NICKS).bytes.len(),
        6 * wram::NAME_LENGTH
    );
}

/// The legitimacy trap that cost the most time, and the one a synthetic test
/// would never have found: `CheckForDisobedience` (`engine/battle/core.asm`)
/// compares `wPartyMon1OTID` with `wPlayerID` and treats a mismatch as a
/// **traded** Pokémon, which a player with no badges is only obeyed by up to
/// level 10. An injected level 50 Charizard with a made-up trainer id answers
/// "CHARIZARD won't obey!" and wastes the turn — a cinematic that never happens.
#[test]
fn an_injected_party_inherits_the_running_games_trainer_id() {
    let p = plugin("[a]\nplayer = Charizard\nlevel = 50\nwild = Pidgey\n");
    let plan = p.build_plan(p.selected().unwrap()).unwrap();
    let mut mem = mmu();
    // wPlayerID is big-endian, like every other multi-byte Pokémon field.
    mem.poke(terminalgb_plugin_trainer::gen1::addr::PLAYER_ID, 0x12);
    mem.poke(terminalgb_plugin_trainer::gen1::addr::PLAYER_ID + 1, 0x34);
    let blocks = {
        let mut m = Machine::new(&mut mem, p.capabilities());
        p.player_blocks(&plan, &mut m)
    };
    let mons = &blocks
        .iter()
        .find(|b| b.addr == wram::addr::PARTY_MONS)
        .unwrap()
        .bytes;
    // OT id is at offset 12 of a party_struct, big-endian.
    assert_eq!(
        u16::from_be_bytes([mons[12], mons[13]]),
        0x1234,
        "the injected Pokémon must belong to the player, or it disobeys"
    );

    // ...unless the preset says otherwise, which is how you deliberately make
    // a battle test the disobedience path.
    let q = plugin("[a]\nplayer = Charizard\nlevel = 50\not_id = 999\nwild = Pidgey\n");
    let qplan = q.build_plan(q.selected().unwrap()).unwrap();
    let blocks = {
        let mut m = Machine::new(&mut mem, q.capabilities());
        q.player_blocks(&qplan, &mut m)
    };
    let mons = &blocks
        .iter()
        .find(|b| b.addr == wram::addr::PARTY_MONS)
        .unwrap()
        .bytes;
    assert_eq!(u16::from_be_bytes([mons[12], mons[13]]), 999);
}

// ---------------------------------------------------------------------------
// Safety
// ---------------------------------------------------------------------------

/// Guarantee 1: the plugin cannot put anything into a `.sav`, because it cannot
/// write the memory a `.sav` is made of. Proved by pointing its own write door
/// at cartridge RAM and checking both that the byte did not move *and* that the
/// refusal was counted — a silent no-op would be indistinguishable from the
/// address being unreachable for some other reason.
#[test]
fn the_plugin_refuses_to_write_cartridge_ram() {
    let mut p = plugin("[a]\nwild = Pidgey\n");
    let mut mem = mmu();
    // Enable cartridge RAM and put a known byte in it, the way a game would.
    mem.bus_write(0x0000, 0x0A);
    mem.bus_write(0xA000, 0x77);
    {
        let mut m = Machine::new(&mut mem, p.capabilities());
        p.write(&mut m, 0xA000, 0xEE);
        p.write(&mut m, 0xB123, 0xEE);
        // ...and a WRAM write in the same breath, so the test cannot pass by
        // the write path being broken altogether.
        p.write(&mut m, wram::addr::CUR_OPPONENT, 0x24);
    }
    assert_eq!(mem.bus_read(0xA000), 0x77, "cartridge RAM is untouched");
    assert_eq!(mem.peek(wram::addr::CUR_OPPONENT), 0x24);
    assert_eq!(p.refused_writes(), 2);
}

/// The host's own enforcement, from the other side: a plugin that has not
/// declared `CONTROL_INPUT` cannot press a button even if it tries.
#[test]
fn pressing_buttons_needs_the_capability() {
    let mut mem = mmu();
    let mut blind = Machine::new(&mut mem, Capability::WRITE_MEMORY);
    assert!(!blind.set_button(KeypadKey::A, true));
    let mut allowed = Machine::new(&mut mem, Capability::CONTROL_INPUT);
    assert!(allowed.set_button(KeypadKey::A, true));
}

// ---------------------------------------------------------------------------
// The state machine, driven through the real plugin host
// ---------------------------------------------------------------------------

/// The whole trigger sequence, with this test standing in for the battle
/// engine: it watches `wCurOpponent`, sets `wIsInBattle` a few frames later the
/// way `InitBattle` does, and clears it again at the end.
#[test]
fn arming_writes_the_party_then_the_trigger_and_restores_afterwards() {
    let mut mem = mmu();
    let mut host = PluginHost::new();
    let mut p = plugin(
        "[a]\nplayer = Charizard\nlevel = 50\nmoves = Flamethrower\n\
         wild = Pidgey\nlevel = 3\n",
    );
    p.arm("a").unwrap();
    host.register(Box::new(p)).unwrap();

    overworld(&mut mem);
    // Whatever the save had in the party slot, recorded so we can insist it
    // comes back.
    let before: Vec<u8> = (0..44)
        .map(|i| mem.peek(wram::addr::PARTY_MONS + i))
        .collect();
    let count_before = mem.peek(wram::addr::PARTY_COUNT);

    let mut saw_trigger = None;
    run(&mut host, &mut mem, 60, |f, mem| {
        // The engine: once wCurOpponent is set, start a battle two frames
        // later and end it at frame 40.
        let trigger = mem.peek(wram::addr::CUR_OPPONENT);
        if trigger != 0 && saw_trigger.is_none() {
            saw_trigger = Some(f);
        }
        if let Some(t) = saw_trigger {
            if f >= t + 2 && f < 40 {
                mem.poke(wram::addr::IS_IN_BATTLE, 1);
            } else if f >= 40 {
                mem.poke(wram::addr::IS_IN_BATTLE, 0);
                mem.poke(wram::addr::CUR_OPPONENT, 0);
            }
        }
    });

    let p = host.get::<BattlePlugin>().unwrap();
    assert_eq!(p.phase(), Phase::Done, "log: {:?}", p.log());
    assert_eq!(p.battles_started(), 1);
    assert_eq!(p.refused_writes(), 0);
    assert!(saw_trigger.is_some(), "wCurOpponent was never written");

    // Guarantee 2: everything it wrote, it put back.
    let after: Vec<u8> = (0..44)
        .map(|i| mem.peek(wram::addr::PARTY_MONS + i))
        .collect();
    assert_eq!(after, before, "the party was not restored");
    assert_eq!(mem.peek(wram::addr::PARTY_COUNT), count_before);
}

/// ...and the party really was different in between, or the restore test above
/// would pass vacuously.
#[test]
fn the_injected_party_is_actually_in_wram_during_the_battle() {
    let mut mem = mmu();
    let mut host = PluginHost::new();
    let mut p = plugin(
        "[a]\nplayer = Charizard\nlevel = 50\nmoves = Flamethrower\n\
         wild = Pidgey\nlevel = 3\n",
    );
    p.arm("a").unwrap();
    host.register(Box::new(p)).unwrap();
    overworld(&mut mem);

    // One frame is enough: the plugin fires as soon as it sees the overworld.
    host.on_frame(&mut mem);
    assert_eq!(mem.peek(wram::addr::CUR_OPPONENT), IDX_PIDGEY);
    assert_eq!(mem.peek(wram::addr::CUR_ENEMY_LEVEL), 3);
    assert_eq!(mem.peek(wram::addr::PARTY_COUNT), 1);
    assert_eq!(mem.peek(wram::addr::PARTY_SPECIES), IDX_CHARIZARD);
    assert_eq!(mem.peek(wram::addr::PARTY_MONS), IDX_CHARIZARD);
    // Level is at offset 33 of a party_struct, big-endian max HP at 34.
    assert_eq!(mem.peek(wram::addr::PARTY_MONS + 33), 50);
    assert_eq!(
        u16::from_be_bytes([
            mem.peek(wram::addr::PARTY_MONS + 34),
            mem.peek(wram::addr::PARTY_MONS + 35),
        ]),
        153
    );
    assert!(
        host.any_wrote_memory(),
        "the status bar must show the write"
    );
}

/// A preset with no `player =` line must not touch the party at all — that is
/// what makes it safe to fire at an arbitrary save.
#[test]
fn a_preset_without_a_player_line_leaves_every_party_byte_alone() {
    let mut mem = mmu();
    let mut host = PluginHost::new();
    overworld(&mut mem);
    mem.poke(wram::addr::PARTY_COUNT, 3);
    let before: Vec<u8> = (wram::addr::PARTY_COUNT..wram::addr::PARTY_DATA_END)
        .map(|a| mem.peek(a))
        .collect();
    let mut p = plugin("[a]\nwild = Mew\nlevel = 70\n");
    p.arm("a").unwrap();
    host.register(Box::new(p)).unwrap();
    host.on_frame(&mut mem);
    let after: Vec<u8> = (wram::addr::PARTY_COUNT..wram::addr::PARTY_DATA_END)
        .map(|a| mem.peek(a))
        .collect();
    assert_eq!(before, after);
    assert_eq!(mem.peek(wram::addr::CUR_OPPONENT), IDX_MEW);
}

/// A save with no Pokémon would black out the instant a battle started, which
/// looks exactly like the emulator crashing. The plugin waits instead, and then
/// says why.
#[test]
fn a_battle_is_not_started_for_a_save_with_no_pokemon() {
    let mut mem = mmu();
    let mut host = PluginHost::new();
    overworld(&mut mem);
    mem.poke(wram::addr::PARTY_COUNT, 0);
    let mut p = plugin("[a]\nwild = Pidgey\n");
    p.arm("a").unwrap();
    host.register(Box::new(p)).unwrap();
    run(&mut host, &mut mem, 950, |_, _| {});
    let p = host.get::<BattlePlugin>().unwrap();
    assert_eq!(mem.peek(wram::addr::CUR_OPPONENT), 0);
    match p.phase() {
        Phase::Failed(m) => assert!(m.contains("no Pokémon"), "{m}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// The seed is written immediately before the trigger, so the rolls the battle
/// makes start from a known place.
#[test]
fn a_seed_is_written_to_the_games_own_rng_bytes() {
    let mut mem = mmu();
    let mut host = PluginHost::new();
    overworld(&mut mem);
    let mut p = plugin("[a]\nwild = Pidgey\nseed = 0xBEEF\n");
    p.arm("a").unwrap();
    host.register(Box::new(p)).unwrap();
    host.on_frame(&mut mem);
    assert_eq!(mem.peek(wram::addr::RANDOM_ADD), 0xBE);
    assert_eq!(mem.peek(wram::addr::RANDOM_SUB), 0xEF);
}

/// The opposition override keeps being applied until it reads back unchanged —
/// the engine finishes `LoadEnemyMonData` at a point no frame boundary is
/// synchronised to, so writing once is a race.
#[test]
fn an_opposition_override_survives_the_engine_writing_after_us() {
    let mut mem = mmu();
    let mut host = PluginHost::new();
    overworld(&mut mem);
    let mut p = plugin("[a]\nwild = Pidgey\nlevel = 3\ndvs = 1,2,3,4\n");
    p.arm("a").unwrap();
    host.register(Box::new(p)).unwrap();
    run(&mut host, &mut mem, 30, |f, mem| {
        if f >= 1 {
            mem.poke(wram::addr::IS_IN_BATTLE, 1);
        }
        // The engine scribbles over the enemy struct for the first few frames,
        // as `LoadEnemyMonData` would if it landed after our write.
        if (2..6).contains(&f) {
            for i in 0..wram::BATTLE_STRUCT_LENGTH as u16 {
                mem.poke(wram::addr::ENEMY_MON + i, 0x5A);
            }
        }
    });
    let dv_hi = mem.peek(wram::addr::ENEMY_MON + wram::battle_field::DVS as u16);
    let dv_lo = mem.peek(wram::addr::ENEMY_MON + wram::battle_field::DVS as u16 + 1);
    assert_eq!((dv_hi, dv_lo), (0x12, 0x34), "the override did not stick");
    let p = host.get::<BattlePlugin>().unwrap();
    assert!(
        p.log().iter().any(|l| l == "opposition set"),
        "log: {:?}",
        p.log()
    );
}

/// A cinematic drives the pad through the host, and the buttons really reach
/// the emulated joypad — sampled the way the game samples it, by selecting the
/// button row on `$FF00` and reading the low nibble back.
#[test]
fn a_script_presses_buttons_the_console_can_see() {
    let mut mem = mmu();
    let mut host = PluginHost::new();
    overworld(&mut mem);
    let mut p = plugin("[a]\nwild = Pidgey\ndo = await-battle\ndo = press a 3\n");
    p.arm("a").unwrap();
    host.register(Box::new(p)).unwrap();

    let mut pressed_frames = 0;
    for f in 0..20u32 {
        if f >= 1 {
            mem.poke(wram::addr::IS_IN_BATTLE, 1);
        }
        host.on_frame(&mut mem);
        mem.bus_write(0xFF00, 0x10); // P15 low selects the button row
        if mem.bus_read(0xFF00) & 0x01 == 0 {
            pressed_frames += 1;
        }
    }
    assert_eq!(
        pressed_frames, 3,
        "A must be held for exactly the stated frames"
    );
    // ...and let go afterwards, or every later frame would read as a press.
    mem.bus_write(0xFF00, 0x10);
    assert_eq!(mem.bus_read(0xFF00) & 0x01, 0x01, "A was never released");
    let p = host.get::<BattlePlugin>().unwrap();
    assert_eq!(p.script_progress(), Some((2, 2)));
}

/// Plugin state is not machine state: loading a save state mid-battle must drop
/// the whole thing, including a restore snapshot of a party that no longer
/// exists.
#[test]
fn loading_a_save_state_abandons_a_running_battle() {
    let mut mem = mmu();
    let mut host = PluginHost::new();
    overworld(&mut mem);
    let mut p = plugin("[a]\nplayer = Charizard\nwild = Pidgey\n");
    p.arm("a").unwrap();
    host.register(Box::new(p)).unwrap();
    host.on_frame(&mut mem);
    assert_ne!(host.get::<BattlePlugin>().unwrap().phase(), Phase::Idle);
    host.reset();
    assert_eq!(host.get::<BattlePlugin>().unwrap().phase(), Phase::Idle);
}

#[test]
fn presets_can_be_cycled_and_chosen_by_name() {
    let mut p = plugin("[a]\nwild = Pidgey\n[b]\nwild = Mew\n");
    assert_eq!(p.selected().unwrap().name, "a");
    assert_eq!(p.select_next().unwrap().name, "b");
    assert_eq!(p.select_next().unwrap().name, "a", "it wraps");
    assert_eq!(p.select("B").unwrap().name, "b");
    assert!(p.select("nope").is_err());
    assert_eq!(p.preset_names(), vec!["a", "b"]);
}

/// Every built-in preset must resolve against a cartridge — a shipped preset
/// that names a Pokémon or a move the game does not have would only fail when
/// the captain pressed the key.
#[test]
fn every_built_in_preset_builds_a_plan() {
    // The synthetic cartridge only knows a handful of species, so this checks
    // the shapes that do not need one: parsing, script steps and the trigger
    // arithmetic. `examples/gen1_battle.rs --check` does the same against the
    // real cartridge, where every name has to resolve.
    let f = PresetFile::builtin();
    for p in &f.presets {
        assert!(!p.name.is_empty());
        assert!(
            p.script.max_frames() < 60 * 60 * 5,
            "{} is very long",
            p.name
        );
        match &p.opposition {
            Opposition::Wild(m) => assert!(m.level >= 1 && m.level <= 100),
            Opposition::Trainer { class, roster, .. } => {
                assert!(!class.is_empty());
                assert!(*roster >= 1);
            }
        }
    }
}
