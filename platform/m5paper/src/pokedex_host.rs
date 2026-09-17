//! Turn a whole `pixelgb species` directory into the archive
//! `src/pokedex.rs` reads: one file, all 151 front sprites, a directory
//! naming each by its own Pokédex number.
//!
//! **The same discipline as `src/sprite_host.rs`, run once per species
//! instead of once.** No game art is committed to this repository — the
//! picture is decompressed from the player's own Pokémon Blue image, by the
//! player, and reaches flash at *build* time:
//!
//! ```sh
//! pixelgb species --rom ~/roms/blue.gb --out /tmp/species
//! TERMINALGB_SPECIES_DIR=/tmp/species/pokemon-blue/sprites/four-shade/1x \
//!   tools/m5paper-build.sh pokedex
//! ```
//!
//! The PNG decode, the alpha-flatten and the shade quantiser are exactly
//! `src/sprite_host.rs`'s — this file does not reimplement any of them, it
//! just calls the same three steps once per `pokemon-front-*.png` in the
//! directory instead of once for a single named file, and writes the results
//! **concatenated with a directory in front of them** rather than as 151
//! separate files: one `include_bytes!`, matching the convention
//! `TERMINALGB_SPRITE` and `TERMINALGB_ROM` already use for the same reason
//! (a memory-mapped flash blob costs no RAM, however many pictures are in it).
//!
//! The archive format is documented once, in `src/pokedex.rs` — this file and
//! that one must agree on it and both cite `ENTRY_SIZE`/`HEADER`/`NAME_LEN`
//! from there rather than each carrying its own copy of the layout.
//!
//! **The second job here is the type and the five base stats, read straight
//! out of the cartridge's own `BaseStats` table — never the Pokédex's own
//! written description, which stays declined.** The table's address and
//! record layout are not derived here; they are
//! `crates/terminalgb-gen1-battle/src/data.rs`'s own `BASE_STATS` /
//! `MEW_BASE_STATS` / `BASE_DATA_SIZE`, copied rather than re-guessed,
//! because that crate already checks them against the battle engine's own
//! test suite. Mew's record lives at a different address, outside the main
//! table — pokered's own `assert_table_length NUM_POKEMON - 1 ; discount
//! Mew` is why, and that crate's module docs have the citation.

#![allow(dead_code)]

#[path = "font.rs"]
mod font;
#[path = "geom.rs"]
mod geom;
#[path = "gbscreen.rs"]
mod gbscreen;
#[path = "../../../testharness/pngdec.rs"]
mod pngdec;
#[path = "sprite.rs"]
mod sprite;
#[path = "pokedex.rs"]
mod pokedex;

use std::path::Path;

/// `BaseStats`: 28-byte records for dex numbers 1..150. Address and record
/// layout copied from `crates/terminalgb-gen1-battle/src/data.rs`'s
/// `BASE_STATS` / `BASE_DATA_SIZE` — see that crate for the citation and its
/// own test suite proving the offsets against real cartridges.
const BASE_STATS: usize = flat(0x0E, 0x43DE);
/// Mew's record, apart from the main table (see the module docs above).
const MEW_BASE_STATS: usize = flat(0x01, 0x425B);
const BASE_DATA_SIZE: usize = 28;

const fn flat(bank: usize, addr: u16) -> usize {
    bank * 0x4000 + (addr as usize & 0x3FFF)
}

/// One species' type(s) and five base stats, straight off the record —
/// `BaseStats`' own field order (dex, hp, attack, defense, speed, special,
/// type1, type2, catch_rate, base_exp; only the first eight bytes matter
/// here).
struct Stats {
    type1: u8,
    type2: u8,
    hp: u8,
    attack: u8,
    defense: u8,
    speed: u8,
    special: u8,
}

/// Reads dex number `dex`'s stats out of `rom`. `dex` is 1..=151; 151 is Mew.
fn read_stats(rom: &[u8], dex: u16) -> Stats {
    let r = if dex == 151 {
        &rom[MEW_BASE_STATS..MEW_BASE_STATS + BASE_DATA_SIZE]
    } else {
        let at = BASE_STATS + BASE_DATA_SIZE * (dex as usize - 1);
        &rom[at..at + BASE_DATA_SIZE]
    };
    Stats {
        hp: r[1],
        attack: r[2],
        defense: r[3],
        speed: r[4],
        special: r[5],
        type1: r[6],
        type2: r[7],
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (dir, out, rom_path) = match (args.next(), args.next(), args.next()) {
        (Some(a), Some(b), Some(c)) => (a, b, c),
        _ => {
            eprintln!("usage: pokedex_host <species-1x-dir> <out-archive.bin> <rom.gb>");
            eprintln!();
            eprintln!("  <species-1x-dir>  a `pixelgb species` four-shade 1x directory, e.g.");
            eprintln!("                    out/pokemon-blue/sprites/four-shade/1x");
            eprintln!("                    every pokemon-front-NNN-*.png in it is embedded");
            eprintln!("  <rom.gb>          the same cartridge, so type and base stats can be");
            eprintln!("                    read straight off it for the caption");
            std::process::exit(2);
        }
    };
    let rom = std::fs::read(&rom_path).unwrap_or_else(|e| {
        eprintln!("cannot read {rom_path}: {e}");
        std::process::exit(1);
    });
    if rom.len() < BASE_STATS + BASE_DATA_SIZE * 150 || rom.len() < MEW_BASE_STATS + BASE_DATA_SIZE {
        eprintln!("{rom_path}: too short to hold BaseStats — is this really Pokémon Red/Blue?");
        std::process::exit(1);
    }
    // Bulbasaur's own record (dex 1: hp 45, atk 49, def 49, spd 45, spc 65)
    // is the same signature `crates/terminalgb-gen1-battle` and
    // `tools/gen1-tables/gen1_tables.py` both check before trusting this
    // address — refuse here too rather than embedding stats from the wrong
    // table into every caption silently.
    let b = &rom[BASE_STATS..BASE_STATS + BASE_DATA_SIZE];
    if (b[0], b[1], b[2], b[3], b[4], b[5]) != (1, 45, 49, 49, 45, 65) {
        eprintln!(
            "{rom_path}: BaseStats is not where it should be — Bulbasaur's own record \
             does not match. Is this a hack, a translation, or a different game?"
        );
        std::process::exit(1);
    }

    let mut entries: Vec<(u16, String, std::path::PathBuf)> = Vec::new();
    let read = std::fs::read_dir(&dir).unwrap_or_else(|e| {
        eprintln!("cannot read {dir}: {e}");
        std::process::exit(1);
    });
    for ent in read {
        let path = ent.expect("readdir entry").path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(rest) = stem.strip_prefix("pokemon-front-") else {
            continue; // back sprites and anything else in the directory
        };
        // "006-charizard" -> number "006", slug "charizard"; "029-nidoran-f"
        // -> slug "nidoran-f", which the replace below turns into "NIDORAN F" —
        // the same `cut -d- -f3` / `cut -d- -f4-` split the single-species
        // build command already made in the shell, just done on every file.
        let Some((num_str, slug)) = rest.split_once('-') else {
            eprintln!("{path:?}: cannot read a species number out of {stem:?}");
            std::process::exit(1);
        };
        let number: u16 = num_str.parse().unwrap_or_else(|_| {
            eprintln!("{path:?}: {num_str:?} is not a Pokédex number");
            std::process::exit(1);
        });
        let name = slug.replace('-', " ").to_uppercase();
        if name.len() > pokedex::NAME_LEN {
            eprintln!(
                "{path:?}: name {name:?} is {} characters, longer than the {}-byte field",
                name.len(),
                pokedex::NAME_LEN
            );
            std::process::exit(1);
        }
        entries.push((number, name, path));
    }
    if entries.is_empty() {
        eprintln!("{dir}: no pokemon-front-*.png in it — is this the four-shade/1x directory?");
        std::process::exit(1);
    }
    entries.sort_by_key(|e| e.0);
    for w in entries.windows(2) {
        if w[0].0 == w[1].0 {
            eprintln!("two files both claim Pokédex number {}", w[0].0);
            std::process::exit(1);
        }
    }

    let mut payload = Vec::new();
    #[allow(clippy::type_complexity)]
    let mut directory: Vec<(u16, String, u8, u8, [u8; 5], usize, usize)> = Vec::new();
    for (number, name, path) in &entries {
        let blob = convert_one(path);
        let offset = payload.len();
        payload.extend_from_slice(&blob);
        let stats = read_stats(&rom, *number);
        let stat_bytes = [stats.hp, stats.attack, stats.defense, stats.speed, stats.special];
        directory.push((
            *number,
            name.clone(),
            stats.type1,
            stats.type2,
            stat_bytes,
            offset,
            blob.len(),
        ));
    }

    let mut out_bytes = Vec::from(&pokedex::MAGIC[..]);
    out_bytes.extend_from_slice(&(directory.len() as u16).to_le_bytes());
    for (number, name, type1, type2, stat_bytes, offset, length) in &directory {
        out_bytes.extend_from_slice(&number.to_le_bytes());
        let mut field = [b' '; pokedex::NAME_LEN];
        field[..name.len()].copy_from_slice(name.as_bytes());
        out_bytes.extend_from_slice(&field);
        out_bytes.push(*type1);
        out_bytes.push(*type2);
        out_bytes.extend_from_slice(stat_bytes);
        out_bytes.extend_from_slice(&(*offset as u32).to_le_bytes());
        out_bytes.extend_from_slice(&(*length as u32).to_le_bytes());
    }
    out_bytes.extend_from_slice(&payload);

    std::fs::write(&out, &out_bytes).unwrap_or_else(|e| {
        eprintln!("cannot write {out}: {e}");
        std::process::exit(1);
    });

    // Prove it reads back before flashing sixteen seconds a picture into
    // finding out it does not: the same parser the firmware runs.
    match pokedex::Archive::parse(&out_bytes) {
        Ok(a) => println!(
            "{out}  {} species, {} bytes ({} of directory, {} of pictures)",
            a.len(),
            out_bytes.len(),
            pokedex::HEADER + directory.len() * pokedex::ENTRY_SIZE,
            payload.len(),
        ),
        Err(e) => {
            eprintln!("wrote {out} but it does not parse back: {e}");
            std::process::exit(1);
        }
    }
}

/// One `pixelgb species` front-sprite PNG to the packed blob
/// `sprite::Sprite::parse` reads — exactly `src/sprite_host.rs`'s conversion,
/// called here instead of once from `main`. See that file's own module docs
/// for why each of these two steps has to happen in this order.
fn convert_one(path: &Path) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("cannot read {path:?}: {e}");
        std::process::exit(1);
    });
    let img = pngdec::decode(&bytes).unwrap_or_else(|e| {
        eprintln!("{path:?}: {e}");
        std::process::exit(1);
    });
    if img.width > u16::MAX as usize || img.height > u16::MAX as usize {
        eprintln!("{path:?}: {}x{} does not fit the header", img.width, img.height);
        std::process::exit(1);
    }

    let flat = img.flatten_onto([255, 255, 255]);
    let shades: Vec<u8> = flat
        .chunks_exact(3)
        .map(|p| gbscreen::shade_of(p[0], p[1], p[2]) as u8)
        .collect();

    // The same sanity check `src/sprite_host.rs` makes on its one file: a
    // sprite decompressed out of a cartridge has a background, and the
    // background is the page's own colour (shade 3, lightest-first). Nothing
    // present there means the wrong PNG reached this path — a colour render
    // or a thumbnail, not a `pixelgb species` front sprite.
    let mut present = [0usize; 4];
    for &s in &shades {
        present[s as usize] += 1;
    }
    if present[3] == 0 {
        eprintln!("{path:?}: no page-coloured pixel at all — is this a sprite?");
        std::process::exit(1);
    }

    let mut out = Vec::from(&sprite::MAGIC[..]);
    out.extend_from_slice(&(img.width as u16).to_le_bytes());
    out.extend_from_slice(&(img.height as u16).to_le_bytes());
    let mut packed = vec![0u8; sprite::Sprite::packed_len(img.width, img.height)];
    for (i, &s) in shades.iter().enumerate() {
        packed[i >> 2] |= (s & 0b11) << (6 - 2 * (i & 3));
    }
    out.extend_from_slice(&packed);
    out
}
