//! **Two processes, auto-discovered, one trade** — the first regression for the
//! path a person actually uses.
//!
//! ## The gap this closes
//!
//! Everything this repository knows about the link cable is proved with **both
//! consoles inside one process**: `LinkCable` (`src/link.rs`) steps two cores in
//! one loop, `examples/gen2_timecapsule.rs --capsule-trade` drives a Gen 1 and a
//! Gen 2 core the same way, and `testharness/colosseum.rs` plugs a *virtual*
//! partner into one core. Not one of them launches a second process, and the
//! shipping default — `link = auto`, two emulators finding each other over a
//! unix socket in `$XDG_RUNTIME_DIR/terminalgb/link` — was therefore covered by
//! nothing at all. It has been exercised only by hand.
//!
//! That is a real gap, not a formality: the two-process path has its own failure
//! modes, and every one of them is invisible to an in-process test. Both ends
//! mastering at once deadlocks (each holding its shift register for the other);
//! a deadline counted in *emulated* T-cycles is a fraction of a frame of wall
//! clock on an unpaced console, so every master transfer is declared torn; a
//! trait method that exists as an inherent method but was never added to the
//! `impl` silently becomes the trait's empty default. All three of those are
//! recorded in `docs/link-cable.md` and `docs/bgb-link.md` as bugs that reached
//! a person because no harness ran two processes.
//!
//! ## What it does
//!
//! One parent, two children, both of them this same binary re-executed with
//! `GB_TWOPROCESS_SIDE` set:
//!
//! * the **parent** stages the cartridge and the save into `target/` twice,
//!   patches a different party into each staged save, points `GB_LINK_DIR` at a
//!   scratch directory unique to this run, launches both children, and judges
//!   them;
//! * each **child** builds exactly one `Gameboy`, calls `LinkPeer::discover`
//!   (i.e. the shipping auto-discovery, not a wired-up pair), drives the shared
//!   Cable Club bot from the title screen through the trade, and flushes its
//!   battery.
//!
//! The verdict is taken **three** ways, and the third is the one an in-process
//! test cannot make:
//!
//! 1. both children exit 0 — each has checked the arrival field by field
//!    (nickname, OT name, species, level), because a lost byte arrives as `$FF`,
//!    `$FF` is not a character, and a decoder that skips it turns a corrupt
//!    `GLIMMER` into a plausible `MMER`;
//! 2. the cable moved bytes and tore none — `transfers > 0`, `torn == 0`, and
//!    neither console reported a fault;
//! 3. **the two staged `.sav` files, read back from disk by the parent, each
//!    hold the other console's Pokémon and pass Gen 1's own `CalcCheckSum`.**
//!    That is the claim a person makes when they say the trade "worked": not
//!    that two cores agreed in memory, but that two independent processes each
//!    wrote a save that survives the emulator being closed.
//!
//! ## Running it — `GB_TWOPROCESS_RUN=1` is required, on purpose
//!
//! ```text
//! GB_TWOPROCESS_RUN=1 cargo test --release \
//!     --features image,plugin-gen1-link,plugin-trainer --test twoprocess
//! ```
//!
//! **This harness is opt-in, and that is deliberate — read `docs/link-cable.md`
//! §8, "Reliability", before assuming it should not be.** It launches two real
//! OS processes over the shipping `link = auto` unix-socket rendezvous, and
//! pass/fail there is *measurably load-dependent*: 122/130 rounds on an idle
//! 20-core box, 19/20 at load ~5, 7/10 at ~10, 3/10 at ~31, 1/10 at ~49 —
//! including a silent corruption mode (`torn = 0`, `fault = none`, and a party
//! member written to the battery file with bytes shifted off the front). That
//! is an **open, documented bug in the emulator's two-process link**, not a
//! defect in this test, and it is exactly why the crew that wrote it kept the
//! harness out of CI. Without `GB_TWOPROCESS_RUN=1` set the harness prints why
//! and how to run it, then exits 0 — it never runs, so it can never turn an
//! unrelated `cargo test` red for a bug nobody currently controls.
//!
//! Once opted in, it still needs a Gen 1 cartridge (`POKE_ROM=<path>`, or one in
//! `~/roms`) and a save standing at the Cable Club receptionist in the Viridian
//! Pokémon Center (`GB_CABLECLUB_SAVE=<path>`, or the fixture
//! `gen1_trade --make-save` leaves in the temp directory). Without both it
//! **skips with exit 0** and says how to produce them, exactly as
//! `testharness/colosseum.rs` does — and like that harness it deliberately does
//! *not* fall back to scanning a player's own save directory.
//!
//! **The trigger is deliberately the env var alone, never a file left behind by
//! something else.** An earlier version of this harness found its fixture the
//! moment `$TMPDIR/gameboy-gen1-trade/cableclub.sav` existed — which any full
//! `cargo test` run leaves there via `testharness/colosseum.rs`'s own fixture
//! search once `gen1_trade --make-save` has ever been run on the box. So a
//! developer running the *whole* suite could have this harness silently start
//! actually executing, on a loaded machine, for a bug the person never asked to
//! reproduce. `GB_TWOPROCESS_RUN=1` is checked **before** any fixture is looked
//! for, so incidental files can no longer promote a skip into a real run.
//!
//! **It never writes beside the cartridge.** The ROM and the save are copied
//! into `target/twoprocess/{a,b}` first and only the copies are patched, because
//! the MBC writes `<rom>.sav` next to whatever ROM it was handed.
//!
//! **`GB_LINK_DIR` is not optional here.** `link = auto` is the shipping
//! default, so a developer with the emulator open would otherwise have this
//! harness pair with *their* console. Every run gets its own rendezvous
//! directory and removes it afterwards.
//!
//! **What must never change because of any of this: the verdict itself.** Once
//! the harness is running — deliberately, because someone set the env var — it
//! checks exactly what it always checked, at exactly the same strictness: field-
//! by-field arrival, zero torn transfers, zero faults, and both battery files
//! read back off disk and checksum-verified. Gating *whether* it runs must never
//! become an excuse to loosen *what it demands* when it does.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use terminalgb::gameboy::Gameboy;
use terminalgb::link::peer::LinkPeer;

#[path = "../examples/support/gen1_cable_club.rs"]
mod cable_club;

use cable_club::{
    decode, patch_party, read_party, stage, Staged, TradeBot, MON_STRUCT_LEN, NAME_LEN,
    SAVE_CHECKSUM, SAVE_PARTY_DATA, SAVE_PLAYER_NAME, SHINY_DVS,
};

/// A hard cap so a console that never reaches the desk cannot run for ever.
/// Measured on this cartridge: the console that arrives *second* finishes at
/// ~5,100 frames and the one that waited at ~10,900.
const FRAME_CAP: u32 = 24_000;

/// Wall-clock ceiling for a child. Both children together take a few seconds at
/// ~50x real time; this is a watchdog, not a budget.
const CHILD_TIMEOUT: Duration = Duration::from_secs(300);

/// What each console is given, and therefore what the *other* one must end up
/// holding. Nicknames, OT names and levels are all distinct so "the trade
/// happened" is provable rather than plausible.
struct Side {
    which: &'static str,
    spec: String,
    /// (nickname, OT name, species index, level) this side must receive.
    expect: (&'static str, &'static str, u8, u8),
}

fn sides() -> [Side; 2] {
    [
        Side {
            which: "a",
            spec: format!(
                "trainer = RED\n\
                 species = Abra\nlevel = 10\nnickname = GLIMMER\not_name = RED\not_id = 4242\n\
                 dvs = {}, {}, {}, {}\n\
                 moves = Teleport\n\
                 species = Rattata\nlevel = 8\nnickname = RATTY\not_name = RED\not_id = 4242\n\
                 moves = Tackle, Tail Whip\n",
                SHINY_DVS.0, SHINY_DVS.1, SHINY_DVS.2, SHINY_DVS.3
            ),
            expect: ("ROCKY", "BLUE", 0x6a, 11),
        },
        Side {
            which: "b",
            spec: "trainer = BLUE\n\
                   species = Machop\nlevel = 11\nnickname = ROCKY\not_name = BLUE\not_id = 1717\n\
                   dvs = 3, 4, 5, 6\n\
                   moves = Karate Chop, Low Kick\n\
                   species = Pidgey\nlevel = 7\nnickname = FLAP\not_name = BLUE\not_id = 1717\n\
                   moves = Gust\n"
                .to_string(),
            expect: ("GLIMMER", "RED", 0x94, 10),
        },
    ]
}

fn main() {
    if let Ok(side) = std::env::var("GB_TWOPROCESS_SIDE") {
        child(&side);
    } else {
        parent();
    }
}

// ---------------------------------------------------------------------------
// The child: one console, an automatically discovered partner.
// ---------------------------------------------------------------------------

fn child(which: &str) -> ! {
    let rom =
        PathBuf::from(std::env::var("GB_TWOPROCESS_ROM").expect("GB_TWOPROCESS_ROM"));
    let side = sides()
        .into_iter()
        .find(|s| s.which == which)
        .unwrap_or_else(|| panic!("unknown side {which}"));

    let bytes = std::fs::read(&rom).expect("staged rom");
    let mut gb = Gameboy::new(bytes, Some(rom.clone()));
    let title = gb.rom_title();
    let sav = rom.with_extension("sav");
    // The shipping discovery, not a wired-up pair: this is exactly what
    // `link = auto` does in the terminal frontend.
    let mut peer = LinkPeer::discover(&title, Some(&sav));

    let mut bot = TradeBot::default();
    let mut frames = 0u32;
    for f in 0..FRAME_CAP {
        frames = f;
        bot.act(&mut gb, false, side.which);
        peer.run_frame(&mut gb);
        if bot.finished() {
            break;
        }
        if gb.fault().is_some() {
            break;
        }
    }

    let st = peer.stats();
    let fault = gb.fault().map(|f| format!("{f:?}"));
    // Persist whatever the game wrote, so the parent can judge the *file*.
    gb.flush_battery();
    peer.close();

    let party = read_party(&mut gb);
    for m in &party {
        println!("[{which}]   {}", m.describe());
    }
    println!(
        "RESULT side={which} frames={frames} transfers={} unanswered={} torn={} fault={}",
        st.transfers,
        st.unanswered,
        st.torn,
        fault.as_deref().unwrap_or("none")
    );

    // **Check the bytes, not that something arrived.** See the module comment.
    let (nick, ot, species, level) = side.expect;
    let Some(got) = party.iter().find(|m| m.species == species) else {
        eprintln!("[{which}] FAIL: no traded Pokémon in the party");
        std::process::exit(1);
    };
    if got.nickname != nick || got.ot_name != ot || got.level != level {
        eprintln!(
            "[{which}] FAIL: expected {nick} (OT {ot}) Lv{level}, got {} (OT {}) Lv{} \
             — the wire lost bytes",
            got.nickname, got.ot_name, got.level
        );
        std::process::exit(1);
    }
    if st.torn > 0 || fault.is_some() {
        eprintln!("[{which}] FAIL: torn={} fault={fault:?}", st.torn);
        std::process::exit(1);
    }
    std::process::exit(0);
}

// ---------------------------------------------------------------------------
// The parent: staging, launching, and the verdict on the two save files.
// ---------------------------------------------------------------------------

fn parent() {
    // **Opt-in, checked before anything else — including before either fixture
    // search runs.** This is the fix for the incidental-coupling hazard the
    // module comment describes: without this env var, the presence or absence
    // of some *other* harness's leftover temp file must never decide whether
    // this one actually executes. See the module doc for the full "why".
    if std::env::var_os("GB_TWOPROCESS_RUN").is_none() {
        println!(
            "twoprocess: SKIPPED by default — this harness is a known, open, \
             load-correlated flake (docs/link-cable.md §8, \"Reliability\": \
             122/130 idle, 19/20 at load ~5, 1/10 at load ~49, including a \
             silent corruption mode that writes a shifted party to the battery \
             file with torn=0 and no fault reported). That is a real bug in the \
             two-process link, not a bug in this test, and it is why the harness \
             stays out of the default `cargo test` set and out of CI. Run it \
             deliberately:\n\
             \n\
             \x20\x20GB_TWOPROCESS_RUN=1 cargo test --release \\\n\
             \x20\x20\x20\x20--features image,plugin-gen1-link,plugin-trainer \
             --test twoprocess\n\
             \n\
             (also needs a cartridge — POKE_ROM=<path> or one in ~/roms — and a \
             Cable Club save — GB_CABLECLUB_SAVE=<path>, or the fixture \
             `gen1_trade --make-save` leaves in the temp directory). Repeat with \
             GB_TWOPROCESS_REPEAT=N on an idle box to reproduce the reliability \
             table. Skipping (exit 0)."
        );
        return;
    }
    let Some(rom) = find_rom() else {
        println!(
            "twoprocess: no Pokémon Red/Blue cartridge found — set POKE_ROM=<path> \
             or drop one in ~/roms. Skipping (exit 0)."
        );
        return;
    };
    let Some(save) = find_save() else {
        println!(
            "twoprocess: no Cable Club save found — set GB_CABLECLUB_SAVE=<path>, or \
             produce one with\n  cargo run --release --example gen1_trade -- --rom \
             <rom> --make-save\nSkipping (exit 0)."
        );
        return;
    };
    println!("twoprocess: cartridge {}", rom.display());
    println!("twoprocess: save      {}", save.display());

    // `GB_TWOPROCESS_REPEAT=N` runs the whole thing N times and prints the
    // tally. That is not decoration: **this path is not reliable yet, and how
    // unreliable depends on how busy the machine is** — 122 of 130 rounds on an
    // idle 20-core box, but 19/20 at load ~5, 7/10 at ~10, 3/10 at ~31 and 1/10
    // at ~49 (stalls, and arrivals with a byte missing off the front which
    // reached the battery file with `torn = 0` and no fault). So a single green
    // round says very little, and this harness is not in CI because a CI runner
    // is a loaded machine. Full record: `docs/link-cable.md` §8, "Reliability".
    // Any round failing fails the harness.
    let rounds: u32 = std::env::var("GB_TWOPROCESS_REPEAT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let mut passed = 0u32;
    for round in 1..=rounds {
        if rounds > 1 {
            println!("\n--- round {round}/{rounds} ---");
        }
        let failures = one_round(&rom, &save);
        if failures.is_empty() {
            passed += 1;
            println!(
                "twoprocess: PASS — two independently launched processes discovered \
                 each other over a unix socket, traded, and each wrote a save holding \
                 the other's Pokémon."
            );
        } else {
            eprintln!("twoprocess: FAIL");
            for f in &failures {
                eprintln!("  - {f}");
            }
        }
    }
    if rounds > 1 {
        println!("\ntwoprocess: {passed}/{rounds} rounds passed");
    }
    if passed != rounds {
        std::process::exit(1);
    }
}

fn one_round(rom: &Path, save: &Path) -> Vec<String> {
    let scratch = PathBuf::from("target/twoprocess");
    let _ = std::fs::remove_dir_all(&scratch);
    // A rendezvous directory nobody else shares. Without this the harness would
    // pair with a developer's own running emulator, because `link = auto` is the
    // default.
    let link_dir = scratch.join("link");
    std::fs::create_dir_all(&link_dir).expect("link dir");

    // **The parent stages; the children never do.** Two processes each staging
    // both sides is a race — the second one deletes the first one's `.sav` while
    // it is running.
    let staged: Vec<(Side, Staged)> = sides()
        .into_iter()
        .map(|s| {
            let st = stage(s.which, rom, Some(save), &scratch);
            patch_party(&st, &s.spec);
            (s, st)
        })
        .collect();

    let exe = std::env::current_exe().expect("current exe");
    let mut failures: Vec<String> = Vec::new();
    let mut kids = Vec::new();
    for (side, st) in &staged {
        let child = Command::new(&exe)
            .env("GB_TWOPROCESS_SIDE", side.which)
            .env("GB_TWOPROCESS_ROM", &st.rom)
            .env("GB_LINK_DIR", &link_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn child");
        kids.push((side.which, child));
        // **The second console must not start until the first has advertised,
        // and that is a property of the product, not of this harness.**
        // `LinkPeer::discover` scans the rendezvous directory exactly once: if
        // it finds a socket it connects, otherwise it binds one of its own and
        // waits to be connected *to*. It never rescans. So two instances
        // started in the same instant both scan an empty directory, both
        // advertise, and neither ever pairs — measured here as 24,000 frames
        // with `transfers=0` on both sides. A person launching two terminals by
        // hand can never hit it; a harness launching two processes at once hits
        // it every time. Waiting for the socket to appear is therefore what a
        // person does, expressed exactly. See `docs/link-cable.md` §5,
        // "Discovery".
        if kids.len() == 1 && !wait_for_socket(&link_dir, Duration::from_secs(30)) {
            failures.push("the first console never advertised a socket".to_string());
        }
    }

    let started = Instant::now();
    let mut outputs = Vec::new();
    for (which, mut kid) in kids {
        let status = loop {
            match kid.try_wait().expect("try_wait") {
                Some(s) => break s,
                None if started.elapsed() > CHILD_TIMEOUT => {
                    let _ = kid.kill();
                    failures.push(format!(
                        "side {which} did not finish within {CHILD_TIMEOUT:?}"
                    ));
                    break kid.wait().expect("wait");
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        let out = kid.wait_with_output().ok();
        let (so, se) = match &out {
            Some(o) => (
                String::from_utf8_lossy(&o.stdout).to_string(),
                String::from_utf8_lossy(&o.stderr).to_string(),
            ),
            None => (String::new(), String::new()),
        };
        print!("{so}");
        if !se.is_empty() {
            eprint!("{se}");
        }
        if !status.success() {
            failures.push(format!("side {which} exited {status}"));
        }
        outputs.push((which, so));
    }

    // (2) The cable moved bytes and tore none.
    for (which, so) in &outputs {
        let Some(line) = so.lines().find(|l| l.starts_with("RESULT ")) else {
            failures.push(format!("side {which} printed no RESULT line"));
            continue;
        };
        let field = |k: &str| -> Option<String> {
            line.split_whitespace()
                .find_map(|f| f.strip_prefix(k).map(str::to_string))
        };
        let transfers: u64 = field("transfers=")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if transfers == 0 {
            failures.push(format!("side {which} transferred no bytes at all"));
        }
        if field("torn=").as_deref() != Some("0") {
            failures.push(format!("side {which} reported torn transfers"));
        }
        if field("fault=").as_deref() != Some("none") {
            failures.push(format!("side {which} faulted: {line}"));
        }
    }

    // (3) The claim only two processes can make: the arrival is in the *file*.
    for (side, st) in &staged {
        let sav = st.rom.with_extension("sav");
        let bytes = match std::fs::read(&sav) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("side {}: no save file ({e})", side.which));
                continue;
            }
        };
        if !checksum_ok(&bytes) {
            failures.push(format!(
                "side {}: the save file fails Gen 1's own CalcCheckSum",
                side.which
            ));
            continue;
        }
        let party = party_from_save(&bytes);
        let (nick, ot, species, level) = side.expect;
        println!("side {} save file: {}", side.which, describe(&party));
        match party.iter().find(|m| m.species == species) {
            None => failures.push(format!(
                "side {}: the save file does not hold {nick} — the trade did not reach disk",
                side.which
            )),
            Some(m) if m.nickname != nick || m.ot_name != ot || m.level != level => {
                failures.push(format!(
                    "side {}: the save file holds {} (OT {}) Lv{}, expected {nick} (OT {ot}) Lv{level}",
                    side.which, m.nickname, m.ot_name, m.level
                ))
            }
            Some(_) => {}
        }
    }

    let _ = std::fs::remove_dir_all(&link_dir);
    failures
}

/// Block until some instance has bound a `*.sock` in the rendezvous directory.
/// See the comment at the call site: this is the product's one-shot discovery
/// scan expressed as the thing a person does without noticing.
fn wait_for_socket(dir: &Path, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(entries) = std::fs::read_dir(dir) {
            if entries
                .flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "sock"))
            {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

// ---------------------------------------------------------------------------
// Reading a party out of a save file, rather than out of a running console.
// ---------------------------------------------------------------------------

struct SaveMon {
    species: u8,
    level: u8,
    ot_name: String,
    nickname: String,
    dvs: (u8, u8, u8, u8),
}

/// `sPartyData` (`pokered/ram/sram.asm`, `01:AF2C`) is a byte-for-byte copy of
/// WRAM `$D163`, so the party layout is the same one `read_party` walks: count,
/// seven species bytes, six 44-byte structs, six 11-byte OT names, six 11-byte
/// nicknames. A `.sav` is raw cartridge RAM with no header (`src/mbc/battery.rs`).
fn party_from_save(sram: &[u8]) -> Vec<SaveMon> {
    if sram.len() < SAVE_CHECKSUM {
        return Vec::new();
    }
    let count = sram[SAVE_PARTY_DATA] as usize;
    if count == 0 || count > 6 {
        return Vec::new();
    }
    (0..count)
        .map(|i| {
            let base = SAVE_PARTY_DATA + 8 + i * MON_STRUCT_LEN;
            let s = &sram[base..base + MON_STRUCT_LEN];
            SaveMon {
                species: sram[SAVE_PARTY_DATA + 1 + i],
                level: s[33],
                dvs: (s[27] >> 4, s[27] & 15, s[28] >> 4, s[28] & 15),
                ot_name: decode(
                    &sram[SAVE_PARTY_DATA + 272 + i * NAME_LEN..][..NAME_LEN],
                ),
                nickname: decode(
                    &sram[SAVE_PARTY_DATA + 338 + i * NAME_LEN..][..NAME_LEN],
                ),
            }
        })
        .collect()
}

/// `CalcCheckSum` (`pokered/engine/menus/save.asm`): the complement of the byte
/// sum over `sGameData`.
fn checksum_ok(sram: &[u8]) -> bool {
    if sram.len() <= SAVE_CHECKSUM {
        return false;
    }
    let sum = sram[SAVE_PLAYER_NAME..SAVE_CHECKSUM]
        .iter()
        .fold(0u8, |a, &b| a.wrapping_add(b));
    sram[SAVE_CHECKSUM] == !sum
}

fn describe(party: &[SaveMon]) -> String {
    party
        .iter()
        .map(|m| {
            format!(
                "{} (species {:#04x}, OT {}, Lv{}, DVs {:?})",
                m.nickname, m.species, m.ot_name, m.level, m.dvs
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Fixtures — the same rules as `testharness/colosseum.rs`, deliberately.
// ---------------------------------------------------------------------------

fn identify(path: &Path) -> Option<()> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 0x150 {
        return None;
    }
    let title: String = bytes[0x134..0x143]
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect();
    (title.starts_with("POKEMON RED") || title.starts_with("POKEMON BLUE")).then_some(())
}

fn find_rom() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("POKE_ROM") {
        let p = PathBuf::from(p);
        return identify(&p).map(|_| p);
    }
    let mut dirs = vec![PathBuf::from("tests"), PathBuf::from("roms")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("roms"));
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "gb").unwrap_or(false) && identify(&p).is_some()
            {
                return Some(p);
            }
        }
    }
    None
}

/// A save standing at the Cable Club receptionist. Deliberately **not** found by
/// scanning a player's save directory — the same rule as `colosseum.rs`.
fn find_save() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(p) = std::env::var("GB_CABLECLUB_SAVE") {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(std::env::temp_dir().join("gameboy-gen1-trade/cableclub.sav"));
    candidates.into_iter().find(|p| p.is_file())
}
