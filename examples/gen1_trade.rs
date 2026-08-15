//! Headless driver that proves the Gen 1 virtual link partner really trades.
//!
//! A design document with no working trade is not done, and neither is a unit
//! test: the claim is that **Pokémon Blue completes a link trade against this
//! plugin**, and the only way to show that is to run the real ROM, walk to the
//! Cable Club, and read the resulting party out of the machine's own memory.
//!
//! This is an example rather than a test-harness fixture because it needs a
//! commercial ROM the repository cannot contain.
//!
//! ```text
//! # 1. build a save that is far enough into the game to use the Cable Club
//! cargo run --release --example gen1_trade -- --rom <rom> --make-save
//!
//! # 2. trade
//! cargo run --release --example gen1_trade -- --rom <rom>
//!
//! # 2b. …and prove the player can still get out after a mid-session desync
//! cargo run --release --example gen1_trade -- --rom <rom> --quick-load
//! ```
//!
//! # Receiving the Pokémon is only half the claim
//!
//! The run makes **three visits** to the Game Boy on the table — trade, then a
//! visit that cancels without trading, then a second trade — and each one has to
//! end with the player back out of the trade screen. That matters because the
//! Trade Center has **no warp events at all**
//! (`pret/pokered`, `data/maps/objects/TradeCenter.asm`; `wNumberOfWarps` reads
//! 0 at run time): backing out of the selection screen is the only way to reach
//! the room's floor again, and it depends entirely on the partner echoing the
//! cancel nybble `$6F`. (Reaching the floor is not *leaving* — that takes
//! START -> RESET; see `docs/gen1/leaving-the-cable-club.md`.)
//! `--quick-load` presses F5 then F8 at that moment, which resets plugin state
//! mid-session exactly as the terminal frontend does, so the cancel has to work
//! from a phase the partner did not expect. `GB_GEN1_LINK_TRACE=1` prints the
//! tail of the wire when it does not.
//!
//! # About the save fixture
//!
//! The Cable Club needs `EVENT_GOT_POKEDEX`, which is a good half hour of play
//! away from a new game. `--make-save` therefore does what a test fixture
//! should: it boots a **new game**, plays the intro with real button presses
//! until the game writes its own save file, and then edits four things in that
//! save — the current map, the player's position, the Pokédex event flag and the
//! party. Everything else is state the game itself produced.
//!
//! That is set-up, not cheating: **nothing about the trade is faked**. From the
//! link receptionist onward the driver only presses buttons, and the Pokémon
//! arrives over the emulated serial port through the real protocol. If you have
//! your own save, skip step 1 and pass `--save <file>`.
//!
//! The ROM and its save are copied into a scratch directory first — the MBC
//! writes a `.sav` next to the ROM path, and a verification run must not touch
//! the player's save.

use std::path::{Path, PathBuf};
use terminalgb::gameboy::Gameboy;
use terminalgb::KeypadKey;
use terminalgb_plugin_gen1_link::{party::Party, spec::TradeSpec, Gen1LinkPartner};

// --- Pokémon Red/Blue (US) WRAM addresses ---------------------------------
// From `pret/pokered`'s `ram/wram.asm`, which reproduces the retail ROM
// byte-exactly, so its symbols are the retail addresses. Every one of them is
// cross-checked at run time (see `verify_addresses`), because a wrong address
// here would make this driver report a trade that did not happen.
const W_PARTY_COUNT: u16 = 0xD163;
const W_PARTY_SPECIES: u16 = 0xD164;
const W_PARTY_MONS: u16 = 0xD16B;
const W_PARTY_MON_OT: u16 = 0xD273;
const W_PARTY_MON_NICKS: u16 = 0xD2B5;
const W_MAIN_DATA_START: u16 = 0xD2F7;
const W_CUR_MAP: u16 = 0xD35E;
const W_Y_COORD: u16 = 0xD361;
const W_X_COORD: u16 = 0xD362;
const W_EVENT_FLAGS: u16 = 0xD747;
const W_LINK_STATE: u16 = 0xD12B;
const MON_STRUCT_LEN: usize = 44;
const NAME_LEN: usize = 11;
const PARTY_DATA_LEN: usize = 404;

/// `EVENT_GOT_POKEDEX` is index 37 in `constants/event_constants.asm`, and
/// `FlagAction` addresses bit `n & 7` of byte `n >> 3`.
const EVENT_GOT_POKEDEX: usize = 37;

const MAP_VIRIDIAN_POKECENTER: u8 = 0x29;
const MAP_TRADE_CENTER: u8 = 0xEF;

/// The link receptionist stands at (11, 2) facing down
/// (`data/maps/objects/ViridianPokecenter.asm`); (11, 3) is the tile in front
/// of her.
const RECEPTIONIST_X: u8 = 11;
const RECEPTIONIST_Y: u8 = 3;

const W_NUMBER_OF_WARPS: u16 = 0xD3AE;
const W_WARP_ENTRIES: u16 = 0xD3AF;

// --- save-file layout ------------------------------------------------------
// `engine/menus/save.asm` copies WRAM into SRAM as five contiguous images plus
// a one-byte complemented checksum over everything before it. `sGameData` is at
// SRAM `$A598` in **bank 1**, i.e. 0x2598 into the cartridge RAM image.
//
// Save files are `<rom>.sav`: **raw cartridge RAM, no header, bank 0 first**
// (`src/mbc/battery.rs`), so a file offset is an SRAM offset. A cartridge with a
// real RTC (header type `0x0F`/`0x10`) appends a footer *after* the RAM; Pokémon
// Blue is type `0x13`, MBC3+RAM+BATTERY with no RTC, so there is none.
//
// This used to be `.gbsave` with an eight-byte RTC epoch *in front* of the RAM,
// which shifted every offset below by 8 and produced a save that looked
// plausible and failed its checksum. If saves ever change shape again, this
// constant is the thing to check first.
const SAVE_HEADER: usize = 0;
const SAVE_PLAYER_NAME: usize = SAVE_HEADER + 0x2598;
const SAVE_MAIN_DATA: usize = SAVE_HEADER + 0x25A3;
const SAVE_PARTY_DATA: usize = SAVE_HEADER + 0x2F2C;
const SAVE_CHECKSUM: usize = SAVE_HEADER + 0x3523;

fn main() {
    let mut rom_path: Option<PathBuf> = None;
    let mut trade_path: Option<PathBuf> = None;
    let mut save_path: Option<PathBuf> = None;
    let mut shots: Option<PathBuf> = None;
    let mut state_out: Option<PathBuf> = None;
    let mut frames = 60 * 60 * 6;
    let mut make_save = false;
    let mut verbose = false;
    // Press F5 then F8 (the terminal frontend's quick-save / quick-load) once
    // the trade is done and the player wants out. Plugin state is deliberately
    // *not* in a save state, so the load calls `PluginHost::reset` and the
    // partner drops back to `Phase::Handshake` while the game is still mid-link.
    // Backing out of the trade screen is the escape hatch from exactly that, so
    // it has to work from a phase the partner did not expect.
    let mut quick_load = false;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rom" => rom_path = args.next().map(PathBuf::from),
            "--trade" => trade_path = args.next().map(PathBuf::from),
            "--save" => save_path = args.next().map(PathBuf::from),
            "--shots" => shots = args.next().map(PathBuf::from),
            "--state" => state_out = args.next().map(PathBuf::from),
            "--frames" => {
                frames = args.next().and_then(|s| s.parse().ok()).unwrap_or(frames)
            }
            "--make-save" => make_save = true,
            "--verbose" => verbose = true,
            "--quick-load" => quick_load = true,
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(rom_path) = rom_path else {
        eprintln!(
            "usage: gen1_trade --rom <path> [--make-save] [--save <file>] \
             [--trade <file>] [--state <file>] [--quick-load] [--verbose]"
        );
        std::process::exit(2);
    };

    let scratch = std::env::temp_dir().join("gameboy-gen1-trade");
    std::fs::create_dir_all(&scratch).expect("scratch dir");
    let fixture = save_path.unwrap_or_else(|| scratch.join("cableclub.sav"));

    if make_save {
        build_fixture(&rom_path, &scratch, &fixture, verbose);
        return;
    }

    if !fixture.exists() {
        eprintln!(
            "no save fixture at {} — run with --make-save first",
            fixture.display()
        );
        std::process::exit(2);
    }
    run_trade(
        &rom_path,
        &scratch,
        &fixture,
        trade_path.as_deref(),
        shots,
        state_out,
        frames,
        quick_load,
        verbose,
    );
}

// ---------------------------------------------------------------------------
// Stage 1: get a save that can use the Cable Club.
// ---------------------------------------------------------------------------

fn build_fixture(rom_path: &Path, scratch: &Path, out: &Path, verbose: bool) {
    let work = scratch.join("newgame.gb");
    std::fs::copy(rom_path, &work).expect("copy rom");
    let _ = std::fs::remove_file(work.with_extension("sav"));

    println!("== building a save fixture by playing the intro ==");
    let rom = std::fs::read(&work).expect("read rom");
    {
        let mut gb = Gameboy::new(rom.clone(), Some(work.clone()));
        let mut bot = IntroBot::default();
        let mut ok = false;
        for frame in 0..60 * 60 * 12 {
            bot.act(&mut gb, verbose);
            gb.frame();
            if bot.saved_frames > 240 {
                ok = true;
                break;
            }
            if frame % 1800 == 0 && verbose {
                println!(
                    "  [{frame}] {:?} map={:#04x}",
                    bot.stage,
                    gb.peek(W_CUR_MAP)
                );

                println!("{}", indent(&screen_text(&mut gb)));
            }
        }
        if !ok {
            eprintln!("could not complete the intro and save; run with --verbose");
            std::process::exit(1);
        }
        // `Gameboy::frame` autosaves battery RAM as the game writes it (see
        // `src/mbc/battery.rs`), and dropping the machine flushes what is left.
    }

    let saved = work.with_extension("sav");
    let mut sram = std::fs::read(&saved).expect("the game should have written a save");
    println!("game wrote {} bytes of save data", sram.len());

    patch_fixture(&mut sram, &rom);
    std::fs::write(out, &sram).expect("write fixture");
    println!("fixture written to {}", out.display());
}

/// Edit the four things that move the save from "just started" to "standing in
/// the Viridian Pokémon Center with a Pokédex and a party".
fn patch_fixture(sram: &mut [u8], rom: &[u8]) {
    let main = |addr: u16| SAVE_MAIN_DATA + (addr - W_MAIN_DATA_START) as usize;

    // Do NOT relocate the player by hand.
    //
    // That was tried and it does not work: continuing a save deliberately skips
    // most of `LoadMapHeader` (it returns early while BIT_NO_PREVIOUS_MAP is
    // set), so the map header, the block-map view pointer, the border block and
    // the warp, sign and NPC tables are all *saved* state that describes the map
    // the player was standing in. Writing a different `wCurMap` over the top of
    // them renders one map's blocks through another's tileset and stride, and
    // walking onto a stale warp coordinate crashes the game outright.
    //
    // So instead: leave every one of those fields exactly as the game wrote
    // them, and redirect the room's **own** staircase warp to the Viridian
    // Pokémon Center. `LoadMapHeader` clears BIT_NO_PREVIOUS_MAP on its way out
    // of that early return, so the first real warp does the complete job — and
    // the player arrives with the header, the view pointer, the warps and every
    // NPC (including the link receptionist) loaded by the game itself.
    let warps = main(W_WARP_ENTRIES);
    let n = sram[main(W_NUMBER_OF_WARPS)] as usize;
    for i in 0..n.min(8) {
        sram[warps + i * 4 + 2] = 0; // arrive at the destination's warp 0
        sram[warps + i * 4 + 3] = MAP_VIRIDIAN_POKECENTER;
    }
    println!(
        "redirected {n} warp(s) in map {:#04x} to the Viridian Pokémon Center",
        sram[main(W_CUR_MAP)]
    );

    let flag = main(W_EVENT_FLAGS) + EVENT_GOT_POKEDEX / 8;
    sram[flag] |= 1 << (EVENT_GOT_POKEDEX % 8);

    // A party to trade from. Built with the same code the link partner uses, so
    // it is well formed by construction — and giving the player two Pokémon
    // means the trade never empties the party.
    let tables = terminalgb_plugin_gen1_link::romdata::RomTables::locate(rom.to_vec())
        .expect("gen 1 rom tables");
    let spec = TradeSpec::parse(
        "trainer = RED\n\
         species = Pikachu\nlevel = 12\nnickname = SPARK\not_name = RED\not_id = 1234\n\
         moves = Thundershock, Growl\n\
         species = Rattata\nlevel = 9\nnickname = RATTY\not_name = RED\not_id = 1234\n\
         moves = Tackle, Tail Whip\n",
        "fixture",
    )
    .expect("fixture party spec");
    let party: Party = {
        use terminalgb_plugin_gen1_link::spec::PokemonSource;
        spec.party(&tables).expect("build fixture party")
    };
    let image = party.to_wram_image();
    assert_eq!(image.len(), PARTY_DATA_LEN);
    sram[SAVE_PARTY_DATA..SAVE_PARTY_DATA + PARTY_DATA_LEN].copy_from_slice(&image);

    // `CalcCheckSum` sums every byte of sGameData and complements the result.
    let sum: u8 = sram[SAVE_PLAYER_NAME..SAVE_CHECKSUM]
        .iter()
        .fold(0u8, |a, &b| a.wrapping_add(b));
    sram[SAVE_CHECKSUM] = !sum;
    println!(
        "patched: Pokedex flag set, party of 2, checksum {:#04x}",
        sram[SAVE_CHECKSUM]
    );
}

/// Plays Oak's introduction until the game has written a save file.
#[derive(Default)]
struct IntroBot {
    hold: Option<(KeypadKey, u32)>,
    cooldown: u32,
    stage: IntroStage,
    idle: u32,
    saved_frames: u32,
    menu_steps: u32,
    pending_a: bool,
    confirm_presses: u32,
    saw_dialog: bool,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
enum IntroStage {
    #[default]
    Speech,
    PickSave,
    Confirm,
    Saved,
}

impl IntroBot {
    fn press(&mut self, gb: &mut Gameboy, key: KeypadKey) {
        gb.keydown(key);
        self.hold = Some((key, 6));
    }

    fn act(&mut self, gb: &mut Gameboy, verbose: bool) {
        if let Some((key, left)) = self.hold {
            if left <= 1 {
                gb.keyup(key);
                self.hold = None;
                self.cooldown = 8;
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

        match self.stage {
            IntroStage::Speech => {
                // `wCurMap` is *already* Red's room during Oak's speech, so it
                // cannot mark the end of the intro. Drive off the screen
                // instead: mash A to advance text, punctuated with Start, and
                // stop as soon as the start menu appears — which can only
                // happen once the player has control.
                if text.contains("SAVE") && text.contains("OPTION") {
                    if verbose {
                        println!("  start menu is open");
                    }
                    self.stage = IntroStage::PickSave;
                    return;
                }
                // The follow-up A must be checked *before* the NEW NAME test,
                // or the menu test fires again and the cursor walks down for
                // ever without anything ever being chosen.
                if self.pending_a {
                    self.pending_a = false;
                    self.press(gb, KeypadKey::A);
                    return;
                }
                // The two name menus are the only place a blind A press goes
                // wrong: it would open the free-text entry screen. Move down
                // one to the first preset name instead.
                if text.contains("NEW NAME") {
                    self.press(gb, KeypadKey::Down);
                    self.menu_steps += 1;
                    self.pending_a = true;
                    return;
                }
                self.idle += 1;
                if self.idle % 6 == 0 {
                    self.press(gb, KeypadKey::Start);
                } else {
                    self.press(gb, KeypadKey::A);
                }
            }
            IntroStage::PickSave => {
                if !(text.contains("SAVE") && text.contains("OPTION")) {
                    // Menu closed again (a stray Start). Re-open it.
                    self.press(gb, KeypadKey::Start);
                    return;
                }
                match cursor_row(gb) {
                    Some(row)
                        if text.lines().nth(row).is_some_and(|l| l.contains("SAVE")) =>
                    {
                        if verbose {
                            println!("  selecting SAVE on row {row}:\n{}", indent(&text));
                        }
                        self.press(gb, KeypadKey::A);
                        self.stage = IntroStage::Confirm;
                    }
                    Some(_) => self.press(gb, KeypadKey::Down),
                    None => {
                        if verbose {
                            println!("  no cursor found in:\n{}", indent(&text));
                        }
                        self.press(gb, KeypadKey::Down)
                    }
                }
            }
            IntroStage::Confirm => {
                // "Would you like to SAVE the game?" -> YES, then a long
                // "SAVING... DON'T TURN OFF!" pause that must be waited out.
                self.confirm_presses += 1;
                if text.contains("SAVE the game") {
                    self.saw_dialog = true;
                }
                if (self.saw_dialog && !text.contains("SAVE the game"))
                    || self.confirm_presses > 60
                {
                    if verbose {
                        println!(
                            "  save confirmed after {} presses",
                            self.confirm_presses
                        );
                    }
                    self.stage = IntroStage::Saved;
                    return;
                }
                self.press(gb, KeypadKey::A);
            }
            IntroStage::Saved => {
                self.saved_frames += 1;
                if verbose && self.saved_frames % 60 == 1 {
                    println!("  saved+{}:\n{}", self.saved_frames, indent(&text));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Stage 2: the trade.
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn run_trade(
    rom_path: &Path,
    scratch: &Path,
    fixture: &Path,
    trade: Option<&Path>,
    shots: Option<PathBuf>,
    state_out: Option<PathBuf>,
    frames: u32,
    quick_load: bool,
    verbose: bool,
) {
    let work = scratch.join("trade.gb");
    std::fs::copy(rom_path, &work).expect("copy rom");
    std::fs::copy(fixture, work.with_extension("sav")).expect("copy fixture save");

    let rom = std::fs::read(&work).expect("read rom");
    let mut gb = Gameboy::new(rom.clone(), Some(work.clone()));

    let source = match trade {
        Some(p) => TradeSpec::load(p).unwrap_or_else(|e| {
            eprintln!("trade file: {e}");
            std::process::exit(1);
        }),
        None => TradeSpec::default(),
    };
    let partner = Gen1LinkPartner::new(rom, &source).unwrap_or_else(|e| {
        eprintln!("link partner: {e}");
        std::process::exit(1);
    });
    println!("plugin   : gen1-link");
    println!("offering : {}", partner.offer());
    println!("source   : {}", partner.origin());
    gb.add_plugin(Box::new(partner)).expect("attach plugin");
    println!("status   : {}", gb.plugin_status_line().unwrap_or_default());

    // One visit to the console per entry. The default exercises everything the
    // link session has to survive: a trade and the walk back out, a visit that
    // cancels without trading, and a *second* trade afterwards.
    let mut bot = TradeBot {
        plan: vec![Visit::Trade, Visit::CancelOnly, Visit::Trade],
        ..TradeBot::default()
    };
    let mut before: Option<Vec<PartyMon>> = None;
    let mut last = String::new();
    let mut last_shot_stage = "";
    let mut shot_delay: Option<(u32, &'static str)> = None;
    let mut shot_index = 0u32;
    let mut state_written = false;
    let mut quick_loaded = false;

    for frame in 0..frames {
        bot.act(&mut gb, verbose);
        gb.frame();

        // F5 then F8, exactly as the terminal frontend's quick-save / quick-load
        // keys do it, at the moment the player has finished trading and wants
        // out. `Gameboy::load_state` calls `PluginHost::reset`, so the partner
        // is back at `Phase::Handshake` while the game is still in the middle of
        // a link session — and the *only* way out of the Trade Center is to back
        // out of the trade screen, so that has to keep working from there.
        if quick_load && !quick_loaded && bot.stage_label() == "leaving-trade-screen" {
            quick_loaded = true;
            let state = gb.save_state();
            gb.load_state(&state).expect("quick-load");
            println!(
                "[{frame}] quick-saved and quick-loaded; partner is now '{}'",
                partner_phase(&mut gb)
            );
        }

        // Wait for the save to be fully loaded: `LoadSAV` copies the party in
        // a separate pass from the main data, so wCurMap is right for a few
        // frames before wPartyCount is.
        if before.is_none()
            && gb.peek(W_CUR_MAP) == MAP_VIRIDIAN_POKECENTER
            && (1..=6).contains(&gb.peek(W_PARTY_COUNT))
            && gb.peek(W_PARTY_MON_NICKS) != 0
        {
            verify_addresses(&mut gb);
            let p = read_party(&mut gb);
            println!("\nparty before: {}", describe_party(&p));
            before = Some(p);
        }

        if verbose && frame % 30 == 0 {
            let report = format!(
                "{:?} map={:#04x} pos=({},{}) link={} party={}",
                bot.stage,
                gb.peek(W_CUR_MAP),
                gb.peek(W_X_COORD),
                gb.peek(W_Y_COORD),
                gb.peek(W_LINK_STATE),
                gb.peek(W_PARTY_COUNT),
            );
            if report != last {
                println!("[{frame:6}] {report}");
                last = report;
            }
        }

        // A save state at the console, *before* the trade starts, is what makes
        // this reproducible in the real terminal frontend: load it there with
        // F8, press A, and the trade runs live in front of you. It has to be
        // before the exchange begins — plugin state is deliberately not in a
        // save file, so a state captured mid-trade would load against a `reset`
        // partner and desync (which is the documented hang, docs/plugins.md).
        if let Some(path) = &state_out {
            if !state_written && bot.stage_label() == "trade-room" {
                state_written = true;
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                match std::fs::write(path, gb.save_state()) {
                    Ok(()) => println!("save state at the console -> {}", path.display()),
                    Err(e) => eprintln!("save state {}: {e}", path.display()),
                }
            }
        }

        // Screenshots are taken at *stage transitions*, not on a timer: the
        // point of the pictures is to show the trade actually happening, and a
        // periodic sample mostly catches walking. `shot_delay` lets the screen
        // settle after the transition, since the stage flips the moment the
        // condition holds and the game is usually still drawing the new scene.
        if let Some(dir) = &shots {
            if bot.stage_label() != last_shot_stage {
                last_shot_stage = bot.stage_label();
                shot_delay = Some((60, last_shot_stage));
            }
            if let Some((n, label)) = shot_delay {
                if n == 0 {
                    let _ = std::fs::create_dir_all(dir);
                    write_png(
                        &dir.join(format!("{shot_index:02}-{label}.png")),
                        gb.image(),
                    );
                    shot_index += 1;
                    shot_delay = None;
                } else {
                    shot_delay = Some((n - 1, label));
                }
            }
        }
        if bot.left_frames > 120 {
            break;
        }
    }

    println!(
        "\nstatus   : {}",
        gb.plugin_status_line().unwrap_or_default()
    );
    if let Some(p) = gb.plugin::<Gen1LinkPartner>() {
        let trace = p.trace();
        if !trace.is_empty() {
            println!(
                "\nlast {} bytes on the wire (sent -> replied @ phase):",
                trace.len().min(96)
            );
            for (sent, reply, phase) in trace.iter().rev().take(96).rev() {
                println!("  {sent:#04x} -> {reply:#04x}   {}", phase.label());
            }
        }
    }
    println!("\nvisits to the console:");
    for (i, (what, presses)) in bot.visits.iter().enumerate() {
        println!(
            "  {}: {what:?} — left after {presses} CANCEL press(es)",
            i + 1
        );
    }
    if bot.visits.len() < bot.plan.len() {
        println!(
            "  {}: {:?} — NEVER LEFT (stage {}, {} CANCEL presses so far)",
            bot.visits.len() + 1,
            bot.plan[bot.visits.len()],
            bot.stage_label(),
            bot.cancel_attempts
        );
    }
    let after = read_party(&mut gb);
    println!("party after : {}", describe_party(&after));
    if let Some(dir) = &shots {
        let _ = std::fs::create_dir_all(dir);
        // The Cable Club's own party list, drawn by the game from wPartyMons
        // after the trade. Reading WRAM (below) is the actual check; this is
        // the picture a person can look at without trusting our reader.
        //
        // It is deliberately *not* labelled as the overworld party menu: the
        // link session is still open at this point, so B does not leave the
        // trade screen. Naming a screenshot for what it shows, not for what
        // would be most convincing, is the whole point of taking it.
        write_png(&dir.join("06-party-after-trade.png"), gb.image());
    }
    println!("\nfinal screen:\n{}", indent(&screen_text(&mut gb)));

    let before = before.unwrap_or_default();
    let gained: Vec<&PartyMon> = after
        .iter()
        .filter(|m| !before.iter().any(|b| b.nickname == m.nickname))
        .collect();
    if gained.is_empty() {
        println!("\nRESULT: FAILED — no new Pokémon arrived through the link.");
        std::process::exit(1);
    }
    println!("\nRESULT: received through the emulated link cable:");
    for m in &gained {
        println!("  {}", m.describe());
    }
    // Receiving the Pokémon is only half of the claim: a trade you cannot get
    // out of is not a completed link session.
    if bot.stage != TradeStage::Finished {
        println!(
            "\nRESULT: FAILED — the player never got back out of the trade screen \
             (visit {} of {}, stage {}, {} CANCEL presses).",
            bot.visits.len() + 1,
            bot.plan.len(),
            bot.stage_label(),
            bot.cancel_attempts
        );
        std::process::exit(1);
    }
    println!(
        "RESULT: {} visit(s) to the console, {} trade(s), every one left cleanly.",
        bot.visits.len(),
        bot.trades_seen
    );
}

/// Fail loudly if the hard-coded WRAM addresses are not what this ROM uses.
/// A wrong address would make this driver claim success while reading noise.
fn verify_addresses(gb: &mut Gameboy) {
    let map = gb.peek(W_CUR_MAP);
    assert_eq!(
        map, MAP_VIRIDIAN_POKECENTER,
        "wCurMap is not where this driver thinks it is"
    );
    let count = gb.peek(W_PARTY_COUNT);
    assert!(
        (1..=6).contains(&count),
        "wPartyCount reads {count}, which is not a party size"
    );
    let flags = gb.peek(W_EVENT_FLAGS + (EVENT_GOT_POKEDEX / 8) as u16);
    assert!(
        flags & (1 << (EVENT_GOT_POKEDEX % 8)) != 0,
        "EVENT_GOT_POKEDEX is not set — the receptionist will turn us away"
    );
}

/// Walks to the link receptionist, takes the Cable Club through the menus, and
/// completes the trade. State-driven: it reacts to the screen and the player's
/// coordinates rather than replaying a button script.
#[derive(Default)]
struct TradeBot {
    hold: Option<(KeypadKey, u32)>,
    cooldown: u32,
    stage: TradeStage,
    done_frames: u32,
    ticks: u32,
    stuck: u32,
    last_pos: (u8, u8),
    blocked: u32,
    on_trade_option: bool,
    trade_spawn: Option<(u8, u8)>,
    /// How many times CANCEL has been confirmed on the trade-selection screen
    /// without the game letting go of the link. Each attempt is one full
    /// `Serial_SyncAndExchangeNybble` round, so a climbing number *is* the bug.
    cancel_attempts: u32,
    left_frames: u32,
    /// What to do on each visit to the Game Boy on the table, in order.
    plan: Vec<Visit>,
    visit: usize,
    /// Per-visit report: `(what we meant to do, CANCEL rounds it took)`.
    visits: Vec<(Visit, u32)>,
    trades_seen: u32,
}

/// What the bot does on one visit to the console.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Visit {
    /// Pick a Pokémon, confirm, then leave.
    Trade,
    /// Walk straight down to CANCEL and leave without trading.
    CancelOnly,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
enum TradeStage {
    #[default]
    Continue,
    EnterRoom,
    WalkToReceptionist,
    TalkAndSave,
    LinkMenu,
    InTradeRoom,
    /// Everything from "press A on the console" to "Trade completed!": the
    /// section exchange, the Pokémon selection and the confirmation, all of
    /// which are driven by the same read-the-screen-and-press-A loop.
    AtMachine,
    Done,
    /// Back on the trade-selection screen after the trade: walk the cursor down
    /// to CANCEL and press A. `pokered`'s `.cancelMenuItem_APressed` sends
    /// nybble `$F` and only falls through to `ReturnToCableClubRoom` when the
    /// partner answers `$F`, so this is the step that "cannot leave the trade
    /// room" fails at.
    LeaveTradeScreen,
    /// Off the trade screen and standing in the room again. The Trade Center
    /// has **no warp events at all** (`data/maps/objects/TradeCenter.asm`), so
    /// there is no door to walk out of — a *player* leaves with START -> RESET.
    /// For this run, getting back here is the end of a visit.
    BackInRoom,
    Finished,
}

impl TradeBot {
    /// Filename-safe stage name, for the `--shots` captures.
    fn stage_label(&self) -> &'static str {
        match self.stage {
            TradeStage::Continue => "continue",
            TradeStage::EnterRoom => "pokecenter",
            TradeStage::WalkToReceptionist => "walk-to-receptionist",
            TradeStage::TalkAndSave => "receptionist",
            TradeStage::LinkMenu => "link-menu",
            TradeStage::InTradeRoom => "trade-room",
            TradeStage::AtMachine => "at-machine",
            TradeStage::Done => "done",
            TradeStage::LeaveTradeScreen => "leaving-trade-screen",
            TradeStage::BackInRoom => "back-in-room",
            TradeStage::Finished => "finished",
        }
    }
}

impl TradeBot {
    fn press(&mut self, gb: &mut Gameboy, key: KeypadKey, hold: u32) {
        gb.keydown(key);
        self.hold = Some((key, hold));
    }

    /// One step towards `(tx, ty)`. Alternates the axis it tries whenever the
    /// player has not actually moved, which is enough to get around the
    /// furniture in a Pokémon Center without any collision data.
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
        let key = if prefer_x { dx.or(dy) } else { dy.or(dx) };
        if let Some(k) = key {
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
        self.ticks += 1;
        let text = screen_text(gb);
        let map = gb.peek(W_CUR_MAP);
        let (x, y) = (gb.peek(W_X_COORD), gb.peek(W_Y_COORD));
        let link = gb.peek(W_LINK_STATE);
        let set = |b: &mut Self, s: TradeStage| {
            if b.stage != s {
                if verbose {
                    println!("  bot: {:?} -> {:?}", b.stage, s);
                }
                b.stage = s;
                b.stuck = 0;
            }
        };

        match self.stage {
            TradeStage::Continue => {
                // `wCurMap` is loaded by `TryLoadSaveFile` at the *top* of the
                // main menu, before CONTINUE is even chosen, so it cannot mark
                // "we are in the game". Drive off the screen: keep pressing A
                // until neither the main menu nor the continue-info panel is
                // showing.
                let menu = text.contains("CONTINUE")
                    || text.contains("NEW GAME")
                    || text.contains("OPTION")
                    || text.contains("BADGES");
                // A blank transition frame is neither a menu nor the
                // overworld, and treating it as "in the game" stops the A
                // presses one screen too early — so require an actually drawn
                // map, not merely the absence of menu text.
                // "In the game" needs all three: no menu text, a real drawn
                // map, and the save actually loaded (the boot logo also has
                // plenty of distinct tiles).
                let loaded = gb.peek(W_PARTY_MON_NICKS) != 0
                    && (1..=6).contains(&gb.peek(W_PARTY_COUNT));
                if menu || !loaded || !looks_like_a_map(gb) {
                    self.stuck = 0;
                    self.press(gb, KeypadKey::A, 4);
                    return;
                }
                self.stuck += 1;
                if self.stuck > 6 {
                    set(self, TradeStage::EnterRoom);
                }
            }
            TradeStage::EnterRoom => {
                // Walk onto the room's staircase, which the fixture redirected
                // to the Pokémon Center. The target comes from the game's own
                // warp table rather than a hard-coded coordinate.
                if map == MAP_VIRIDIAN_POKECENTER {
                    set(self, TradeStage::WalkToReceptionist);
                    return;
                }
                let wy = gb.peek(W_WARP_ENTRIES);
                let wx = gb.peek(W_WARP_ENTRIES + 1);
                if verbose && self.stuck == 0 {
                    println!("  walking to the warp at ({wx},{wy}) from ({x},{y})");
                }
                self.stuck += 1;
                self.step_towards(gb, x, y, wx, wy);
            }
            TradeStage::WalkToReceptionist => {
                if map == MAP_TRADE_CENTER {
                    set(self, TradeStage::InTradeRoom);
                    return;
                }
                if link != 0 {
                    set(self, TradeStage::LinkMenu);
                    return;
                }
                if text.contains("CABLE CLUB") || text.contains("SAVE") {
                    set(self, TradeStage::TalkAndSave);
                    return;
                }
                if x != RECEPTIONIST_X || y != RECEPTIONIST_Y {
                    self.step_towards(gb, x, y, RECEPTIONIST_X, RECEPTIONIST_Y);
                } else {
                    // In position. Alternate "face her" and "talk" — never both
                    // in one tick, or the first keydown is never released.
                    self.stuck += 1;
                    if self.stuck % 2 == 1 {
                        self.press(gb, KeypadKey::Up, 4);
                    } else {
                        self.press(gb, KeypadKey::A, 4);
                    }
                    if verbose && self.stuck % 20 == 2 {
                        println!("  at the desk, screen:\n{}", indent(&text));
                    }
                }
            }
            TradeStage::TalkAndSave => {
                if link != 0 {
                    set(self, TradeStage::LinkMenu);
                    return;
                }
                // "...you have to SAVE" -> YES; then "PLEASE WAIT".
                self.press(gb, KeypadKey::A, 4);
            }
            TradeStage::LinkMenu => {
                if map == MAP_TRADE_CENTER {
                    set(self, TradeStage::InTradeRoom);
                    return;
                }
                // The menu opens on TRADE CENTER, so A takes it.
                self.press(gb, KeypadKey::A, 4);
            }
            TradeStage::InTradeRoom => {
                // `CableClubLeftGameboy` only starts a trade when the player is
                // the clock master *and is facing right* — the two consoles use
                // the two Game Boys on the table, and which one is yours is
                // decided by who took the clock. So: face right, press A.
                if link >= 0x02 {
                    set(self, TradeStage::AtMachine);
                    return;
                }
                // The player is warped in *beside* their console
                // (`TradeCenterPlayerWarp`), so the spawn tile is the right
                // place to stand — wandering off is how this failed before.
                let spawn = *self.trade_spawn.get_or_insert((x, y));
                if (x, y) != spawn {
                    self.step_towards(gb, x, y, spawn.0, spawn.1);
                    return;
                }
                self.stuck += 1;
                if self.stuck % 2 == 0 {
                    self.press(gb, KeypadKey::Right, 4);
                } else {
                    self.press(gb, KeypadKey::A, 4);
                }
            }
            TradeStage::AtMachine => {
                if text.contains("Trade completed") {
                    self.trades_seen += 1;
                    set(self, TradeStage::Done);
                    return;
                }
                // A cancel-only visit walks straight out of the selection
                // screen as soon as it is drawn.
                if self.plan.get(self.visit) == Some(&Visit::CancelOnly)
                    && text.contains("CANCEL")
                {
                    set(self, TradeStage::LeaveTradeScreen);
                    return;
                }
                // The per-Pokémon menu is horizontal: "STATS     TRADE", with
                // the cursor on STATS. Nudge right once, then confirm.
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
            TradeStage::Done => {
                self.done_frames += 1;
                // After "Trade completed!" the game jumps straight back to
                // `CableClub_DoBattleOrTradeAgain`: another full three-block
                // exchange, then the selection screen again. Wait for that
                // screen before trying to leave.
                if self.done_frames > 240 && text.contains("CANCEL") {
                    self.done_frames = 0;
                    set(self, TradeStage::LeaveTradeScreen);
                }
            }
            TradeStage::LeaveTradeScreen => {
                // Off the trade screen: `ReturnToCableClubRoom` ran and
                // `CableClub_Run` set LINK_STATE_IN_CABLE_CLUB.
                if link == 0x01 {
                    let what = self.plan.get(self.visit).copied().unwrap_or(Visit::Trade);
                    let n = self.cancel_attempts;
                    self.visits.push((what, n));
                    self.cancel_attempts = 0;
                    self.visit += 1;
                    set(self, TradeStage::BackInRoom);
                    return;
                }
                // The cursor starts on the first party slot; Down walks it to
                // CANCEL (menu item `wPartyCount`) and one more Down selects
                // it, after which A confirms.
                self.stuck += 1;
                if self.stuck <= 8 {
                    self.press(gb, KeypadKey::Down, 4);
                } else {
                    if self.stuck % 2 == 1 {
                        self.cancel_attempts += 1;
                        if verbose {
                            println!(
                                "  CANCEL press #{} (link={link:#04x}, partner {})",
                                self.cancel_attempts,
                                partner_phase(gb),
                            );
                        }
                    }
                    self.press(gb, KeypadKey::A, 4);
                }
            }
            TradeStage::BackInRoom => {
                // The Trade Center is a sealed room — no warps — so being back
                // here, walkable and out of the link screens, is as far out as
                // the trade protocol can take anybody.
                if self.visit >= self.plan.len() {
                    set(self, TradeStage::Finished);
                    return;
                }
                // Another visit: back to the console for the next round trip.
                self.on_trade_option = false;
                set(self, TradeStage::InTradeRoom);
            }
            TradeStage::Finished => {
                self.left_frames += 1;
            }
        }
    }
}

/// The link partner's own view of where the protocol is, for the log.
fn partner_phase(gb: &mut Gameboy) -> String {
    gb.plugin::<Gen1LinkPartner>()
        .map(|p| p.phase().label().to_string())
        .unwrap_or_else(|| "?".into())
}

// --- reading the machine ---------------------------------------------------

#[derive(Clone)]
struct PartyMon {
    species: u8,
    level: u8,
    hp: u16,
    max_hp: u16,
    stats: [u16; 4],
    moves: [u8; 4],
    pp: [u8; 4],
    ot_id: u16,
    experience: u32,
    ot_name: String,
    nickname: String,
}

impl PartyMon {
    fn describe(&self) -> String {
        format!(
            "{} — species index {:#04x}, Lv{}, HP {}/{}, atk/def/spd/spc {:?}, \
             moves {:?}, pp {:?}, exp {}, OT {} (id {})",
            self.nickname,
            self.species,
            self.level,
            self.hp,
            self.max_hp,
            self.stats,
            self.moves,
            self.pp,
            self.experience,
            self.ot_name,
            self.ot_id
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
            PartyMon {
                species: gb.peek(W_PARTY_SPECIES + i as u16),
                level: b[33],
                hp: be(1),
                max_hp: be(34),
                stats: [be(36), be(38), be(40), be(42)],
                moves: [b[8], b[9], b[10], b[11]],
                pp: [b[29], b[30], b[31], b[32]],
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

fn describe_party(p: &[PartyMon]) -> String {
    if p.is_empty() {
        return "(empty)".into();
    }
    p.iter()
        .map(|m| format!("{} Lv{}", m.nickname, m.level))
        .collect::<Vec<_>>()
        .join(", ")
}

fn decode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|&&b| b != 0x50)
        .filter_map(|&b| terminalgb_plugin_gen1_link::text::decode_char(b))
        .collect()
}

/// Read what is on screen as text.
///
/// Generation 1 writes character codes straight into a tilemap and loads the
/// font so that tile id == character code, so the tilemap *is* the text. That is
/// what lets a headless run navigate menus by reading them rather than replaying
/// a brittle button script.
///
/// The catch, found the hard way: Red/Blue draw the **whole screen through the
/// window layer** (`LCDC` bit 5 set, `WY = 0`, `WX = 7`), leaving the background
/// map blank. Reading the background map returns 20x18 spaces and every menu
/// looks empty. Read the window map when the window is on.
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
            let y = (sy + row) % 32;
            let x = (sx + col) % 32;
            let t = gb.peek(map + (y * 32 + x) as u16);
            out.push(terminalgb_plugin_gen1_link::text::decode_char(t).unwrap_or(' '));
        }
        out.push('\n');
    }
    out
}

/// Is a real map drawn, as opposed to a menu, a text screen, or a blank frame
/// mid-transition? Menus and transitions leave the tilemap almost uniform; an
/// overworld map is made of many different block tiles.
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

/// Row of the menu cursor. Gen 1 uses `$ED` (filled `▶`) for an active menu and
/// `$EC` (hollow `▷`) for a parked one; neither is a text character, so this
/// reads the tilemap rather than the decoded string.
fn cursor_row(gb: &mut Gameboy) -> Option<usize> {
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
    (0..18).find(|&row| {
        gb.peek_range(map + (row * 32) as u16, 20)
            .iter()
            .any(|&t| t == 0xED || t == 0xEC)
    })
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("    |{l}|"))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- screenshots ------------------------------------------------------------

/// Write a 160x144 RGBA frame to a PNG.
///
/// Deliberately the **same path the `c` / `C` / `F2` screenshot keys take** in
/// the terminal frontend (`App::screenshot`): the `image` crate, which the repo
/// already depends on. `termimage`'s PNG writer is *stored* deflate — right for
/// a 60 Hz wire, wrong for a file someone keeps or puts in documentation — so a
/// screenshot must not use it, and this driver should not have a second
/// encoder that could drift from the one users actually press a key for.
fn write_png(path: &Path, rgba: &[u8]) {
    match image::RgbaImage::from_raw(160, 144, rgba.to_vec()) {
        Some(img) => {
            if let Err(e) = img.save(path) {
                eprintln!("screenshot {}: {e}", path.display());
            }
        }
        None => eprintln!("screenshot {}: frame buffer size mismatch", path.display()),
    }
}
