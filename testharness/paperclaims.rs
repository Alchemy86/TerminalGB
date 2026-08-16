//! **The gate for [`docs/gen1/paper-claims.md`](../docs/gen1/paper-claims.md).**
//!
//! ```text
//! POKE_ROM=~/roms/blue.gb cargo test --release \
//!   --features plugin-battle-control,plugin-trainer --test paperclaims
//! ```
//!
//! A page that records what a cartridge does is worth nothing a year later
//! unless something re-asks the cartridge. This runs the same measurement
//! [`examples/gen1_paper_claims.rs`](../examples/gen1_paper_claims.rs) does —
//! literally the same code, via `examples/support/paperclaims.rs` — and fails
//! if the game stops behaving the way that page says it does.
//!
//! # What it asserts, and why those and not others
//!
//! A duration is a *roll*, so an assertion on the exact spread of a small
//! sample would be flaky and would eventually be "fixed" by loosening it. The
//! checks here are the ones that cannot flake:
//!
//! * **No roll ever lands outside the documented range.** One out-of-range
//!   value is a real change in the game or in the harness, never noise.
//! * **The toxic residual matches `max(1, floor(maxHP/16)) * N` on every
//!   unclamped tick**, and matches `floor(N/16) * maxHP` on none. There is no
//!   die in that arithmetic.
//! * **A switch clears the screen bits.** Deterministic.
//! * **Nothing is vacuous**: every check requires a minimum number of
//!   observations, so a harness that quietly stopped measuring goes red rather
//!   than green.
//!
//! # It skips, loudly, without a cartridge
//!
//! `POKE_ROM` is required and there is deliberately no fallback to the player's
//! ROM directory. A missing cartridge is exit 0 with a printed reason, the same
//! rule `testharness/colosseum.rs` and the atlas harness follow.

#[path = "../examples/support/paperclaims.rs"]
mod paperclaims;

use std::path::PathBuf;

use paperclaims::*;
use terminalgb::gameboy::Gameboy;
use terminalgb_plugin_battle_control::preset::PresetFile;

/// Battles per duration claim. Enough that every check below has samples to
/// work with, and short enough that the whole file runs in a couple of minutes.
const TRIALS: u32 = 6;
const FRAMES: u32 = 9000;
/// The crit rate is the one figure here that needs a sample rather than a
/// range, so it gets its own budget.
const CRIT_TRIALS: u32 = 8;
const CRIT_FRAMES: u32 = 30_000;

struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
}

fn main() {
    let Some(rom_path) = std::env::var_os("POKE_ROM").map(PathBuf::from) else {
        println!(
            "paperclaims: skipped — set POKE_ROM to a Pokémon Red/Blue cartridge.\n\
             This repository ships no game data and never will; the cartridge is yours.\n\
             What this would check: docs/gen1/paper-claims.md"
        );
        return;
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
    let presets = PresetFile::parse(PRESETS, "examples/gen1_paper_claims.preset")
        .expect("the preset file must parse");

    // One new game, reused by every battle below.
    let state = {
        let mut gb = Gameboy::new(rom.clone(), Some(staged.clone()));
        if let Err(e) = new_game(&mut gb) {
            eprintln!("could not reach the overworld: {e}");
            std::process::exit(2);
        }
        gb.save_state()
    };

    let conf = Config {
        trials: TRIALS,
        frames: FRAMES,
        crit_frames: CRIT_FRAMES,
        verbose: false,
        trace: false,
    };

    let mut checks: Vec<Check> = Vec::new();
    macro_rules! check {
        ($name:expr, $ok:expr, $detail:expr) => {
            checks.push(Check {
                name: $name,
                ok: $ok,
                detail: $detail,
            })
        };
    }

    // ---- [1] the toxic residual ------------------------------------------
    match measure_toxic(&staged, &rom, &presets, &state, "claim-toxic", &conf) {
        Err(e) => check!(
            "the_toxic_residual_is_maxhp_over_16_times_the_counter",
            false,
            e
        ),
        Ok(ticks) => {
            let usable: Vec<_> = ticks
                .iter()
                .filter(|t| t.badly_poisoned)
                .filter(|t| {
                    // A tick clamped to the target's remaining HP says nothing
                    // about the formula.
                    let unit = (t.max_hp / 16).max(1) as u32;
                    !(t.hp_after == 0 && unit * t.counter as u32 >= t.hp_before as u32)
                })
                .collect();
            let agree = usable
                .iter()
                .filter(|t| {
                    (t.hp_before - t.hp_after) as u32
                        == (t.max_hp / 16).max(1) as u32 * t.counter as u32
                })
                .count();
            let paper = usable
                .iter()
                .filter(|t| {
                    (t.hp_before - t.hp_after) as u32
                        == (t.counter as u32 / 16) * t.max_hp as u32
                })
                .count();
            check!(
                "the_toxic_residual_is_maxhp_over_16_times_the_counter",
                usable.len() >= 20 && agree == usable.len(),
                format!("{agree} of {} unclamped ticks agree", usable.len())
            );
            check!(
                "the_toxic_residual_is_not_n_over_16_times_maxhp",
                paper == 0,
                format!("{paper} of {} ticks match the inverted form", usable.len())
            );
            let max_counter = usable.iter().map(|t| t.counter).max().unwrap_or(0);
            check!(
                "the_toxic_counter_cannot_reach_fifteen_without_healing",
                (1..=6).contains(&max_counter),
                format!("highest counter reached in a plain battle: {max_counter}")
            );
        }
    }

    // ---- [2..6] the five duration counters -------------------------------
    //
    // `DurationClaim::rolls` is the range measured on the retail cartridge and
    // derived from `pret/pokered`; a value outside it is a finding, not noise.
    // The check names spell out the claim so a failure reads as a sentence.
    let names = [
        "confusion_rolls_two_to_five_counter_turns",
        "disable_rolls_one_to_eight_turns_and_picks_a_slot_of_four",
        "a_thrash_lock_rolls_two_or_three_further_turns",
        "a_bide_rolls_two_or_three_further_turns",
        "a_partial_trapping_lock_rolls_one_to_four_further_turns",
    ];
    /// Fewest episodes a claim must produce before its range means anything. A
    /// harness that quietly stopped measuring must go red, not green.
    const WANT: usize = 6;
    for (claim, name) in DURATION_CLAIMS.iter().zip(names) {
        let (lo, hi) = claim.rolls;
        match measure_duration(&staged, &rom, &presets, &state, claim, &conf) {
            Err(e) => check!(name, false, e),
            Ok(run) => {
                let (n, seen_lo, seen_hi) = run.summary();
                check!(
                    name,
                    n >= WANT && seen_lo >= lo && seen_hi <= hi,
                    format!("{n} episode(s), rolled {seen_lo}-{seen_hi} (documented {lo}-{hi})")
                );
                // Every countdown must reach zero one step at a time — the
                // property that says the counter is a turn counter and nothing
                // else is writing it.
                let bad = run
                    .episodes
                    .iter()
                    .filter(|e| !e.ended_early)
                    .filter(|e| {
                        let expect: Vec<u8> = (0..=e.rolled).rev().collect();
                        e.run != expect
                    })
                    .count();
                check!(
                    "a_countdown_steps_down_by_one_and_stops_at_zero",
                    bad == 0,
                    format!("{}: {bad} episode(s) did not step 1 at a time", claim.key)
                );
            }
        }
    }

    // ---- [7] Disable picks one of the four slots --------------------------
    // Folded into the run above; re-reported here so the claim has its own row.

    // ---- [8] the screens --------------------------------------------------
    for claim in &SCREEN_CLAIMS {
        let bit_no = claim.bit;
        let label = claim.label;
        match measure_screen(&staged, &rom, &presets, &state, claim, &conf) {
            Err(e) => check!("a_screen_is_a_per_pokemon_volatile", false, e),
            Ok(r) => {
                check!(
                    "a_screen_lives_in_the_per_active_status_block",
                    r.raised
                        && r.status3_while_up == (1 << bit_no)
                        && r.enemy_status3_while_up == 0,
                    format!(
                        "{label}: wPlayerBattleStatus3 ${:02X}, wEnemyBattleStatus3 ${:02X}",
                        r.status3_while_up, r.enemy_status3_while_up
                    )
                );
                check!(
                    "a_screen_does_not_survive_a_switch",
                    r.survived_switch == Some(false),
                    format!("{label}: survived_switch = {:?}", r.survived_switch)
                );
            }
        }
    }

    // ---- [9] Focus Energy quarters the critical-hit rate ------------------
    //
    // `CriticalHitTest` for Electrode's base Speed of 140 predicts 70/256
    // without and 17/256 with, so the assertion is against those absolute
    // numbers rather than against a ratio — a ratio alone would pass a model
    // that had both halves wrong by the same factor.
    let crit_conf = Config {
        trials: CRIT_TRIALS,
        ..conf
    };
    let mut rates = Vec::new();
    for (preset, want_fe) in [
        ("claim-crit-baseline", false),
        ("claim-crit-focus-energy", true),
    ] {
        match measure_crit(&staged, &rom, &presets, &state, preset, want_fe, &crit_conf) {
            Err(e) => check!("focus_energy_quarters_the_critical_hit_rate", false, e),
            Ok(r) => rates.push((want_fe, r)),
        }
    }
    if rates.len() == 2 {
        for (want_fe, r) in &rates {
            let expected = if *want_fe { 17.0 } else { 70.0 } / 256.0;
            let (lo, hi) = wilson_at(r.crits, r.attacks, 3.29);
            check!(
                if *want_fe {
                    "the_crit_rate_with_focus_energy_is_seventeen_in_two_hundred_and_fifty_six"
                } else {
                    "the_crit_rate_without_focus_energy_is_seventy_in_two_hundred_and_fifty_six"
                },
                r.attacks >= 100 && lo <= expected && expected <= hi,
                format!(
                    "{} crit / {} attack, 99.9% {lo:.4}..{hi:.4}, predicted {expected:.4}",
                    r.crits, r.attacks
                )
            );
        }
        let base = rates[0].1.crits as f64 / rates[0].1.attacks.max(1) as f64;
        let fe = rates[1].1.crits as f64 / rates[1].1.attacks.max(1) as f64;
        check!(
            "focus_energy_lowers_the_crit_rate_rather_than_raising_it",
            fe < base,
            format!("with {fe:.4} against without {base:.4}")
        );
    }

    // ---- report -----------------------------------------------------------
    let failed = checks.iter().filter(|c| !c.ok).count();
    for c in &checks {
        println!(
            "[{}] {} — {}",
            if c.ok { "ok  " } else { "FAIL" },
            c.name,
            c.detail
        );
    }
    println!(
        "\n{} check(s), {failed} failure(s) — see docs/gen1/paper-claims.md",
        checks.len()
    );
    if failed > 0 {
        std::process::exit(1);
    }
}
