//! **The synthetic Gen 1-shaped cartridge**, shared by every harness that needs
//! a game to drive without needing a game.
//!
//! `#[path]`-included by `testharness/agent.rs` and `testharness/scenario.rs`
//! rather than copied, for the same reason `examples/support/suite_baseline.rs`
//! is shared: two copies of a fixture eventually become two different fixtures,
//! and then the numbers the two harnesses produce stop being comparable.
//!
//! It hand-assembles a 32 KiB ROM presenting **the RAM layout Pokemon Red/Blue
//! does** -- player name at `$D158`, party count at `$D163`, map and coordinates
//! at `$D35E`/`$D361`/`$D362`, facing at `$C109`, one warp at `$D3AE` -- and
//! walks the player around a walled 8x8 room in response to the d-pad. It
//! contains **no game data whatsoever**: it is twenty instructions of our own,
//! and its header says `POKEMON BLUE` purely so the Gen 1 observer engages. See
//! the licence-boundary rule in `AGENTS.md` -- formats and addresses are facts,
//! assets are not ours.
//!
//! That is what makes the harnesses runnable in CI. A test that needs the
//! captain's cartridge is a test CI skips, and a skipped test proves nothing.
#![allow(dead_code)]

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// A tiny absolute-jump assembler, so the program below can be read
// ---------------------------------------------------------------------------

pub struct Asm {
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
pub const W_PLAYER_NAME: u16 = 0xD158;
pub const W_PARTY_COUNT: u16 = 0xD163;
pub const W_CUR_MAP: u16 = 0xD35E;
pub const W_Y: u16 = 0xD361;
pub const W_X: u16 = 0xD362;
pub const W_FACING: u16 = 0xC109;
pub const W_WARPS: u16 = 0xD3AE;
/// `wIsInBattle`, `wObtainedBadges` and `wPlayerMoney`. The fixture zeroes
/// them for the same reason the real game does: WRAM boots full of noise, and
/// an observer that reads it reports a level-201 opponent and four badges.
/// (It did, on the first run of this harness.)
pub const W_IN_BATTLE: u16 = 0xD057;
pub const W_BADGES: u16 = 0xD356;
pub const W_MONEY: u16 = 0xD347;
/// Where the fixture counts A presses, so a test can see them land.
pub const A_PRESSES: u16 = 0xD000;
pub const A_PREV: u16 = 0xD001;

pub const START_X: u8 = 5;
pub const START_Y: u8 = 5;
pub const MIN_XY: u8 = 1;
pub const MAX_XY: u8 = 8;
/// A wall down column `x = WALL_X`, with a single doorway at `y = DOOR_Y`.
///
/// The room needs an obstacle a *straight line* cannot get round, because that
/// is the shape `goto` was bad at: single-step axis alternation walks one tile
/// aside, finds the direct route still blocked, walks back, and repeats. A bed
/// it can manage; a building it cannot. This is the building.
pub const WALL_X: u8 = 2;
pub const DOOR_Y: u8 = 8;

/// **The working door**, and where it leads. Reaching this tile changes
/// `wCurMap`, so "leave the room" is a real goal a scenario can be written
/// against and a map graph is something a run can actually *learn* here.
///
/// It is a second warp beside the staircase at (7,1), rather than the
/// staircase itself, so that the harnesses which walk to the staircase and
/// then press A keep measuring exactly what they always did.
pub const WARP_X: u8 = 8;
pub const WARP_Y: u8 = 8;
/// Red's house 1F — the map the warp table already advertises.
pub const WARP_TO: u8 = 0x25;
pub const START_MAP: u8 = 0x26;

/// A 32 KiB cartridge that walks a player round a walled room.
pub fn build_fake_gen1() -> Vec<u8> {
    build_fake_gen1_with(&[])
}

/// Gen 1 character code for the few glyphs the fixtures draw. The game loads
/// its font so a tile id *is* a character code; these are the same values
/// `plugins/gen1-link/src/text.rs` decodes back.
pub fn charcode(c: u8) -> u8 {
    match c {
        b'A'..=b'Z' => 0x80 + (c - b'A'),
        b'a'..=b'z' => 0xA0 + (c - b'a'),
        b'?' => 0xE6,
        // `>` stands in for the `▶` menu cursor ($ED) so a fixture row can put
        // a real cursor on screen; `~` for the `▼` "press A" arrow ($EE).
        b'>' => 0xED,
        b'~' => 0xEE,
        _ => 0x7F, // space
    }
}

/// The same cartridge, with extra text rows drawn into the background tilemap
/// at boot — enough to put any screen the observer must recognise (the naming
/// keyboard, say) in front of a real `Screen::read`.
pub fn build_fake_gen1_with(extra_rows: &[(u16, &str)]) -> Vec<u8> {
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
    a.ld_a(START_MAP); // Red's house 2F, so the map name resolves
    a.st(W_CUR_MAP);
    a.ld_a(START_Y);
    a.st(W_Y);
    a.ld_a(START_X);
    a.st(W_X);
    // One exit at (7,1) leading to map $25, in the warp-table layout the
    // observer reads: count, then y, x, dest-warp, dest-map.
    a.ld_a(1);
    a.st(W_WARPS);
    a.st(W_WARPS + 1); // y = WARP_Y, which is 1
    a.ld_a(WARP_X);
    a.st(W_WARPS + 2);
    a.xor_a();
    a.st(W_WARPS + 3);
    a.ld_a(WARP_TO);
    a.st(W_WARPS + 4);
    // The second entry: the working door. Same four-byte layout — y, x,
    // destination warp id, destination map.
    a.ld_a(2);
    a.st(W_WARPS); // two warps on this map
    a.ld_a(WARP_Y);
    a.st(W_WARPS + 5);
    a.ld_a(WARP_X);
    a.st(W_WARPS + 6);
    a.xor_a();
    a.st(W_WARPS + 7);
    a.ld_a(WARP_TO);
    a.st(W_WARPS + 8);

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
    // Extra rows a case asked for, drawn as character codes like the ROOM
    // text above. Row-major on the visible 20x18 grid (SCX/SCY are 0 here).
    for (row, text) in extra_rows {
        a.ld_hl(0x9800 + row * 32);
        for &c in text.as_bytes() {
            a.ld_a(charcode(c));
            a.st_hli();
        }
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

    // The doorway. Standing on the warp tile at (WARP_X, WARP_Y) puts the
    // player on `WARP_TO`, exactly as a Gen 1 warp does when you step *through*
    // a door rather than press A at it. One way, and the coordinates are left
    // alone: what a harness needs from this is a map id that changes because
    // somebody walked somewhere, which is what makes a learned map graph
    // testable without a commercial cartridge.
    a.ld_from(W_X);
    a.cp_n(WARP_X);
    a.jp_nz("no_warp");
    a.ld_from(W_Y);
    a.cp_n(WARP_Y);
    a.jp_nz("no_warp");
    a.ld_a(WARP_TO);
    a.st(W_CUR_MAP);
    a.label("no_warp");

    a.jp("loop");

    let code = a.finish();
    rom[0x0150..0x0150 + code.len()].copy_from_slice(&code);
    rom
}
