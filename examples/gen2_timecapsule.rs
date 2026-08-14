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
    eprintln!("nothing to do: pass --probe or --make-save");
}

// ---------------------------------------------------------------------------
// Probe: what does the intro actually look like?
// ---------------------------------------------------------------------------

fn run_probe(rom: &Path, frames: u32) {
    let mut gb = Gameboy::new(std::fs::read(rom).expect("rom"), Some(rom.to_path_buf()));
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
}

/// `wNextWarp` / `wNextMapGroup` / `wNextMapNumber` — where the warp the player
/// is stepping through will take them. Runtime only (below `wPlayerData`, so
/// not in the save), read by `EnterMapWarp` (`home/map.asm`).
const W_NEXT_WARP: u16 = 0xD043;

/// Which warp of Pokémon Center 2F to arrive at: warp 1 is the staircase at
/// (0, 7) (`maps/Pokecenter2F.asm`).
const PC2F_ARRIVE_WARP: u8 = 1;

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

#[derive(Default)]
struct VerifyBot {
    stage: VerifyStage,
    hold: Option<(KeypadKey, u32)>,
    cooldown: u32,
    ticks: u32,
    settle: u32,
    last_pos: (u8, u8),
    blocked: u32,
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
        match self.stage {
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
                let panel = text.contains("BADGES")
                    || text.contains("CONTINUE")
                    || text.contains("NEW GAME");
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
                // The bedroom's only warp is the staircase at (7, 0)
                // (`maps/PlayersHouse2F.asm`). Walking through it performs a
                // real map load — with the destination overridden to Pokémon
                // Center 2F, that is a fully set-up map: objects, camera,
                // scene scripts, everything a continue skips.
                let (g, n) = (gb.peek(W_MAP_GROUP), gb.peek(W_MAP_NUMBER));
                if g == GROUP_POKECENTER_2F && n == MAP_POKECENTER_2F {
                    if verbose {
                        println!("  warped into Pokemon Center 2F");
                    }
                    self.stage = VerifyStage::ToDesk;
                    return;
                }
                let (x, y) = (gb.peek(W_X_COORD), gb.peek(W_Y_COORD));
                self.step_towards(gb, x, y, 7, 0);
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
