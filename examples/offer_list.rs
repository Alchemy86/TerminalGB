//! List everything a loaded save can offer to the trade portal — the
//! evidence tool for the offer-from-save flow (`docs/portal.md`).
//!
//! Boots a cartridge (staged into `target/` first, with its save copied
//! beside it, so nothing ever writes next to the real ROM), mashes through
//! the title screen until the save is loaded, then reads the party and every
//! PC box through exactly the code path the TUI's `O` key uses:
//! `collection::read_raw` over `Gameboy::with_machine`, decoded against the
//! cartridge's own tables, each entry carrying its tradeability verdict and
//! the would-be-shiny-in-Gen-2 flag.
//!
//! ```text
//! cargo run --release --example offer_list -- <rom.gb> [--save <file.sav>] [--json]
//! ```

use terminalgb::gameboy::Gameboy;
use terminalgb::plugin::Capability;
use terminalgb_plugin_trainer::{collection, TrainerPlugin};

// The shared Cable Club bot: its Continue stage is the screen-driven way to
// boot a Gen 1 cartridge into its save (a blind button masher stalls — see
// the module's own doc and docs/gen1/).
#[path = "support/gen1_cable_club.rs"]
mod cable_club;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut rom_path = None;
    let mut save_path = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--save" => save_path = args.next(),
            other => rom_path = Some(other.to_string()),
        }
    }
    let Some(rom_path) = rom_path else {
        eprintln!("usage: offer_list <rom.gb> [--save <file.sav>]");
        std::process::exit(2);
    };

    // Stage into target/ with the shared helper: the MBC writes a `.sav`
    // beside the ROM path it is given (the *extension replaced*, so
    // `game.gb` saves to `game.sav`), and it must never write beside the
    // user's real ROM.
    let rom_file = std::path::PathBuf::from(&rom_path);
    let save_file = save_path
        .as_deref()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let beside = rom_file.with_extension("sav");
            beside.is_file().then_some(beside)
        });
    let staged = cable_club::stage(
        "offer-list",
        &rom_file,
        save_file.as_deref(),
        std::path::Path::new("target"),
    );

    let rom = std::fs::read(&staged.rom).expect("read ROM");
    let mut gb = Gameboy::new(rom.clone(), Some(staged.rom.clone()));
    let mut trainer = TrainerPlugin::new(rom);
    trainer.set_active(false);
    gb.add_plugin(Box::new(trainer)).expect("attach trainer");

    let read_caps = Capability::OBSERVE.union(Capability::READ_MEMORY);
    // Boot into the save with the shared bot's screen-driven Continue stage;
    // the moment it moves past "continue" the save is loaded and the player
    // is standing on a real map.
    let mut bot = cable_club::TradeBot::default();
    let mut raw = None;
    for _ in 0..8000u32 {
        bot.act(&mut gb, std::env::var("OFFER_LIST_DEBUG").is_ok(), "offer");
        gb.frame();
        if bot.stage_label() != "continue" {
            raw = gb.with_machine(read_caps, collection::read_raw).ok();
            break;
        }
    }
    let Some(raw) = raw else {
        eprintln!("the save never loaded — is this a Gen 1 cartridge with a save?");
        std::process::exit(1);
    };

    let plugin = gb
        .plugin::<TrainerPlugin>()
        .expect("the trainer plugin is attached");
    if let Some(why) = plugin.refusal() {
        eprintln!("refused: {why}");
        std::process::exit(1);
    }
    let col = collection::decode(&raw, plugin.tables());

    println!(
        "{}'s Pokémon (id {:05}) — {} entries, read from the loaded save",
        col.player_name,
        col.player_id,
        col.entries.len()
    );
    for e in &col.entries {
        let star = if e.shiny_if_traded { " ★" } else { "  " };
        let verdict = match &e.refusal {
            None => String::new(),
            Some(why) => format!("  [cannot offer: {why}]"),
        };
        println!(
            "  {:>3}  {:<10} {:<10} Lv{:<3} DV{:>2} OT {:<8} {:>5}{star}{verdict}",
            e.location.label(),
            e.nickname,
            e.species_name,
            e.level,
            e.dv_total(),
            e.ot_name,
            e.ot_id,
        );
    }
    for n in &col.notes {
        println!("  note: {n}");
    }
    println!("  ★ would be shiny if traded to Gen 2 — Gen 1 itself has no shinies");
}
