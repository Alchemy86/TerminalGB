//! **Measure the Generation 1 mechanics an outside paper asserted, on the
//! cartridge.**
//!
//! ```text
//! cargo run --release --features gen1-battle,plugin-battle-control,plugin-trainer \
//!   --example gen1_paper_claims -- --rom ~/roms/blue.gb
//! ```
//!
//! # What this is
//!
//! [`docs/gen1/paper-claims.md`](../docs/gen1/paper-claims.md) checks the Gen 1
//! state-space derivation in *The PokéAgent Challenge* (arXiv 2603.15563v2,
//! Appendix G) against real hardware behaviour. This is the instrument that
//! produced the "measured" column of that table.
//!
//! It is **not** a differential harness like
//! [`gen1_battle_sim`](../examples/gen1_battle_sim.rs), and the difference
//! matters: that one holds *our model* to the cartridge, this one asks the
//! cartridge a question our model has no standing to answer. Nothing here reads
//! `terminalgb-gen1-battle` at all. Every number printed is a byte the game
//! wrote to its own work RAM.
//!
//! # How a duration is measured
//!
//! Each volatile counter lives at a symbol in `pret/pokered`'s own map file,
//! and every one of them is in
//! [the atlas](../third_party/atlasgb/pokemon-rb/atlas.tsv) with live evidence.
//! The harness samples them **every frame** and records each `0 -> n`
//! transition (the roll) and the descending run that follows (how the game
//! spends it). No inference, no model: the counter is the measurement.
//!
//! # Two things this deliberately does not do
//!
//! It does not touch the player's ROM directory — the cartridge is copied into
//! `target/` first, because the MBC writes a `.sav` next to the ROM it is given
//! and this harness presses buttons at menus. And it writes no baseline of its
//! own output: a baseline here would be this program marking its own homework.

use std::path::PathBuf;
use std::time::Instant;

use terminalgb::gameboy::Gameboy;
use terminalgb_plugin_battle_control::preset::PresetFile;

// The measurement layer, shared with `testharness/paperclaims.rs` so the
// instrument and the gate cannot drift apart.
#[path = "support/paperclaims.rs"]
mod paperclaims;

use paperclaims::*;

struct Args {
    rom: Option<PathBuf>,
    claims: Vec<String>,
    trials: u32,
    frames: u32,
    crit_frames: u32,
    verbose: bool,
    trace: bool,
    json: bool,
}

fn parse() -> Args {
    let mut a = Args {
        rom: None,
        claims: Vec::new(),
        trials: 24,
        frames: 6000,
        crit_frames: 120_000,
        verbose: false,
        trace: false,
        json: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--rom" => a.rom = it.next().map(PathBuf::from),
            "--claim" => {
                if let Some(v) = it.next() {
                    a.claims.push(v);
                }
            }
            "--trials" => a.trials = it.next().and_then(|v| v.parse().ok()).unwrap_or(24),
            "--frames" => {
                a.frames = it.next().and_then(|v| v.parse().ok()).unwrap_or(6000)
            }
            "--crit-frames" => {
                a.crit_frames = it.next().and_then(|v| v.parse().ok()).unwrap_or(120_000)
            }
            "--verbose" | "-v" => a.verbose = true,
            "--trace" => {
                a.trace = true;
                a.verbose = true;
            }
            "--json" => a.json = true,
            "--help" | "-h" => {
                eprintln!(
                    "gen1_paper_claims — measure Gen 1 volatile mechanics on the cartridge\n\n\
                     --rom <path>        the cartridge (default: a Pokémon in ~/roms)\n\
                     --claim <name>      toxic|confusion|disable|thrash|bide|trapping|\n\
                     \x20                   screens|crit   (repeatable, default: all)\n\
                     --trials <n>        battles per duration claim (default 24)\n\
                     --frames <n>        frames per duration battle (default 6000)\n\
                     --crit-frames <n>   frames per crit-rate battle (default 120000)\n\
                     --verbose           print every sample\n\
                     --trace             also print the frame of every counter change\n\
                     --json              one machine-readable object at the end\n"
                );
                std::process::exit(0);
            }
            other => eprintln!("ignoring unknown argument `{other}`"),
        }
    }
    a
}

fn wanted(args: &Args, name: &str) -> bool {
    args.claims.is_empty() || args.claims.iter().any(|c| c.eq_ignore_ascii_case(name))
}

fn main() {
    let args = parse();
    let conf = Config {
        trials: args.trials,
        frames: args.frames,
        crit_frames: args.crit_frames,
        verbose: args.verbose,
        trace: args.trace,
    };
    let rom_path = match args.rom.clone().or_else(find_rom) {
        Some(p) => p,
        None => {
            eprintln!(
                "no cartridge. Pass --rom <path>, or put a Pokémon Red/Blue image in ~/roms.\n\
                 This repository ships no game data and never will; the cartridge is yours."
            );
            std::process::exit(2);
        }
    };
    let rom = match std::fs::read(&rom_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot read {}: {e}", rom_path.display());
            std::process::exit(2);
        }
    };
    let staged = match stage(&rom_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let presets = match PresetFile::parse(
        include_str!("gen1_paper_claims.preset"),
        "examples/gen1_paper_claims.preset",
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("the preset file does not parse: {e}");
            std::process::exit(2);
        }
    };

    println!("cartridge   {}", rom_path.display());
    println!("staged      {}", staged.display());
    println!("trials      {} per duration claim", args.trials);
    println!();

    // One new game, reused by every battle. Playing the intro once instead of
    // once per trial is most of this harness's wall clock.
    let started = Instant::now();
    let state = {
        let mut gb = Gameboy::new(rom.clone(), Some(staged.clone()));
        // A machine with no APU is not a quiet Game Boy, it is one with the sound
        // chip pulled out, and a cartridge can tell — see `examples/blackout_probe.rs`.
        gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
        if let Err(e) = new_game(&mut gb) {
            eprintln!("{e}");
            std::process::exit(2);
        }
        gb.save_state()
    };
    println!(
        "overworld reached and saved ({} bytes) in {:.1}s",
        state.len(),
        started.elapsed().as_secs_f64()
    );
    println!();

    let mut failures = 0u32;
    let mut json: Vec<String> = Vec::new();

    macro_rules! fail {
        ($e:expr) => {{
            eprintln!("  FAILED: {}", $e);
            failures += 1;
        }};
    }

    // ---- Toxic ----------------------------------------------------------
    for (name, preset, title) in [
        (
            "toxic",
            "claim-toxic",
            "TOXIC — the badly-poisoned residual",
        ),
        (
            "toxic-heal",
            "claim-toxic-heal",
            "TOXIC AGAINST A HEALING TARGET — how far the counter climbs",
        ),
    ] {
        if wanted(&args, name) {
            println!("{title}");
            match measure_toxic(&staged, &rom, &presets, &state, preset, &conf) {
                Err(e) => fail!(e),
                Ok(ticks) => {
                    let mut agree = 0u32;
                    let mut disagree = 0u32;
                    let mut max_counter = 0u8;
                    let not_poisoned = ticks.iter().filter(|t| !t.badly_poisoned).count();
                    if not_poisoned > 0 {
                        println!(
                        "    {not_poisoned} tick(s) had BADLY_POISONED clear — not this mechanic, dropped"
                    );
                    }
                    for t in ticks.iter().filter(|t| t.badly_poisoned) {
                        let unit = (t.max_hp / 16).max(1);
                        let expected = unit as u32 * t.counter as u32;
                        let observed = (t.hp_before - t.hp_after) as u32;
                        // A tick that would take the target below zero is clamped
                        // to the remaining HP, so it says nothing about the formula.
                        let clamped = t.hp_after == 0 && expected >= t.hp_before as u32;
                        if clamped {
                            continue;
                        }
                        if observed == expected {
                            agree += 1;
                        } else {
                            disagree += 1;
                            println!(
                                "    counter {} maxHP {} -> expected {} got {}",
                                t.counter, t.max_hp, expected, observed
                            );
                        }
                        max_counter = max_counter.max(t.counter);
                        if args.verbose {
                            println!(
                                "    n={} maxHP={} unit={} hp {}->{} (drop {})",
                                t.counter,
                                t.max_hp,
                                unit,
                                t.hp_before,
                                t.hp_after,
                                observed
                            );
                        }
                    }
                    println!(
                    "  {} unclamped tick(s): {agree} match max(1, floor(maxHP/16)) * N, {disagree} do not",
                    agree + disagree
                );
                    println!(
                        "  highest counter reached in a plain battle: {max_counter}"
                    );
                    // The other formula, for the record.
                    let wrong = ticks
                        .iter()
                        .filter(|t| {
                            (t.hp_before - t.hp_after) as u32
                                == (t.counter as u32 / 16) * t.max_hp as u32
                        })
                        .count();
                    println!(
                    "  ticks consistent with floor(N/16) * maxHP instead: {wrong} of {}",
                    ticks.len()
                );
                    json.push(format!(
                    "\"{name}\":{{\"ticks\":{},\"agree\":{agree},\"disagree\":{disagree},\"max_counter\":{max_counter}}}",
                    ticks.len()
                ));
                    if disagree > 0 {
                        failures += 1;
                    }
                }
            }
            println!();
        }
    }

    // ---- the five duration counters -------------------------------------
    for claim in &DURATION_CLAIMS {
        if !wanted(&args, claim.key) {
            continue;
        }
        println!("{} — from `{}`", claim.label.to_uppercase(), claim.preset);
        match measure_duration(&staged, &rom, &presets, &state, claim, &conf) {
            Err(e) => fail!(e),
            Ok(run) => {
                run.report(args.verbose);
                let (n, lo, hi) = run.summary();
                json.push(format!(
                    "\"{}\":{{\"episodes\":{n},\"rolled_min\":{lo},\"rolled_max\":{hi}}}",
                    claim.key
                ));
                if n == 0 {
                    failures += 1;
                }
            }
        }
        println!();
    }

    // ---- screens ---------------------------------------------------------
    if wanted(&args, "screens") {
        println!("REFLECT AND LIGHT SCREEN — where the bit lives, and a switch");
        for claim in &SCREEN_CLAIMS {
            let b = claim.bit;
            match measure_screen(&staged, &rom, &presets, &state, claim, &conf) {
                Err(e) => fail!(e),
                Ok(r) => {
                    println!(
                        "  {:<14} raised: {}  wPlayerBattleStatus3 while up: ${:02X}  (bit {b})",
                        r.label, r.raised, r.status3_while_up
                    );
                    println!(
                        "  {:<14} wEnemyBattleStatus3 at the same moment: ${:02X}",
                        "", r.enemy_status3_while_up
                    );
                    match r.survived_switch {
                        None => println!(
                            "  {:<14} the switch did not happen — not determinable",
                            ""
                        ),
                        Some(true) => println!(
                            "  {:<14} SURVIVED the switch — the bit is still set",
                            ""
                        ),
                        Some(false) => println!(
                            "  {:<14} CLEARED by the switch — the bit is gone",
                            ""
                        ),
                    }
                    if !r.block_before.is_empty() {
                        let fmt = |v: &[u8]| {
                            v.iter()
                                .map(|x| format!("{x:02X}"))
                                .collect::<Vec<_>>()
                                .join(" ")
                        };
                        println!(
                            "  {:<14} $D060-$D069 before the switch: {}",
                            "",
                            fmt(&r.block_before)
                        );
                        println!(
                            "  {:<14} $D060-$D069 after  the switch: {}",
                            "",
                            fmt(&r.block_after)
                        );
                    }
                    json.push(format!(
                        "\"{}\":{{\"raised\":{},\"survived_switch\":{}}}",
                        r.label.to_lowercase().replace(' ', "_"),
                        r.raised,
                        match r.survived_switch {
                            None => "null".into(),
                            Some(v) => v.to_string(),
                        }
                    ));
                    if !r.raised {
                        failures += 1;
                    }
                }
            }
        }
        println!();
    }

    // ---- the critical-hit rate -------------------------------------------
    if wanted(&args, "crit") {
        println!("CRITICAL HITS — the rate with and without Focus Energy");
        let mut rates = Vec::new();
        for (preset, label, fe) in [
            ("claim-crit-baseline", "no Focus Energy", false),
            ("claim-crit-focus-energy", "Focus Energy up", true),
        ] {
            match measure_crit(&staged, &rom, &presets, &state, preset, fe, &conf) {
                Err(e) => fail!(e),
                Ok(r) => {
                    let (lo, hi) = wilson(r.crits, r.attacks);
                    let p = if r.attacks > 0 {
                        r.crits as f64 / r.attacks as f64
                    } else {
                        f64::NAN
                    };
                    println!(
                        "  {:<18} {:>4} crit / {:>4} attack = {:.4}  (95% {:.4}..{:.4}){}",
                        label,
                        r.crits,
                        r.attacks,
                        p,
                        lo,
                        hi,
                        if fe && !r.pumped {
                            "   *** Focus Energy never came up ***"
                        } else {
                            ""
                        }
                    );
                    println!(
                        "  {:<18} over {} battle(s){}",
                        "",
                        r.battles,
                        if r.stopped.is_empty() {
                            String::new()
                        } else {
                            format!("; last: {}", r.stopped)
                        }
                    );
                    if r.before_pump > 0 {
                        println!(
                            "  {:<18} ({} attack(s) before Focus Energy landed, discarded)",
                            "", r.before_pump
                        );
                    }
                    rates.push((label, r.crits, r.attacks, p));
                    json.push(format!(
                        "\"crit_{}\":{{\"crits\":{},\"attacks\":{},\"rate\":{:.6}}}",
                        if fe { "focus_energy" } else { "baseline" },
                        r.crits,
                        r.attacks,
                        p
                    ));
                }
            }
        }
        if rates.len() == 2 && rates[0].3 > 0.0 {
            println!(
                "  ratio (Focus Energy / baseline): {:.3}   — 4.0 would be the folklore, 0.243 the bug",
                rates[1].3 / rates[0].3
            );
        }
        // What `CriticalHitTest` computes for THIS attacker, from the source,
        // so the measurement is checked against an absolute prediction and not
        // only against itself. Electrode's base Speed is 140.
        //
        //   b = 140 >> 1 = 70
        //   no Focus Energy: sla b -> 140, then srl b for a normal move -> 70
        //   Focus Energy:    srl b ->  35, then srl b for a normal move -> 17
        //
        // and the crit fires when a rotated random byte compares below b, which
        // is uniform over 0..255. Note the ratio is 17/70 = 0.243, not exactly a
        // quarter: the second right-shift truncates 35/2 to 17.
        println!(
            "  predicted by CriticalHitTest for base Speed 140: {:.4} without, {:.4} with (ratio {:.3})",
            70.0 / 256.0,
            17.0 / 256.0,
            17.0 / 70.0
        );
        println!();
    }

    if args.json {
        println!("{{{}}}", json.join(","));
    }
    println!(
        "done in {:.1}s, {failures} failure(s)",
        started.elapsed().as_secs_f64()
    );
    if failures > 0 {
        std::process::exit(1);
    }
}
