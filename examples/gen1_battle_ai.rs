//! Measure an agent through the published evaluation protocol.
//!
//! ```text
//! cargo run --release --features gen1-battle --example gen1_battle_ai -- --rom ~/roms/blue.gb
//! cargo run --release --features gen1-battle --example gen1_battle_ai -- \
//!     --rom ~/roms/blue.gb --agent expectimax --depth 3 --episodes 100
//! cargo run --release --features gen1-battle --example gen1_battle_ai -- \
//!     --rom ~/roms/blue.gb --agent 'external:python3 my_llm_adapter.py'
//! ```
//!
//! # The protocol is the product
//!
//! The rules are in [`terminalgb_gen1_battle::eval`] and each one closes a
//! specific way of faking a win rate: the seed comes from the episode index and
//! not from the agent, both sides of every matchup are played, unmodelled
//! effects and illegal choices are counted and printed, and any Gen 1 bug that
//! showed up in a winning line is named. **Every run prints the command that
//! reproduces it**, so a number in a document can be checked rather than
//! trusted.
//!
//! # `--baseline` is not optional in spirit
//!
//! A win rate against an unnamed opponent means nothing, so the run always
//! scores [`terminalgb_gen1_battle::ai::RandomAgent`] as well and prints the two
//! together. An agent that does not clearly beat random has measured nothing,
//! and burying that would be the easiest lie in the whole exercise.
//!
//! # Teams come from the cartridge
//!
//! The evaluation teams are built from the player's own ROM by species name, so
//! no game data is embedded here. `--teams` names them; the default is a small
//! fixed set chosen for type diversity and printed with the result.

use std::path::{Path, PathBuf};
use std::time::Instant;

use terminalgb_gen1_battle::ai::{Agent, Expectimax, ExternalAgent, Greedy, RandomAgent};
use terminalgb_gen1_battle::data::GameData;
use terminalgb_gen1_battle::eval::{Protocol, Report, Team};
use terminalgb_gen1_battle::sim::Engine;
use terminalgb_gen1_battle::state::{BattleMon, Dvs, Party};

/// The default roster, by species name so it is resolved out of the player's
/// own cartridge rather than embedded here.
///
/// Chosen for type spread rather than for power: a win rate measured only on
/// mirror matches would say nothing about type reasoning, which is most of what
/// a Gen 1 battle rewards.
const DEFAULT_TEAMS: &[(&str, &[&str])] = &[
    ("fire", &["CHARMELEON", "GROWLITHE"]),
    ("water", &["WARTORTLE", "PSYDUCK"]),
    ("grass", &["IVYSAUR", "ODDISH"]),
];

struct Args {
    rom: Option<PathBuf>,
    agent: String,
    depth: u8,
    samples: u8,
    episodes: usize,
    seed: u64,
    level: u8,
    badges: u8,
    opponent: String,
    verbose: bool,
}

fn parse() -> Args {
    let mut a = Args {
        rom: None,
        agent: "expectimax".into(),
        depth: 2,
        samples: 3,
        episodes: 25,
        seed: 0x9E37_79B9,
        level: 30,
        badges: 0,
        opponent: "greedy".into(),
        verbose: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--rom" => a.rom = it.next().map(PathBuf::from),
            "--agent" => a.agent = it.next().unwrap_or(a.agent),
            "--opponent" => a.opponent = it.next().unwrap_or(a.opponent),
            "--depth" => {
                a.depth = it.next().and_then(|v| v.parse().ok()).unwrap_or(a.depth)
            }
            "--samples" => {
                a.samples = it.next().and_then(|v| v.parse().ok()).unwrap_or(a.samples)
            }
            "--episodes" => {
                a.episodes = it.next().and_then(|v| v.parse().ok()).unwrap_or(a.episodes)
            }
            "--seed" => a.seed = it.next().and_then(|v| v.parse().ok()).unwrap_or(a.seed),
            "--level" => {
                a.level = it.next().and_then(|v| v.parse().ok()).unwrap_or(a.level)
            }
            "--badges" => {
                a.badges = it.next().and_then(|v| v.parse().ok()).unwrap_or(a.badges)
            }
            "--verbose" | "-v" => a.verbose = true,
            "--help" | "-h" => {
                eprintln!(
                    "gen1_battle_ai — measure an agent through the published protocol\n\n\
                     --rom <path>       the cartridge the teams are built from\n\
                     --agent <spec>     random | greedy | expectimax | external:<command>\n\
                     --opponent <spec>  the fixed opponent (default: greedy)\n\
                     --depth <n>        expectimax plies (default 2)\n\
                     --samples <n>      dice samples per chance node (default 3)\n\
                     --episodes <n>     per matchup per side (default 25)\n\
                     --seed <n>         the base seed; the whole run reproduces from it\n\
                     --level <n>        the level every Pokémon is built at (default 30)\n\
                     --badges <n>       wObtainedBadges, as a byte (default 0)\n"
                );
                std::process::exit(0);
            }
            other => eprintln!("ignoring unknown argument `{other}`"),
        }
    }
    a
}

fn find_rom() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = Path::new(&home).join("roms");
    for e in std::fs::read_dir(dir).ok()? {
        let p = e.ok()?.path();
        let name = p.file_name()?.to_string_lossy().to_lowercase();
        if name.ends_with(".gb") && (name.contains("blue") || name.contains("red")) {
            return Some(p);
        }
    }
    None
}

/// Resolve a species name against the cartridge's own `MonsterNames`.
fn species_index(data: &GameData, want: &str) -> Option<u8> {
    let want = want.to_uppercase();
    (1u8..=255)
        .find(|&i| data.species_name(i).map(str::to_uppercase).as_deref() == Some(&want))
}

/// Build a Pokémon at `level` with the four moves the cartridge would give it —
/// its base learnset is not in the tables this crate reads, so the moves are the
/// strongest ones it could plausibly have. Printed with the result, because a
/// team is part of the claim.
fn build(data: &GameData, name: &str, level: u8) -> Result<(BattleMon, String), String> {
    let idx =
        species_index(data, name).ok_or_else(|| format!("no species called `{name}`"))?;
    let base = *data
        .base_stats_for_index(idx)
        .ok_or_else(|| format!("`{name}` has no base stats"))?;

    // Four moves: the strongest damaging move of each of the species' own types
    // plus a neutral filler. Deterministic, derived from the cartridge, and
    // printed with the result — never hand-picked to flatter an agent.
    //
    // Two effects are excluded, and the exclusion is the interesting part.
    // `EXPLODE` faints the user and `OHKO` ends the battle on a coin flip; a
    // roster containing either measures a different game — the first run of
    // this harness handed every team Explosion and both agents spent the
    // evaluation killing themselves. Everything else the cartridge offers,
    // including recoil, charging and Hyper Beam, is left in.
    let excluded = |e: u8| {
        matches!(
            e,
            terminalgb_gen1_battle::effects::id::EXPLODE
                | terminalgb_gen1_battle::effects::id::OHKO
        )
    };
    let mut picks: Vec<u8> = Vec::new();
    let mut want = vec![base.type1];
    if base.type2 != base.type1 {
        want.push(base.type2);
    }
    want.push(terminalgb_gen1_battle::types::Type::NORMAL);
    for t in want {
        let best = data
            .moves()
            .iter()
            .filter(|m| {
                m.kind == t
                    && m.power > 0
                    && m.accuracy >= 200
                    && !excluded(m.effect)
                    && !picks.contains(&m.id)
            })
            .max_by_key(|m| u16::from(m.power) * u16::from(m.accuracy));
        if let Some(m) = best {
            picks.push(m.id);
        }
    }
    // The filler is the strongest remaining move that is not degenerate — the
    // first version took the *weakest*, which handed every team SONICBOOM.
    while picks.len() < 4 {
        let filler = data
            .moves()
            .iter()
            .filter(|m| {
                m.power > 0
                    && m.accuracy >= 200
                    && !excluded(m.effect)
                    && !picks.contains(&m.id)
            })
            .max_by_key(|m| u16::from(m.power) * u16::from(m.accuracy));
        match filler {
            Some(m) => picks.push(m.id),
            None => break,
        }
    }
    let mut ids = [0u8; 4];
    for (i, id) in picks.iter().take(4).enumerate() {
        ids[i] = *id;
    }

    let mon = BattleMon::build(data, idx, level, Dvs::PERFECT, ids)
        .ok_or_else(|| format!("could not build `{name}`"))?;
    let moves = ids
        .iter()
        .filter(|&&i| i != 0)
        .map(|&i| data.move_name(i).unwrap_or("?").to_string())
        .collect::<Vec<_>>()
        .join("/");
    Ok((mon, format!("{name} L{level} [{moves}]")))
}

fn make_agent(
    spec: &str,
    data: &GameData,
    args: &Args,
) -> Result<Box<dyn Agent>, String> {
    match spec {
        "random" => Ok(Box::new(RandomAgent::new(1))),
        "greedy" => Ok(Box::new(Greedy::new())),
        "expectimax" => Ok(Box::new(Expectimax::new(
            data.clone(),
            args.depth,
            args.samples,
        ))),
        s if s.starts_with("external:") => {
            let cmd = &s["external:".len()..];
            let mut parts = cmd.split_whitespace();
            let program = parts.next().ok_or("`external:` needs a command")?;
            let rest: Vec<String> = parts.map(str::to_string).collect();
            Ok(Box::new(ExternalAgent::new(cmd.to_string(), program, rest)))
        }
        other => Err(format!(
            "unknown agent `{other}` — try random, greedy, expectimax, or external:<command>"
        )),
    }
}

fn main() {
    let args = parse();
    let rom_path = match args.rom.clone().or_else(find_rom) {
        Some(p) => p,
        None => {
            eprintln!(
                "no cartridge. Pass --rom <path>, or put a Pokémon Red/Blue image in ~/roms.\n\
                 The teams are built from the cartridge's own tables; this repository ships\n\
                 no game data."
            );
            std::process::exit(2);
        }
    };
    let rom = std::fs::read(&rom_path).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", rom_path.display());
        std::process::exit(2);
    });
    let data = GameData::load_from_rom(&rom).unwrap_or_else(|e| {
        eprintln!("this cartridge cannot be read as Generation 1 battle data: {e}");
        std::process::exit(2);
    });

    // Build the teams, and print them — a team is part of the claim.
    let mut teams = Vec::new();
    let mut descriptions = Vec::new();
    for (name, members) in DEFAULT_TEAMS {
        let mut mons = Vec::new();
        let mut desc = Vec::new();
        for m in *members {
            match build(&data, m, args.level) {
                Ok((mon, d)) => {
                    mons.push(mon);
                    desc.push(d);
                }
                Err(e) => {
                    eprintln!("team `{name}`: {e}");
                    std::process::exit(2);
                }
            }
        }
        descriptions.push(format!("  {name}: {}", desc.join(", ")));
        teams.push(Team::new(*name, Party::new(mons)));
    }

    let engine = Engine::new(data.clone());
    let mut protocol = Protocol::new(args.seed, args.episodes);
    protocol.teams = teams.clone();
    protocol.opponents = teams;
    protocol.opponent_policy = args.opponent.clone();
    protocol.badges = args.badges;

    println!("cartridge   {}", rom_path.display());
    println!("engine      v{}", terminalgb_gen1_battle::ENGINE_VERSION);
    println!("teams (each plays every other, from both sides):");
    for d in &descriptions {
        println!("{d}");
    }
    println!(
        "reproduce   --seed {} --episodes {} --level {} --badges {} --agent {} --opponent {}",
        args.seed, args.episodes, args.level, args.badges, args.agent, args.opponent
    );
    println!();

    let opponent_spec = args.opponent.clone();
    let make_opponent = || {
        make_agent(&opponent_spec, &data, &args).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2);
        })
    };

    let run = |spec: &str| -> Report {
        let mut agent = make_agent(spec, &data, &args).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2);
        });
        let started = Instant::now();
        let r = protocol.run(&engine, agent.as_mut(), make_opponent);
        if args.verbose {
            eprintln!("{spec}: {:.2}s", started.elapsed().as_secs_f32());
        }
        r
    };

    // The baseline is not optional. A win rate that is not clearly above
    // random's has measured nothing, and the two are printed together so nobody
    // has to take that on trust.
    let baseline = run("random");
    let subject = run(&args.agent);

    println!("── baseline ─────────────────────────────────────────────");
    print!("{}", baseline.to_text());
    println!(
        "\n── {} ─────────────────────────────────────────",
        args.agent
    );
    print!("{}", subject.to_text());

    let (blo, bhi) = baseline.win_rate_interval();
    let (slo, _shi) = subject.win_rate_interval();
    println!();
    if slo > bhi {
        println!(
            "VERDICT: {} beats the random baseline — its 95% interval ({:.1}–{:.1}%) \n\
             does not overlap random's ({:.1}–{:.1}%).",
            args.agent,
            slo * 100.0,
            _shi * 100.0,
            blo * 100.0,
            bhi * 100.0
        );
    } else {
        println!(
            "VERDICT: {} is NOT distinguishable from random at this sample size.\n\
             Its interval ({:.1}–{:.1}%) overlaps random's ({:.1}–{:.1}%). Run more\n\
             episodes, or the agent is not doing anything.",
            args.agent,
            slo * 100.0,
            _shi * 100.0,
            blo * 100.0,
            bhi * 100.0
        );
    }
}
