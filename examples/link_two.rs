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
use terminalgb::link::peer::{LinkPeer, PeerState};
use terminalgb::link::LinkCable;

/// The Cable Club bot, the save staging and the screen reader, shared with
/// `examples/bgb_link.rs` — the same trade, over a socket instead of a
/// process-local wire. Two copies of this would eventually be two different
/// bots, and the proofs would stop being comparable.
#[path = "support/gen1_cable_club.rs"]
mod cable_club;

use cable_club::*;

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
    // Which half of the pair this *process* is, when the two consoles are two
    // separate programs joined by `LinkPeer` rather than two cores in one.
    let mut peer_side: Option<String> = None;

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
            "--peer" => peer_side = args.next(),
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

    // --- one process, one console, an automatically discovered partner -----
    if let Some(side) = peer_side {
        let me = if side == "b" { &b } else { &a };
        run_as_peer(me, &side, trade, frames, shots.as_deref(), verbose);
        return;
    }

    let mut gb_a =
        Gameboy::new(std::fs::read(&a.rom).expect("rom a"), Some(a.rom.clone()));
    // A machine with no APU is not a quiet Game Boy, it is one with the sound
    // chip pulled out, and a cartridge can tell — see `examples/blackout_probe.rs`.
    gb_a.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    let mut gb_b =
        Gameboy::new(std::fs::read(&b.rom).expect("rom b"), Some(b.rom.clone()));
    // Same rule as above: a machine with no APU is not a quiet Game Boy.
    gb_b.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
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
// Peer mode: this process is ONE console, and the other one is another
// process. Same bot, same reporting — only the transport differs, which is the
// point: if the trade completes over both, the bot is not the thing making it
// work.
// ---------------------------------------------------------------------------

fn run_as_peer(
    me: &Staged,
    side: &str,
    trade: bool,
    frames: u32,
    shots: Option<&Path>,
    verbose: bool,
) {
    let mut gb = Gameboy::new(std::fs::read(&me.rom).expect("rom"), Some(me.rom.clone()));
    // Same rule as above: a machine with no APU is not a quiet Game Boy.
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    let title = gb.rom_title();
    let sav = me.rom.with_extension("sav");
    let mut peer = LinkPeer::discover(&title, Some(&sav));
    println!("[{side}] {title} — {:?}", peer.state());

    if let Some(dir) = shots {
        std::fs::create_dir_all(dir).expect("shots dir");
    }
    let mut bot = trade.then(TradeBot::default);
    let mut last = String::new();
    let mut last_peer = format!("{:?}", peer.state());

    for frame in 0..frames {
        if let Some(b) = &mut bot {
            b.act(&mut gb, verbose, side);
        }
        peer.run_frame(&mut gb);

        let now_peer = format!("{:?}", peer.state());
        if now_peer != last_peer {
            println!("[{side}][{frame:6}] cable: {now_peer}");
            last_peer = now_peer;
        }
        if let Some(b) = &bot {
            let label = b.stage_label().to_string();
            if label != last {
                println!(
                    "[{side}][{frame:6}] {label:<14} transfers {:<5} clock {:#04x}",
                    peer.stats().transfers,
                    gb.peek(H_SERIAL_CONNECTION_STATUS),
                );
                if let Some(dir) = shots {
                    write_one(&dir.join(format!("{side}-{label}.png")), &mut gb);
                }
                last = label;
            }
            if b.finished() {
                println!("[{side}] bot finished at frame {frame}");
                break;
            }
        }
        if gb.fault().is_some() {
            eprintln!("[{side}] FAULT: {:?}", gb.fault());
            break;
        }
    }

    let st = peer.stats();
    println!(
        "[{side}] cable: {} transfers, {} unanswered, {} torn",
        st.transfers, st.unanswered, st.torn
    );
    if let PeerState::Paired {
        title,
        label,
        same_save,
    } = peer.state()
    {
        println!(
            "[{side}] partner: {title}{}{}",
            if label.is_empty() {
                String::new()
            } else {
                format!(" ({label})")
            },
            if *same_save { "  [SAME SAVE FILE]" } else { "" }
        );
    }
    println!("[{side}] party:");
    for m in read_party(&mut gb) {
        println!("[{side}]   {}", m.describe());
    }
    // Persist whatever the game wrote, so the checksum can be verified from
    // outside the process.
    gb.flush_battery();
    peer.close();

    if !trade {
        return;
    }
    // **Check the bytes, not just that something arrived.** A lost byte on the
    // wire shows up as `$FF` inside a name, and `$FF` is not a character — so a
    // decoder that skips unknown bytes turns a corrupt "GLIMMER" into a
    // plausible "MMER" and a corrupt "RED" into "D". That is exactly how the
    // first cross-process run looked fine and was not, so the expected
    // Pokémon is named here and compared in full.
    let want = if side == "b" {
        ("GLIMMER", "RED", 0x94u8, 10u8)
    } else {
        ("ROCKY", "BLUE", 0x6au8, 11u8)
    };
    let party = read_party(&mut gb);
    let Some(got) = party.iter().find(|m| m.species == want.2) else {
        eprintln!("[{side}] NO TRADED POKEMON IN THE PARTY");
        std::process::exit(1);
    };
    let ok = got.nickname == want.0 && got.ot_name == want.1 && got.level == want.3;
    println!(
        "[{side}] traded Pokemon: {} (OT {}) Lv{} — {}",
        got.nickname,
        got.ot_name,
        got.level,
        if ok { "INTACT" } else { "CORRUPT" }
    );
    if !ok {
        eprintln!(
            "[{side}] EXPECTED {} (OT {}) Lv{} — the wire lost bytes",
            want.0, want.1, want.3
        );
        std::process::exit(1);
    }
}
