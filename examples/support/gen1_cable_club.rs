//! Shared Gen 1 Cable Club machinery for the link examples.
//!
//! Extracted verbatim from `examples/link_two.rs` when `examples/bgb_link.rs`
//! arrived and needed exactly the same thing: a bot that plays Pokémon Red/Blue
//! from a save file into the Trade Center and through a trade, plus the
//! staging, save patching and screen reading that make that reproducible.
//!
//! It lives here rather than being copied because the *interesting* claim in
//! every one of those examples is "a real Gen 1 trade completed" — and two
//! copies of the bot would eventually be two different bots, at which point the
//! proofs stop being comparable. `link_two` uses it over a process-local cable
//! and over the auto-discovery peer; `bgb_link` uses it over a TCP socket.
//!
//! Included with `#[path = "support/gen1_cable_club.rs"] mod ...;`, the same way
//! the suite scouts include `support/suite_baseline.rs`. Each consumer uses a
//! subset, hence the blanket `dead_code` allowance.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use terminalgb::gameboy::Gameboy;
use terminalgb::KeypadKey;

use terminalgb_plugin_gen1_link::party::Party;
use terminalgb_plugin_gen1_link::spec::TradeSpec;

// --- Gen 1 addresses, all from `pret/pokered`'s symbols -------------------
// (`ram/wram.asm` and `ram/hram.asm`; see docs/gen1/README.md for the standard
// this file follows — cite the symbol, never a bare hex address.)

/// `wPartyCount`.
pub const W_PARTY_COUNT: u16 = 0xD163;
/// `wPartySpecies`.
pub const W_PARTY_SPECIES: u16 = 0xD164;
/// `wPartyMons`.
pub const W_PARTY_MONS: u16 = 0xD16B;
/// `wPartyMonOT`.
pub const W_PARTY_MON_OT: u16 = 0xD273;
/// `wPartyMonNicks`.
pub const W_PARTY_MON_NICKS: u16 = 0xD2B5;
/// `wPlayerName`, the first byte of `sMainData` when saved.
pub const W_MAIN_DATA_START: u16 = 0xD2F7;
/// `wCurMap`.
pub const W_CUR_MAP: u16 = 0xD35E;
/// `wYCoord` / `wXCoord`.
pub const W_Y_COORD: u16 = 0xD361;
pub const W_X_COORD: u16 = 0xD362;
/// `wObtainedBadges` is `$D356`; `wEventFlags` is the 320-byte flag array.
pub const W_EVENT_FLAGS: u16 = 0xD747;
/// `wLinkState` — `LINK_STATE_*` in `constants/serial_constants.asm`.
pub const W_LINK_STATE: u16 = 0xD12B;
/// `wNumberOfWarps` / `wWarpEntries`.
pub const W_NUMBER_OF_WARPS: u16 = 0xD3AE;
pub const W_WARP_ENTRIES: u16 = 0xD3AF;
/// `hSerialConnectionStatus`. `engine/link/cable_club_npc.asm` asserts this
/// address explicitly (`vc_assert hSerialConnectionStatus == $ffaa`), which is
/// as strong a citation as a Gen 1 address gets.
pub const H_SERIAL_CONNECTION_STATUS: u16 = 0xFFAA;
/// `USING_EXTERNAL_CLOCK` / `USING_INTERNAL_CLOCK`
/// (`constants/serial_constants.asm`). Note the values: **external is `$01`**
/// and internal is `$02`, which is the opposite way round from the
/// `ESTABLISH_CONNECTION_WITH_*` bytes that carry them.
pub const USING_EXTERNAL_CLOCK: u8 = 0x01;
pub const USING_INTERNAL_CLOCK: u8 = 0x02;

pub const MON_STRUCT_LEN: usize = 44;
pub const NAME_LEN: usize = 11;
/// `wPartyDataEnd - wPartyCount`: count + species list + 6 structs + OTs +
/// nicknames.
pub const PARTY_DATA_LEN: usize = 404;

/// `EVENT_GOT_POKEDEX`. The Cable Club receptionist refuses anybody without it
/// (`CableClubNPC`, `engine/link/cable_club_npc.asm`).
pub const EVENT_GOT_POKEDEX: usize = 37;

/// `VIRIDIAN_POKECENTER` and `TRADE_CENTER` (`constants/map_constants.asm`).
pub const MAP_VIRIDIAN_POKECENTER: u8 = 0x29;
pub const MAP_TRADE_CENTER: u8 = 0xEF;

/// Where the link receptionist's desk is in the Viridian Pokémon Center.
pub const RECEPTIONIST_X: u8 = 11;
pub const RECEPTIONIST_Y: u8 = 3;

// Offsets inside a raw `.sav` (`src/mbc/battery.rs`: raw cartridge RAM, bank 0
// first, no header — so a file offset *is* an SRAM offset).
pub const SAVE_PLAYER_NAME: usize = 0x2598;
pub const SAVE_MAIN_DATA: usize = 0x25A3;
pub const SAVE_PARTY_DATA: usize = 0x2F2C;
pub const SAVE_CHECKSUM: usize = 0x3523;

/// The DVs that make a Gen 1 Pokémon shiny once it reaches Gen 2.
///
/// Defence, speed and special all exactly 10, and the attack DV with bit 1 set.
/// `pokecrystal`'s `CheckShininess` (`engine/gfx/color.asm`) is the rule; the
/// canonical pair it names is `ATKDEFDV_SHINY = $EA` / `SPDSPCDV_SHINY = $AA`,
/// i.e. attack 14, defence 10, speed 10, special 10. **The verdict itself is
/// not recomputed here** — it comes from the trainer plugin's
/// `derived::shiny_if_traded`, which is the one copy of that rule.
pub const SHINY_DVS: (u8, u8, u8, u8) = (14, 10, 10, 10);

// ---------------------------------------------------------------------------
// Staging: copies only, never the player's own files.
// ---------------------------------------------------------------------------

pub struct Staged {
    pub rom: PathBuf,
}

pub fn stage(which: &str, rom: &Path, save: Option<&Path>, scratch: &Path) -> Staged {
    let dir = scratch.join(which);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let name = rom.file_name().expect("rom filename");
    let dest = dir.join(name);
    std::fs::copy(rom, &dest).expect("copy rom");
    let sav = dest.with_extension("sav");
    let _ = std::fs::remove_file(&sav);
    if let Some(s) = save {
        std::fs::copy(s, &sav).expect("copy save");
    }
    Staged { rom: dest }
}

/// Replace the staged save's party with `spec`, make sure the player has a
/// Pokédex, point the room's own staircase at the Viridian Pokémon Center, and
/// fix the checksum the game will check on load.
///
/// The warp redirection is the trick from `examples/gen1_trade.rs` and the
/// reasoning there still applies: continuing a save skips most of
/// `LoadMapHeader` (it returns early while `BIT_NO_PREVIOUS_MAP` is set), so the
/// map header, view pointer, border block and warp/sign/NPC tables are all
/// *saved* state. Writing a different `wCurMap` over the top renders one map
/// through another's tileset and crashes on the first warp. Moving the world's
/// pointers rather than the player leaves every one of those consistent.
pub fn patch_party(s: &Staged, spec: &str) {
    let sav = s.rom.with_extension("sav");
    let mut sram = std::fs::read(&sav).expect("staged save");
    let rom = std::fs::read(&s.rom).expect("staged rom");
    let main = |addr: u16| SAVE_MAIN_DATA + (addr - W_MAIN_DATA_START) as usize;

    let n = sram[main(W_NUMBER_OF_WARPS)] as usize;
    for i in 0..n.min(8) {
        sram[main(W_WARP_ENTRIES) + i * 4 + 2] = 0;
        sram[main(W_WARP_ENTRIES) + i * 4 + 3] = MAP_VIRIDIAN_POKECENTER;
    }
    sram[main(W_EVENT_FLAGS) + EVENT_GOT_POKEDEX / 8] |= 1 << (EVENT_GOT_POKEDEX % 8);

    let tables = terminalgb_plugin_gen1_link::romdata::RomTables::locate(rom)
        .expect("gen 1 rom tables");
    let parsed = TradeSpec::parse(spec, "link_two").expect("party spec");
    let party: Party = {
        use terminalgb_plugin_gen1_link::spec::PokemonSource;
        parsed.party(&tables).expect("build party")
    };
    let image = party.to_wram_image();
    assert_eq!(image.len(), PARTY_DATA_LEN);
    sram[SAVE_PARTY_DATA..SAVE_PARTY_DATA + PARTY_DATA_LEN].copy_from_slice(&image);

    // `CalcCheckSum` (`engine/menus/save.asm`) sums every byte of sGameData and
    // complements the result.
    let sum: u8 = sram[SAVE_PLAYER_NAME..SAVE_CHECKSUM]
        .iter()
        .fold(0u8, |a, &b| a.wrapping_add(b));
    sram[SAVE_CHECKSUM] = !sum;
    std::fs::write(&sav, &sram).expect("write staged save");
}

// ---------------------------------------------------------------------------
// The bot. One implementation drives both consoles: which seat it takes is read
// off the game, not decided in advance.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct TradeBot {
    hold: Option<(KeypadKey, u32)>,
    cooldown: u32,
    stage: Stage,
    stuck: u32,
    last_pos: (u8, u8),
    blocked: u32,
    on_trade_option: bool,
    spawn: Option<(u8, u8)>,
    done_frames: u32,
    traded: bool,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
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

impl TradeBot {
    pub fn stage_label(&self) -> &'static str {
        match self.stage {
            Stage::Continue => "continue",
            Stage::EnterRoom => "leave-room",
            Stage::WalkToReceptionist => "walk-to-desk",
            Stage::TalkAndSave => "receptionist",
            Stage::LinkMenu => "link-menu",
            Stage::InTradeRoom => "trade-room",
            Stage::AtMachine => "at-machine",
            Stage::Done => "traded",
            Stage::LeaveTradeScreen => "leaving",
            Stage::Finished => "finished",
        }
    }

    pub fn finished(&self) -> bool {
        self.stage == Stage::Finished
    }

    /// A bot that starts already standing in the Trade Center at `spawn`, so a
    /// caller that drove the consoles into the room itself — because it wanted
    /// to do something to the room first — can take the trade half only.
    pub fn in_trade_room(spawn: (u8, u8)) -> Self {
        Self {
            stage: Stage::InTradeRoom,
            spawn: Some(spawn),
            ..Self::default()
        }
    }

    /// Where the bot is up to, for a caller driving several stages by hand.
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// Did the trade actually complete, as opposed to the bot merely finishing?
    pub fn traded(&self) -> bool {
        self.traded
    }

    pub fn press(&mut self, gb: &mut Gameboy, key: KeypadKey, hold: u32) {
        gb.keydown(key);
        self.hold = Some((key, hold));
    }

    pub fn step_towards(&mut self, gb: &mut Gameboy, x: u8, y: u8, tx: u8, ty: u8) {
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

    pub fn act(&mut self, gb: &mut Gameboy, verbose: bool, who: &str) {
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
        let text = screen_text(gb);
        let map = gb.peek(W_CUR_MAP);
        let (x, y) = (gb.peek(W_X_COORD), gb.peek(W_Y_COORD));
        let link = gb.peek(W_LINK_STATE);
        let set = |b: &mut Self, s: Stage| {
            if b.stage != s {
                if verbose {
                    println!("  [{who}] {:?} -> {:?}", b.stage, s);
                }
                b.stage = s;
                b.stuck = 0;
            }
        };

        match self.stage {
            Stage::Continue => {
                let menu = text.contains("CONTINUE")
                    || text.contains("NEW GAME")
                    || text.contains("OPTION")
                    || text.contains("BADGES");
                let loaded = gb.peek(W_PARTY_MON_NICKS) != 0
                    && (1..=6).contains(&gb.peek(W_PARTY_COUNT));
                if menu || !loaded || !looks_like_a_map(gb) {
                    self.stuck = 0;
                    self.press(gb, KeypadKey::A, 4);
                    return;
                }
                self.stuck += 1;
                if self.stuck > 6 {
                    set(self, Stage::EnterRoom);
                }
            }
            Stage::EnterRoom => {
                if map == MAP_VIRIDIAN_POKECENTER {
                    set(self, Stage::WalkToReceptionist);
                    return;
                }
                let (wy, wx) = (gb.peek(W_WARP_ENTRIES), gb.peek(W_WARP_ENTRIES + 1));
                self.step_towards(gb, x, y, wx, wy);
            }
            Stage::WalkToReceptionist => {
                if map == MAP_TRADE_CENTER {
                    set(self, Stage::InTradeRoom);
                    return;
                }
                if link != 0 {
                    set(self, Stage::LinkMenu);
                    return;
                }
                if text.contains("CABLE CLUB") || text.contains("SAVE") {
                    set(self, Stage::TalkAndSave);
                    return;
                }
                if x != RECEPTIONIST_X || y != RECEPTIONIST_Y {
                    self.step_towards(gb, x, y, RECEPTIONIST_X, RECEPTIONIST_Y);
                } else {
                    self.stuck += 1;
                    if self.stuck % 2 == 1 {
                        self.press(gb, KeypadKey::Up, 4);
                    } else {
                        self.press(gb, KeypadKey::A, 4);
                    }
                }
            }
            Stage::TalkAndSave => {
                if link != 0 {
                    set(self, Stage::LinkMenu);
                    return;
                }
                self.press(gb, KeypadKey::A, 4);
            }
            Stage::LinkMenu => {
                if map == MAP_TRADE_CENTER {
                    set(self, Stage::InTradeRoom);
                    return;
                }
                self.press(gb, KeypadKey::A, 4);
            }
            Stage::InTradeRoom => {
                if link >= 0x02 {
                    set(self, Stage::AtMachine);
                    return;
                }
                // **Which seat is not a choice — the handshake decided it.**
                // `CableClubLeftGameboy` returns immediately unless this console
                // took the internal clock, and wants the player facing RIGHT;
                // `CableClubRightGameboy` is the mirror image for the external
                // clock (`engine/pokemon/bills_pc.asm`). The game has already
                // warped the player next to the correct console
                // (`LoadSpecialWarpData` picks `TradeCenterPlayerWarp` or
                // `TradeCenterFriendWarp` off the same byte), so all that is
                // left is to face the right way.
                let spawn = *self.spawn.get_or_insert((x, y));
                if (x, y) != spawn {
                    self.step_towards(gb, x, y, spawn.0, spawn.1);
                    return;
                }
                let face = match gb.peek(H_SERIAL_CONNECTION_STATUS) {
                    USING_INTERNAL_CLOCK => KeypadKey::Right,
                    USING_EXTERNAL_CLOCK => KeypadKey::Left,
                    // The handshake has not settled yet. Wait rather than guess
                    // — pressing A at the wrong console is a silent no-op that
                    // looks exactly like a broken link.
                    _ => return,
                };
                self.stuck += 1;
                if self.stuck % 2 == 0 {
                    self.press(gb, face, 4);
                } else {
                    self.press(gb, KeypadKey::A, 4);
                }
            }
            Stage::AtMachine => {
                if text.contains("Trade completed") {
                    self.traded = true;
                    set(self, Stage::Done);
                    return;
                }
                // The per-Pokémon menu is horizontal — "STATS     TRADE", with
                // the cursor on STATS — so nudge right once, then confirm.
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
            Stage::Done => {
                // After "Trade completed!" the game runs another full
                // three-block exchange and redraws the selection screen.
                self.done_frames += 1;
                if self.done_frames > 240 && text.contains("CANCEL") {
                    set(self, Stage::LeaveTradeScreen);
                }
            }
            Stage::LeaveTradeScreen => {
                // `LINK_STATE_IN_CABLE_CLUB`: `ReturnToCableClubRoom` has run
                // and the trade *screen* is behind us. It is not the *room* —
                // the Trade Center has no warp events at all and a real player
                // still has to press START -> RESET to get out of it (see
                // `docs/gen1/leaving-the-cable-club.md`). For a bot that only
                // wants the trade, being back here is the finish line.
                if link == 0x01 {
                    set(self, Stage::Finished);
                    return;
                }
                self.stuck += 1;
                if self.stuck <= 8 {
                    self.press(gb, KeypadKey::Down, 4);
                } else {
                    self.press(gb, KeypadKey::A, 4);
                }
            }
            Stage::Finished => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Reading the two consoles.
// ---------------------------------------------------------------------------

pub struct PartyMon {
    pub species: u8,
    pub level: u8,
    pub hp: u16,
    pub max_hp: u16,
    pub dvs: (u8, u8, u8, u8),
    pub ot_id: u16,
    pub experience: u32,
    pub ot_name: String,
    pub nickname: String,
}

impl PartyMon {
    pub fn describe(&self) -> String {
        // The verdict is the trainer plugin's, not a second copy of the rule.
        let dvs = terminalgb_plugin_trainer::gen1::Dvs::from_bytes(
            (self.dvs.0 << 4) | self.dvs.1,
            (self.dvs.2 << 4) | self.dvs.3,
        );
        let shiny = terminalgb_plugin_trainer::derived::shiny_if_traded(dvs);
        format!(
            "{:<10} species {:#04x} Lv{:<3} HP {}/{} DVs {:?} exp {:<7} OT {} (id {}){}",
            self.nickname,
            self.species,
            self.level,
            self.hp,
            self.max_hp,
            self.dvs,
            self.experience,
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

pub fn read_party(gb: &mut Gameboy) -> Vec<PartyMon> {
    let count = gb.peek(W_PARTY_COUNT) as usize;
    if count == 0 || count > 6 {
        return Vec::new();
    }
    (0..count)
        .map(|i| {
            let b =
                gb.peek_range(W_PARTY_MONS + (i * MON_STRUCT_LEN) as u16, MON_STRUCT_LEN);
            let be = |o: usize| u16::from_be_bytes([b[o], b[o + 1]]);
            // `party_struct`: the DV word sits at offset 27 (`wPartyMon1DVs`).
            let (d0, d1) = (b[27], b[28]);
            PartyMon {
                species: gb.peek(W_PARTY_SPECIES + i as u16),
                level: b[33],
                hp: be(1),
                max_hp: be(34),
                dvs: (d0 >> 4, d0 & 15, d1 >> 4, d1 & 15),
                ot_id: be(12),
                experience: ((b[14] as u32) << 16) | ((b[15] as u32) << 8) | b[16] as u32,
                ot_name: decode(
                    &gb.peek_range(W_PARTY_MON_OT + (i * NAME_LEN) as u16, NAME_LEN),
                ),
                nickname: decode(
                    &gb.peek_range(W_PARTY_MON_NICKS + (i * NAME_LEN) as u16, NAME_LEN),
                ),
            }
        })
        .collect()
}

pub fn decode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|&&b| b != 0x50)
        .filter_map(|&b| terminalgb_plugin_gen1_link::text::decode_char(b))
        .collect()
}

/// Read what is on screen as text. Gen 1 writes character codes straight into a
/// tilemap and loads the font so tile id == character code — but it draws the
/// **whole screen through the window layer**, so reading the background map
/// returns 20x18 spaces and every menu looks empty.
pub fn screen_text(gb: &mut Gameboy) -> String {
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

pub fn looks_like_a_map(gb: &mut Gameboy) -> bool {
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

/// Both screens, decoded, side by side — the "see both consoles" answer for a
/// headless run.
pub fn side_by_side(a: &mut Gameboy, b: &mut Gameboy) -> String {
    let (ta, tb) = (screen_text(a), screen_text(b));
    let mut out = String::from("      console A                console B\n");
    for (la, lb) in ta.lines().zip(tb.lines()) {
        out.push_str(&format!("    |{la}|  |{lb}|\n"));
    }
    out
}

/// One PNG with both 160x144 frames in it, separated by a one-pixel gutter.
///
/// Uses the `image` crate — the same path the frontend's screenshot keys take.
/// `termimage`'s writer is *stored* deflate, which is right for a 60 Hz wire and
/// wrong for a file somebody keeps.
pub fn write_pair(path: &Path, a: &mut Gameboy, b: &mut Gameboy) {
    const W: u32 = 160;
    const H: u32 = 144;
    const GUTTER: u32 = 4;
    let mut out = image::RgbaImage::new(W * 2 + GUTTER, H);
    for (i, src) in [a.image(), b.image()].iter().enumerate() {
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

pub fn write_one(path: &Path, gb: &mut Gameboy) {
    if let Some(img) = image::RgbaImage::from_raw(160, 144, gb.image().to_vec()) {
        let _ = img.save(path);
    }
}
