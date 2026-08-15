//! Render the trainer panel to stdout at a range of widths, with no terminal,
//! no ROM and no emulator, so a layout change can be *looked at* rather than
//! reasoned about.
//!
//! ```text
//! cargo run --release --example trainer_layout
//! cargo run --release --example trainer_layout -- 40 34 28 22
//! ```
//!
//! The state is a hand-built party rather than a real save because the point is
//! the *layout*: it deliberately holds the awkward cases — a nickname longer
//! than the column, a dual type, a fainted Pokémon, a status, a three-digit HP,
//! level 100 — which a real save shows one at a time if ever. What it cannot
//! check is the sprite, which is an image and needs a real terminal; see
//! `docs/trainer.md`.

use terminalgb::screen::trainer_view as tv;
use terminalgb_plugin_trainer::derived::StatExp;
use terminalgb_plugin_trainer::gen1::{BattleKind, Dvs, Mon, MoveSlot, Status};
use terminalgb_plugin_trainer::{TeamMon, TrainerState};

fn mon(nick: &str, name: &str, level: u8, hp: u16, max: u16, ty: (&str, &str)) -> Mon {
    Mon {
        species: 0x99,
        dex: 25,
        name: name.into(),
        nickname: nick.into(),
        level,
        hp,
        max_hp: max,
        status: Status(0),
        types: (ty.0.into(), ty.1.into()),
        attack: 42,
        defense: 38,
        speed: 61,
        special: 50,
        moves: vec![
            MoveSlot {
                id: 85,
                name: "THUNDERBOLT".into(),
                pp: 15,
                pp_max: 15,
                pp_ups: 0,
                power: 95,
                type_name: "ELECTRIC".into(),
                accuracy: 100,
            },
            MoveSlot {
                id: 98,
                name: "QUICK ATTACK".into(),
                pp: 0,
                pp_max: 30,
                pp_ups: 0,
                power: 40,
                type_name: "NORMAL".into(),
                accuracy: 100,
            },
            // A status move: zero power is a fact, and the table has to say so
            // without printing a `0` that invites comparison with a 40.
            MoveSlot {
                id: 45,
                name: "GROWL".into(),
                pp: 40,
                pp_max: 40,
                pp_ups: 0,
                power: 0,
                type_name: "NORMAL".into(),
                accuracy: 100,
            },
            // …and one that can miss, so the ACC column is not all 100s.
            MoveSlot {
                id: 56,
                name: "HYDRO PUMP".into(),
                pp: 2,
                pp_max: 5,
                pp_ups: 0,
                power: 120,
                type_name: "WATER".into(),
                accuracy: 80,
            },
        ],
        dvs: Dvs::from_bytes(0xAB, 0xCD),
        stat_mods: None,
        stat_exp: Some(StatExp {
            hp: 1200,
            attack: 900,
            defense: 400,
            speed: 2500,
            special: 100,
        }),
    }
}

fn state(battle: bool) -> TrainerState {
    let mut st = TrainerState {
        in_game: true,
        player_name: "BLUE".into(),
        player_id: 62800,
        money: 3000,
        play_time: (0, 1, 41),
        badges: 0b0000_0011,
        ..Default::default()
    };
    st.party.push(mon(
        "SPARK",
        "PIKACHU",
        12,
        34,
        34,
        ("ELECTRIC", "ELECTRIC"),
    ));
    let mut poisoned = mon("RATTY", "RATTATA", 9, 11, 27, ("NORMAL", "NORMAL"));
    poisoned.status = Status(1 << 3);
    st.party.push(poisoned);
    st.party.push(mon(
        "LONGNICKNAME",
        "BULBASAUR",
        100,
        0,
        999,
        ("GRASS", "POISON"),
    ));
    st.party
        .push(mon("ZUB", "ZUBAT", 7, 3, 22, ("POISON", "FLYING")));
    let mut asleep = mon("GHOSTY", "GASTLY", 24, 40, 55, ("GHOST", "POISON"));
    asleep.status = Status(3);
    st.party.push(asleep);
    st.party
        .push(mon("BIG", "GYARADOS", 41, 122, 130, ("WATER", "FLYING")));
    if battle {
        st.battle = BattleKind::Trainer;
        st.trainer_name = "JOEY".into();
        let mut o = mon("", "CHARMANDER", 7, 18, 20, ("FIRE", "FIRE"));
        o.moves.truncate(2);
        o.stat_mods = Some(terminalgb_plugin_trainer::derived::StatMods::from_bytes([
            6, 7, 9, 7, 5, 7,
        ]));
        st.opponent = Some(o);
        let mut active = st.party[0].clone();
        active.stat_mods =
            Some(terminalgb_plugin_trainer::derived::StatMods::from_bytes([
                7, 5, 7, 7, 7, 7,
            ]));
        st.active = Some(active);
        st.enemy_team = vec![
            TeamMon {
                species: 0xB0,
                name: "GEODUDE".into(),
                level: 12,
                hp: 0,
                max_hp: 34,
                known: true,
            },
            TeamMon {
                species: 0xB1,
                name: "ONIX".into(),
                level: 14,
                hp: 40,
                max_hp: 40,
                known: true,
            },
            TeamMon::default(),
        ];
    }
    st
}

fn draw(
    title: &str,
    st: &TrainerState,
    focus: Option<tv::Focus>,
    detail: bool,
    w: usize,
) {
    let inner = w.saturating_sub(2);
    // Compose exactly as the frontend does, with pictures asked for and the
    // rows a generous terminal would give, so the printed shape is the shape.
    let body = tv::compose_body(st, focus, detail, inner, tv::Pictures::Image, 60);
    println!("\n{title} — {w} columns");
    println!("┌{}┐", "─".repeat(inner));
    for l in tv::plain(&tv::header_lines(st, "Pokémon Blue", inner)) {
        println!("│{l:<inner$}│");
    }
    // The gutters are terminal images, which this tool cannot draw — so it
    // marks the cells they occupy instead. That is the point of printing them:
    // a gutter cell that does not line up with the text beside it is a bug
    // this view makes obvious and a unit test would state in numbers.
    let g = body.gutter as usize;
    let mark: Vec<usize> = {
        let mut rows = Vec::new();
        if let Some((r, n)) = body.focus_gutter {
            rows.extend(r as usize..(r + n) as usize);
        }
        if let Some((r, n)) = body.party_gutter {
            rows.extend(r as usize..(r + n * tv::SLOT_ROWS) as usize);
        }
        rows
    };
    for (i, l) in tv::plain(&body.lines).into_iter().enumerate() {
        let l = if g > 0 && mark.contains(&i) {
            // `▚` stands in for the picture; the text is untouched.
            format!("{}{}", "▚".repeat(g), &l[l.len().min(g)..])
        } else {
            l
        };
        println!("│{l:<inner$}│");
    }
    println!("└{}┘", "─".repeat(inner));
}

/// Drive a real cartridge instead of the fixture: boot it, optionally load a
/// save state, run frames, and print the panel from **live** state.
///
/// This is the honest check — the fixture proves the layout, this proves the
/// layout against numbers the game actually wrote, including the opposing
/// trainer's roster, whose addresses are only a claim until something reads
/// them out of a running battle.
///
/// ```text
/// cargo run --release --example trainer_layout -- \
///     --rom target/live/blue.gb --state slot1.gbss --frames 120 --width 40
/// ```
///
/// **Copy the ROM and its save into `target/` first** — the MBC writes a `.sav`
/// next to the ROM path.
fn live(rom: &str, state: Option<&str>, frames: u64, w: usize, keys: &str) {
    let path = std::path::PathBuf::from(rom);
    let data = std::fs::read(&path).expect("read the ROM");
    let mut gb = terminalgb::gameboy::Gameboy::new(data.clone(), Some(path));
    let mut plugin = terminalgb_plugin_trainer::TrainerPlugin::new(data);
    plugin.set_active(true);
    gb.add_plugin(Box::new(plugin))
        .expect("attach the trainer view");
    if let Some(s) = state {
        let bytes = std::fs::read(s).expect("read the save state");
        gb.load_state(&bytes).expect("load the save state");
    }
    // A scripted key is held for 8 frames and released for 8 — long enough for
    // the game to see it, short enough to walk.
    let script: Vec<char> = keys.chars().collect();
    for f in 0..frames {
        if !script.is_empty() {
            let i = (f / 16) as usize;
            if i < script.len() {
                if let Some(k) = key_for(script[i]) {
                    if f % 16 < 8 {
                        gb.keydown(k);
                    } else {
                        gb.keyup(k);
                    }
                }
            }
        }
        gb.frame();
    }
    // What the *game* is showing, decoded from `wTileMap` — the only reliable
    // way to know where a scripted run has actually got to (see
    // `examples/gen1_trade.rs`, which navigates the same way).
    if std::env::args().any(|a| a == "--screen") {
        let mut tiles = [0u8; 20 * 18];
        for (i, t) in tiles.iter_mut().enumerate() {
            *t = gb.peek(0xC3A0 + i as u16);
        }
        println!("{}", terminalgb_plugin_trainer::text::decode_screen(&tiles));
    }
    let Some(p) = gb.plugin::<terminalgb_plugin_trainer::TrainerPlugin>() else {
        println!("no trainer plugin");
        return;
    };
    match p.game() {
        None => println!("refused: {}", p.refusal().unwrap_or_default()),
        Some(g) => {
            let st = p.state();
            println!("in_game={} battle={:?}", st.in_game, st.battle);
            println!(
                "enemy_team={:?} unknown={}",
                st.enemy_team
                    .iter()
                    .map(|m| (m.name.clone(), m.level, m.hp, m.max_hp, m.known))
                    .collect::<Vec<_>>(),
                st.enemy_team_unknown
            );
            let focus = tv::focus_at(st, 0);
            let inner = w.saturating_sub(2);
            println!("┌{}┐", "─".repeat(inner));
            for l in tv::plain(&tv::header_lines(st, g.label(), inner)) {
                println!("│{l:<inner$}│");
            }
            for l in tv::plain(&tv::body_lines(st, focus, false, inner)) {
                println!("│{l:<inner$}│");
            }
            println!("└{}┘", "─".repeat(inner));
        }
    }
}

/// Play a **new** game from the title screen until the game is in a battle, and
/// dump the raw enemy block beside what the plugin made of it.
///
/// This is how the enemy-roster addresses in `trainer::gen1::addr` stop being a
/// claim: the disassembly says the enemy block mirrors the player's, and this
/// walks a real cartridge into a real battle and checks that the two
/// independent copies of each species — the list and the `party_struct` — agree
/// with what the battle engine put in `wEnemyMon`. Mashing A/Start plays Gen 1's
/// intro on its own; walking north out of Pallet Town then runs the Oak script,
/// which ends in the rival's trainer battle.
///
/// ```text
/// cargo run --release --example trainer_layout -- --rom target/live/blue.gb --to-battle
/// ```
fn to_battle(rom: &str, want_trainer: bool, states: Option<&str>, seed0: u64) {
    let path = std::path::PathBuf::from(rom);
    let data = std::fs::read(&path).expect("read the ROM");
    for ext in ["sav", "srm", "gbsave"] {
        let _ = std::fs::remove_file(path.with_extension(ext));
    }
    let mut gb = terminalgb::gameboy::Gameboy::new(data.clone(), Some(path));
    let mut plugin = terminalgb_plugin_trainer::TrainerPlugin::new(data);
    plugin.set_active(true);
    gb.add_plugin(Box::new(plugin)).expect("attach");

    let press =
        |gb: &mut terminalgb::gameboy::Gameboy, k: terminalgb::KeypadKey, gap: u32| {
            gb.keydown(k);
            for _ in 0..4 {
                gb.frame();
            }
            gb.keyup(k);
            for _ in 0..gap {
                gb.frame();
            }
        };
    use terminalgb::KeypadKey::*;
    // The intro plays itself if you keep confirming.
    for step in 0..400u32 {
        press(&mut gb, if step % 2 == 0 { Start } else { A }, 20);
        if gb.peek(0xDA41) != 0 || gb.peek(0xDA43) != 0 || gb.peek(0xDA44) != 0 {
            break;
        }
    }
    println!("overworld reached, play clock running");
    // The walk is a random walk, so a given seed either finds the doorways or
    // does not; `--seed` lets a run that wedges be retried rather than tuned.
    let mut seed: u64 = seed0;
    // North out of the bedroom and the town: Oak intercepts, the lab scene
    // runs, a starter is chosen and the rival challenges you.
    for step in 0..12000u32 {
        let kind = gb.peek(0xD057);
        if kind != 0 && (!want_trainer || kind >= 2) {
            println!("battle after {step} rounds, wIsInBattle={kind}");
            break;
        }
        // A biased random walk with A mixed in. Gen 1's opening is a corridor
        // of scripted events — leave the bedroom, leave the house, walk north
        // out of Pallet Town — and a walk that is biased north but not blind
        // finds each doorway without anyone hard-coding a route that a
        // different starting tile would break. `seed` is an LCG, so the run is
        // deterministic and reproducible.
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let r = (seed >> 33) % 10;
        // Inside the house the way out is *down* the stairs and out of the
        // door; outside, the scripted opening is *north*. Biasing by which map
        // the player is on is the difference between escaping the building and
        // bouncing between its two floors forever.
        // 2F: the stairs are at the top. 1F: the front door is at the bottom.
        // Outside: the opening script is north. Three maps, three biases —
        // without them the walk parks against whichever wall it started facing.
        let r = match (gb.peek(0xD35E), r) {
            (0x25, 0..=3) => 4, // Red's house 1F — head for the door
            (0x26, 4) => 0,     // Red's house 2F — head for the stairs
            // Oak's lab: the starter table is at the top, and taking a ball is
            // several confirmations deep, so press A far more than you walk.
            // …and never B, which answers NO to "do you want it?" and puts the
            // scene straight back to the start. That single substitution is the
            // difference between choosing a starter and looping forever.
            (0x28, 9) => 7,
            _ => r,
        };
        match r {
            0..=3 => press(&mut gb, Up, 16),
            4 => press(&mut gb, Down, 16),
            5 => press(&mut gb, Left, 16),
            6 => press(&mut gb, Right, 16),
            7 | 8 => press(&mut gb, A, 16),
            // B backs out of the menus the walk blunders into — without it the
            // run parks itself in the bedroom PC forever.
            _ => press(&mut gb, B, 16),
        }
        if step % 500 == 0 {
            println!(
                "  step {step}: map={:#04X} y={} x={} battle={}",
                gb.peek(0xD35E),
                gb.peek(0xD361),
                gb.peek(0xD362),
                gb.peek(0xD057)
            );
            let rows: Vec<String> = (0..18)
                .map(|y| {
                    let row: Vec<u8> =
                        (0..20).map(|x| gb.peek(0xC3A0 + y * 20 + x)).collect();
                    terminalgb_plugin_trainer::text::decode_screen(&row)
                })
                .collect();
            for r in rows.iter().filter(|r| !r.trim().is_empty()) {
                println!("      |{r}|");
            }
        }
    }
    let kind = gb.peek(0xD057);
    if kind == 0 {
        println!("never reached a battle");
        return;
    }
    // `wIsInBattle` goes non-zero before `LoadEnemyMonData` has filled
    // `wEnemyMon`, so settle first — which is also the transient the roster's
    // consistency check exists for.
    for _ in 0..40 {
        press(&mut gb, A, 16);
        if gb.peek(0xCFE5) != 0 {
            break;
        }
    }
    println!("wIsInBattle = {kind}");
    // The raw block, and the two copies of each species that must agree.
    let count = gb.peek(0xD89C);
    println!("wEnemyPartyCount = {count}");
    for i in 0..6u16 {
        let listed = gb.peek(0xD89D + i);
        let base = 0xD8A4 + i * 44;
        let stored = gb.peek(base);
        let level = gb.peek(base + 33);
        let hp = (gb.peek(base + 1) as u16) << 8 | gb.peek(base + 2) as u16;
        let max = (gb.peek(base + 34) as u16) << 8 | gb.peek(base + 35) as u16;
        println!(
            "  slot {i}: list={listed:#04X} struct={stored:#04X} \
             L{level} {hp}/{max}{}",
            if i < count as u16 && listed == stored && listed != 0 {
                "   AGREE"
            } else if i < count as u16 {
                "   MISMATCH"
            } else {
                ""
            }
        );
    }
    println!("wEnemyMonSpecies (active) = {:#04X}", gb.peek(0xCFE5));
    // Play a few turns so the panel is exercised on a battle in progress
    // rather than one that has only just started: the rival opens with GROWL,
    // which is what puts a stage on the board.
    // FIGHT, then *down* to the second move — the rival's Charmander opens
    // SCRATCH / GROWL and only GROWL puts a stage on the board, which is the
    // case the stage line exists for.
    for round in 0..40 {
        press(&mut gb, A, 12);
        if round % 2 == 0 {
            press(&mut gb, Down, 12);
        }
        press(&mut gb, A, 20);
        if gb.peek(0xD057) == 0 {
            break;
        }
        let mods: Vec<u8> = (0..6)
            .flat_map(|i| [gb.peek(0xCD2E + i), gb.peek(0xCD1A + i)])
            .collect();
        // Only once the engine has initialised the block: it is zeroed WRAM
        // until then, and a zero would read as a -6 that never happened.
        if mods.iter().all(|v| (1..=13).contains(v)) && mods.iter().any(|v| *v != 7) {
            println!("a stat stage landed on round {round}");
            break;
        }
    }
    println!(
        "enemy stat mods (raw, 7 = neutral) = {:?}",
        (0..6).map(|i| gb.peek(0xCD2E + i)).collect::<Vec<_>>()
    );
    println!(
        "player stat mods = {:?}",
        (0..6).map(|i| gb.peek(0xCD1A + i)).collect::<Vec<_>>()
    );
    // Save states, so the *real* frontend can be pointed at exactly this
    // moment: one in the battle, one after it, which is how the panel gets
    // screenshotted in a terminal rather than described.
    if let Some(dir) = states {
        std::fs::create_dir_all(dir).expect("create the state directory");
        std::fs::write(format!("{dir}/battle.gbss"), gb.save_state())
            .expect("write the battle state");
        let mut after = gb.save_state();
        for _ in 0..400 {
            press(&mut gb, A, 12);
            if gb.peek(0xD057) == 0 {
                after = gb.save_state();
                break;
            }
        }
        std::fs::write(format!("{dir}/after.gbss"), after)
            .expect("write the post-battle state");
        println!("states written to {dir}");
        // Put the machine back where the report expects it.
        let bytes = std::fs::read(format!("{dir}/battle.gbss")).unwrap();
        gb.load_state(&bytes).unwrap();
    }
    let p = gb
        .plugin::<terminalgb_plugin_trainer::TrainerPlugin>()
        .unwrap();
    let st = p.state();
    println!(
        "plugin: battle={:?} trainer={:?} team={:?} unknown={}",
        st.battle,
        st.trainer_name,
        st.enemy_team
            .iter()
            .map(|m| format!("{} L{} {}/{}", m.name, m.level, m.hp, m.max_hp))
            .collect::<Vec<_>>(),
        st.enemy_team_unknown
    );
    let inner = 38;
    println!("┌{}┐", "─".repeat(inner));
    for l in tv::plain(&tv::header_lines(st, "Pokémon Blue", inner)) {
        println!("│{l:<inner$}│");
    }
    for l in tv::plain(&tv::body_lines(st, tv::focus_at(st, 0), false, inner)) {
        println!("│{l:<inner$}│");
    }
    println!("└{}┘", "─".repeat(inner));
}

/// Write each named species' front sprite to a PNG **with the background made
/// transparent**, plus an ASCII map of the mask, so the flood fill can be
/// looked at rather than argued about.
///
/// The failure this exists to catch is the naive one: keying on "lightest
/// shade" instead of flood-filling from the border punches holes through
/// Charmander's belly and the core of its flame, and through the highlight on
/// Squirtle's shell. Those are the two the captain is looking at, so those are
/// the two checked by eye.
fn sprites(rom: &str, out_dir: &str) {
    let data = std::fs::read(rom).expect("read the ROM");
    let tables = terminalgb_plugin_trainer::rom::RomTables::new(data);
    std::fs::create_dir_all(out_dir).expect("create the output directory");
    for (label, index) in [
        ("bulbasaur", 0x99u8),
        ("charmander", 0xB0),
        ("squirtle", 0xB1),
        ("pikachu", 0x54),
    ] {
        let Some((pixels, w, h)) = tables.front_sprite(index) else {
            println!("{label}: no sprite");
            continue;
        };
        let bg = terminalgb_plugin_trainer::sprite::background_mask(&pixels, w, h);
        let lightest = pixels.iter().filter(|&&p| p == 0).count();
        let outside = bg.iter().filter(|b| **b).count();
        println!(
            "{label}: {w}x{h}  lightest={lightest}  background={outside}  \
             KEPT INSIDE THE ARTWORK={}",
            lightest - outside
        );
        for palette in [
            terminalgb::screen::config::Palette::Grayscale,
            terminalgb::screen::config::Palette::Green,
        ] {
            let mut rgba = Vec::with_capacity(pixels.len() * 4);
            for (i, &px) in pixels.iter().enumerate() {
                let [r, g, b] = palette.shade(px);
                rgba.extend_from_slice(&[r, g, b, if bg[i] { 0 } else { 255 }]);
            }
            let path = format!("{out_dir}/{label}-{}.png", palette.name());
            image::save_buffer(&path, &rgba, w as u32, h as u32, image::ColorType::Rgba8)
                .expect("write the PNG");
        }
        // The mask, as text: `#` is artwork, `.` is the page, `o` is a light
        // pixel the fill did NOT reach — the interior highlights a colour key
        // would have erased.
        for y in 0..h {
            let row: String = (0..w)
                .map(|x| {
                    let i = y * w + x;
                    if bg[i] {
                        '.'
                    } else if pixels[i] == 0 {
                        'o'
                    } else {
                        '#'
                    }
                })
                .collect();
            println!("  {row}");
        }
    }
}

/// Walk a real cartridge into a **wild** encounter and check the catch
/// arithmetic against the game's own data.
///
/// Two independent things are checked, and both have to hold:
///
/// 1. `wEnemyMonActualCatchRate` (`$D007`, the byte `ItemUsePokeBall` compares)
///    equals the species' catch rate in the cartridge's `BaseStats` table.
///    That is the address claim, checked against the ROM rather than against
///    another copy of our own assumption.
/// 2. The odds move the way the routine says as the encounter goes on.
///
/// ```text
/// cargo run --release --example trainer_layout -- --rom <rom> --wild
/// ```
fn to_wild(rom: &str, seed0: u64) {
    use terminalgb_plugin_trainer::capture::{ball_set, odds, Ball, Target};
    let path = std::path::PathBuf::from(rom);
    let data = std::fs::read(&path).expect("read the ROM");
    for ext in ["sav", "srm", "gbsave"] {
        let _ = std::fs::remove_file(path.with_extension(ext));
    }
    let tables = terminalgb_plugin_trainer::rom::RomTables::new(data.clone());
    let mut gb = terminalgb::gameboy::Gameboy::new(data.clone(), Some(path));
    let mut plugin = terminalgb_plugin_trainer::TrainerPlugin::new(data);
    plugin.set_active(true);
    gb.add_plugin(Box::new(plugin)).expect("attach");

    let press =
        |gb: &mut terminalgb::gameboy::Gameboy, k: terminalgb::KeypadKey, gap: u32| {
            gb.keydown(k);
            for _ in 0..4 {
                gb.frame();
            }
            gb.keyup(k);
            for _ in 0..gap {
                gb.frame();
            }
        };
    use terminalgb::KeypadKey::*;
    for step in 0..400u32 {
        press(&mut gb, if step % 2 == 0 { Start } else { A }, 20);
        if gb.peek(0xDA41) != 0 || gb.peek(0xDA43) != 0 || gb.peek(0xDA44) != 0 {
            break;
        }
    }
    let mut seed = seed0;
    for step in 0..30000u32 {
        if gb.peek(0xD057) == 1 && gb.peek(0xCFE5) != 0 {
            println!("wild battle after {step} rounds");
            break;
        }
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let r = (seed >> 33) % 10;
        let r = match (gb.peek(0xD35E), r) {
            (0x25, 0..=3) => 4,
            (0x26, 4) => 0,
            (0x28, 9) => 7,
            _ => r,
        };
        match r {
            0..=3 => press(&mut gb, Up, 16),
            4 => press(&mut gb, Down, 16),
            5 => press(&mut gb, Left, 16),
            6 => press(&mut gb, Right, 16),
            7 | 8 => press(&mut gb, A, 16),
            _ => press(&mut gb, B, 16),
        }
    }
    if gb.peek(0xD057) != 1 {
        println!("never reached a wild battle");
        return;
    }
    let species = gb.peek(0xCFE5);
    let live = gb.peek(0xD007);
    // Offset 8 of a `BaseStats` entry is the catch rate
    // (`data/pokemon/base_stats/*.asm`).
    let from_rom = tables.base_stats(species).map(|e| e[8]);
    println!(
        "species {species:#04X} {}: wEnemyMonActualCatchRate($D007) = {live}, \
         BaseStats catch rate = {from_rom:?}  {}",
        tables.species_name(species),
        if Some(live) == from_rom {
            "AGREE"
        } else {
            "MISMATCH"
        }
    );
    let p = gb
        .plugin::<terminalgb_plugin_trainer::TrainerPlugin>()
        .unwrap();
    let st = p.state();
    let o = st.opponent.as_ref().unwrap();
    println!("opponent {} L{} {}/{}", o.name, o.level, o.hp, o.max_hp);
    for b in ball_set(false) {
        let full = odds(Target::of(o, live), *b);
        println!("  {:<6} now {:.2}%", b.short(), full.chance * 100.0);
    }
    // The same target at one hit point and asleep — the direction the panel
    // promises the number moves.
    let mut hurt = Target::of(o, live);
    hurt.hp = 1;
    println!(
        "  at 1 HP:      POKé {:.2}%",
        odds(hurt, Ball::Poke).chance * 100.0
    );
    hurt.status = terminalgb_plugin_trainer::gen1::Status(0x04);
    println!(
        "  at 1 HP asleep: POKé {:.2}%",
        odds(hurt, Ball::Poke).chance * 100.0
    );
    let inner = 38;
    println!("┌{}┐", "─".repeat(inner));
    for l in tv::plain(&tv::header_lines(st, "Pokémon Blue", inner)) {
        println!("│{l:<inner$}│");
    }
    for l in tv::plain(&tv::body_lines(st, tv::focus_at(st, 0), false, inner)) {
        println!("│{l:<inner$}│");
    }
    println!("└{}┘", "─".repeat(inner));
}

fn key_for(c: char) -> Option<terminalgb::KeypadKey> {
    use terminalgb::KeypadKey::*;
    Some(match c {
        'a' => A,
        'b' => B,
        's' => Start,
        'e' => Select,
        'u' => Up,
        'd' => Down,
        'l' => Left,
        'r' => Right,
        '.' => return None,
        _ => return None,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if let Some(rom) = flag("--rom") {
        if let Some(dir) = flag("--sprites") {
            sprites(&rom, &dir);
            return;
        }
        if args.iter().any(|a| a == "--wild") {
            to_wild(
                &rom,
                flag("--seed")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0x5EED_1234),
            );
            return;
        }
        if args.iter().any(|a| a == "--to-battle") {
            to_battle(
                &rom,
                args.iter().any(|a| a == "--trainer"),
                flag("--states").as_deref(),
                flag("--seed")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0x5EED_1234),
            );
            return;
        }
        live(
            &rom,
            flag("--state").as_deref(),
            flag("--frames").and_then(|v| v.parse().ok()).unwrap_or(60),
            flag("--width").and_then(|v| v.parse().ok()).unwrap_or(40),
            &flag("--keys").unwrap_or_default(),
        );
        return;
    }
    let widths: Vec<usize> = {
        if args.is_empty() {
            vec![tv::PANEL_W as usize, 34, 28, 24]
        } else {
            args.iter().filter_map(|a| a.parse().ok()).collect()
        }
    };
    for w in widths {
        let idle = state(false);
        draw("out of battle", &idle, Some(tv::Focus::Party(0)), false, w);
        let fight = state(true);
        draw("in battle", &fight, Some(tv::Focus::Opponent), false, w);
        draw(
            "in battle, opponent detail",
            &fight,
            Some(tv::Focus::Opponent),
            true,
            w,
        );
        draw(
            "in battle, your Pokémon's detail",
            &fight,
            Some(tv::Focus::Party(0)),
            true,
            w,
        );
    }
}
