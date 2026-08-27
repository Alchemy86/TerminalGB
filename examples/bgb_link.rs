//! One emulated Game Boy with its link port on a TCP socket, speaking **BGB's
//! link-cable protocol**.
//!
//! ```text
//! # two consoles, two processes, one socket — and a real Gen 1 trade over it
//! cargo run --release --features image,plugin-gen1-link,plugin-trainer \
//!     --example bgb_link -- --rom ~/roms/blue.gb --save <fixture.sav> \
//!     --name a --listen 127.0.0.1:8765 --party red  --expect-ot BLUE
//!
//! cargo run --release --features image,plugin-gen1-link,plugin-trainer \
//!     --example bgb_link -- --rom ~/roms/blue.gb --save <fixture.sav> \
//!     --name b --connect 127.0.0.1:8765 --party blue --expect-ot RED
//! ```
//!
//! # What this proves that `link_two` does not
//!
//! [`examples/link_two.rs`](link_two.rs) joins two cores that share a loop and
//! a clock. It is the strongest possible statement about the *emulated* link
//! port and says nothing at all about interoperating with anybody: both ends
//! are ours, stepped by one `for` loop, and a byte cannot be late because there
//! is no "late".
//!
//! This runs **one** console and puts the far end on a socket. The peer can be
//! another copy of this (which is what `--expect-ot` checks end to end), BGB
//! itself, or — the reason the protocol was worth implementing —
//! `Lorenzooone/PokemonGB_Online_Trades`, whose
//! `utilities/bgb_link_cable_server.py` relays the same bytes onward to a
//! **real Game Boy** on a USB link adapter. Nothing in this file knows which of
//! those is on the other end.
//!
//! # `--expect-ot` is the whole point
//!
//! A link that handshakes and never carries a trade is not a link. After the
//! run the party must hold a Pokémon whose **original trainer is the other
//! console's player** — a value this process never wrote and could not have
//! invented — and that Pokémon's nickname, species and level must match what
//! was sent, because a lost byte shows up as `$FF` inside a name and a decoder
//! that skips it turns a corrupt "GLIMMER" into a plausible "MMER". Exit status
//! 1 otherwise, so a shell script can be the judge.
//!
//! # Never touch the player's ROM directory
//!
//! The MBC writes `<rom>.sav` next to the ROM it was given, so the cartridge
//! and the save are **copied** into `target/bgb-link/<name>/` first. Nothing
//! here opens a path in `~/roms` for writing.

use std::path::PathBuf;
use std::time::Instant;

use terminalgb::bgblink::BgbLink;
use terminalgb::gameboy::Gameboy;
use terminalgb::link::TransportLink;

/// The Cable Club bot, the save staging and the screen reader, shared with
/// `examples/link_two.rs`.
#[path = "support/gen1_cable_club.rs"]
mod cable_club;

use cable_club::*;

/// The two parties, so "the trade happened" is provable rather than plausible:
/// each console's Pokémon carry an original trainer the other console has never
/// heard of. `red` also carries the DVs that make a Gen 1 Pokémon shiny once it
/// reaches Gen 2, which survives the trade and is a second independent witness.
fn party_spec(which: &str) -> Option<String> {
    match which {
        "red" => Some(format!(
            "trainer = RED\n\
             species = Abra\nlevel = 10\nnickname = GLIMMER\not_name = RED\not_id = 4242\n\
             dvs = {}, {}, {}, {}\n\
             moves = Teleport\n\
             species = Rattata\nlevel = 8\nnickname = RATTY\not_name = RED\not_id = 4242\n\
             moves = Tackle, Tail Whip\n",
            SHINY_DVS.0, SHINY_DVS.1, SHINY_DVS.2, SHINY_DVS.3
        )),
        "blue" => Some(
            "trainer = BLUE\n\
             species = Machop\nlevel = 11\nnickname = ROCKY\not_name = BLUE\not_id = 1717\n\
             dvs = 3, 4, 5, 6\n\
             moves = Karate Chop, Low Kick\n\
             species = Pidgey\nlevel = 7\nnickname = FLAP\not_name = BLUE\not_id = 1717\n\
             moves = Gust\n"
                .to_string(),
        ),
        _ => None,
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: bgb_link --rom <blue.gb> (--listen <addr> | --connect <addr>)\n\
         \n\
         --save <fixture.sav>   start from this battery save (copied, never opened for writing)\n\
         --party red|blue       patch a distinguishable party into the staged save\n\
         --trade                drive the Cable Club bot into the Trade Center and trade\n\
         --echo N               ignore --rom; run a built-in echo ROM until N bytes have\n\
                                been exchanged, and print them. For interoperability\n\
                                testing against another implementation of the protocol.\n\
         --expect-ot NAME       exit 1 unless the party ends up holding NAME's Pokemon\n\
         --name <label>         staging directory + print prefix (default \"gb\")\n\
         --frames N             give up after N frames (default 8 minutes\' worth)\n\
         --settle N             keep servicing the link for N frames after the bot\n\
                                finishes, so the console that finishes SECOND still\n\
                                has a cable (default 600)\n\
         --shots <dir>          a PNG per stage transition\n\
         --verbose              print the console's own screen at every transition\n\
         \n\
         One side listens and the other connects; that is the protocol's own\n\
         rule, not ours (https://bgb.bircd.org/bgblink.html)."
    );
    std::process::exit(2)
}

fn main() {
    let mut rom: Option<PathBuf> = None;
    let mut save: Option<PathBuf> = None;
    let mut spec: Option<String> = None;
    let mut listen: Option<String> = None;
    let mut connect: Option<String> = None;
    let mut expect_ot: Option<String> = None;
    let mut name = "gb".to_string();
    let mut shots: Option<PathBuf> = None;
    let mut frames = 60 * 60 * 8;
    let mut trade = false;
    let mut verbose = false;
    let mut echo: Option<usize> = None;
    let mut settle = 600u32;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rom" => rom = args.next().map(PathBuf::from),
            "--save" => save = args.next().map(PathBuf::from),
            "--party" => spec = args.next(),
            "--listen" => listen = args.next(),
            "--connect" => connect = args.next(),
            "--expect-ot" => expect_ot = args.next(),
            "--name" => name = args.next().unwrap_or(name),
            "--shots" => shots = args.next().map(PathBuf::from),
            "--frames" => {
                frames = args.next().and_then(|s| s.parse().ok()).unwrap_or(frames)
            }
            "--echo" => {
                echo = Some(args.next().and_then(|s| s.parse().ok()).unwrap_or(16))
            }
            "--settle" => {
                settle = args.next().and_then(|s| s.parse().ok()).unwrap_or(settle)
            }
            "--trade" => trade = true,
            "--verbose" => verbose = true,
            "-h" | "--help" => usage(),
            other => {
                eprintln!("unknown argument {other}");
                usage();
            }
        }
    }
    if listen.is_some() == connect.is_some() {
        usage();
    }
    if echo.is_none() && rom.is_none() {
        usage();
    }

    // Staging first: everything after this point works on copies.
    let scratch = PathBuf::from("target/bgb-link");
    let staged = rom.as_ref().map(|rom| {
        let staged = stage(&name, rom, save.as_deref(), &scratch);
        if let Some(which) = &spec {
            match party_spec(which) {
                Some(s) => patch_party(&staged, &s),
                None => {
                    eprintln!("--party wants `red` or `blue`, not `{which}`");
                    std::process::exit(2);
                }
            }
        }
        staged
    });

    // Open the socket **before** the console, so a misconfigured address is a
    // clear message rather than eight minutes of a game linking to nothing.
    let opened = match (&listen, &connect) {
        (Some(a), _) => BgbLink::listen(a.as_str()),
        (_, Some(a)) => BgbLink::connect(a.as_str()),
        _ => unreachable!("checked above"),
    };
    let transport = match opened {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[{name}] link: {e}");
            std::process::exit(1);
        }
    };
    let mut link = TransportLink::new(Box::new(transport));
    println!("[{name}] {}", link.describe());

    let mut gb = match (&staged, echo) {
        (_, Some(_)) => Gameboy::new(echo_rom(), None),
        (Some(s), None) => Gameboy::new(
            std::fs::read(&s.rom).expect("staged rom"),
            Some(s.rom.clone()),
        ),
        _ => unreachable!("checked above"),
    };
    // A machine with no APU is not a quiet Game Boy, it is one with the sound
    // chip pulled out, and a cartridge can tell — see `examples/blackout_probe.rs`.
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    if echo.is_some() {
        println!("[{name}] running the built-in echo ROM");
    } else {
        println!("[{name}] cartridge: {}", gb.rom_title());
    }

    if let Some(dir) = &shots {
        std::fs::create_dir_all(dir).expect("shots dir");
    }

    let mut bot = trade.then(TradeBot::default);
    let mut last_label = String::new();
    let mut shot_n = 0usize;
    let mut announced = false;
    let started = Instant::now();

    for frame in 0..frames {
        if let Some(want) = echo {
            link.run_frame(&mut gb);
            if !announced && link.live() {
                announced = true;
                println!("[{name}] {} (frame {frame})", link.describe());
            }
            if let Some(e) = link.failure() {
                eprintln!("[{name}] link failed at frame {frame}: {e}");
                break;
            }
            if link.stats().transfers as usize >= want {
                println!("[{name}] echoed {want} bytes by frame {frame}");
                break;
            }
            continue;
        }
        if let Some(b) = &mut bot {
            b.act(&mut gb, verbose, &name);
        }
        link.run_frame(&mut gb);

        if !announced && link.live() {
            announced = true;
            println!("[{name}] {} (frame {frame})", link.describe());
        }
        if let Some(e) = link.failure() {
            // A `Setup` failure is an unplugged cable and the console carries
            // on; a `Broken` one has already faulted it. Either way, say so
            // once and stop pretending a link is running.
            eprintln!("[{name}] link failed at frame {frame}: {e}");
            break;
        }

        if let Some(b) = &bot {
            let label = b.stage_label().to_string();
            if label != last_label {
                let st = link.stats();
                println!(
                    "[{name}] {frame:6} {label:<14} transfers {:<6} unanswered {:<6} \
                     missed {:<4} clock {:#04x}",
                    st.transfers,
                    st.unanswered,
                    st.missed,
                    gb.peek(H_SERIAL_CONNECTION_STATUS),
                );
                if let Some(dir) = &shots {
                    shot_n += 1;
                    write_one(&dir.join(format!("{shot_n:02}-{label}.png")), &mut gb);
                }
                if verbose {
                    println!("{}", screen_text(&mut gb));
                }
                last_label = label;
            }
            if b.finished() {
                println!(
                    "[{name}] bot finished at frame {frame} ({:.2}s)",
                    started.elapsed().as_secs_f32()
                );
                break;
            }
        }
    }

    // **Keep the cable plugged in after our own bot is done.** The two
    // consoles do not finish together — Gen 1 runs another full three-block
    // exchange after "Trade completed!" — so whichever finishes first would
    // otherwise hang up on a partner still mid-exchange, and the party read
    // below would catch the game rewriting it. Observed exactly that: a run
    // that had traded correctly reported a party entry shifted one byte
    // (`LIMMER`, OT `ED`, level 0) because it was read mid-write.
    for _ in 0..settle {
        if link.failure().is_some() {
            gb.frame();
        } else {
            link.run_frame(&mut gb);
        }
    }

    // How the link ended is worth printing even when nothing went wrong: a
    // `Setup` here is the peer saying goodbye first, which is the *good* case
    // and the one that leaves both saves writable.
    if let Some(e) = link.failure() {
        println!("[{name}] link ended: {e:?}");
    }

    // Say goodbye before the process ends, so the peer records an unplugged
    // cable rather than a broken one and keeps writing its own save. `Drop`
    // would not do — this example exits normally, but the frontends this
    // pattern is copied into do not always get to run destructors.
    link.disconnect();

    // --- report ----------------------------------------------------------
    let st = link.stats();
    println!(
        "\n[{name}] link: {} transfers, {} unanswered, {} missed, {} timeouts, \
         {} stalls, over {:.1}s",
        st.transfers,
        st.unanswered,
        st.missed,
        st.timeouts,
        st.stalls,
        started.elapsed().as_secs_f32()
    );
    if let Some(dir) = &shots {
        write_one(&dir.join("99-final.png"), &mut gb);
    }
    if let Some(want) = echo {
        let got: Vec<u8> = (0..want.min(256))
            .map(|i| gb.peek(0xC000 + i as u16))
            .collect();
        let hex: Vec<String> = got.iter().map(|b| format!("{b:02x}")).collect();
        println!("[{name}] echo-received: {}", hex.join(" "));
        let ok = link.stats().transfers as usize >= want;
        println!(
            "[{name}] echo: {}",
            if ok { "COMPLETE" } else { "INCOMPLETE" }
        );
        std::process::exit(if ok { 0 } else { 1 });
    }

    println!("{}", screen_text(&mut gb));

    let party = read_party(&mut gb);
    println!("[{name}] party:");
    for m in &party {
        println!("  {}", m.describe());
    }

    if let Some(ot) = expect_ot {
        // **Check the bytes, not just that something arrived.** A lost byte on
        // the wire shows up as `$FF` inside a name, `$FF` is not a character,
        // and a decoder that skips unknown bytes turns a corrupt "GLIMMER" into
        // a plausible "MMER" and a corrupt "RED" into "D". So the expected
        // Pokémon is named in full and compared field by field — the same
        // reasoning `examples/link_two.rs`'s peer mode records, where a
        // cross-process run once looked fine and was not.
        let want = match ot.as_str() {
            "BLUE" => Some(("ROCKY", 0x6au8, 11u8)),
            "RED" => Some(("GLIMMER", 0x94u8, 10u8)),
            _ => None,
        };
        let held = party.iter().find(|m| m.ot_name == ot);
        let intact = match (held, want) {
            (Some(m), Some(w)) => m.nickname == w.0 && m.species == w.1 && m.level == w.2,
            // An OT we do not have an expectation for: the presence of a name
            // this console never wrote is all that can honestly be claimed.
            (Some(_), None) => true,
            _ => false,
        };
        println!(
            "\n[{name}] trade: {}",
            match (held, intact) {
                (Some(m), true) => format!(
                    "COMPLETE — holding {} (OT {ot}) Lv{}, INTACT; {ot} is a name \
                     this console never wrote",
                    m.nickname, m.level
                ),
                (Some(m), false) => format!(
                    "CORRUPT — holding {} (OT {ot}) Lv{}, which is not what was \
                     sent: the wire lost bytes",
                    m.nickname, m.level
                ),
                _ => format!("NOT COMPLETE — nothing in the party has {ot} as its OT"),
            }
        );
        if !intact {
            std::process::exit(1);
        }
    }
}

/// A cartridge that does nothing but answer the link cable: it idles for a
/// moment, then arms `SC = $80` (external clock), waits to be clocked, parks
/// what arrived at `$C000` onwards, and offers the **complement** of it back.
///
/// The complement is what makes an interoperability test a real test rather
/// than a coincidence. An echo of the same byte cannot tell a working link from
/// a peer reading its own transmission back; `~b` is a value only this console
/// can have produced, and it changes with every byte.
///
/// **The startup delay is load-bearing, and it is a property of the
/// counterparty rather than of this emulator.** `PokemonGB_Online_Trades`'
/// server sends no data at all until it has seen one sync3 `b2 = 1` from the
/// client — its `can_go` flag — i.e. until the client has been caught *not*
/// armed once. Every real cartridge satisfies that without trying, because it
/// boots and draws a title screen before it ever touches the link port. A test
/// ROM that arms on its first instruction never would, and the link would come
/// up and then sit there for ever. Roughly 1.3 M cycles, about twenty frames.
///
/// ```text
/// 0100  01 00 00    LD   BC, $0000
/// 0103  0B          DEC  BC            ; ~1.3M cycles of ordinary boot-time
/// 0104  78          LD   A, B
/// 0105  B1          OR   C
/// 0106  20 FB       JR   NZ, $0103
/// 0108  21 00 C0    LD   HL, $C000
/// 010B  3E A5       LD   A, $A5        ; the byte offered before anything arrives
/// 010D  E0 01       LDH  ($01), A      ; SB
/// 010F  3E 80       LD   A, $80
/// 0111  E0 02       LDH  ($02), A      ; SC = armed, external clock
/// 0113  F0 02       LDH  A, ($02)
/// 0115  CB 7F       BIT  7, A
/// 0117  20 FA       JR   NZ, $0113     ; ...until the far end clocks us
/// 0119  F0 01       LDH  A, ($01)      ; what it sent
/// 011B  22          LD   (HL+), A
/// 011C  2F          CPL
/// 011D  18 EE       JR   $010D         ; offer the complement, round again
/// ```
fn echo_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];
    rom[0x100..0x11F].copy_from_slice(&[
        0x01, 0x00, 0x00, 0x0B, 0x78, 0xB1, 0x20, 0xFB, 0x21, 0x00, 0xC0, 0x3E, 0xA5,
        0xE0, 0x01, 0x3E, 0x80, 0xE0, 0x02, 0xF0, 0x02, 0xCB, 0x7F, 0x20, 0xFA, 0xF0,
        0x01, 0x22, 0x2F, 0x18, 0xEE,
    ]);
    rom
}
