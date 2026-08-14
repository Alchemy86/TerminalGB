//! Two emulated Game Boys, one link cable, in one process.
//!
//! ```text
//! cargo run --release --features image,plugin-gen1-link,plugin-trainer \
//!     --example link_two -- --rom ~/roms/blue.gb --save <fixture.sav> --trade
//! ```
//!
//! # What this is, and why it is not the Gen 1 link *plugin*
//!
//! `plugins/gen1-link` impersonates a second console: it speaks Pokémon
//! Red/Blue's trade protocol from Rust, well enough that the real game completes
//! a real trade with it. It is the right answer when you own one cartridge, and
//! it can only ever know the protocols somebody wrote down.
//!
//! This is the other answer. [`terminalgb::link::LinkCable`] joins **two whole
//! cores** and relays bytes; neither end knows anything about Pokémon. Whatever
//! the two games agree on works, because both ends are the genuine article.
//!
//! # Presentation: side by side, headless
//!
//! Both consoles are always visible — the `--shots` captures are a single PNG
//! with both screens in it, and `--verbose` prints both tilemaps decoded as
//! text, which is what a Gen 1 game's screen really is. That is a deliberate
//! choice over building a two-pane terminal frontend: the terminal render path
//! transmits **one** picture under one image id and the whole of
//! `docs/terminal-render.md` is about not making that more expensive, so a
//! second live picture is real work with real regression risk. The link is the
//! deliverable; a side-by-side capture proves both machines are running and
//! costs nothing that could break the frontend.
//!
//! # Never touch the player's ROM directory
//!
//! The MBC writes `<rom>.sav` next to the ROM it was given, so both consoles
//! get a **copy** of the cartridge and a **copy** of the save, staged under
//! `target/link-two/`. Nothing here opens a path in `~/roms` for writing.

use std::path::{Path, PathBuf};

use terminalgb::gameboy::Gameboy;
use terminalgb::link::LinkCable;
use terminalgb::KeypadKey;

use terminalgb_plugin_gen1_link::party::Party;
use terminalgb_plugin_gen1_link::spec::TradeSpec;

// --- Gen 1 addresses, all from `pret/pokered`'s symbols -------------------
// (`ram/wram.asm` and `ram/hram.asm`; see docs/gen1/README.md for the standard
// this file follows — cite the symbol, never a bare hex address.)

/// `wPartyCount`.
const W_PARTY_COUNT: u16 = 0xD163;
/// `wPartySpecies`.
const W_PARTY_SPECIES: u16 = 0xD164;
/// `wPartyMons`.
const W_PARTY_MONS: u16 = 0xD16B;
/// `wPartyMonOT`.
const W_PARTY_MON_OT: u16 = 0xD273;
/// `wPartyMonNicks`.
const W_PARTY_MON_NICKS: u16 = 0xD2B5;
/// `wPlayerName`, the first byte of `sMainData` when saved.
const W_MAIN_DATA_START: u16 = 0xD2F7;
/// `wCurMap`.
const W_CUR_MAP: u16 = 0xD35E;
/// `wYCoord` / `wXCoord`.
const W_Y_COORD: u16 = 0xD361;
const W_X_COORD: u16 = 0xD362;
/// `wObtainedBadges` is `$D356`; `wEventFlags` is the 320-byte flag array.
const W_EVENT_FLAGS: u16 = 0xD747;
/// `wLinkState` — `LINK_STATE_*` in `constants/serial_constants.asm`.
const W_LINK_STATE: u16 = 0xD12B;
/// `wNumberOfWarps` / `wWarpEntries`.
const W_NUMBER_OF_WARPS: u16 = 0xD3AE;
const W_WARP_ENTRIES: u16 = 0xD3AF;
/// `hSerialConnectionStatus`. `engine/link/cable_club_npc.asm` asserts this
/// address explicitly (`vc_assert hSerialConnectionStatus == $ffaa`), which is
/// as strong a citation as a Gen 1 address gets.
const H_SERIAL_CONNECTION_STATUS: u16 = 0xFFAA;
/// `USING_EXTERNAL_CLOCK` / `USING_INTERNAL_CLOCK`
/// (`constants/serial_constants.asm`). Note the values: **external is `$01`**
/// and internal is `$02`, which is the opposite way round from the
/// `ESTABLISH_CONNECTION_WITH_*` bytes that carry them.
const USING_EXTERNAL_CLOCK: u8 = 0x01;
const USING_INTERNAL_CLOCK: u8 = 0x02;

const MON_STRUCT_LEN: usize = 44;
const NAME_LEN: usize = 11;
/// `wPartyDataEnd - wPartyCount`: count + species list + 6 structs + OTs +
/// nicknames.
const PARTY_DATA_LEN: usize = 404;

/// `EVENT_GOT_POKEDEX`. The Cable Club receptionist refuses anybody without it
/// (`CableClubNPC`, `engine/link/cable_club_npc.asm`).
const EVENT_GOT_POKEDEX: usize = 37;

/// `VIRIDIAN_POKECENTER` and `TRADE_CENTER` (`constants/map_constants.asm`).
const MAP_VIRIDIAN_POKECENTER: u8 = 0x29;
const MAP_TRADE_CENTER: u8 = 0xEF;

/// Where the link receptionist's desk is in the Viridian Pokémon Center.
const RECEPTIONIST_X: u8 = 11;
const RECEPTIONIST_Y: u8 = 3;

// Offsets inside a raw `.sav` (`src/mbc/battery.rs`: raw cartridge RAM, bank 0
// first, no header — so a file offset *is* an SRAM offset).
const SAVE_PLAYER_NAME: usize = 0x2598;
const SAVE_MAIN_DATA: usize = 0x25A3;
const SAVE_PARTY_DATA: usize = 0x2F2C;
const SAVE_CHECKSUM: usize = 0x3523;

/// The DVs that make a Gen 1 Pokémon shiny once it reaches Gen 2.
///
/// Defence, speed and special all exactly 10, and the attack DV with bit 1 set.
/// `pokecrystal`'s `CheckShininess` (`engine/gfx/color.asm`) is the rule; the
/// canonical pair it names is `ATKDEFDV_SHINY = $EA` / `SPDSPCDV_SHINY = $AA`,
/// i.e. attack 14, defence 10, speed 10, special 10. **The verdict itself is
/// not recomputed here** — it comes from the trainer plugin's
/// `derived::shiny_if_traded`, which is the one copy of that rule.
const SHINY_DVS: (u8, u8, u8, u8) = (14, 10, 10, 10);

fn main() {
    let mut rom_a: Option<PathBuf> = None;
    let mut rom_b: Option<PathBuf> = None;
    let mut save_a: Option<PathBuf> = None;
    let mut save_b: Option<PathBuf> = None;
    let mut shots: Option<PathBuf> = None;
    let mut frames = 60 * 60 * 8;
    let mut trade = false;
    let mut verbose = false;
    let mut skew: u32 = 0;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rom" => rom_a = args.next().map(PathBuf::from),
            "--rom-b" => rom_b = args.next().map(PathBuf::from),
            "--save" => save_a = args.next().map(PathBuf::from),
            "--save-b" => save_b = args.next().map(PathBuf::from),
            "--shots" => shots = args.next().map(PathBuf::from),
            "--frames" => {
                frames = args.next().and_then(|s| s.parse().ok()).unwrap_or(frames)
            }
            "--skew" => skew = args.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            "--trade" => trade = true,
            "--verbose" => verbose = true,
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(rom_a) = rom_a else {
        eprintln!(
            "usage: link_two --rom <blue.gb> [--rom-b <other.gb>] \
             --save <fixture.sav> [--save-b <fixture.sav>] \
             [--trade] [--shots <dir>] [--frames N] [--verbose]\n\
             \n\
             --trade drives both consoles into the Cable Club and trades. \
             Without it the two just run side by side with the cable live, \
             which is the honest smoke test for any pair of ROMs."
        );
        std::process::exit(2);
    };
    let rom_b = rom_b.unwrap_or_else(|| rom_a.clone());
    // Both consoles start from the same fixture unless told otherwise — the
    // parties are what distinguish them, and those are patched in below.
    let save_b = save_b.or_else(|| save_a.clone());

    let scratch = PathBuf::from("target/link-two");
    let a = stage("a", &rom_a, save_a.as_deref(), &scratch);
    let b = stage("b", &rom_b, save_b.as_deref(), &scratch);

    if trade {
        // Distinct parties, so "the trade happened" is provable rather than
        // plausible. Console A carries the shiny-DV Pokémon; console B carries
        // an ordinary one to send back.
        patch_party(
            &a,
            &format!(
                "trainer = RED\n\
                 species = Abra\nlevel = 10\nnickname = GLIMMER\not_name = RED\not_id = 4242\n\
                 dvs = {}, {}, {}, {}\n\
                 moves = Teleport\n\
                 species = Rattata\nlevel = 8\nnickname = RATTY\not_name = RED\not_id = 4242\n\
                 moves = Tackle, Tail Whip\n",
                SHINY_DVS.0, SHINY_DVS.1, SHINY_DVS.2, SHINY_DVS.3
            ),
        );
        patch_party(
            &b,
            "trainer = BLUE\n\
             species = Machop\nlevel = 11\nnickname = ROCKY\not_name = BLUE\not_id = 1717\n\
             dvs = 3, 4, 5, 6\n\
             moves = Karate Chop, Low Kick\n\
             species = Pidgey\nlevel = 7\nnickname = FLAP\not_name = BLUE\not_id = 1717\n\
             moves = Gust\n",
        );
    }

    let mut gb_a =
        Gameboy::new(std::fs::read(&a.rom).expect("rom a"), Some(a.rom.clone()));
    let mut gb_b =
        Gameboy::new(std::fs::read(&b.rom).expect("rom b"), Some(b.rom.clone()));
    println!(
        "console A: {}\nconsole B: {}",
        gb_a.rom_title(),
        gb_b.rom_title()
    );

    if let Some(dir) = &shots {
        std::fs::create_dir_all(dir).expect("shots dir");
    }

    // Two real consoles never share a crystal, and two real players never press
    // A at the same instant — so their link handshakes drift past each other
    // until they interleave. Two emulated cores stepped by the same loop have
    // **no** such drift: start them together and they stay in phase for ever,
    // both mastering at the same moment and each finding the other deaf. That
    // is not a cable bug, it is the price of determinism, and the cure is to
    // start one console a little ahead of the other. See
    // `docs/link-cable.md` §"Phase".
    for _ in 0..skew {
        gb_b.frame();
    }
    if skew > 0 {
        println!("console B started {skew} frames ahead of console A");
    }

    let mut cable = LinkCable::new();
    let mut bots = trade.then(|| (TradeBot::default(), TradeBot::default()));
    let mut shot_n = 0usize;
    let mut last_labels = (String::new(), String::new());

    for frame in 0..frames {
        if let Some((ba, bb)) = &mut bots {
            ba.act(&mut gb_a, verbose, "A");
            bb.act(&mut gb_b, verbose, "B");
        }
        cable.run_frame(&mut gb_a, &mut gb_b);

        if cable.desynced() {
            eprintln!(
                "\nLINK DESYNCHRONISED at frame {frame} — both machines faulted, \
                 neither save will be written.\n  A: {:?}\n  B: {:?}",
                gb_a.fault(),
                gb_b.fault()
            );
            std::process::exit(1);
        }

        if let Some((ba, bb)) = &bots {
            let labels = (ba.stage_label().to_string(), bb.stage_label().to_string());
            if labels != last_labels {
                println!(
                    "[{frame:6}] A:{:<14} B:{:<14} transfers {:<5} clock A={:#04x} B={:#04x}",
                    labels.0,
                    labels.1,
                    cable.stats().transfers,
                    gb_a.peek(H_SERIAL_CONNECTION_STATUS),
                    gb_b.peek(H_SERIAL_CONNECTION_STATUS),
                );
                if let Some(dir) = &shots {
                    shot_n += 1;
                    write_pair(
                        &dir.join(format!("{shot_n:02}-{}-{}.png", labels.0, labels.1)),
                        &mut gb_a,
                        &mut gb_b,
                    );
                }
                if verbose {
                    println!("{}", side_by_side(&mut gb_a, &mut gb_b));
                }
                last_labels = labels;
            }
            if ba.finished() && bb.finished() {
                println!("both bots finished at frame {frame}");
                break;
            }
        }
    }

    // --- report ----------------------------------------------------------
    let stats = cable.stats();
    println!(
        "\ncable: {} transfers, {} unanswered, {} torn",
        stats.transfers, stats.unanswered, stats.torn
    );
    if let Some(dir) = &shots {
        write_pair(&dir.join("99-final.png"), &mut gb_a, &mut gb_b);
    }
    println!("{}", side_by_side(&mut gb_a, &mut gb_b));

    let pa = read_party(&mut gb_a);
    let pb = read_party(&mut gb_b);
    println!("\nconsole A party:");
    for m in &pa {
        println!("  {}", m.describe());
    }
    println!("console B party:");
    for m in &pb {
        println!("  {}", m.describe());
    }

    if trade {
        let swapped = pa.iter().any(|m| m.ot_name == "BLUE")
            && pb.iter().any(|m| m.ot_name == "RED");
        println!(
            "\ntrade: {}",
            if swapped {
                "COMPLETE — each console is holding a Pokémon whose original \
                 trainer is the other console's player"
            } else {
                "NOT COMPLETE — neither party holds the other's Pokémon"
            }
        );
        if !swapped {
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Staging: copies only, never the player's own files.
// ---------------------------------------------------------------------------

struct Staged {
    rom: PathBuf,
}

fn stage(which: &str, rom: &Path, save: Option<&Path>, scratch: &Path) -> Staged {
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
fn patch_party(s: &Staged, spec: &str) {
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
struct TradeBot {
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
enum Stage {
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
    fn stage_label(&self) -> &'static str {
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

    fn finished(&self) -> bool {
        self.stage == Stage::Finished
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

    fn act(&mut self, gb: &mut Gameboy, verbose: bool, who: &str) {
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
                // `LINK_STATE_IN_CABLE_CLUB`: `ReturnToCableClubRoom` has run.
                // The Trade Center has no warp events at all, so being back in
                // the room *is* leaving.
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

struct PartyMon {
    species: u8,
    level: u8,
    hp: u16,
    max_hp: u16,
    dvs: (u8, u8, u8, u8),
    ot_id: u16,
    experience: u32,
    ot_name: String,
    nickname: String,
}

impl PartyMon {
    fn describe(&self) -> String {
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

fn read_party(gb: &mut Gameboy) -> Vec<PartyMon> {
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

fn decode(bytes: &[u8]) -> String {
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
fn screen_text(gb: &mut Gameboy) -> String {
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

fn looks_like_a_map(gb: &mut Gameboy) -> bool {
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
fn side_by_side(a: &mut Gameboy, b: &mut Gameboy) -> String {
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
fn write_pair(path: &Path, a: &mut Gameboy, b: &mut Gameboy) {
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
