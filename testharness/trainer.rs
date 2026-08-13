//! Trainer-view harness: proves the Gen 1 reader against a real cartridge.
//!
//! Run with `cargo test --release --test trainer`.
//!
//! Three things are checked, in increasing order of how much they prove:
//!
//! 1. **Sprite decompression, structurally** — every Pokémon's front picture in
//!    the cartridge decodes to a square of 1..=7 tiles with the right number of
//!    pixels. A decompressor with a framing bug fails this on some species.
//! 2. **Sprite decompression, against a reference** — with `POKERED_GFX=<dir>`
//!    pointing at a built [`pret/pokered`](https://github.com/pret/pokered)
//!    checkout's `gfx/pokemon/front`, each named species is compared byte for
//!    byte against the 2bpp tiles the disassembly itself produces. That is the
//!    exact oracle: the `.pic` in the ROM was compressed *from* those bytes.
//!    Skipped when the variable is unset, because the reference graphics cannot
//!    be committed here.
//! 3. **Live state** — the ROM is booted, driven into the save, and the party
//!    is read through the plugin exactly as the terminal panel reads it. The
//!    result is printed so a human can compare it with the game, and checked
//!    for self-consistency (HP within maximum, levels in range, every species
//!    resolving to a name and a Pokédex number).
//!
//! **The ROM and its save are copied into `target/` before anything runs.** The
//! MBC writes battery saves next to the ROM path, and this harness presses
//! buttons at a menu — running it against the original file could modify a real
//! save. It never touches the original.
//!
//! With no Red/Blue cartridge present the harness prints why and exits 0: this
//! is a check that needs a game nobody can ship.

use std::path::{Path, PathBuf};

use gameboy::gameboy::Gameboy;
use gameboy::screen::trainer_view as tv;
use gameboy::trainer::{gen1, text, Game, TrainerPlugin};
use gameboy::KeypadKey;

fn main() {
    let Some(rom_path) = find_rom() else {
        println!(
            "trainer: no Pokémon Red/Blue cartridge found — set POKE_ROM=<path> \
             or drop one in ~/roms. Skipping (exit 0)."
        );
        return;
    };
    println!("trainer: using {}", rom_path.display());

    let staged = match stage(&rom_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("trainer: could not stage the ROM into target/: {e}");
            std::process::exit(1);
        }
    };

    let rom = std::fs::read(&staged).expect("staged ROM is readable");
    let mut failures = 0;

    failures += check_sprites(&rom);
    failures += check_against_reference(&rom);
    failures += check_live_state(&staged, &rom);
    failures += check_against_the_games_own_screen(&staged, &rom);

    if failures > 0 {
        eprintln!("\ntrainer: {failures} check(s) FAILED");
        std::process::exit(1);
    }
    println!("\ntrainer: all checks passed");
}

// ---- locating and staging the cartridge ---------------------------------

fn find_rom() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("POKE_ROM") {
        let p = PathBuf::from(p);
        return identify(&p).map(|_| p);
    }
    let mut dirs = vec![PathBuf::from("tests"), PathBuf::from("roms")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("roms"));
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "gb").unwrap_or(false) && identify(&p).is_some()
            {
                return Some(p);
            }
        }
    }
    None
}

/// Read the cartridge header directly, without booting anything.
fn identify(path: &Path) -> Option<Game> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 0x150 {
        return None;
    }
    let title: String = data[0x134..0x144]
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect();
    let checksum = ((data[0x14E] as u16) << 8) | data[0x14F] as u16;
    Game::detect(&title, checksum)
}

/// Copy the cartridge and any battery save into `target/`, so nothing this
/// harness does can reach the originals.
fn stage(rom: &Path) -> std::io::Result<PathBuf> {
    let dir = PathBuf::from("target/trainer-harness");
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(rom.file_name().unwrap_or_default());
    std::fs::copy(rom, &dest)?;
    // Battery saves live beside the ROM (`crate::mbc::battery`): `.sav` is the
    // current format, `.srm`/`.gbsave` are read as legacy. Stage whichever
    // exist so the live check sees the real game, and so nothing this harness
    // does can reach the originals.
    let mut staged_any = false;
    for ext in ["sav", "srm", "gbsave"] {
        let save = rom.with_extension(ext);
        if save.exists() {
            std::fs::copy(&save, dest.with_extension(ext))?;
            staged_any = true;
        }
    }
    if staged_any {
        println!("trainer: staged the battery save alongside it");
    } else {
        println!(
            "trainer: no battery save next to the ROM — the live check will only \
             see a fresh game"
        );
    }
    Ok(dest)
}

// ---- 1. sprite decompression, structurally ------------------------------

fn check_sprites(rom: &[u8]) -> usize {
    let tables = gameboy::trainer::rom::RomTables::new(rom.to_vec());
    let (mut ok, mut skipped, mut bad) = (0, 0, 0);
    let mut widths = [0usize; 8];

    // Internal index numbers run 1..=190; the gaps are the MissingNo. slots,
    // which have no base-stats entry and are expected to be skipped.
    for index in 1u8..=190 {
        if tables.base_stats(index).is_none() {
            skipped += 1;
            continue;
        }
        match tables.front_sprite(index) {
            Some((pixels, w, h)) => {
                let name = tables.species_name(index);
                if w != h || w == 0 || w > 56 || pixels.len() != w * h {
                    eprintln!(
                        "  FAIL {name} (index ${index:02X}): decoded {w}x{h}, \
                         {} pixels",
                        pixels.len()
                    );
                    bad += 1;
                    continue;
                }
                if pixels.iter().any(|&p| p > 3) {
                    eprintln!("  FAIL {name}: pixel outside the 2bpp range");
                    bad += 1;
                    continue;
                }
                // A picture that decodes to a single flat colour is what a
                // subtly-wrong decoder produces, so treat it as a failure
                // rather than a pass.
                if pixels.iter().all(|&p| p == pixels[0]) {
                    eprintln!("  FAIL {name}: decoded to a single flat colour");
                    bad += 1;
                    continue;
                }
                widths[w / 8] += 1;
                ok += 1;
            }
            None => {
                eprintln!(
                    "  FAIL {} (index ${index:02X}): no sprite decoded",
                    tables.species_name(index)
                );
                bad += 1;
            }
        }
    }

    println!("\n[1] sprite decompression, structural");
    println!("    {ok} decoded, {skipped} MissingNo. slots skipped, {bad} failed");
    print!("    sizes:");
    for (tiles, n) in widths.iter().enumerate() {
        if *n > 0 {
            print!(" {tiles}x{tiles}={n}");
        }
    }
    println!();
    bad
}

// ---- 2. sprite decompression, against pokered's own graphics ------------

fn check_against_reference(rom: &[u8]) -> usize {
    let Ok(dir) = std::env::var("POKERED_GFX") else {
        println!("\n[2] reference comparison: skipped (set POKERED_GFX=<dir>)");
        return 0;
    };
    let dir = PathBuf::from(dir);
    let tables = gameboy::trainer::rom::RomTables::new(rom.to_vec());

    // Species whose file names are unambiguous, chosen to cover the whole
    // index range (and therefore every "Pics" bank) and every picture size.
    // The numbers are Gen 1 *internal* index numbers (constants/
    // pokemon_constants.asm), which famously are not the Pokédex order.
    const SAMPLES: &[(u8, &str)] = &[
        (0x01, "rhydon"), // first index, "Pics 1"
        (0x09, "ivysaur"),
        (0x0E, "gengar"),
        (0x15, "mew"),       // special-cased bank $01
        (0x1E, "tangela"),   // last of "Pics 1"
        (0x49, "moltres"),   // last of "Pics 2"
        (0x54, "pikachu"),   // "Pics 3"
        (0x83, "mewtwo"),    // "Pics 4"
        (0x98, "starmie"),   // last of "Pics 4"
        (0x99, "bulbasaur"), // first of "Pics 5"
        (0x9A, "venusaur"),
        (0xB0, "charmander"),
        (0xB4, "charizard"),
        (0xB6, "fossil_kabutops"), // special-cased bank $0B
    ];

    let (mut ok, mut bad, mut missing) = (0, 0, 0);
    for &(index, name) in SAMPLES {
        let path = dir.join(format!("{name}.2bpp"));
        let Ok(expected) = std::fs::read(&path) else {
            missing += 1;
            continue;
        };
        match tables.front_pic(index).map(|p| p.tiles) {
            Some(tiles) if tiles == expected => ok += 1,
            Some(tiles) => {
                eprintln!(
                    "  FAIL {name}: {} bytes decoded, {} expected, {} bytes differ",
                    tiles.len(),
                    expected.len(),
                    tiles
                        .iter()
                        .zip(expected.iter())
                        .filter(|(a, b)| a != b)
                        .count()
                );
                bad += 1;
            }
            None => {
                eprintln!("  FAIL {name}: did not decode");
                bad += 1;
            }
        }
    }
    println!("\n[2] reference comparison against pokered's own 2bpp");
    println!("    {ok} byte-identical, {bad} wrong, {missing} reference files absent");
    bad
}

// ---- 3. live state ------------------------------------------------------

fn check_live_state(rom_path: &Path, rom: &[u8]) -> usize {
    println!("\n[3] live state");
    let mut gb = Gameboy::new(rom.to_vec(), Some(rom_path.to_path_buf()));
    let mut plugin = TrainerPlugin::new(rom.to_vec());
    plugin.set_active(true);
    gb.add_plugin(Box::new(plugin))
        .expect("trainer plugin attaches");

    // Boot, then mash A/START to get past the intro, the title screen and the
    // CONTINUE menu. CONTINUE is the preselected entry when a save exists, so
    // this can never start a new game; and the ROM is a copy regardless.
    let mut frames = 0u64;
    for step in 0..1600u64 {
        if step % 24 == 0 {
            let key = if (step / 24) % 2 == 0 {
                KeypadKey::Start
            } else {
                KeypadKey::A
            };
            gb.keydown(key);
            gb.frame();
            gb.keyup(key);
        } else {
            gb.frame();
        }
        frames += 1;
        if really_in_game(&gb) {
            break;
        }
    }

    let Some(plugin) = gb.plugin::<TrainerPlugin>() else {
        eprintln!("  FAIL the plugin did not attach");
        return 1;
    };
    if plugin.game().is_none() {
        eprintln!(
            "  FAIL the cartridge was not recognised: {:?}",
            plugin.refusal()
        );
        return 1;
    }
    let st = plugin.state();
    // `in_game` alone is not enough: the attract-mode demo writes a player name
    // into WRAM before any save is loaded, so it passes the name check while
    // the party is still uninitialised. A running play clock or a non-empty
    // party is what actually distinguishes a loaded save.
    if !really_in_game(&gb) {
        println!(
            "    after {frames} frames no save had loaded — the cartridge's \
             battery file is empty or absent, so there is no existing game to \
             read. Check [4] builds its own save instead."
        );
        return 0;
    }

    println!("    reached the save after {frames} frames");
    println!(
        "    {} · ID {:05} · ¥{} · {:02}:{:02}:{:02}",
        st.player_name,
        st.player_id,
        st.money,
        st.play_time.0,
        st.play_time.1,
        st.play_time.2
    );
    let flags = st.badge_flags();
    let badges: Vec<&str> = gen1::BADGE_NAMES
        .iter()
        .zip(flags)
        .filter(|(_, got)| *got)
        .map(|(n, _)| *n)
        .collect();
    println!(
        "    badges ({}): {}",
        st.badge_count(),
        if badges.is_empty() {
            "none".to_string()
        } else {
            badges.join(", ")
        }
    );

    let mut bad = 0;
    println!("    party ({}):", st.party.len());
    for m in &st.party {
        let moves: Vec<String> = m
            .moves
            .iter()
            .map(|mv| format!("{} {}/{}", mv.name, mv.pp, mv.pp_max))
            .collect();
        println!(
            "      #{:03} {:<10} {:<10} Lv{:<3} {:>3}/{:<3} HP  {}  \
             atk{} def{} spd{} spc{}  [{}]",
            m.dex,
            m.name,
            m.nickname,
            m.level,
            m.hp,
            m.max_hp,
            m.type_label(),
            m.attack,
            m.defense,
            m.speed,
            m.special,
            moves.join(", ")
        );

        // Self-consistency: these are the invariants a wrong address breaks.
        if m.name.is_empty() {
            eprintln!("      FAIL species ${:02X} resolved to no name", m.species);
            bad += 1;
        }
        if m.dex == 0 || m.dex > 151 {
            eprintln!("      FAIL {} has Pokédex number {}", m.name, m.dex);
            bad += 1;
        }
        if m.level == 0 || m.level > 100 {
            eprintln!("      FAIL {} is level {}", m.name, m.level);
            bad += 1;
        }
        if m.hp > m.max_hp {
            eprintln!("      FAIL {} has {}/{} HP", m.name, m.hp, m.max_hp);
            bad += 1;
        }
        if m.max_hp == 0 || m.max_hp > 999 {
            eprintln!("      FAIL {} has max HP {}", m.name, m.max_hp);
            bad += 1;
        }
        if m.types.0.is_empty() {
            eprintln!("      FAIL {} has no type name", m.name);
            bad += 1;
        }
        if m.moves.is_empty() {
            eprintln!("      FAIL {} knows no moves", m.name);
            bad += 1;
        }
        for mv in &m.moves {
            if mv.name.is_empty() || mv.pp > mv.pp_max {
                eprintln!(
                    "      FAIL {}: move ${:02X} \"{}\" {}/{}",
                    m.name, mv.id, mv.name, mv.pp, mv.pp_max
                );
                bad += 1;
            }
        }
    }
    if st.party.is_empty() {
        println!("      (empty — a save from before the first Pokémon)");
    }
    bad
}

/// A save is genuinely loaded, as opposed to the title-screen demo having left
/// a player name in WRAM.
fn really_in_game(gb: &Gameboy) -> bool {
    gb.plugin::<TrainerPlugin>()
        .map(|p| {
            let st = p.state();
            st.in_game
                && !st.player_name.is_empty()
                && (!st.party.is_empty() || st.play_time != (0, 0, 0) || st.badges != 0)
        })
        .unwrap_or(false)
}

// ---- 4. against the game's own screen ------------------------------------

/// The strongest check available without a human: put a *known* party into the
/// save, let the real game load it, and compare three independent things —
/// what was written, what the plugin reads out of WRAM, and what the game
/// itself prints on screen.
///
/// The last one is the point. `wTileMap` (`$C3A0`, 20x18) stores Gen 1 charmap
/// codes directly, so decoding it reads back exactly the pixels a player would
/// see. If an address in [`gen1`] were wrong, the panel and the game's own
/// party menu would disagree, and this fails.
///
/// The save is *not* hand-built: a hand-built one has no valid map, tileset or
/// sprite state and the game hangs loading it. Instead the harness plays a new
/// game far enough to reach the overworld, drives the START menu to SAVE so the
/// **game** writes a consistent save, and only then edits the party block and
/// fixes the checksum.
fn check_against_the_games_own_screen(rom_path: &Path, rom: &[u8]) -> usize {
    println!("\n[4] against the game's own screen");

    // Start from nothing: a leftover save from check [3] would put the game on
    // a different boot path.
    let save_path = rom_path.with_extension("sav");
    for ext in ["sav", "srm", "gbsave"] {
        let _ = std::fs::remove_file(rom_path.with_extension(ext));
    }
    if let Err(e) = make_a_real_save(rom_path, rom) {
        println!("    could not get the game to write a save ({e}); skipping");
        return 0;
    }
    let Ok(save) = std::fs::read(&save_path) else {
        println!("    the game wrote no save; skipping");
        return 0;
    };
    let expected = Party::sample();
    let save = expected.inject(save);
    if std::fs::write(&save_path, &save).is_err() {
        println!("    could not write the edited save; skipping");
        return 0;
    }

    // Boot into it.
    let mut gb = Gameboy::new(rom.to_vec(), Some(rom_path.to_path_buf()));
    let mut plugin = TrainerPlugin::new(rom.to_vec());
    plugin.set_active(true);
    gb.add_plugin(Box::new(plugin))
        .expect("trainer plugin attaches");
    let mut loaded = false;
    for step in 0..300u32 {
        let key = if step % 2 == 0 {
            KeypadKey::Start
        } else {
            KeypadKey::A
        };
        gb.keydown(key);
        for _ in 0..4 {
            gb.frame();
        }
        gb.keyup(key);
        for _ in 0..20 {
            gb.frame();
        }
        if gb.peek(0xD163) == expected.count {
            loaded = true;
            break;
        }
    }
    if !loaded {
        eprintln!("    FAIL the game never loaded the edited save");
        return 1;
    }

    let mut bad = 0;
    let st = gb
        .plugin::<TrainerPlugin>()
        .expect("plugin attached")
        .state()
        .clone();

    // (a) the plugin's reading against what was written.
    bad += expected.compare(&st);

    // (b) the plugin's reading against the game's own party menu. Get to the
    // overworld, open START, choose POKéMON, and read the tiles.
    for _ in 0..12 {
        press(&mut gb, KeypadKey::A, 40);
    }
    for _ in 0..120 {
        gb.frame();
    }
    press(&mut gb, KeypadKey::Start, 40);
    for _ in 0..8 {
        if gb.peek(0xCC26) == 0 {
            break;
        }
        press(&mut gb, KeypadKey::Up, 12);
    }
    press(&mut gb, KeypadKey::A, 90);

    let rows = read_screen(&mut gb);
    println!("    the game's own party menu:");
    for l in rows.iter().take(6) {
        println!("      |{l}|");
    }
    let screen = rows.join("\n");
    for m in &st.party {
        // The game prints the nickname, the level and cur/max HP. All three
        // come from addresses this plugin also reads, so agreement here is the
        // addresses being right, checked against the game rather than against
        // another copy of our own assumptions.
        for needle in [
            m.nickname.clone(),
            format!("{}", m.level),
            format!("{}/", m.hp),
        ] {
            if !screen.contains(&needle) {
                eprintln!(
                    "      FAIL the game's screen does not show {:?} for {}",
                    needle, m.nickname
                );
                bad += 1;
            }
        }
        if let Some(label) = m.status.label() {
            if !screen.contains(label) {
                eprintln!(
                    "      FAIL the game does not show {label} for {}",
                    m.nickname
                );
                bad += 1;
            }
        }
    }
    if bad == 0 {
        println!("    the panel and the game agree on every party member");
    }

    // (c) print the panel itself, so the change is visible in the log.
    print_panel(&st);
    bad
}

fn press(gb: &mut Gameboy, key: KeypadKey, gap: u32) {
    gb.keydown(key);
    for _ in 0..4 {
        gb.frame();
    }
    gb.keyup(key);
    for _ in 0..gap {
        gb.frame();
    }
}

/// Decode the 20x18 tilemap as the text a player sees.
fn read_screen(gb: &mut Gameboy) -> Vec<String> {
    (0..18)
        .map(|y| {
            let row: Vec<u8> = (0..20).map(|x| gb.peek(0xC3A0 + y * 20 + x)).collect();
            text::decode_screen(&row)
        })
        .collect()
}

/// Play a new game until the overworld, then use the game's own SAVE so the
/// resulting file is internally consistent.
fn make_a_real_save(rom_path: &Path, rom: &[u8]) -> Result<(), String> {
    let mut gb = Gameboy::new(rom.to_vec(), Some(rom_path.to_path_buf()));
    let mut in_game = false;
    for step in 0..400u32 {
        let key = if step % 2 == 0 {
            KeypadKey::Start
        } else {
            KeypadKey::A
        };
        gb.keydown(key);
        for _ in 0..4 {
            gb.frame();
        }
        gb.keyup(key);
        for _ in 0..20 {
            gb.frame();
        }
        // The play clock only advances once the game is really running.
        if gb.peek(0xDA43) > 0 || gb.peek(0xDA44) > 4 {
            in_game = true;
            break;
        }
    }
    if !in_game {
        return Err("never reached the overworld".to_string());
    }
    // START -> SAVE. Find the row the game drew SAVE on rather than assuming an
    // item count, because the menu's entries depend on progress.
    tap(&mut gb, KeypadKey::Start, 40);
    let rows = read_screen(&mut gb);
    let save_row = rows
        .iter()
        .position(|l| l.contains("SAVE"))
        .ok_or("no SAVE entry in the start menu")?;
    // Menu rows are two apart starting at screen row 2.
    let want = ((save_row.saturating_sub(2)) / 2) as u8;
    for _ in 0..8 {
        if gb.peek(0xCC26) == want {
            break;
        }
        tap(&mut gb, KeypadKey::Down, 12);
    }
    tap(&mut gb, KeypadKey::A, 90);
    for _ in 0..4 {
        tap(&mut gb, KeypadKey::A, 150);
    }
    // The battery save is written when the cartridge is dropped.
    drop(gb);
    Ok(())
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

/// A known party written into the save block.
struct Party {
    count: u8,
    mons: Vec<SampleMon>,
    badges: u8,
    money: u32,
    play_time: (u8, u8, u8),
    player_id: u16,
}

struct SampleMon {
    species: u8,
    nickname: &'static str,
    level: u8,
    hp: u16,
    max_hp: u16,
    status: u8,
    types: (u8, u8),
    moves: [u8; 4],
    pp: [u8; 4],
    dvs: (u8, u8),
    stats: [u16; 4],
}

impl Party {
    /// Two Pokémon chosen to exercise the awkward cases: a two-type one and a
    /// single-type one, one healthy and one poisoned, and DVs that are
    /// **exactly the Gen 2 shiny pair** so the derived layer is checked too.
    fn sample() -> Party {
        Party {
            count: 2,
            badges: 0b0000_0101, // Boulder + Thunder
            money: 1234,
            play_time: (12, 34, 56),
            player_id: 0x1234,
            mons: vec![
                SampleMon {
                    species: 0x99, // BULBASAUR
                    nickname: "BULBY",
                    level: 15,
                    hp: 22,
                    max_hp: 41,
                    status: 0,
                    types: (0x16, 0x03), // GRASS / POISON
                    moves: [33, 45, 22, 73],
                    pp: [35, 40, 10, 10],
                    dvs: (0x9A, 0x88),
                    stats: [25, 26, 24, 30],
                },
                SampleMon {
                    species: 0x54, // PIKACHU
                    nickname: "SPARKY",
                    level: 12,
                    hp: 18,
                    max_hp: 34,
                    status: 0x08, // PSN
                    types: (0x17, 0x17),
                    moves: [84, 45, 98, 0],
                    pp: [30, 40, 30, 0],
                    dvs: (0xEA, 0xAA), // the canonical shiny DVs
                    stats: [22, 15, 35, 20],
                },
            ],
        }
    }

    /// Overwrite the party and main-data fields in a save the game itself
    /// wrote, then fix `sMainDataCheckSum` so the game accepts it.
    ///
    /// SRAM offsets are `pokered/ram/sram.asm` resolved through the same symbol
    /// file as everything else: `sPlayerName` `01:A598`, `sMainData` `01:A5A3`
    /// (a copy of WRAM `$D2F7..$DA7F`), `sPartyData` `01:AF2C` (a copy of WRAM
    /// `$D163..$D2F6`), `sMainDataCheckSum` `01:B523`. The checksum is the
    /// complement of the byte sum over `sGameData..sGameDataEnd`
    /// (`CalcCheckSum`, `engine/menus/save.asm`). A `.sav` file is **raw
    /// cartridge RAM with no header, bank 0 first** (`crate::mbc::battery`), so
    /// the save data in SRAM bank 1 starts at file offset `0x2000`.
    fn inject(&self, mut save: Vec<u8>) -> Vec<u8> {
        let off = |addr: u16| 0x2000 + (addr as usize - 0xA000);
        let put = |save: &mut Vec<u8>, addr: u16, bytes: &[u8]| {
            let at = off(addr);
            if at + bytes.len() <= save.len() {
                save[at..at + bytes.len()].copy_from_slice(bytes);
            }
        };
        const MAIN: u16 = 0xA5A3; // == WRAM $D2F7
        const PARTY: u16 = 0xAF2C; // == WRAM $D163
        let main_off = |wram: u16| MAIN + (wram - 0xD2F7);

        let m = self.money;
        put(
            &mut save,
            main_off(0xD347),
            &[
                (((m / 100000) % 10) << 4) as u8 | ((m / 10000) % 10) as u8,
                (((m / 1000) % 10) << 4) as u8 | ((m / 100) % 10) as u8,
                (((m / 10) % 10) << 4) as u8 | (m % 10) as u8,
            ],
        );
        put(&mut save, main_off(0xD356), &[self.badges]);
        put(&mut save, main_off(0xD359), &self.player_id.to_be_bytes());
        put(&mut save, main_off(0xDA41), &[self.play_time.0]);
        put(&mut save, main_off(0xDA43), &[self.play_time.1]);
        put(&mut save, main_off(0xDA44), &[self.play_time.2]);

        put(&mut save, PARTY, &[self.count]);
        let mut species: Vec<u8> = self.mons.iter().map(|m| m.species).collect();
        species.push(0xFF);
        put(&mut save, PARTY + 1, &species);
        for (i, mon) in self.mons.iter().enumerate() {
            let base = PARTY + 8 + (i as u16) * gen1::PARTY_MON_SIZE;
            let mut b = [0u8; 44];
            b[0] = mon.species;
            b[1..3].copy_from_slice(&mon.hp.to_be_bytes());
            b[3] = mon.level;
            b[4] = mon.status;
            b[5] = mon.types.0;
            b[6] = mon.types.1;
            b[7] = 45;
            b[8..12].copy_from_slice(&mon.moves);
            b[27] = mon.dvs.0;
            b[28] = mon.dvs.1;
            b[29..33].copy_from_slice(&mon.pp);
            b[33] = mon.level;
            b[34..36].copy_from_slice(&mon.max_hp.to_be_bytes());
            b[36..38].copy_from_slice(&mon.stats[0].to_be_bytes());
            b[38..40].copy_from_slice(&mon.stats[1].to_be_bytes());
            b[40..42].copy_from_slice(&mon.stats[2].to_be_bytes());
            b[42..44].copy_from_slice(&mon.stats[3].to_be_bytes());
            put(&mut save, base, &b);
            put(
                &mut save,
                PARTY + 272 + (i as u16) * 11,
                &encode_name("AARON"),
            );
            put(
                &mut save,
                PARTY + 338 + (i as u16) * 11,
                &encode_name(mon.nickname),
            );
        }

        let sum: u8 = save[off(0xA598)..off(0xB523)]
            .iter()
            .fold(0u8, |a, b| a.wrapping_add(*b));
        let at = off(0xB523);
        save[at] = !sum;
        save
    }

    fn compare(&self, st: &gameboy::trainer::TrainerState) -> usize {
        let mut bad = 0;
        let mut check = |what: &str, got: String, want: String| {
            if got != want {
                eprintln!("      FAIL {what}: plugin says {got}, save holds {want}");
                bad += 1;
            }
        };
        check(
            "badges",
            format!("{:08b}", st.badges),
            format!("{:08b}", self.badges),
        );
        check("money", st.money.to_string(), self.money.to_string());
        check(
            "play time",
            format!("{:?}", st.play_time),
            format!("{:?}", self.play_time),
        );
        check(
            "trainer id",
            st.player_id.to_string(),
            self.player_id.to_string(),
        );
        check(
            "party size",
            st.party.len().to_string(),
            self.count.to_string(),
        );
        for (got, want) in st.party.iter().zip(self.mons.iter()) {
            check("nickname", got.nickname.clone(), want.nickname.to_string());
            check("level", got.level.to_string(), want.level.to_string());
            check("hp", got.hp.to_string(), want.hp.to_string());
            check("max hp", got.max_hp.to_string(), want.max_hp.to_string());
            check("status", got.status.0.to_string(), want.status.to_string());
            check("attack", got.attack.to_string(), want.stats[0].to_string());
            check(
                "special",
                got.special.to_string(),
                want.stats[3].to_string(),
            );
        }
        // The derived layer, on the Pokémon deliberately given the shiny DVs.
        if let Some(pika) = st.party.get(1) {
            if !pika.would_be_shiny() {
                eprintln!(
                    "      FAIL {} has the canonical shiny DVs $EA/$AA but is not \
                     reported as shiny-if-traded",
                    pika.nickname
                );
                bad += 1;
            }
        }
        if let Some(bulby) = st.party.first() {
            if bulby.would_be_shiny() {
                eprintln!(
                    "      FAIL {} is not shiny but was reported as",
                    bulby.nickname
                );
                bad += 1;
            }
        }
        bad
    }
}

fn encode_name(s: &str) -> [u8; 11] {
    let mut out = [0u8; 11];
    let mut i = 0;
    for c in s.chars() {
        if i >= 10 {
            break;
        }
        out[i] = match c {
            'A'..='Z' => 0x80 + (c as u8 - b'A'),
            ' ' => 0x7F,
            _ => 0x7F,
        };
        i += 1;
    }
    out[i] = 0x50;
    out
}

/// Render the trainer panel to an off-screen terminal and print it, so the log
/// carries a picture of what the change actually produces.
fn print_panel(st: &gameboy::trainer::TrainerState) {
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::widgets::Paragraph;
    use ratatui::Terminal;

    for (title, detail) in [(" compact ", false), (" detail ", true)] {
        let mut term = Terminal::new(TestBackend::new(tv::PANEL_W, 26)).unwrap();
        term.draw(|f| {
            let area = Rect::new(0, 0, tv::PANEL_W, 26);
            // No sprite here: a terminal image is not text and cannot be shown
            // in a text dump. It is drawn by the TUI into `layout().sprite`.
            let l = tv::layout(area, false);
            f.render_widget(tv::header(st, "Pokémon Blue"), l.header);
            let mut lines = if detail {
                st.party.first().map(tv::detail_lines).unwrap_or_default()
            } else {
                tv::party_lines(st, 0)
            };
            lines.extend(tv::battle_lines(st));
            if !detail {
                lines.extend(tv::badge_lines(st));
            }
            f.render_widget(Paragraph::new(lines).block(tv::body_block(title)), l.body);
        })
        .unwrap();
        println!("\n    panel{title}");
        let buf = term.backend().buffer().clone();
        for y in 0..buf.area.height {
            let row: String = (0..buf.area.width)
                .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                .collect();
            println!("      {}", row.trim_end());
        }
    }
}
