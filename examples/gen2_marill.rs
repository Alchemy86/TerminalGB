//! **A real Marill, out of a real Pokémon Gold cartridge** — its identity, its
//! picture, its battle-entrance reveal and its cry, none of it stored here.
//!
//! ```text
//! cargo run --release --features image,chiptune --example gen2_marill -- \
//!     --rom ~/roms/gold.gbc --save ~/roms/goldsave/gold.sav
//! ```
//!
//! # What this is, and why it is not the Generation 1 tool with the numbers changed
//!
//! [`plugins/gen1-link/src/romdata.rs`](../plugins/gen1-link/src/romdata.rs) is
//! the method this follows: **locate the cartridge's own tables by a
//! distinctive byte signature, cross-check each against a second known entry,
//! and read the data out of them** — so no game data lives in this repository
//! and nothing can drift away from the cartridge in the player's hand.
//!
//! Generation 2 needs the same *method* and almost none of the same *facts*:
//!
//! | | Generation 1 (Red/Blue) | Generation 2 (Gold) |
//! |---|---|---|
//! | species numbering | internal index ≠ Pokédex number, 190 slots with 39 holes | **internal index *is* the Pokédex number**, 251 of them |
//! | base-stat record | 28 bytes, Mew held apart | **32 bytes**, no special case |
//! | picture size | a header byte at the front of the compressed stream | **a nibble pair in the base-stat record** |
//! | picture compression | `pkmncompress` — two delta-coded bitplanes | **a byte-oriented LZ**, a different algorithm entirely |
//! | picture pointers | two pointers inside the base-stat record | **one table, front and back alternating** |
//! | colour | none — a Game Boy game has no per-species palette to have | **two RGB555 colours per species, and two more for its shiny** |
//!
//! So `plugins/trainer/src/sprite.rs` — this repository's Generation 1
//! decompressor — cannot read a Generation 2 picture, and the `romdata.rs`
//! table offsets cannot find a Generation 2 table. Both are rewritten here.
//!
//! # Every address is *read out of the cartridge*
//!
//! [`docs/gen2/README.md`](../docs/gen2/README.md) grades Gen 2 addresses and
//! names the grade this repository did not have: **read out of the
//! cartridge** — located in the ROM image at run time rather than transcribed
//! from a symbol file — calling it "the highest-value piece of unglamorous work
//! this area could gain", because it is the only grade that lets a tool
//! *refuse* a cartridge it does not recognise. Nothing here is transcribed.
//! Four tables are located by signature and then verified:
//!
//! * `BaseData` — by Bulbasaur's record, and verified on Pikachu's.
//! * `PokemonNames` — by `BULBASAUR`, and verified by the table's own
//!   arithmetic landing `MARILL` on 183 and `AZUMARILL` on 184.
//! * `PokemonPalettes` — by Bulbasaur's whole 8-byte record, and verified two
//!   ways: every one of the table's 1004 words must have bit 15 clear because
//!   an RGB555 colour leaves it unused, and two species' colours must be the
//!   colours the emulator's own framebuffer shows with them on screen.
//! * `PokemonPicPointers` — by **the hole Unown leaves in it**. Unown's 26
//!   forms live in their own table, so species 201's pair of entries is
//!   `FF FF FF FF FF FF`; a table whose blank pair sits exactly `2 × 200`
//!   entries in is the right table, at the right alignment, indexed the way we
//!   think it is. Verified by decompressing every species (below).
//! * the picture banks — **derived, not assumed**; see [`PicBanks`].
//!
//! The whole thing is then checked at once, and the check is strong enough to
//! be worth stating: **all 250 front pictures decompress to exactly the size
//! their own base-stat record predicts, and all 250 back pictures to exactly
//! 6×6 tiles.** A mislocated table, a misaligned index or a wrong bank cannot
//! survive 500 exact length agreements.
//!
//! Measured on English Pokémon Gold, SHA-1 `d8b8a3600a465308c9953dfa04f0081c05bdcb94`
//! — the cartridge [`docs/gen2/building-the-disassembly.md`](../docs/gen2/building-the-disassembly.md)
//! records `pret/pokegold` reproducing byte for byte.
//!
//! # The formats are facts; no code and no data is taken from anyone
//!
//! The record layouts and the compression are documented facts, cited by symbol
//! name against the [`pret/pokegold`](https://github.com/pret/pokegold)
//! disassembly — `data/pokemon/base_stats/`, `data/pokemon/names.asm`,
//! `data/pokemon/pic_pointers.asm`, and the decompressor `home/decompress.asm`.
//! Both the decompressor and the table reader below are written from the format
//! as described, and every one of them is checked against the cartridge rather
//! than against a copy of anyone's output.

use terminalgb::gameboy::Gameboy;
use terminalgb::KeypadKey;

use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Generation 2's picture compression
// ---------------------------------------------------------------------------

/// Decompress one Generation 2 graphic (`Decompress`, `home/decompress.asm`).
///
/// A byte-oriented LZ, nothing like Generation 1's bitplane coder. The stream
/// is a run of commands; `$FF` ends it.
///
/// ```text
/// bbbnnnnn          command bbb, length nnnnn + 1
/// 111bbbnn nnnnnnnn command bbb, length nnnnnnnnnn + 1   ("long", bbb == 7)
/// ```
///
/// | command | meaning |
/// |---|---|
/// | 0 `literal` | copy the next `length` bytes from the stream |
/// | 1 `iterate` | one byte, repeated `length` times |
/// | 2 `alternate` | two bytes, alternating, `length` bytes in all |
/// | 3 `zero` | `length` zero bytes |
/// | 4 `repeat` | copy `length` bytes already written |
/// | 5 `flip` | the same, each byte's bits reversed |
/// | 6 `reverse` | the same, read backwards |
///
/// Commands 4–6 take an offset into what has been written: a byte with bit 7
/// set means "`n + 1` bytes back from here", otherwise it and the byte after it
/// are a 14-bit offset from the start. A `repeat` may read bytes it is itself
/// writing, which is how the format expresses a run — so the copy is
/// byte-at-a-time on purpose.
///
/// Returns the bytes and the offset just past the terminator, or `None` for a
/// stream that runs off the end or asks for an offset that does not exist yet.
/// **Cartridge-supplied input must never panic** (`panic = 'abort'` makes a
/// panic process death and the loss of unwritten cartridge RAM — see
/// `AGENTS.md`), so every read is checked and a malformed stream is a `None`.
fn decompress(rom: &[u8], start: usize) -> Option<(Vec<u8>, usize)> {
    /// No Game Boy graphic is anywhere near this large; the cap is what stops a
    /// corrupt stream from being an out-of-memory instead of an error.
    const MAX: usize = 0x10000;
    let mut out: Vec<u8> = Vec::new();
    let mut i = start;
    loop {
        let b = *rom.get(i)?;
        i += 1;
        if b == 0xFF {
            return Some((out, i));
        }
        let (cmd, len) = if b >> 5 == 7 {
            let lo = *rom.get(i)?;
            i += 1;
            ((b >> 2) & 7, ((((b & 3) as usize) << 8) | lo as usize) + 1)
        } else {
            (b >> 5, (b & 0x1F) as usize + 1)
        };
        if out.len() + len > MAX {
            return None;
        }
        match cmd {
            0 => {
                out.extend_from_slice(rom.get(i..i + len)?);
                i += len;
            }
            1 => {
                let v = *rom.get(i)?;
                i += 1;
                out.extend(std::iter::repeat(v).take(len));
            }
            2 => {
                let (a, c) = (*rom.get(i)?, *rom.get(i + 1)?);
                i += 2;
                out.extend((0..len).map(|k| if k % 2 == 0 { a } else { c }));
            }
            3 => out.extend(std::iter::repeat(0u8).take(len)),
            4..=6 => {
                let b1 = *rom.get(i)?;
                i += 1;
                let off = if b1 & 0x80 != 0 {
                    out.len().checked_sub((b1 & 0x7F) as usize + 1)?
                } else {
                    let lo = *rom.get(i)?;
                    i += 1;
                    ((b1 as usize) << 8) | lo as usize
                };
                for k in 0..len {
                    let src = if cmd == 6 {
                        off.checked_sub(k)?
                    } else {
                        off + k
                    };
                    let v = *out.get(src)?;
                    out.push(if cmd == 5 { v.reverse_bits() } else { v });
                }
            }
            // `bbb == 7` after a long command is not a command. A stream that
            // contains one is not a graphic, which is a finding, not a panic.
            _ => return None,
        }
    }
}

// ---------------------------------------------------------------------------
// The cartridge's own tables, located by signature
// ---------------------------------------------------------------------------

/// One base-stat record (`data/pokemon/base_stats/`). 32 bytes in Gen 2, and
/// the fields this tool reads.
const BASE_DATA_ENTRY: usize = 32;
const OFF_HP: usize = 1;
const OFF_TYPE1: usize = 7;
const OFF_TYPE2: usize = 8;
const OFF_CATCH_RATE: usize = 9;
const OFF_BASE_EXP: usize = 10;
const OFF_GENDER: usize = 13;
const OFF_HATCH_CYCLES: usize = 15;
/// `dn <width>, <height>` — the front picture's size in 8×8 tiles. Generation 1
/// put this at the head of the compressed stream; Generation 2 keeps it here,
/// so the picture cannot be decompressed without the base-stat table.
const OFF_PIC_DIMS: usize = 17;
const OFF_GROWTH_RATE: usize = 22;
const OFF_EGG_GROUPS: usize = 23;

/// Species-name records: fixed 10-byte fields, in species order.
const MON_NAME_LENGTH: usize = 10;
/// Generation 2 has 251 species and the internal index *is* the Pokédex number.
const NUM_SPECIES: usize = 251;
/// Unown's 26 forms live in `UnownPicPointers`, so its pair of entries in the
/// main table is blank. That hole is how the table is found.
const UNOWN: usize = 201;
/// Every back picture is 6×6 tiles (`BASE_PIC_SIZE`-independent: the back pic
/// dimensions are not in the base-stat record because they never vary).
const BACK_PIC_TILES: usize = 6 * 6;

/// One `PokemonPalettes` record (`data/pokemon/palettes.asm`): four RGB555
/// words, little-endian — the normal palette's two colours, then the shiny
/// palette's two.
///
/// **A Generation 2 Pokémon has real colour, and it is in the cartridge.**
/// Only two colours per palette are stored because the other two never vary:
/// a Pokémon's four shades are white, colour 1, colour 2, black. Generation 1
/// had nothing of the kind — a Game Boy game has no per-species palette to
/// have — so a tool that renders a Gen 1 sprite in colour is inventing a ramp,
/// and a tool that renders a Gen 2 one in grey is throwing away data the
/// player owns.
const PALETTE_ENTRY: usize = 8;

/// Why a lookup failed. These are user-facing: the wrong cartridge should say
/// so, not produce a corrupt picture.
#[derive(Debug)]
enum RomError {
    NotFound(&'static str),
    Ambiguous(&'static str),
    FailedVerification(String),
}

impl std::fmt::Display for RomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RomError::NotFound(t) => write!(
                f,
                "could not find the '{t}' table in this ROM — this tool needs an \
                 English Pokémon Gold cartridge"
            ),
            RomError::Ambiguous(t) => write!(
                f,
                "the '{t}' signature matched more than once — refusing to guess"
            ),
            RomError::FailedVerification(m) => {
                write!(f, "a located table did not verify: {m}")
            }
        }
    }
}

/// Search `hay` for `needle`, requiring exactly one match. Ambiguity means the
/// signature is too weak; that is a bug to fix, not a coin to flip.
fn find_unique(hay: &[u8], needle: &[u8], what: &'static str) -> Result<usize, RomError> {
    let mut found = None;
    for i in 0..hay.len().saturating_sub(needle.len()) + 1 {
        if &hay[i..i + needle.len()] == needle {
            if found.is_some() {
                return Err(RomError::Ambiguous(what));
            }
            found = Some(i);
        }
    }
    found.ok_or(RomError::NotFound(what))
}

/// Generation 2's character codes for a name. `$80` is `A` and `$A0` is `a`,
/// the same charmap Generation 1 uses (`constants/charmap.asm`).
fn encode_name(s: &str) -> Vec<u8> {
    s.bytes()
        .map(|c| match c {
            b'A'..=b'Z' => 0x80 + (c - b'A'),
            b'a'..=b'z' => 0xA0 + (c - b'a'),
            _ => 0x50,
        })
        .collect()
}

fn decode_name(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|&&c| c != 0x50)
        .map(|&c| match c {
            0x80..=0x99 => (b'A' + (c - 0x80)) as char,
            0xA0..=0xB9 => (b'a' + (c - 0xA0)) as char,
            0xF6..=0xFF => (b'0' + (c - 0xF6)) as char,
            0xE8 | 0xF2 => '.',
            _ => ' ',
        })
        .collect()
}

/// How a picture pointer's bank byte becomes a ROM bank **in this cartridge**.
///
/// `PokemonPicPointers` entries are `bank, address` (three bytes, front then
/// back). The address is a plain `$4000..$7FFF` bank-window address, but the
/// bank byte is *not* the ROM bank for every entry: measured on Gold, entries
/// with bank byte `$12` and `$15..$1E` are the bank, while `$13` and `$14` are
/// `+$0C` and `$1F` is `+$0F`. Generation 2's engine fixes the bank up before
/// it pages it in (`FixPicBank` is the name the Crystal disassembly gives the
/// equivalent routine); **that routine has not been located in this cartridge**,
/// so the correction is not transcribed from anywhere.
///
/// It is *derived* instead, which is better than knowing it. For each bank byte
/// the table uses, every candidate ROM bank is tried and the one that
/// decompresses **every** picture with that bank byte to exactly the size its
/// own base-stat record predicts is taken; anything less than unanimous is an
/// error. A cartridge that lays its pictures out differently — Silver, Crystal,
/// a translation — either derives its own map or is refused.
struct PicBanks {
    /// `bank byte -> ROM bank`, for every bank byte the table actually uses.
    map: [Option<u8>; 256],
}

/// Everything the cartridge knows about one species.
#[derive(Clone, Debug)]
struct SpeciesInfo {
    /// The Pokédex number, which in Generation 2 is also the internal index.
    dex: u8,
    name: String,
    base: [u16; 6],
    type1: u8,
    type2: u8,
    catch_rate: u8,
    base_exp: u8,
    gender: u8,
    hatch_cycles: u8,
    growth_rate: u8,
    egg_groups: u8,
    /// Front picture size, in 8×8 tiles.
    pic_w: usize,
    pic_h: usize,
}

/// A decompressed picture: 2bpp tiles in the Game Boy's own format, and the
/// grid they form.
#[derive(Clone)]
struct Pic {
    tiles: Vec<u8>,
    w: usize,
    h: usize,
}

impl Pic {
    /// Colour index 0..=3 at a pixel.
    ///
    /// Generation 2 stores a picture **column-major**: tile `n` is at column
    /// `n / height`, row `n % height`. Reading it row-major produces a picture
    /// that is plainly wrong rather than subtly wrong, which is the one mercy
    /// in the format.
    fn pixel(&self, x: usize, y: usize) -> u8 {
        let (tx, ty) = (x / 8, y / 8);
        let t = tx * self.h + ty;
        let (lo, hi) = (
            self.tiles[t * 16 + (y % 8) * 2],
            self.tiles[t * 16 + (y % 8) * 2 + 1],
        );
        let bit = 7 - (x % 8);
        ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1)
    }

    fn width(&self) -> usize {
        self.w * 8
    }
    fn height(&self) -> usize {
        self.h * 8
    }
}

/// A Pokémon's four colours, as 8-bit RGB.
///
/// Shade 0 is white and shade 3 is black on every species; shades 1 and 2 are
/// the two the cartridge stores. The 5-bit channels are widened with the
/// Game Boy Color correction this emulator already uses in
/// [`Gpu::setrgb`](../src/gpu/mod.rs) — a *display* curve, taken from Gambatte,
/// not a linear scale — so that a picture rendered here and the same picture on
/// the emulator's own screen are the same colour rather than nearly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Palette([[u8; 3]; 4]);

impl Palette {
    /// The two stored RGB555 words, as the four shades.
    fn from_words(light: u16, dark: u16) -> Palette {
        Palette([
            Palette::rgb8(0x7FFF),
            Palette::rgb8(light),
            Palette::rgb8(dark),
            Palette::rgb8(0x0000),
        ])
    }

    fn rgb8(w: u16) -> [u8; 3] {
        let (r, g, b) = (
            (w & 31) as u32,
            ((w >> 5) & 31) as u32,
            ((w >> 10) & 31) as u32,
        );
        [
            ((r * 13 + g * 2 + b) >> 1) as u8,
            ((g * 3 + b) << 1) as u8,
            ((r * 3 + g * 2 + b * 11) >> 1) as u8,
        ]
    }
}

struct Gen2Rom {
    rom: Vec<u8>,
    base_data: usize,
    names: usize,
    pic_pointers: usize,
    palettes: usize,
    banks: PicBanks,
}

impl Gen2Rom {
    /// Locate every table this tool needs, then verify the lot.
    fn locate(rom: Vec<u8>) -> Result<Gen2Rom, RomError> {
        // `BaseData`, by Bulbasaur's record: dex 1, 45/49/49/45/65/65,
        // GRASS ($16) / POISON ($03), catch rate 45, base exp 64.
        let base_data = find_unique(
            &rom,
            &[1, 45, 49, 49, 45, 65, 65, 0x16, 0x03, 45, 64],
            "BaseData",
        )?;
        // `PokemonNames`, by its first *two* entries. One name alone is not
        // enough — `BULBASAUR` also appears in the Pokédex text — but the
        // 10-byte field padding makes the pair unmistakable: `BULBASAUR` fills
        // its field exactly and needs one terminator, `IVYSAUR` needs three.
        let mut sig = encode_name("BULBASAUR");
        sig.push(0x50);
        sig.extend(encode_name("IVYSAUR"));
        sig.extend([0x50; 3]);
        let names = find_unique(&rom, &sig, "PokemonNames")?;
        let pic_pointers = find_pic_pointers(&rom)?;
        // `PokemonPalettes`, by Bulbasaur's whole 8-byte record — its normal
        // pair and its shiny pair. Eight bytes is a locator, not content, and
        // it is unique in the ROM; four alone are not distinctive enough to
        // risk.
        let palettes = find_unique(
            &rom,
            &[0xEC, 0x2F, 0x5F, 0x19, 0x94, 0x2F, 0x5F, 0x19],
            "PokemonPalettes",
        )?;

        let mut me = Gen2Rom {
            rom,
            base_data,
            names,
            pic_pointers,
            palettes,
            banks: PicBanks { map: [None; 256] },
        };
        me.banks = me.derive_pic_banks()?;
        me.verify()?;
        Ok(me)
    }

    fn base_record(&self, dex: u8) -> &[u8] {
        let o = self.base_data + (dex as usize - 1) * BASE_DATA_ENTRY;
        &self.rom[o..o + BASE_DATA_ENTRY]
    }

    fn species(&self, dex: u8) -> SpeciesInfo {
        let r = self.base_record(dex);
        let n = self.names + (dex as usize - 1) * MON_NAME_LENGTH;
        SpeciesInfo {
            dex,
            name: decode_name(&self.rom[n..n + MON_NAME_LENGTH]),
            base: [
                r[OFF_HP] as u16,
                r[OFF_HP + 1] as u16,
                r[OFF_HP + 2] as u16,
                r[OFF_HP + 3] as u16,
                r[OFF_HP + 4] as u16,
                r[OFF_HP + 5] as u16,
            ],
            type1: r[OFF_TYPE1],
            type2: r[OFF_TYPE2],
            catch_rate: r[OFF_CATCH_RATE],
            base_exp: r[OFF_BASE_EXP],
            gender: r[OFF_GENDER],
            hatch_cycles: r[OFF_HATCH_CYCLES],
            growth_rate: r[OFF_GROWTH_RATE],
            egg_groups: r[OFF_EGG_GROUPS],
            pic_w: (r[OFF_PIC_DIMS] >> 4) as usize,
            pic_h: (r[OFF_PIC_DIMS] & 0xF) as usize,
        }
    }

    /// The raw `(bank byte, address)` of a picture pointer. `back` picks the
    /// second of the species' two entries.
    fn pic_pointer(&self, dex: u8, back: bool) -> Option<(u8, u16)> {
        let e = 2 * (dex as usize - 1) + usize::from(back);
        let o = self.pic_pointers + e * 3;
        let (b, a) = (
            self.rom[o],
            u16::from_le_bytes([self.rom[o + 1], self.rom[o + 2]]),
        );
        (b != 0xFF).then_some((b, a))
    }

    /// Where a `(bank byte, address)` pointer lands in the ROM image, given a
    /// candidate ROM bank.
    fn file_offset(bank: u8, addr: u16) -> usize {
        bank as usize * 0x4000 + (addr as usize - 0x4000)
    }

    /// Decompress a picture, given a ROM bank to read it from.
    fn pic_at(&self, bank: u8, addr: u16, tiles: usize) -> Option<Pic> {
        let off = Gen2Rom::file_offset(bank, addr);
        if off >= self.rom.len() {
            return None;
        }
        let (bytes, _) = decompress(&self.rom, off)?;
        (bytes.len() == tiles * 16).then_some(Pic {
            tiles: bytes,
            w: 0,
            h: 0,
        })
    }

    /// See [`PicBanks`]: derive the bank-byte correction rather than assume it.
    fn derive_pic_banks(&self) -> Result<PicBanks, RomError> {
        let mut map: [Option<u8>; 256] = [None; 256];
        // Every bank byte the table uses, and the species that use it.
        let mut users: Vec<Vec<(u8, u16, usize)>> = vec![Vec::new(); 256];
        for dex in 1..=NUM_SPECIES as u8 {
            let s = self.species(dex);
            for (back, tiles) in [(false, s.pic_w * s.pic_h), (true, BACK_PIC_TILES)] {
                if let Some((b, a)) = self.pic_pointer(dex, back) {
                    users[b as usize].push((b, a, tiles));
                }
            }
        }
        for (byte, list) in users.iter().enumerate() {
            if list.is_empty() {
                continue;
            }
            let mut winners = Vec::new();
            for cand in 0..=0x7Fu8 {
                if list
                    .iter()
                    .all(|&(_, a, tiles)| self.pic_at(cand, a, tiles).is_some())
                {
                    winners.push(cand);
                }
            }
            match winners.as_slice() {
                [one] => map[byte] = Some(*one),
                [] => {
                    return Err(RomError::FailedVerification(format!(
                        "no ROM bank decompresses all {} pictures whose pointer \
                         bank byte is ${byte:02X}",
                        list.len()
                    )))
                }
                many => {
                    return Err(RomError::FailedVerification(format!(
                        "bank byte ${byte:02X} is ambiguous: banks {many:?} all \
                         decompress its {} pictures",
                        list.len()
                    )))
                }
            }
        }
        Ok(PicBanks { map })
    }

    fn pic(&self, dex: u8, back: bool) -> Option<Pic> {
        let s = self.species(dex);
        let (w, h) = if back { (6, 6) } else { (s.pic_w, s.pic_h) };
        let (b, a) = self.pic_pointer(dex, back)?;
        let bank = self.banks.map[b as usize]?;
        let mut p = self.pic_at(bank, a, w * h)?;
        p.w = w;
        p.h = h;
        Some(p)
    }

    /// A species' palette. `shiny` picks the second pair in the record — the
    /// cartridge stores both, which is why a shiny Pokémon needs no separate
    /// picture.
    fn palette(&self, dex: u8, shiny: bool) -> Palette {
        let o =
            self.palettes + (dex as usize - 1) * PALETTE_ENTRY + usize::from(shiny) * 4;
        let w = |i: usize| u16::from_le_bytes([self.rom[o + i], self.rom[o + i + 1]]);
        Palette::from_words(w(0), w(2))
    }

    /// Cross-check every located table against something a misalignment would
    /// break. See the module comment: 500 exact agreements is the check.
    fn verify(&self) -> Result<(), RomError> {
        let fail = |m: String| Err(RomError::FailedVerification(m));
        // A one-record slip in `BaseData` or `PokemonNames` fails here: Pikachu
        // is 25 in both tables and nothing else has its numbers.
        let pika = self.species(25);
        if pika.name != "PIKACHU" || pika.base != [35, 55, 30, 90, 50, 40] {
            return fail(format!("species 25 read back as {pika:?}, not PIKACHU"));
        }
        // The species this tool is for, and its evolution, which is the entry
        // straight after it — so a one-record slip in either direction shows.
        for (dex, want) in [(182, "BELLOSSOM"), (183, "MARILL"), (184, "AZUMARILL")] {
            let got = self.species(dex).name;
            if got != want {
                return fail(format!("species {dex} is '{got}', not '{want}'"));
            }
        }
        // `PokemonPalettes`. Two checks, and the second is the one with teeth.
        //
        // Every one of the 1004 words in the table must have bit 15 clear,
        // because an RGB555 colour leaves it unused. That is a constraint 251
        // records long and it is not something arbitrary bytes satisfy.
        for dex in 1..=NUM_SPECIES {
            let o = self.palettes + (dex - 1) * PALETTE_ENTRY;
            for k in 0..4 {
                let w =
                    u16::from_le_bytes([self.rom[o + 2 * k], self.rom[o + 2 * k + 1]]);
                if w & 0x8000 != 0 {
                    return fail(format!(
                        "palette word {k} of species {dex} is ${w:04X}, which is \
                         not an RGB555 colour"
                    ));
                }
            }
        }
        // And the colours must be the colours the cartridge actually draws.
        // These two were read off the emulator's own framebuffer with Marill
        // and a wild Hoothoot on screen — see `docs/gen2/marill.md` — so this
        // check is against the running machine, not against itself. `--save`
        // repeats it live on every run.
        for (dex, name, want) in [
            (183u8, "MARILL", [[86, 136, 186], [173, 58, 73]]),
            (163u8, "HOOTHOOT", [[165, 90, 80], [40, 68, 100]]),
        ] {
            let got = self.palette(dex, false);
            if [got.0[1], got.0[2]] != want {
                return fail(format!(
                    "{name}'s palette reads {:?}, not the {want:?} the cartridge \
                     draws",
                    [got.0[1], got.0[2]]
                ));
            }
        }

        let mut fronts = 0usize;
        let mut backs = 0usize;
        for dex in 1..=NUM_SPECIES as u8 {
            if dex as usize == UNOWN {
                if self.pic_pointer(dex, false).is_some() {
                    return fail("Unown's picture entry is not blank".into());
                }
                continue;
            }
            fronts += usize::from(self.pic(dex, false).is_some());
            backs += usize::from(self.pic(dex, true).is_some());
        }
        let want = NUM_SPECIES - 1;
        if fronts != want || backs != want {
            return fail(format!(
                "{fronts}/{want} front and {backs}/{want} back pictures \
                 decompressed to the size their own record predicts"
            ));
        }
        Ok(())
    }
}

/// Find `PokemonPicPointers` by the hole Unown leaves in it.
///
/// Every entry is `bank, address-little-endian` with the address in the
/// `$4000..$7FFF` bank window; species 201's two entries are `$FF` filled
/// because Unown's forms live in `UnownPicPointers`. So: find six `$FF` bytes,
/// assume they are Unown's pair, and accept the table only if all 502 entries
/// then look like entries. That checks the base *and* the alignment *and* the
/// front/back interleave in one go.
fn find_pic_pointers(rom: &[u8]) -> Result<usize, RomError> {
    let entries = NUM_SPECIES * 2;
    let unown_entry = 2 * (UNOWN - 1);
    let mut found = None;
    for j in 0..rom.len().saturating_sub(6) {
        if rom[j..j + 6] != [0xFF; 6] {
            continue;
        }
        let Some(base) = j.checked_sub(unown_entry * 3) else {
            continue;
        };
        if base + entries * 3 > rom.len() {
            continue;
        }
        let ok = (0..entries).all(|e| {
            let o = base + e * 3;
            let addr = u16::from_le_bytes([rom[o + 1], rom[o + 2]]);
            if e == unown_entry || e == unown_entry + 1 {
                rom[o] == 0xFF && addr == 0xFFFF
            } else {
                rom[o] < 0x80 && (0x4000..=0x7FFF).contains(&addr)
            }
        });
        if ok {
            if found.is_some() {
                return Err(RomError::Ambiguous("PokemonPicPointers"));
            }
            found = Some(base);
        }
    }
    found.ok_or(RomError::NotFound("PokemonPicPointers"))
}

// ---------------------------------------------------------------------------
// Driving the cartridge
// ---------------------------------------------------------------------------

/// `wTilemap` — 20x18, and a tile id **is** a character code, so the screen
/// reads back as text. Same addresses as `examples/gen2_timecapsule.rs`, which
/// derived them; see `docs/gen2/reading-the-screen.md`.
const W_TILEMAP: u16 = 0xC3A0;
/// `wPlayerName`.
const W_PLAYER_NAME: u16 = 0xD1A3;
/// `wYCoord` / `wXCoord`, the player's tile on the current map.
const W_Y_COORD: u16 = 0xDA02;
const W_X_COORD: u16 = 0xDA03;
/// `wMapGroup` / `wMapNumber`.
const W_MAP_GROUP: u16 = 0xDA00;
const W_MAP_NUMBER: u16 = 0xDA01;
/// `wPartyCount` / `wPartySpecies` / `wPartyMons` / OT names / nicknames.
const W_PARTY_COUNT: u16 = 0xDA22;
const W_PARTY_SPECIES: u16 = 0xDA23;
const W_PARTY_MONS: u16 = 0xDA2A;
const W_PARTY_MON_OTS: u16 = 0xDB4A;
const W_PARTY_MON_NICKS: u16 = 0xDB8C;
/// `PARTYMON_STRUCT_LENGTH`.
const PARTYMON_LEN: usize = 48;
/// `sGameData` = bank 1 `$A009`; the 16-bit checksum covers up to
/// `sGameDataEnd` = `sChecksum` = bank 1 `$AD69`.
const S_GAME_DATA: usize = 0x2000 + 0x009;
const S_GAME_DATA_END: usize = 0x2000 + 0xD69;

/// Map a WRAM address to its offset in the `.sav`. Gold saves three separate
/// WRAM runs into three separate SRAM runs, so this is not one base plus an
/// offset (`docs/gen2/the-save-file.md`).
fn save_offset(addr: u16) -> usize {
    const BANK1: usize = 0x2000;
    match addr {
        0xD1A1..=0xD9ED => BANK1 + 0x009 + (addr - 0xD1A1) as usize,
        0xD9EE..=0xDA21 => BANK1 + 0x856 + (addr - 0xD9EE) as usize,
        0xDA22..=0xDF00 => BANK1 + 0x88A + (addr - 0xDA22) as usize,
        _ => panic!("{addr:#06X} is not in a saved WRAM region"),
    }
}

fn decode_char(t: u8) -> char {
    match t {
        0x80..=0x99 => (b'A' + (t - 0x80)) as char,
        0xA0..=0xB9 => (b'a' + (t - 0xA0)) as char,
        0xF6..=0xFF => (b'0' + (t - 0xF6)) as char,
        0x9C => ':',
        0xE8 | 0xF2 => '.',
        0xF3 => '/',
        0xF4 => ',',
        _ => ' ',
    }
}

fn screen_text(gb: &mut Gameboy) -> String {
    (0..18u16)
        .map(|row| {
            (0..20u16)
                .map(|col| decode_char(gb.peek(W_TILEMAP + row * 20 + col)))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Which screen row the menu cursor is on — Gen 2 draws it as tile `$ED`.
fn cursor_row(gb: &mut Gameboy) -> Option<usize> {
    (0..18usize)
        .find(|&r| (0..20u16).any(|c| gb.peek(W_TILEMAP + (r as u16) * 20 + c) == 0xED))
}

/// An [`AudioPlayer`] that keeps the samples the emulated APU produces, but
/// only while it is **armed**.
///
/// Two things this has to get right, both learned elsewhere in this repository:
///
/// * A machine with **no** player installed has no sound chip at all rather
///   than a quiet one — `Sound::do_cycle` returns immediately and every
///   `$FF10-$FF3F` read answers `$FF`, which real cartridges hang on (see
///   `AGENTS.md`). So the recorder is installed before the first frame and
///   counts its own calls: "the cartridge was silent" and "the APU never ran"
///   are different findings and a capture that cannot tell them apart is
///   worthless.
/// * It must not keep everything. This run walks the player around for as long
///   as it takes to find tall grass, and half an hour of emulated audio is
///   hundreds of megabytes of `f32` pairs. Arming is what makes the clip the
///   length of the thing being recorded rather than the length of the run.
#[derive(Default)]
struct Recorder {
    armed: bool,
    samples: Vec<(f32, f32)>,
    calls: usize,
}

/// The recorder lives behind a handle so the driver can arm and disarm it
/// between frames — that is what turns "a button was pressed on frame N" into
/// "this clip is the cry".
#[derive(Clone, Default)]
struct Tape(std::sync::Arc<std::sync::Mutex<Recorder>>);

impl Tape {
    fn arm(&self) {
        let mut r = self.0.lock().unwrap_or_else(|p| p.into_inner());
        r.samples.clear();
        r.armed = true;
    }
    /// Stop recording and hand back what was recorded.
    fn cut(&self) -> Vec<(f32, f32)> {
        let mut r = self.0.lock().unwrap_or_else(|p| p.into_inner());
        r.armed = false;
        std::mem::take(&mut r.samples)
    }
    fn calls(&self) -> usize {
        self.0.lock().map(|r| r.calls).unwrap_or(0)
    }
}

impl terminalgb::sound::AudioPlayer for Tape {
    fn play(&mut self, left: &[f32], right: &[f32]) {
        let mut r = self.0.lock().unwrap_or_else(|p| p.into_inner());
        r.calls += 1;
        if r.armed {
            r.samples
                .extend(left.iter().copied().zip(right.iter().copied()));
        }
    }
    fn samples_rate(&self) -> u32 {
        // The APU's resampler is retuned to whatever rate the player reports,
        // so pitch is correct at any of them; 44.1 kHz is what a WAV a person
        // will open expects.
        RATE
    }
    fn underflowed(&self) -> bool {
        false
    }
}

/// Sample rate every capture runs at.
const RATE: u32 = 44_100;
/// Frames kept from the status page — five seconds, which is long enough to
/// show that nothing moves after the picture lands.
const STATUS_FRAMES: usize = 300;
/// Frames kept from a wild battle: twelve seconds covers the encounter text,
/// the wild Pokémon's own cry and the send-out.
const BATTLE_FRAMES: usize = 720;

/// Presses a button for a few frames and then waits, because a Game Boy game
/// reads the pad once a frame and a press shorter than its poll is a press that
/// did not happen.
#[derive(Default)]
struct Pad {
    held: Option<(KeypadKey, u32)>,
    gap: u32,
}

impl Pad {
    fn busy(&self) -> bool {
        self.held.is_some() || self.gap > 0
    }
    fn press(&mut self, gb: &mut Gameboy, key: KeypadKey) {
        self.press_for(gb, key, 6)
    }
    /// Hold a key for `frames`. Walking needs a long hold: in the overworld the
    /// first press of a direction the player is not already facing only turns
    /// them, so a short press cycled through four directions turns on the spot
    /// for ever — measured, and the reason this is a parameter.
    fn press_for(&mut self, gb: &mut Gameboy, key: KeypadKey, frames: u32) {
        gb.keydown(key);
        self.held = Some((key, frames));
    }
    fn tick(&mut self, gb: &mut Gameboy) {
        if let Some((key, left)) = self.held {
            if left <= 1 {
                gb.keyup(key);
                self.held = None;
                self.gap = 10;
            } else {
                self.held = Some((key, left - 1));
            }
        } else if self.gap > 0 {
            self.gap -= 1;
        }
    }
}

/// Put a Marill in the player's party, in the save, and fix the checksum.
///
/// The stats are **computed from the cartridge's own base-stat record** by the
/// published Generation 2 formula, not typed in: a party entry whose stats do
/// not follow from its species and level is exactly the kind of thing that
/// looks right in a hex dump and reads wrong in game.
fn put_marill_in_party(sram: &mut [u8], m: &SpeciesInfo, level: u8) {
    let ot: Vec<u8> = {
        let b = save_offset(W_PLAYER_NAME);
        sram[b..b + 11].to_vec()
    };
    let id = {
        let b = save_offset(W_PLAYER_NAME) - 2; // `wPlayerID` precedes the name
        [sram[b], sram[b + 1]]
    };

    let mut p = [0u8; PARTYMON_LEN];
    p[0] = m.dex;
    p[1] = 0; // no held item

    // A Marill knows TACKLE at level 1 and DEFENSE CURL at 3; both are Gen 1
    // move ids, and the moves are not what this demo is about.
    p[2] = 33; // TACKLE
    p[3] = 111; // DEFENSE CURL
    p[6] = id[0];
    p[7] = id[1];
    let exp = experience_for_level(m.growth_rate, level);
    p[8] = (exp >> 16) as u8;
    p[9] = (exp >> 8) as u8;
    p[10] = exp as u8;
    // Perfect DVs: two bytes of attack/defense and speed/special nibbles.
    p[21] = 0xFF;
    p[22] = 0xFF;
    p[23] = 35; // TACKLE PP
    p[24] = 40; // DEFENSE CURL PP
    p[27] = 70; // happiness
    p[31] = level;
    let dv = [15u8, 15, 15, 15];
    let hp = stat(m.base[0], hp_dv(dv), level, true);
    let be = |p: &mut [u8; PARTYMON_LEN], off: usize, v: u16| {
        p[off] = (v >> 8) as u8;
        p[off + 1] = v as u8;
    };
    be(&mut p, 34, hp);
    be(&mut p, 36, hp);
    be(&mut p, 38, stat(m.base[1], dv[0], level, false));
    be(&mut p, 40, stat(m.base[2], dv[1], level, false));
    be(&mut p, 42, stat(m.base[3], dv[2], level, false));
    be(&mut p, 44, stat(m.base[4], dv[3], level, false));
    be(&mut p, 46, stat(m.base[5], dv[3], level, false));

    sram[save_offset(W_PARTY_COUNT)] = 1;
    sram[save_offset(W_PARTY_SPECIES)] = m.dex;
    sram[save_offset(W_PARTY_SPECIES) + 1] = 0xFF; // `wPartyEnd`
    let o = save_offset(W_PARTY_MONS);
    sram[o..o + PARTYMON_LEN].copy_from_slice(&p);
    let nick = save_offset(W_PARTY_MON_NICKS);
    let mut n = encode_name(&m.name);
    n.resize(11, 0x50);
    sram[nick..nick + 11].copy_from_slice(&n);
    let ots = save_offset(W_PARTY_MON_OTS);
    sram[ots..ots + 11].copy_from_slice(&ot);

    // `SaveChecksum` / `Checksum` (`engine/menus/save.asm`): a 16-bit sum of
    // every byte of `sGameData`, little-endian. A stale one is not a soft
    // failure — the game rejects the save and starts a new one.
    let sum: u16 = sram[S_GAME_DATA..S_GAME_DATA_END]
        .iter()
        .fold(0u16, |a, &b| a.wrapping_add(b as u16));
    sram[S_GAME_DATA_END] = sum as u8;
    sram[S_GAME_DATA_END + 1] = (sum >> 8) as u8;
}

/// The HP DV is assembled from the low bit of each of the other four, most
/// significant first — the same rule in both generations.
fn hp_dv(dv: [u8; 4]) -> u8 {
    ((dv[0] & 1) << 3) | ((dv[1] & 1) << 2) | ((dv[2] & 1) << 1) | (dv[3] & 1)
}

/// `((base + DV) * 2) * level / 100 + level + 10` for HP, `+ 5` otherwise,
/// with stat experience zero ([Bulbapedia, "Stat"](https://bulbapedia.bulbagarden.net/wiki/Stat)).
fn stat(base: u16, dv: u8, level: u8, is_hp: bool) -> u16 {
    let core = (base as u32 + dv as u32) * 2 * level as u32 / 100;
    let v = if is_hp {
        core + level as u32 + 10
    } else {
        core + 5
    };
    v.min(999) as u16
}

/// Total experience to *be* at `level`. Marill's growth rate is 4 (fast).
fn experience_for_level(growth_rate: u8, level: u8) -> u32 {
    let n = level as i64;
    let e: i64 = match growth_rate {
        1 | 4 => 4 * n * n * n / 5,
        2 | 5 => 5 * n * n * n / 4,
        3 => (6 * n * n * n) / 5 - 15 * n * n + 100 * n - 140,
        _ => n * n * n,
    };
    e.clamp(0, 0x00FF_FFFF) as u32
}

fn main() {
    let mut rom: Option<PathBuf> = None;
    let mut save: Option<PathBuf> = None;
    let mut out = PathBuf::from("target/gen2-marill");
    let mut level = 10u8;
    let mut verbose = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rom" => rom = args.next().map(PathBuf::from),
            "--save" => save = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from).unwrap_or(out),
            "--level" => {
                level = args.next().and_then(|s| s.parse().ok()).unwrap_or(level)
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
            "usage: gen2_marill --rom <gold.gbc> [--save <gold.sav>] \
             [--out <dir>] [--level N] [--verbose]"
        );
        std::process::exit(2);
    };
    std::fs::create_dir_all(&out).expect("output directory");

    let bytes = std::fs::read(&rom).expect("read the cartridge");
    let cart = match Gen2Rom::locate(bytes) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!(
        "tables located: BaseData ${:06X}  PokemonNames ${:06X}  \
         PokemonPicPointers ${:06X}",
        cart.base_data, cart.names, cart.pic_pointers
    );
    let banks: Vec<String> = (0..256)
        .filter_map(|i| cart.banks.map[i].map(|b| format!("${i:02X}->${b:02X}")))
        .collect();
    println!("picture banks derived: {}", banks.join(" "));
    println!(
        "verified: every one of the {} species' front picture decompressed to \
         the size its own base-stat record predicts, and every back picture to \
         6x6 tiles",
        NUM_SPECIES - 1
    );

    let marill = cart.species(183);
    println!(
        "#{} {} — base {:?}, type ${:02X}/${:02X}, catch {}, base exp {}, \
         growth {}, egg groups ${:02X}, gender ${:02X}, {} hatch cycles, \
         front picture {}x{} tiles",
        marill.dex,
        marill.name,
        marill.base,
        marill.type1,
        marill.type2,
        marill.catch_rate,
        marill.base_exp,
        marill.growth_rate,
        marill.egg_groups,
        marill.gender,
        marill.hatch_cycles,
        marill.pic_w,
        marill.pic_h
    );
    let front = cart.pic(183, false).expect("Marill's front picture");
    let back = cart.pic(183, true).expect("Marill's back picture");
    let pal = cart.palette(183, false);
    let shiny = cart.palette(183, true);
    println!(
        "palette: normal {:?} / {:?}, shiny {:?} / {:?} (shade 0 is white and \
         shade 3 black on every species)",
        pal.0[1], pal.0[2], shiny.0[1], shiny.0[2]
    );
    for y in 0..front.height() {
        let row: String = (0..front.width())
            .map(|x| [' ', '.', ':', '@'][front.pixel(x, y) as usize])
            .collect();
        println!("{row}");
    }
    save_pic_png(&out.join("marill-front.png"), &front, pal, 6);
    save_pic_png(&out.join("marill-back.png"), &back, pal, 6);
    save_pic_png(&out.join("marill-front-shiny.png"), &front, shiny, 6);
    println!(
        "wrote {}, {} and {}",
        out.join("marill-front.png").display(),
        out.join("marill-back.png").display(),
        out.join("marill-front-shiny.png").display()
    );

    // The entrance, replayed off the cartridge. See [`ENTER_MON_BACK`].
    let full_back = ENTER_MON_BACK[2].1;
    for (i, (keep, at)) in ENTER_MON_BACK.iter().enumerate() {
        let f = entrance_frame(&back, keep, *at, full_back);
        save_pic_png(&out.join(format!("entrance-back-{i}.png")), &f, pal, 6);
    }
    let full_front = ENTER_MON_FRONT[2].1;
    for (i, (keep, at)) in ENTER_MON_FRONT.iter().enumerate() {
        let f = entrance_frame(&front, keep, *at, full_front);
        save_pic_png(&out.join(format!("entrance-front-{i}.png")), &f, pal, 6);
    }
    println!(
        "wrote the entrance: {} back-picture steps ({}) and {} front-picture \
         steps ({}) to {}",
        ENTER_MON_BACK.len(),
        ENTER_MON_BACK
            .iter()
            .map(|(k, _)| format!("{}x{}", k.len(), k.len()))
            .collect::<Vec<_>>()
            .join(" -> "),
        ENTER_MON_FRONT.len(),
        ENTER_MON_FRONT
            .iter()
            .map(|(k, _)| format!("{}x{}", k.len(), k.len()))
            .collect::<Vec<_>>()
            .join(" -> "),
        out.join("entrance-*.png").display()
    );

    let hp = stat(marill.base[0], 15, level, true);
    for (name, bytes) in [
        ("marill.pwpet1", pwpet(&marill, &front, level, hp, None)),
        (
            "marill.pwpet2",
            pwpet(&marill, &front, level, hp, Some(&cart)),
        ),
    ] {
        let path = out.join(name);
        std::fs::write(&path, &bytes).expect("write the pack");
        println!("wrote {} ({} bytes)", path.display(), bytes.len());
    }

    let Some(save) = save else {
        println!("no --save given, so the cartridge was not driven");
        return;
    };
    drive(&rom, &save, &out, &cart, &marill, level, verbose);
}

// ---------------------------------------------------------------------------
// The entrance, replayed off the cartridge
// ---------------------------------------------------------------------------

/// The Pokémon's entrance, exactly as the cartridge performs it.
///
/// This is `BattleBGEffect_EnterMon` in
/// [`engine/battle_anims/bg_effects.asm`](https://github.com/pret/pokegold/blob/master/engine/battle_anims/bg_effects.asm),
/// reached from `ANIM_SEND_OUT_MON` in
/// [`data/moves/animations.asm`](https://github.com/pret/pokegold/blob/master/data/moves/animations.asm),
/// which `SendOutPlayerMon` plays immediately before `PlayStereoCry` — the cry
/// and the entrance are one moment in the engine, not two.
///
/// **It is not a scale. It is a decimation, and that is why it reads as a
/// squash.** The picture's tiles are in VRAM once, at their own size, and the
/// effect fills the background map with *fewer of them*, throwing away middle
/// columns and rows and keeping the outer ones. The engine's own tables say
/// which:
///
/// | step | tiles | columns and rows kept | at tilemap |
/// |---|---|---|---|
/// | 1 | 2×2 | `0, 5` | `(4, 10)` |
/// | 2 | 4×4 | `0, 2, 3, 5` | `(3, 8)` |
/// | 3 | 6×6 | all six | `(2, 6)` |
///
/// The box's bottom edge stays on tilemap row 11 throughout, so the Pokémon
/// grows upward out of the ball rather than outward from its middle. Nothing
/// is resampled, so no pixel is ever a colour the artist did not draw.
///
/// **What it looks like is species-dependent even though the effect is not.**
/// Marill is wide and low, so cutting the middle columns takes more width from
/// it than cutting the middle rows takes height — which is exactly the
/// "squished thinner and taller, then released" a player sees. A tall, narrow
/// Pokémon put through the identical three steps would look squashed the other
/// way.
///
/// **Verified against the cartridge**, not argued: with `--save`, each step is
/// rendered from the extracted picture and compared pixel for pixel against the
/// frames the emulator drew. Step 3 matches 1273 of 1273 inked pixels exactly;
/// steps 1 and 2 match 36 of 43 and 432 of 441, the difference being the
/// Poké Ball's own poof sprite drawn over the top of them.
///
/// There is nothing per-species in any of it, and nothing per-species in the
/// cartridge to put there: every front picture in Gold decompresses to exactly
/// the tiles its own dimensions call for, with nothing left over (the check
/// [`Gen2Rom::verify`] runs). The same measurement on Crystal finds 18 to 25
/// *extra* tiles per species, because Crystal is where animated front sprites
/// arrive.
const ENTER_MON_BACK: [(&[usize], (usize, usize)); 3] = [
    (&[0, 5], (4, 10)),
    (&[0, 2, 3, 5], (3, 8)),
    (&[0, 1, 2, 3, 4, 5], (2, 6)),
];

/// The same effect on the **front** picture, which is 7×7 tiles in battle.
///
/// `.EnemyData` in the same routine, with `.ThreeByThree` / `.FiveByFive` /
/// `.SevenBySeven` standing in for the back picture's three patterns. It is the
/// one a face-on demo wants — and, unlike [`ENTER_MON_BACK`], **it is
/// transcribed rather than verified here**: the wild Pokémon is already on
/// screen by the time this tool starts recording, so no frame of an enemy
/// entrance was captured to check it against. Capturing the encounter
/// transition would close that.
const ENTER_MON_FRONT: [(&[usize], (usize, usize)); 3] = [
    (&[0, 3, 6], (14, 4)),
    (&[0, 1, 3, 5, 6], (13, 2)),
    (&[0, 1, 2, 3, 4, 5, 6], (12, 0)),
];

/// One step of the entrance: the tiles the step keeps, drawn into a box the
/// size of the last step, sitting where the step's own tilemap coordinate puts
/// it relative to the last one.
fn entrance_frame(
    pic: &Pic,
    keep: &[usize],
    at: (usize, usize),
    full: (usize, usize),
) -> Pic {
    let side = pic.w.max(pic.h);
    let mut out = Pic {
        tiles: vec![0u8; side * side * 16],
        w: side,
        h: side,
    };
    // The step's box is placed by the engine's own coordinate; the difference
    // from the full-size step's coordinate is where it sits inside this frame.
    let (ox, oy) = ((at.0 - full.0) * 8, (at.1 - full.1) * 8);
    for (r, &rr) in keep.iter().enumerate() {
        for (c, &cc) in keep.iter().enumerate() {
            for y in 0..8 {
                for x in 0..8 {
                    if cc >= pic.w || rr >= pic.h {
                        continue;
                    }
                    let v = pic.pixel(cc * 8 + x, rr * 8 + y);
                    set_pixel(&mut out, ox + c * 8 + x, oy + r * 8 + y, v);
                }
            }
        }
    }
    out
}

/// Write one pixel into a [`Pic`]'s column-major 2bpp tiles.
fn set_pixel(pic: &mut Pic, x: usize, y: usize, v: u8) {
    if x >= pic.width() || y >= pic.height() {
        return;
    }
    let t = (x / 8) * pic.h + y / 8;
    let bit = 7 - (x % 8);
    let (lo, hi) = (t * 16 + (y % 8) * 2, t * 16 + (y % 8) * 2 + 1);
    pic.tiles[lo] = (pic.tiles[lo] & !(1 << bit)) | ((v & 1) << bit);
    pic.tiles[hi] = (pic.tiles[hi] & !(1 << bit)) | (((v >> 1) & 1) << bit);
}

// ---------------------------------------------------------------------------
// Handing the Pokémon to the Pokéwalker
// ---------------------------------------------------------------------------

/// The `PWPET1` pack the Pokéwalker pet software loads.
///
/// That project keeps **no artwork of its own**: a pet arrives as this pack,
/// lifted out of the player's cartridge at run time, and its Generation 1
/// producer leans on this repository's Generation 1 decompressor. This is the
/// Generation 2 half of the same arrangement, and it is written here because
/// this is where the Generation 2 decompressor now lives.
///
/// ```text
/// offset  size  field
///      0     8  magic "PWPET1\0\0"
///      8     2  national dex number, little-endian
///     10     1  level
///     11     1  padding
///     12     2  current HP, little-endian
///     14     2  max HP, little-endian
///     16    16  species name, ASCII, NUL-padded
///     32   768  large sprite frame 1   (64x48, 2 bpp planar)
///    800   768  large sprite frame 2
///   1568   192  small sprite frame 1   (32x24, 2 bpp planar)
///   1760   192  small sprite frame 2
/// ```
///
/// **Frame 2 is frame 1 nudged down one pixel**, which is the convention the
/// Generation 1 producer already uses and says plainly is not what a shipping
/// implementation would do. It is the right thing here for a reason Generation
/// 1's tool could only assume and this one has *measured*: Gold holds no
/// second frame for any species (see [`ENTRANCE_STEPS`]), so there is no real
/// frame 2 to put there and making one up would be worse.
const PWPET1_LARGE: (usize, usize) = (64, 48);
const PWPET1_SMALL: (usize, usize) = (32, 24);

fn pwpet(
    m: &SpeciesInfo,
    front: &Pic,
    level: u8,
    hp: u16,
    colour: Option<&Gen2Rom>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(1960);
    out.extend_from_slice(if colour.is_some() {
        b"PWPET2\0\0"
    } else {
        b"PWPET1\0\0"
    });
    out.extend_from_slice(&(m.dex as u16).to_le_bytes());
    out.push(level);
    out.push(0);
    out.extend_from_slice(&hp.to_le_bytes());
    out.extend_from_slice(&hp.to_le_bytes());
    let mut name = m.name.as_bytes().to_vec();
    name.resize(16, 0);
    out.extend_from_slice(&name);
    for (i, (w, h)) in [PWPET1_LARGE, PWPET1_LARGE, PWPET1_SMALL, PWPET1_SMALL]
        .into_iter()
        .enumerate()
    {
        out.extend(planar(front, w, h, i % 2));
    }
    // `PWPET2` is `PWPET1` with the cartridge's own colours on the end: the
    // four RGB555 words of the species' record, normal pair then shiny pair,
    // exactly as `PokemonPalettes` stores them. A new magic rather than a
    // longer `PWPET1`, because the reader this is for checks the length
    // exactly and a silently longer pack would be a pack it refuses.
    if let Some(cart) = colour {
        let o = cart.palettes + (m.dex as usize - 1) * PALETTE_ENTRY;
        out.extend_from_slice(&cart.rom[o..o + PALETTE_ENTRY]);
    }
    out
}

/// The picture, boxed into `w`x`h` and packed the way the Pokéwalker panel
/// decodes it: byte pairs of (upper plane, lower plane) carrying eight
/// *vertically* stacked pixels, marching along x before stepping down eight
/// rows. `dy` nudges the picture down, which is how frame 2 is made.
fn planar(pic: &Pic, w: usize, h: usize, dy: usize) -> Vec<u8> {
    // Scale down only when the picture does not fit; never scale up, because
    // a whole-number magnification of somebody's sprite is the sprite.
    let shrink = 1 + (pic.width().max(pic.height())).saturating_sub(1) / w.min(h);
    let (sw, sh) = (pic.width() / shrink, pic.height() / shrink);
    let (ox, oy) = ((w - sw.min(w)) / 2, (h - sh.min(h)) / 2 + dy);
    let mut data = vec![0u8; w * h * 2 / 8];
    for y in 0..h {
        for x in 0..w {
            if x < ox || y < oy || x >= ox + sw || y >= oy + sh {
                continue;
            }
            let v = pic.pixel((x - ox) * shrink, (y - oy) * shrink);
            let i = ((y / 8) * w + x) * 2;
            let bit = y % 8;
            data[i] |= ((v >> 1) & 1) << bit;
            data[i + 1] |= (v & 1) << bit;
        }
    }
    data
}

/// The extent of whatever is in the lower-left quarter of a battle screen: the
/// bounding box of every pixel that is not the quarter's most common colour,
/// which in a Gen 2 battle is the ground the Pokémon stands on.
///
/// "Whatever is in" is deliberate. During a send-out that includes the Poké
/// Ball's poof sprite, which is bigger than the Pokémon to begin with — so this
/// measures the event, not the Pokémon. [`check_entrance`] measures the
/// Pokémon.
fn sprite_box(f: &[u8]) -> Option<(usize, usize, usize, usize)> {
    let px = |x: usize, y: usize| {
        let i = (y * 160 + x) * 4;
        [f[i], f[i + 1], f[i + 2]]
    };
    let mut counts: std::collections::HashMap<[u8; 3], usize> =
        std::collections::HashMap::new();
    for y in 48..96 {
        for x in 0..80 {
            *counts.entry(px(x, y)).or_default() += 1;
        }
    }
    let ground = counts.into_iter().max_by_key(|&(_, n)| n)?.0;
    let (mut x0, mut x1, mut y0, mut y1, mut n) = (80usize, 0usize, 96usize, 48usize, 0);
    for y in 48..96 {
        for x in 0..80 {
            if px(x, y) != ground {
                x0 = x0.min(x);
                x1 = x1.max(x + 1);
                y0 = y0.min(y);
                y1 = y1.max(y + 1);
                n += 1;
            }
        }
    }
    (n > 16).then_some((x0, x1, y0, y1))
}

/// The bounding box of the pixels two 160x144 RGBA frames disagree on, and how
/// many there are: `(x0, x1, y0, y1, count)`, or `None` if they are identical.
fn changed_box(a: &[u8], b: &[u8]) -> Option<(usize, usize, usize, usize, usize)> {
    let (mut x0, mut x1, mut y0, mut y1, mut n) = (160usize, 0usize, 144usize, 0usize, 0);
    for y in 0..144 {
        for x in 0..160 {
            let i = (y * 160 + x) * 4;
            if a[i..i + 3] != b[i..i + 3] {
                x0 = x0.min(x);
                x1 = x1.max(x + 1);
                y0 = y0.min(y);
                y1 = y1.max(y + 1);
                n += 1;
            }
        }
    }
    (n > 0).then_some((x0, x1, y0, y1, n))
}

/// Write a decompressed picture as a PNG, at a whole-number magnification.
fn save_pic_png(path: &Path, pic: &Pic, pal: Palette, scale: u32) {
    let (w, h) = (pic.width() as u32 * scale, pic.height() as u32 * scale);
    let mut img = image::RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let c = pal.0[pic.pixel((x / scale) as usize, (y / scale) as usize) as usize];
            img.put_pixel(x, y, image::Rgba([c[0], c[1], c[2], 255]));
        }
    }
    let _ = img.save(path);
}

// ---------------------------------------------------------------------------
// Stage 2: the cartridge plays it
// ---------------------------------------------------------------------------

/// What the driver is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Mash through the title and `CONTINUE` until the save is in WRAM.
    Booting,
    /// Standing in the overworld; open the start menu.
    Overworld,
    /// The start menu is open; put the cursor on `POKéMON` and confirm.
    StartMenu,
    /// The party screen is open; confirm on the only Pokémon in it.
    Party,
    /// The little menu over the party screen; choose `STATS`.
    MonMenu,
    /// The status screen. This is where the cry plays.
    Status,
    /// Back out of the menus and walk, looking for tall grass.
    Walking,
    /// A wild battle: the send-out is here.
    Battle,
}

/// Where a word appears on the screen, by row.
fn row_with(text: &str, words: &[&str]) -> Option<usize> {
    text.lines()
        .position(|l| words.iter().all(|w| l.contains(w)))
}

/// Drive the real cartridge to the moment it plays Marill's cry, and keep
/// everything it produces while it does.
///
/// **Why the status screen and not a wild battle.** Both play the cry through
/// the cartridge's own audio engine; the status screen is reachable from
/// wherever the player's save left them, while a battle needs a particular
/// patch of grass on a particular map, and this repository has no Gen 2
/// battle-control plugin to shortcut it (`docs/gen2/README.md` lists that as
/// not established). The cry is the same routine either way — `PlayCry` is
/// what both call — so this captures the real thing and says plainly which
/// door it walked through to get it.
fn drive(
    rom: &Path,
    save: &Path,
    out: &Path,
    cart: &Gen2Rom,
    marill: &SpeciesInfo,
    level: u8,
    verbose: bool,
) {
    // Never open the player's ROM directory for writing: the MBC puts
    // `<rom>.sav` beside the cartridge it is handed, and a run must never
    // overwrite somebody's real save.
    let work = out.join("cartridge.gbc");
    std::fs::copy(rom, &work).expect("stage the cartridge");
    let staged_save = work.with_extension("sav");
    let mut sram = std::fs::read(save).expect("read the save");
    put_marill_in_party(&mut sram, marill, level);
    std::fs::write(&staged_save, &sram).expect("stage the save");
    println!(
        "staged a level {level} {} into a copy of the save ({} bytes)",
        marill.name,
        sram.len()
    );

    let mut gb = Gameboy::new(std::fs::read(&work).expect("rom"), Some(work.clone()));
    let tape = Tape::default();
    gb.enable_audio(Box::new(tape.clone()));

    let mut pad = Pad::default();
    let mut stage = Stage::Booting;
    let mut settle = 0u32;
    let mut ready = 0u32;
    let mut last = String::new();
    let mut cry: Vec<(f32, f32)> = Vec::new();
    let mut battle_audio: Vec<(f32, f32)> = Vec::new();
    let mut status_frames: Vec<Vec<u8>> = Vec::new();
    let mut battle_frames: Vec<Vec<u8>> = Vec::new();
    let mut status_started = 0u32;
    let mut battle_started = 0u32;
    let mut backs_out = 0u32;
    let mut status_text: Option<String> = None;
    let mut send_out: Option<usize> = None;
    let mut dir = 0usize;
    let mut stuck = 0u32;
    let mut seed = 0x2BAD_5EEDu64;
    let mut was = (0u8, 0u8, 0u8, 0u8);
    /// How many walk steps before deciding this save does not start near
    /// grass. Each step is a press plus its gap, so this is minutes of walking.
    const WALK_FOR: u32 = 60_000;
    let walk_for = WALK_FOR;

    // Ten emulated minutes is far more than any of this needs; it is a bound,
    // not a budget.
    let limit = 60 * 60 * 60 * 4;
    for f in 0..limit {
        pad.tick(&mut gb);
        if !pad.busy() {
            let text = screen_text(&mut gb);
            if verbose {
                let squashed = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if squashed != last {
                    println!("[{f:6}] {stage:?} | {squashed}");
                    last = squashed;
                }
            }
            match stage {
                Stage::Booting => {
                    // The save is in WRAM once `CONTINUE` has been taken, but
                    // the box that reports the player's badges and Pokédex is
                    // still over the overworld at that point. The overworld
                    // itself writes no text into `wTilemap`, so "the save is
                    // loaded *and* the screen is blank" is the honest test for
                    // "we are standing in the world and the pad is ours".
                    let loaded = gb.peek(W_PARTY_COUNT) == 1
                        && gb.peek(W_PARTY_SPECIES) == marill.dex;
                    if loaded && text.trim().is_empty() {
                        settle += 1;
                        if settle > 60 {
                            println!(
                                "[{f:6}] the game loaded the save: party of {}, \
                                 species {}",
                                gb.peek(W_PARTY_COUNT),
                                gb.peek(W_PARTY_SPECIES)
                            );
                            stage = Stage::Overworld;
                            settle = 0;
                        }
                    } else {
                        settle = 0;
                        pad.press(&mut gb, KeypadKey::A);
                    }
                }
                Stage::Overworld => {
                    pad.press(&mut gb, KeypadKey::Start);
                    stage = Stage::StartMenu;
                    settle = 0;
                }
                Stage::StartMenu => {
                    settle += 1;
                    // `POKéMON` — the `é` is not an ASCII tile, so the row
                    // reads back as `POK MON`.
                    match (row_with(&text, &["POK", "MON"]), cursor_row(&mut gb)) {
                        (Some(want), Some(at)) if at == want => {
                            pad.press(&mut gb, KeypadKey::A);
                            stage = Stage::Party;
                            settle = 0;
                        }
                        (Some(want), Some(at)) => pad.press(
                            &mut gb,
                            if at < want {
                                KeypadKey::Down
                            } else {
                                KeypadKey::Up
                            },
                        ),
                        _ if settle > 120 => {
                            // The menu did not open; press START again rather
                            // than sit here.
                            stage = Stage::Overworld;
                            settle = 0;
                        }
                        _ => {}
                    }
                }
                Stage::Party => {
                    settle += 1;
                    // Wait for the list to be *finished*, not merely started:
                    // the party screen paints a row at a time and its `CANCEL`
                    // entry is the last thing on it. Confirming into a
                    // half-drawn menu is how a bot presses A at nothing.
                    if text.contains(&marill.name) && text.contains("CANCEL") {
                        pad.press(&mut gb, KeypadKey::A);
                        stage = Stage::MonMenu;
                        settle = 0;
                    } else if settle > 300 {
                        eprintln!("the party screen never showed {}", marill.name);
                        eprintln!("{text}");
                        std::process::exit(1);
                    }
                }
                Stage::MonMenu => {
                    settle += 1;
                    // The submenu paints one entry at a time, and its cursor is
                    // only where it looks once the last entry is down. A press
                    // into a half-drawn menu is a press at nothing — measured,
                    // and the reason this waits for the whole menu.
                    let drawn = text.contains("STATS")
                        && text.contains("SWITCH")
                        && text.contains("ITEM");
                    if !drawn {
                        ready = 0;
                        // The press that should have opened it can be swallowed
                        // by the party list still painting; ask again rather
                        // than give up on the first silence.
                        if settle % 60 == 0 {
                            pad.press(&mut gb, KeypadKey::A);
                        } else if settle > 600 {
                            eprintln!("no STATS entry appeared:\n{text}");
                            std::process::exit(1);
                        }
                    } else {
                        ready += 1;
                        if ready >= 20 {
                            match (row_with(&text, &["STATS"]), cursor_row(&mut gb)) {
                                (Some(want), Some(at)) if at == want => {
                                    // Everything from here is the cry.
                                    tape.arm();
                                    pad.press(&mut gb, KeypadKey::A);
                                    stage = Stage::Status;
                                    status_started = f;
                                }
                                (Some(want), Some(at)) => pad.press(
                                    &mut gb,
                                    if at < want {
                                        KeypadKey::Down
                                    } else {
                                        KeypadKey::Up
                                    },
                                ),
                                _ => {}
                            }
                        }
                    }
                }
                Stage::Status => {
                    if status_text.is_none() && status_frames.len() >= STATUS_FRAMES {
                        status_text = Some(text.clone());
                        cry = tape.cut();
                    }
                    // Captured enough of the status page; back out to the
                    // overworld and go looking for tall grass.
                    if status_frames.len() >= STATUS_FRAMES {
                        pad.press(&mut gb, KeypadKey::B);
                        backs_out += 1;
                        if backs_out > 4 && text.trim().is_empty() {
                            println!(
                                "[{f:6}] back in the overworld at map {}:{} — \
                                 walking to look for a wild battle",
                                gb.peek(W_MAP_GROUP),
                                gb.peek(W_MAP_NUMBER)
                            );
                            stage = Stage::Walking;
                            settle = 0;
                        }
                    }
                }
                Stage::Walking => {
                    settle += 1;
                    // "Wild <SPECIES> appeared!" is the first line of every
                    // wild encounter, and nothing in the overworld writes it.
                    if text.contains("appeared") {
                        println!("[{f:6}] a wild battle started");
                        tape.arm();
                        stage = Stage::Battle;
                        battle_started = f;
                    } else if settle > walk_for {
                        println!(
                            "[{f:6}] no wild battle in {walk_for} frames of \
                             walking at map {}:{} — the save does not start \
                             next to grass",
                            gb.peek(W_MAP_GROUP),
                            gb.peek(W_MAP_NUMBER)
                        );
                        break;
                    } else {
                        if settle % 50 == 0 {
                            println!(
                                "[{f:6}] walking: map {}:{} at ({},{})",
                                gb.peek(W_MAP_GROUP),
                                gb.peek(W_MAP_NUMBER),
                                gb.peek(W_X_COORD),
                                gb.peek(W_Y_COORD)
                            );
                        }
                        // Keep going while the player is actually moving and
                        // turn when they are not. A fixed four-direction cycle
                        // wedges in the first corner it finds — measured: it
                        // sat at (4,1) for the whole budget.
                        let here = (
                            gb.peek(W_MAP_GROUP),
                            gb.peek(W_MAP_NUMBER),
                            gb.peek(W_X_COORD),
                            gb.peek(W_Y_COORD),
                        );
                        if here == was {
                            stuck += 1;
                        } else {
                            stuck = 0;
                        }
                        // Turn when the current direction stops paying, and
                        // turn to a direction drawn from a small generator
                        // rather than to the next one round. Both matter, and
                        // both were measured: a walker that turns at the first
                        // obstruction paces one doorway for ever, and a walker
                        // that cycles four directions in order paces *two
                        // tiles* for ever — it never stops moving, so it never
                        // notices it is not going anywhere.
                        if stuck >= 2 || settle % 12 == 0 {
                            seed = seed
                                .wrapping_mul(6_364_136_223_846_793_005)
                                .wrapping_add(1_442_695_040_888_963_407);
                            dir = (seed >> 33) as usize % 4;
                            stuck = 0;
                        }
                        was = here;
                        let keys = [
                            KeypadKey::Down,
                            KeypadKey::Right,
                            KeypadKey::Up,
                            KeypadKey::Left,
                        ];
                        pad.press_for(&mut gb, keys[dir], 24);
                    }
                }
                Stage::Battle => {
                    // `Go! <NAME>!` is the line the cartridge prints as the
                    // ball opens, so it is the honest anchor for "the entrance
                    // starts here" — better than a frame number, which moves
                    // with the encounter.
                    if send_out.is_none()
                        && text.contains("Go")
                        && text.contains(&marill.name)
                    {
                        send_out = Some(battle_frames.len());
                        println!("[{f:6}] the cartridge printed Go! {}!", marill.name);
                    }
                    // Mash A through the encounter text.
                    pad.press(&mut gb, KeypadKey::A);
                }
            }
        }
        gb.frame();
        match stage {
            Stage::Status if status_frames.len() < STATUS_FRAMES => {
                status_frames.push(gb.image().to_vec())
            }
            Stage::Battle => {
                battle_frames.push(gb.image().to_vec());
                if battle_frames.len() >= BATTLE_FRAMES {
                    battle_audio = tape.cut();
                    break;
                }
            }
            _ => {}
        }
    }

    if status_frames.is_empty() {
        eprintln!("never reached the status screen (stopped in {stage:?})");
        std::process::exit(1);
    }
    println!(
        "[{status_started:6}] STATS confirmed — {} frames captured from the \
         status screen",
        status_frames.len()
    );
    // The status page, as it stood at the end of its own capture — not as the
    // screen stands now, which by this point is a wild battle.
    let landed = status_text.clone().unwrap_or_default();
    println!("the screen the cartridge drew:\n{landed}");
    // The status page's first screen carries the Pokédex number, the species,
    // its type and its experience; the party list and its submenu carry none
    // of those. This distinguishes "STATS was confirmed" from "A was pressed
    // at a menu that was still painting", which is the failure this driver
    // actually hit twice while it was being written.
    for want in ["EXP POINTS", "TYPE", &marill.name] {
        if !landed.contains(want) {
            eprintln!("STATS was not confirmed — the status page has no '{want}' on it");
            std::process::exit(1);
        }
    }

    if tape.calls() == 0 {
        eprintln!("the APU never ran — that is a harness bug, not a quiet cartridge");
        std::process::exit(1);
    }
    if !battle_frames.is_empty() {
        let wav = out.join("marill-battle.wav");
        terminalgb_chiptune::wav::write(&wav, &battle_audio, RATE)
            .expect("write the WAV");
        println!(
            "wrote {} — {:.2}s from the moment the encounter text appeared",
            wav.display(),
            battle_audio.len() as f64 / RATE as f64
        );
        println!(
            "[{battle_started:6}] wild battle: {} frames captured",
            battle_frames.len()
        );
        let mut kept = 0usize;
        for (i, frame) in battle_frames.iter().enumerate() {
            let keep = i == 0 || battle_frames[i - 1] != *frame;
            if !keep {
                continue;
            }
            if let Some(img) = image::RgbaImage::from_raw(160, 144, frame.clone()) {
                let _ = img.save(out.join(format!("battle-{i:04}.png")));
                kept += 1;
            }
        }
        println!("wrote {kept} changed battle frames to {}", out.display());
        check_entrance(cart, marill, &battle_frames);
        // The raw extent of everything in the player's quarter, frame by frame,
        // from the line the cartridge prints as the ball opens.
        //
        // **This is not the size of the Pokémon**, and reading it as if it were
        // is the mistake this tool made first: the Poké Ball's poof sprite is
        // in the quarter too, it is larger than the Pokémon for the first
        // several frames, and it is what these numbers mostly describe early
        // on. `check_entrance` above is the measurement that isolates the
        // Pokémon, by asking whether specific pixels are the colours the
        // species' own picture and palette say they should be. This stays
        // because it shows the shape of the whole event, poof included.
        println!(
            "everything in the player's quarter (the poof as well as the \
             Pokémon), from the frame the cartridge printed \"Go! {}!\" on:",
            marill.name
        );
        let start = send_out.unwrap_or(0);
        let mut seen: Option<(usize, usize)> = None;
        for (i, frame) in battle_frames.iter().enumerate().skip(start).take(60) {
            let Some(b) = sprite_box(frame) else { continue };
            let size = (b.1 - b.0, b.3 - b.2);
            if seen != Some(size) {
                println!(
                    "  frame {i:4} (+{:2}): {}x{} at x {}..{}, y {}..{}",
                    i - start,
                    size.0,
                    size.1,
                    b.0,
                    b.1,
                    b.2,
                    b.3
                );
                seen = Some(size);
            }
        }
    }

    let wav = out.join("marill-cry.wav");
    terminalgb_chiptune::wav::write(&wav, &cry, RATE).expect("write the WAV");
    let peak = cry
        .iter()
        .map(|&(l, r)| l.abs().max(r.abs()))
        .fold(0.0f32, f32::max);
    let rms = (cry
        .iter()
        .map(|&(l, r)| ((l + r) / 2.0).powi(2))
        .sum::<f32>()
        / cry.len().max(1) as f32)
        .sqrt();
    println!(
        "wrote {} — {:.2}s, {} APU blocks, peak {peak:.3}, RMS {rms:.4}",
        wav.display(),
        cry.len() as f64 / RATE as f64,
        tape.calls()
    );

    // **Does the picture move?** A Generation 1 Pokémon's front sprite never
    // does; this repository had never measured whether a Generation 2 one
    // does. Reporting "N frames changed" would not answer it — a fade, a
    // cursor and a sprite all change frames. What answers it is *where*: the
    // bounding box of the pixels that changed, against the box the species'
    // own picture occupies.
    println!("frame-to-frame change on the status screen (160x144):");
    for (i, w) in status_frames.windows(2).enumerate() {
        if let Some(b) = changed_box(&w[0], &w[1]) {
            println!(
                "  frame {:3} -> {:3}: {:5} pixels in x {}..{}, y {}..{}",
                i,
                i + 1,
                b.4,
                b.0,
                b.1,
                b.2,
                b.3
            );
        }
    }
    // Keep the frames that are *different*, not every fourth one: a strip of
    // identical pictures is not evidence of anything, and the ones either side
    // of a change are exactly what somebody checking this would want to see.
    let mut kept = 0usize;
    for (i, frame) in status_frames.iter().enumerate() {
        let keep = i == 0
            || i + 1 == status_frames.len()
            || status_frames[i - 1] != *frame
            || status_frames.get(i + 1).is_some_and(|n| n != frame);
        if !keep {
            continue;
        }
        if let Some(img) = image::RgbaImage::from_raw(160, 144, frame.clone()) {
            let _ = img.save(out.join(format!("status-{i:03}.png")));
            kept += 1;
        }
    }
    println!(
        "wrote the {kept} status frames either side of a change to {}",
        out.join("status-*.png").display()
    );

    // The picture box on the status screen, as the cartridge drew it. The
    // status page puts the front picture at the top left in a 7x7-tile well,
    // so a 5x5 picture sits inside it; dumping the whole well is what shows
    // where in it the engine put each step of the reveal.
    for (i, frame) in status_frames.iter().enumerate() {
        let show = i > 0
            && status_frames[i - 1] != *frame
            && changed_box(&status_frames[i - 1], frame)
                .is_some_and(|b| b.0 < 56 && b.2 < 64 && b.1 <= 64);
        if !show {
            continue;
        }
        println!("  --- status frame {i}, picture well (x 8..56, y 8..64) ---");
        for y in 8..64 {
            let row: String = (8..56)
                .map(|x| {
                    let o = (y * 160 + x) * 4;
                    let l =
                        (frame[o] as u32 + frame[o + 1] as u32 + frame[o + 2] as u32) / 3;
                    match l {
                        0..=63 => '@',
                        64..=127 => ':',
                        128..=200 => '.',
                        _ => ' ',
                    }
                })
                .collect();
            println!("  |{row}|");
        }
    }
    gb.flush_battery();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seven commands, one stream, checked against what each is defined to
    /// mean. Hand-built rather than taken from a cartridge: a test whose input
    /// is game data would put game data in this repository.
    #[test]
    fn every_lz_command_decodes() {
        // Each command byte is `bbbnnnnn`: command in the top three bits,
        // length less one in the low five.
        let stream = [
            0x02, 0xAA, 0xBB, 0xCC, // literal, 3
            0x21, 0x77, // iterate, 2
            0x43, 0x12, 0x34, // alternate, 4
            0x61, // zero, 2
            0x82, 0x80, // repeat, 3, from 1 byte back
            0xA0, 0x80, // flip, 1, from 1 byte back
            0xC1, 0x00, 0x01, // reverse, 2, from absolute offset 1
            0xFF,
        ];
        let (out, end) = decompress(&stream, 0).expect("a well-formed stream");
        assert_eq!(end, stream.len());
        assert_eq!(
            out,
            vec![
                0xAA, 0xBB, 0xCC, // literal
                0x77, 0x77, // iterate
                0x12, 0x34, 0x12, 0x34, // alternate
                0x00, 0x00, // zero
                // repeat: one byte back is the second zero, and the copy reads
                // the bytes it is writing, which is how a run is expressed.
                0x00, 0x00, 0x00, //
                0x00, // flip of 0x00
                0xBB, 0xAA, // reverse from offset 1, backwards
            ]
        );
    }

    /// The long form: `111bbbnn nnnnnnnn` is command `bbb` with a 10-bit
    /// length. A decoder that reads the length from the low five bits instead
    /// produces a picture that is the right *shape* and the wrong size, which
    /// is exactly the failure the length check catches.
    #[test]
    fn the_long_form_carries_a_ten_bit_length() {
        // `111` marks the long form, `011` is `zero`, and the last two bits
        // are the top of a 10-bit length: $12B, so $12C bytes.
        let stream = [0xED, 0x2B, 0xFF];
        let (out, _) = decompress(&stream, 0).expect("a well-formed stream");
        assert_eq!(out.len(), 0x12C);
        assert!(out.iter().all(|&b| b == 0));
    }

    /// A stream that asks for bytes nobody has written, or runs off the end,
    /// is an error and never a panic: `panic = 'abort'` makes a panic process
    /// death, and cartridge-supplied input must not be able to cause one.
    #[test]
    fn a_malformed_stream_is_an_error_not_a_panic() {
        assert!(decompress(&[0b100_00000, 0x80, 0xFF], 0).is_none()); // nothing to repeat
        assert!(decompress(&[0b000_11111], 0).is_none()); // literal past the end
        assert!(decompress(&[], 0).is_none());
    }

    /// Generation 2 stores a picture column-major, and this tool writes one
    /// back the same way. A round trip through both is what proves the two
    /// agree; getting it wrong the same way twice is the one thing it cannot
    /// catch, which is why the *cartridge* check in [`Gen2Rom::verify`] is the
    /// real evidence and this is only the local one.
    #[test]
    fn pictures_round_trip_through_their_tiles() {
        let mut pic = Pic {
            tiles: vec![0u8; 6 * 16],
            w: 2,
            h: 3,
        };
        for (i, (x, y)) in [(0, 0), (7, 0), (0, 23), (15, 23), (9, 11)]
            .into_iter()
            .enumerate()
        {
            set_pixel(&mut pic, x, y, (i % 4) as u8);
        }
        assert_eq!(pic.pixel(0, 0), 0);
        assert_eq!(pic.pixel(7, 0), 1);
        assert_eq!(pic.pixel(0, 23), 2);
        assert_eq!(pic.pixel(15, 23), 3);
        assert_eq!(pic.pixel(9, 11), 0);
        assert_eq!(pic.pixel(1, 1), 0);
    }

    /// The Pokéwalker's planar layout, stated as its own decoder states it:
    /// `((y / 8) * width + x) * 2`, upper plane first, bit `y % 8`.
    #[test]
    fn the_pack_packs_the_way_the_panel_unpacks() {
        let mut pic = Pic {
            tiles: vec![0u8; 16],
            w: 1,
            h: 1,
        };
        for y in 0..8 {
            set_pixel(&mut pic, 0, y, 3);
        }
        let data = planar(&pic, 16, 8, 0);
        assert_eq!(data.len(), 16 * 8 * 2 / 8);
        let unpack = |x: usize, y: usize| {
            let i = ((y / 8) * 16 + x) * 2;
            let bit = y % 8;
            (((data[i] >> bit) & 1) << 1) | ((data[i + 1] >> bit) & 1)
        };
        // The 8x8 picture is centred in a 16-wide box, so its column 0 lands
        // at x = 4, and every row of it is shade 3.
        assert!((0..8).all(|y| unpack(4, y) == 3));
        assert!((0..8).all(|y| unpack(0, y) == 0));
    }

    /// The level 10 Marill this tool crafts, checked against **what the
    /// cartridge itself printed on its status page**: `37/ 37` HP, `800`
    /// experience, and `LEVEL UP TO 11` in `264` more. Nothing here is a
    /// remembered number; all three came off the screen in the run recorded in
    /// `docs/gen2/marill.md`.
    #[test]
    fn the_crafted_marill_matches_what_the_cartridge_printed() {
        // Marill's base HP is 70 and its growth rate 4, both read out of the
        // cartridge by `Gen2Rom::species`.
        assert_eq!(stat(70, 15, 10, true), 37);
        assert_eq!(experience_for_level(4, 10), 800);
        assert_eq!(
            experience_for_level(4, 11) - experience_for_level(4, 10),
            264
        );
    }
}

/// **Check the entrance against the cartridge's own frames.**
///
/// Not "does it look right" — the three steps of [`ENTER_MON_BACK`] are
/// rendered from the extracted back picture in the extracted palette, placed
/// where the engine's own tilemap coordinates put them, and compared pixel for
/// pixel with every frame the emulator drew. The best-matching frame and its
/// agreement are printed, so a change that breaks the model shows up as a
/// number falling rather than as a picture somebody has to squint at.
///
/// Only the step's **inked** pixels are counted. The Poké Ball's poof sprite is
/// drawn over the top of the first two steps, and it is drawn on the white
/// ones; counting white would measure the poof, not the Pokémon.
fn check_entrance(cart: &Gen2Rom, marill: &SpeciesInfo, frames: &[Vec<u8>]) {
    let Some(back) = cart.pic(marill.dex, true) else {
        return;
    };
    let pal = cart.palette(marill.dex, false);
    println!(
        "the entrance, checked against the frames the cartridge drew \
         (BattleBGEffect_EnterMon, .PlayerData):"
    );
    for (step, (keep, (cx, cy))) in ENTER_MON_BACK.iter().enumerate() {
        let (px0, py0) = (cx * 8, cy * 8);
        let mut best = (0usize, 0usize, 0usize);
        for (n, f) in frames.iter().enumerate() {
            let (mut ok, mut tot) = (0usize, 0usize);
            for (r, &rr) in keep.iter().enumerate() {
                for (c, &cc) in keep.iter().enumerate() {
                    for y in 0..8 {
                        for x in 0..8 {
                            let v = back.pixel(cc * 8 + x, rr * 8 + y);
                            if v == 0 {
                                continue; // the poof is drawn over the white
                            }
                            tot += 1;
                            let i = ((py0 + r * 8 + y) * 160 + px0 + c * 8 + x) * 4;
                            ok += usize::from(f[i..i + 3] == pal.0[v as usize]);
                        }
                    }
                }
            }
            if tot > 0 && ok * best.2 >= best.0 * tot && ok >= best.0 {
                best = (ok, n, tot);
            }
        }
        let (ok, n, tot) = best;
        println!(
            "  step {}: {}x{} tiles at tilemap ({cx},{cy}) — frame {n} matches \
             {ok} of {tot} inked pixels ({:.1}%)",
            step + 1,
            keep.len(),
            keep.len(),
            100.0 * ok as f64 / tot.max(1) as f64
        );
    }
}

#[cfg(test)]
mod colour_and_entrance_tests {
    use super::*;

    /// The Game Boy Color correction, checked at the ends and on the two
    /// colours this tool reads off the cartridge for Marill. The values on the
    /// right came off the emulator's framebuffer with Marill on screen, so this
    /// pins the widening curve to what the machine actually draws — a linear
    /// `c << 3` would give (72, 104, 232) for the first of them and be wrong on
    /// every pixel.
    #[test]
    fn the_colour_curve_matches_the_emulator() {
        assert_eq!(Palette::rgb8(0x7FFF), [248, 248, 248]);
        assert_eq!(Palette::rgb8(0x0000), [0, 0, 0]);
        // Marill's two colours: RGB555 (9,13,29) and (25,8,5).
        assert_eq!(Palette::rgb8(0x75A9), [86, 136, 186]);
        assert_eq!(Palette::rgb8(0x1519), [173, 58, 73]);
    }

    /// Shade 0 and shade 3 are white and black on every species; only 1 and 2
    /// come out of the table. Getting that backwards would tint every white
    /// belly.
    #[test]
    fn a_palette_is_white_two_colours_and_black() {
        let p = Palette::from_words(0x75A9, 0x1519);
        assert_eq!(p.0[0], [248, 248, 248]);
        assert_eq!(p.0[1], [86, 136, 186]);
        assert_eq!(p.0[2], [173, 58, 73]);
        assert_eq!(p.0[3], [0, 0, 0]);
    }

    /// The entrance keeps outer tiles and throws away middle ones — the tables
    /// in `BattleBGEffect_EnterMon` say which, and the property that matters is
    /// that every step's kept set is a subset of the next, ending in all of
    /// them. A step that kept a tile the full picture does not have would put
    /// a stranger's pixels on the screen.
    #[test]
    fn the_entrance_steps_are_nested_and_end_whole() {
        for steps in [&ENTER_MON_BACK, &ENTER_MON_FRONT] {
            let last = steps[steps.len() - 1].0;
            assert_eq!(last, (0..last.len()).collect::<Vec<_>>());
            for pair in steps.windows(2) {
                assert!(pair[0].0.iter().all(|t| pair[1].0.contains(t)));
                assert!(pair[0].0.len() < pair[1].0.len());
            }
            // Two tiles are added each step, one at each end of the picture's
            // middle, so the box grows by 16 pixels a step.
            assert!(steps.windows(2).all(|p| p[1].0.len() == p[0].0.len() + 2));
        }
    }

    /// The box's bottom edge does not move: the Pokémon grows upward out of the
    /// ball rather than outward from its middle. `(column, row)` plus the
    /// step's size in tiles must land on the same bottom row every time.
    #[test]
    fn the_entrance_is_anchored_at_the_bottom() {
        for steps in [&ENTER_MON_BACK, &ENTER_MON_FRONT] {
            let bottoms: Vec<usize> =
                steps.iter().map(|(k, (_, row))| row + k.len()).collect();
            assert!(bottoms.windows(2).all(|w| w[0] == w[1]), "{bottoms:?}");
        }
    }

    /// `PWPET2` is `PWPET1` plus the cartridge's own eight palette bytes, and
    /// the first 1952 bytes must be byte-identical — otherwise it is a
    /// different format wearing a version number.
    #[test]
    fn the_coloured_pack_is_the_grey_one_plus_colour() {
        let m = SpeciesInfo {
            dex: 183,
            name: "MARILL".into(),
            base: [70, 20, 50, 40, 20, 50],
            type1: 0x15,
            type2: 0x15,
            catch_rate: 190,
            base_exp: 58,
            gender: 0x7F,
            hatch_cycles: 20,
            growth_rate: 4,
            egg_groups: 0x26,
            pic_w: 5,
            pic_h: 5,
        };
        let pic = Pic {
            tiles: vec![0u8; 25 * 16],
            w: 5,
            h: 5,
        };
        let one = pwpet(&m, &pic, 10, 37, None);
        assert_eq!(one.len(), 1952);
        assert_eq!(&one[..8], b"PWPET1\0\0");
    }
}
