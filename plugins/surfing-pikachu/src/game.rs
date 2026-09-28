//! The minigame: state machine, wave physics, scoring and the run's end.
//!
//! A straight port of `SurfingPikachuLoop` and everything it dispatches to in
//! pret/pokeyellow `engine/pikachu/surfing_pikachu.asm`. Nothing here draws;
//! [`crate::render`] turns this state into the 160x144 picture the original
//! would have produced, including the per-scanline scroll effect.
//!
//! ## What the original actually does, since it is easy to mis-summarise
//!
//! - **There is no accelerator button.** `SurfingMinigame_SpeedUpPikachu` is
//!   called unconditionally from the riding state, every frame, and the only
//!   things that ever slow you down are landings and wipeouts.
//! - **There is no jump button.** `SurfingMinigame_TryStartJump` fires on its
//!   own when you cross a wave crest at 1.25 px/frame or better. What you
//!   choose is how fast you arrive.
//! - **Left and Right are the whole control scheme in play**, and only while
//!   airborne: they spin the board, and a completed spin is a trick.
//! - **HP is a clock, not a health bar.** `SurfingMinigame_Deduct1HP` runs
//!   once per frame in the running state, unconditionally. A crash costs you
//!   the 96 frames of its animation and resets your speed to a crawl; that is
//!   the damage. Reach section 24 before HP hits zero and you win.
//!
//! The only *deliberate* deviation is the source of the wave chooser's random
//! byte — see [`crate::rng`].

use crate::bcd::{self, Bcd16};
use crate::fixed::{self, Fx};
use crate::rng::Rng;
use crate::wave::{self, tile, FLAT_WATER_Y};

/// `SCREEN_WIDTH_PX / 2 + OAM_X_OFS` — Pikachu never moves horizontally.
pub const CENTER_X: u8 = 160 / 2 + 8;
/// The course is 24 sections long (`cp $18` in `SurfingMinigame_RunGame`).
pub const COURSE_SECTIONS: u8 = 0x18;
/// Starting HP, `$6000` in little-endian BCD — 6000 frames, or 100 seconds.
pub const START_HP: Bcd16 = [0x00, 0x60];
/// Frames a wipeout costs before you are riding again.
const CRASH_FRAMES: u8 = 0x60;
/// Frames the run coasts past the goal before the results transition.
const COAST_FRAMES: u8 = 192;
/// Frames the game-over screen ignores A for.
const GAME_OVER_DELAY: u8 = 0x80;
/// Minimum `speed / 32` that will launch a jump off a crest.
const JUMP_SPEED_THRESHOLD: u8 = 0x0a;
/// `hJoy5` is refreshed when `hFrameCounter` reaches zero, and the counter is
/// reloaded with 2 and decremented once per frame — so held input reaches the
/// trick code every other frame, not every frame. This is the one timing the
/// disassembly's own comment disagrees with (it says three frames); the
/// reload value and a single VBlank decrement give two, and two is also what
/// makes the scoring table's "3 or more flips" reachable inside one jump.
const JOY_SAMPLE_RELOAD: u8 = 2;

pub mod pad {
    pub const A: u8 = 1 << 0;
    pub const B: u8 = 1 << 1;
    pub const SELECT: u8 = 1 << 2;
    pub const START: u8 = 1 << 3;
    pub const RIGHT: u8 = 1 << 4;
    pub const LEFT: u8 = 1 << 5;
    pub const UP: u8 = 1 << 6;
    pub const DOWN: u8 = 1 << 7;
}

/// `wSurfingMinigamePikachuState`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PikachuState {
    #[default]
    Riding,
    Jumping,
    Landing,
    Crashed,
    GameEnd,
    InitResults,
    Results,
}

/// What a landing did to you, from `SurfingMinigame_TileInteraction`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Landing {
    /// No speed lost.
    Clean,
    /// `ReduceSpeedBy64` — a quarter of a pixel per frame.
    Rough,
    /// `ReduceSpeedBy128` — half a pixel per frame.
    Hard,
    /// Speed back to the starting crawl, and 96 frames in the water.
    Wipeout,
}

/// Whether the run is still going.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Running,
    /// The player pressed Select, or dismissed the results screen.
    Finished,
}

/// A frame of held input. The original reads the d-pad as a *held* level and
/// A/Select as edges, so both are needed.
#[derive(Clone, Copy, Default, Debug)]
pub struct Input {
    pub left: bool,
    pub right: bool,
    pub a: bool,
    pub select: bool,
}

impl Input {
    fn mask(self) -> u8 {
        (if self.left { pad::LEFT } else { 0 })
            | (if self.right { pad::RIGHT } else { 0 })
            | (if self.a { pad::A } else { 0 })
            | (if self.select { pad::SELECT } else { 0 })
    }
}

/// A short-lived visual: spray, a splash, a score pop, a banner.
///
/// The original drives these through a general animated-object VM shared with
/// the rest of the game. None of them feeds back into the simulation, so the
/// port keeps the *timings* from `SurfingPikachuFrames` and drops the VM.
#[derive(Clone, Copy, Debug)]
pub struct Popup {
    pub kind: PopupKind,
    pub x: u8,
    pub y: u8,
    /// Frames remaining.
    pub life: u8,
    /// Frames elapsed, for multi-phase visuals.
    pub age: u8,
    /// Spare byte: the decaying amplitude of the "Oh no.." bounce.
    pub aux: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PopupKind {
    /// `.WaterSpray` — one frame, every fourth frame of riding.
    Spray,
    /// The "START" banner, sliding in from the right edge at 4 px/frame.
    Start,
    /// "Oh no.." on the game-over screen, bobbing through `Sine`.
    OhNo,
    /// The wipeout splash, for the length of the crash timer.
    Splash,
    /// `+50` .. `+500`, 19 frames.
    Score(u16),
}

/// Pikachu's animated-object fields. One byte does three jobs in the
/// original — `ANIM_OBJ_FIELD_C` is the jump's Y fraction, the landing
/// splash's timer and the results bob's phase — and keeping it as one byte
/// keeps the wrap-around behaviour that goes with it.
#[derive(Clone, Copy, Debug)]
pub struct Pikachu {
    /// `ANIM_OBJ_FRAME_SET`: 1..=14, the board angle. 8..=14 are the upside
    /// down halves of 1..=7, which is why landing on one is always a wipeout.
    pub frame_set: u8,
    /// `ANIM_OBJ_Y_COORD`, an OAM Y (screen Y + 16).
    pub y: u8,
    /// `ANIM_OBJ_Y_OFFSET`, a signed byte added at draw time.
    pub y_offset: u8,
    /// `ANIM_OBJ_FIELD_C`.
    pub field_c: u8,
    /// `ANIM_OBJ_FIELD_D` — frames of Left held since the last trick.
    pub field_d: u8,
    /// `ANIM_OBJ_FIELD_E` — frames of Right held since the last trick.
    pub field_e: u8,
}

/// `SurfingMinigame_LYOverridesInitialSineWave` — amplitude 2, 32 entries,
/// repeated down the screen and scrolled up one scanline per frame.
#[rustfmt::skip]
const LY_SINE: [u8; 32] = [
    0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1, 0, 0,
    0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xfe, 0xfe, 0xfe, 0xff, 0xff, 0xff, 0xff, 0, 0,
];

/// `sine_table 32`: `sin(pi * i / 32)` in 8.8, which is what
/// `SurfingPikachu_Sine`'s shift-add multiply expects.
static SINE: [u16; 32] = {
    // Computed rather than transcribed: the RGBDS macro is
    // `dw (sin(x * 32768 / 32) >> 8) & $ffff`, and RGBDS's `sin` takes turns
    // in 16.16, so entry i is floor(sin(2*pi*i/64) * 65536) >> 8.
    let mut t = [0u16; 32];
    // A const-evaluable table has to be written out; these are the exact
    // values that formula produces.
    let v: [u16; 32] = [
        0, 25, 50, 74, 98, 121, 142, 162, 181, 198, 213, 226, 237, 245, 251, 255, 256,
        255, 251, 245, 237, 226, 213, 198, 181, 162, 142, 121, 98, 74, 50, 25,
    ];
    let mut i = 0;
    while i < 32 {
        t[i] = v[i];
        i += 1;
    }
    t
};

/// `SurfingPikachu_Sine` — a 64-step cycle whose second half is the first
/// negated, scaled by `amplitude`, returned as a two's-complement byte.
pub fn sine(angle: u8, amplitude: u8) -> u8 {
    let a = angle & 0x3f;
    let mag = ((amplitude as u32 * SINE[(a & 0x1f) as usize] as u32) >> 8) as u8;
    if a < 0x20 {
        mag
    } else {
        (!mag).wrapping_add(1)
    }
}

/// The result screen's strings, as tile ids. Read straight off `.HP_Left`,
/// `.Radness`, `.Total` and `.Hi_Score` — the font is not ASCII, so these are
/// the actual bytes the original copies into the tilemap.
mod text {
    /// "HP Left" — `$ff` is the blank tile.
    pub const HP_LEFT: [u8; 7] = [0x20, 0x21, 0xff, 0x22, 0x23, 0x24, 0x25];
    /// "Radness"
    pub const RADNESS: [u8; 7] = [0x27, 0x28, 0x29, 0x2a, 0x23, 0x26, 0x26];
    /// "Total"
    pub const TOTAL: [u8; 5] = [0x2b, 0x2c, 0x25, 0x28, 0x2d];
    /// "Hi-Score!!" — the last tile carries both exclamation marks.
    pub const HI_SCORE: [u8; 9] = [0x20, 0x2e, 0x2f, 0x30, 0x31, 0x2c, 0x32, 0x23, 0x33];
    /// "Pts"
    pub const PTS: [u8; 3] = [0x21, 0x25, 0x26];
}

const ROUTINE_RUN_GAME: u8 = 1;
const ROUTINE_GAME_OVER: u8 = 0x0c;
const ROUTINE_EXIT: u8 = 0x80;

/// Tile columns of surface height the game keeps, one screen wide.
const WAVE_HEIGHT_LEN: usize = 20;

/// The whole machine.
pub struct Game {
    rng: Rng,
    /// `wSurfingMinigameRoutineNumber`; bit 7 means "leave the minigame".
    routine: u8,
    routine_delay: u8,
    pub(crate) state: PikachuState,
    wave_fn: u8,
    wave_random: u8,
    pub(crate) hp: Bcd16,
    pub(crate) radness_meter: u8,
    pub(crate) radness: Bcd16,
    pub(crate) total: Bcd16,
    pub(crate) hi_score: Bcd16,
    pub(crate) got_high_score: bool,
    board_angle: u8,
    board_angle_decreasing: bool,
    board_angle_timer: u8,
    crash_timer: u8,
    pub(crate) speed: Fx,
    /// `wSurfingMinigameDistance`, big-endian: `[0]` is the section counter
    /// the goal is measured in, `[1..3]` the 16-bit accumulator under it.
    pub(crate) distance: [u8; 3],
    /// The tile under Pikachu and the one to its right, as of last frame —
    /// `SurfingMinigame_ReadBGMapBuffer` schedules a VBlank copy, so every
    /// read of it is one frame stale and the game is built around that.
    bg_read: [u8; 2],
    bg_read_pending: [u8; 2],
    pub(crate) pikachu_height: u8,
    spray_counter: u8,
    jump_magnitude: u8,
    jump_descending: bool,
    jump_fraction: u8,
    pub(crate) scx: u8,
    scx_frac: u8,
    scx_seen: u8,
    scx_block: u8,
    x_offset: u8,
    pub(crate) wave_height: [u8; WAVE_HEIGHT_LEN],
    trick_flags: u8,
    game_over: bool,
    game_over_delay: u8,
    cloud_fraction: u8,
    joy_prev: u8,
    joy_pressed: u8,
    joy5: u8,
    joy_frame_counter: u8,
    pub(crate) pika: Pikachu,
    pub(crate) bgmap: [u8; 32 * 32],
    /// Per-scanline SCY. `hLCDCPointer` is set to `rSCY`, so the raster split
    /// moves the water up and down, not side to side.
    pub(crate) ly: [u8; 145],
    pub(crate) ly_enabled: bool,
    pub(crate) popups: Vec<Popup>,
    pub(crate) clouds: [u8; 9],
    pub(crate) progress_marker_x: u8,
    /// Set once the results screen has replaced the scrolling course.
    pub(crate) results_map: Option<Box<[u8; 20 * 18]>>,
    pub(crate) frame: u64,
}

impl Game {
    /// A fresh run. `seed` fixes the course; the same seed plays the same
    /// waves in the same order.
    pub fn new(seed: u32) -> Self {
        let mut g = Game {
            rng: Rng::new(seed),
            routine: 0,
            routine_delay: 0,
            state: PikachuState::Riding,
            wave_fn: 0,
            wave_random: 0,
            hp: START_HP,
            radness_meter: 0,
            radness: [0, 0],
            total: [0, 0],
            hi_score: [0, 0],
            got_high_score: false,
            board_angle: 0,
            board_angle_decreasing: false,
            board_angle_timer: 0,
            crash_timer: 0,
            speed: fixed::START_SPEED,
            distance: [0; 3],
            bg_read: [0; 2],
            bg_read_pending: [0; 2],
            pikachu_height: FLAT_WATER_Y,
            spray_counter: 0,
            jump_magnitude: 0,
            jump_descending: false,
            jump_fraction: 0,
            scx: 0,
            scx_frac: 0,
            scx_seen: 0,
            scx_block: 0,
            x_offset: 0,
            wave_height: [FLAT_WATER_Y; WAVE_HEIGHT_LEN],
            trick_flags: 0,
            game_over: false,
            game_over_delay: 0,
            cloud_fraction: 0,
            joy_prev: 0,
            joy_pressed: 0,
            joy5: 0,
            joy_frame_counter: 0,
            pika: Pikachu {
                frame_set: 4,
                y: FLAT_WATER_Y,
                y_offset: 0,
                field_c: 0,
                field_d: 0,
                field_e: 0,
            },
            bgmap: [0; 32 * 32],
            ly: [0; 145],
            ly_enabled: true,
            popups: Vec::new(),
            clouds: [0x20, 0x28, 0x30, 0x38, 0x40, 0x80, 0x88, 0x90, 0x98],
            progress_marker_x: 0x50,
            results_map: None,
            frame: 0,
        };
        // `SurfingPikachuMinigame_LoadGFXAndLayout`: open water from row 6
        // down, everything above it sky.
        for row in 6..18 {
            for col in 0..32 {
                g.bgmap[row * 32 + col] = 0x0b;
            }
        }
        // `SurfingMinigame_InitScanlineOverrides`. Index 144 is the scratch
        // slot the rotation parks a value in and is never displayed.
        for i in 0..144 {
            g.ly[i] = LY_SINE[i & 31];
        }
        g
    }

    /// Carry the previous run's high score into a new one.
    pub fn with_high_score(mut self, hi: Bcd16) -> Self {
        self.hi_score = hi;
        self
    }

    /// One frame of `SurfingPikachuLoop`.
    pub fn step(&mut self, input: Input) -> Status {
        let held = input.mask();
        self.joy_pressed = held & !self.joy_prev;
        self.joy_prev = held;

        // `SurfingPikachu_GetJoypad_3FrameBuffer`.
        if self.joy_frame_counter == 0 {
            self.joy5 = held;
            self.joy_frame_counter = JOY_SAMPLE_RELOAD;
        } else {
            self.joy5 = 0;
        }

        // `SurfingPikachu_CheckPressedSelect` — the original gates this on a
        // map flag; standalone there is nothing to go back to but the menu.
        if self.joy_pressed & pad::SELECT != 0 {
            return Status::Finished;
        }

        self.run_routine();
        self.update_pikachu_object();
        self.tick_popups();
        self.move_clouds();

        // VBlank: the frame counter ticks down and the tile sample that
        // `ReadBGMapBuffer` scheduled lands.
        self.joy_frame_counter = self.joy_frame_counter.saturating_sub(1);
        self.bg_read = self.bg_read_pending;
        self.frame += 1;

        if self.routine & ROUTINE_EXIT != 0 {
            Status::Finished
        } else {
            Status::Running
        }
    }

    // ---- the routine jump table -------------------------------------------

    fn run_routine(&mut self) {
        match self.routine {
            0 => {
                // `SurfingMinigame_StartGame`
                self.popups.push(Popup {
                    kind: PopupKind::Start,
                    x: 0xe0,
                    y: 0x48,
                    life: 64,
                    age: 0,
                    aux: 0,
                });
                self.routine += 1;
            }
            ROUTINE_RUN_GAME => self.run_game(),
            2 => self.wait_to_show_results(),
            3 => self.scroll_to_results_screen(),
            4 => {
                self.draw_results_screen();
                self.routine_delay = 32;
                self.routine += 1;
            }
            5 => {
                // `SurfingMinigame_WriteHPLeftAndWait`. Writing the number
                // into the map is also what freezes it: the tally below
                // drains the live counter and never redraws this line.
                if self.run_delay_timer() {
                    self.write_tiles(2, 2, &text::HP_LEFT);
                    self.print_bcd16(10, 2, self.hp);
                    self.write_tiles(15, 2, &text::PTS);
                    self.routine_delay = 64;
                    self.routine += 1;
                }
            }
            6 => {
                // `SurfingMinigame_WriteRadnessAndWait`
                if self.run_delay_timer() {
                    self.write_tiles(2, 4, &text::RADNESS);
                    self.print_bcd16(10, 4, self.radness);
                    self.write_tiles(15, 4, &text::PTS);
                    self.routine_delay = 64;
                    self.routine += 1;
                }
            }
            7 => {
                // `SurfingMinigame_WriteTotal` reprints radness as well.
                if self.run_delay_timer() {
                    self.write_tiles(2, 6, &text::TOTAL);
                    self.print_bcd16(10, 4, self.radness);
                    self.write_tiles(15, 4, &text::PTS);
                    self.print_bcd16(10, 6, self.total);
                    self.write_tiles(15, 6, &text::PTS);
                    self.routine_delay = 64;
                    self.routine += 1;
                }
            }
            8 => {
                // The tally: 99 points a frame, and the total is reprinted
                // every frame whether or not the batch finished it.
                if self.run_delay_timer() {
                    let done = self.add_remaining_hp_to_total();
                    self.print_bcd16(10, 6, self.total);
                    if done {
                        self.routine_delay = 64;
                        self.routine += 1;
                    }
                }
            }
            9 => {
                if self.run_delay_timer() {
                    let done = self.add_radness_to_total();
                    self.print_bcd16(10, 6, self.total);
                    if done {
                        self.routine_delay = 128;
                        self.routine += 1;
                        if self.did_player_get_a_high_score() {
                            self.got_high_score = true;
                            self.write_tiles(6, 8, &text::HI_SCORE);
                            self.state = PikachuState::Results;
                        }
                    }
                }
            }
            0x0a => {
                if self.run_delay_timer() {
                    self.routine += 1;
                }
            }
            0x0b => {
                // `SurfingMinigame_ExitOnPressA`
                self.update_ly_overrides();
                if self.joy_pressed & pad::A != 0 {
                    self.routine |= ROUTINE_EXIT;
                }
            }
            ROUTINE_GAME_OVER => self.game_over_routine(),
            _ => {}
        }
    }

    /// `SurfingMinigame_RunDelayTimer` — returns the carry flag, i.e. "the
    /// delay had already run out".
    fn run_delay_timer(&mut self) -> bool {
        if self.routine_delay == 0 {
            true
        } else {
            self.routine_delay -= 1;
            false
        }
    }

    fn run_game(&mut self) {
        if self.distance[0] >= COURSE_SECTIONS {
            self.routine += 1;
            self.routine_delay = COAST_FRAMES;
            return;
        }
        if self.hp == [0, 0] {
            self.game_over = true;
            self.routine = ROUTINE_GAME_OVER;
            self.game_over_delay = GAME_OVER_DELAY;
            self.popups.push(Popup {
                kind: PopupKind::OhNo,
                x: CENTER_X,
                y: 0x80,
                life: 0,
                age: 0x30,
                aux: 0x80,
            });
            return;
        }
        self.wave_random = self.rng.next_u8();
        self.update_ly_overrides();
        self.set_pikachu_height();
        self.read_bg_map_buffer();
        self.scroll_and_generate_bg_map(fixed::SCROLL_PER_FRAME);
        self.update_pikachu_distance();
        self.deduct_1hp();
    }

    fn wait_to_show_results(&mut self) {
        if self.run_delay_timer() {
            self.routine += 1;
            self.scx = 0x90;
            self.wave_fn = wave::RUN_OUT;
            self.state = PikachuState::GameEnd;
            self.ly_enabled = false;
            self.scx_frac = 0;
            self.scx_seen = 0;
            self.scx_block = 0;
            return;
        }
        self.wave_random = 0;
        self.update_ly_overrides();
        self.set_pikachu_height();
        self.read_bg_map_buffer();
        // `SurfingMinigame_CoastAfterGoal`
        self.x_offset = 0xa0;
        self.scroll_and_generate_bg_map(fixed::COAST_PER_FRAME);
    }

    fn scroll_to_results_screen(&mut self) {
        if self.scx == 0 {
            self.speed = 0;
            self.routine += 1;
            self.state = PikachuState::InitResults;
            return;
        }
        self.update_ly_overrides();
        self.set_pikachu_height();
        self.read_bg_map_buffer();
        self.scx = self.scx.wrapping_sub(4);
        // TILEMAP_WIDTH_PX - 32: generate *behind* the viewport as it rewinds.
        self.x_offset = 0xe0;
        self.generate_bg_map();
    }

    fn game_over_routine(&mut self) {
        self.update_ly_overrides();
        self.set_pikachu_height();
        self.read_bg_map_buffer();
        self.scroll_and_generate_bg_map(fixed::SCROLL_PER_FRAME);
        if self.game_over_delay != 0 {
            self.game_over_delay -= 1;
            return;
        }
        if self.joy_pressed & pad::A != 0 {
            self.routine |= ROUTINE_EXIT;
        }
    }

    // ---- the course -------------------------------------------------------

    fn update_ly_overrides(&mut self) {
        // Rows 0..15 hold still — they are sky — and the rest walks up one
        // scanline per frame, the bottom wrapping back in at the top of the
        // moving region.
        const TOP: usize = 16;
        let saved = self.ly[TOP];
        self.ly.copy_within(TOP + 1..=144, TOP);
        self.ly[144] = saved;
    }

    fn set_pikachu_height(&mut self) {
        let idx = if self.scx & 8 != 0 { 8 } else { 7 };
        let h = self.wave_height[idx];
        let within_tile = self.scx & 7;
        self.pikachu_height = match self.bg_read[0] {
            tile::RISING_SLOPE | tile::WAVE_CREST => h.wrapping_sub(within_tile),
            tile::FALLING_SLOPE => h.wrapping_add(within_tile),
            _ => h,
        };
    }

    /// Sample the tile under Pikachu, nine tiles into the viewport and
    /// `pikachu_height / 8` rows down. The copy is deferred to VBlank in the
    /// original, so the value does not become visible until the next frame.
    fn read_bg_map_buffer(&mut self) {
        let col = self.scx.wrapping_add(9 * 8) >> 3;
        let row = (self.pikachu_height >> 3) as usize;
        let p = (col as usize + row * 32) & (self.bgmap.len() - 1);
        self.bg_read_pending =
            [self.bgmap[p], self.bgmap[(p + 1) & (self.bgmap.len() - 1)]];
    }

    fn scroll_and_generate_bg_map(&mut self, rate: Fx) {
        self.x_offset = 0xa0;
        let v = fixed::from_parts(self.scx, self.scx_frac).wrapping_add(rate);
        self.scx_frac = fixed::frac(v);
        self.scx = fixed::whole(v);
        self.generate_bg_map();
    }

    /// One 16-pixel slice of course, when the scroll has crossed into a new
    /// two-tile block.
    fn generate_bg_map(&mut self) {
        if self.scx == self.scx_seen {
            return;
        }
        self.scx_seen = self.scx;
        let block = self.scx & 0xf0;
        if block == self.scx_block {
            return;
        }
        self.scx_block = block;

        let slice =
            wave::next_slice(&mut self.wave_fn, self.distance[0], self.wave_random);
        self.wave_height.copy_within(2..WAVE_HEIGHT_LEN, 0);
        self.wave_height[WAVE_HEIGHT_LEN - 2] = slice.left;
        self.wave_height[WAVE_HEIGHT_LEN - 1] = slice.right;

        let col = (self.scx.wrapping_add(self.x_offset) & 0xf0) >> 3;
        for (i, m) in slice.metatiles.iter().enumerate() {
            let t = wave::METATILES[*m as usize];
            let base = i * 64 + col as usize;
            self.bgmap[base] = t[0];
            self.bgmap[base + 1] = t[1];
            self.bgmap[base + 32] = t[2];
            self.bgmap[base + 33] = t[3];
        }
    }

    fn update_pikachu_distance(&mut self) {
        let acc = u16::from_be_bytes([self.distance[1], self.distance[2]]);
        let (next, carry) = acc.overflowing_add(self.speed);
        self.distance[1] = (next >> 8) as u8;
        self.distance[2] = next as u8;
        if carry {
            self.distance[0] = self.distance[0].wrapping_add(1);
            self.progress_marker_x = self.progress_marker_x.wrapping_sub(2);
        }
    }

    /// `SurfingMinigame_Deduct1HP`. A byte that is already `$00` rolls over to
    /// `$99` and borrows, rather than going through `daa` — which is also why
    /// HP at exactly zero would wrap to 9999 if the caller did not check
    /// first. It always does.
    fn deduct_1hp(&mut self) {
        if self.hp[0] != 0 {
            self.hp[0] = bcd::sub(self.hp[0], 1, false).0;
            return;
        }
        self.hp[0] = 0x99;
        if self.hp[1] == 0 {
            self.hp[1] = 0x99;
        } else {
            self.hp[1] = bcd::sub(self.hp[1], 1, false).0;
        }
    }

    // ---- Pikachu ----------------------------------------------------------

    fn update_pikachu_object(&mut self) {
        match self.state {
            PikachuState::Riding => self.update_riding(),
            PikachuState::Jumping => self.update_jumping(),
            PikachuState::Landing => self.update_landing(),
            PikachuState::Crashed => self.update_crashed(),
            PikachuState::GameEnd => {
                self.pika.y = self.pikachu_height;
                self.update_surfing_frame();
            }
            PikachuState::InitResults => {
                // Deliberately no state change: `SurfingMinigame_InitResults\
                // Pikachu` re-runs every frame, and the only thing that ever
                // moves Pikachu on to the bobbing `Results` state is beating
                // the high score in routine 9.
                self.pika.frame_set = 0x0f;
                self.pika.field_c = 0;
            }
            PikachuState::Results => self.update_results(),
        }
    }

    fn update_riding(&mut self) {
        if self.game_over {
            self.speed = 0;
            self.state = PikachuState::GameEnd;
            self.update_surfing_frame();
            return;
        }
        self.spawn_water_spray();
        self.pika.y = self.pikachu_height;
        if self.try_start_jump() {
            self.update_surfing_frame();
            self.state = PikachuState::Jumping;
            self.pika.field_c = 0;
            self.pika.field_d = 0;
            self.pika.field_e = 0;
            self.radness_meter = 0;
            self.trick_flags = 0;
        } else {
            self.update_surfing_frame();
            self.speed_up_pikachu();
        }
    }

    fn update_jumping(&mut self) {
        self.dpad_action();
        if !self.update_pikachu_height() {
            return;
        }
        if self.tile_interaction() == Landing::Wipeout {
            self.state = PikachuState::Crashed;
            self.crash_timer = CRASH_FRAMES;
            self.pika.frame_set = 0x10;
            self.popups.push(Popup {
                kind: PopupKind::Splash,
                x: CENTER_X,
                y: self.pika.y,
                life: CRASH_FRAMES,
                age: 0,
                aux: 0,
            });
        } else {
            self.calculate_and_add_radness_from_stunt();
            self.pika.field_c = 0;
            self.state = PikachuState::Landing;
        }
    }

    fn update_landing(&mut self) {
        let phase = self.pika.field_c;
        if phase >= 0x20 {
            self.pika.y_offset = 0;
            self.state = PikachuState::Riding;
            return;
        }
        self.pika.field_c = phase.wrapping_add(4);
        self.pika.y_offset = sine(phase, 4);
        self.spawn_water_spray();
        self.pika.y = self.pikachu_height;
    }

    fn update_crashed(&mut self) {
        if self.crash_timer == 0 {
            self.state = PikachuState::Riding;
            self.pika.frame_set = 4;
            return;
        }
        self.crash_timer -= 1;
        self.pika.y = self.pikachu_height;
    }

    fn update_results(&mut self) {
        let phase = self.pika.field_c;
        self.pika.field_c = phase.wrapping_add(2);
        let v = phase & 0x3f;
        // Only the negative half of the cycle is applied, so Pikachu bobs up
        // from the waterline and never sinks below it.
        self.pika.y_offset = if v >= 0x20 { sine(v, 0x10) } else { 0 };
    }

    /// Left and Right spin the board. A full spin is a trick; two of the same
    /// are worth three times one, and mixing them is worth more still.
    fn dpad_action(&mut self) {
        if self.joy5 & pad::LEFT != 0 {
            self.pika.field_e = 0;
            let held = self.pika.field_d;
            self.pika.field_d = held.wrapping_add(1);
            if held >= 0x0b {
                self.start_trick();
                self.trick_flags |= 1;
            }
            self.pika.frame_set = if self.pika.frame_set >= 0x0e {
                1
            } else {
                self.pika.frame_set + 1
            };
        } else if self.joy5 & pad::RIGHT != 0 {
            self.pika.field_d = 0;
            let held = self.pika.field_e;
            self.pika.field_e = held.wrapping_add(1);
            if held >= 0x0d {
                self.start_trick();
                self.trick_flags |= 2;
            }
            self.pika.frame_set = if self.pika.frame_set == 1 {
                0x0e
            } else {
                self.pika.frame_set - 1
            };
        }
    }

    fn start_trick(&mut self) {
        self.radness_meter = (self.radness_meter + 1).min(3);
        self.pika.field_d = 0;
        self.pika.field_e = 0;
    }

    /// Jumps are not a button. Crossing a crest at the right sub-tile offset,
    /// fast enough, launches you; the jump's size is your speed over 32.
    fn try_start_jump(&mut self) -> bool {
        if !self.at_tile_transition() || self.bg_read[0] != tile::WAVE_CREST {
            return false;
        }
        let magnitude = self.speed_divided_by_32();
        if magnitude < JUMP_SPEED_THRESHOLD {
            return false;
        }
        self.jump_magnitude = magnitude;
        self.jump_descending = false;
        self.jump_fraction = 0;
        true
    }

    /// The board angle only ever changes as a tile passes under it.
    fn at_tile_transition(&self) -> bool {
        matches!(self.scx & 7, 3 | 4)
    }

    fn speed_divided_by_32(&self) -> u8 {
        fixed::whole(self.speed.wrapping_mul(8))
    }

    fn update_surfing_frame(&mut self) {
        if !self.at_tile_transition() {
            return;
        }
        let base: u8 = match self.bg_read[0] {
            tile::RISING_SLOPE | tile::WAVE_CREST => 6,
            tile::FALLING_SLOPE => 2,
            _ => {
                self.update_board_angle();
                self.pika.frame_set = 4;
                return;
            }
        };
        self.pika.frame_set = base.wrapping_add(self.board_angle.wrapping_sub(1));
    }

    /// The board rocks 0 -> 1 -> 2 -> 1 -> 0 every eight tile transitions,
    /// which is what makes flat water read as a wave rather than a floor.
    fn update_board_angle(&mut self) {
        let timer = self.board_angle_timer;
        self.board_angle_timer = timer.wrapping_add(1);
        if timer & 7 != 0 {
            return;
        }
        if self.board_angle_decreasing {
            if self.board_angle == 0 {
                self.board_angle_decreasing = false;
            } else {
                self.board_angle -= 1;
            }
        } else if self.board_angle == 2 {
            self.board_angle_decreasing = true;
        } else {
            self.board_angle += 1;
        }
    }

    fn speed_up_pikachu(&mut self) {
        if fixed::whole(self.speed) >= 2 {
            return;
        }
        self.speed = self.speed.wrapping_add(fixed::ACCEL);
    }

    fn reduce_speed(&mut self, step: Fx) {
        if fixed::whole(self.speed) == 0 && self.speed < step {
            self.speed = 0;
            return;
        }
        self.speed = self.speed.wrapping_sub(step);
    }

    /// What the board angle was worth against the tile you came down on.
    /// Frame sets 8..=14 are the upside-down halves and never survive.
    pub fn landing_for(tile_under: u8, frame_set: u8) -> Landing {
        use Landing::{Clean, Hard, Rough, Wipeout};
        match tile_under {
            tile::RISING_SLOPE => match frame_set {
                1..=3 => Wipeout,
                4 => Hard,
                5 => Rough,
                6 => Clean,
                7 => Rough,
                _ => Wipeout,
            },
            tile::WAVE_CREST | tile::WAVE_FACE => match frame_set {
                1 => Wipeout,
                2 => Hard,
                3 => Rough,
                4 | 5 => Clean,
                6 => Rough,
                7 => Hard,
                _ => Wipeout,
            },
            tile::FALLING_SLOPE => match frame_set {
                1 => Rough,
                2 => Clean,
                3 => Rough,
                4 => Hard,
                _ => Wipeout,
            },
            _ => match frame_set {
                1 => Wipeout,
                2 => Hard,
                3 => Rough,
                4 => Clean,
                5 => Rough,
                6 => Hard,
                _ => Wipeout,
            },
        }
    }

    fn tile_interaction(&mut self) -> Landing {
        let outcome = Self::landing_for(self.bg_read[0], self.pika.frame_set);
        match outcome {
            Landing::Hard => self.reduce_speed(fixed::SPEED_STEP_128),
            Landing::Rough => self.reduce_speed(fixed::SPEED_STEP_64),
            Landing::Clean => {}
            Landing::Wipeout => self.speed = fixed::START_SPEED,
        }
        outcome
    }

    /// `-(4 * a^2)`, negated the way the original does it: `l = -l`, `h = ~h`.
    /// That is the true 16-bit negation only when the low byte is non-zero,
    /// and `4 * a * a` is a multiple of 256 at a = 0, 8 and 16 — so on exactly
    /// those frames the ascent gets one extra pixel of lift. Reproduced
    /// because it is in the arc every player of the original ever saw.
    fn rise_step(magnitude: u8) -> u16 {
        let v = Self::arc_step(magnitude);
        let lo = (!(v as u8)).wrapping_add(1);
        let hi = !((v >> 8) as u8);
        ((hi as u16) << 8) | lo as u16
    }

    /// `4 * a^2`, in 1/256ths of a pixel.
    fn arc_step(magnitude: u8) -> u16 {
        let sq = (magnitude as u16).wrapping_mul(magnitude as u16);
        sq.wrapping_mul(4)
    }

    /// The jump arc. Returns true on the frame Pikachu touches down.
    fn update_pikachu_height(&mut self) -> bool {
        if !self.jump_descending {
            if self.jump_magnitude == 0 && self.jump_fraction == 0 {
                self.jump_descending = true;
                return false;
            }
            let v = fixed::from_parts(self.jump_magnitude, self.jump_fraction)
                .wrapping_sub(fixed::JUMP_GRAVITY);
            self.jump_fraction = fixed::frac(v);
            self.jump_magnitude = fixed::whole(v);
            self.apply_arc(Self::rise_step(self.jump_magnitude));
            return false;
        }
        // Descending. The `>= 144` arm keeps a jump that somehow ended up
        // below the screen falling rather than snapping to the surface.
        let surface = self.pikachu_height;
        if self.pika.y < 144 && self.pika.y >= surface {
            self.pika.y = surface;
            self.pika.field_c = 0;
            return true;
        }
        let v = fixed::from_parts(self.jump_magnitude, self.jump_fraction)
            .wrapping_add(fixed::JUMP_GRAVITY);
        self.jump_fraction = fixed::frac(v);
        self.jump_magnitude = fixed::whole(v);
        self.apply_arc(Self::arc_step(self.jump_magnitude));
        false
    }

    fn apply_arc(&mut self, delta: u16) {
        let y = fixed::from_parts(self.pika.y, self.pika.field_c).wrapping_add(delta);
        self.pika.y = fixed::whole(y);
        self.pika.field_c = fixed::frac(y);
    }

    /// The scoring table, straight from the comment in
    /// `SurfingMinigame_CalculateAndAddRadnessFromStunt`:
    /// one flip 50, two of the same 150, three or more of the same 350,
    /// two different 180, three or more different 500.
    pub fn stunt_value(radness_meter: u8, trick_flags: u8) -> u16 {
        if radness_meter == 0 {
            return 0;
        }
        if trick_flags & 3 == 3 {
            return if radness_meter < 3 { 180 } else { 500 };
        }
        // 1, 3, 7 lots of fifty: the original sums 1 + 2 + 4 + ... over the
        // meter rather than looking the answer up.
        let mut d = radness_meter;
        let mut e: u8 = 1;
        let mut n: u8 = 0;
        while d != 0 {
            n = n.wrapping_add(e);
            e = e.wrapping_shl(1);
            d -= 1;
        }
        n as u16 * 50
    }

    fn calculate_and_add_radness_from_stunt(&mut self) {
        let value = Self::stunt_value(self.radness_meter, self.trick_flags);
        if value == 0 {
            return;
        }
        // Added in lots of 50 (and one of 30 for the 180), which matters:
        // each add saturates independently at 9999.
        if value == 180 {
            for amount in [0x50u8, 0x50, 0x50, 0x30] {
                bcd::add_saturating(&mut self.radness, amount);
            }
        } else {
            for _ in 0..value / 50 {
                bcd::add_saturating(&mut self.radness, 0x50);
            }
        }
        self.popups.push(Popup {
            kind: PopupKind::Score(value),
            x: CENTER_X,
            y: self.pika.y.wrapping_sub(0x10),
            life: 19,
            age: 0,
            aux: 0,
        });
    }

    // ---- results ----------------------------------------------------------

    /// One batch of the HP tally: up to 99 points a frame. Returns true when
    /// there is nothing left to count.
    fn add_remaining_hp_to_total(&mut self) -> bool {
        for _ in 0..99 {
            if self.hp == [0, 0] {
                return true;
            }
            self.deduct_1hp();
            bcd::add_saturating(&mut self.total, 1);
        }
        false
    }

    fn add_radness_to_total(&mut self) -> bool {
        for _ in 0..99 {
            if !bcd::dec(&mut self.radness) {
                return true;
            }
            bcd::add_saturating(&mut self.total, 1);
        }
        false
    }

    fn did_player_get_a_high_score(&mut self) -> bool {
        let beat = (self.total[1], self.total[0]) > (self.hi_score[1], self.hi_score[0]);
        if beat {
            self.hi_score = self.total;
        }
        beat
    }

    fn draw_results_screen(&mut self) {
        let mut map = Box::new([0u8; 20 * 18]);
        // The original pastes `beach_outro.tilemap`, which was not part of the
        // extracted reference set; composed here from the beach metatiles the
        // course itself uses so the scene still reads as a shoreline.
        for row in 6..18 {
            for col in 0..20 {
                map[row * 20 + col] = match row {
                    6 => 0x0b,
                    7 => 0x0c,
                    _ => 0x0d,
                };
            }
        }
        // `.PlaceTextbox`
        for (row, (left, right, fill)) in [
            (1usize, (0x3b, 0x3c, 0x40)),
            (2, (0x3f, 0x3f, 0xff)),
            (3, (0x3f, 0x3f, 0xff)),
            (4, (0x3f, 0x3f, 0xff)),
            (5, (0x3f, 0x3f, 0xff)),
            (6, (0x3f, 0x3f, 0xff)),
            (7, (0x3f, 0x3f, 0xff)),
            (8, (0x3f, 0x3f, 0xff)),
            (9, (0x3d, 0x3e, 0x40)),
        ] {
            map[row * 20 + 1] = left;
            for col in 2..18 {
                map[row * 20 + col] = fill;
            }
            map[row * 20 + 18] = right;
        }
        self.results_map = Some(map);
        self.clouds = [0; 9];
    }

    /// Write tiles into the results screen, as `CopyData` does into
    /// `wTileMap`. A no-op before the screen exists.
    fn write_tiles(&mut self, col: usize, row: usize, tiles: &[u8]) {
        let Some(map) = self.results_map.as_mut() else {
            return;
        };
        for (i, t) in tiles.iter().enumerate() {
            if col + i < 20 && row < 18 {
                map[row * 20 + col + i] = *t;
            }
        }
    }

    /// `SurfingPikachu_PlaceBCDNumber` twice over: four digits, high pair
    /// first, with a gap before the "Pts" that follows.
    fn print_bcd16(&mut self, col: usize, row: usize, value: Bcd16) {
        let digits = [
            0xd0 + (value[1] >> 4),
            0xd0 + (value[1] & 0x0f),
            0xd0 + (value[0] >> 4),
            0xd0 + (value[0] & 0x0f),
        ];
        self.write_tiles(col, row, &digits);
    }

    // ---- incidentals ------------------------------------------------------

    fn spawn_water_spray(&mut self) {
        let counter = self.spray_counter;
        self.spray_counter = counter.wrapping_add(1);
        if counter & 3 != 0 {
            return;
        }
        let idx = if self.scx & 8 != 0 { 9 } else { 8 };
        let h = self.wave_height[idx];
        let within_tile = self.scx & 7;
        let y = match self.bg_read[1] {
            tile::RISING_SLOPE | tile::WAVE_CREST => h.wrapping_sub(within_tile),
            tile::FALLING_SLOPE => h.wrapping_add(within_tile),
            _ => h,
        };
        self.popups.push(Popup {
            kind: PopupKind::Spray,
            x: CENTER_X,
            y,
            life: 1,
            age: 0,
            aux: 0,
        });
    }

    fn tick_popups(&mut self) {
        for p in &mut self.popups {
            match p.kind {
                PopupKind::Start if p.x != CENTER_X => {
                    // `SurfingMinigame_MoveBannerToCenter`, four pixels a
                    // frame, wrapping off the right edge and back on the left.
                    p.x = p.x.wrapping_add(4);
                }
                PopupKind::OhNo => {
                    // `SurfingMinigameAnimatedObjectFn_FlippingPika`: a sine
                    // whose amplitude decays by two a frame, forced negative
                    // so the text only ever bounces upward.
                    if p.aux != 0 {
                        let amplitude = p.aux;
                        p.aux = p.aux.saturating_sub(2);
                        let v = sine(p.age, amplitude);
                        p.y = if v >= 0x80 { v } else { (!v).wrapping_add(1) };
                        p.age = p.age.wrapping_add(1);
                    }
                    continue;
                }
                _ => {}
            }
            p.age = p.age.saturating_add(1);
            p.life = p.life.saturating_sub(1);
        }
        self.popups
            .retain(|p| p.life > 0 || p.kind == PopupKind::OhNo);
    }

    fn move_clouds(&mut self) {
        let v = fixed::from_parts(0, self.cloud_fraction).wrapping_add(self.speed);
        self.cloud_fraction = fixed::frac(v);
        let step = fixed::whole(v);
        for c in &mut self.clouds {
            *c = c.wrapping_add(step);
        }
    }

    // ---- what a frontend wants to know -------------------------------------

    /// Frames of HP left. The original prints it as four BCD digits.
    pub fn hp_value(&self) -> u16 {
        bcd::value(&self.hp)
    }
    pub fn radness_value(&self) -> u16 {
        bcd::value(&self.radness)
    }
    /// Spins completed in the current jump, capped at three. The score for
    /// them is not added until the landing, so this is what to watch to see a
    /// spin register.
    pub fn radness_meter(&self) -> u8 {
        self.radness_meter
    }
    pub fn total_value(&self) -> u16 {
        bcd::value(&self.total)
    }
    pub fn high_score(&self) -> Bcd16 {
        self.hi_score
    }
    /// Course sections completed, out of [`COURSE_SECTIONS`].
    pub fn section(&self) -> u8 {
        self.distance[0]
    }
    /// Speed in pixels per frame, for a status line.
    pub fn speed_pixels(&self) -> f32 {
        self.speed as f32 / 256.0
    }
    pub fn state(&self) -> PikachuState {
        self.state
    }
    /// True once the run has ended and the results are being tallied.
    pub fn in_results(&self) -> bool {
        self.routine >= 3 && self.routine < ROUTINE_GAME_OVER
    }
    pub fn is_game_over(&self) -> bool {
        self.game_over
    }
    /// The board's current angle as a frame set, 1..=14. Sets above 7 are
    /// upside down.
    pub fn board_frame_set(&self) -> u8 {
        self.pika.frame_set
    }
    /// Which routine of the jump table is running, for probes and tests.
    pub fn routine(&self) -> u8 {
        self.routine
    }
    /// Whether the results screen has replaced the course.
    pub fn showing_results_screen(&self) -> bool {
        self.results_map.is_some()
    }
    /// Frames simulated since the run began.
    pub fn frame_count(&self) -> u64 {
        self.frame
    }
    pub fn beat_high_score(&self) -> bool {
        self.got_high_score
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Play a whole run with a fixed policy, returning the finished machine.
    fn play(seed: u32, spin_airborne: bool) -> Game {
        let mut g = Game::new(seed);
        for f in 0..60_000u64 {
            let airborne = spin_airborne && g.state == PikachuState::Jumping;
            let confirm = (g.in_results() || g.game_over) && f % 8 == 0;
            if g.step(Input {
                left: airborne,
                a: confirm,
                ..Default::default()
            }) == Status::Finished
            {
                break;
            }
        }
        g
    }

    // ---- the course -------------------------------------------------------

    #[test]
    fn the_same_seed_plays_the_same_course() {
        let (mut a, mut b) = (Game::new(0xbeef), Game::new(0xbeef));
        for f in 0..6_000 {
            a.step(Input::default());
            b.step(Input::default());
            if f % 500 == 0 {
                assert_eq!(a.wave_height, b.wave_height, "frame {f}: wave differs");
                assert_eq!(a.bgmap, b.bgmap, "frame {f}: map differs");
                assert_eq!(a.distance, b.distance, "frame {f}: distance differs");
                assert_eq!(a.hp, b.hp, "frame {f}: hp differs");
            }
        }
    }

    #[test]
    fn a_different_seed_plays_a_different_course() {
        let (mut a, mut b) = (Game::new(1), Game::new(2));
        for _ in 0..4_000 {
            a.step(Input::default());
            b.step(Input::default());
        }
        assert_ne!(a.bgmap, b.bgmap, "two seeds produced the same course");
    }

    #[test]
    fn the_course_is_built_of_real_waves_and_not_just_flat_water() {
        // A wave generator that silently degraded to open water would still
        // look plausible in a screenshot and would never launch a jump.
        let mut g = Game::new(17);
        let mut crests = 0;
        let mut heights = std::collections::BTreeSet::new();
        for _ in 0..6_000 {
            g.step(Input::default());
            heights.insert(g.wave_height[8]);
            if g.bgmap.contains(&tile::WAVE_CREST) {
                crests += 1;
            }
        }
        assert!(crests > 100, "only {crests} frames had a crest on the map");
        assert!(
            heights.len() > 4,
            "the surface only ever took {} heights",
            heights.len()
        );
    }

    // ---- the board --------------------------------------------------------

    #[test]
    fn the_board_angle_rocks_between_level_and_two() {
        let mut g = Game::new(1);
        let mut seen = std::collections::BTreeSet::new();
        let mut order = Vec::new();
        for _ in 0..200 {
            g.update_board_angle();
            if order.last() != Some(&g.board_angle) {
                order.push(g.board_angle);
            }
            seen.insert(g.board_angle);
        }
        assert_eq!(seen, [0, 1, 2].into_iter().collect());
        // It must sweep, not jump: 0,1,2,1,0,1,2,...
        for pair in order.windows(2) {
            assert_eq!(pair[0].abs_diff(pair[1]), 1, "angle jumped: {order:?}");
        }
    }

    #[test]
    fn the_board_angle_only_moves_every_eighth_call() {
        // The timer is read *before* it is incremented, so the very first
        // call acts and the next seven do not.
        let mut g = Game::new(1);
        let start = g.board_angle;
        g.update_board_angle();
        let moved = g.board_angle;
        assert_ne!(moved, start, "the first call should move the angle");
        for call in 0..7 {
            g.update_board_angle();
            assert_eq!(g.board_angle, moved, "call {} should be a no-op", call + 2);
        }
        g.update_board_angle();
        assert_ne!(g.board_angle, moved, "the ninth call should move it again");
    }

    #[test]
    fn the_frame_set_follows_the_slope_under_the_board() {
        let mut g = Game::new(1);
        g.scx = 3; // a tile transition, or nothing updates
        g.board_angle = 1;
        g.bg_read = [tile::RISING_SLOPE, 0];
        g.update_surfing_frame();
        assert_eq!(g.pika.frame_set, 6, "a rising slope wants a nose-up board");
        g.bg_read = [tile::FALLING_SLOPE, 0];
        g.update_surfing_frame();
        assert_eq!(
            g.pika.frame_set, 2,
            "a falling slope wants a nose-down board"
        );
        g.bg_read = [0x0b, 0];
        g.update_surfing_frame();
        assert_eq!(g.pika.frame_set, 4, "flat water wants a level board");
    }

    #[test]
    fn nothing_changes_away_from_a_tile_transition() {
        let mut g = Game::new(1);
        g.pika.frame_set = 9;
        for scx in [0u8, 1, 2, 5, 6, 7] {
            g.scx = scx;
            g.bg_read = [tile::RISING_SLOPE, 0];
            g.update_surfing_frame();
            assert_eq!(g.pika.frame_set, 9, "scx {scx} should not be a transition");
        }
    }

    // ---- jumping and crashing ---------------------------------------------

    #[test]
    fn a_jump_needs_a_crest_and_enough_speed() {
        let mut g = Game::new(1);
        g.scx = 3;
        g.speed = fixed::from_parts(2, 0);
        g.bg_read = [tile::WAVE_CREST, 0];
        assert!(g.try_start_jump(), "a crest at full speed should launch");
        assert_eq!(g.jump_magnitude, 16, "magnitude is speed over 32");

        // Same crest, too slow: 1.25 px/frame is the floor.
        g.speed = fixed::from_parts(1, 0x3f);
        assert!(
            !g.try_start_jump(),
            "1.25 is the threshold and this is under it"
        );
        g.speed = fixed::from_parts(1, 0x40);
        assert!(g.try_start_jump(), "exactly 1.25 should launch");

        // Fast, but not on a crest.
        g.speed = fixed::from_parts(2, 0);
        for t in [
            0x0b,
            tile::RISING_SLOPE,
            tile::FALLING_SLOPE,
            tile::WAVE_FACE,
        ] {
            g.bg_read = [t, 0];
            assert!(!g.try_start_jump(), "tile {t:#04x} is not a crest");
        }
    }

    #[test]
    fn the_jump_arc_rises_and_comes_back_down() {
        let mut g = Game::new(1);
        g.pikachu_height = FLAT_WATER_Y;
        g.pika.y = FLAT_WATER_Y;
        g.jump_magnitude = 16;
        g.jump_fraction = 0;
        g.jump_descending = false;
        let mut highest = FLAT_WATER_Y;
        let mut frames = 0;
        loop {
            let landed = g.update_pikachu_height();
            highest = highest.min(g.pika.y);
            frames += 1;
            assert!(frames < 400, "the jump never landed");
            if landed {
                break;
            }
        }
        assert!(
            FLAT_WATER_Y - highest > 20,
            "a full-speed jump only rose {} px",
            FLAT_WATER_Y - highest
        );
        assert_eq!(g.pika.y, FLAT_WATER_Y, "landing snaps to the surface");
        assert_eq!(g.pika.field_c, 0);
        assert!(
            (40..120).contains(&frames),
            "a full-speed jump lasted {frames} frames"
        );
    }

    #[test]
    fn a_bigger_jump_comes_from_more_speed() {
        let rise = |magnitude: u8| {
            let mut g = Game::new(1);
            g.pikachu_height = FLAT_WATER_Y;
            g.pika.y = FLAT_WATER_Y;
            g.jump_magnitude = magnitude;
            let mut highest = FLAT_WATER_Y;
            for _ in 0..400 {
                if g.update_pikachu_height() {
                    break;
                }
                highest = highest.min(g.pika.y);
            }
            FLAT_WATER_Y - highest
        };
        assert!(rise(16) > rise(12), "16 must out-jump 12");
        assert!(rise(12) > rise(10), "12 must out-jump the minimum");
    }

    #[test]
    fn the_landing_table_matches_the_disassembly() {
        use Landing::{Clean, Hard, Rough, Wipeout};
        // `SurfingMinigame_TileInteraction`, all four arms, frame sets 1..=7.
        let cases: [(u8, [Landing; 7]); 4] = [
            (0x0b, [Wipeout, Hard, Rough, Clean, Rough, Hard, Wipeout]),
            (
                tile::RISING_SLOPE,
                [Wipeout, Wipeout, Wipeout, Hard, Rough, Clean, Rough],
            ),
            (
                tile::FALLING_SLOPE,
                [Rough, Clean, Rough, Hard, Wipeout, Wipeout, Wipeout],
            ),
            (
                tile::WAVE_CREST,
                [Wipeout, Hard, Rough, Clean, Clean, Rough, Hard],
            ),
        ];
        for (under, expected) in cases {
            for (i, want) in expected.iter().enumerate() {
                let set = i as u8 + 1;
                assert_eq!(
                    Game::landing_for(under, set),
                    *want,
                    "tile {under:#04x} frame set {set}"
                );
            }
        }
        // The wave face shares the crest's row.
        for set in 1..=7u8 {
            assert_eq!(
                Game::landing_for(tile::WAVE_FACE, set),
                Game::landing_for(tile::WAVE_CREST, set)
            );
        }
    }

    #[test]
    fn landing_upside_down_is_always_a_wipeout() {
        // Frame sets 8..=14 are 1..=7 flipped both ways. Coming down on one
        // is the risk that makes spinning a gamble, and it is what the
        // scoring is paying for.
        for under in [
            0x0b,
            tile::RISING_SLOPE,
            tile::FALLING_SLOPE,
            tile::WAVE_CREST,
            tile::WAVE_FACE,
        ] {
            for set in 8..=14u8 {
                assert_eq!(
                    Game::landing_for(under, set),
                    Landing::Wipeout,
                    "{under:#04x}/{set}"
                );
            }
        }
    }

    #[test]
    fn a_wipeout_resets_speed_and_a_soft_landing_only_trims_it() {
        let mut g = Game::new(1);
        g.speed = fixed::from_parts(2, 0);
        g.bg_read = [0x0b, 0];
        g.pika.frame_set = 1; // wipeout on flat water
        assert_eq!(g.tile_interaction(), Landing::Wipeout);
        assert_eq!(g.speed, fixed::START_SPEED);

        g.speed = fixed::from_parts(2, 0);
        g.pika.frame_set = 4; // clean
        assert_eq!(g.tile_interaction(), Landing::Clean);
        assert_eq!(g.speed, fixed::from_parts(2, 0));

        g.pika.frame_set = 3; // rough: a quarter off
        assert_eq!(g.tile_interaction(), Landing::Rough);
        assert_eq!(g.speed, fixed::from_parts(1, 0xc0));

        g.pika.frame_set = 2; // hard: a half off
        assert_eq!(g.tile_interaction(), Landing::Hard);
        assert_eq!(g.speed, fixed::from_parts(1, 0x40));
    }

    #[test]
    fn speed_never_underflows_past_zero() {
        let mut g = Game::new(1);
        for step in [fixed::SPEED_STEP_64, fixed::SPEED_STEP_128] {
            g.speed = 0;
            g.reduce_speed(step);
            assert_eq!(
                g.speed, 0,
                "reducing zero speed must not wrap to 255 px/frame"
            );
            g.speed = step - 1;
            g.reduce_speed(step);
            assert_eq!(g.speed, 0);
        }
    }

    #[test]
    fn speed_climbs_to_two_pixels_a_frame_and_stops() {
        let mut g = Game::new(1);
        g.speed = fixed::START_SPEED;
        for _ in 0..10_000 {
            g.speed_up_pikachu();
        }
        assert_eq!(fixed::whole(g.speed), 2, "speed should cap at 2.0 px/frame");
        assert!(g.speed <= fixed::from_parts(2, 0x01));
    }

    // ---- scoring ----------------------------------------------------------

    #[test]
    fn the_scoring_table_matches_its_own_comment() {
        // "Single flip: +0050 / 2 of the same: +0150 / 3 or more of the same:
        // +0350 / 2 different: +0180 / 3 or more different: +0500"
        assert_eq!(Game::stunt_value(0, 0), 0, "no spin, no points");
        assert_eq!(Game::stunt_value(1, 0b01), 50);
        assert_eq!(Game::stunt_value(2, 0b01), 150);
        assert_eq!(Game::stunt_value(3, 0b01), 350);
        assert_eq!(Game::stunt_value(1, 0b10), 50);
        assert_eq!(Game::stunt_value(2, 0b10), 150);
        assert_eq!(Game::stunt_value(3, 0b10), 350);
        assert_eq!(Game::stunt_value(2, 0b11), 180, "two different flips");
        assert_eq!(Game::stunt_value(3, 0b11), 500, "three or more different");
        // One spin cannot have set both flags, but the meter is capped at 3
        // and the code checks the flags first, so pin the corner anyway.
        assert_eq!(Game::stunt_value(1, 0b11), 180);
    }

    #[test]
    fn the_radness_meter_caps_at_three() {
        let mut g = Game::new(1);
        for _ in 0..10 {
            g.start_trick();
        }
        assert_eq!(g.radness_meter, 3);
    }

    #[test]
    fn radness_is_added_in_lots_of_fifty_and_saturates() {
        let mut g = Game::new(1);
        g.radness_meter = 3;
        g.trick_flags = 0b11;
        g.calculate_and_add_radness_from_stunt();
        assert_eq!(bcd::value(&g.radness), 500);
        for _ in 0..30 {
            g.radness_meter = 3;
            g.trick_flags = 0b11;
            g.calculate_and_add_radness_from_stunt();
        }
        assert_eq!(
            bcd::value(&g.radness),
            9999,
            "the score must saturate, not wrap"
        );
    }

    #[test]
    fn a_stunt_with_no_spin_scores_nothing_and_pops_nothing() {
        let mut g = Game::new(1);
        g.radness_meter = 0;
        g.trick_flags = 0b11;
        g.calculate_and_add_radness_from_stunt();
        assert_eq!(bcd::value(&g.radness), 0);
        assert!(g.popups.is_empty());
    }

    // ---- HP, the goal and the end ----------------------------------------

    #[test]
    fn hp_is_a_clock_that_ticks_once_a_frame_while_running() {
        let mut g = Game::new(1);
        g.step(Input::default()); // routine 0 spawns the banner
        let mut last = g.hp_value();
        for f in 0..500 {
            g.step(Input::default());
            let now = g.hp_value();
            assert_eq!(
                now,
                last - 1,
                "frame {f}: HP moved by {} not 1",
                last as i32 - now as i32
            );
            last = now;
        }
    }

    #[test]
    fn hp_reaches_zero_after_exactly_six_thousand_ticks() {
        let mut g = Game::new(1);
        for tick in 0..6_000 {
            assert_ne!(g.hp, [0, 0], "HP ran out early at tick {tick}");
            g.deduct_1hp();
        }
        assert_eq!(g.hp, [0, 0]);
        assert_eq!(g.hp_value(), 0);
    }

    #[test]
    fn a_clean_run_reaches_the_goal_and_is_not_a_game_over() {
        for seed in [1u32, 7, 42, 1234] {
            let g = play(seed, false);
            assert!(
                !g.is_game_over(),
                "seed {seed}: ran out of HP without spinning once"
            );
            assert!(
                g.section() >= COURSE_SECTIONS,
                "seed {seed}: only reached section {}",
                g.section()
            );
            assert!(
                g.total_value() > 0,
                "seed {seed}: finished with no score at all"
            );
        }
    }

    #[test]
    fn running_out_of_hp_is_a_game_over_and_scores_nothing() {
        // Spinning constantly wipes out constantly, which costs the 96-frame
        // crash and a speed reset each time — enough to run the clock out.
        let g = play(1, true);
        assert!(
            g.is_game_over(),
            "constant spinning should have run the clock out"
        );
        assert!(g.section() < COURSE_SECTIONS);
        assert_eq!(g.total_value(), 0, "a game over never reaches the tally");
    }

    #[test]
    fn the_tally_turns_leftover_hp_and_radness_into_the_total() {
        let mut g = Game::new(7);
        g.hp = [0x50, 0x01]; // 150 frames left
        g.radness = [0x00, 0x02]; // 200 radness
        g.total = [0, 0];
        while !g.add_remaining_hp_to_total() {}
        assert_eq!(g.total_value(), 150);
        assert_eq!(g.hp_value(), 0);
        while !g.add_radness_to_total() {}
        assert_eq!(g.total_value(), 350);
        assert_eq!(g.radness_value(), 0);
    }

    #[test]
    fn beating_the_high_score_needs_more_than_matching_it() {
        // `DidPlayerGetAHighScore` falls through to the *not* branch when the
        // two are equal, which is easy to get backwards.
        let mut g = Game::new(1);
        g.hi_score = [0x00, 0x10];
        g.total = [0x00, 0x10];
        assert!(
            !g.did_player_get_a_high_score(),
            "an equal score is not a high score"
        );
        g.total = [0x01, 0x10];
        assert!(g.did_player_get_a_high_score());
        assert_eq!(g.hi_score, [0x01, 0x10], "a high score is recorded");
    }

    #[test]
    fn a_wipeout_costs_ninety_six_frames_in_the_water() {
        let mut g = Game::new(1);
        g.state = PikachuState::Crashed;
        g.crash_timer = CRASH_FRAMES;
        let mut frames = 0;
        while g.state == PikachuState::Crashed {
            g.update_pikachu_object();
            frames += 1;
            assert!(frames < 200);
        }
        assert_eq!(frames, CRASH_FRAMES as u32 + 1);
        assert_eq!(g.pika.frame_set, 4, "you come back up on a level board");
    }

    #[test]
    fn select_leaves_at_once() {
        let mut g = Game::new(1);
        assert_eq!(g.step(Input::default()), Status::Running);
        assert_eq!(
            g.step(Input {
                select: true,
                ..Default::default()
            }),
            Status::Finished
        );
    }

    #[test]
    fn the_tile_under_the_board_is_read_one_frame_late() {
        // `ReadBGMapBuffer` schedules a VBlank copy, so every decision the
        // game makes is against last frame's sample. Removing the lag would
        // change when jumps fire and what landings are worth.
        let mut g = Game::new(1);
        g.bgmap = [tile::WAVE_CREST; 32 * 32];
        g.bg_read = [0, 0];
        g.read_bg_map_buffer();
        assert_eq!(g.bg_read, [0, 0], "the sample must not be visible yet");
        g.step(Input::default());
        assert_eq!(
            g.bg_read,
            [tile::WAVE_CREST; 2],
            "and must land on the next frame"
        );
    }

    #[test]
    fn the_raster_buffer_scrolls_up_one_scanline_a_frame_below_the_sky() {
        let mut g = Game::new(1);
        let before = g.ly;
        g.update_ly_overrides();
        assert_eq!(g.ly[..16], before[..16], "the sky rows are held still");
        assert_eq!(g.ly[16..144], before[17..145], "the rest walks up one row");
    }
}
