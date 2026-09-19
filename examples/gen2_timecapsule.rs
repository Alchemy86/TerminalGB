//! Craft a Pokémon Gold save that can use the **Time Capsule**, and drive it.
//!
//! ```text
//! cargo run --release --features image,plugin-gen1-link,plugin-trainer \
//!     --example gen2_timecapsule -- --rom ~/roms/gold.gbc --make-save
//! ```
//!
//! # Why a crafted save
//!
//! The Time Capsule has three in-game prerequisites (established from
//! `pret/pokegold` in [`docs/link-cable.md`](../docs/link-cable.md) §7): Bill
//! spoken to in Ecruteak City's Pokémon Center, one day elapsed since, and a
//! party with nothing Gen 1 cannot represent. Reaching Ecruteak legitimately is
//! several hours of play, so this builds the state instead — the same class of
//! work as `plugins/battle-control`, which writes game state to start a battle.
//!
//! # Every address here comes from the disassembly's own symbol table
//!
//! `pokegold` was built at RGBDS 1.0.3 and the result is **byte-identical to
//! the cartridge** (SHA-1 `d8b8a360…`, matching the disassembly's own
//! `roms.sha1`), so `pokegold.sym` is exact for it. Constants come from the
//! assembler rather than being counted by hand. Symbol names are cited beside
//! every constant below, per the standard in
//! [`docs/gen1/README.md`](../docs/gen1/README.md).
//!
//! # The save is written by the *game*, then edited
//!
//! Hand-building a whole save is how you get a file that looks right in a hex
//! dump and hangs on load — the lesson Gen 1 taught this repository twice. So
//! the intro is played until the game saves by itself, and only then are the
//! four things that matter edited on top, with both checksums recomputed.

use std::path::{Path, PathBuf};

use terminalgb::gameboy::Gameboy;
use terminalgb::KeypadKey;

// --- Gen 2 addresses, all from pokegold.sym (WRAM bank 1 unless noted) ------

/// `wTilemap` — 20x18, and as in Gen 1 a tile id **is** a character code, so
/// the screen reads back as text.
const W_TILEMAP: u16 = 0xC3A0;
/// `wPlayerName`.
const W_PLAYER_NAME: u16 = 0xD1A3;
/// `wEventFlags` — `NUM_EVENTS` = 2048 bits.
const W_EVENT_FLAGS: u16 = 0xD7B7;
/// `wDailyFlags1`.
const W_DAILY_FLAGS1: u16 = 0xD968;
/// `wMapGroup` / `wMapNumber` / `wYCoord` / `wXCoord`.
const W_MAP_GROUP: u16 = 0xDA00;
const W_MAP_NUMBER: u16 = 0xDA01;
const W_Y_COORD: u16 = 0xDA02;
const W_X_COORD: u16 = 0xDA03;
/// `wPartyCount` / `wPartySpecies` / `wPartyMons`.
const W_PARTY_COUNT: u16 = 0xDA22;
const W_PARTY_SPECIES: u16 = 0xDA23;
const W_PARTY_MONS: u16 = 0xDA2A;
const W_PARTY_MON_OTS: u16 = 0xDB4A;
const W_PARTY_MON_NICKS: u16 = 0xDB8C;

/// `EVENT_MET_BILL` = 1810, so byte 226, bit 2 of `wEventFlags`.
///
/// Cleared by Bill's own script in Ecruteak
/// (`EcruteakPokcenter1FBillActivatesTimeCapsuleScript`), and
/// `LinkReceptionistScript_TimeCapsule` refuses while it is **set**.
const EVENT_MET_BILL: usize = 1810;
/// `DAILYFLAGS1_TIME_CAPSULE_F`. Bill *sets* it, and the receptionist refuses
/// while it is set; `CheckDailyResetTimer` clears `wDailyFlags1` once a day has
/// passed. Clearing it is "and then a day went by".
const DAILYFLAGS1_TIME_CAPSULE_F: u8 = 3;
/// `PARTYMON_STRUCT_LENGTH`.
const PARTYMON_LEN: usize = 48;
/// `JOHTO_POKEMON` — species at or above this are Gen 2 and the Time Capsule
/// rejects them (`CheckTimeCapsuleCompatibility`).
const JOHTO_POKEMON: u8 = 152;
/// `STRUGGLE` — moves above this are Gen 2 and are rejected the same way.
const STRUGGLE: u8 = 165;
/// `GROUP_POKECENTER_2F` / `MAP_POKECENTER_2F`. Every Pokémon Center's upstairs
/// is one shared map; the Time Capsule receptionist stands at (13, 3) with no
/// gating of her own beyond the script.
const GROUP_POKECENTER_2F: u8 = 20;
const MAP_POKECENTER_2F: u8 = 1;

// --- SRAM, from pokegold.sym. A `.sav` is raw cartridge RAM, bank 0 first,
// 8 KiB per bank (src/mbc/battery.rs), so a bank/address pair is a file offset.
/// `sGameData` = bank 1, `$A009`. The 16-bit checksum covers up to
/// `sGameDataEnd`.
const S_GAME_DATA: usize = 0x2000 + 0x009;
/// `sGameDataEnd` / `sChecksum` = bank 1, `$AD69`.
const S_GAME_DATA_END: usize = 0x2000 + 0xD69;
const S_CHECKSUM: usize = 0x2000 + 0xD69;
/// `sBackupChecksum` = bank 3, `$BE6D`. Gold keeps a second copy of the save
/// and checks it when the first fails, so both have to agree.
const S_BACKUP_CHECKSUM: usize = 0x6000 + 0x1E6D;

fn main() {
    let mut rom: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut probe = false;
    let mut make = false;
    let mut verify: Option<PathBuf> = None;
    let mut link_trade: Option<PathBuf> = None;
    let mut capsule_trade: Option<PathBuf> = None;
    let mut blue_rom: Option<PathBuf> = None;
    let mut blue_save: Option<PathBuf> = None;
    let mut shots: Option<PathBuf> = None;
    let mut frames = 60 * 60 * 6;
    let mut verbose = false;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rom" => rom = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from),
            "--probe" => probe = true,
            "--make-save" => make = true,
            "--verify" => verify = args.next().map(PathBuf::from),
            "--link-trade" => link_trade = args.next().map(PathBuf::from),
            "--capsule-trade" => capsule_trade = args.next().map(PathBuf::from),
            "--blue-rom" => blue_rom = args.next().map(PathBuf::from),
            "--blue-save" => blue_save = args.next().map(PathBuf::from),
            "--shots" => shots = args.next().map(PathBuf::from),
            "--frames" => {
                frames = args.next().and_then(|s| s.parse().ok()).unwrap_or(frames)
            }
            "--verbose" => verbose = true,
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(rom) = rom else {
        eprintln!(
            "usage: gen2_timecapsule --rom <gold.gbc> [--probe] [--make-save] \
             [--out <fixture.sav>] [--frames N] [--verbose]"
        );
        std::process::exit(2);
    };

    // Never open the player's ROM directory for writing: the MBC puts `<rom>.sav`
    // beside the cartridge it was handed.
    let dir = PathBuf::from("target/gen2-timecapsule");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let work = dir.join(rom.file_name().expect("rom filename"));
    std::fs::copy(&rom, &work).expect("copy rom");
    let sav = work.with_extension("sav");
    let _ = std::fs::remove_file(&sav);

    if probe {
        run_probe(&work, frames);
        return;
    }
    if make {
        make_save(&work, &sav, out.as_deref(), frames, verbose);
        return;
    }
    if let Some(fixture) = verify {
        std::fs::copy(&fixture, &sav).expect("stage the fixture");
        run_verify(&work, frames, verbose);
        return;
    }
    if let Some(fixture) = link_trade {
        std::fs::copy(&fixture, &sav).expect("stage the fixture");
        run_link_trade(&work, frames, verbose);
        return;
    }
    if let Some(fixture) = capsule_trade {
        // Any Gold save works here, the maintainer's included: `patch` is
        // idempotent on the two Time Capsule gates and replaces the party with
        // one the receptionist accepts — on the STAGED COPY only.
        let mut sram = std::fs::read(&fixture).expect("gold save fixture");
        patch(&mut sram);
        std::fs::write(&sav, &sram).expect("stage gold save");
        let Some(blue_rom) = blue_rom else {
            eprintln!("--capsule-trade needs --blue-rom <blue.gb>");
            std::process::exit(2);
        };
        run_capsule_trade(
            &work,
            &blue_rom,
            blue_save.as_deref(),
            frames,
            shots.as_deref(),
            verbose,
        );
        return;
    }
    eprintln!("nothing to do: pass --probe or --make-save");
}

// ---------------------------------------------------------------------------
// Probe: what does the intro actually look like?
// ---------------------------------------------------------------------------

fn run_probe(rom: &Path, frames: u32) {
    let mut gb = Gameboy::new(std::fs::read(rom).expect("rom"), Some(rom.to_path_buf()));
    // A machine with no APU is not a quiet Game Boy, it is one with the sound
    // chip pulled out, and a cartridge can tell — see `examples/blackout_probe.rs`.
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    println!("title: {}", gb.rom_title());
    let mut bot = IntroBot::default();
    let mut last = String::new();
    for f in 0..frames {
        bot.act(&mut gb);
        gb.frame();
        if f % 60 == 0 {
            let t = screen_text(&mut gb);
            let squashed = squash(&t);
            if squashed != last && !squashed.is_empty() {
                println!(
                    "[{f:6}] map {}:{} ({},{}) | {squashed}",
                    gb.peek(W_MAP_GROUP),
                    gb.peek(W_MAP_NUMBER),
                    gb.peek(W_X_COORD),
                    gb.peek(W_Y_COORD),
                );
                last = squashed;
            }
        }
    }
    println!("final screen:\n{}", indent(&screen_text(&mut gb)));
}

/// Collapse the screen to one line of the words on it, so a change is obvious.
fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// Stage 1: play the intro until the game writes a save of its own.
// ---------------------------------------------------------------------------

/// Mashes through Gold's opening. Gen 2 asks for the time before the main menu
/// and for a name after Oak's speech, so this is not a pure A-masher — but it
/// is close, because every one of those screens accepts a default.
#[derive(Default)]
struct IntroBot {
    hold: Option<(KeypadKey, u32)>,
    cooldown: u32,
    ticks: u32,
}

impl IntroBot {
    fn press(&mut self, gb: &mut Gameboy, key: KeypadKey, hold: u32) {
        gb.keydown(key);
        self.hold = Some((key, hold));
    }

    fn act(&mut self, gb: &mut Gameboy) {
        if let Some((key, left)) = self.hold {
            if left <= 1 {
                gb.keyup(key);
                self.hold = None;
                self.cooldown = 4;
            } else {
                self.hold = Some((key, left - 1));
            }
            return;
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return;
        }
        self.ticks += 1;
        // Alternate A and START: A advances text and confirms menus, START is
        // what the naming screen needs (mashing A there types letters for ever
        // — the trap the agent crew already paid for in Gen 1).
        if self.ticks % 8 == 0 {
            self.press(gb, KeypadKey::Start, 4);
        } else {
            self.press(gb, KeypadKey::A, 4);
        }
    }
}

/// `wMapObjects` — the player is object 0. `MAPOBJECT_Y_COORD` = 2,
/// `MAPOBJECT_X_COORD` = 3.
const W_MAP_OBJECTS: u16 = 0xD445;
/// `wObjectStructs` (= `wPlayerStruct`). `OBJECT_MAP_X` = 16,
/// `OBJECT_MAP_Y` = 17.
const W_OBJECT_STRUCTS: u16 = 0xD1FD;

/// Where the player is put: directly below the Time Capsule receptionist, who
/// stands at (13, 3) with the Time Capsule warp at (13, 2) right behind her
/// (`maps/Pokecenter2F.asm`). The map is 8x4 blocks, i.e. 16x8 tiles.
const START_X: u8 = 13;
const START_Y: u8 = 4;

fn make_save(rom: &Path, sav: &Path, out: Option<&Path>, frames: u32, verbose: bool) {
    let out = out
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("target/gen2-timecapsule/timecapsule.sav"));

    {
        let mut gb =
            Gameboy::new(std::fs::read(rom).expect("rom"), Some(rom.to_path_buf()));
        // Same rule as above: a machine with no APU is not a quiet Game Boy.
        gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
        let mut bot = SaveBot::default();
        let mut ok = false;
        for f in 0..frames {
            bot.act(&mut gb, verbose);
            gb.frame();
            if bot.stage == SaveStage::Saved && bot.settle > 180 {
                ok = true;
                println!("the game saved by itself at frame {f}");
                break;
            }
        }
        if !ok {
            eprintln!(
                "could not get the game to save (stage {:?}). Last screen:\n{}",
                bot.stage,
                indent(&screen_text(&mut gb))
            );
            std::process::exit(1);
        }
        gb.flush_battery();
    }

    let mut sram = std::fs::read(sav).expect("the game should have written a save");
    println!("game wrote {} bytes", sram.len());
    patch(&mut sram);
    std::fs::write(&out, &sram).expect("write fixture");
    println!("fixture written to {}", out.display());
}

/// Map a WRAM address to its offset in the `.sav`.
///
/// Gold saves three separate WRAM runs into three separate SRAM runs
/// (`SavePlayerData` / `SavePokemonData`, `engine/menus/save.asm`), so this is
/// not one base plus an offset — getting that wrong is how a patch lands in the
/// middle of somebody else's data and the checksum still passes.
fn save_offset(addr: u16) -> usize {
    // `wPlayerData` $D1A1..$D9EE -> `sPlayerData` bank 1 $A009
    // `wCurMapData` $D9EE..$DA22 -> `sCurMapData` bank 1 $A856
    // `wPokemonData` $DA22..$DF01 -> `sPokemonData` bank 1 $A88A
    const BANK1: usize = 0x2000;
    match addr {
        0xD1A1..=0xD9ED => BANK1 + 0x009 + (addr - 0xD1A1) as usize,
        0xD9EE..=0xDA21 => BANK1 + 0x856 + (addr - 0xD9EE) as usize,
        0xDA22..=0xDF00 => BANK1 + 0x88A + (addr - 0xDA22) as usize,
        _ => panic!("{addr:#06X} is not in a saved WRAM region"),
    }
}

/// The four edits that turn "just started, standing in the bedroom" into "can
/// use the Time Capsule", and the checksum that makes the game believe them.
fn patch(sram: &mut [u8]) {
    // 1. Bill has been spoken to. `LinkReceptionistScript_TimeCapsule` refuses
    //    while `EVENT_MET_BILL` is set; Bill's own script clears it.
    let ev = save_offset(W_EVENT_FLAGS) + EVENT_MET_BILL / 8;
    sram[ev] &= !(1 << (EVENT_MET_BILL % 8));

    // 2. ...and a day has gone by. `ENGINE_TIME_CAPSULE` is a *daily* flag that
    //    Bill sets and `CheckDailyResetTimer` clears; the receptionist refuses
    //    while it is set. This is the "come back tomorrow" the game means.
    let df = save_offset(W_DAILY_FLAGS1);
    sram[df] &= !(1 << DAILYFLAGS1_TIME_CAPSULE_F);

    // 3. The player is **not** moved here. A continue runs
    //    `MapSetupScript_Continue`, whose second command is
    //    `LoadMapAttributes_SkipObjects` — a continue deliberately does not
    //    load map objects, because it assumes you are on the map you saved on.
    //    Editing `wMapGroup` in the save therefore lands you on the right map
    //    with the *previous* map's NPCs, no receptionist to talk to, and a
    //    stale camera. Measured: the room drew, the player sprite sat in the
    //    wrong place, and nothing responded.
    //
    //    So the player walks to the stairs instead and the *warp* is
    //    redirected at run time (see `VerifyBot`), which takes the full
    //    `MAPSETUP_DOOR` path and loads objects, camera and scene scripts the
    //    way the game does for itself.

    // 4. A party the Time Capsule will accept: a Gen 1 species, Gen 1 moves,
    //    and **no held item** — those are the three things
    //    `CheckTimeCapsuleCompatibility` rejects.
    let ot: Vec<u8> = {
        let base = save_offset(W_PLAYER_NAME);
        sram[base..base + 11].to_vec()
    };
    let id = {
        let base = save_offset(W_PLAYER_NAME) - 2; // `wPlayerID` precedes the name
        [sram[base], sram[base + 1]]
    };
    let mon = gen2_pidgey(id);
    assert!(mon[0] < JOHTO_POKEMON, "species must exist in Gen 1");
    assert_eq!(mon[1], 0, "a held item is rejected by the Time Capsule");
    assert!(
        mon[2..6].iter().all(|&m| m <= STRUGGLE),
        "every move must exist in Gen 1"
    );

    sram[save_offset(W_PARTY_COUNT)] = 1;
    sram[save_offset(W_PARTY_SPECIES)] = mon[0];
    sram[save_offset(W_PARTY_SPECIES) + 1] = 0xFF; // `wPartyEnd`
    let p = save_offset(W_PARTY_MONS);
    sram[p..p + PARTYMON_LEN].copy_from_slice(&mon);
    let nick = save_offset(W_PARTY_MON_NICKS);
    sram[nick..nick + 11]
        .copy_from_slice(b"\x8f\xa8\xa3\xa6\xa4\xb8\x50\x50\x50\x50\x50");
    let ots = save_offset(W_PARTY_MON_OTS);
    sram[ots..ots + 11].copy_from_slice(&ot);

    // 5. The checksum. A 16-bit sum of every byte of `sGameData`, little-endian
    //    (`SaveChecksum` / `Checksum`, `engine/menus/save.asm`). A stale one is
    //    not a soft failure: the game rejects the save.
    let sum: u16 = sram[S_GAME_DATA..S_GAME_DATA_END]
        .iter()
        .fold(0u16, |a, &b| a.wrapping_add(b as u16));
    sram[S_CHECKSUM] = sum as u8;
    sram[S_CHECKSUM + 1] = (sum >> 8) as u8;
    println!(
        "patched: Bill met, day elapsed, party of 1 (species {}), \
         Pokemon Center 2F ({START_X},{START_Y}), checksum {sum:#06x}",
        mon[0]
    );
    let _ = S_BACKUP_CHECKSUM;
}

/// A level 5 Pidgey, in Gen 2's 48-byte `party_struct`
/// (`box_struct` + battle stats, `macros/ram.asm`). Multi-byte fields are
/// **big-endian**, as they are throughout both generations.
fn gen2_pidgey(id: [u8; 2]) -> [u8; PARTYMON_LEN] {
    let mut m = [0u8; PARTYMON_LEN];
    m[0] = 16; // PIDGEY
    m[1] = 0; // no item — the Time Capsule rejects mail and any held item
    m[2] = 33; // TACKLE
    m[3] = 45; // GROWL
    m[6] = id[0];
    m[7] = id[1];
    // Exp for level 5 on the medium-fast curve is 5^3 = 125.
    m[8] = 0;
    m[9] = 0;
    m[10] = 125;
    // Stat exp all zero (bytes 11..21), DVs at 21..23.
    m[21] = 0x9A;
    m[22] = 0xAA;
    m[23] = 35; // PP
    m[24] = 40;
    m[27] = 70; // Happiness
    m[31] = 5; // Level
               // Battle stats. Nothing validates these; they are what a level 5 Pidgey
               // with these DVs works out to, so the panel and the trade look sane.
    let be = |m: &mut [u8; PARTYMON_LEN], off: usize, v: u16| {
        m[off] = (v >> 8) as u8;
        m[off + 1] = v as u8;
    };
    be(&mut m, 34, 20); // HP
    be(&mut m, 36, 20); // MaxHP
    be(&mut m, 38, 10); // Attack
    be(&mut m, 40, 9); // Defense
    be(&mut m, 42, 11); // Speed
    be(&mut m, 44, 9); // SpclAtk
    be(&mut m, 46, 9); // SpclDef
    m
}

// ---------------------------------------------------------------------------
// The bot that gets the game to save.
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
enum SaveStage {
    #[default]
    Intro,
    CloseMenu,
    OpenMenu,
    PickSave,
    Confirm,
    Saved,
}

#[derive(Default)]
struct SaveBot {
    intro: IntroBot,
    stage: SaveStage,
    hold: Option<(KeypadKey, u32)>,
    cooldown: u32,
    ticks: u32,
    settle: u32,
}

impl SaveBot {
    fn press(&mut self, gb: &mut Gameboy, key: KeypadKey, hold: u32) {
        gb.keydown(key);
        self.hold = Some((key, hold));
    }

    fn act(&mut self, gb: &mut Gameboy, verbose: bool) {
        if self.stage == SaveStage::Intro {
            // "In the game" has to be a **stable, sane** map, not merely a
            // non-zero byte. `wMapGroup` lives in WRAM bank 1 and reads as
            // uninitialised noise before the game settles — the first probe
            // logged `map 127:211` on frame 0 — so a bare `!= 0` test leaves
            // the intro immediately and then mashes B and START at the title
            // screen for ever.
            let (g, n) = (gb.peek(W_MAP_GROUP), gb.peek(W_MAP_NUMBER));
            let plausible = (1..=27).contains(&g) && (1..=60).contains(&n);
            if plausible {
                self.settle += 1;
            } else {
                self.settle = 0;
            }
            if self.settle >= 60 {
                self.settle = 0;
                self.stage = SaveStage::CloseMenu;
                // Whatever the intro was holding must be let go, or the next
                // press stacks on top of it.
                for k in [
                    KeypadKey::A,
                    KeypadKey::B,
                    KeypadKey::Start,
                    KeypadKey::Select,
                    KeypadKey::Up,
                    KeypadKey::Down,
                ] {
                    gb.keyup(k);
                }
                if verbose {
                    println!("  in the game: map {g}:{n}");
                }
                return;
            }
            self.intro.act(gb);
            return;
        }

        if let Some((key, left)) = self.hold {
            if left <= 1 {
                gb.keyup(key);
                self.hold = None;
                self.cooldown = 6;
            } else {
                self.hold = Some((key, left - 1));
            }
            return;
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return;
        }
        self.ticks += 1;
        let text = screen_text(gb);
        let has_menu = text.contains("SAVE") && text.contains("OPTION");

        match self.stage {
            SaveStage::Intro => unreachable!(),
            SaveStage::CloseMenu => {
                if has_menu {
                    self.press(gb, KeypadKey::B, 4);
                } else {
                    self.stage = SaveStage::OpenMenu;
                }
            }
            SaveStage::OpenMenu => {
                if has_menu {
                    self.stage = SaveStage::PickSave;
                } else {
                    self.press(gb, KeypadKey::Start, 4);
                }
            }
            SaveStage::PickSave => {
                // **Find the cursor on screen** rather than trusting a WRAM
                // variable. `wMenuCursorY` reads 2 for this menu whatever the
                // cursor is doing — dumping the tilemap showed the real cursor,
                // tile `$ED`, sitting on row 4 while that byte insisted on 2.
                // The screen is the thing the player sees, so it is the thing
                // to drive from.
                let row = cursor_row(gb);
                let on_save = row
                    .and_then(|r| text.lines().nth(r).map(|l| l.contains("SAVE")))
                    .unwrap_or(false);
                if verbose && self.ticks % 10 == 0 {
                    println!(
                        "  cursor row {row:?} -> {:?}",
                        row.and_then(|r| text.lines().nth(r)).map(str::trim)
                    );
                }
                if on_save {
                    self.stage = SaveStage::Confirm;
                    self.press(gb, KeypadKey::A, 4);
                } else {
                    self.press(gb, KeypadKey::Down, 4);
                }
            }
            SaveStage::Confirm => {
                // "Would you like to save the game?" opens on YES, and the
                // "SAVED!" box needs one more A.
                if text.contains("SAVING") || text.contains("DON") {
                    self.stage = SaveStage::Saved;
                    return;
                }
                self.press(gb, KeypadKey::A, 4);
                if self.ticks > 400 {
                    self.stage = SaveStage::Saved;
                }
            }
            SaveStage::Saved => {
                self.settle += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Reading the screen. Gen 2 loads the same font Gen 1 does and writes character
// codes straight into `wTilemap`, so the tilemap reads back as text — the same
// trick, and `constants/charmap.asm` confirms `$80` is still "A".
// ---------------------------------------------------------------------------

fn screen_text(gb: &mut Gameboy) -> String {
    let mut out = String::new();
    for row in 0..18u16 {
        for col in 0..20u16 {
            out.push(decode_char(gb.peek(W_TILEMAP + row * 20 + col)));
        }
        out.push('\n');
    }
    out
}

/// Which screen row the menu cursor is on. Gen 2 draws it as tile `$ED`, the
/// same glyph Gen 1 uses, and it appears exactly once in an open menu.
fn cursor_row(gb: &mut Gameboy) -> Option<usize> {
    (0..18usize)
        .find(|&r| (0..20u16).any(|c| gb.peek(W_TILEMAP + (r as u16) * 20 + c) == 0xED))
}

fn decode_char(t: u8) -> char {
    match t {
        0x80..=0x99 => (b'A' + (t - 0x80)) as char,
        0xA0..=0xB9 => (b'a' + (t - 0xA0)) as char,
        0xF6..=0xFF => (b'0' + (t - 0xF6)) as char,
        0x9A => '(',
        0x9B => ')',
        0x9C => ':',
        0x9D => ';',
        0xE8 => '.',
        0xF2 => '.',
        0xF3 => '/',
        0xF4 => ',',
        _ => ' ',
    }
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("    |{l}|"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Stage 2: prove it. **Boot the crafted save and walk up to the receptionist**
// — a save that looks right in a hex dump and fails in game is worthless, so
// the check is what the game says, not what the bytes say.
// ---------------------------------------------------------------------------

fn run_verify(rom: &Path, frames: u32, verbose: bool) {
    let mut gb = Gameboy::new(std::fs::read(rom).expect("rom"), Some(rom.to_path_buf()));
    // Same rule as above: a machine with no APU is not a quiet Game Boy.
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    let mut bot = VerifyBot::default();
    let mut last = String::new();
    let mut verdict = None;

    for f in 0..frames {
        bot.act(&mut gb, verbose);
        step_frame_forcing_warp(&mut gb, bot.stage == VerifyStage::ToStairs);
        let text = screen_text(&mut gb);
        let squashed = squash(&text);
        if verbose && squashed != last && !squashed.is_empty() {
            println!(
                "[{f:6}] {:?} map {}:{} ({},{}) | {squashed}",
                bot.stage,
                gb.peek(W_MAP_GROUP),
                gb.peek(W_MAP_NUMBER),
                gb.peek(W_X_COORD),
                gb.peek(W_Y_COORD),
            );
            last = squashed.clone();
        }
        // The receptionist's own words are the verdict.
        if squashed.contains("being adjusted") {
            verdict = Some(Err("TIME CAPSULE is being adjusted — a prerequisite is \
                                still unmet (Bill, or the daily flag)"));
            break;
        }
        // Her own words, in order of how far we got.
        if squashed.contains("trade across time") {
            verdict = Some(Ok(
                "the receptionist offered the TIME CAPSULE — every prerequisite \
                 is satisfied",
            ));
        }
        if squashed.contains("too new") || squashed.contains("t be taken") {
            verdict = Some(Err("the party was rejected as incompatible with Gen 1"));
            break;
        }
        if squashed.contains("wait") && verdict.is_some() {
            verdict = Some(Ok(
                "the receptionist accepted the party and is waiting for a link \
                 partner",
            ));
            break;
        }
    }

    for i in 0..8u16 {
        let b = W_MAP_OBJECTS + i * 16;
        let (spr, y, x) = (gb.peek(b + 1), gb.peek(b + 2), gb.peek(b + 3));
        if spr != 0 {
            println!(
                "  map object {i}: sprite {spr:#04x} at map ({},{})",
                x.wrapping_sub(4),
                y.wrapping_sub(4)
            );
        }
    }
    println!(
        "player map object: y={} x={} | object struct: mapx={} mapy={} facing={} \
         spritex={} spritey={}",
        gb.peek(W_MAP_OBJECTS + 2),
        gb.peek(W_MAP_OBJECTS + 3),
        gb.peek(W_OBJECT_STRUCTS + 16),
        gb.peek(W_OBJECT_STRUCTS + 17),
        gb.peek(W_OBJECT_STRUCTS + 13),
        gb.peek(W_OBJECT_STRUCTS + 23),
        gb.peek(W_OBJECT_STRUCTS + 24),
    );
    println!(
        "loaded save: map {}:{} at ({},{}), party {} (species {})",
        gb.peek(W_MAP_GROUP),
        gb.peek(W_MAP_NUMBER),
        gb.peek(W_X_COORD),
        gb.peek(W_Y_COORD),
        gb.peek(W_PARTY_COUNT),
        gb.peek(W_PARTY_SPECIES),
    );
    println!("screen:\n{}", indent(&screen_text(&mut gb)));
    let shot = PathBuf::from("target/gen2-timecapsule/verify.png");
    if let Some(img) = image::RgbaImage::from_raw(160, 144, gb.image().to_vec()) {
        let _ = img.save(&shot);
        println!("screenshot: {}", shot.display());
    }
    match verdict {
        Some(Ok(m)) => println!("VERIFIED: {m}"),
        Some(Err(m)) => {
            eprintln!("FAILED: {m}");
            std::process::exit(1);
        }
        None => {
            eprintln!("FAILED: never reached the receptionist");
            std::process::exit(1);
        }
    }
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
enum VerifyStage {
    #[default]
    Continue,
    /// Walking to the bedroom stairs with the warp destination overridden.
    ToStairs,
    /// On Pokémon Center 2F, walking to the Time Capsule desk.
    ToDesk,
    Talk,
    /// Through the door: in the Time Capsule room with the link up.
    Linked,
}

/// `wNextWarp` / `wNextMapGroup` / `wNextMapNumber` — where the warp the player
/// is stepping through will take them. Runtime only (below `wPlayerData`, so
/// not in the save), read by `EnterMapWarp` (`home/map.asm`).
const W_NEXT_WARP: u16 = 0xD043;

/// Which warp of Pokémon Center 2F to arrive at: warp 1 is the staircase at
/// (0, 7) (`maps/Pokecenter2F.asm`).
const PC2F_ARRIVE_WARP: u8 = 1;

/// `GROUP_TIME_CAPSULE` / `MAP_TIME_CAPSULE` — the room the receptionist sends
/// you into once the link is up.
const GROUP_TIME_CAPSULE: u8 = 20;
const MAP_TIME_CAPSULE: u8 = 4;

/// One frame, optionally holding the warp destination at Pokémon Center 2F.
///
/// The override is written **between instructions**, not once a frame: the game
/// loads `wNextWarp` from the warp event and reads it back a handful of
/// instructions later, so a per-frame poke would lose that race about half the
/// time. This is a one-off tool, so paying an instruction-granular loop for
/// certainty is the right trade.
fn step_frame_forcing_warp(gb: &mut Gameboy, force: bool) {
    if !force {
        gb.frame();
        return;
    }
    gb.begin_frame();
    let mut ticks = 0u32;
    while ticks < terminalgb::gameboy::FRAME_TICKS {
        ticks += gb.step_instruction();
        gb.debug_write(W_NEXT_WARP, PC2F_ARRIVE_WARP);
        gb.debug_write(W_NEXT_WARP + 1, GROUP_POKECENTER_2F);
        gb.debug_write(W_NEXT_WARP + 2, MAP_POKECENTER_2F);
    }
    gb.end_frame();
}

struct VerifyBot {
    stage: VerifyStage,
    hold: Option<(KeypadKey, u32)>,
    cooldown: u32,
    ticks: u32,
    settle: u32,
    last_pos: (u8, u8),
    blocked: u32,
    /// The tile whose warp is stepped through (with the destination overridden
    /// to Pokémon Center 2F). `(7, 0)` is the bedroom staircase the crafted
    /// fixture starts beside; a save standing elsewhere needs its own — see
    /// `warp_tile_for`.
    warp_tile: (u8, u8),
    /// Trade-screen bookkeeping, used only in the capsule-trade run.
    on_trade_option: bool,
    traded: bool,
    /// After the trade: open the received Pokémon's stats screen, so the
    /// screenshot is Gold *displaying* it rather than our reading of WRAM.
    after_trade: u32,
    stats_open: bool,
}

impl Default for VerifyBot {
    fn default() -> Self {
        VerifyBot {
            stage: VerifyStage::default(),
            hold: None,
            cooldown: 0,
            ticks: 0,
            settle: 0,
            last_pos: (0, 0),
            blocked: 0,
            warp_tile: (7, 0),
            on_trade_option: false,
            traded: false,
            after_trade: 0,
            stats_open: false,
        }
    }
}

/// Which tile to walk onto to leave the save's starting map through a warp,
/// per map. Only maps a supplied save has actually started on are listed —
/// anything else is refused rather than guessed, because walking onto a
/// non-warp tile forever looks exactly like a broken bot.
fn warp_tile_for(group: u8, number: u8) -> Option<(u8, u8)> {
    match (group, number) {
        // PLAYERS_HOUSE_2F: the staircase, `warp_event 7, 0`
        // (`maps/PlayersHouse2F.asm`) — the crafted fixture's bedroom.
        (24, 7) => Some((7, 0)),
        // NEW_BARK_TOWN: the player's own front door, `warp_event 13, 5,
        // PLAYERS_HOUSE_1F` (`maps/NewBarkTown.asm`) — where the maintainer's
        // real save is standing, one tile below it at (13, 6).
        (24, 4) => Some((13, 5)),
        _ => None,
    }
}

/// The Time Capsule receptionist stands at (13, 3). Which way to turn to face
/// her from `(x, y)`, or `None` if not next to her yet.
fn facing_desk(x: u8, y: u8) -> Option<KeypadKey> {
    match (x, y) {
        (13, 4) => Some(KeypadKey::Up),
        (12, 3) => Some(KeypadKey::Right),
        (14, 3) => Some(KeypadKey::Left),
        _ => None,
    }
}

impl VerifyBot {
    fn press(&mut self, gb: &mut Gameboy, key: KeypadKey, hold: u32) {
        gb.keydown(key);
        self.hold = Some((key, hold));
    }

    /// One step towards `(tx, ty)`, alternating the axis whenever the player
    /// has not actually moved — enough to get round indoor furniture with no
    /// collision data at all.
    fn step_towards(&mut self, gb: &mut Gameboy, x: u8, y: u8, tx: u8, ty: u8) {
        if (x, y) == self.last_pos {
            self.blocked += 1;
        } else {
            self.blocked = 0;
        }
        self.last_pos = (x, y);
        let prefer_x = (self.blocked / 3) % 2 == 0;
        let dx = match x.cmp(&tx) {
            std::cmp::Ordering::Less => Some(KeypadKey::Right),
            std::cmp::Ordering::Greater => Some(KeypadKey::Left),
            std::cmp::Ordering::Equal => None,
        };
        let dy = match y.cmp(&ty) {
            std::cmp::Ordering::Less => Some(KeypadKey::Down),
            std::cmp::Ordering::Greater => Some(KeypadKey::Up),
            std::cmp::Ordering::Equal => None,
        };
        if let Some(k) = if prefer_x { dx.or(dy) } else { dy.or(dx) } {
            self.press(gb, k, 8);
        }
    }

    fn act(&mut self, gb: &mut Gameboy, verbose: bool) {
        if let Some((key, left)) = self.hold {
            if left <= 1 {
                gb.keyup(key);
                self.hold = None;
                self.cooldown = 6;
            } else {
                self.hold = Some((key, left - 1));
            }
            return;
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return;
        }
        self.ticks += 1;
        // **Once the receptionist has sent us through, stop navigating.** The
        // player is warped into the Time Capsule room, so the "am I beside the
        // desk?" test goes false and a bot that keeps steering walks straight
        // back out of the link. Measured: exactly that, mid-handshake.
        if gb.peek(W_MAP_GROUP) == GROUP_TIME_CAPSULE
            && gb.peek(W_MAP_NUMBER) == MAP_TIME_CAPSULE
        {
            self.stage = VerifyStage::Linked;
        }
        match self.stage {
            VerifyStage::Linked => {
                // The trade selection screen is Gen 1's, redrawn by Gen 2: both
                // parties in two columns with CANCEL, and picking a Pokémon
                // opens the same horizontal "STATS     TRADE" menu
                // (`engine/link/link.asm` carries the literal
                // `db "STATS     TRADE@"`). While that screen is up, a
                // direction key moves the *cursor* — the seat logic below must
                // not run, or its face-the-console presses walk the selection
                // onto the wrong Pokémon.
                let text = screen_text(gb);
                if text.contains("Trade completed") {
                    self.traded = true;
                }
                if self.traded {
                    // The game runs one more exchange and redraws the
                    // selection screen. Then open the received Pokémon's
                    // STATS page — the cursor lands on our own slot 1 (the
                    // only party member) and the per-Pokémon menu opens on
                    // STATS, so it is A twice — which makes the screenshot
                    // Gold *displaying* the Pokémon, shiny palette and all,
                    // rather than our reading of its WRAM.
                    self.after_trade += 1;
                    if self.stats_open {
                        return;
                    }
                    let stats_trade = text.contains("STATS") && text.contains("TRADE");
                    if stats_trade {
                        self.press(gb, KeypadKey::A, 6);
                        self.stats_open = true;
                        return;
                    }
                    if self.after_trade > 600
                        && self.after_trade % 40 == 0
                        && text.contains("CANCEL")
                    {
                        self.press(gb, KeypadKey::A, 6);
                    }
                    return;
                }
                let stats_trade = text.contains("STATS") && text.contains("TRADE");
                if stats_trade || text.contains("CANCEL") {
                    if stats_trade && !self.on_trade_option {
                        self.on_trade_option = true;
                        self.press(gb, KeypadKey::Right, 6);
                        return;
                    }
                    if !stats_trade {
                        self.on_trade_option = false;
                    }
                    self.press(gb, KeypadKey::A, 6);
                    return;
                }
                // Two consoles, at (4, 4) and (5, 4), and each is a
                // *direction-restricted* bg event: (4, 4) fires only when the
                // player faces RIGHT into it and (5, 4) only when they face
                // LEFT (`maps/TimeCapsule.asm`). So the seat is (3, 4) facing
                // right or (6, 4) facing left — and which one is OURS is not
                // geometry: the partner's avatar (a `SPRITE_CHRIS` object)
                // stands on one of those two tiles.
                // `TimeCapsuleSetWhichChrisCallback` picks it with
                // `CableClubCheckWhichChris` (`engine/link/link.asm`), which
                // reads `hSerialConnectionStatus` — at **$FFCD** on Gen 2, not
                // Gen 1's $FFAA — and shows Chris on the LEFT seat when this
                // console took the external clock. Guessing the seat from the
                // arrival x walked the bot straight into the NPC, where it
                // stood blocked for a whole 60,000-frame run while the
                // partner's screen said "PLEASE WAIT!".
                let (x, y) = (gb.peek(W_X_COORD), gb.peek(W_Y_COORD));
                let left_seat = match gb.peek(0xFFCD) {
                    0x02 => true,  // internal clock: friend at (6,4), we sit left
                    0x01 => false, // external clock: friend at (3,4), we sit right
                    _ => x <= 4,   // not settled yet; geometry as a fallback
                };
                let (tx, ty) = if left_seat { (3, 4) } else { (6, 4) };
                let face = if left_seat {
                    KeypadKey::Right
                } else {
                    KeypadKey::Left
                };
                if verbose && self.ticks % 30 == 0 {
                    println!("  time capsule: at ({x},{y}) heading for ({tx},{ty})");
                }
                if (x, y) == (tx, ty) {
                    if self.ticks % 2 == 0 {
                        self.press(gb, face, 6);
                    } else {
                        self.press(gb, KeypadKey::A, 6);
                    }
                } else {
                    self.step_towards(gb, x, y, tx, ty);
                }
            }
            VerifyStage::Continue => {
                // CONTINUE is the top item and the cursor starts on it, so A
                // through the main menu and the save-info panel.
                //
                // **`wMapGroup` is not the test for "we are in the game".** The
                // save is copied into WRAM *before* the continue panel is
                // dismissed, so the map bytes read 20:1 while the panel is
                // still up — stop pressing A there and the game sits on that
                // panel for ever. Measured: exactly that, and it looked like a
                // corrupt save. The honest test is that the panel is gone.
                let (g, n) = (gb.peek(W_MAP_GROUP), gb.peek(W_MAP_NUMBER));
                let text = screen_text(gb);
                // The test is "no text anywhere on screen", not a word list.
                // A REAL save showed why a word list loses: after CONTINUE,
                // Gold put up a "PLAYER <name>" box, then "The clock's time
                // may be wrong." (its RTC consistency check failing against
                // the emulator-restored clock), then the whole clock-set
                // screen ("Set with the Control Pad.") — three screens in a
                // row containing none of CONTINUE/NEW GAME/BADGES, while the
                // map bytes already read plausible. Each escaped word cost a
                // full run walking at nothing. The overworld renders no font
                // glyphs at all (map tiles are ids below $80, which decode to
                // spaces), so "any letter visible" is exactly "a menu or
                // dialog still owns the joypad".
                let panel = text.chars().any(|c| c.is_ascii_alphabetic());
                if !panel && (1..=27).contains(&g) && (1..=60).contains(&n) {
                    self.settle += 1;
                    if self.settle > 30 {
                        if verbose {
                            println!("  in the overworld on map {g}:{n}");
                        }
                        self.stage = VerifyStage::ToStairs;
                    }
                    return;
                }
                self.settle = 0;
                self.press(gb, KeypadKey::A, 4);
            }
            VerifyStage::ToStairs => {
                // Walk onto `warp_tile` — the bedroom staircase, or the front
                // door for a save standing in New Bark Town. Walking through it
                // performs a real map load — with the destination overridden to
                // Pokémon Center 2F, that is a fully set-up map: objects,
                // camera, scene scripts, everything a continue skips.
                let (g, n) = (gb.peek(W_MAP_GROUP), gb.peek(W_MAP_NUMBER));
                if g == GROUP_POKECENTER_2F && n == MAP_POKECENTER_2F {
                    if verbose {
                        println!("  warped into Pokemon Center 2F");
                    }
                    self.stage = VerifyStage::ToDesk;
                    return;
                }
                // A late dialog swallows every direction key — the clock-set
                // screen a real save's failed RTC check produces lands right
                // here — so while ANY text is visible, press A (which also
                // confirms the clock-set defaults through to the overworld)
                // instead of walking. Never B: measured, B backs out through
                // the continue menu to the title screen and the run is over.
                let text = screen_text(gb);
                if text.chars().any(|c| c.is_ascii_alphabetic()) {
                    self.press(gb, KeypadKey::A, 4);
                    return;
                }
                let (x, y) = (gb.peek(W_X_COORD), gb.peek(W_Y_COORD));
                self.step_towards(gb, x, y, self.warp_tile.0, self.warp_tile.1);
            }
            VerifyStage::ToDesk | VerifyStage::Talk => {
                // **Re-derive everything from the current position every
                // tick.** A stored "I am beside her, now face up" goes stale
                // the moment a press moves the player, and then the bot faces
                // a wall for ever. Walking back is one branch.
                let (x, y) = (gb.peek(W_X_COORD), gb.peek(W_Y_COORD));
                if verbose && self.ticks % 20 == 0 {
                    println!("  at ({x},{y}) facing the desk: {:?}", facing_desk(x, y));
                }
                match facing_desk(x, y) {
                    Some(face) => {
                        self.stage = VerifyStage::Talk;
                        if self.ticks % 2 == 0 {
                            self.press(gb, face, 6);
                        } else {
                            self.press(gb, KeypadKey::A, 6);
                        }
                    }
                    None => {
                        self.stage = VerifyStage::ToDesk;
                        self.step_towards(gb, x, y, START_X, START_Y);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The real thing: Gold on one end of an automatically discovered cable, a Gen 1
// game on the other, trading through the Time Capsule.
// ---------------------------------------------------------------------------

fn run_link_trade(rom: &Path, frames: u32, verbose: bool) {
    use terminalgb::link::peer::LinkPeer;

    let mut gb = Gameboy::new(std::fs::read(rom).expect("rom"), Some(rom.to_path_buf()));
    // Same rule as above: a machine with no APU is not a quiet Game Boy.
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    let sav = rom.with_extension("sav");
    let mut peer = LinkPeer::discover(&gb.rom_title(), Some(&sav));
    let mut bot = VerifyBot::default();
    println!("[gold] {} — cable {:?}", gb.rom_title(), peer.state());

    let mut last_stage = None;
    let mut last_peer = String::new();
    for f in 0..frames {
        bot.act(&mut gb, verbose);
        // The warp override needs instruction granularity; so does the cable.
        // Both are serviced in the same loop.
        gb.begin_frame();
        let mut ticks = 0u32;
        let force = bot.stage == VerifyStage::ToStairs;
        while ticks < terminalgb::gameboy::FRAME_TICKS {
            let t = gb.step_instruction();
            ticks += t;
            if force {
                gb.debug_write(W_NEXT_WARP, PC2F_ARRIVE_WARP);
                gb.debug_write(W_NEXT_WARP + 1, GROUP_POKECENTER_2F);
                gb.debug_write(W_NEXT_WARP + 2, MAP_POKECENTER_2F);
            }
            peer.service(&mut gb, t);
        }
        gb.end_frame();

        if Some(bot.stage) != last_stage {
            println!("[gold][{f:6}] {:?}", bot.stage);
            last_stage = Some(bot.stage);
        }
        let ps = format!("{:?}", peer.state());
        if ps != last_peer {
            println!("[gold][{f:6}] cable {ps}");
            last_peer = ps;
        }
        if verbose && f % 600 == 0 {
            println!(
                "[gold][{f:6}] transfers {} | {}",
                peer.stats().transfers,
                squash(&screen_text(&mut gb))
            );
        }
        if gb.fault().is_some() {
            eprintln!("[gold] FAULT: {:?}", gb.fault());
            break;
        }
    }

    println!(
        "[gold] cable: {} transfers, {} unanswered, {} torn",
        peer.stats().transfers,
        peer.stats().unanswered,
        peer.stats().torn
    );
    report_party(&mut gb);
    if let Some(img) = image::RgbaImage::from_raw(160, 144, gb.image().to_vec()) {
        let _ = img.save("target/gen2-timecapsule/link-trade.png");
        println!("[gold] screenshot: target/gen2-timecapsule/link-trade.png");
    }
    gb.flush_battery();
    peer.close();
}

/// What Gold is holding, and what **Gold's own rule** says about each one.
fn report_party(gb: &mut Gameboy) {
    let n = gb.peek(W_PARTY_COUNT);
    println!("[gold] party of {n}:");
    for i in 0..n.min(6) as u16 {
        let base = W_PARTY_MONS + i * PARTYMON_LEN as u16;
        let species = gb.peek(W_PARTY_SPECIES + i);
        let dv_hi = gb.peek(base + 21);
        let dv_lo = gb.peek(base + 22);
        let (atk, def, spd, spc) = (dv_hi >> 4, dv_hi & 15, dv_lo >> 4, dv_lo & 15);
        let level = gb.peek(base + 31);
        let ot_id = ((gb.peek(base + 6) as u16) << 8) | gb.peek(base + 7) as u16;
        let nick: String = (0..11)
            .map(|c| gb.peek(W_PARTY_MON_NICKS + i * 11 + c))
            .take_while(|&b| b != 0x50)
            .map(decode_char)
            .collect();
        // `CheckShininess`, pokegold `engine/gfx/color.asm`:
        //   SHINY_ATK_MASK = %0010, SHINY_DEF_DV = SHINY_SPD_DV = SHINY_SPC_DV = 10.
        let shiny = atk & 0b0010 != 0 && def == 10 && spd == 10 && spc == 10;
        println!(
            "[gold]   slot {}: {nick:<10} species {species} Lv{level} OT id {ot_id} \
             DVs {atk}/{def}/{spd}/{spc} (${dv_hi:02X}${dv_lo:02X}) -> Gold says {}",
            i + 1,
            if shiny { "SHINY" } else { "not shiny" }
        );
    }
}

// ---------------------------------------------------------------------------
// The whole chain in one process: Gold at the Time Capsule, Blue in its Cable
// Club, joined by `LinkCable` — two real cores, the wire knowing nothing about
// Pokémon. This is the run that proves the shiny prediction end to end.
//
// # Why one process, when `--link-trade` already exists
//
// The cross-process run (`--link-trade`, over `LinkPeer`) got Gold to
// "Welcome to CABLE CLUB" and then lost the trade to *phase*: Blue walks its
// short Cable Club route in ~2,000 frames while Gold is still queuing at the
// receptionist, and the two protocols never overlap. Two separate processes
// cannot see each other's game state, so they cannot wait for each other. One
// process can: Blue's bot is simply held at the desk until Gold's bot reaches
// its own receptionist, and the two link attempts then land inside each
// other's retry windows.
//
// # The timing that makes it work (measured out of `pret/pokegold`)
//
// Gold's receptionist script (`LinkReceptionistScript_TimeCapsule`,
// `maps/Pokecenter2F.asm`) runs `WaitForLinkedFriend` *before* the save
// prompt, and that special (`engine/link/link.asm`) arms the console as an
// externally clocked slave with `SB = USING_INTERNAL_CLOCK ($02)` for
// `wLinkTimeoutFrames = $2FF` = 767 frames — and $02 is exactly the answer
// Gen 1's master handshake is looking for. So the window is ~13 seconds wide,
// Blue's whole receptionist sequence fits inside it, and both sides retry on
// a miss. Holding Blue back also avoids the trap the cross-process run hit:
// a Gen 2 console answers handshake bytes from its overworld serial handler,
// so an ungated Blue completes its handshake against a Gold that is still
// walking, enters the Trade Center alone, and desyncs.
// ---------------------------------------------------------------------------

// --- Gen 1 addresses, from `pret/pokered` (same table as examples/link_two.rs,
// which proved them over a Blue<->Blue trade on this cable) -------------------

/// `wPartyCount` / `wPartySpecies` / `wPartyMons` / `wPartyMonOT` /
/// `wPartyMonNicks`.
const G1_PARTY_COUNT: u16 = 0xD163;
const G1_PARTY_SPECIES: u16 = 0xD164;
const G1_PARTY_MONS: u16 = 0xD16B;
const G1_PARTY_MON_OT: u16 = 0xD273;
const G1_PARTY_MON_NICKS: u16 = 0xD2B5;
/// `wPlayerName`, the first byte of `sMainData` when saved.
const G1_MAIN_DATA_START: u16 = 0xD2F7;
/// `wCurMap`, `wYCoord` / `wXCoord`.
const G1_CUR_MAP: u16 = 0xD35E;
const G1_Y_COORD: u16 = 0xD361;
const G1_X_COORD: u16 = 0xD362;
/// `wEventFlags`.
const G1_EVENT_FLAGS: u16 = 0xD747;
/// `wLinkState` — `LINK_STATE_*` in `constants/serial_constants.asm`.
const G1_LINK_STATE: u16 = 0xD12B;
/// `wNumberOfWarps` / `wWarpEntries`.
const G1_NUMBER_OF_WARPS: u16 = 0xD3AE;
const G1_WARP_ENTRIES: u16 = 0xD3AF;
/// `hSerialConnectionStatus` (`vc_assert hSerialConnectionStatus == $ffaa`).
const G1_H_SERIAL_STATUS: u16 = 0xFFAA;
/// `USING_EXTERNAL_CLOCK` / `USING_INTERNAL_CLOCK`
/// (`constants/serial_constants.asm`) — external is `$01`, internal `$02`.
const G1_USING_EXTERNAL_CLOCK: u8 = 0x01;
const G1_USING_INTERNAL_CLOCK: u8 = 0x02;
/// `EVENT_GOT_POKEDEX` — the Cable Club receptionist refuses anybody without
/// it (`CableClubNPC`, `engine/link/cable_club_npc.asm`).
const G1_EVENT_GOT_POKEDEX: usize = 37;
/// `VIRIDIAN_POKECENTER` and `TRADE_CENTER` (`constants/map_constants.asm`).
const G1_MAP_VIRIDIAN_POKECENTER: u8 = 0x29;
const G1_MAP_TRADE_CENTER: u8 = 0xEF;
/// The link receptionist's desk in the Viridian Pokémon Center.
const G1_RECEPTIONIST: (u8, u8) = (11, 3);
/// Gen 1 `.sav` offsets (`src/mbc/battery.rs`: raw cartridge RAM, no header).
const G1_SAVE_PLAYER_NAME: usize = 0x2598;
const G1_SAVE_MAIN_DATA: usize = 0x25A3;
const G1_SAVE_PARTY_DATA: usize = 0x2F2C;
const G1_SAVE_CHECKSUM: usize = 0x3523;
const G1_MON_STRUCT_LEN: usize = 44;
const G1_PARTY_DATA_LEN: usize = 404;

/// The DVs that make a Gen 1 Pokémon shiny once it reaches Gen 2: attack DV
/// with bit 1 set, defence/speed/special all exactly 10 (`CheckShininess`,
/// pokegold `engine/gfx/color.asm` — read out of the very cartridge this run
/// trades into). The verdict printed on the Blue side is NOT recomputed here;
/// it comes from the trainer plugin's `derived::shiny_if_traded`, the one copy
/// of that rule.
const G1_SHINY_DVS: (u8, u8, u8, u8) = (14, 10, 10, 10);

/// Replace the staged Blue save's party (GLIMMER, the shiny-DV Abra, in slot
/// 1), grant the Pokédex, point the current room's warps at the Viridian
/// Pokémon Center, and fix the checksum. Same recipe `examples/link_two.rs`
/// proved over a Blue<->Blue trade.
fn patch_blue_party(staged_rom: &Path, staged_sav: &Path) {
    use terminalgb_plugin_gen1_link::spec::{PokemonSource, TradeSpec};

    let mut sram = std::fs::read(staged_sav).expect("staged blue save");
    let rom = std::fs::read(staged_rom).expect("staged blue rom");
    let main = |addr: u16| G1_SAVE_MAIN_DATA + (addr - G1_MAIN_DATA_START) as usize;

    let n = sram[main(G1_NUMBER_OF_WARPS)] as usize;
    for i in 0..n.min(8) {
        sram[main(G1_WARP_ENTRIES) + i * 4 + 2] = 0;
        sram[main(G1_WARP_ENTRIES) + i * 4 + 3] = G1_MAP_VIRIDIAN_POKECENTER;
    }
    sram[main(G1_EVENT_FLAGS) + G1_EVENT_GOT_POKEDEX / 8] |=
        1 << (G1_EVENT_GOT_POKEDEX % 8);

    let spec = format!(
        "trainer = RED\n\
         species = Abra\nlevel = 10\nnickname = GLIMMER\not_name = RED\not_id = 4242\n\
         dvs = {}, {}, {}, {}\n\
         moves = Teleport\n\
         species = Rattata\nlevel = 8\nnickname = RATTY\not_name = RED\not_id = 4242\n\
         moves = Tackle, Tail Whip\n",
        G1_SHINY_DVS.0, G1_SHINY_DVS.1, G1_SHINY_DVS.2, G1_SHINY_DVS.3
    );
    let tables = terminalgb_plugin_gen1_link::romdata::RomTables::locate(rom)
        .expect("gen 1 rom tables");
    let parsed = TradeSpec::parse(&spec, "capsule_trade").expect("party spec");
    let party = parsed.party(&tables).expect("build party");
    let image = party.to_wram_image();
    assert_eq!(image.len(), G1_PARTY_DATA_LEN);
    sram[G1_SAVE_PARTY_DATA..G1_SAVE_PARTY_DATA + G1_PARTY_DATA_LEN]
        .copy_from_slice(&image);

    // `CalcCheckSum` (`engine/menus/save.asm`): sum of sMainData, complemented.
    let sum: u8 = sram[G1_SAVE_PLAYER_NAME..G1_SAVE_CHECKSUM]
        .iter()
        .fold(0u8, |a, &b| a.wrapping_add(b));
    sram[G1_SAVE_CHECKSUM] = !sum;
    std::fs::write(staged_sav, &sram).expect("write staged blue save");
    println!(
        "[blue] staged party: GLIMMER the Abra, DVs {}/{}/{}/{} (the shiny-if-traded \
         spread), and RATTY the Rattata to receive into",
        G1_SHINY_DVS.0, G1_SHINY_DVS.1, G1_SHINY_DVS.2, G1_SHINY_DVS.3
    );
}

/// Read what is on a **Gen 1** screen as text. Unlike Gen 2 (where `wTilemap`
/// is authoritative), Gen 1 draws the whole screen through the window layer,
/// so this reads VRAM the way `examples/link_two.rs` does.
fn screen_text_gen1(gb: &mut Gameboy) -> String {
    let lcdc = gb.peek(0xFF40);
    let window_on = lcdc & 0x20 != 0 && gb.peek(0xFF4A) < 144;
    let (map, sy, sx) = if window_on {
        (if lcdc & 0x40 != 0 { 0x9C00u16 } else { 0x9800 }, 0, 0)
    } else {
        (
            if lcdc & 0x08 != 0 { 0x9C00u16 } else { 0x9800 },
            gb.peek(0xFF42) as usize / 8,
            gb.peek(0xFF43) as usize / 8,
        )
    };
    let mut out = String::new();
    for row in 0..18 {
        for col in 0..20 {
            let t = gb.peek(map + (((sy + row) % 32) * 32 + (sx + col) % 32) as u16);
            out.push(terminalgb_plugin_gen1_link::text::decode_char(t).unwrap_or(' '));
        }
        out.push('\n');
    }
    out
}

fn gen1_looks_like_a_map(gb: &mut Gameboy) -> bool {
    let lcdc = gb.peek(0xFF40);
    let window_on = lcdc & 0x20 != 0 && gb.peek(0xFF4A) < 144;
    let map = if window_on {
        if lcdc & 0x40 != 0 {
            0x9C00u16
        } else {
            0x9800
        }
    } else if lcdc & 0x08 != 0 {
        0x9C00
    } else {
        0x9800
    };
    let mut seen = [false; 256];
    let mut distinct = 0;
    for row in 0..18u16 {
        for t in gb.peek_range(map + row * 32, 20) {
            if !seen[t as usize] {
                seen[t as usize] = true;
                distinct += 1;
            }
        }
    }
    distinct > 6
}

struct Gen1Mon {
    species: u8,
    level: u8,
    dvs: (u8, u8, u8, u8),
    ot_id: u16,
    ot_name: String,
    nickname: String,
}

impl Gen1Mon {
    fn describe(&self) -> String {
        // The verdict is the trainer plugin's, not a second copy of the rule.
        let dvs = terminalgb_plugin_trainer::gen1::Dvs::from_bytes(
            (self.dvs.0 << 4) | self.dvs.1,
            (self.dvs.2 << 4) | self.dvs.3,
        );
        let shiny = terminalgb_plugin_trainer::derived::shiny_if_traded(dvs);
        format!(
            "{:<10} species {:#04x} Lv{:<3} DVs {:?} OT {} (id {}){}",
            self.nickname,
            self.species,
            self.level,
            self.dvs,
            self.ot_name,
            self.ot_id,
            if shiny {
                "  <- would be SHINY if traded to Gen 2"
            } else {
                ""
            }
        )
    }
}

fn read_party_gen1(gb: &mut Gameboy) -> Vec<Gen1Mon> {
    let count = gb.peek(G1_PARTY_COUNT) as usize;
    if count == 0 || count > 6 {
        return Vec::new();
    }
    let dec = |gb: &mut Gameboy, base: u16| -> String {
        gb.peek_range(base, 11)
            .into_iter()
            .take_while(|&b| b != 0x50)
            .filter_map(terminalgb_plugin_gen1_link::text::decode_char)
            .collect()
    };
    (0..count)
        .map(|i| {
            let b = gb.peek_range(
                G1_PARTY_MONS + (i * G1_MON_STRUCT_LEN) as u16,
                G1_MON_STRUCT_LEN,
            );
            let (d0, d1) = (b[27], b[28]);
            Gen1Mon {
                species: gb.peek(G1_PARTY_SPECIES + i as u16),
                level: b[33],
                dvs: (d0 >> 4, d0 & 15, d1 >> 4, d1 & 15),
                ot_id: u16::from_be_bytes([b[12], b[13]]),
                ot_name: dec(gb, G1_PARTY_MON_OT + (i * 11) as u16),
                nickname: dec(gb, G1_PARTY_MON_NICKS + (i * 11) as u16),
            }
        })
        .collect()
}

/// The Gen 1 side's bot — the `TradeBot` that completed a Blue<->Blue trade in
/// `examples/link_two.rs`, plus one thing: a `held` flag that stops it
/// *initiating* the receptionist conversation until Gold has reached its own
/// desk. Everything after initiation is left to the games' own retry loops.
#[derive(Default)]
struct Gen1TradeBot {
    hold: Option<(KeypadKey, u32)>,
    cooldown: u32,
    stage: Gen1Stage,
    stuck: u32,
    last_pos: (u8, u8),
    blocked: u32,
    on_trade_option: bool,
    spawn: Option<(u8, u8)>,
    done_frames: u32,
    traded: bool,
    held: bool,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
enum Gen1Stage {
    #[default]
    Continue,
    EnterRoom,
    WalkToReceptionist,
    TalkAndSave,
    LinkMenu,
    InTradeRoom,
    AtMachine,
    Done,
    LeaveTradeScreen,
    Finished,
}

impl Gen1TradeBot {
    fn stage_label(&self) -> &'static str {
        match self.stage {
            Gen1Stage::Continue => "continue",
            Gen1Stage::EnterRoom => "leave-room",
            Gen1Stage::WalkToReceptionist => "walk-to-desk",
            Gen1Stage::TalkAndSave => "receptionist",
            Gen1Stage::LinkMenu => "link-menu",
            Gen1Stage::InTradeRoom => "trade-room",
            Gen1Stage::AtMachine => "at-machine",
            Gen1Stage::Done => "traded",
            Gen1Stage::LeaveTradeScreen => "leaving",
            Gen1Stage::Finished => "finished",
        }
    }

    fn finished(&self) -> bool {
        self.stage == Gen1Stage::Finished
    }

    fn press(&mut self, gb: &mut Gameboy, key: KeypadKey, hold: u32) {
        gb.keydown(key);
        self.hold = Some((key, hold));
    }

    fn step_towards(&mut self, gb: &mut Gameboy, x: u8, y: u8, tx: u8, ty: u8) {
        let moved = (x, y) != self.last_pos;
        self.last_pos = (x, y);
        if moved {
            self.blocked = 0;
        } else {
            self.blocked += 1;
        }
        let prefer_x = (self.blocked / 3) % 2 == 0;
        let dx = match x.cmp(&tx) {
            std::cmp::Ordering::Less => Some(KeypadKey::Right),
            std::cmp::Ordering::Greater => Some(KeypadKey::Left),
            std::cmp::Ordering::Equal => None,
        };
        let dy = match y.cmp(&ty) {
            std::cmp::Ordering::Less => Some(KeypadKey::Down),
            std::cmp::Ordering::Greater => Some(KeypadKey::Up),
            std::cmp::Ordering::Equal => None,
        };
        if let Some(k) = if prefer_x { dx.or(dy) } else { dy.or(dx) } {
            self.press(gb, k, 12);
        }
    }

    fn act(&mut self, gb: &mut Gameboy, verbose: bool) {
        if let Some((key, left)) = self.hold {
            if left <= 1 {
                gb.keyup(key);
                self.hold = None;
                self.cooldown = 6;
            } else {
                self.hold = Some((key, left - 1));
            }
            return;
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return;
        }
        let text = screen_text_gen1(gb);
        let map = gb.peek(G1_CUR_MAP);
        let (x, y) = (gb.peek(G1_X_COORD), gb.peek(G1_Y_COORD));
        let link = gb.peek(G1_LINK_STATE);
        let set = |b: &mut Self, s: Gen1Stage| {
            if b.stage != s {
                if verbose {
                    println!("  [blue] {:?} -> {:?}", b.stage, s);
                }
                b.stage = s;
                b.stuck = 0;
            }
        };

        match self.stage {
            Gen1Stage::Continue => {
                let menu = text.contains("CONTINUE")
                    || text.contains("NEW GAME")
                    || text.contains("OPTION")
                    || text.contains("BADGES");
                let loaded = gb.peek(G1_PARTY_MON_NICKS) != 0
                    && (1..=6).contains(&gb.peek(G1_PARTY_COUNT));
                if menu || !loaded || !gen1_looks_like_a_map(gb) {
                    self.stuck = 0;
                    self.press(gb, KeypadKey::A, 4);
                    return;
                }
                self.stuck += 1;
                if self.stuck > 6 {
                    set(self, Gen1Stage::EnterRoom);
                }
            }
            Gen1Stage::EnterRoom => {
                if map == G1_MAP_VIRIDIAN_POKECENTER {
                    set(self, Gen1Stage::WalkToReceptionist);
                    return;
                }
                let (wy, wx) = (gb.peek(G1_WARP_ENTRIES), gb.peek(G1_WARP_ENTRIES + 1));
                self.step_towards(gb, x, y, wx, wy);
            }
            Gen1Stage::WalkToReceptionist => {
                if map == G1_MAP_TRADE_CENTER {
                    set(self, Gen1Stage::InTradeRoom);
                    return;
                }
                if link != 0 {
                    set(self, Gen1Stage::LinkMenu);
                    return;
                }
                if text.contains("CABLE CLUB") || text.contains("SAVE") {
                    set(self, Gen1Stage::TalkAndSave);
                    return;
                }
                if (x, y) != G1_RECEPTIONIST {
                    self.step_towards(gb, x, y, G1_RECEPTIONIST.0, G1_RECEPTIONIST.1);
                } else if self.held {
                    // Standing at the desk, saying nothing: Gold has not
                    // reached its receptionist yet. Initiating now would win
                    // the handshake against Gold's *overworld* serial handler
                    // and walk into the Trade Center alone — the exact failure
                    // the cross-process run recorded.
                } else {
                    self.stuck += 1;
                    if self.stuck % 2 == 1 {
                        self.press(gb, KeypadKey::Up, 4);
                    } else {
                        self.press(gb, KeypadKey::A, 4);
                    }
                }
            }
            Gen1Stage::TalkAndSave => {
                if link != 0 {
                    set(self, Gen1Stage::LinkMenu);
                    return;
                }
                self.press(gb, KeypadKey::A, 4);
            }
            Gen1Stage::LinkMenu => {
                if map == G1_MAP_TRADE_CENTER {
                    set(self, Gen1Stage::InTradeRoom);
                    return;
                }
                self.press(gb, KeypadKey::A, 4);
            }
            Gen1Stage::InTradeRoom => {
                if link >= 0x02 {
                    set(self, Gen1Stage::AtMachine);
                    return;
                }
                let spawn = *self.spawn.get_or_insert((x, y));
                if (x, y) != spawn {
                    self.step_towards(gb, x, y, spawn.0, spawn.1);
                    return;
                }
                let face = match gb.peek(G1_H_SERIAL_STATUS) {
                    G1_USING_INTERNAL_CLOCK => KeypadKey::Right,
                    G1_USING_EXTERNAL_CLOCK => KeypadKey::Left,
                    _ => return,
                };
                self.stuck += 1;
                if self.stuck % 2 == 0 {
                    self.press(gb, face, 4);
                } else {
                    self.press(gb, KeypadKey::A, 4);
                }
            }
            Gen1Stage::AtMachine => {
                if text.contains("Trade completed") {
                    self.traded = true;
                    set(self, Gen1Stage::Done);
                    return;
                }
                let stats_trade = text.contains("STATS") && text.contains("TRADE");
                if stats_trade && !self.on_trade_option {
                    self.on_trade_option = true;
                    self.press(gb, KeypadKey::Right, 4);
                    return;
                }
                if !stats_trade {
                    self.on_trade_option = false;
                }
                self.press(gb, KeypadKey::A, 4);
            }
            Gen1Stage::Done => {
                self.done_frames += 1;
                if self.done_frames > 240 && text.contains("CANCEL") {
                    set(self, Gen1Stage::LeaveTradeScreen);
                }
            }
            Gen1Stage::LeaveTradeScreen => {
                if link == 0x01 {
                    set(self, Gen1Stage::Finished);
                    return;
                }
                self.stuck += 1;
                if self.stuck <= 8 {
                    self.press(gb, KeypadKey::Down, 4);
                } else {
                    self.press(gb, KeypadKey::A, 4);
                }
            }
            Gen1Stage::Finished => {}
        }
    }
}

/// One PNG with both 160x144 frames side by side, Gold on the left.
fn write_pair(path: &Path, gold: &mut Gameboy, blue: &mut Gameboy) {
    const W: u32 = 160;
    const H: u32 = 144;
    const GUTTER: u32 = 4;
    let mut out = image::RgbaImage::new(W * 2 + GUTTER, H);
    for (i, src) in [gold.image(), blue.image()].iter().enumerate() {
        let x0 = i as u32 * (W + GUTTER);
        for y in 0..H {
            for x in 0..W {
                let o = ((y * W + x) * 4) as usize;
                out.put_pixel(
                    x0 + x,
                    y,
                    image::Rgba([src[o], src[o + 1], src[o + 2], 0xFF]),
                );
            }
        }
    }
    if let Err(e) = out.save(path) {
        eprintln!("screenshot {}: {e}", path.display());
    }
}

fn run_capsule_trade(
    gold_rom: &Path,
    blue_rom: &Path,
    blue_save: Option<&Path>,
    frames: u32,
    shots: Option<&Path>,
    verbose: bool,
) {
    use terminalgb::gameboy::FRAME_TICKS;
    use terminalgb::link::LinkCable;

    // --- stage Blue (copies only; the MBC writes `.sav` beside the ROM) ----
    let bdir = PathBuf::from("target/gen2-timecapsule/blue");
    std::fs::create_dir_all(&bdir).expect("blue scratch dir");
    let bwork = bdir.join(blue_rom.file_name().expect("blue rom filename"));
    std::fs::copy(blue_rom, &bwork).expect("copy blue rom");
    let bsav = bwork.with_extension("sav");
    let _ = std::fs::remove_file(&bsav);
    if let Some(s) = blue_save {
        std::fs::copy(s, &bsav).expect("copy blue save");
    }
    patch_blue_party(&bwork, &bsav);

    let mut gold = Gameboy::new(
        std::fs::read(gold_rom).expect("gold rom"),
        Some(gold_rom.to_path_buf()),
    );
    // Same rule as above: a machine with no APU is not a quiet Game Boy.
    gold.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    let mut blue = Gameboy::new(
        std::fs::read(&bwork).expect("blue rom"),
        Some(bwork.clone()),
    );
    // Same rule as above: a machine with no APU is not a quiet Game Boy.
    blue.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    println!(
        "[capsule] gold: {} | blue: {}",
        gold.rom_title(),
        blue.rom_title()
    );

    // Gold's route out of its starting map depends on where the save stands.
    let gsav = gold_rom.with_extension("sav");
    let sram = std::fs::read(&gsav).expect("staged gold save");
    let (g, n) = (
        sram[save_offset(W_MAP_GROUP)],
        sram[save_offset(W_MAP_NUMBER)],
    );
    let Some(tile) = warp_tile_for(g, n) else {
        eprintln!(
            "[capsule] the Gold save stands on map {g}:{n}, which this runner has \
             no route out of — add its warp tile to `warp_tile_for` (see \
             pokegold's maps/) rather than letting the bot walk at nothing"
        );
        std::process::exit(1);
    };
    let mut gold_bot = VerifyBot {
        warp_tile: tile,
        ..VerifyBot::default()
    };
    let mut blue_bot = Gen1TradeBot {
        held: true,
        ..Gen1TradeBot::default()
    };

    if let Some(dir) = shots {
        std::fs::create_dir_all(dir).expect("shots dir");
    }

    println!("[capsule] parties before:");
    for m in read_party_gen1(&mut blue) {
        println!("[blue]   {}", m.describe());
    }
    // Gold's WRAM is uninitialised noise until the save loads at the continue
    // panel (the first run printed a "party of 93"), so the before-picture is
    // read from the staged save file, whose layout `save_offset` knows.
    let gn = sram[save_offset(W_PARTY_COUNT)];
    println!("[gold] staged save party of {gn} (read from the .sav):");
    for i in 0..gn.min(6) as usize {
        let base = save_offset(W_PARTY_MONS) + i * PARTYMON_LEN;
        let (dv_hi, dv_lo) = (sram[base + 21], sram[base + 22]);
        println!(
            "[gold]   slot {}: species {} Lv{} DVs {}/{}/{}/{} (${dv_hi:02X}${dv_lo:02X})",
            i + 1,
            sram[save_offset(W_PARTY_SPECIES) + i],
            sram[base + 31],
            dv_hi >> 4,
            dv_hi & 15,
            dv_lo >> 4,
            dv_lo & 15,
        );
    }

    let mut cable = LinkCable::new();
    let mut last_labels = (String::new(), String::new());
    let mut shot_n = 0usize;
    let mut settle_after_trade = 0u32;

    for frame in 0..frames {
        gold_bot.act(&mut gold, verbose);
        // The gate: Blue may not initiate until Gold is at its own desk.
        blue_bot.held =
            !matches!(gold_bot.stage, VerifyStage::Talk | VerifyStage::Linked);
        // Once Blue has seen "Trade completed!" it is frozen where it stands:
        // its exit sequence would cancel the whole link session, and Gold is
        // still browsing the received Pokémon's stats for the screenshot.
        if !blue_bot.traded {
            blue_bot.act(&mut blue, verbose);
        }

        // `LinkCable::run_frame` inlined, plus the warp-destination override,
        // which needs instruction granularity (the game reads `wNextWarp` back
        // a handful of instructions after writing it).
        gold.begin_frame();
        blue.begin_frame();
        let force = gold_bot.stage == VerifyStage::ToStairs;
        let (mut tg, mut tb) = (0u32, 0u32);
        while tg < FRAME_TICKS || tb < FRAME_TICKS {
            if tb >= FRAME_TICKS || (tg <= tb && tg < FRAME_TICKS) {
                tg += gold.step_instruction();
                if force {
                    gold.debug_write(W_NEXT_WARP, PC2F_ARRIVE_WARP);
                    gold.debug_write(W_NEXT_WARP + 1, GROUP_POKECENTER_2F);
                    gold.debug_write(W_NEXT_WARP + 2, MAP_POKECENTER_2F);
                }
            } else {
                tb += blue.step_instruction();
            }
            cable.service(&mut gold, &mut blue);
        }
        gold.end_frame();
        blue.end_frame();

        if cable.desynced() {
            eprintln!(
                "[capsule] LINK DESYNCHRONISED at frame {frame}\n  gold: {:?}\n  blue: {:?}",
                gold.fault(),
                blue.fault()
            );
            break;
        }

        let labels = (
            format!(
                "{:?}{}",
                gold_bot.stage,
                if gold_bot.traded { "+traded" } else { "" }
            ),
            blue_bot.stage_label().to_string(),
        );
        if labels != last_labels {
            println!(
                "[{frame:6}] gold:{:<16} blue:{:<14} transfers {}",
                labels.0,
                labels.1,
                cable.stats().transfers
            );
            if let Some(dir) = shots {
                shot_n += 1;
                write_pair(
                    &dir.join(format!("{shot_n:02}-{}-{}.png", labels.0, labels.1)),
                    &mut gold,
                    &mut blue,
                );
            }
            if verbose {
                println!("gold screen:\n{}", indent(&screen_text(&mut gold)));
                println!("blue screen:\n{}", indent(&screen_text_gen1(&mut blue)));
            }
            last_labels = labels;
        }

        if gold_bot.traded && (blue_bot.traded || blue_bot.finished()) {
            settle_after_trade += 1;
            // Let both games finish the post-trade exchange and their own
            // saves (Gen 1 runs `SavePartyAndDexData` right after "Trade
            // completed!"), and give Gold's bot time to open the received
            // Pokémon's stats screen for the closing screenshot.
            let stats_drawn = gold_bot.stats_open && settle_after_trade > 900;
            if stats_drawn || settle_after_trade > 4000 {
                println!(
                    "[capsule] both sides report the trade complete at frame {frame} \
                     (stats screen {})",
                    if gold_bot.stats_open {
                        "open"
                    } else {
                        "never opened"
                    }
                );
                break;
            }
        }
    }

    let stats = cable.stats();
    println!(
        "\n[capsule] cable: {} transfers, {} unanswered, {} torn",
        stats.transfers, stats.unanswered, stats.torn
    );
    if let Some(dir) = shots {
        write_pair(&dir.join("99-final.png"), &mut gold, &mut blue);
    }

    println!("[capsule] parties after:");
    let blue_party = read_party_gen1(&mut blue);
    for m in &blue_party {
        println!("[blue]   {}", m.describe());
    }
    report_party(&mut gold);

    // The claim, checked from Gold's own bytes: a party slot holding the
    // shiny-DV Pokémon that Blue offered, judged by the same DV rule Gold's
    // `CheckShininess` implements.
    let n = gold.peek(W_PARTY_COUNT);
    let arrived = (0..n.min(6) as u16).find(|&i| {
        let base = W_PARTY_MONS + i * PARTYMON_LEN as u16;
        let (dv_hi, dv_lo) = (gold.peek(base + 21), gold.peek(base + 22));
        (dv_hi >> 4) & 0b0010 != 0
            && dv_hi & 15 == 10
            && dv_lo >> 4 == 10
            && dv_lo & 15 == 10
    });

    gold.flush_battery();
    blue.flush_battery();

    match arrived {
        Some(slot) => {
            println!(
                "\n[capsule] VERIFIED: Gold party slot {} holds a Pokémon whose DVs \
                 satisfy Gold's own CheckShininess rule — the Gen 1 shiny \
                 prediction survived a real Time Capsule trade",
                slot + 1
            );
        }
        None => {
            eprintln!(
                "\n[capsule] NOT VERIFIED: no Pokémon in Gold's party carries the \
                 shiny DV pattern — see the party dumps above for what actually \
                 arrived"
            );
            std::process::exit(1);
        }
    }
}
