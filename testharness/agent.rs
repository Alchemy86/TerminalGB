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
//! That is what makes this runnable in CI. A test that needs the captain's
//! cartridge is a test CI skips, and a skipped test proves nothing.
//!
//! # Every case here failed at least once during development
//!
//! The walk-retry rule, the `advance_text` priming press and the "you did not
//! move at all" note were all found by watching a real agent flounder. Their
//! cases are marked.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gameboy::gameboy::Gameboy;
use gameboy_plugin_agent::commentary::Problem;
use gameboy_plugin_agent::driver::{Driver, Poll, ScriptedDriver};
use gameboy_plugin_agent::governor::Budget;
use gameboy_plugin_agent::{AgentPlugin, Config, PhaseView};

// ---------------------------------------------------------------------------
// A tiny absolute-jump assembler, so the program below can be read
// ---------------------------------------------------------------------------

struct Asm {
    code: Vec<u8>,
    base: u16,
    labels: HashMap<&'static str, u16>,
    fixups: Vec<(usize, &'static str)>,
}

impl Asm {
    fn new(base: u16) -> Asm {
        Asm {
            code: Vec::new(),
            base,
            labels: HashMap::new(),
            fixups: Vec::new(),
        }
    }

    fn here(&self) -> u16 {
        self.base + self.code.len() as u16
    }

    fn label(&mut self, name: &'static str) {
        let at = self.here();
        self.labels.insert(name, at);
    }

    fn raw(&mut self, bytes: &[u8]) {
        self.code.extend_from_slice(bytes);
    }

    /// `LD A,n`
    fn ld_a(&mut self, n: u8) {
        self.raw(&[0x3E, n]);
    }
    /// `LD (nn),A`
    fn st(&mut self, addr: u16) {
        self.raw(&[0xEA, addr as u8, (addr >> 8) as u8]);
    }
    /// `LD A,(nn)`
    fn ld_from(&mut self, addr: u16) {
        self.raw(&[0xFA, addr as u8, (addr >> 8) as u8]);
    }
    /// `LD HL,nn`
    fn ld_hl(&mut self, nn: u16) {
        self.raw(&[0x21, nn as u8, (nn >> 8) as u8]);
    }
    /// `LD (HL+),A`
    fn st_hli(&mut self) {
        self.raw(&[0x22]);
    }
    fn xor_a(&mut self) {
        self.raw(&[0xAF]);
    }
    fn cpl(&mut self) {
        self.raw(&[0x2F]);
    }
    fn and_n(&mut self, n: u8) {
        self.raw(&[0xE6, n]);
    }
    fn cp_n(&mut self, n: u8) {
        self.raw(&[0xFE, n]);
    }
    fn inc_a(&mut self) {
        self.raw(&[0x3C]);
    }
    fn dec_a(&mut self) {
        self.raw(&[0x3D]);
    }
    /// `LDH ($FF00+n),A`
    fn ldh_st(&mut self, n: u8) {
        self.raw(&[0xE0, n]);
    }
    /// `LDH A,($FF00+n)`
    fn ldh_ld(&mut self, n: u8) {
        self.raw(&[0xF0, n]);
    }
    /// `BIT b,D` — D is register index 2 in the CB table.
    fn bit_d(&mut self, b: u8) {
        self.raw(&[0xCB, 0x40 + 8 * b + 2]);
    }

    fn jump(&mut self, opcode: u8, target: &'static str) {
        self.raw(&[opcode]);
        self.fixups.push((self.code.len(), target));
        self.raw(&[0x00, 0x00]);
    }
    /// `JP nn`
    fn jp(&mut self, target: &'static str) {
        self.jump(0xC3, target);
    }
    /// `JP Z,nn`
    fn jp_z(&mut self, target: &'static str) {
        self.jump(0xCA, target);
    }
    /// `JP NC,nn`
    fn jp_nc(&mut self, target: &'static str) {
        self.jump(0xD2, target);
    }
    /// `JP C,nn`
    fn jp_c(&mut self, target: &'static str) {
        self.jump(0xDA, target);
    }
    /// `JP NZ,nn`
    fn jp_nz(&mut self, target: &'static str) {
        self.jump(0xC2, target);
    }
    /// `LD L,A` / `LD A,L` — a scratch byte for "where would this move put me?"
    fn ld_l_a(&mut self) {
        self.raw(&[0x6F]);
    }
    fn ld_a_l(&mut self) {
        self.raw(&[0x7D]);
    }

    fn finish(mut self) -> Vec<u8> {
        for (at, name) in std::mem::take(&mut self.fixups) {
            let target = *self
                .labels
                .get(name)
                .unwrap_or_else(|| panic!("undefined label {name}"));
            self.code[at] = target as u8;
            self.code[at + 1] = (target >> 8) as u8;
        }
        self.code
    }
}

// Gen 1 addresses this fixture reproduces. Same values as `src/agent/observe`
// and `src/trainer/gen1` — deliberately repeated here rather than imported, so
// the fixture is an independent statement of the layout the reader expects.
const W_PLAYER_NAME: u16 = 0xD158;
const W_PARTY_COUNT: u16 = 0xD163;
const W_CUR_MAP: u16 = 0xD35E;
const W_Y: u16 = 0xD361;
const W_X: u16 = 0xD362;
const W_FACING: u16 = 0xC109;
const W_WARPS: u16 = 0xD3AE;
/// `wIsInBattle`, `wObtainedBadges` and `wPlayerMoney`. The fixture zeroes
/// them for the same reason the real game does: WRAM boots full of noise, and
/// an observer that reads it reports a level-201 opponent and four badges.
/// (It did, on the first run of this harness.)
const W_IN_BATTLE: u16 = 0xD057;
const W_BADGES: u16 = 0xD356;
const W_MONEY: u16 = 0xD347;
/// Where the fixture counts A presses, so a test can see them land.
const A_PRESSES: u16 = 0xD000;
const A_PREV: u16 = 0xD001;

const START_X: u8 = 5;
const START_Y: u8 = 5;
const MIN_XY: u8 = 1;
const MAX_XY: u8 = 8;
/// A wall down column `x = WALL_X`, with a single doorway at `y = DOOR_Y`.
///
/// The room needs an obstacle a *straight line* cannot get round, because that
/// is the shape `goto` was bad at: single-step axis alternation walks one tile
/// aside, finds the direct route still blocked, walks back, and repeats. A bed
/// it can manage; a building it cannot. This is the building.
const WALL_X: u8 = 2;
const DOOR_Y: u8 = 8;

/// A 32 KiB cartridge that walks a player round a walled room.
fn build_fake_gen1() -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];

    // Entry: NOP; JP $0150.
    rom[0x0100..0x0104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
    for (i, b) in b"POKEMON BLUE".iter().enumerate() {
        rom[0x0134 + i] = *b;
    }
    rom[0x0143] = 0x00; // DMG cartridge
    rom[0x0147] = 0x00; // ROM only
    rom[0x0148] = 0x00;
    rom[0x0149] = 0x00;
    // The header checksum the observer checks against, so the Gen 1 reader
    // engages. Nothing of the real game is here — only this number.
    rom[0x014E] = 0x9D;
    rom[0x014F] = 0x0A;
    let mut sum: u8 = 0;
    for b in rom.iter().take(0x014D).skip(0x0134) {
        sum = sum.wrapping_sub(*b).wrapping_sub(1);
    }
    rom[0x014D] = sum;

    let mut a = Asm::new(0x0150);

    // --- init -------------------------------------------------------------
    // A player name, so `looks_like_a_name` passes and the observer reports
    // `in_game`. Gen 1 character codes: $80 = 'A'.
    a.ld_hl(W_PLAYER_NAME);
    for c in [0x80u8, 0x86, 0x84, 0x8D, 0x93, 0x50] {
        a.ld_a(c);
        a.st_hli();
    }
    a.xor_a();
    a.st(W_PARTY_COUNT);
    a.st(W_FACING); // facing down
    a.st(A_PRESSES);
    a.st(A_PREV);
    a.st(W_IN_BATTLE);
    a.st(W_BADGES);
    a.st(W_MONEY);
    a.st(W_MONEY + 1);
    a.st(W_MONEY + 2);
    a.ld_a(0x26); // Red's house 2F, so the map name resolves
    a.st(W_CUR_MAP);
    a.ld_a(START_Y);
    a.st(W_Y);
    a.ld_a(START_X);
    a.st(W_X);
    // One exit at (7,1) leading to map $25, in the warp-table layout the
    // observer reads: count, then y, x, dest-warp, dest-map.
    a.ld_a(1);
    a.st(W_WARPS);
    a.st(W_WARPS + 1); // y = 1
    a.ld_a(7);
    a.st(W_WARPS + 2); // x = 7
    a.xor_a();
    a.st(W_WARPS + 3);
    a.ld_a(0x25);
    a.st(W_WARPS + 4);

    // Text into the background tilemap. Gen 1 loads its font so a tile id *is*
    // a character code, which is what makes the screen readable as text; the
    // fixture reproduces that by writing character codes straight in.
    a.ld_hl(0x9800);
    for c in b"ROOM" {
        a.ld_a(0x80 + (c - b'A'));
        a.st_hli();
    }
    // ...and enough distinct tiles after it that the screen classifies as a
    // drawn map rather than a menu. Real maps are made of many block tiles;
    // `Screen::looks_like_a_map` counts them, so a fixture with four tiles on
    // screen would be reported as a transition.
    for t in 0x01u8..0x10 {
        a.ld_a(t);
        a.st_hli();
    }

    a.xor_a();
    a.raw(&[0x47]); // LD B,A — previous direction bits

    // --- main loop ---------------------------------------------------------
    a.label("loop");
    a.ld_a(0x20); // select the direction row
    a.ldh_st(0x00);
    a.ldh_ld(0x00);
    a.cpl();
    a.and_n(0x0F); // 1 = pressed
    a.raw(&[0x4F]); // LD C,A
    a.raw(&[0x78]); // LD A,B
    a.cpl();
    a.raw(&[0xA1]); // AND C   -> newly-pressed edges
    a.raw(&[0x57]); // LD D,A
    a.raw(&[0x79]); // LD A,C
    a.raw(&[0x47]); // LD B,A  -> previous = current

    // Each direction: face that way, work out where the step would land, and
    // refuse it if that is off the map or inside the wall column.
    a.bit_d(0); // Right
    a.jp_z("no_right");
    a.ld_a(0x0C);
    a.st(W_FACING);
    a.ld_from(W_X);
    a.cp_n(MAX_XY);
    a.jp_nc("no_right");
    a.inc_a();
    a.ld_l_a(); // L = the x this step would land on
    a.cp_n(WALL_X);
    a.jp_nz("right_ok");
    a.ld_from(W_Y);
    a.cp_n(DOOR_Y);
    a.jp_nz("no_right");
    a.label("right_ok");
    a.ld_a_l();
    a.st(W_X);
    a.label("no_right");

    a.bit_d(1); // Left
    a.jp_z("no_left");
    a.ld_a(0x08);
    a.st(W_FACING);
    a.ld_from(W_X);
    a.cp_n(MIN_XY + 1);
    a.jp_c("no_left");
    a.dec_a();
    a.ld_l_a();
    a.cp_n(WALL_X);
    a.jp_nz("left_ok");
    a.ld_from(W_Y);
    a.cp_n(DOOR_Y);
    a.jp_nz("no_left");
    a.label("left_ok");
    a.ld_a_l();
    a.st(W_X);
    a.label("no_left");

    a.bit_d(2); // Up
    a.jp_z("no_up");
    a.ld_a(0x04);
    a.st(W_FACING);
    a.ld_from(W_Y);
    a.cp_n(MIN_XY + 1);
    a.jp_c("no_up");
    a.dec_a();
    a.ld_l_a(); // L = the y this step would land on
    a.ld_from(W_X);
    a.cp_n(WALL_X);
    a.jp_nz("up_ok");
    a.ld_a_l();
    a.cp_n(DOOR_Y);
    a.jp_nz("no_up");
    a.label("up_ok");
    a.ld_a_l();
    a.st(W_Y);
    a.label("no_up");

    a.bit_d(3); // Down
    a.jp_z("no_down");
    a.xor_a();
    a.st(W_FACING);
    a.ld_from(W_Y);
    a.cp_n(MAX_XY);
    a.jp_nc("no_down");
    a.inc_a();
    a.ld_l_a();
    a.ld_from(W_X);
    a.cp_n(WALL_X);
    a.jp_nz("down_ok");
    a.ld_a_l();
    a.cp_n(DOOR_Y);
    a.jp_nz("no_down");
    a.label("down_ok");
    a.ld_a_l();
    a.st(W_Y);
    a.label("no_down");

    // A button, counted on its rising edge.
    a.ld_a(0x10); // select the button row
    a.ldh_st(0x00);
    a.ldh_ld(0x00);
    a.cpl();
    a.and_n(0x01);
    a.raw(&[0x5F]); // LD E,A
    a.ld_from(A_PREV);
    a.cpl();
    a.raw(&[0xA3]); // AND E
    a.jp_z("a_store");
    a.ld_from(A_PRESSES);
    a.inc_a();
    a.st(A_PRESSES);
    a.label("a_store");
    a.raw(&[0x7B]); // LD A,E
    a.st(A_PREV);
    a.jp("loop");

    let code = a.finish();
    rom[0x0150..0x0150 + code.len()].copy_from_slice(&code);
    rom
}

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
        && halted == Some(gameboy_plugin_agent::governor::Halt::Calls)
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
            Poll::Failed(gameboy_plugin_agent::driver::DriverError::Crashed(
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
    let ok = halted == Some(gameboy_plugin_agent::governor::Halt::Broken)
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
    gb.queue_press(gameboy::KeypadKey::Right, 16, 14);
    gb.tap(gameboy::KeypadKey::A);
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

fn main() {
    let cases = vec![
        case_the_fixture_is_not_inert(),
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
