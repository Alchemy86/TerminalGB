//! Agent-substrate harness: proves the whole "Claude plays Pokémon" stack
//! end to end, deterministically, **for free**, and with no commercial ROM.
//!
//! `cargo test --release --test agent`
//!
//! # What is being proved, and why it needs a harness rather than unit tests
//!
//! The pieces each have unit tests (`src/agent/*`), but the property that
//! matters is a *seam*: a driver's JSON reply becomes a button press that the
//! running game observes, and the change it produces comes back to the driver.
//! Nothing short of a real CPU executing real code proves that.
//!
//! # The synthetic cartridge
//!
//! [`build_fake_gen1`] hand-assembles a 32 KiB ROM that presents **the same RAM
//! layout Pokémon Red/Blue does** — player name at `$D158`, party count at
//! `$D163`, map/coordinates at `$D35E`/`$D361`/`$D362`, facing at `$C109`, one
//! warp in the table at `$D3AE` — and moves the player around a walled 8x8 room
//! in response to the d-pad. It contains **no game data whatsoever**: it is
//! twenty instructions of our own, and its header says `POKEMON BLUE` purely so
//! the observer engages the Gen 1 reader (see the licence-boundary rule in
//! `AGENTS.md` — formats and addresses are facts, assets are not ours).
//!
//! That is what makes this runnable in CI. A test that needs the maintainer's
//! cartridge is a test CI skips, and a skipped test proves nothing.
//!
//! # Every case here failed at least once during development
//!
//! The walk-retry rule, the `advance_text` priming press and the "you did not
//! move at all" note were all found by watching a real agent flounder. Their
//! cases are marked.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use terminalgb::gameboy::Gameboy;
use terminalgb_plugin_agent::commentary::Problem;
use terminalgb_plugin_agent::driver::{Driver, Poll, ScriptedDriver};
use terminalgb_plugin_agent::governor::Budget;
use terminalgb_plugin_agent::{AgentPlugin, Config, PhaseView};

#[path = "support/fake_gen1.rs"]
mod fake_gen1;
use fake_gen1::*;

// ---------------------------------------------------------------------------
// Harness plumbing
// ---------------------------------------------------------------------------

struct Case {
    name: &'static str,
    ok: bool,
    detail: String,
}

fn dir(tag: &str) -> PathBuf {
    let d = PathBuf::from("target/agent-harness").join(tag);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("create work dir");
    d
}

/// Stage the fixture ROM on disk — the MBC writes beside the ROM path, so each
/// case gets its own directory.
fn stage(d: &Path) -> PathBuf {
    let p = d.join("fake-gen1.gb");
    std::fs::write(&p, build_fake_gen1()).expect("write fixture");
    p
}

fn machine(tag: &str) -> (Gameboy, PathBuf) {
    let d = dir(tag);
    let p = stage(&d);
    let data = std::fs::read(&p).unwrap();
    (Gameboy::new(data, Some(p.clone())), p)
}

fn reply(say: &str, plan: &str) -> String {
    format!("{{\"say\":\"{say}\",\"plan\":[{plan}]}}")
}

/// A machine whose screen is showing the Gen 1 naming keyboard.
fn machine_on_the_naming_screen(tag: &str) -> (Gameboy, PathBuf) {
    let d = dir(tag);
    let p = d.join("fake-gen1.gb");
    std::fs::write(
        &p,
        build_fake_gen1_with(&[(0, "YOUR NAME?"), (2, "A B C D E F G H I")]),
    )
    .expect("write fixture");
    let data = std::fs::read(&p).unwrap();
    (Gameboy::new(data, Some(p.clone())), p)
}

/// The same keyboard with the `▶` cursor on it, and a second letter row so the
/// menu's columns line up — the layout `Screen::selected` reads to name the
/// single letter A would type. Column 1 is the cursor, column 2 is `A`.
fn machine_on_the_naming_screen_with_a_cursor(tag: &str) -> (Gameboy, PathBuf) {
    let d = dir(tag);
    let p = d.join("fake-gen1.gb");
    std::fs::write(
        &p,
        build_fake_gen1_with(&[
            (0, "YOUR NAME?"),
            (2, " >A B C D E F G H I"),
            (4, "  J K L M N O P Q R"),
        ]),
    )
    .expect("write fixture");
    let data = std::fs::read(&p).unwrap();
    (Gameboy::new(data, Some(p.clone())), p)
}

/// A machine whose screen is showing a YES/NO confirmation box, cursor on YES.
fn machine_showing_a_yes_no_box(tag: &str) -> (Gameboy, PathBuf) {
    let d = dir(tag);
    let p = d.join("fake-gen1.gb");
    std::fs::write(
        &p,
        build_fake_gen1_with(&[(8, "SAVE THE GAME?"), (12, " >YES"), (14, "  NO")]),
    )
    .expect("write fixture");
    let data = std::fs::read(&p).unwrap();
    (Gameboy::new(data, Some(p.clone())), p)
}

/// Run until the agent stops or the frame budget runs out.
fn run(gb: &mut Gameboy, frames: u64) -> u64 {
    for f in 0..frames {
        gb.frame();
        if let Some(p) = gb.plugin::<AgentPlugin>() {
            if p.phase() == PhaseView::Stopped {
                return f;
            }
        }
    }
    frames
}

fn attach(gb: &mut Gameboy, driver: Box<dyn Driver>, cfg: Config) {
    gb.add_plugin(Box::new(AgentPlugin::new(driver, cfg)))
        .expect("attach the agent");
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

/// The seam: a driver's JSON reply becomes a real button press the running
/// program sees, and the coordinate it changes comes back as an outcome.
fn case_scripted_sequence_drives_the_game() -> Case {
    let (mut gb, _) = machine("sequence");
    let script = vec![
        reply("Heading right.", r#"{"do":"walk","dir":"right","steps":2}"#),
        reply("Now down.", r#"{"do":"walk","dir":"down","steps":3}"#),
        reply(
            "Off to the stairs.",
            r#"{"do":"goto","x":7,"y":1},{"do":"press","button":"a","times":2}"#,
        ),
    ];
    attach(
        &mut gb,
        Box::new(ScriptedDriver::new(script)),
        Config::default(),
    );
    run(&mut gb, 20_000);

    let (x, y, a_presses) = (gb.peek(W_X), gb.peek(W_Y), gb.peek(A_PRESSES));
    let ok = x == 7 && y == 1 && a_presses == 2;
    Case {
        name: "a scripted plan walks the player to the exit and presses A",
        ok,
        detail: format!(
            "ended at ({x},{y}), wanted (7,1); {a_presses} A presses, wanted 2"
        ),
    }
}

/// The regression that cost a whole real run: a press the game turns into a
/// turn rather than a step used to be reported as a wall, so the agent was
/// told every direction was blocked.
fn case_a_wall_is_only_reported_after_repeated_failures() -> Case {
    let (mut gb, _) = machine("wall");
    // Walk left far enough to hit the wall at x = 1, then try to keep going.
    let script = vec![
        reply(
            "Left as far as it goes.",
            r#"{"do":"walk","dir":"left","steps":10}"#,
        ),
        reply("Again.", r#"{"do":"walk","dir":"left","steps":3}"#),
        reply("Back right.", r#"{"do":"walk","dir":"right","steps":2}"#),
    ];
    attach(
        &mut gb,
        Box::new(ScriptedDriver::new(script)),
        Config::default(),
    );
    run(&mut gb, 30_000);
    let x = gb.peek(W_X);
    // It walked up to the wall column and stopped at the tile before it, was
    // not fooled into stopping early, and could still move away afterwards.
    let want = WALL_X + 1 + 2;
    let ok = x == want;
    Case {
        name: "walking into a wall stops there and moving away still works",
        ok,
        detail: format!("ended at x={x}, wanted {want}"),
    }
}

/// `goto` has to get round a *building*, not just a bed.
///
/// The room has a wall down column `WALL_X` with one doorway at `DOOR_Y`, and
/// the target is on the far side of it on a row the wall blocks. A greedy
/// walker that alternates one step at a time never gets there: it steps aside,
/// finds the direct route still blocked, steps back, and gives up — which is
/// exactly what a real run did in Pallet Town, a few tiles from Oak's lab.
fn case_goto_routes_around_a_wall() -> Case {
    let (mut gb, _) = machine("detour");
    let (tx, ty) = (MIN_XY, START_Y);
    let script = vec![reply(
        "Round the back.",
        &format!(r#"{{"do":"goto","x":{tx},"y":{ty}}}"#),
    )];
    attach(
        &mut gb,
        Box::new(ScriptedDriver::new(script)),
        Config::default(),
    );
    run(&mut gb, 60_000);
    let (x, y) = (gb.peek(W_X), gb.peek(W_Y));
    let ok = (x, y) == (tx, ty);
    Case {
        name: "goto routes around a wall instead of giving up in front of it",
        ok,
        detail: format!(
            "ended at ({x},{y}), wanted ({tx},{ty}) — the only way through is \
             the doorway at y={DOOR_Y}"
        ),
    }
}

/// The property the whole architecture rests on. A driver that takes 600
/// emulated frames to answer must cost the emulator nothing.
fn case_the_emulator_never_waits_for_the_agent() -> Case {
    let (mut gb, _) = machine("nonblocking");
    attach(
        &mut gb,
        Box::new(
            ScriptedDriver::new(vec![reply("Slowly.", r#"{"do":"press","button":"a"}"#)])
                .looping()
                .with_latency(600),
        ),
        Config::default(),
    );
    // Warm up, then time 3000 frames while the driver is permanently thinking.
    for _ in 0..120 {
        gb.frame();
    }
    let t = Instant::now();
    let mut worst = Duration::ZERO;
    for _ in 0..3000 {
        let f = Instant::now();
        gb.frame();
        worst = worst.max(f.elapsed());
    }
    let per_frame = t.elapsed().as_secs_f64() * 1000.0 / 3000.0;
    // A real DMG frame is 16.743 ms. Anything close to that means the agent is
    // in the way; healthy is well under 1 ms.
    let ok = per_frame < 4.0 && worst < Duration::from_millis(50);
    Case {
        name: "the emulator runs at full speed while the agent is thinking",
        ok,
        detail: format!(
            "{per_frame:.3} ms/frame (budget 16.743), worst frame {:.1} ms",
            worst.as_secs_f64() * 1000.0
        ),
    }
}

/// ...and a driver that has *died* must be no worse.
fn case_a_dead_agent_does_not_stop_the_game() -> Case {
    let (mut gb, _) = machine("dead");
    // A driver that answers nothing, ever: `submit` fails immediately.
    struct Dead;
    impl Driver for Dead {
        fn name(&self) -> &str {
            "dead"
        }
        fn submit(&mut self, _: &str) -> bool {
            false
        }
        fn poll(&mut self) -> Poll {
            Poll::Idle
        }
    }
    attach(&mut gb, Box::new(Dead), Config::default());
    let t = Instant::now();
    for _ in 0..3000 {
        gb.frame();
    }
    let per_frame = t.elapsed().as_secs_f64() * 1000.0 / 3000.0;
    let ok = per_frame < 4.0;
    Case {
        name: "a dead agent leaves the game running at full speed",
        ok,
        detail: format!("{per_frame:.3} ms/frame with no driver answering"),
    }
}

/// The budget cap must be impossible to exceed, and it must end the session
/// rather than merely slowing it.
fn case_the_budget_cap_fires() -> Case {
    let (mut gb, _) = machine("budget");
    let cfg = Config {
        budget: Budget {
            max_calls: 4,
            ..Budget::default()
        },
        ..Config::default()
    };
    attach(
        &mut gb,
        Box::new(
            ScriptedDriver::new(vec![reply("go", r#"{"do":"press","button":"a"}"#)])
                .looping(),
        ),
        cfg,
    );
    run(&mut gb, 60_000);
    let p = gb.plugin::<AgentPlugin>().unwrap();
    let calls = p.governor().calls();
    let halted = p.governor().halted();
    let said = p.commentary().pause_reason().unwrap_or("").to_string();
    let ok = calls == 4
        && halted == Some(terminalgb_plugin_agent::governor::Halt::Calls)
        && said.contains("decision budget");
    Case {
        name: "the call budget stops the agent and says so publicly",
        ok,
        detail: format!("{calls} calls, halted {halted:?}, panel says {said:?}"),
    }
}

/// The circuit breaker: consecutive failures stop the retries.
fn case_the_circuit_breaker_fires() -> Case {
    let (mut gb, _) = machine("breaker");
    // A driver that answers, but with something unusable every time.
    struct Broken {
        calls: u32,
        busy: bool,
    }
    impl Driver for Broken {
        fn name(&self) -> &str {
            "broken"
        }
        fn submit(&mut self, _: &str) -> bool {
            self.calls += 1;
            self.busy = true;
            true
        }
        fn poll(&mut self) -> Poll {
            if !self.busy {
                return Poll::Idle;
            }
            self.busy = false;
            Poll::Failed(terminalgb_plugin_agent::driver::DriverError::Crashed(
                "/home/somebody/secret.py exploded".to_string(),
            ))
        }
    }
    let cfg = Config {
        budget: Budget {
            max_calls: 500,
            max_consecutive_failures: 3,
            breaker_cooldown: Duration::from_millis(30),
            ..Budget::default()
        },
        settle_frames: 2,
        ..Config::default()
    };
    attach(
        &mut gb,
        Box::new(Broken {
            calls: 0,
            busy: false,
        }),
        cfg,
    );
    run(&mut gb, 40_000);
    let p = gb.plugin::<AgentPlugin>().unwrap();
    let g = p.governor();
    let calls = g.calls();
    let halted = g.halted();
    // And the panel must never have seen the path in that error message.
    let leaked = p
        .commentary()
        .history()
        .any(|e| e.text.contains("/home/") || e.text.contains(".py"));
    let saw_category = p
        .commentary()
        .history()
        .any(|e| e.text == Problem::Crashed.public_text());
    let ok = halted == Some(terminalgb_plugin_agent::governor::Halt::Broken)
        && calls < 60
        && !leaked
        && saw_category;
    Case {
        name: "the circuit breaker stops a broken driver without leaking its error",
        ok,
        detail: format!(
            "{calls} calls before giving up, halted {halted:?}, \
             leaked={leaked}, showed the category={saw_category}"
        ),
    }
}

/// A driver that keeps proposing the same useless plan must be noticed and
/// escalated, not repeated for ever.
fn case_stuck_detection_escalates() -> Case {
    let (mut gb, _) = machine("stuck");
    // Walking left into the wall forever, from the wall.
    let script = vec![
        reply(
            "Get to the wall.",
            r#"{"do":"walk","dir":"left","steps":8}"#,
        ),
        reply("Left.", r#"{"do":"walk","dir":"left","steps":1}"#),
    ];
    attach(
        &mut gb,
        Box::new(ScriptedDriver::new(script).looping()),
        Config {
            budget: Budget {
                max_calls: 14,
                ..Budget::default()
            },
            settle_frames: 4,
            ..Config::default()
        },
    );
    run(&mut gb, 120_000);
    let p = gb.plugin::<AgentPlugin>().unwrap();
    // The host must have told it, in the panel, that it was going in circles,
    // and must have intervened locally rather than paying for another identical
    // decision.
    let intervened = p
        .commentary()
        .history()
        .any(|e| e.text.contains("Going in circles"));
    Case {
        name: "an agent repeating itself is detected and interrupted",
        ok: intervened,
        detail: format!(
            "intervened={intervened} after {} calls",
            p.governor().calls()
        ),
    }
}

/// The observation layer, read back through the real machine: position,
/// facing, map name, the warp table, and the screen decoded as text.
fn case_observation_reads_the_running_game() -> Case {
    let (mut gb, _) = machine("observe");
    // A driver that just records the snapshot it was given.
    struct Peek {
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        busy: bool,
    }
    impl Driver for Peek {
        fn name(&self) -> &str {
            "peek"
        }
        fn submit(&mut self, request: &str) -> bool {
            self.seen.lock().unwrap().push(request.to_string());
            self.busy = true;
            true
        }
        fn poll(&mut self) -> Poll {
            if !self.busy {
                return Poll::Idle;
            }
            self.busy = false;
            Poll::Ready(reply("look", r#"{"do":"walk","dir":"right","steps":1}"#))
        }
    }
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    attach(
        &mut gb,
        Box::new(Peek {
            seen: seen.clone(),
            busy: false,
        }),
        Config {
            budget: Budget {
                max_calls: 3,
                ..Budget::default()
            },
            ..Config::default()
        },
    );
    run(&mut gb, 20_000);
    let snaps = seen.lock().unwrap().clone();
    let last = snaps.last().cloned().unwrap_or_default();
    let checks = [
        ("in_game", last.contains("\"in_game\":true")),
        ("map name", last.contains("Red's house 2F")),
        ("facing", last.contains("\"facing\":\"right\"")),
        ("exit", last.contains("\"to_map\":37")),
        ("screen text", last.contains("ROOM")),
        ("mode", last.contains("\"mode\":\"overworld\"")),
        ("outcome fed back", last.contains("walked 1 step(s)")),
    ];
    let missing: Vec<&str> = checks
        .iter()
        .filter(|(_, ok)| !ok)
        .map(|(n, _)| *n)
        .collect();
    Case {
        name: "the snapshot reports position, facing, exits and the screen",
        ok: missing.is_empty() && snaps.len() >= 2,
        detail: if missing.is_empty() {
            format!("{} snapshots, all fields present", snaps.len())
        } else {
            format!("missing: {missing:?}")
        },
    }
}

/// A snapshot goes out to a driver, so it must not be a leak either. Nothing
/// in it should be a host path.
fn case_the_snapshot_carries_no_host_paths() -> Case {
    let (mut gb, _) = machine("nopaths");
    let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    struct Grab(std::sync::Arc<std::sync::Mutex<String>>, bool);
    impl Driver for Grab {
        fn name(&self) -> &str {
            "grab"
        }
        fn submit(&mut self, r: &str) -> bool {
            *self.0.lock().unwrap() = r.to_string();
            self.1 = true;
            true
        }
        fn poll(&mut self) -> Poll {
            if !self.1 {
                return Poll::Idle;
            }
            self.1 = false;
            Poll::Ready(reply("ok", r#"{"do":"wait","frames":10}"#))
        }
    }
    attach(
        &mut gb,
        Box::new(Grab(seen.clone(), false)),
        Config {
            budget: Budget {
                max_calls: 1,
                ..Budget::default()
            },
            ..Config::default()
        },
    );
    run(&mut gb, 5_000);
    let snap = seen.lock().unwrap().clone();
    // Frame capture is off by default, so no path may appear at all.
    let ok = !snap.contains("/home/")
        && !snap.contains("frame_png")
        && snap.contains("\"schema\":\"gbagent/1\"");
    Case {
        name: "a snapshot carries no host paths unless frame capture is on",
        ok,
        detail: format!("{} bytes, schema present={}", snap.len(), ok),
    }
}

/// A driver reply full of nonsense must be survivable: the game keeps running,
/// the bad actions are reported back, and the good one still happens.
fn case_a_malformed_reply_is_survivable() -> Case {
    let (mut gb, _) = machine("malformed");
    let script = vec![
        "not json at all".to_string(),
        "Sure! ```json\n{\"say\":\"fenced\",\"plan\":[{\"do\":\"walk\",\"dir\":\"right\",\"steps\":1}]}\n```".to_string(),
        r#"{"say":"mixed","plan":[{"do":"fly"},{"do":"walk","dir":"down","steps":1},{"do":"press"}]}"#.to_string(),
    ];
    attach(
        &mut gb,
        Box::new(ScriptedDriver::new(script)),
        Config {
            settle_frames: 4,
            ..Config::default()
        },
    );
    run(&mut gb, 30_000);
    let (x, y) = (gb.peek(W_X), gb.peek(W_Y));
    let ok = x == START_X + 1 && y == START_Y + 1;
    Case {
        name: "prose, fences and bad actions are tolerated; the good ones still run",
        ok,
        detail: format!(
            "ended at ({x},{y}), wanted ({},{})",
            START_X + 1,
            START_Y + 1
        ),
    }
}

/// The harness must be able to fail. If the input queue never reached the
/// keypad, every case above would still "pass" by doing nothing, so this one
/// asserts the fixture actually responds to a button.
fn case_the_fixture_is_not_inert() -> Case {
    let (mut gb, _) = machine("selfcheck");
    for _ in 0..60 {
        gb.frame();
    }
    let before = (gb.peek(W_X), gb.peek(A_PRESSES));
    gb.queue_press(terminalgb::KeypadKey::Right, 16, 14);
    gb.tap(terminalgb::KeypadKey::A);
    for _ in 0..120 {
        gb.frame();
    }
    let after = (gb.peek(W_X), gb.peek(A_PRESSES));
    let ok = after == (before.0 + 1, before.1 + 1);
    Case {
        name: "self-check: the fixture really does respond to the buttons",
        ok,
        detail: format!("{before:?} -> {after:?}"),
    }
}

/// The naming keyboard is the one screen where A types a letter instead of
/// advancing, and a real run typed a name of A's **from the host's own
/// `advance_text` loop** — the model never got a say. The rule is therefore
/// enforced where the presses happen: `advance_text` must refuse to press
/// anything there, tell the driver why, and the snapshot must carry the
/// situational note.
fn case_advance_text_refuses_the_naming_keyboard() -> Case {
    let (mut gb, rom) = machine_on_the_naming_screen("naming");
    let log = rom.parent().unwrap().join("log.ndjson");
    let script = vec![
        reply("Reading on.", r#"{"do":"advance_text"}"#),
        reply("Waiting.", r#"{"do":"wait","frames":1}"#),
    ];
    attach(
        &mut gb,
        Box::new(ScriptedDriver::new(script)),
        Config {
            log: Some(log.clone()),
            ..Config::default()
        },
    );
    run(&mut gb, 20_000);

    let a_presses = gb.peek(A_PRESSES);
    let logged = std::fs::read_to_string(&log).unwrap_or_default();
    let refused = logged.contains("naming keyboard is up");
    let noted = logged.contains("NAMING KEYBOARD");
    let ok = a_presses == 0 && refused && noted;
    Case {
        name: "advance_text refuses to type on the naming keyboard",
        ok,
        detail: format!(
            "{a_presses} A press(es), wanted 0; outcome told the driver: \
             {refused}; snapshot note present: {noted}"
        ),
    }
}

/// A YES/NO box is a question, and `advance_text` pressing A through it answers
/// that question with whatever the cursor happens to rest on — before the model
/// has seen it asked. Blind spot 6 of the live-run list, which was harmless that
/// session and will not always be. The stop is enforced where the presses
/// happen, like the naming-keyboard one, and the snapshot says so in words.
fn case_advance_text_refuses_a_yes_no_box() -> Case {
    let (mut gb, rom) = machine_showing_a_yes_no_box("yesno");
    let log = rom.parent().unwrap().join("log.ndjson");
    let script = vec![
        reply("Reading on.", r#"{"do":"advance_text"}"#),
        reply("Waiting.", r#"{"do":"wait","frames":1}"#),
    ];
    attach(
        &mut gb,
        Box::new(ScriptedDriver::new(script)),
        Config {
            log: Some(log.clone()),
            ..Config::default()
        },
    );
    run(&mut gb, 20_000);

    let a_presses = gb.peek(A_PRESSES);
    // The log embeds each request as a JSON *string*, so its own quotes arrive
    // escaped; unescape before looking for a field by name.
    let logged = std::fs::read_to_string(&log)
        .unwrap_or_default()
        .replace("\\\"", "\"");
    let checks = [
        ("no A pressed", a_presses == 0),
        ("driver told why", logged.contains("YES/NO box is up")),
        ("cursor named", logged.contains("cursor on YES")),
        ("kind reported", logged.contains("\"kind\":\"yes-no\"")),
        ("note present", logged.contains("This is a QUESTION")),
    ];
    let missing: Vec<&str> = checks
        .iter()
        .filter(|(_, ok)| !ok)
        .map(|(n, _)| *n)
        .collect();
    Case {
        name: "advance_text refuses to answer a YES/NO box",
        ok: missing.is_empty(),
        detail: if missing.is_empty() {
            format!("{a_presses} A press(es), wanted 0; refused, named and noted")
        } else {
            format!("missing: {missing:?}")
        },
    }
}

/// The screen channel, end to end: what the observer decodes has to arrive in
/// the JSON a driver is actually handed, in a shape it can act on. Four of the
/// six blind spots the live-run list records
/// (`docs/claude-plays.md` §11) are answered by fields checked here — the
/// cursor's column, the item it points at, the trust verdict, and the kind of
/// screen — so this is the case that fails if any of them stops being sent.
fn case_the_screen_reaches_the_driver_as_a_first_class_field() -> Case {
    let (mut gb, _) = machine_on_the_naming_screen_with_a_cursor("screenfield");
    struct Peek {
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        busy: bool,
    }
    impl Driver for Peek {
        fn name(&self) -> &str {
            "peek"
        }
        fn submit(&mut self, request: &str) -> bool {
            self.seen.lock().unwrap().push(request.to_string());
            self.busy = true;
            true
        }
        fn poll(&mut self) -> Poll {
            if !self.busy {
                return Poll::Idle;
            }
            self.busy = false;
            Poll::Ready(reply("looking", r#"{"do":"wait","frames":4}"#))
        }
    }
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    attach(
        &mut gb,
        Box::new(Peek {
            seen: seen.clone(),
            busy: false,
        }),
        Config {
            budget: Budget {
                max_calls: 3,
                ..Budget::default()
            },
            ..Config::default()
        },
    );
    run(&mut gb, 20_000);
    let snaps = seen.lock().unwrap().clone();
    let first = snaps.first().cloned().unwrap_or_default();
    let last = snaps.last().cloned().unwrap_or_default();
    let checks = [
        // The text itself, as the player sees it.
        ("text", last.contains("YOUR NAME?")),
        ("letter grid", last.contains("A B C D E F G H I")),
        // The cursor is *in* the text, not decoded away to a blank.
        ("cursor glyph", last.contains('\u{25B6}')),
        // Blind spot 2: which letter is under the cursor.
        ("cursor column", last.contains("\"menu_cursor_col\":1")),
        // Blind spot 3: the item the cursor points at, as a fact.
        ("selected", last.contains("\"selected\":\"A\"")),
        // The kind of screen, and that the read is trustworthy.
        ("kind", last.contains("\"kind\":\"naming-keyboard\"")),
        ("trust", last.contains("\"trust\":\"ok\"")),
        // Sent once, so the fields mean something to whoever reads them.
        ("screen help once", first.contains("screen_help")),
        (
            "help not repeated",
            snaps.len() < 2 || !last.contains("screen_help"),
        ),
    ];
    let missing: Vec<&str> = checks
        .iter()
        .filter(|(_, ok)| !ok)
        .map(|(n, _)| *n)
        .collect();
    Case {
        name: "the decoded screen reaches the driver, with cursor, selection and trust",
        ok: missing.is_empty() && snaps.len() >= 2,
        detail: if missing.is_empty() {
            format!("{} snapshots, every screen field present", snaps.len())
        } else {
            format!("missing: {missing:?}")
        },
    }
}

/// Text that cannot be trusted must be *withheld*, not sent looking like data.
/// A real run spent a model call reasoning about `KU:N:` — half of two screens.
/// Here the untrustworthy case is the one a single read can prove: artwork laid
/// out as sequential tiles through the font range, which decodes as alphabet
/// salad with a perfectly straight face.
fn case_untrustworthy_text_is_withheld_and_explained() -> Case {
    let d = dir("salad");
    let p = d.join("fake-gen1.gb");
    // Sequential tile ids through the letter range: `ABCDEFGHIJKL`. No English
    // text contains that, and the Game Freak intro on a real cartridge does.
    let salad: String = ('A'..='L').collect();
    std::fs::write(&p, build_fake_gen1_with(&[(4, &salad)])).expect("write fixture");
    let data = std::fs::read(&p).unwrap();
    let mut gb = Gameboy::new(data, Some(p.clone()));

    struct Peek {
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        busy: bool,
    }
    impl Driver for Peek {
        fn name(&self) -> &str {
            "peek"
        }
        fn submit(&mut self, request: &str) -> bool {
            self.seen.lock().unwrap().push(request.to_string());
            self.busy = true;
            true
        }
        fn poll(&mut self) -> Poll {
            if !self.busy {
                return Poll::Idle;
            }
            self.busy = false;
            Poll::Ready(reply("looking", r#"{"do":"wait","frames":4}"#))
        }
    }
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    attach(
        &mut gb,
        Box::new(Peek {
            seen: seen.clone(),
            busy: false,
        }),
        Config {
            budget: Budget {
                max_calls: 2,
                ..Budget::default()
            },
            ..Config::default()
        },
    );
    run(&mut gb, 20_000);
    let last = seen.lock().unwrap().last().cloned().unwrap_or_default();
    let checks = [
        ("flagged", last.contains("\"trust\":\"graphics-not-text\"")),
        ("withheld", last.contains("\"text\":\"\"")),
        ("no salad", !last.contains("ABCDEF")),
        ("explained", last.contains("GRAPHICS, not text")),
    ];
    let missing: Vec<&str> = checks
        .iter()
        .filter(|(_, ok)| !ok)
        .map(|(n, _)| *n)
        .collect();
    Case {
        name: "a screen that is graphics, not text, is withheld and says why",
        ok: missing.is_empty(),
        detail: if missing.is_empty() {
            "flagged, withheld and explained in the notes".to_string()
        } else {
            format!("missing: {missing:?}")
        },
    }
}

fn main() {
    let cases = vec![
        case_the_fixture_is_not_inert(),
        case_advance_text_refuses_the_naming_keyboard(),
        case_advance_text_refuses_a_yes_no_box(),
        case_the_screen_reaches_the_driver_as_a_first_class_field(),
        case_untrustworthy_text_is_withheld_and_explained(),
        case_scripted_sequence_drives_the_game(),
        case_a_wall_is_only_reported_after_repeated_failures(),
        case_goto_routes_around_a_wall(),
        case_observation_reads_the_running_game(),
        case_the_snapshot_carries_no_host_paths(),
        case_a_malformed_reply_is_survivable(),
        case_the_emulator_never_waits_for_the_agent(),
        case_a_dead_agent_does_not_stop_the_game(),
        case_the_budget_cap_fires(),
        case_the_circuit_breaker_fires(),
        case_stuck_detection_escalates(),
    ];

    let mut failed = 0;
    println!("\nagent substrate harness\n");
    for c in &cases {
        println!(
            "  [{}] {}\n        {}",
            if c.ok { "PASS" } else { "FAIL" },
            c.name,
            c.detail
        );
        if !c.ok {
            failed += 1;
        }
    }
    println!("\n{} of {} passed\n", cases.len() - failed, cases.len());
    if failed > 0 {
        std::process::exit(1);
    }
}
