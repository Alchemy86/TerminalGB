//! Fetch this player's pending portal trade and arm a link partner with it —
//! the whole client path (`sign-in → trades/mine → resolve against the real
//! cartridge → partner constructed`), runnable by hand against any portal.
//!
//! ```text
//! GB_PORTAL=<url> cargo run --release --example portal_trade -- <rom.gb> [--done]
//! ```
//!
//! Sign in first (`terminal login`). The ROM is **copied** into `target/`
//! before anything touches it — never point an emulator at a ROM beside its
//! real save. `--done` additionally POSTs the trade's completion, so the
//! whole contract can be walked against a stub or the live portal.
//!
//! This is also the arm-path proof the test harness deliberately cannot give:
//! `testharness/portal.rs` must not link the plugin crate directly (the
//! two-rlib trap recorded in `AGENTS.md`), so constructing a real
//! `Gen1LinkPartner` from a `PortalSource` happens here, where examples and
//! the lib share one build graph.

use terminalgb::portal::{client::Portal, portal_url, source::PortalSource};
use terminalgb_plugin_gen1_link::Gen1LinkPartner;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rom_arg = args.iter().find(|a| !a.starts_with("--"));
    let done = args.iter().any(|a| a == "--done");
    let Some(rom_path) = rom_arg else {
        eprintln!("usage: GB_PORTAL=<url> portal_trade <rom.gb> [--done]");
        std::process::exit(2);
    };
    let Some(url) = portal_url(None) else {
        eprintln!("portal_trade: set GB_PORTAL=<url> (and `terminal login` first)");
        std::process::exit(2);
    };

    // Stage a copy — the MBC writes a .sav beside whatever ROM path it is
    // given, and this example must never write next to a real cartridge.
    let staged = std::path::Path::new("target").join("portal-trade.gb");
    std::fs::copy(rom_path, &staged).expect("stage the ROM into target/");
    let rom = std::fs::read(&staged).expect("read the staged ROM");

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
    match Gen1LinkPartner::new(rom, &src) {
        Ok(partner) => {
            println!("  receive: {}  (partner armed)", partner.offer());
            println!("  origin:  {}", partner.origin());
        }
        Err(e) => {
            eprintln!(
                "portal_trade: trade #{trade_id} cannot be armed on this cartridge: {e}"
            );
            std::process::exit(1);
        }
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
