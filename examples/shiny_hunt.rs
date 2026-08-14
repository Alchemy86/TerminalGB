//! Gen 1 shiny hunting — N headless emulators, one encounter loop each.
//!
//! **Gen 1 has no shiny Pokémon.** This hunts the DV spread that *becomes*
//! shiny when the Pokémon is traded forward to Gen 2, which is the only thing
//! "shiny hunting in Gen 1" can mean. The rule is
//! `terminalgb_plugin_trainer::derived::shiny_if_traded` and there is one copy of
//! it in the tree. Full write-up: `docs/shiny-hunt.md`.
//!
//! ```text
//! # build a starting position from a fresh game (Route 1 grass)
//! cargo run --release --example shiny_hunt -- --rom blue.gb --make-state route1.gbss
//!
//! # hunt
//! cargo run --release --example shiny_hunt -- --rom blue.gb --state route1.gbss
//!
//! # prove the instances diverge
//! cargo run --release --example shiny_hunt -- --rom blue.gb --state route1.gbss --probe 3
//!
//! # demonstrate stop-on-hit against a deliberately common spread
//! cargo run --release --example shiny_hunt -- --rom blue.gb --state route1.gbss \
//!     --target spc=10 --out /tmp/hit
//! ```
//!
//! It never writes next to the ROM: every machine is built with
//! `Gameboy::new(bytes, None)`, and `src/mbc/battery.rs` only wires a battery
//! when it is given a path. A hunt therefore cannot touch anybody's real save,
//! however many instances it runs.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use terminalgb::gameboy::Gameboy;
use terminalgb::KeypadKey;
use terminalgb_plugin_gen1_shinyhunt::hunt::{
    self, default_instances, Hunt, HuntConfig, Outcome, Shape, Snapshot,
};
use terminalgb_plugin_gen1_shinyhunt::{ShinyHunt, Target, Watch};
use terminalgb_plugin_trainer::rom::RomTables;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        usage();
        return;
    }
    match run(&args) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("shiny_hunt: {e}");
            std::process::exit(2);
        }
    }
}

fn usage() {
    println!(
        "\
Gen 1 shiny hunting: many headless emulators rolling encounters in parallel.

  --rom <path>            Pokémon Red or Blue (required)
  --state <path>          the save state every attempt restarts from
  --make-state <path>     build one: a fresh game walked to Route 1 grass, then exit
  --target <spec>         what to stop for; default `shiny`
                          shiny | perfect | spc=10 | spd>=14 | atk&2 |
                          total>=70 | species=NIDORAN  (comma or space separated)
  --shape grass|reset     wild grass (default), or a static/gift reached by reset
  --watch party:<n>       the Pokémon lands in party slot n instead of a battle
  --instances <n>         default {n} (cores - 1)
  --seed <n>              default is fixed, so a whole hunt is reproducible
  --max-attempts <n>      stop after this many attempts
  --time-limit <secs>     stop after this long
  --out <dir>             where a hit's state and report are written
  --probe <n>             run n attempts per instance, print the DV sequences
                          and whether they diverged, then exit
  --run-length <n>        encounters met per reload, fleeing between them
                          (default 32). This is what lets a wild hunt reach more
                          than a thin band of the DV space — see
                          docs/shiny-hunt.md §4. 1 = reload every time.
  --no-decorrelate        give every instance the same schedule (the control
                          case for --probe; never hunt with it)
  --quiet                 no progress line
  --diagnose              walk one attempt with a running trace of the game's
                          own state, then exit — what to run when a hunt reports
                          nothing but stalls
  --replay <inst>:<att>   re-run exactly one attempt in a fresh machine and print
                          what it rolled. With the same --state, --seed,
                          --instances, --shape and --target this is how a hit is
                          reproduced from three numbers rather than from a file.

Exit status: 0 a hit, 1 no hit within the limits, 2 an error.",
        n = default_instances()
    );
}

struct Args {
    rom: PathBuf,
    state: Option<PathBuf>,
    make_state: Option<PathBuf>,
    target: String,
    shape: Shape,
    watch: Watch,
    instances: usize,
    seed: Option<u64>,
    max_attempts: Option<u64>,
    time_limit: Option<Duration>,
    out: Option<PathBuf>,
    probe: Option<u64>,
    decorrelate: bool,
    quiet: bool,
    diagnose: bool,
    replay: Option<(usize, u64)>,
    run_length: Option<u32>,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut a = Args {
        rom: PathBuf::new(),
        state: None,
        make_state: None,
        target: "shiny".into(),
        shape: Shape::Grass,
        watch: Watch::Encounter,
        instances: default_instances(),
        seed: None,
        max_attempts: None,
        time_limit: None,
        out: None,
        probe: None,
        decorrelate: true,
        quiet: false,
        diagnose: false,
        replay: None,
        run_length: None,
    };
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let mut value = || -> Result<String, String> {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag {
            "--rom" => a.rom = PathBuf::from(value()?),
            "--state" => a.state = Some(PathBuf::from(value()?)),
            "--make-state" => a.make_state = Some(PathBuf::from(value()?)),
            "--target" => a.target = value()?,
            "--shape" => {
                let v = value()?;
                a.shape =
                    Shape::parse(&v).ok_or_else(|| format!("unknown shape `{v}`"))?;
            }
            "--watch" => {
                let v = value()?;
                a.watch = match v.strip_prefix("party:") {
                    Some(n) => Watch::PartySlot(
                        n.parse().map_err(|_| format!("`{v}`: not a party slot"))?,
                    ),
                    None if v == "encounter" => Watch::Encounter,
                    None => return Err(format!("unknown watch `{v}`")),
                };
            }
            "--instances" => {
                a.instances =
                    value()?.parse().map_err(|_| "--instances wants a number")?
            }
            "--seed" => {
                a.seed = Some(value()?.parse().map_err(|_| "--seed wants a number")?)
            }
            "--max-attempts" => {
                a.max_attempts = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--max-attempts wants a number")?,
                )
            }
            "--time-limit" => {
                let s: f64 =
                    value()?.parse().map_err(|_| "--time-limit wants seconds")?;
                a.time_limit = Some(Duration::from_secs_f64(s));
            }
            "--out" => a.out = Some(PathBuf::from(value()?)),
            "--probe" => {
                a.probe = Some(value()?.parse().map_err(|_| "--probe wants a number")?)
            }
            "--run-length" => {
                a.run_length = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--run-length wants a number")?,
                )
            }
            "--no-decorrelate" => a.decorrelate = false,
            "--quiet" => a.quiet = true,
            "--diagnose" => a.diagnose = true,
            "--replay" => {
                let v = value()?;
                let (i, n) = v.split_once(':').ok_or_else(|| {
                    format!("`{v}`: --replay wants <instance>:<attempt>")
                })?;
                a.replay = Some((
                    i.parse().map_err(|_| format!("`{v}`: bad instance"))?,
                    n.parse().map_err(|_| format!("`{v}`: bad attempt"))?,
                ));
            }
            other => return Err(format!("unknown option `{other}` (try --help)")),
        }
        i += 1;
    }
    if a.rom.as_os_str().is_empty() {
        return Err("--rom is required".into());
    }
    if a.instances == 0 {
        return Err("--instances must be at least 1".into());
    }
    Ok(a)
}

fn run(args: &[String]) -> Result<i32, String> {
    let a = parse_args(args)?;
    let rom = std::fs::read(&a.rom).map_err(|e| format!("{}: {e}", a.rom.display()))?;
    let tables = RomTables::new(rom.clone());

    if let Some(dest) = &a.make_state {
        return make_state(&rom, dest).map(|()| 0);
    }

    let state_path = a
        .state
        .as_ref()
        .ok_or("--state is required (or use --make-state to build one)")?;
    let base_state = std::fs::read(state_path)
        .map_err(|e| format!("{}: {e}", state_path.display()))?;

    // A species *name* is resolved against the player's own cartridge — we ship
    // no species table (`AGENTS.md` §"Licence boundary").
    let lookup = |name: &str| -> Option<(u8, String)> {
        (1u8..=190).find_map(|i| {
            let n = tables.species_name(i);
            n.eq_ignore_ascii_case(name.trim()).then_some((i, n))
        })
    };
    let target = Target::parse(&a.target, Some(&lookup))?;

    let mut cfg = HuntConfig::new(base_state, target);
    cfg.shape = a.shape;
    cfg.watch = a.watch;
    cfg.instances = a.instances;
    cfg.decorrelate = a.decorrelate;
    cfg.max_attempts = a.max_attempts;
    cfg.time_limit = a.time_limit;
    if let Some(s) = a.seed {
        cfg.seed = s;
    }
    if let Some(r) = a.run_length {
        cfg.run_length = r;
    }

    let build = {
        let rom = &rom;
        let target = cfg.target.clone();
        let watch = cfg.watch;
        move |_index: usize| -> Result<Gameboy, String> {
            // No file path: nothing on the filesystem is opened, created or
            // written by an instance, so a hunt cannot touch a real save.
            let mut gb = Gameboy::try_new_with_model(
                rom.clone(),
                None,
                terminalgb::mode::Model::Auto,
            )
            .map_err(|e| e.to_string())?;
            gb.add_plugin(Box::new(ShinyHunt::new(target.clone()).watching(watch)))
                .map_err(|e| e.to_string())?;
            Ok(gb)
        }
    };

    println!(
        "cartridge  {} ({} KiB)",
        a.rom.file_name().unwrap_or_default().to_string_lossy(),
        rom.len() / 1024
    );
    println!("target     {}", cfg.target.label());
    let p = cfg.target.dv_probability();
    println!(
        "odds       {:.6} per encounter  (1 in {:.0}; median {:.0} encounters)",
        p,
        cfg.target.expected_encounters(),
        cfg.target.median_encounters()
    );
    if let Some(c) = cfg.target.species_caveat() {
        println!("           NOTE: {c}");
    }
    println!(
        "shape      {}   instances {}   run length {}   seed {:#018x}{}",
        cfg.shape.label(),
        cfg.instances,
        cfg.run_length,
        cfg.seed,
        if cfg.decorrelate {
            ""
        } else {
            "   DECORRELATION OFF (control case)"
        }
    );
    println!();

    if a.diagnose {
        return diagnose(&cfg, &build, &tables);
    }
    if let Some((instance, attempt)) = a.replay {
        if instance >= cfg.instances {
            return Err(format!(
                "instance {instance} does not exist in a hunt of {} — pass the \
                 same --instances the hit was found with, since the instance \
                 index is what picks its seed",
                cfg.instances
            ));
        }
        let started = Instant::now();
        let got = hunt::replay(build(instance)?, &cfg, instance, attempt)?;
        if got.is_empty() {
            println!("instance {instance} attempt {attempt}: stalled, no encounter");
            return Ok(1);
        }
        println!(
            "instance {instance} attempt {attempt} (seed {:#018x}), replayed in {}:",
            cfg.seed_for(instance),
            human_time(started.elapsed())
        );
        let mut hit = 1;
        for (i, s) in got.iter().enumerate() {
            let matched = cfg.target.matches(s.dvs, s.species);
            if matched {
                hit = 0;
            }
            println!(
                "  #{i:<3} {:<12} lv{:<3} {}  {}",
                tables.species_name(s.species),
                s.level,
                s.dv_line(),
                if matched { "<- MATCHES THE TARGET" } else { "" }
            );
        }
        return Ok(hit);
    }

    if let Some(n) = a.probe {
        return probe_mode(&cfg, &build, n, &tables);
    }

    let started = Instant::now();
    let mut last_line = Instant::now();
    let outcome = Hunt::run(&cfg, &build, Duration::from_millis(250), |s| {
        if a.quiet || last_line.elapsed() < Duration::from_millis(500) {
            return;
        }
        last_line = Instant::now();
        eprint!("\r{}\x1b[K", progress_line(s));
    });
    if !a.quiet {
        eprintln!();
    }
    report(&outcome, &cfg, &tables, started);

    if let (Some(hit), Some(dir)) = (&outcome.hit, &a.out) {
        preserve(hit, dir, &cfg, &tables, &a.rom)?;
    } else if outcome.hit.is_some() {
        println!(
            "\n(pass --out <dir> to write the save state, the report and a frame \
             of the encounter)"
        );
    }
    Ok(if outcome.hit.is_some() { 0 } else { 1 })
}

fn progress_line(s: &Snapshot) -> String {
    let eta = match s.eta_median() {
        Some(d) => format!("median ETA {}", human_time(d)),
        None => "median ETA -".into(),
    };
    format!(
        "{:>8} enc  {:>8} att  {:>5.1} enc/s  {:>6.0}x realtime  {}  {}",
        s.encounters,
        s.attempts,
        s.encounters_per_second(),
        s.speed_multiple(),
        human_time(s.elapsed),
        eta
    )
}

fn report(out: &Outcome, cfg: &HuntConfig, tables: &RomTables, started: Instant) {
    let elapsed = started.elapsed();
    println!("attempts          {}", out.attempts);
    println!(
        "encounters        {}  ({:.2}/s measured)",
        out.encounters,
        out.encounters_per_second()
    );
    if out.stalls > 0 {
        println!(
            "stalls            {}  (attempts that burned their frame budget with \
             no encounter — check the base state is standing in grass)",
            out.stalls
        );
    }
    println!(
        "emulated          {} frames, {:.0}x realtime across {} instances",
        out.frames,
        out.speed_multiple(),
        out.per_instance.len()
    );
    println!("per instance      {:?}", out.per_instance);
    println!("elapsed           {}", human_time(elapsed));
    for e in &out.errors {
        println!("error             {e}");
    }
    match &out.hit {
        None => {
            println!("\nNo hit.");
            // The number that answers "leave it running overnight?".
            if out.encounters_per_second() > 0.0 {
                let want = cfg.target.expected_encounters();
                let median = cfg.target.median_encounters();
                let rate = out.encounters_per_second();
                println!(
                    "At the measured {rate:.2} encounters/s this target needs a mean of \
                     {want:.0} encounters ({}) and a median of {median:.0} ({}).",
                    human_time(Duration::from_secs_f64(want / rate)),
                    human_time(Duration::from_secs_f64(median / rate)),
                );
            }
        }
        Some(h) => {
            let s = h.sighting;
            println!("\n=== HIT ===");
            println!(
                "{} (internal #{}, dex #{}) level {}",
                tables.species_name(s.species),
                s.species,
                tables.dex_number(s.species),
                s.level
            );
            println!("  {}", s.dv_line());
            println!(
                "  shiny if traded to Gen 2: {}   hidden power {} {}",
                if s.shiny_if_traded() { "YES" } else { "no" },
                s.hidden_power().type_name,
                s.hidden_power().power
            );
            println!(
                "  found by instance {} on attempt {}, encounter {} of that run \
                 (seed {:#018x})",
                h.instance, h.attempt, h.encounter_in_run, h.seed
            );
            println!(
                "  map ${:02X} at ({}, {}), after {}",
                h.location.0,
                h.location.1,
                h.location.2,
                human_time(h.elapsed)
            );
            println!(
                "  replayed from (base state, seed, attempt) in a fresh machine: {}",
                if h.reproduced {
                    "SAME POKÉMON"
                } else {
                    "DID NOT REPRODUCE — treat this result with suspicion"
                }
            );
        }
    }
}

/// Write everything needed to reload the encounter and catch it by hand.
fn preserve(
    hit: &hunt::Hit,
    dir: &Path,
    cfg: &HuntConfig,
    tables: &RomTables,
    rom_path: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let state = dir.join("hit.gbss");
    std::fs::write(&state, &hit.state)
        .map_err(|e| format!("{}: {e}", state.display()))?;

    let s = hit.sighting;
    // **The preamble has to match the target.** A hunt for `spc=10` stops on a
    // Pokémon that is emphatically not shiny-if-traded, and a report that opens
    // by explaining the Time Capsule rule would be claiming something the run
    // never checked. Say what was actually asked for.
    let preamble = if s.shiny_if_traded() {
        "Gen 1 has no shiny Pokémon. This one carries the DV spread that becomes\n\
         shiny when it is traded forward to Gen 2 (CheckShininess,\n\
         pokecrystal/engine/gfx/color.asm), which is what the hunt was for."
            .to_string()
    } else {
        format!(
            "This Pokémon matches the target the hunt was given ({}). It is NOT\n\
             the shiny-if-traded spread — Gen 1 has no shiny Pokémon at all, and\n\
             this one would not become one in the Time Capsule either.",
            cfg.target.label()
        )
    };
    let report = format!(
        "Gen 1 shiny hunt — HIT\n\
         =====================\n\
         \n\
         {preamble}\n\
         \n\
         species          {species} (internal #{internal}, dex #{dex})\n\
         level            {level}\n\
         DVs              {dvs}\n\
         shiny if traded  {shiny}\n\
         hidden power     {hp_type} {hp_power}\n\
         \n\
         target           {target}\n\
         odds             1 in {odds:.0} encounters\n\
         \n\
         instance         {instance}\n\
         attempt          {attempt}\n\
         instance seed    {seed:#018x}\n\
         root seed        {root:#018x}\n\
         shape            {shape}\n\
         location         map ${map:02X}, x {x}, y {y}\n\
         elapsed          {elapsed}\n\
         replayed         {replayed}\n\
         \n\
         To catch it: load hit.gbss into the emulator with this cartridge\n\
         ({rom}) — the encounter is on screen and the Ball is unspent.\n\
         \n\
         To reproduce it from scratch, without hit.gbss — this re-runs the one\n\
         attempt that found it, in a fresh machine, and prints what it rolled:\n\
         \n\
         \x20 shiny_hunt --rom {rom} --state <the base state> \\\n\
         \x20            --seed {root:#018x} --instances {instances} \\\n\
         \x20            --shape {shape_flag} --run-length {run_length} \\\n\
         \x20            --replay {instance}:{attempt}\n\
         \n\
         and it is encounter #{in_run} of that run.\n",
        species = tables.species_name(s.species),
        internal = s.species,
        dex = tables.dex_number(s.species),
        level = s.level,
        dvs = s.dv_line(),
        shiny = if s.shiny_if_traded() { "YES" } else { "no" },
        hp_type = s.hidden_power().type_name,
        hp_power = s.hidden_power().power,
        target = cfg.target.label(),
        odds = cfg.target.expected_encounters(),
        instance = hit.instance,
        attempt = hit.attempt,
        seed = hit.seed,
        root = cfg.seed,
        shape = cfg.shape.label(),
        map = hit.location.0,
        x = hit.location.1,
        y = hit.location.2,
        elapsed = human_time(hit.elapsed),
        replayed = if hit.reproduced {
            "yes — a fresh machine given the same three numbers produced the same Pokémon"
        } else {
            "NO — treat this result with suspicion"
        },
        rom = rom_path.display(),
        instances = cfg.instances,
        shape_flag = match cfg.shape {
            Shape::Grass => "grass",
            Shape::Reset => "reset",
        },
        run_length = cfg.run_length,
        in_run = hit.encounter_in_run,
    );
    let txt = dir.join("hit.txt");
    std::fs::write(&txt, report).map_err(|e| format!("{}: {e}", txt.display()))?;

    // The picture, as a Netpbm P6 — no image codec, no extra cargo feature, and
    // the same format `tools/conformance` already reads.
    if let Ok(mut gb) = Gameboy::try_new_with_model(
        std::fs::read(rom_path).unwrap_or_default(),
        None,
        terminalgb::mode::Model::Auto,
    ) {
        if gb.load_state(&hit.state).is_ok() {
            let ppm = dir.join("hit.ppm");
            let mut buf = b"P6\n160 144\n255\n".to_vec();
            for px in gb.image().chunks_exact(4) {
                buf.extend_from_slice(&px[..3]);
            }
            let _ = std::fs::write(&ppm, buf);
        }
    }

    println!("\npreserved in {}", dir.display());
    println!("  hit.gbss   the save state at the moment of the encounter");
    println!("  hit.txt    the full report, including how to reproduce it");
    println!("  hit.ppm    the frame (convert with `magick hit.ppm hit.png`)");
    Ok(())
}

/// The divergence proof, and the control case that gives it teeth.
fn probe_mode(
    cfg: &HuntConfig,
    build: &(impl Fn(usize) -> Result<Gameboy, String> + Sync),
    attempts: u64,
    tables: &RomTables,
) -> Result<i32, String> {
    let started = Instant::now();
    let (seqs, health) = hunt::probe(cfg, build, attempts)?;
    let diverged = hunt::sequences_diverge(&seqs);
    for (i, seq) in seqs.iter().enumerate() {
        println!("instance {i}  seed {:#018x}", cfg.seed_for(i));
        for (n, run) in seq.iter().enumerate() {
            if run.is_empty() {
                println!("  attempt {n}: (stalled — no encounter)");
                continue;
            }
            for (k, s) in run.iter().enumerate() {
                println!(
                    "  attempt {n}.{k:<2} {:<12} lv{:<3} {}",
                    tables.species_name(s.species),
                    s.level,
                    s.dv_line()
                );
            }
        }
    }
    // **Divergence between instances is necessary and NOT sufficient**, and
    // this is the line that says so. Instances can differ from one another
    // perfectly while the union of what the rig can produce is a handful of
    // spreads — measured at 968 of 65,536 once, which is why a 97,212-encounter
    // hunt for a 1-in-8192 target found nothing while reporting a healthy rate
    // the whole time. See docs/shiny-hunt.md §4.
    let cov = hunt::coverage(&seqs);
    println!("\nran in {}", human_time(started.elapsed()));
    println!(
        "runs:     {} attempts, mean run {:.1} of {} encounters  \
         (ended early: {} stalled, {} could not flee, {} blacked out)",
        health.runs,
        health.mean_run_length(),
        cfg.run_length,
        health.stalls,
        health.flee_failures,
        health.blackouts
    );
    println!(
        "coverage: {} encounters, {} distinct DV spreads -> ~{} of 65,536 reachable",
        cov.samples,
        cov.distinct,
        if cov.reachable.is_finite() {
            format!("{:.0}", cov.reachable)
        } else {
            format!(">{}", cov.distinct)
        }
    );
    println!(
        "          expected shiny-if-traded spreads inside a set that size: {:.2}  — {}",
        cov.expected_shiny_spreads(),
        cov.verdict()
    );
    if cfg.decorrelate {
        println!(
            "instances diverge: {}",
            if diverged {
                "YES — every instance produced a different sequence"
            } else {
                "NO — two instances produced the same sequence, which means they \
                 are one instance wearing two hats"
            }
        );
        Ok(if diverged { 0 } else { 1 })
    } else {
        // The control: identical seeds must give identical sequences, or the
        // divergence above proves nothing — it could have been non-determinism.
        let identical = seqs.iter().all(|s| s == &seqs[0]);
        println!(
            "control (decorrelation off) — every instance identical: {}",
            if identical {
                "YES, as it must be: emulation is deterministic, so the same \
                 schedule gives the same Pokémon"
            } else {
                "NO — the emulator is not deterministic, which would make every \
                 divergence claim meaningless"
            }
        );
        Ok(if identical { 0 } else { 1 })
    }
}

/// Walk one attempt with a running trace of the bytes the plugin watches.
///
/// This exists because "every attempt stalled" has several very different
/// causes — the base state is not standing in grass, the walk is turning on the
/// spot instead of stepping, a text box is eating every direction key, or the
/// encounter *is* happening and the latch is not firing — and they are
/// indistinguishable from the outside. It prints enough to tell them apart.
fn diagnose(
    cfg: &HuntConfig,
    build: &(impl Fn(usize) -> Result<Gameboy, String> + Sync),
    tables: &RomTables,
) -> Result<i32, String> {
    let mut gb = build(0)?;
    gb.load_state(&cfg.base_state).map_err(|e| e.to_string())?;
    gb.frame();
    println!(
        "start: map ${:02X} at ({}, {}), wIsInBattle {}",
        gb.peek(CUR_MAP),
        gb.peek(Y_COORD),
        gb.peek(X_COORD),
        gb.peek(0xD057)
    );
    println!(
        "{:>6}  {:>4} {:>4}  {:>3}  {:>4}  {:>4}  {:>4}  note",
        "frame", "y", "x", "bat", "opp", "spec", "lvl"
    );
    let mut frames = 0u32;
    let mut going_up = true;
    let start_y = gb.peek(Y_COORD);
    let mut steps = 0u32;
    let mut met = 0u32;
    while frames < cfg.max_frames_per_attempt {
        let y = gb.peek(Y_COORD);
        if going_up && y <= start_y.saturating_sub(cfg.grass_band) {
            going_up = false;
        } else if !going_up && y >= start_y.saturating_add(cfg.grass_band) {
            going_up = true;
        }
        let key = if going_up {
            KeypadKey::Up
        } else {
            KeypadKey::Down
        };
        gb.keydown(key);
        for _ in 0..16 {
            gb.frame();
            frames += 1;
        }
        gb.keyup(key);
        for _ in 0..4 {
            gb.frame();
            frames += 1;
        }
        steps += 1;
        let battle = gb.peek(0xD057);
        let sighting = <Gameboy as hunt::HuntMachine>::sighting(&gb);
        println!(
            "{frames:>6}  {:>4} {:>4}  {battle:>3}  {:>4}  {:>4}  {:>4}  {}",
            gb.peek(Y_COORD),
            gb.peek(X_COORD),
            gb.peek(0xD059),
            gb.peek(0xCFE5),
            gb.peek(0xCFF3),
            match &sighting {
                Some(s) =>
                    format!("LATCHED {} {}", tables.species_name(s.species), s.dv_line()),
                None if battle != 0 => "in a battle, nothing latched yet".to_string(),
                None => String::new(),
            }
        );
        if battle != 0 {
            // Stop walking and let `LoadEnemyMonData` finish, exactly as the
            // driver does, then run away and keep going — the run of encounters
            // is where a wild hunt gets its DV coverage, so the thing worth
            // tracing is what the *fifth* encounter of a run looks like.
            for _ in 0..240 {
                gb.frame();
                frames += 1;
                if <Gameboy as hunt::HuntMachine>::sighting(&gb).is_some() {
                    break;
                }
            }
            match <Gameboy as hunt::HuntMachine>::sighting(&gb) {
                Some(s) => {
                    met += 1;
                    println!(
                        "  -> encounter {met}: {} lv{} {}",
                        tables.species_name(s.species),
                        s.level,
                        s.dv_line()
                    );
                }
                None => {
                    println!("  -> STILL NOTHING LATCHED after settling");
                    return Ok(1);
                }
            }
            if met >= 8 {
                println!("\nmet {met} Pokémon in one run — the run loop is working");
                return Ok(0);
            }
            // Run away, exactly as the driver does.
            let before = (gb.peek(Y_COORD), gb.peek(X_COORD));
            let mut escaped = false;
            for _ in 0..60 {
                for key in [KeypadKey::Right, KeypadKey::Down, KeypadKey::A] {
                    tap(&mut gb, key, 14);
                }
                if gb.peek(0xD057) == 0 {
                    escaped = true;
                    break;
                }
                if gb.peek(0xD057) == 0xFF {
                    println!("  -> BLACKED OUT — the party cannot survive a run");
                    return Ok(1);
                }
            }
            if !escaped {
                println!("  -> COULD NOT FLEE");
                return Ok(1);
            }
            for _ in 0..6 {
                tap(&mut gb, KeypadKey::B, 30);
            }
            <Gameboy as hunt::HuntMachine>::arm(&mut gb);
            println!(
                "  -> fled: was at {:?}, now at ({}, {}), map ${:02X}",
                before,
                gb.peek(Y_COORD),
                gb.peek(X_COORD),
                gb.peek(CUR_MAP)
            );
            frames = 0;
        }
    }
    println!("\nno encounter in {frames} frames / {steps} steps after meeting {met}");
    println!(
        "if y never changed, the walk is turning on the spot or a text box is \
         eating the keys; if it changed but no battle started, the player is not \
         standing in grass any more"
    );
    Ok(1)
}

fn human_time(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 90.0 {
        format!("{s:.1}s")
    } else if s < 5400.0 {
        format!("{:.1}m", s / 60.0)
    } else if s < 172_800.0 {
        format!("{:.1}h", s / 3600.0)
    } else {
        format!("{:.1} days", s / 86_400.0)
    }
}

// ---------------------------------------------------------------------------
// Building a starting position
// ---------------------------------------------------------------------------

/// `wCurMap`, `wYCoord`, `wXCoord` — `pokered/ram/wram.asm`.
const CUR_MAP: u16 = 0xD35E;
const Y_COORD: u16 = 0xD361;
const X_COORD: u16 = 0xD362;
/// `wCurMapHeight` / `wCurMapWidth`. Zero until `LoadMapHeader` has run, and no
/// real map is 0x0 — so this, not `wCurMap`, is the "am I in control?" test.
const CUR_MAP_HEIGHT: u16 = 0xD368;
const CUR_MAP_WIDTH: u16 = 0xD369;
/// `wPartyCount` / `wPartySpecies` / `wPartyMons` / `wPartyMonOT` /
/// `wPartyMonNicks`, and `wPlayerID` (big-endian).
const PARTY_COUNT: u16 = 0xD163;
const PARTY_SPECIES: u16 = 0xD164;
const PARTY_MONS: u16 = 0xD16B;
const PARTY_MON_OT: u16 = 0xD273;
const PARTY_MON_NICKS: u16 = 0xD2B5;
const PLAYER_ID: u16 = 0xD359;
/// `wEventFlags` — bit 0 of byte 0 is `EVENT_FOLLOWED_OAK_INTO_LAB`, the first
/// `const` after `const_def` in `constants/event_constants.asm`.
/// `PalletTownDefaultScript` (`scripts/PalletTown.asm`) opens
/// `CheckEvent EVENT_FOLLOWED_OAK_INTO_LAB / ret nz`, so setting it is what
/// stops Oak running out to block the north exit of a brand-new game.
const EVENT_FLAGS: u16 = 0xD747;
/// `map_const ROUTE_1, 10, 18 ; $0C` — the first map with a grass encounter
/// table (`data/wild/maps/Route1.asm`, 25/256 per step).
const ROUTE_1: u8 = 0x0C;
const PALLET_TOWN: u8 = 0x00;

/// Build a base save state: a **new game** walked to the grass at the southern
/// end of Route 1.
///
/// This is a convenience, not the feature — a hunt takes whatever save state you
/// give it, and the interesting hunts start from positions only a player can
/// reach. It exists so the whole thing can be demonstrated from a bare
/// cartridge, and because "you need a save state, good luck" is a bad first
/// experience.
///
/// Two things it does *not* do, both for reasons that cost other people days:
///
/// * **It does not edit `wCurMap` to teleport.** `LoadSAV` sets
///   `BIT_NO_PREVIOUS_MAP` and `LoadMapHeader` then returns without loading the
///   header, so a moved map is read through the old map's data pointers and the
///   game jumps into `rst $38` within a second. The player walks; the game loads
///   every header itself.
/// * **It does not write a `.sav`.** The machine is built with no file path, so
///   nothing next to the cartridge is touched.
fn make_state(rom: &[u8], dest: &Path) -> Result<(), String> {
    let mut gb =
        Gameboy::try_new_with_model(rom.to_vec(), None, terminalgb::mode::Model::Auto)
            .map_err(|e| e.to_string())?;
    if terminalgb_plugin_trainer::Game::detect(&gb.rom_title(), gb.global_checksum())
        .is_none()
    {
        return Err(format!(
            "`{}` is not a verified Pokémon Red/Blue cartridge",
            gb.rom_title().trim()
        ));
    }

    // Through the title screen, Oak's speech and the naming screens. Alternating
    // START and A is what gets past both: the naming screen needs START (mashing
    // A types `AAAAA` for ever), and everything else wants A.
    print!("new game: ");
    let mut in_game = false;
    for step in 0..600u32 {
        let key = if step % 2 == 0 {
            KeypadKey::Start
        } else {
            KeypadKey::A
        };
        tap(&mut gb, key, 20);
        if gb.peek(CUR_MAP_HEIGHT) > 0 && gb.peek(CUR_MAP_WIDTH) > 0 {
            in_game = true;
            break;
        }
    }
    if !in_game {
        return Err("never reached the overworld".into());
    }
    println!(
        "in the overworld, map ${:02X} at ({}, {})",
        gb.peek(CUR_MAP),
        gb.peek(Y_COORD),
        gb.peek(X_COORD)
    );

    // A brand-new game has no Pokémon, and walking into grass without one hangs
    // the battle. Give it one, and give it the running game's own trainer id and
    // name — `CheckForDisobedience` treats an OT id that differs from
    // `wPlayerID` as a *traded* Pokémon, which a badgeless player is only obeyed
    // by up to level 10. (A hunt never fights, but a state a player might pick up
    // and use should not carry a booby trap.)
    let player_id = ((gb.peek(PLAYER_ID) as u16) << 8) | gb.peek(PLAYER_ID + 1) as u16;
    give_a_pokemon(&mut gb, player_id);
    // Skip Oak's "wait, don't go out" script at the north exit.
    let flags = gb.peek(EVENT_FLAGS);
    gb.debug_write(EVENT_FLAGS, flags | 1);

    // Bedroom -> stairs (`warp_event 7, 1`, data/maps/objects/RedsHouse2F.asm).
    walk_toward(&mut gb, 1, 7, 60);
    // Downstairs Mum has something to say before she will let anyone past.
    clear_text(&mut gb, 6);
    // Ground floor -> front door (`warp_event 2, 7` and `3, 7`,
    // data/maps/objects/RedsHouse1F.asm). Both sit on row 7, the bottom edge of
    // the map — and a Gen 1 warp on a map *edge* fires when the player walks
    // into the edge, not when they step onto the tile. So arriving at the door
    // is not leaving through it: press Down again once standing there.
    walk_toward(&mut gb, 7, 2, 120);
    for _ in 0..6 {
        if gb.peek(CUR_MAP) != 0x25 {
            break;
        }
        hold(&mut gb, KeypadKey::Down, 24);
    }
    if gb.peek(CUR_MAP) != PALLET_TOWN {
        return Err(format!(
            "never got out of the house (map ${:02X} at ({}, {}))",
            gb.peek(CUR_MAP),
            gb.peek(Y_COORD),
            gb.peek(X_COORD)
        ));
    }
    // The player is standing *on* the doorstep, which is the warp back inside
    // (`warp_event 5, 5`) — step off it before heading anywhere.
    for _ in 0..2 {
        hold(&mut gb, KeypadKey::Down, 24);
    }
    // Pallet Town's only northern opening is block x = 5 of row 0 in
    // `maps/PalletTown.blk` (`0B` among fences), i.e. tiles x = 10..11. Go east
    // along the bottom path first: north straight off the doorstep walks back
    // through the door.
    walk_toward(&mut gb, 7, 10, 80);
    walk_toward(&mut gb, 1, 10, 160);
    for _ in 0..12 {
        if gb.peek(CUR_MAP) == ROUTE_1 {
            break;
        }
        hold(&mut gb, KeypadKey::Up, 24);
    }
    if gb.peek(CUR_MAP) != ROUTE_1 {
        return Err(format!(
            "never reached ROUTE_1 (map ${:02X} at ({}, {}))",
            gb.peek(CUR_MAP),
            gb.peek(Y_COORD),
            gb.peek(X_COORD)
        ));
    }
    // The grass at the southern end of Route 1 is blocks `0B` in rows 16-17 of
    // `maps/Route1.blk` at block x = 5 — tiles x = 10..11, y = 32..35, which is
    // where the connection from Pallet Town drops the player. Park in the middle
    // of it so the hunt's ±1-tile pacing never leaves the grass.
    const GRASS: (u8, u8) = (33, 10);
    // **The last step into grass is quite likely to be the step that rolls an
    // encounter**, and a base state saved mid-battle makes every later attempt
    // stall with no explanation. So: walk, and if a battle starts, run from it
    // and walk again — until the player is standing on the grass tile with
    // `wIsInBattle` clear. That is the state worth saving.
    let mut settled = false;
    let verbose = std::env::var("GB_HUNT_VERBOSE").is_ok();
    for round in 0..40 {
        let (y, x) = (gb.peek(Y_COORD), gb.peek(X_COORD));
        let battle = gb.peek(0xD057);
        if verbose {
            eprintln!("  settle round {round}: at ({y}, {x}) battle {battle}");
        }
        if battle != 0 {
            if !flee(&mut gb) {
                return Err("could not get out of a wild battle while positioning".into());
            }
            continue;
        }
        if (y, x) == GRASS {
            settled = true;
            break;
        }
        if x != GRASS.1 {
            walk_toward(&mut gb, GRASS.0, GRASS.1, 40);
            continue;
        }
        // The right column, wrong row. **A 24-frame hold is one and a half Gen 1
        // steps**, and the game commits to a step once it has begun, so a
        // navigation press moves the player *two* tiles as often as one — which
        // is why `walk_toward` alone oscillated 34 → 32 → 34 for ever and never
        // landed on 33. Close the last tile with a press short enough to buy
        // exactly one step.
        step(
            &mut gb,
            if y > GRASS.0 {
                KeypadKey::Up
            } else {
                KeypadKey::Down
            },
        );
    }
    if !settled {
        return Err(format!(
            "could not settle on the grass tile ({}, {}) — ended at ({}, {}), \
             wIsInBattle {}",
            GRASS.0,
            GRASS.1,
            gb.peek(Y_COORD),
            gb.peek(X_COORD),
            gb.peek(0xD057)
        ));
    }
    println!(
        "walked to ROUTE_1, standing in grass at ({}, {}), out of battle",
        gb.peek(Y_COORD),
        gb.peek(X_COORD)
    );

    let state = gb.save_state();
    std::fs::write(dest, &state).map_err(|e| format!("{}: {e}", dest.display()))?;
    println!("wrote {} ({} bytes)", dest.display(), state.len());
    println!(
        "\nCheck it before trusting it:\n  \
         shiny_hunt --rom <rom> --state {} --probe 2",
        dest.display()
    );
    Ok(())
}

/// A single Pokémon written straight into WRAM.
///
/// The party is WRAM, not cartridge RAM, so this is immediate and touches no
/// file. The 44-byte `party_struct` layout is `pokered/ram/wram.asm`
/// (`wPartyMon1Species` onwards); every multi-byte field is **big-endian**.
fn give_a_pokemon(gb: &mut Gameboy, player_id: u16) {
    const PIDGEOT: u8 = 0x24 + 2; // internal index of PIDGEOT; dex #18
    const LEVEL: u8 = 50;
    // **Fast and tough on purpose, and it is not a cheat — it is what makes the
    // hunt work.** A wild-grass run meets dozens of Pokémon between reloads and
    // flees from every one of them (see docs/shiny-hunt.md §4). Gen 1's escape
    // chance rises with the player's Speed, and a party that faints blacks out
    // to a Pokémon Center where there is no grass, ending the run. A slow,
    // fragile starter turns a run of 32 into a run of 3 — measured.
    let max_hp: u16 = 250;

    gb.debug_write(PARTY_COUNT, 1);
    gb.debug_write(PARTY_SPECIES, PIDGEOT);
    gb.debug_write(PARTY_SPECIES + 1, 0xFF); // list terminator

    let mut b = [0u8; 44];
    b[0] = PIDGEOT;
    b[1..3].copy_from_slice(&max_hp.to_be_bytes()); // current HP
    b[3] = LEVEL; // box level
    b[4] = 0; // status
    b[5] = 0x00; // NORMAL
    b[6] = 0x02; // FLYING
    b[7] = 255; // catch rate
    b[8] = 0x10; // GUST
    b[12..14].copy_from_slice(&player_id.to_be_bytes());
    // Experience is three bytes, big-endian. Pidgeot is medium-fast, so
    // `level^3` is exactly the threshold for its level and the game will not
    // level it up on sight.
    let exp = u32::from(LEVEL).pow(3);
    b[14] = (exp >> 16) as u8;
    b[15] = (exp >> 8) as u8;
    b[16] = exp as u8;
    b[27] = 0x9A; // DVs — arbitrary, and deliberately not the shiny pair
    b[28] = 0x88;
    b[29] = 35; // PP of the one move
    b[33] = LEVEL;
    b[34..36].copy_from_slice(&max_hp.to_be_bytes());
    b[36..38].copy_from_slice(&120u16.to_be_bytes()); // attack
    b[38..40].copy_from_slice(&120u16.to_be_bytes()); // defense
    b[40..42].copy_from_slice(&250u16.to_be_bytes()); // speed — the escape stat
    b[42..44].copy_from_slice(&120u16.to_be_bytes()); // special
    for (i, v) in b.iter().enumerate() {
        gb.debug_write(PARTY_MONS + i as u16, *v);
    }

    // Names are 11 bytes, Gen 1 charmap ('A' = $80), terminated by $50.
    let name = |s: &str| -> [u8; 11] {
        let mut out = [0x50u8; 11];
        for (i, c) in s.bytes().take(10).enumerate() {
            out[i] = 0x80 + (c - b'A');
        }
        out
    };
    for (i, v) in name("SCOUT").iter().enumerate() {
        gb.debug_write(PARTY_MON_NICKS + i as u16, *v);
        gb.debug_write(PARTY_MON_OT + i as u16, *v);
    }
}

/// Hold a direction long enough for the player to turn *and* take a step. A Gen
/// 1 step is 16 frames, and a press that only turns the character moves nothing.
fn hold(gb: &mut Gameboy, key: KeypadKey, frames: u32) {
    gb.keydown(key);
    for _ in 0..frames {
        gb.frame();
    }
    gb.keyup(key);
    for _ in 0..4 {
        gb.frame();
    }
}

/// Exactly one tile. A Gen 1 step takes 16 frames and the game commits to it as
/// soon as it starts, so a short hold buys one step and one only — where
/// [`hold`]'s 24 frames buys one *or* two depending on where the walk began.
fn step(gb: &mut Gameboy, key: KeypadKey) {
    gb.keydown(key);
    for _ in 0..10 {
        gb.frame();
    }
    gb.keyup(key);
    for _ in 0..12 {
        gb.frame();
    }
}

/// Run from a wild battle. `RUN` is the bottom-right entry of the 2x2 battle
/// menu (`FIGHT ITEM` over `PKMN RUN`), so from the default `FIGHT` it is Right
/// then Down then A. Running can fail, so this keeps trying.
///
/// Afterwards "Got away safely!" is still on screen with `wIsInBattle` already
/// clear, and **a text box swallows every direction key** — press through it, or
/// the walk that follows moves nothing.
fn flee(gb: &mut Gameboy) -> bool {
    let mut out = false;
    for _ in 0..60 {
        for key in [KeypadKey::Right, KeypadKey::Down, KeypadKey::A] {
            tap(gb, key, 14);
        }
        if gb.peek(0xD057) == 0 {
            out = true;
            break;
        }
    }
    clear_text(gb, 6);
    for _ in 0..60 {
        gb.frame();
    }
    out
}

/// Press B a few times to dismiss whatever text box is up. Harmless when there
/// is none — B in the overworld does nothing at all.
fn clear_text(gb: &mut Gameboy, times: u32) {
    for _ in 0..times {
        tap(gb, KeypadKey::B, 30);
    }
}

fn tap(gb: &mut Gameboy, key: KeypadKey, gap: u32) {
    gb.keydown(key);
    for _ in 0..4 {
        gb.frame();
    }
    gb.keyup(key);
    for _ in 0..gap {
        gb.frame();
    }
}

/// Greedily walk toward tile `(y, x)` on the current map. Returns when it
/// arrives, when the map changes, when a battle starts, or when the budget runs
/// out.
fn walk_toward(gb: &mut Gameboy, ty: u8, tx: u8, budget: u32) -> u8 {
    let map0 = gb.peek(CUR_MAP);
    let mut stuck = 0u32;
    for _ in 0..budget {
        let map = gb.peek(CUR_MAP);
        let (y, x) = (gb.peek(Y_COORD), gb.peek(X_COORD));
        if map != map0 || gb.peek(0xD057) != 0 || (y == ty && x == tx) {
            // Let a warp finish: stepping onto a door tile starts a map change
            // that takes a fade and several frames.
            for _ in 0..48 {
                gb.frame();
            }
            return gb.peek(CUR_MAP);
        }
        let dy = ty as i16 - y as i16;
        let dx = tx as i16 - x as i16;
        let vert = if dy > 0 {
            KeypadKey::Down
        } else {
            KeypadKey::Up
        };
        let horiz = if dx > 0 {
            KeypadKey::Right
        } else {
            KeypadKey::Left
        };
        // Try the axis with the larger error first, then the other, then probe
        // sideways — a wall in a Gen 1 house is walked around, not into.
        let order = if dy.abs() >= dx.abs() {
            [vert, horiz, KeypadKey::Left, KeypadKey::Right, vert]
        } else {
            [horiz, vert, KeypadKey::Up, KeypadKey::Down, horiz]
        };
        // Every few failures, assume it is a text box rather than a wall. A new
        // game has several the harness never asked for — Mum's send-off
        // downstairs is the first — and a text box swallows every direction
        // key, so a walk that meets one looks exactly like a walk into a wall
        // and never recovers. B, not A: both advance a box, but B can only ever
        // *close* a menu, and an A one frame after the box closes opens the
        // START menu, which then eats every direction key for the rest of the
        // run.
        if stuck > 0 && stuck % order.len() as u32 == 0 {
            clear_text(gb, 3);
        }
        hold(gb, order[stuck as usize % order.len()], 24);
        stuck = if (gb.peek(Y_COORD), gb.peek(X_COORD)) == (y, x) {
            stuck + 1
        } else {
            0
        };
    }
    gb.peek(CUR_MAP)
}
