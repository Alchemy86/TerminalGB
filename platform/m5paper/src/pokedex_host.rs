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

fn main() {
    let mut args = std::env::args().skip(1);
    let (dir, out) = match (args.next(), args.next()) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            eprintln!("usage: pokedex_host <species-1x-dir> <out-archive.bin>");
            eprintln!();
            eprintln!("  <species-1x-dir>  a `pixelgb species` four-shade 1x directory, e.g.");
            eprintln!("                    out/pokemon-blue/sprites/four-shade/1x");
            eprintln!("                    every pokemon-front-NNN-*.png in it is embedded");
            std::process::exit(2);
        }
    };

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
    let mut directory: Vec<(u16, String, usize, usize)> = Vec::new();
    for (number, name, path) in &entries {
        let blob = convert_one(path);
        let offset = payload.len();
        payload.extend_from_slice(&blob);
        directory.push((*number, name.clone(), offset, blob.len()));
    }

    let mut out_bytes = Vec::from(&pokedex::MAGIC[..]);
    out_bytes.extend_from_slice(&(directory.len() as u16).to_le_bytes());
    for (number, name, offset, length) in &directory {
        out_bytes.extend_from_slice(&number.to_le_bytes());
        let mut field = [b' '; pokedex::NAME_LEN];
        field[..name.len()].copy_from_slice(name.as_bytes());
        out_bytes.extend_from_slice(&field);
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
