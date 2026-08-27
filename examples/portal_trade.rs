//! Fetch this player's pending portal trade and arm a link partner with it —
//! the whole client path (`sign-in → trades/mine → resolve against the real
//! cartridge → partner constructed`), runnable by hand against any portal.
//!
//! ```text
//! GB_PORTAL=<url> cargo run --release --example portal_trade -- <rom.gb> [--done]
//! GB_PORTAL=<url> cargo run --release --example portal_trade -- <rom.gb> \
//!     --play --save <cableclub.sav> [--shots <dir>] [--frames N] [--verbose]
//! ```
//!
//! Sign in first (`terminal login`). The ROM is **copied** into `target/`
//! before anything touches it — never point an emulator at a ROM beside its
//! real save. `--done` additionally POSTs the trade's completion, so the
//! whole contract can be walked against a stub or the live portal.
//!
//! `--play` goes all the way: it boots the staged cartridge on the supplied
//! Cable Club save, walks the player to the Trade Center with the shared bot,
//! completes the trade against the portal-armed partner, verifies the
//! arriving Pokémon **in the party** field by field against the portal's
//! spec, and only then POSTs `/done`. That is the end-to-end proof the
//! arm-only path cannot give.
//!
//! This is also the arm-path proof the test harness deliberately cannot give:
//! `testharness/portal.rs` must not link the plugin crate directly (the
//! two-rlib trap recorded in `AGENTS.md`), so constructing a real
//! `Gen1LinkPartner` from a `PortalSource` happens here, where examples and
//! the lib share one build graph.

use std::path::PathBuf;

use terminalgb::gameboy::Gameboy;
use terminalgb::portal::{client::Portal, portal_url, source::PortalSource};
use terminalgb_plugin_gen1_link::Gen1LinkPartner;

/// The Cable Club bot, save staging and screen reader shared with
/// `link_two.rs` and `bgb_link.rs` — one bot, comparable proofs.
#[path = "support/gen1_cable_club.rs"]
mod cable_club;

fn main() {
    let mut rom_path: Option<PathBuf> = None;
    let mut save_path: Option<PathBuf> = None;
    let mut shots: Option<PathBuf> = None;
    let mut done = false;
    let mut play = false;
    let mut verbose = false;
    let mut frames: u32 = 60 * 60 * 8;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--done" => done = true,
            "--play" => play = true,
            "--verbose" => verbose = true,
            "--save" => save_path = args.next().map(PathBuf::from),
            "--shots" => shots = args.next().map(PathBuf::from),
            "--frames" => {
                frames = args.next().and_then(|v| v.parse().ok()).unwrap_or(frames)
            }
            other if !other.starts_with("--") => rom_path = Some(PathBuf::from(other)),
            other => {
                eprintln!("portal_trade: unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(rom_path) = rom_path else {
        eprintln!(
            "usage: GB_PORTAL=<url> portal_trade <rom.gb> [--done] \
             [--play --save <cableclub.sav>] [--shots <dir>] [--frames N] [--verbose]"
        );
        std::process::exit(2);
    };
    let Some(url) = portal_url(None) else {
        eprintln!("portal_trade: set GB_PORTAL=<url> (and `terminal login` first)");
        std::process::exit(2);
    };

    // Stage a copy — the MBC writes a .sav beside whatever ROM path it is
    // given, and this example must never write next to a real cartridge.
    let scratch = PathBuf::from("target/portal-trade");
    let staged = cable_club::stage("game", &rom_path, save_path.as_deref(), &scratch);
    let rom = std::fs::read(&staged.rom).expect("read the staged ROM");

    let portal = match Portal::connect(&url) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("portal_trade: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "connected to {url} as {}",
        portal.user().unwrap_or("(name unknown)")
    );

    let src = match PortalSource::fetch(&portal) {
        Ok(Some(s)) => s,
        Ok(None) => {
            println!("no pending trade — nothing to arm");
            return;
        }
        Err(e) => {
            eprintln!("portal_trade: {e}");
            std::process::exit(1);
        }
    };
    println!("pending: {}", src.describe_line());
    println!("  give:    {}", src.give);

    // The real resolution, against the real cartridge's own tables — the
    // step that refuses a Pokémon this game cannot hold.
    let trade_id = src.trade_id.clone();
    let partner = match Gen1LinkPartner::new(rom.clone(), &src) {
        Ok(partner) => {
            println!("  receive: {}  (partner armed)", partner.offer());
            println!("  origin:  {}", partner.origin());
            partner
        }
        Err(e) => {
            eprintln!(
                "portal_trade: trade #{trade_id} cannot be armed on this cartridge: {e}"
            );
            std::process::exit(1);
        }
    };

    if play {
        if save_path.is_none() {
            eprintln!(
                "portal_trade: --play needs --save <cableclub.sav> — a save with \
                 the player standing in the Viridian Pokémon Center"
            );
            std::process::exit(2);
        }
        if !play_trade(&staged, partner, &src, shots.as_deref(), frames, verbose) {
            std::process::exit(1);
        }
        // The trade landed and was verified in the party — that is the moment
        // the portal is told, mirroring the TUI's automatic confirmation.
        match portal.trade_done(&trade_id) {
            Ok(()) => println!("confirmed: POST /api/trades/{trade_id}/done"),
            Err(e) => {
                eprintln!("portal_trade: the trade landed but confirming failed: {e}");
                eprintln!("  confirm it on the portal website instead.");
                std::process::exit(1);
            }
        }
        return;
    }

    if done {
        match portal.trade_done(&trade_id) {
            Ok(()) => println!("confirmed: POST /api/trades/{trade_id}/done"),
            Err(e) => {
                eprintln!("portal_trade: confirming failed: {e}");
                std::process::exit(1);
            }
        }
    }
}

/// Boot the staged cartridge, attach the armed partner, drive the Cable Club
/// bot until the trade completes, and verify the arrived Pokémon in the party
/// against the portal's spec. Returns whether every check held.
fn play_trade(
    staged: &cable_club::Staged,
    partner: Gen1LinkPartner,
    src: &PortalSource,
    shots: Option<&std::path::Path>,
    frames: u32,
    verbose: bool,
) -> bool {
    let rom_bytes = std::fs::read(&staged.rom).expect("staged rom");
    let mut gb = Gameboy::new(rom_bytes, Some(staged.rom.clone()));
    // A machine with no APU is not a quiet Game Boy, it is one with the sound
    // chip pulled out, and a cartridge can tell — see `examples/blackout_probe.rs`.
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    gb.add_plugin(Box::new(partner)).expect("attach partner");

    let before = cable_club::read_party(&mut gb);
    println!("party before:");
    for m in &before {
        println!("  {}", m.describe());
    }

    if let Some(dir) = shots {
        std::fs::create_dir_all(dir).expect("shots dir");
    }
    let mut bot = cable_club::TradeBot::default();
    let mut last_stage = "";
    let mut shot_index = 0u32;
    let mut ran = 0u32;
    while ran < frames && !bot.finished() {
        bot.act(&mut gb, verbose, "P");
        gb.frame();
        ran += 1;
        let label = bot.stage_label();
        if label != last_stage {
            if verbose {
                println!("[{ran:>6}] stage: {label}");
            }
            if let Some(dir) = shots {
                cable_club::write_one(
                    &dir.join(format!("{shot_index:02}-{label}.png")),
                    &mut gb,
                );
                shot_index += 1;
            }
            last_stage = label;
        }
    }
    if !bot.finished() {
        eprintln!(
            "portal_trade: the trade did not complete within {frames} frames \
             (stage: {})",
            bot.stage_label()
        );
        return false;
    }
    println!("trade completed in {ran} frames");

    let after = cable_club::read_party(&mut gb);
    println!("party after:");
    for m in &after {
        println!("  {}", m.describe());
    }

    // The arriving Pokémon, field by field against the portal's spec. The
    // nickname is how it is found; everything else must then agree.
    let want = src.receive();
    let want_nick = if want.nickname.is_empty() {
        want.species.to_ascii_uppercase()
    } else {
        want.nickname.to_ascii_uppercase()
    };
    let Some(got) = after.iter().find(|m| {
        m.nickname == want_nick && !before.iter().any(|b| b.nickname == m.nickname)
    }) else {
        eprintln!("portal_trade: {want_nick} did not arrive in the party");
        return false;
    };
    let mut ok = true;
    let mut check = |what: &str, held: bool, detail: String| {
        println!("  [{}] {what}: {detail}", if held { "ok" } else { "FAIL" });
        ok &= held;
    };
    check(
        "level",
        got.level == want.level,
        format!("{} (wanted {})", got.level, want.level),
    );
    check(
        "dvs",
        got.dvs == want.dvs,
        format!("{:?} (wanted {:?})", got.dvs, want.dvs),
    );
    check(
        "ot id",
        got.ot_id == want.ot_id,
        format!("{} (wanted {})", got.ot_id, want.ot_id),
    );
    check(
        "ot name",
        got.ot_name == want.ot_name.to_ascii_uppercase(),
        format!("{} (wanted {})", got.ot_name, want.ot_name),
    );
    ok
}
